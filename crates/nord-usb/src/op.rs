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

/// Body bytes per `WRITE_DATA` frame. NSM wrote a sample at `32726` and a piano at
/// `32720` — the same unexplained 6-byte pair the read side shows, per object. The
/// host appears to choose; the smaller is used uniformly, mirroring `READ_CHUNK`.
const WRITE_CHUNK: usize = 32720;

/// Bytes per storage block in the library partitions (piano, sample), the unit their
/// `STATUS` counters count. Measured 2026-08-20: deleting a 5,894,704-byte piano
/// dropped `used` by 23 (= ceil at 256KiB), and NSM's cleaning argument for that
/// upload was exactly `ceil(len/256KiB) - free`.
const LIBRARY_BLOCK: usize = 262_144;

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

/// How long to keep asking `0x26` whether the cleaning pass has finished. The pass took
/// under a second on hardware; the bound exists for a longer pass on a churned library,
/// not as an expected wait.
const CLEANING_POLLS: u32 = 120;
const CLEANING_POLL_SPACING: std::time::Duration = std::time::Duration::from_millis(250);

/// Reclaim `blocks` of library space and wait for the pass to finish. Paints
/// "Cleaning..." on the display, exactly as NSM does before an upload that needs room.
///
/// The library refuses a write it has no prepared room for — `BEGIN_WRITE` answers
/// `0x16` (the state a whole day of 2026-08-19/20 failures traced to) until a cleaning
/// pass has reclaimed enough blocks, **in the write's own session**. `0x22 ⟨n⟩` asks
/// for `n` blocks; `0x26` then reads `[requested, done, running]` — poll until
/// `running` returns to 0 (writing earlier is refused `0x1e`). `done` may end above
/// `requested`. ⚠️ A bare `0x26` with no `0x22` before it answers a stale-looking
/// ready triple while a write would still be refused, so it is only read here, after
/// the pass is started.
async fn clean_library<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    blocks: u32,
) -> Result<()> {
    session.notify(&ui::label("Cleaning...")?).await?;
    session.notify(&ui::percent(0)).await?;
    session
        .request(Service::Program, 10, cmd::WRITE_PREPARE, &blocks.to_be_bytes())
        .await?;

    for polls in 0..CLEANING_POLLS {
        // The pass runs on the instrument; give it room before asking again. The first
        // ask is immediate, which is the path a clean library takes.
        if polls > 0 {
            crate::deadline::with_timeout(std::future::pending::<()>(), CLEANING_POLL_SPACING)
                .await;
        }
        let resp = session
            .request(Service::Program, 10, cmd::WRITE_PREPARE_2, &[])
            .await?;
        let p = resp.payload();
        if p.len() >= 12 {
            // Ready is the third word — `running` — returning to 0. The middle word
            // (`done`) is not part of the test: it can end above the request.
            let running = u32::from_be_bytes(p[8..12].try_into().unwrap());
            if running == 0 {
                return Ok(());
            }
        }
    }
    Err(Error::Transport(format!(
        "the library's cleaning pass did not report ready within {} polls",
        CLEANING_POLLS
    )))
}

/// Write an entity into a slot. One shape for every class.
///
/// The object's **name** is an argument of the write: the file carries none, so
/// whatever is passed here is what the slot ends up called — a placeholder becomes the
/// slot's name. Hardware-verified on samples (2026-08-19) and programs (2026-08-20);
/// the nameless-looking program form NSM was captured sending is this same shape
/// carrying the one-byte name `"0"`, which is what a restored slot used to end up
/// called.
///
/// The body is pushed in [`WRITE_CHUNK`]-sized `WRITE_DATA` frames, exactly as NSM
/// sends them: every chunk but the last goes unacknowledged, and only the final one is
/// answered. ⚠️ A body over the device's maximum transfer **must** be chunked — a
/// single oversized frame leaves the instrument consuming everything that follows as
/// continuation bytes, silent on bulk IN until a power cycle (found the hard way,
/// 2026-08-20, with an 82KB sample).
///
/// A **library** class (piano, sample) is cleaned first — see [`clean_library`]: its
/// `BEGIN_WRITE` is refused with `0x16` whenever a write has dirtied the library since
/// its last cleaning pass, and the pass is what re-arms it.
pub async fn write<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
    file: &[u8],
    name: &str,
    timestamp: u32,
) -> Result<()> {
    let file = envelope::unwrap(file)?;
    let body = &file.body.0;

    if matches!(session.class(), ObjectClass::Piano | ObjectClass::Sample) {
        // A write needs one prepared block per LIBRARY_BLOCK of body. Reclaim exactly
        // the shortfall, as NSM does; with enough already free the pair is skipped
        // entirely, which is also NSM's behavior.
        let needed = body.len().div_ceil(LIBRARY_BLOCK) as u32;
        let free = status(session).await?.free;
        if needed > free {
            clean_library(session, needed - free).await?;
        }
    }

    session.notify(&ui::label("Downloading...")?).await?;

    let mut begin = Vec::new();
    at.write_to(&mut begin);
    begin.extend_from_slice(&(body.len() as u32).to_be_bytes());
    begin.extend_from_slice(&file.header.tag);
    begin.extend_from_slice(&timestamp.to_be_bytes());
    begin.extend_from_slice(&u32::MAX.to_be_bytes());
    begin.extend_from_slice(&(name.len() as u32).to_be_bytes());
    begin.extend_from_slice(name.as_bytes());
    session
        .request(Service::Program, 10, cmd::BEGIN_WRITE, &begin)
        .await?;

    let mut offset = 0usize;
    while offset < body.len() {
        let end = (offset + WRITE_CHUNK).min(body.len());
        let chunk = &body[offset..end];
        let mut data = Vec::new();
        at.write_to(&mut data);
        data.extend_from_slice(&(offset as u32).to_be_bytes());
        data.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
        data.extend_from_slice(chunk);
        if end == body.len() {
            // Only the final chunk is acknowledged; the device answers once the whole
            // body has arrived.
            session
                .request(Service::Program, 10, cmd::WRITE_DATA, &data)
                .await?;
        } else {
            let msg = Message::new(Service::Program, 10, cmd::WRITE_DATA, data);
            session.notify(&msg).await?;
        }
        offset = end;
    }

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

/// Release anything the instrument is still holding from an abandoned session.
///
/// **Operator-driven, and deliberately not automatic.** Two faults hide behind "the
/// instrument is broken", and each is one frame to cure:
///
/// - An abandoned **UI** session (`HELLO` with no `GOODBYE`) makes the device answer
///   every slot in every class as **empty** — a wrong answer that looks like a right one.
///   Nothing detects it, because nothing fails. A bare `GOODBYE` clears it.
/// - An abandoned **class** session makes it refuse operations with status `0x12`.
///   A bare `SESSION_CLOSE` clears that, and [`Session::open`] already does it.
///
/// Both are sent **bare** — no session wrapped around them — because the session
/// machinery is exactly what is broken. Both are best-effort: sending them to a healthy
/// instrument is harmless, so this needs no diagnosis first.
///
/// This is not folded into [`Session::open`] on purpose. NSM sends no such frame, the
/// golden replays pin our exchanges against real captures, and quietly diverging from
/// that ground truth to paper over an operator-caused fault would cost more than it saves.
/// Read and discard anything the device still has queued, until it goes quiet.
///
/// Unread replies are how the stream gets out of step; nothing here writes, so it is safe
/// on a healthy instrument — it simply finds nothing.
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

pub async fn recover<T: Transport>(transport: &mut T) -> Result<()> {
    // Drain first. A reply nobody read leaves the stream one message ahead, so every
    // later request is answered by the *previous* one's reply — the tell is an error
    // naming two commands that are one apart. Sending anything before draining keeps the
    // offset intact, which is why the two frames below cannot cure it on their own.
    drain(transport).await?;

    let goodbye = Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, Vec::new());
    transport.write(&goodbye.encode()).await?;
    let _ = transport.read(crate::transport::READ_BUFFER).await?;

    let close = Message::new(Service::Program, 10, cmd::SESSION_CLOSE, Vec::new());
    transport.write(&close.encode()).await?;
    let _ = transport.read(crate::transport::READ_BUFFER).await?;
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
            at.bank + 1,
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
            at.slot + 1
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

/// Device status refusing a [`cmd::NEXT_SLOT`] request that lacks the direction word.
///
/// The two-word `[bank][slot]` shape is accepted on a boot with no mutations behind it
/// and refused with this status after any write since power-up — which looked like a
/// power-cycle-only enumeration lockout until NSM's own sync traffic showed the
/// three-word form answering during the "lockout". [`next_occupied`] sends the
/// three-word form, so this status should no longer occur; it stays surfaced as
/// [`Error::DeviceStatus`] rather than swallowed, because a walk refused for any
/// reason must not pass off a partial list as an inventory.
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
    // Direction: 0 walks forward. The word is required — without it the device
    // answers only until the first write of the power cycle (ENUMERATION_DISABLED).
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
                    if next.bank == bank
                        && (at.slot == SLOT_BOUNDARY || next.slot > at.slot) =>
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
