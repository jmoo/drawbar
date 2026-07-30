//! Typed operations.
//!
//! Each is a single-item primitive that runs inside a [`Session`]; a caller batches by
//! opening one session and looping (which is exactly how NSM batches — the wrapper once,
//! the per-item unit repeated).
//!
//! # What is reproduced, and what is not
//!
//! These emit the command bytes NSM sends to *effect* an operation, including the
//! fire-and-forget progress strings that paint the instrument's own display
//! ([`ui::label`]/[`ui::percent`]). They deliberately omit the reads NSM issues purely
//! to repaint its **host-side browser** — the `INFO`/`DEPENDENCIES` refresh after a
//! copy, the `STATUS` counter re-read that closes each transaction, and the whole
//! bank-refresh transaction that follows a write. Those change nothing on the device;
//! reproducing a specific GUI's bookkeeping is not the library's job. Everything sent
//! here is verified byte-for-byte against the capture corpus.

use crate::envelope;
use crate::error::{Error, Result};
use crate::session::ReadWrite;
use crate::session::Session;
use crate::transport::Transport;
use crate::wire::{cmd, ui, Dependency, Location, ObjectClass, ProgramInfo, Service, Status};

/// Query the inventory for the class the session was opened with.
///
/// **Read-only.** It sends one request and reads counters back; nothing on the
/// instrument changes. That makes it the safe way to prove the whole stack works
/// against real hardware.
pub async fn status<T: Transport, C>(session: &mut Session<'_, T, C>) -> Result<Status> {
    let class = session.class();
    let resp = session
        .request(
            Service::Program,
            10,
            cmd::STATUS,
            &class.to_raw().to_be_bytes(),
        )
        .await?;
    Status::decode(class, &resp)
}

/// Query every class worth reporting, one transaction each.
///
/// Each class needs its own session because the class is fixed at `SESSION_OPEN`.
/// A class that errors is skipped rather than failing the sweep — instruments differ
/// in which classes they answer for.
pub async fn inventory<T: Transport>(transport: &mut T) -> Result<Vec<Status>> {
    let mut out = Vec::new();
    for class in ObjectClass::INVENTORY {
        let mut session = match Session::open(transport, class).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        match status(&mut session).await {
            Ok(s) => {
                session.commit().await?;
                out.push(s);
            }
            Err(_) => session.abort(),
        }
    }
    Ok(out)
}

/// Ask the device about one slot: format tag, body length, name, body checksum.
///
/// **Read-only.**
pub async fn info<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<ProgramInfo> {
    let mut args = Vec::new();
    at.write_to(&mut args);
    let resp = session
        .request(Service::Program, 10, cmd::INFO, &args)
        .await?;
    ProgramInfo::decode(&resp)
}

/// Read one program off the instrument, returning the bytes of a `.ne5p` file.
///
/// **Read-only.** The body is wrapped in a `CBIN` header ([`envelope`]) so the result
/// is a real file, and the device's own CRC-32 is checked against it when the device
/// supplies one.
pub async fn read_program<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Vec<u8>> {
    let (meta, body) = transfer_out(session, at).await?;

    let file = envelope::wrap(&meta.format, at, &body)?;
    if let Some(expected) = meta.crc32 {
        let (_, _, wrapped) = envelope::unwrap(&file)?;
        let actual = crc32_of(wrapped);
        if expected != actual {
            return Err(Error::Transport(format!(
                "body checksum mismatch: device reported {expected:08x}, received {actual:08x}"
            )));
        }
    }
    Ok(file)
}

/// Read an entity's body off the instrument **without** wrapping it in a CBIN header.
///
/// For formats whose header layout is not yet known — notably CBIN **type-0**, the
/// legacy no-CRC variant — wrapping would fabricate a header rather than reproduce one.
/// This returns exactly the bytes the device sent, which is the safe thing to archive.
pub async fn read_body<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Vec<u8>> {
    Ok(transfer_out(session, at).await?.1)
}

/// The shared read sequence NSM uses, reproduced byte-for-byte: `INFO` to learn the
/// body length, the `"Uploading..."` progress label the instrument paints, `BEGIN_READ`,
/// `READ`, the 100% bar, then `END_TRANSFER`. Returns the metadata and the raw body.
///
/// ("Uploading" is NSM's own — and backwards — word for keyboard → host.)
async fn transfer_out<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<(ProgramInfo, Vec<u8>)> {
    let meta = info(session, at).await?;

    session.notify(&ui::label("Uploading...")?).await?;

    let mut args = Vec::new();
    at.write_to(&mut args);
    session
        .request(Service::Program, 10, cmd::BEGIN_READ, &args)
        .await?;

    let mut req = args.clone();
    req.extend_from_slice(&0u32.to_be_bytes()); // offset
    req.extend_from_slice(&meta.body_len.to_be_bytes());
    let resp = session
        .request(Service::Program, 10, cmd::READ, &req)
        .await?;

    // Payload is bank, slot, offset, length, then the body.
    let p = resp.payload();
    let body = p
        .get(16..)
        .ok_or(Error::Truncated {
            got: p.len(),
            need: 16,
        })?
        .to_vec();
    if body.len() != meta.body_len as usize {
        return Err(Error::Transport(format!(
            "device announced a {}-byte body but sent {}",
            meta.body_len,
            body.len()
        )));
    }

    session.notify(&ui::percent(100)).await?;
    session
        .request(Service::Program, 10, cmd::END_TRANSFER, &args)
        .await?;
    Ok((meta, body))
}

/// Write a `.ne5p` file into a slot, **overwriting whatever is there**.
///
/// Requires a [`ReadWrite`] session, which callers must obtain deliberately.
///
/// ⚠️ The `BEGIN_WRITE` argument layout is only partly understood: the fourth word is
/// a Unix timestamp (NSM sends the file's mtime) and the trailing bytes are copied
/// from an observed capture. This has been validated against a recorded exchange but
/// **not yet against real hardware**. Back up before using it.
pub async fn write_program<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
    file: &[u8],
    timestamp: u32,
) -> Result<()> {
    let (format, _, body) = envelope::unwrap(file)?;

    // The "Downloading..." label the instrument paints — NSM's backwards word for
    // host → keyboard. Fire-and-forget, exactly as on the wire.
    session.notify(&ui::label("Downloading...")?).await?;

    let mut begin = Vec::new();
    at.write_to(&mut begin);
    begin.extend_from_slice(&(body.len() as u32).to_be_bytes());
    begin.extend_from_slice(format.as_bytes());
    begin.extend_from_slice(&timestamp.to_be_bytes());
    begin.extend_from_slice(&u32::MAX.to_be_bytes());
    begin.extend_from_slice(&1u32.to_be_bytes());
    begin.push(b'0'); // trailing flag byte, copied from the observed capture
    session
        .request(Service::Program, 10, cmd::BEGIN_WRITE, &begin)
        .await?;

    let mut data = Vec::new();
    at.write_to(&mut data);
    data.extend_from_slice(&0u32.to_be_bytes()); // offset
    data.extend_from_slice(&(body.len() as u32).to_be_bytes());
    data.extend_from_slice(body);
    session
        .request(Service::Program, 10, cmd::WRITE_DATA, &data)
        .await?;

    session.notify(&ui::percent(100)).await?;

    let mut args = Vec::new();
    at.write_to(&mut args);
    session
        .request(Service::Program, 10, cmd::END_TRANSFER, &args)
        .await?;
    Ok(())
}

/// Load a stored object live on the instrument ("open on device" / double-click in
/// NSM). The device switches to it immediately.
///
/// **Non-destructive** — nothing stored changes, so this needs no [`ReadWrite`] session.
/// This is the one command with inverted parity (`0x2f` request, `0x30` response).
pub async fn select<T: Transport, C>(session: &mut Session<'_, T, C>, at: Location) -> Result<()> {
    let mut args = Vec::new();
    at.write_to(&mut args);
    session
        .request(Service::Program, 10, cmd::SELECT, &args)
        .await?;
    Ok(())
}

/// List the piano/sample library objects an entity depends on.
///
/// **Read-only.** The returned [`Dependency`] ids match the ids the objects carry in
/// their own files, which is the bridge between wire content and file bytes.
pub async fn dependencies<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Vec<Dependency>> {
    let mut args = Vec::new();
    at.write_to(&mut args);
    let resp = session
        .request(Service::Program, 10, cmd::DEPENDENCIES, &args)
        .await?;
    Dependency::decode_all(&resp)
}

/// Move an object from one slot to another. The device relocates it internally — no
/// body crosses the wire.
///
/// Requires a [`ReadWrite`] session. Class-generalised: works for whichever object
/// class the session opened (programs, set lists).
pub async fn move_object<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    from: Location,
    to: Location,
) -> Result<()> {
    let mut args = Vec::new();
    from.write_to(&mut args);
    to.write_to(&mut args);
    session
        .request(Service::Program, 10, cmd::MOVE, &args)
        .await?;
    Ok(())
}

/// Delete the object in a slot. Requires a [`ReadWrite`] session.
///
/// Sends the `"Deleting..."` progress label the instrument paints, then the delete —
/// exactly the two OUT frames NSM sends (the `O36 O26 I30` shape).
pub async fn delete<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
) -> Result<()> {
    session.notify(&ui::label("Deleting...")?).await?;
    let mut args = Vec::new();
    at.write_to(&mut args);
    session
        .request(Service::Program, 10, cmd::DELETE, &args)
        .await?;
    Ok(())
}

/// Rename the object in a slot. Requires a [`ReadWrite`] session.
///
/// The name is sent big-endian length-prefixed and unpadded — the same encoding
/// strings use everywhere on the wire.
pub async fn rename<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
    name: &str,
) -> Result<()> {
    let mut args = Vec::new();
    at.write_to(&mut args);
    args.extend_from_slice(&(name.len() as u32).to_be_bytes());
    args.extend_from_slice(name.as_bytes());
    session
        .request(Service::Program, 10, cmd::RENAME, &args)
        .await?;
    Ok(())
}

/// Duplicate the object at `from` into `to`. Requires a [`ReadWrite`] session.
///
/// A deep copy the device performs internally: the arguments are just the two
/// addresses, and no body crosses the wire. (NSM follows a copy with `INFO`/`DEPENDENCIES`
/// reads to repaint its browser; those are UI bookkeeping and are not sent here — see
/// the module-level note.)
pub async fn duplicate<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    from: Location,
    to: Location,
) -> Result<()> {
    let mut args = Vec::new();
    from.write_to(&mut args);
    to.write_to(&mut args);
    session
        .request(Service::Program, 10, cmd::COPY, &args)
        .await?;
    Ok(())
}

fn crc32_of(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
