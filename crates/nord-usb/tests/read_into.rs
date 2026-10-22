//! A read streamed into a `FileSink`: the frames it sends, what it hands the sink, and
//! what it holds while it runs.
//!
//! The exchanges are the claim, so every trial asserts its script was fully consumed.

#![cfg(feature = "replay")]

#[path = "support/frames.rs"]
mod frames;
#[path = "support/scripts.rs"]
mod scripts;

use std::cell::Cell;
use std::io;

use frames::{notify, request, response, session_close, session_open, slot_args, words};
use nord_format::crc::{crc32, Crc32Stream};
use nord_usb::op::{self, Received};
use nord_usb::transport::{ReplayTransport, Step};
use nord_usb::wire::{cmd, ui, ObjectClass};
use nord_usb::{envelope, Error, FileSink, Location, Result, Session};

/// The body bytes one `READ` asks for, as Nord Sound Manager asks.
const CHUNK: usize = 32720;

/// Where a type-1 file's body starts, behind its header.
const BODY_START: u64 = 0x2c;

thread_local! {
    /// The largest allocation this thread has made since the last [`largest_allocation`]
    /// began, or `None` outside one.
    static LARGEST: Cell<Option<usize>> = const { Cell::new(None) };
}

fn note(size: usize) {
    // `try_with`: the allocator runs during thread teardown too.
    let _ = LARGEST.try_with(|largest| {
        if let Some(seen) = largest.get() {
            largest.set(Some(seen.max(size)));
        }
    });
}

struct Watching;

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl std::alloc::GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        note(size);
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static WATCHING: Watching = Watching;

/// What `f` answers, and the largest single allocation it made on this thread.
fn largest_allocation<T>(f: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|largest| largest.set(Some(0)));
    let answer = f();
    let largest = LARGEST.with(|largest| largest.take()).unwrap_or_default();
    (answer, largest)
}

/// A body whose every chunk differs, so a chunk written at the wrong offset is caught.
fn body(len: usize) -> Vec<u8> {
    (0..len as u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect()
}

/// An `INFO` reply for a sample of `len` body bytes, carrying no checksum, as a library
/// slot reports none.
fn sample_info(at: Location, len: usize, name: &str) -> Vec<u8> {
    let mut p = words(&[at.bank, at.slot, len as u32]);
    p.extend_from_slice(b"nsmp");
    p.extend_from_slice(&words(&[200, 0x5541_012f, 0x0008_0000, name.len() as u32]));
    p.extend_from_slice(name.as_bytes());
    p.extend_from_slice(&u32::MAX.to_be_bytes());
    p
}

/// The frames of a read of `body` from a sample slot, in the order the recorded program
/// read shows: `INFO`, the label, `BEGIN_READ`, each chunk asked for and answered with
/// the progress after every chunk that moves it, then `END_TRANSFER`.
fn sample_read(at: Location, body: &[u8]) -> Vec<Step> {
    let mut steps = session_open(ObjectClass::Sample);
    steps.push(request(cmd::INFO, &slot_args(at)));
    steps.push(response(cmd::INFO, &sample_info(at, body.len(), "Long")));
    steps.push(notify(ui::label("Uploading...").unwrap()));
    steps.push(request(cmd::BEGIN_READ, &slot_args(at)));
    steps.push(response(cmd::BEGIN_READ, &slot_args(at)));
    let mut painted = None;
    for (i, chunk) in body.chunks(CHUNK).enumerate() {
        let (offset, end) = (i * CHUNK, i * CHUNK + chunk.len());
        let asked = words(&[at.bank, at.slot, offset as u32, chunk.len() as u32]);
        steps.push(request(cmd::READ, &asked));
        steps.push(response(cmd::READ, &[&asked[..], chunk].concat()));
        let pct = (end * 100 / body.len()) as u16;
        if painted != Some(pct) {
            steps.push(notify(ui::percent(pct)));
            painted = Some(pct);
        }
    }
    steps.push(request(cmd::END_TRANSFER, &slot_args(at)));
    steps.push(response(cmd::END_TRANSFER, &slot_args(at)));
    steps.extend(session_close());
    steps
}

/// What one read returned, the frames it sent, and where the script stopped short.
struct Run<R> {
    result: Result<R>,
    sent: Vec<Vec<u8>>,
    exhausted: Option<String>,
}

/// `read` in one session over `steps`, closed whatever it returned.
fn run<R>(
    class: ObjectClass,
    steps: Vec<Step>,
    read: impl AsyncFnOnce(&mut Session<'_, ReplayTransport, nord_usb::ReadOnly>) -> Result<R>,
) -> Run<R> {
    let mut t = ReplayTransport::new(steps);
    let result = pollster::block_on(async {
        let mut s = Session::open(&mut t, class).await.unwrap();
        let r = read(&mut s).await;
        s.commit().await.expect("the session closes after the read");
        r
    });
    let exhausted = match t.is_exhausted() {
        true => None,
        false => Some(format!("{:?} at step {}", t.mismatch(), t.position())),
    };
    Run {
        result,
        sent: t.sent().to_vec(),
        exhausted,
    }
}

/// A sink that keeps only what it was handed: each write's offset and length, a running
/// checksum of the body region in the order it arrived, and the header.
#[derive(Default)]
struct Hashing {
    writes: Vec<(u64, usize)>,
    body: Option<Crc32Stream<'static>>,
    head: Vec<u8>,
    /// The write that fails, by its position among the writes.
    fails_at: Option<usize>,
}

impl FileSink for Hashing {
    async fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        if self.fails_at == Some(self.writes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "the disk is full",
            ));
        }
        self.writes.push((offset, buf.len()));
        match offset {
            0 => self.head = buf.to_vec(),
            _ => self.body.get_or_insert_with(Crc32Stream::new).update(buf),
        }
        Ok(())
    }
}

/// The recorded program read, into memory and into a sink: both send what Nord Sound
/// Manager sent, and both rebuild the file it saved.
#[test]
fn the_recorded_read_rebuilds_the_saved_file_through_a_sink() {
    let script = scripts::fixture("program/read_prog_bank8_loc14.script");
    let steps = script.steps();
    let saved = std::fs::read(scripts::fixtures().join("program/prog_8-14.ne5p")).unwrap();
    let at = Location::from_user(8, 14);

    let held = run(ObjectClass::Program, steps.clone(), async |s| {
        op::read_program(s, at).await
    });
    let mut file = Vec::new();
    let streamed = run(ObjectClass::Program, steps, async |s| {
        op::read_into(s, at, &mut file).await
    });

    assert_eq!(held.exhausted, None, "into memory");
    assert_eq!(streamed.exhausted, None, "into a sink");
    assert!(held.result.unwrap() == saved, "the file read into memory");
    let received = streamed.result.unwrap();
    assert!(file == saved, "the file streamed into a sink");
    assert_eq!(received.body_crc32, crc32(&saved[BODY_START as usize..]));
    assert_eq!(received.info.crc32, Some(received.body_crc32));
}

/// A body of many chunks sends the same frames into memory and into a sink. The sink is
/// handed the body a chunk at a time, in order behind the header's room, then the header
/// last, and no allocation the read makes comes near the body's size.
#[test]
fn a_body_of_many_chunks_streams_a_chunk_at_a_time_in_bounded_memory() {
    let at = Location { bank: 0, slot: 41 };
    let body = body(96 * CHUNK + 1234);
    let steps = sample_read(at, &body);
    let file = envelope::wrap("nsmp", at, 200, &body).unwrap();

    let held = run(ObjectClass::Sample, steps.clone(), async |s| {
        op::read_program(s, at).await
    });
    let mut sink = Hashing::default();
    let (streamed, largest) = largest_allocation(|| {
        run(ObjectClass::Sample, steps, async |s| {
            op::read_into(s, at, &mut sink).await
        })
    });

    assert_eq!(held.exhausted, None, "into memory");
    assert_eq!(streamed.exhausted, None, "into a sink");
    assert!(
        held.sent == streamed.sent,
        "the two reads sent different frames"
    );
    assert!(
        held.result.unwrap() == file,
        "into memory, the file is wrapped"
    );

    let Received { info, body_crc32 } = streamed.result.unwrap();
    assert_eq!(info.body_len as usize, body.len());
    assert_eq!(body_crc32, crc32(&body));
    assert_eq!(sink.body.map(|crc| crc.value()), Some(crc32(&body)));
    assert!(
        sink.head == file[..BODY_START as usize],
        "the header is the one wrap writes"
    );
    let (last, chunks) = sink.writes.split_last().unwrap();
    assert_eq!(*last, (0, BODY_START as usize), "the header comes last");
    let mut next = BODY_START;
    for &(offset, len) in chunks {
        assert_eq!(offset, next, "{:?}", sink.writes);
        assert!(len <= CHUNK, "one write handed over {len} bytes");
        next += len as u64;
    }
    assert_eq!(next, BODY_START + body.len() as u64);
    assert!(
        largest < 4 * CHUNK,
        "the largest allocation was {largest} bytes, of a {}-byte body",
        body.len()
    );
}

/// A sink that fails partway through leaves the session as a refused read does: in step,
/// closed in full by the caller, with nothing more asked of the device.
#[test]
fn a_sink_failing_mid_read_stops_the_read_and_the_session_still_closes() {
    let at = Location { bank: 0, slot: 41 };
    let body = body(3 * CHUNK);
    let steps = sample_read(at, &body);
    // Open, INFO and its reply, the label, BEGIN_READ and its reply, then the first
    // chunk's request, reply and progress, then the second chunk's request and reply.
    let opened = session_open(ObjectClass::Sample).len();
    let mut cut = steps[..opened + 5 + 3 + 2].to_vec();
    cut.extend(session_close());

    let mut sink = Hashing {
        fails_at: Some(1),
        ..Hashing::default()
    };
    let r = run(ObjectClass::Sample, cut, async |s| {
        op::read_into(s, at, &mut sink).await
    });
    assert!(
        matches!(&r.result, Err(Error::Io(e)) if e.kind() == io::ErrorKind::StorageFull),
        "{:?}",
        r.result.as_ref().map(|_| ())
    );
    assert_eq!(r.exhausted, None);
    assert!(sink.head.is_empty(), "no header follows a failed body");
}

/// A body that does not match the checksum the device reported is refused, and its
/// header is never written, so the sink holds nothing that reads as a whole file.
#[test]
fn a_body_that_fails_the_reported_checksum_gets_no_header() {
    let script = scripts::fixture("program/read_prog_bank8_loc14.script");
    let mut steps = script.steps();
    let read = steps
        .iter()
        .position(|step| match step {
            Step::In(frame) => nord_usb::wire::Message::decode_response(frame)
                .is_ok_and(|m| m.command == cmd::READ + 1),
            _ => false,
        })
        .expect("the recording answers a READ");
    let Step::In(frame) = &steps[read] else {
        unreachable!()
    };
    let mut reply = nord_usb::wire::Message::decode_response(frame).unwrap();
    *reply.args.last_mut().unwrap() ^= 1;
    steps[read] = Step::In(reply.encode());

    let mut sink = Hashing::default();
    let r = run(ObjectClass::Program, steps, async |s| {
        op::read_into(s, Location::from_user(8, 14), &mut sink).await
    });
    assert!(
        matches!(r.result, Err(Error::Envelope(_))),
        "{:?}",
        r.result
    );
    assert_eq!(r.exhausted, None);
    assert!(
        sink.head.is_empty(),
        "no header for a body that failed its check"
    );
}

/// A file read back off the disk is the read's only when it holds every byte the read
/// handed it: one cut short, one whose header never landed, and one with a byte changed
/// all fail the check.
#[test]
fn a_file_read_back_short_unheaded_or_changed_fails_its_check() {
    let at = Location { bank: 0, slot: 158 };
    let body = body(2 * CHUNK + 7);
    let mut file = Vec::new();
    let read = run(ObjectClass::Sample, sample_read(at, &body), async |s| {
        op::read_into(s, at, &mut file).await
    });
    let received = read.result.unwrap();
    let check = |bytes: &[u8]| pollster::block_on(op::verify_read(&mut { bytes }, &received));

    assert!(check(&file).is_ok(), "{:?}", check(&file));
    assert!(check(&file[..file.len() - 1]).is_err(), "one byte short");
    let mut unheaded = file.clone();
    unheaded[..BODY_START as usize].fill(0);
    assert!(check(&unheaded).is_err(), "a header of zeros");
    let mut changed = file.clone();
    changed[BODY_START as usize + CHUNK] ^= 0xff;
    assert!(check(&changed).is_err(), "a byte changed");
}
