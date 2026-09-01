//! Typed operations.
//!
//! Each primitive runs inside a [`Session`]; callers can batch by opening one
//! session and applying the primitive repeatedly. Operations include device-side
//! progress messages but omit reads used only to refresh a host UI.

use crate::envelope;
use crate::error::{Error, Result};
use crate::session::ReadWrite;
use crate::session::Session;
use crate::transport::Transport;
use crate::wire::{
    cmd, ui, Bank, Dependency, Location, Message, ObjectClass, Partition, ProgramInfo, Service,
    Status,
};

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
            // A skipped class still owes the instrument its closing exchanges.
            Err(_) => {
                session.commit().await?;
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
    let info = ProgramInfo::decode(&resp)?;
    if info.location != at {
        return Err(Error::UnexpectedLocation {
            requested: at,
            reported: info.location,
        });
    }
    Ok(info)
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

/// Body bytes per `WRITE_DATA` frame. The whole frame must stay under the device's
/// max transfer; an oversized frame wedges the instrument until a power cycle.
const WRITE_CHUNK: usize = 32720;

/// Fault-injection overrides; absent variables keep captured sizes, invalid values fail.
#[cfg(any(feature = "fault-injection", test))]
fn parse_chunk(name: &str, value: Option<&str>, default: u64) -> Result<u64> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .parse()
        .ok()
        .filter(|&size| size > 0)
        .ok_or_else(|| Error::InvalidArgument(format!("{name} must be a positive integer")))
}

#[cfg(feature = "fault-injection")]
fn chunk_override(name: &str, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Ok(value) => parse_chunk(name, Some(&value), default),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(Error::InvalidArgument(format!("{name} must be UTF-8")))
        }
    }
}

#[cfg(feature = "fault-injection")]
fn read_chunk() -> Result<u32> {
    let size = chunk_override("NORD_READ_CHUNK", READ_CHUNK.into())?;
    u32::try_from(size).map_err(|_| Error::InvalidArgument("NORD_READ_CHUNK exceeds u32".into()))
}
#[cfg(not(feature = "fault-injection"))]
fn read_chunk() -> Result<u32> {
    Ok(READ_CHUNK)
}

#[cfg(feature = "fault-injection")]
fn write_chunk() -> Result<usize> {
    let size = chunk_override("NORD_WRITE_CHUNK", WRITE_CHUNK as u64)?;
    usize::try_from(size)
        .map_err(|_| Error::InvalidArgument("NORD_WRITE_CHUNK exceeds usize".into()))
}
#[cfg(not(feature = "fault-injection"))]
fn write_chunk() -> Result<usize> {
    Ok(WRITE_CHUNK)
}

/// Bytes per storage block in a library partition — the unit `STATUS`'s free/used
/// words count there. Per class: the piano partition is 4096 × 256 KiB (1 GB), the
/// sample partition 2048 × 128 KiB (256 MB). Undercounting `needed` here makes
/// `BEGIN_WRITE` refuse `0x16` even right after the cleaning pass.
///
/// These are the **gross** blocks. The device reports the same geometry itself, net of
/// per-block overhead, as [`Partition::allocation_unit`] — 261,632 for pianos, 131,064
/// for samples — and the two size a write identically except for a body landing within
/// that overhead of an exact block boundary, where the net number needs one block more.
/// Reading it per write would put a `PARTITIONS` exchange inside the write session,
/// which nothing on the wire has ever done; the equivalence is asserted in the tests
/// against a recorded partition table instead.
fn library_block(class: ObjectClass) -> usize {
    match class {
        ObjectClass::Sample => 131_072,
        _ => 262_144,
    }
}

/// Read the metadata and body through the device's chunked transfer sequence.
async fn transfer_out<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<(ProgramInfo, Vec<u8>)> {
    let chunk_size = read_chunk()?;
    let meta = info(session, at).await?;

    session.notify(&ui::label("Uploading...")?).await?;

    let mut args = Vec::new();
    at.write_to(&mut args);
    session
        .request(Service::Program, 10, cmd::BEGIN_READ, &args)
        .await?;

    // Clamp allocation from the device-supplied length; large valid bodies grow by chunk.
    let mut body = Vec::with_capacity((meta.body_len as usize).min(1 << 20));
    let mut painted = None;
    while (body.len() as u32) < meta.body_len {
        let offset = body.len() as u32;
        let want = chunk_size.min(meta.body_len - offset);

        let mut req = args.clone();
        req.extend_from_slice(&offset.to_be_bytes());
        req.extend_from_slice(&want.to_be_bytes());
        let resp = session
            .request(Service::Program, 10, cmd::READ, &req)
            .await?;

        let chunk = read_payload(resp.payload(), at, offset, want)?;
        body.extend_from_slice(chunk);

        // Progress moves only at whole percentages.
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

fn read_payload(payload: &[u8], at: Location, offset: u32, length: u32) -> Result<&[u8]> {
    if payload.len() < 16 {
        return Err(Error::Truncated {
            got: payload.len(),
            need: 16,
        });
    }
    let word = |start| u32::from_be_bytes(payload[start..start + 4].try_into().unwrap());
    let echoed = (word(0), word(4), word(8), word(12));
    let expected = (at.bank, at.slot, offset, length);
    if echoed != expected {
        return Err(Error::Transport(format!(
            "READ response echoed {echoed:?}, expected {expected:?}"
        )));
    }
    let body = &payload[16..];
    if body.len() != length as usize {
        return Err(Error::Transport(format!(
            "asked for {length} bytes at offset {offset} but the device sent {}",
            body.len()
        )));
    }
    Ok(body)
}

/// Bound on polling `0x26` for the cleaning pass, which normally finishes within a
/// second; the headroom is for a heavily churned library.
const CLEANING_POLLS: u32 = 120;
const CLEANING_POLL_SPACING: std::time::Duration = std::time::Duration::from_millis(250);

/// Reclaim `blocks` of library space and wait for the pass to finish ("Cleaning..."
/// on the display). Writing before it finishes is refused `0x1e`.
async fn clean_library<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    blocks: u32,
) -> Result<()> {
    session.notify(&ui::label("Cleaning...")?).await?;
    session.notify(&ui::percent(0)).await?;
    session
        .request(
            Service::Program,
            10,
            cmd::WRITE_PREPARE,
            &blocks.to_be_bytes(),
        )
        .await?;

    let mut painted = Some(0);
    for polls in 0..CLEANING_POLLS {
        if polls > 0 {
            crate::sleep::sleep(CLEANING_POLL_SPACING).await;
        }
        let resp = session
            .request(Service::Program, 10, cmd::WRITE_PREPARE_2, &[])
            .await?;
        let p = resp.payload();
        if p.len() >= 12 {
            // Reply is `[requested, done, running]`. Ready is `running` returning to
            // 0; `done` can end above the request, so the bar is clamped.
            let requested = u32::from_be_bytes(p[0..4].try_into().unwrap());
            let done = u32::from_be_bytes(p[4..8].try_into().unwrap());
            let running = u32::from_be_bytes(p[8..12].try_into().unwrap());
            if running == 0 {
                if painted != Some(100) {
                    session.notify(&ui::percent(100)).await?;
                }
                return Ok(());
            }
            let pct = (done as u64 * 100 / requested.max(1) as u64).min(99) as u16;
            if painted != Some(pct) {
                session.notify(&ui::percent(pct)).await?;
                painted = Some(pct);
            }
        }
    }
    Err(Error::Transport(format!(
        "the library's cleaning pass did not report ready within {} polls",
        CLEANING_POLLS
    )))
}

/// Write an entity into a slot; one shape for every class. `name` is what the slot
/// ends up called — the file carries none, and a placeholder becomes the slot's name.
pub async fn write<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
    file: &[u8],
    name: &str,
    timestamp: u32,
) -> Result<()> {
    let file = envelope::unwrap(file)?;
    let body = &file.body.0;
    let chunk_size = write_chunk()?;
    let body_len = u32::try_from(body.len())
        .map_err(|_| Error::InvalidArgument("the body is larger than the wire format".into()))?;
    let name_len = u32::try_from(name.len())
        .map_err(|_| Error::InvalidArgument("the name is larger than the wire format".into()))?;

    if matches!(session.class(), ObjectClass::Piano | ObjectClass::Sample) {
        // A library write is refused 0x16 unless a prepared block exists per
        // storage block of body; reclaim exactly the shortfall.
        let needed = body.len().div_ceil(library_block(session.class())) as u32;
        let free = status(session).await?.free;
        if needed > free {
            clean_library(session, needed - free).await?;
        }
    }

    session.notify(&ui::label("Downloading...")?).await?;

    let mut begin = Vec::new();
    at.write_to(&mut begin);
    begin.extend_from_slice(&body_len.to_be_bytes());
    begin.extend_from_slice(&file.header.tag);
    begin.extend_from_slice(&timestamp.to_be_bytes());
    begin.extend_from_slice(&u32::MAX.to_be_bytes());
    begin.extend_from_slice(&name_len.to_be_bytes());
    begin.extend_from_slice(name.as_bytes());
    session
        .request(Service::Program, 10, cmd::BEGIN_WRITE, &begin)
        .await?;

    let mut offset = 0usize;
    let mut painted = None;
    while offset < body.len() {
        let end = offset.saturating_add(chunk_size).min(body.len());
        let chunk = &body[offset..end];
        let mut data = Vec::new();
        at.write_to(&mut data);
        data.extend_from_slice(&(offset as u32).to_be_bytes());
        data.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
        data.extend_from_slice(chunk);
        if end == body.len() {
            // Only the final chunk is acknowledged.
            session
                .request(Service::Program, 10, cmd::WRITE_DATA, &data)
                .await?;
        } else {
            let msg = Message::new(Service::Program, 10, cmd::WRITE_DATA, data);
            session.notify(&msg).await?;
        }
        offset = end;

        let pct = (offset as u64 * 100 / (body.len().max(1)) as u64) as u16;
        if painted != Some(pct) {
            session.notify(&ui::percent(pct)).await?;
            painted = Some(pct);
        }
    }

    if painted != Some(100) {
        session.notify(&ui::percent(100)).await?;
    }

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

/// Drain queued replies until the transport stays quiet.
async fn drain<T: Transport>(transport: &mut T) -> Result<()> {
    for _ in 0..DRAIN_CAP {
        match transport
            .read_timeout(crate::transport::READ_BUFFER, DRAIN_LIMIT)
            .await?
        {
            Some(_) => continue,
            None => break,
        }
    }
    Ok(())
}

/// How long to wait for a straggler before deciding the stream is quiet.
const DRAIN_LIMIT: std::time::Duration = std::time::Duration::from_millis(300);

/// Upper bound on stragglers, so a device that will not stop talking cannot hang this.
const DRAIN_CAP: usize = 16;

/// Release UI and class state left by an abandoned session.
///
/// A bare `GOODBYE` clears the UI state that makes every slot appear empty; a bare
/// `SESSION_CLOSE` clears class status `0x12`. Queued replies are drained first.
pub async fn recover<T: Transport>(transport: &mut T) -> Result<()> {
    // An unread reply leaves every subsequent request paired with its predecessor.
    drain(transport).await?;

    // ⚠️ Bounded reads: the instrument this is for is the one that has stopped
    // answering, and no reply to either frame is the expected outcome, not a failure.
    let goodbye = Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, Vec::new());
    transport.write(&goodbye.encode()).await?;
    let _ = transport
        .read_timeout(crate::transport::READ_BUFFER, DRAIN_LIMIT)
        .await?;

    let close = Message::new(Service::Program, 10, cmd::SESSION_CLOSE, Vec::new());
    transport.write(&close.encode()).await?;
    let _ = transport
        .read_timeout(crate::transport::READ_BUFFER, DRAIN_LIMIT)
        .await?;
    Ok(())
}

/// Every storage partition the device reports. **Read-only.**
///
/// The index of each entry is its object class code, so this is also the authoritative
/// answer to "what classes does this instrument have" — including the `(Native)` library
/// views that have no [`ObjectClass`] name.
pub async fn partitions<T: Transport, C>(
    session: &mut Session<'_, T, C>,
) -> Result<Vec<Partition>> {
    let resp = session
        .request(Service::Program, 10, cmd::PARTITIONS, &[])
        .await?;
    Partition::decode_all(&resp)
}

/// One partition's banks and their slot capacities. **Read-only.**
pub async fn banks<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    partition: u32,
) -> Result<Vec<Bank>> {
    let resp = session
        .request(Service::Program, 10, cmd::BANKS, &partition.to_be_bytes())
        .await?;
    Bank::decode_all(&resp)
}

/// Whether an address exists on this instrument, per the device's own geometry.
///
/// **Read-only**, and the point is that it answers *before* anything is attempted: a write
/// to a bad address otherwise fails only once the transfer is under way, and a write to an
/// occupied one is refused with status `0x4` after the caller has committed to it.
///
/// `Ok(None)` means the address is fine. `Ok(Some(reason))` explains why it is not, in
/// terms of the bank names the instrument itself uses — which for pianos are categories,
/// so "no bank 7 (this class has 6: Grand, Upright, …)" is a far better error than a
/// status code.
pub async fn check_address<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Option<String>> {
    let banks = banks(session, session.class().to_raw()).await?;
    let Some(bank) = banks.get(at.bank as usize) else {
        let names: Vec<&str> = banks.iter().map(|b| b.name.as_str()).collect();
        return Ok(Some(format!(
            "bank {} does not exist; this class has {} ({})",
            at.user_bank(),
            banks.len(),
            names.join(", ")
        )));
    };
    // The `(Native)` partitions report a sentinel rather than a capacity, so there is
    // nothing to check against there.
    if bank.is_bounded() && at.slot >= bank.slots {
        return Ok(Some(format!(
            "\"{}\" holds {} slots, so slot {} is out of range",
            bank.name,
            bank.slots,
            at.user_slot()
        )));
    }
    Ok(None)
}

/// The object the panel currently has loaded, for the session's class. **Read-only.**
///
/// The read half of [`select`]: together they make the player's own position addressable.
pub async fn focus<T: Transport, C>(session: &mut Session<'_, T, C>) -> Result<Location> {
    let resp = session
        .request(Service::Program, 10, cmd::FOCUS, &[])
        .await?;
    let p = resp.payload();
    if p.len() < 8 {
        return Err(Error::Truncated {
            got: p.len(),
            need: 8,
        });
    }
    Ok(Location {
        bank: u32::from_be_bytes(p[0..4].try_into().unwrap()),
        slot: u32::from_be_bytes(p[4..8].try_into().unwrap()),
    })
}

/// Device status refusing a [`cmd::NEXT_SLOT`] without the direction word. Surfaced
/// rather than swallowed: a refused walk must not pass off a partial list.
pub const ENUMERATION_DISABLED: u32 = 0x11;

/// Slot value meaning "from the bank's boundary": the bank's first occupied slot when
/// walking forward, its last when walking backward.
pub const SLOT_BOUNDARY: u32 = 0xffff_ffff;

/// The next occupied slot after `at`, or `None` once the walk runs off the end.
///
/// **Read-only.** Positions inside a gap are safe to pass: the device answers with the
/// next real object rather than an error, which is what makes this an iterator over
/// content instead of over addresses. `at.slot == SLOT_BOUNDARY` starts from before
/// the bank's first slot.
pub async fn next_occupied<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Option<Location>> {
    let mut args = Vec::new();
    at.write_to(&mut args);
    // Direction, 0 = forward; omitting it is refused after any write since power-up.
    args.extend_from_slice(&0u32.to_be_bytes());
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
/// drives it bank by bank, each from [`SLOT_BOUNDARY`]. Pianos span several banks and
/// programs fill eight of them; only the sample library is flat, and walking bank 0
/// alone silently reports a fraction of the class.
///
/// Each bank's slot 0 is tested with [`info`] first, because the cursor cannot say
/// whether a bank *exists*: an empty bank and a bank the class does not have answer a
/// boundary request identically. `info` distinguishes — status `3` (out of range) means
/// the class has no more banks and ends the walk, status `1` a bank that merely holds
/// nothing — the sample library has addressable empty banks past its only populated one.
///
/// Two bounds keep a walk finite when the device does not behave as expected: `cap` on
/// total slots, and a stop after `EMPTY_BANKS_BEFORE_STOP` consecutive empty banks for
/// classes that never report out-of-range at all.
///
/// A refusal mid-walk — [`ENUMERATION_DISABLED`] above all — propagates as its error
/// rather than truncating the list: a partial inventory that looks complete is the one
/// result worse than none.
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
        match info(session, Location { bank, slot: 0 }).await {
            Ok(_) | Err(Error::DeviceStatus(1)) => {}
            Err(Error::DeviceStatus(3)) => break,
            Err(e) => return Err(e),
        }

        let before = found.len();
        let mut at = Location {
            bank,
            slot: SLOT_BOUNDARY,
        };
        while found.len() < cap {
            match next_occupied(session, at).await? {
                // Staying inside the bank and moving forward, or the walk is not making
                // progress and would spin.
                Some(next)
                    if next.bank == bank && (at.slot == SLOT_BOUNDARY || next.slot > at.slot) =>
                {
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

/// The library objects an entity actually needs. **Read-only.**
///
/// [`dependencies`] returns what the device reports, which includes rows that are not
/// dependencies at all — see [`Dependency::is_required`]. This is the one to build on;
/// reach for the unfiltered list only when the extra rows are themselves the subject.
pub async fn required_dependencies<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Vec<Dependency>> {
    Ok(dependencies(session, at)
        .await?
        .into_iter()
        .filter(Dependency::is_required)
        .collect())
}

/// List the piano/sample library objects an entity depends on, as the device reports
/// them — including rows that are not dependencies at all.
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
    let name_len = u32::try_from(name.len())
        .map_err(|_| Error::InvalidArgument("the name is larger than the wire format".into()))?;
    args.extend_from_slice(&name_len.to_be_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_chunk_must_echo_its_request() {
        let at = Location { bank: 2, slot: 3 };
        let mut payload = Vec::new();
        for word in [at.bank, at.slot, 40, 3] {
            payload.extend_from_slice(&word.to_be_bytes());
        }
        payload.extend_from_slice(&[1, 2, 3]);
        assert_eq!(read_payload(&payload, at, 40, 3).unwrap(), [1, 2, 3]);

        payload[3] ^= 1;
        assert!(read_payload(&payload, at, 40, 3).is_err());
    }

    /// The hardcoded block sizes are the device's own geometry, rounded up to the
    /// enclosing power of two.
    ///
    /// [`library_block`] cannot read [`Partition::allocation_unit`] per write without
    /// putting a `PARTITIONS` exchange somewhere no capture has one, so the equivalence
    /// is pinned here instead: if a firmware ever reports a different granularity, this
    /// fails rather than the write path silently miscounting blocks.
    #[cfg(feature = "replay")]
    #[test]
    fn the_block_constants_are_the_devices_own_granularity() {
        use crate::transport::{ReplayTransport, Script};

        let text = include_str!("../tests/scripts/device/geometry.script");
        let mut t = ReplayTransport::new(Script::parse(text).unwrap().steps());
        let parts = pollster::block_on(async {
            let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
            let r = partitions(&mut s).await;
            s.abort();
            r.unwrap()
        });

        for (class, gross) in [
            (ObjectClass::Piano, 262_144usize),
            (ObjectClass::Sample, 131_072),
        ] {
            let net = parts[class.to_raw() as usize].allocation_unit().unwrap() as usize;
            assert_eq!(library_block(class), gross);
            assert!(
                net <= gross && gross - net < 1024,
                "{class:?}: the device reports {net} where the write path uses {gross}"
            );
        }

        // A slot class is byte-granular, which is what says its counters are bytes.
        assert_eq!(
            parts[ObjectClass::Program.to_raw() as usize].allocation_unit(),
            Some(1)
        );
    }

    #[test]
    fn invalid_chunk_overrides_are_refused() {
        assert!(parse_chunk("NORD_READ_CHUNK", Some("0"), READ_CHUNK.into()).is_err());
        assert!(parse_chunk("NORD_WRITE_CHUNK", Some("bad"), WRITE_CHUNK as u64).is_err());
        assert_eq!(
            parse_chunk("NORD_READ_CHUNK", None, READ_CHUNK.into()).unwrap(),
            READ_CHUNK.into()
        );
    }
}
