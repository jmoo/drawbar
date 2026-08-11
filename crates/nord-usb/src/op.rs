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
            // The class is skipped, but the transaction still gets its closing
            // exchanges — an abandoned session strands the instrument on its progress
            // screen. A close that fails too is genuinely unrecoverable here.
            Err(_) => {
                let _ = session.commit().await;
            }
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

    let file = envelope::wrap(&meta.format, at, meta.version, &body)?;
    if let Some(expected) = meta.crc32 {
        let actual = envelope::crc32(&body);
        if expected != actual {
            return Err(Error::Envelope(format!(
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

/// Body bytes to ask for in one `READ`. A body larger than this arrives across several
/// requests with the offset advancing by exactly this much and a short final chunk.
///
/// Confirmed from captures: NSM asks for `32720`. Some objects are instead read at
/// `32726` throughout — a fixed 6-byte difference that is per object, not per chunk, and
/// unexplained. Both fit inside one `READ_BUFFER`, and the host chooses the number, so
/// the smaller is used uniformly.
const READ_CHUNK: u32 = 32720;

/// The shared read sequence NSM uses, reproduced byte-for-byte: `INFO` to learn the
/// body length, the `"Uploading..."` progress label the instrument paints, `BEGIN_READ`,
/// one `READ` per [`READ_CHUNK`] with the bar advancing as they arrive, then
/// `END_TRANSFER`. Returns the metadata and the reassembled body.
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

    // Capacity is clamped: `body_len` is device-supplied, and a corrupt or hostile
    // value must not become a gigabyte allocation up front. Real bodies larger than the
    // clamp (pianos) just grow the vector as chunks arrive.
    let mut body = Vec::with_capacity((meta.body_len as usize).min(1 << 20));
    let mut painted = None;
    while (body.len() as u32) < meta.body_len {
        let offset = body.len() as u32;
        let want = READ_CHUNK.min(meta.body_len - offset);

        let mut req = args.clone();
        req.extend_from_slice(&offset.to_be_bytes());
        req.extend_from_slice(&want.to_be_bytes());
        let resp = session
            .request(Service::Program, 10, cmd::READ, &req)
            .await?;

        // Payload is bank, slot, offset, length, then this chunk of the body.
        let p = resp.payload();
        let chunk = p.get(16..).ok_or(Error::Truncated {
            got: p.len(),
            need: 16,
        })?;
        // A short chunk would silently misalign every subsequent offset, so it is an
        // error rather than something to resynchronize from.
        if chunk.len() != want as usize {
            return Err(Error::Transport(format!(
                "asked for {want} bytes at offset {offset} but the device sent {}",
                chunk.len()
            )));
        }
        body.extend_from_slice(chunk);

        // The bar is bytes transferred over bytes expected, and only moves on a whole
        // percent — one message per step, the way NSM drives it. A body inside one chunk
        // therefore still produces exactly one `100`.
        let pct = (body.len() as u64 * 100 / (meta.body_len.max(1)) as u64) as u16;
        if painted != Some(pct) {
            session.notify(&ui::percent(pct)).await?;
            painted = Some(pct);
        }
    }

    // A zero-length body never enters the loop, so the bar would otherwise never be
    // cleared off the instrument's display.
    if painted != Some(100) {
        session.notify(&ui::percent(100)).await?;
    }
    session
        .request(Service::Program, 10, cmd::END_TRANSFER, &args)
        .await?;
    Ok((meta, body))
}

/// Write a `.ne5p` file into a slot. Requires a [`ReadWrite`] session, which callers
/// must obtain deliberately.
///
/// ⚠️ **The destination must be empty.** The device refuses to overwrite in place with
/// status `4` (confirmed on hardware; NSM grays out its write button for a filled slot,
/// so no capture of a replace exists). Replacing a slot is delete-then-write, and the
/// window in between — where the only copy is in host memory — belongs to the caller.
///
/// ⚠️ The `BEGIN_WRITE` argument layout is only partly understood: the fourth word is
/// a Unix timestamp (NSM sends the file's mtime) and the trailing bytes are copied
/// from an observed capture. The path is hardware-verified for programs (a field
/// edited in Rust, written over USB, confirmed on the panel), but only ever with
/// those captured trailing bytes. Back up before using it.
pub async fn write_program<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
    file: &[u8],
    timestamp: u32,
) -> Result<()> {
    let file = envelope::unwrap(file)?;
    let body = &file.body.0;

    // The "Downloading..." label the instrument paints — NSM's backwards word for
    // host → keyboard. Fire-and-forget, exactly as on the wire.
    session.notify(&ui::label("Downloading...")?).await?;

    let mut begin = Vec::new();
    at.write_to(&mut begin);
    begin.extend_from_slice(&(body.len() as u32).to_be_bytes());
    begin.extend_from_slice(&file.header.tag);
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
/// The next occupied slot after `at`, or `None` once the walk runs off the end.
///
/// **Read-only.** Positions inside a gap are safe to pass: the device answers with the
/// next real object rather than an error, which is what makes this an iterator over
/// content instead of over addresses.
pub async fn next_occupied<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Option<Location>> {
    let mut args = Vec::new();
    at.write_to(&mut args);
    match session
        .request(Service::Program, 10, cmd::NEXT_SLOT, &args)
        .await
    {
        Ok(resp) => {
            let p = resp.payload();
            if p.len() < 8 {
                return Err(Error::Truncated {
                    got: p.len(),
                    need: 8,
                });
            }
            Ok(Some(Location {
                bank: u32::from_be_bytes(p[0..4].try_into().unwrap()),
                slot: u32::from_be_bytes(p[4..8].try_into().unwrap()),
            }))
        }
        // Not a fault: the position asked about is past the end, which is how the walk
        // terminates. A refusal leaves the session in step, so the caller may continue.
        Err(Error::DeviceStatus(1)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Every occupied slot in the session's class, in address order.
///
/// **Read-only.** [`next_occupied`] walks *within* one bank and stops at its end, so this
/// drives it bank by bank. Pianos span several banks and programs fill eight of them;
/// only the sample library is flat, and walking bank 0 alone silently reports a fraction
/// of the class.
///
/// Bank 0 slot 0 of each bank is tested with [`info`], because the cursor reports what
/// comes *after* a position and can never say whether the first one holds anything.
/// That test also ends the walk: an out-of-range bank (status `3`) means the class has no
/// more banks, which is different from a bank that merely holds nothing (status `1`) —
/// the sample library has addressable empty banks past its only populated one.
///
/// Two bounds keep a walk finite when the device does not behave as expected: `cap` on
/// total slots, and a stop after [`EMPTY_BANKS_BEFORE_STOP`] consecutive empty banks for
/// classes that never report out-of-range at all.
pub async fn occupied_slots<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    cap: usize,
) -> Result<Vec<Location>> {
    let mut found: Vec<Location> = Vec::new();
    let mut empty_banks = 0;

    for bank in 0..MAX_BANKS {
        if found.len() >= cap {
            break;
        }
        let start = Location { bank, slot: 0 };
        let start_holds_something = match info(session, start).await {
            Ok(_) => true,
            Err(Error::DeviceStatus(1)) => false,
            Err(Error::DeviceStatus(3)) => break,
            Err(e) => return Err(e),
        };

        let before = found.len();
        if start_holds_something {
            found.push(start);
        }
        let mut at = start;
        while found.len() < cap {
            match next_occupied(session, at).await? {
                // Staying inside the bank and moving forward, or the walk is not making
                // progress and would spin.
                Some(next) if next.bank == bank && next.slot > at.slot => {
                    found.push(next);
                    at = next;
                }
                _ => break,
            }
        }

        if found.len() == before {
            empty_banks += 1;
            if empty_banks >= EMPTY_BANKS_BEFORE_STOP {
                break;
            }
        } else {
            empty_banks = 0;
        }
    }
    Ok(found)
}

/// Highest bank number a walk will try. Programs use eight; nothing observed uses more.
const MAX_BANKS: u32 = 64;

/// How many consecutive empty banks end a walk, for classes whose banks stay addressable
/// past the last populated one instead of reporting out-of-range.
const EMPTY_BANKS_BEFORE_STOP: u32 = 2;

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
/// An occupied destination is **swapped, not overwritten**: its occupant ends up in the
/// source slot, byte-identical. Nothing is destroyed, and no delete-first step is needed
/// (unlike a write, which the device refuses into an occupied slot with status `0x4`).
/// Confirmed on hardware.
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
