//! Typed operations.
//!
//! Each primitive runs inside a [`Session`]; callers can batch by opening one
//! session and applying primitives repeatedly. Operations send the progress labels the
//! instrument displays but omit reads that only refresh a host UI.

use crate::envelope::{self, FileSink, FileSource, Opened};
use crate::error::{Error, Result};
use crate::session::ReadWrite;
use crate::session::{Session, WRITE_LIMIT};
use crate::transport::Transport;
use crate::wire::{
    cmd, read_u32, ui, AllocationUnit, Bank, Dependency, Location, Message, ObjectClass, Partition,
    ProgramInfo, Service, Status,
};
use nord_format::cbin::Generation;
use nord_format::crc::Crc32Stream;

/// Query the inventory for the class the session was opened with.
///
/// **Read-only.** It sends one request and reads counters back, so it is a safe way to
/// check the whole stack against real hardware.
pub async fn status<T: Transport, C>(session: &mut Session<'_, T, C>) -> Result<Status> {
    let class = session.class();
    let resp = session
        .request(&Message::program(cmd::STATUS, class.to_raw().to_be_bytes()))
        .await?;
    Status::decode(class, &resp)
}

/// Query every class worth reporting, one transaction each.
///
/// Each class needs its own session because the class is fixed at `SESSION_OPEN`.
/// Instruments differ in which classes they answer for, so two refusals skip the class:
/// a refused `SESSION_OPEN` ([`Error::ClassRefused`]) and a refused `STATUS`. Every
/// other error, including a refused `HELLO`, propagates.
pub async fn inventory<T: Transport>(transport: &mut T) -> Result<Vec<Status>> {
    let mut out = Vec::new();
    for class in ObjectClass::INVENTORY {
        let mut session = match Session::open(transport, class).await {
            Ok(s) => s,
            Err(Error::ClassRefused { .. }) => continue,
            Err(e) => return Err(e),
        };
        let result = status(&mut session).await;
        session.commit().await?;
        match result {
            Ok(s) => out.push(s),
            Err(Error::DeviceStatus(_)) => {}
            Err(e) => return Err(e),
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
    let resp = session
        .request(&Message::program(cmd::INFO, at.to_bytes()))
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
/// **Read-only.** [`read_into`], held in memory.
pub async fn read_program<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Vec<u8>> {
    let mut file = Vec::new();
    read_into(session, at, &mut file).await?;
    Ok(file)
}

/// What [`read_into`] took off the instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// The slot's object info, as the read's own `INFO` reported it.
    pub info: ProgramInfo,
    /// CRC-32 of the body as it arrived: the checksum a slot holding it reports, where
    /// its class reports one.
    pub body_crc32: u32,
}

/// Read one object off the instrument into `file` as a `CBIN` file, one transfer chunk at
/// a time, so that memory stays bounded by the chunk whatever the object's size.
///
/// **Read-only.** The body is written behind the space its `CBIN` header ([`envelope`])
/// takes, and the header last, once the body's checksum is known. When the device reports
/// a CRC-32, the body is checked against it, and a mismatch is an error with no header
/// written. `file` then holds a partial body, as it does after any error; discarding it
/// is the caller's.
///
/// The frames are the same whatever `file` is: [`read_program`] is this, into memory.
pub async fn read_into<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
    file: &mut impl FileSink,
) -> Result<Received> {
    let (info, body_crc32) = transfer_out(session, at, file, Generation::V1.body_start()).await?;
    if let Some(expected) = info.crc32 {
        if expected != body_crc32 {
            return Err(Error::Envelope(format!(
                "body checksum mismatch: device reported {expected:08x}, received {body_crc32:08x}"
            )));
        }
    }
    let head = envelope::header(&info.format, at, info.version, body_crc32)?;
    file.write_at(0, &head).await?;
    Ok(Received { info, body_crc32 })
}

/// Check that `file` holds the whole of what [`read_into`] took off the instrument as
/// `received` says: its header and every byte of its body, which its stored checksum
/// agrees with. Read back off a disk, it fails for a file the disk did not keep as
/// written.
pub async fn verify_read(file: &mut impl FileSource, received: &Received) -> Result<()> {
    let whole = Generation::V1.body_start() + u64::from(received.info.body_len);
    if file.len() != whole {
        return Err(Error::Envelope(format!(
            "the file holds {} of the {whole} bytes read into it",
            file.len()
        )));
    }
    envelope::verify(file).await.map(drop)
}

/// Read an entity's body off the instrument without wrapping it in a CBIN header.
///
/// For formats whose header layout is unknown, such as CBIN type-0 (the legacy variant
/// without a CRC), wrapping would invent a header. This returns the bytes the device
/// sent, which are safe to archive.
pub async fn read_body<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    transfer_out(session, at, &mut body, 0).await?;
    Ok(body)
}

/// Body bytes to ask for in one `READ`. A larger body arrives across several requests,
/// the offset advancing by this much each time, with a short final chunk.
///
/// Nord Sound Manager asks for `32720`. Inferred from specimens; not confirmed on
/// hardware.
///
/// Unexplained: for some objects it asks for `32726` throughout, a 6-byte difference
/// that is per object, not per chunk. Both sizes fit in one `READ_BUFFER` and the host
/// chooses the size, so the smaller is always used.
const READ_CHUNK: u32 = 32720;

/// Body bytes per `WRITE_DATA` frame. The whole frame must stay under the device's
/// maximum transfer; an oversized frame wedges the instrument until a power cycle.
pub(crate) const WRITE_CHUNK: usize = 32720;

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

#[cfg(not(feature = "fault-injection"))]
fn chunk_override(_name: &str, default: u64) -> Result<u64> {
    Ok(default)
}

fn read_chunk() -> Result<u32> {
    let size = chunk_override("NORD_READ_CHUNK", READ_CHUNK.into())?;
    u32::try_from(size).map_err(|_| Error::InvalidArgument("NORD_READ_CHUNK exceeds u32".into()))
}

fn write_chunk() -> Result<usize> {
    let size = chunk_override("NORD_WRITE_CHUNK", WRITE_CHUNK as u64)?;
    usize::try_from(size)
        .map_err(|_| Error::InvalidArgument("NORD_WRITE_CHUNK exceeds usize".into()))
}

/// Read the metadata and body through the device's chunked transfer sequence, writing
/// the body into `file` from `base`. Returns the metadata and the body's CRC-32.
async fn transfer_out<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
    file: &mut impl FileSink,
    base: u64,
) -> Result<(ProgramInfo, u32)> {
    let chunk_size = read_chunk()?;
    let meta = info(session, at).await?;

    session.notify(&ui::label("Uploading...")?).await?;

    session
        .request(&Message::program(cmd::BEGIN_READ, at.to_bytes()))
        .await?;

    let mut crc = Crc32Stream::new();
    let mut offset = 0u32;
    let mut painted = None;
    while offset < meta.body_len {
        let want = chunk_size.min(meta.body_len - offset);

        let req = [
            &at.to_bytes()[..],
            &offset.to_be_bytes(),
            &want.to_be_bytes(),
        ]
        .concat();
        let resp = session.request(&Message::program(cmd::READ, req)).await?;

        let chunk = read_payload(resp.payload(), at, offset, want)?;
        file.write_at(base + u64::from(offset), chunk).await?;
        crc.update(chunk);
        offset += want;

        // Progress moves only at whole percentages.
        let pct = (offset as u64 * 100 / (meta.body_len.max(1)) as u64) as u16;
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
        .request(&Message::program(cmd::END_TRANSFER, at.to_bytes()))
        .await?;
    Ok((meta, crc.value()))
}

fn read_payload(payload: &[u8], at: Location, offset: u32, length: u32) -> Result<&[u8]> {
    let echoed = (
        read_u32(payload, 0)?,
        read_u32(payload, 4)?,
        read_u32(payload, 8)?,
        read_u32(payload, 12)?,
    );
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

/// Bound on polling `0x26` for the cleaning pass. The pass normally finishes within a
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
        .request(&Message::program(cmd::WRITE_PREPARE, blocks.to_be_bytes()))
        .await?;

    let mut painted = Some(0);
    for polls in 0..CLEANING_POLLS {
        if polls > 0 {
            crate::sleep::sleep(CLEANING_POLL_SPACING).await;
        }
        let resp = session
            .request(&Message::program(cmd::WRITE_PREPARE_2, Vec::new()))
            .await?;
        let (requested, done, running) = cleaning_progress(resp.payload())?;
        // Ready is `running` returning to 0; `done` can end above the request, so the
        // bar is clamped.
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
    Err(Error::Transport(format!(
        "the library's cleaning pass did not report ready within {} polls",
        CLEANING_POLLS
    )))
}

/// The `[requested, done, running]` words a cleaning-progress reply carries.
fn cleaning_progress(payload: &[u8]) -> Result<(u32, u32, u32)> {
    Ok((
        read_u32(payload, 0)?,
        read_u32(payload, 4)?,
        read_u32(payload, 8)?,
    ))
}

/// Make room for `blocks` storage blocks in a library partition, in the session that is
/// about to write. Requires a [`ReadWrite`] session.
///
/// A library write is refused `0x16` unless a prepared block exists per storage block of
/// body. This reads [`status`] and, when `blocks` exceeds what is free, reclaims the
/// shortfall from `dirty` and waits for the pass to finish. Otherwise it sends only the
/// `STATUS` request.
///
/// `blocks` is the body's length in units of the partition's [`AllocationUnit`]; any
/// other count sizes the reclaim wrongly. [`write()`] computes it for its caller.
pub async fn reserve<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    blocks: u32,
) -> Result<()> {
    let free = status(session).await?.free;
    if blocks > free {
        clean_library(session, blocks - free).await?;
    }
    Ok(())
}

/// Write an entity into a slot. The file carries no name, so `name` becomes the slot's
/// name, even when it is a placeholder.
///
/// A library write is refused `0x16` without a prepared block per storage block of body.
/// When `unit` counts blocks, [`reserve`] and the transfer share one transaction; a
/// byte-granular partition sends the transfer alone. `unit` is the partition's
/// [`AllocationUnit`], from
/// [`Geometry::allocation_unit`](crate::device::Geometry::allocation_unit). It sizes the
/// reclaim from the file's CBIN body, which is shorter than the file by its header.
pub async fn write<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    unit: AllocationUnit,
    at: Location,
    mut file: &[u8],
    name: &str,
    timestamp: u32,
) -> Result<()> {
    write_from(session, unit, at, &mut file, name, timestamp).await
}

/// [`write()`], reading the file from `file` one transfer chunk at a time, so that memory
/// stays bounded by the chunk whatever the file's size.
///
/// The body is read twice. The first pass checks it against the file's checksum before
/// any frame is sent, so a damaged file is refused as [`write()`] refuses it. The second
/// sends it, and withholds the final chunk with [`Error::Envelope`] if the bytes no
/// longer match, which leaves the write unfinished rather than completed with a file
/// that changed after it was checked.
///
/// A failed read from `file` returns its [`Error::Io`] the way a failed send returns
/// its error: the session is still in step, and closing it is the caller's.
pub async fn write_from<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    unit: AllocationUnit,
    at: Location,
    file: &mut impl FileSource,
    name: &str,
    timestamp: u32,
) -> Result<()> {
    if !unit.belongs_to(session.class().to_raw()) {
        return Err(Error::InvalidArgument(format!(
            "the allocation unit belongs to another partition, not {}",
            session.class().label()
        )));
    }
    let chunk = write_chunk()?;
    let opened = envelope::open(file, chunk).await?;
    if !unit.is_bytes() {
        reserve(session, unit.blocks_for(opened.body.len())?).await?;
    }
    transfer_in(session, at, file, &opened, chunk, name, timestamp).await
}

/// A [`cmd::BEGIN_WRITE`] argument block: the address, the body's length, the format
/// tag, the timestamp, the `0xffffffff` word, and the slot's name, length-prefixed.
///
/// `BEGIN_WRITE` is the only frame of a write that carries a name, and it becomes the
/// slot's name.
pub fn begin_write_args(
    at: Location,
    body_len: usize,
    tag: &[u8; 4],
    timestamp: u32,
    name: &str,
) -> Result<Vec<u8>> {
    let body_len = u32::try_from(body_len)
        .map_err(|_| Error::InvalidArgument("the body is larger than the wire format".into()))?;
    let mut args = at.to_bytes().to_vec();
    args.extend_from_slice(&body_len.to_be_bytes());
    args.extend_from_slice(tag);
    args.extend_from_slice(&timestamp.to_be_bytes());
    args.extend_from_slice(&u32::MAX.to_be_bytes());
    put_name(&mut args, name)?;
    Ok(args)
}

/// Append a name as every string on the wire is sent: a big-endian `u32` length, then
/// the bytes, unpadded.
fn put_name(args: &mut Vec<u8>, name: &str) -> Result<()> {
    let len = u32::try_from(name.len())
        .map_err(|_| Error::InvalidArgument("the name is larger than the wire format".into()))?;
    args.extend_from_slice(&len.to_be_bytes());
    args.extend_from_slice(name.as_bytes());
    Ok(())
}

/// A [`cmd::WRITE_DATA`] argument block: the address, the chunk's offset and length,
/// then the chunk.
pub fn write_data_args(at: Location, offset: usize, chunk: &[u8]) -> Result<Vec<u8>> {
    let offset = u32::try_from(offset)
        .map_err(|_| Error::InvalidArgument("the offset is larger than the wire format".into()))?;
    let len = u32::try_from(chunk.len())
        .map_err(|_| Error::InvalidArgument("the chunk is larger than the wire format".into()))?;
    let mut args = at.to_bytes().to_vec();
    args.extend_from_slice(&offset.to_be_bytes());
    args.extend_from_slice(&len.to_be_bytes());
    args.extend_from_slice(chunk);
    Ok(args)
}

/// The write transfer itself, identical for every class.
async fn transfer_in<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
    file: &mut impl FileSource,
    opened: &Opened,
    chunk_size: usize,
    name: &str,
    timestamp: u32,
) -> Result<()> {
    let body = opened.body.clone();
    let len = body.len();
    let mut verifier = opened.verifier()?;

    session.notify(&ui::label("Downloading...")?).await?;

    let begin = begin_write_args(at, len, &opened.header.tag, timestamp, name)?;
    session
        .request(&Message::program(cmd::BEGIN_WRITE, begin))
        .await?;

    let mut buf = vec![0; chunk_size.min(len)];
    let mut offset = 0usize;
    let mut painted = None;
    while offset < len {
        let end = offset.saturating_add(chunk_size).min(len);
        let chunk = &mut buf[..end - offset];
        file.read_at((body.start + offset) as u64, chunk).await?;
        verifier
            .update(chunk)
            .map_err(|e| Error::Envelope(e.to_string()))?;
        let data = Message::program(cmd::WRITE_DATA, write_data_args(at, offset, chunk)?);
        // Only the final chunk is acknowledged.
        if end == len {
            if !opened.matches(std::mem::take(&mut verifier))? {
                return Err(Error::Envelope(
                    "the file changed after its checksum was checked, so its last chunk \
                     was not sent"
                        .into(),
                ));
            }
            session.request(&data).await?;
        } else {
            session.notify(&data).await?;
        }
        offset = end;

        let pct = (offset as u64 * 100 / (len.max(1)) as u64) as u16;
        if painted != Some(pct) {
            session.notify(&ui::percent(pct)).await?;
            painted = Some(pct);
        }
    }

    if painted != Some(100) {
        session.notify(&ui::percent(100)).await?;
    }

    session
        .request(&Message::program(cmd::END_TRANSFER, at.to_bytes()))
        .await?;
    Ok(())
}

/// Load a stored object live on the instrument, as "open on device" or a double-click
/// does in Nord Sound Manager. The device switches to it immediately.
///
/// **Non-destructive.** Nothing stored changes, so this needs no [`ReadWrite`] session.
/// This is the only command with inverted parity (`0x2f` request, `0x30` response).
pub async fn select<T: Transport, C>(session: &mut Session<'_, T, C>, at: Location) -> Result<()> {
    session
        .request(&Message::program(cmd::SELECT, at.to_bytes()))
        .await?;
    Ok(())
}

/// Drain queued replies until the transport stays quiet.
async fn drain<T: Transport>(transport: &mut T) -> Result<()> {
    for _ in 0..RECOVER_DRAIN_CAP {
        match transport
            .read_timeout(crate::transport::READ_BUFFER, RECOVER_DRAIN_LIMIT)
            .await?
        {
            Some(_) => continue,
            None => break,
        }
    }
    Ok(())
}

/// How long to wait for a straggler before deciding the stream is quiet.
const RECOVER_DRAIN_LIMIT: std::time::Duration = std::time::Duration::from_millis(300);

/// Upper bound on stragglers, so a device that keeps talking cannot hang this.
const RECOVER_DRAIN_CAP: usize = 16;

/// Send one frame of the recovery sequence, naming the endpoint when it is not accepted.
///
/// ⚠️ An unbounded write blocks forever on the instrument this exists for: a stalled
/// bulk OUT endpoint neither accepts the frame nor fails.
async fn send_recovery<T: Transport>(transport: &mut T, msg: &Message, what: &str) -> Result<()> {
    if transport.write_timeout(&msg.encode(), WRITE_LIMIT).await? {
        return Ok(());
    }
    Err(Error::Transport(format!(
        "the device did not accept {what} within {}s: bulk OUT endpoint {:#04x} is \
         stalled, and only a power cycle clears it",
        WRITE_LIMIT.as_secs(),
        crate::transport::EP_OUT
    )))
}

/// Release UI and class state left by an abandoned session.
///
/// A bare `GOODBYE` clears the UI state that makes every slot appear empty; a bare
/// `SESSION_CLOSE` clears class status `0x12`. Queued replies are drained first.
pub async fn recover<T: Transport>(transport: &mut T) -> Result<()> {
    // An unread reply would pair every later request with the previous request's reply.
    drain(transport).await?;

    // ⚠️ Bounded reads: the instrument this is for has stopped answering, so no reply
    // to either frame is expected and is not a failure.
    let goodbye = Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, Vec::new());
    send_recovery(transport, &goodbye, "GOODBYE").await?;
    let _ = transport
        .read_timeout(crate::transport::READ_BUFFER, RECOVER_DRAIN_LIMIT)
        .await?;

    let close = Message::program(cmd::SESSION_CLOSE, Vec::new());
    send_recovery(transport, &close, "SESSION_CLOSE").await?;
    let _ = transport
        .read_timeout(crate::transport::READ_BUFFER, RECOVER_DRAIN_LIMIT)
        .await?;
    Ok(())
}

/// Every storage partition the device reports. **Read-only.**
///
/// The index of each entry is its object class code, so this also lists the classes the
/// instrument has, including the `(Native)` library views that have no [`ObjectClass`]
/// name.
pub async fn partitions<T: Transport, C>(
    session: &mut Session<'_, T, C>,
) -> Result<Vec<Partition>> {
    let resp = session
        .request(&Message::program(cmd::PARTITIONS, Vec::new()))
        .await?;
    Partition::decode_all(&resp)
}

/// One partition's banks and their slot capacities. **Read-only.**
pub async fn banks<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    partition: u32,
) -> Result<Vec<Bank>> {
    let resp = session
        .request(&Message::program(cmd::BANKS, partition.to_be_bytes()))
        .await?;
    let reported = read_u32(resp.payload(), 0)?;
    if reported != partition {
        return Err(Error::UnexpectedPartition {
            requested: partition,
            reported,
        });
    }
    Bank::decode_all(&resp)
}

/// Whether an address exists on this instrument, per the device's own geometry.
///
/// **Read-only.** It answers before anything is attempted; otherwise a write to a bad
/// address fails only once the transfer is under way. It does not check occupancy.
///
/// `Ok(None)` means the address exists. `Ok(Some(reason))` explains why it does not.
///
/// [`Geometry::check_address`](crate::device::Geometry::check_address) asks the same of
/// geometry already read, and sends nothing.
pub async fn check_address<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Option<String>> {
    let banks = banks(session, session.class().to_raw()).await?;
    Ok(address_refusal(&banks, at))
}

/// Why `at` is not an address among `banks`, or `None` where it is.
///
/// The reason uses the instrument's own bank names, which for pianos are categories
/// such as "Grand" and "Upright".
pub(crate) fn address_refusal(banks: &[Bank], at: Location) -> Option<String> {
    let Some(bank) = banks.get(at.bank as usize) else {
        let names: Vec<&str> = banks.iter().map(|b| b.name.as_str()).collect();
        return Some(format!(
            "bank {} does not exist; this class has {} ({})",
            at.user_bank(),
            banks.len(),
            names.join(", ")
        ));
    };
    // The `(Native)` partitions report a sentinel in place of a capacity.
    (bank.is_bounded() && at.slot >= bank.slots).then(|| {
        format!(
            "\"{}\" holds {} slots, so slot {} is out of range",
            bank.name,
            bank.slots,
            at.user_slot()
        )
    })
}

/// The object the panel currently has loaded, for the session's class. **Read-only.**
///
/// The read half of [`select`].
pub async fn focus<T: Transport, C>(session: &mut Session<'_, T, C>) -> Result<Location> {
    let resp = session
        .request(&Message::program(cmd::FOCUS, Vec::new()))
        .await?;
    Location::read(resp.payload(), 0)
}

/// Device status refusing a [`cmd::NEXT_SLOT`] without the direction word. It is
/// reported as an error so a refused walk cannot return a partial list.
pub const ENUMERATION_DISABLED: u32 = 0x11;

/// Device status refusing a request aimed at an empty slot. Confirmed on hardware.
///
/// It also ends a [`next_occupied`] walk, and answers [`focus`] when nothing of the
/// session's class is loaded.
pub const VACANT: u32 = 0x1;

/// Device status refusing an address past the instrument's geometry. Confirmed on
/// hardware.
pub const OUT_OF_RANGE: u32 = 0x3;

/// Device status refusing a write into an occupied slot of a class that does not
/// [overwrite in place](ObjectClass::overwrites_in_place). Confirmed on hardware.
pub const OCCUPIED: u32 = 0x4;

/// Device status refusing a library write named as another object of that library
/// already is. The names are compared exactly, case included, and the slot written to
/// makes no difference. Confirmed on hardware.
pub const NAME_TAKEN: u32 = 0x18;

/// Slot value meaning "from the bank's boundary": the bank's first occupied slot when
/// walking forward, its last when walking backward.
pub const SLOT_BOUNDARY: u32 = 0xffff_ffff;

/// Host limit on the slots one occupied-slot walk may return. Exceeding it is an error.
pub const ENUMERATION_LIMIT: usize = 4096;

/// The next occupied slot after `at`, or `None` once the walk runs off the end.
///
/// **Read-only.** Positions inside a gap are safe to pass: the device answers with the
/// next occupied slot, so this walks content and skips empty addresses.
/// `at.slot == SLOT_BOUNDARY` starts before the bank's first slot.
pub async fn next_occupied<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Option<Location>> {
    // Direction, 0 = forward. Omitting it is refused after any write since power-up.
    let args = [&at.to_bytes()[..], &0u32.to_be_bytes()].concat();
    match session
        .request(&Message::program(cmd::NEXT_SLOT, args))
        .await
    {
        Ok(resp) => Location::read(resp.payload(), 0).map(Some),
        // Past the end, which ends the walk. A refusal leaves the session in step, so
        // the caller may continue.
        Err(Error::DeviceStatus(VACANT)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Every occupied slot in the session's class, in address order.
///
/// **Read-only.** [`next_occupied`] walks within one bank and stops at its end, so this
/// drives it over `banks` in table order, each from [`SLOT_BOUNDARY`]. Pianos and
/// programs span several banks, and walking bank 0 alone would silently report part of
/// the class.
///
/// `banks` is the instrument's own answer for this class: [`banks`] on the partition
/// whose index is the class code, or [`Geometry::banks`](crate::device::Geometry::banks).
/// A bank ends where the device ends it (status `1` to a cursor request), and its
/// declared capacity bounds how many objects it may yield.
///
/// A cursor answer that leaves the bank, repeats, goes backward, or exceeds the declared
/// capacity is [`Error::Enumeration`]. A walk longer than [`ENUMERATION_LIMIT`] is
/// [`Error::ScanLimit`].
///
/// A refusal mid-walk, such as [`ENUMERATION_DISABLED`], propagates as its error. A
/// partial inventory that looks complete would be worse than none.
pub async fn occupied_slots<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    banks: &[Bank],
) -> Result<Vec<Location>> {
    let mut found: Vec<Location> = Vec::new();

    for bank in banks {
        let mut at = Location {
            bank: bank.index,
            slot: SLOT_BOUNDARY,
        };
        let mut previous = None;
        let limit = match bank.is_bounded() {
            true => bank.slots,
            false => Bank::UNBOUNDED,
        };
        while let Some(next) = next_occupied(session, at).await? {
            let advanced = next.bank == bank.index
                && next.slot < limit
                && previous.is_none_or(|slot| next.slot > slot);
            if !advanced {
                return Err(Error::Enumeration {
                    bank: bank.index,
                    answered: next,
                    slots: bank.slots,
                });
            }
            if found.len() >= ENUMERATION_LIMIT {
                return Err(Error::ScanLimit {
                    bank: bank.index,
                    limit: ENUMERATION_LIMIT as u32,
                });
            }
            found.push(next);
            at = next;
            previous = Some(next.slot);
        }
    }
    Ok(found)
}

/// List the piano and sample library objects an entity depends on, as the device
/// reports them, including rows that are not dependencies.
///
/// **Read-only.** The returned [`Dependency`] ids match the ids the objects carry in
/// their own files, which links wire content to file bytes.
pub async fn dependencies<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    at: Location,
) -> Result<Vec<Dependency>> {
    let resp = session
        .request(&Message::program(cmd::DEPENDENCIES, at.to_bytes()))
        .await?;
    let dependencies = Dependency::decode_all(&resp)?;
    let reported = Location::read(resp.payload(), 0)?;
    if reported != at {
        return Err(Error::UnexpectedLocation {
            requested: at,
            reported,
        });
    }
    Ok(dependencies)
}

/// A set list holding a reference to a program slot a caller is about to disturb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Referrer {
    /// Where the set list itself lives.
    pub at: Location,
    /// The set list's name, as the instrument shows it.
    pub name: String,
    /// The set list's current schema version.
    ///
    /// ⚠️ The device rewrites a referring set list in the current format, so a version-0
    /// set list is migrated to version 1 and cannot be restored.
    pub version: u32,
    /// Which of the queried program slots this set list points at.
    pub programs: Vec<Location>,
}

/// Every set list that references one of `targets`. **Read-only.**
///
/// The session must be open on [`ObjectClass::SetList`]; the walk and every read run
/// inside it, over `banks` as [`occupied_slots`] describes.
///
/// Use this to describe what a program move will do. The instrument maintains
/// referential integrity itself: moving a program rewrites the body of every set list
/// pointing at it, so a move changes objects in another class that the caller never
/// named. Where [`Referrer::version`] is `0`, that rewrite is irreversible. Neither the
/// move request nor its reply reports this, so the only way to know is to ask every set
/// list first.
///
/// Cost is one `DEPENDENCIES` per occupied set list, plus one `INFO` per match. A
/// refusal mid-scan propagates, as does a walk that contradicts the declared geometry,
/// because a short list would read as "no set list is affected".
///
/// Confirmed on hardware.
pub async fn set_lists_referencing<T: Transport, C>(
    session: &mut Session<'_, T, C>,
    banks: &[Bank],
    targets: &[Location],
) -> Result<Vec<Referrer>> {
    if session.class() != ObjectClass::SetList {
        return Err(Error::InvalidArgument(
            "set-list referrers require a set-list session".into(),
        ));
    }
    let mut out = Vec::new();
    if targets.is_empty() {
        return Ok(out);
    }
    for at in occupied_slots(session, banks).await? {
        let mut programs: Vec<Location> = Vec::new();
        for l in dependencies(session, at)
            .await?
            .into_iter()
            .filter(|d| d.class == ObjectClass::Program && d.is_required())
            .filter_map(|d| d.location)
        {
            // A set list may hold the same program in more than one of its four slots.
            if targets.contains(&l) && !programs.contains(&l) {
                programs.push(l);
            }
        }
        if programs.is_empty() {
            continue;
        }
        let meta = info(session, at).await?;
        out.push(Referrer {
            at,
            name: meta.name,
            version: meta.version,
            programs,
        });
    }
    Ok(out)
}

/// Move an object from one slot to another. The device relocates it internally, and no
/// body crosses the wire.
///
/// An occupied destination is swapped: its occupant ends up in the source slot,
/// byte-identical. Nothing is destroyed and no delete is needed first, as it is for a
/// write, which the device refuses into an occupied slot with status `0x4`. Confirmed
/// on hardware.
///
/// ⚠️ Moving a program also changes set lists. The device rewrites every set list
/// referencing either slot so no reference is left dangling, and migrates a version-0
/// set list to version 1, which moving the program back cannot undo.
/// [`set_lists_referencing`] names them beforehand; neither the request nor the reply
/// mentions them.
///
/// Requires a [`ReadWrite`] session. Works for whichever object class the session
/// opened (programs, set lists).
pub async fn move_object<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    from: Location,
    to: Location,
) -> Result<()> {
    let args = [from.to_bytes(), to.to_bytes()].concat();
    session.request(&Message::program(cmd::MOVE, args)).await?;
    Ok(())
}

/// Delete the object in a slot. Requires a [`ReadWrite`] session.
///
/// Sends the `"Deleting..."` progress label, then the delete: the two OUT frames Nord
/// Sound Manager sends (`O36 O26 I30`).
pub async fn delete<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
) -> Result<()> {
    session.notify(&ui::label("Deleting...")?).await?;
    session
        .request(&Message::program(cmd::DELETE, at.to_bytes()))
        .await?;
    Ok(())
}

/// Rename the object in a slot. Requires a [`ReadWrite`] session.
///
/// The name is sent with a big-endian length prefix and no padding, like every string on
/// the wire.
pub async fn rename<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    at: Location,
    name: &str,
) -> Result<()> {
    let mut args = at.to_bytes().to_vec();
    put_name(&mut args, name)?;
    session
        .request(&Message::program(cmd::RENAME, args))
        .await?;
    Ok(())
}

/// Duplicate the object at `from` into `to`. Requires a [`ReadWrite`] session.
///
/// The device performs a deep copy internally: the arguments are the two addresses, and
/// no body crosses the wire. Nord Sound Manager follows a copy with `INFO` and
/// `DEPENDENCIES` reads to refresh its browser; those are not sent here.
pub async fn duplicate<T: Transport>(
    session: &mut Session<'_, T, ReadWrite>,
    from: Location,
    to: Location,
) -> Result<()> {
    let args = [from.to_bytes(), to.to_bytes()].concat();
    session.request(&Message::program(cmd::COPY, args)).await?;
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

    #[test]
    fn invalid_chunk_overrides_are_refused() {
        assert!(parse_chunk("NORD_READ_CHUNK", Some("0"), READ_CHUNK.into()).is_err());
        assert!(parse_chunk("NORD_WRITE_CHUNK", Some("bad"), WRITE_CHUNK as u64).is_err());
        assert_eq!(
            parse_chunk("NORD_READ_CHUNK", None, READ_CHUNK.into()).unwrap(),
            READ_CHUNK.into()
        );
    }

    #[test]
    fn cleaning_progress_requires_all_three_words() {
        let err = cleaning_progress(&[0; 11]).expect_err("a partial cleaning reply");
        assert!(matches!(err, Error::Truncated { got: 11, need: 12 }));
    }
}
