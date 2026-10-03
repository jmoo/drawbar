//! A write streamed from a `FileSource`: the frames it sends, what it asks the source
//! for, and how it fails when the source does.
//!
//! The exchanges are the claim, so every trial asserts its script was fully consumed.

#![cfg(feature = "replay")]

#[path = "support/frames.rs"]
mod frames;
#[path = "support/scripts.rs"]
mod scripts;

use frames::{failed, notify, request, response, session_close, session_open, slot_args, words};
use nord_usb::error::ErrKind;
use nord_usb::transport::{ReplayTransport, Step};
use nord_usb::wire::{cmd, ui, AllocationUnit, ObjectClass, Partition};
use nord_usb::{envelope, op, Error, FileSource, Location, Result, Session};
use std::io;

/// The body bytes one `WRITE_DATA` carries, as Nord Sound Manager sends them.
const CHUNK: usize = 32720;

/// A unit of `bytes` for `class`'s partition.
fn unit(class: ObjectClass, bytes: u32) -> AllocationUnit {
    let mut fields = bytes.to_be_bytes().to_vec();
    fields.resize(29, 0);
    Partition {
        index: class.to_raw(),
        name: class.label(),
        native: false,
        fields,
    }
    .allocation_unit()
    .unwrap()
}

/// A body whose every chunk differs, so a chunk sent at the wrong offset is caught.
fn body(len: usize) -> Vec<u8> {
    (0..len as u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect()
}

/// The frames of a write's transfer, in the order hardware recordings show: the label,
/// `BEGIN_WRITE`, each chunk with only the last acknowledged and the progress after
/// every chunk that moves it, then `END_TRANSFER`.
fn transfer(at: Location, tag: &[u8; 4], body: &[u8], name: &str, stamp: u32) -> Vec<Step> {
    let mut steps = vec![notify(ui::label("Downloading...").unwrap())];
    let mut begin = slot_args(at);
    begin.extend_from_slice(&words(&[body.len() as u32]));
    begin.extend_from_slice(tag);
    begin.extend_from_slice(&words(&[stamp, u32::MAX, name.len() as u32]));
    begin.extend_from_slice(name.as_bytes());
    steps.push(request(cmd::BEGIN_WRITE, &begin));
    steps.push(response(cmd::BEGIN_WRITE, &slot_args(at)));

    let mut painted = None;
    for (i, chunk) in body.chunks(CHUNK).enumerate() {
        let (offset, end) = (i * CHUNK, i * CHUNK + chunk.len());
        let mut data = slot_args(at);
        data.extend_from_slice(&words(&[offset as u32, chunk.len() as u32]));
        data.extend_from_slice(chunk);
        steps.push(request(cmd::WRITE_DATA, &data));
        if end == body.len() {
            steps.push(response(cmd::WRITE_DATA, &slot_args(at)));
        }
        let pct = (end * 100 / body.len()) as u16;
        if painted != Some(pct) {
            steps.push(notify(ui::percent(pct)));
            painted = Some(pct);
        }
    }
    steps.push(request(cmd::END_TRANSFER, &slot_args(at)));
    steps.push(response(cmd::END_TRANSFER, &slot_args(at)));
    steps
}

/// A sample write with room to spare: `STATUS` and the transfer, inside one session.
fn sample_write(at: Location, body: &[u8], name: &str, stamp: u32) -> Vec<Step> {
    let mut steps = session_open(ObjectClass::Sample);
    steps.push(request(
        cmd::STATUS,
        &ObjectClass::Sample.to_raw().to_be_bytes(),
    ));
    // count, free, used, dirty, spare.
    steps.push(response(cmd::STATUS, &words(&[1, 1000, 0, 0, 0])));
    steps.extend(transfer(at, b"nsmp", body, name, stamp));
    steps.extend(session_close());
    steps
}

/// What one write returned, the frames it sent, and where the script stopped short.
struct Run {
    result: Result<()>,
    sent: Vec<Vec<u8>>,
    exhausted: Option<String>,
}

/// Write `file` in one session over `steps`, from `source` when given, else as a slice.
fn run(
    class: ObjectClass,
    unit: AllocationUnit,
    steps: Vec<Step>,
    (at, name, stamp): (Location, &str, u32),
    file: &[u8],
    source: Option<&mut Served>,
) -> Run {
    let mut t = ReplayTransport::new(steps);
    let result = pollster::block_on(async {
        let mut s = Session::open(&mut t, class)
            .await
            .unwrap()
            .allow_destructive_writes();
        let r = match source {
            Some(source) => op::write_from(&mut s, unit, at, source, name, stamp).await,
            None => op::write(&mut s, unit, at, file, name, stamp).await,
        };
        s.commit()
            .await
            .expect("the session closes after the write");
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

/// A file served through the trait, as a disk or a browser serves one, recording each
/// read as `(offset, len)`. A fault strikes one visit to one offset.
struct Served {
    bytes: Vec<u8>,
    reads: Vec<(u64, usize)>,
    fault: Option<(u64, usize, Fault)>,
}

#[derive(Clone, Copy)]
enum Fault {
    /// The read fails, as a file gone from the disk does.
    Fail,
    /// The read returns different bytes, as a file rewritten behind the write does.
    Change,
}

impl Served {
    fn new(bytes: &[u8]) -> Served {
        Served {
            bytes: bytes.to_vec(),
            reads: Vec::new(),
            fault: None,
        }
    }

    fn faulting(bytes: &[u8], offset: u64, visit: usize, fault: Fault) -> Served {
        Served {
            fault: Some((offset, visit, fault)),
            ..Served::new(bytes)
        }
    }
}

impl FileSource for Served {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.reads.push((offset, buf.len()));
        let visit = self.reads.iter().filter(|(at, _)| *at == offset).count();
        let start = offset as usize;
        let bytes = self
            .bytes
            .get(start..start + buf.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(bytes);
        match self.fault {
            Some((at, nth, Fault::Fail)) if (at, nth) == (offset, visit) => {
                Err(io::Error::other("the file went away"))
            }
            Some((at, nth, Fault::Change)) if (at, nth) == (offset, visit) => {
                buf[0] ^= 0xff;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Where a type-1 file's body starts, and so where its first chunk is read from.
const BODY_START: u64 = 0x2c;

fn chunk_at(k: usize) -> u64 {
    BODY_START + (k * CHUNK) as u64
}

const PUT: &str = r#"program put prog_8-14.ne5p 7:10 "prog-8-14" 0x6a89f433"#;

/// The recorded put, from a slice and from a source: both send what the instrument
/// was sent.
#[test]
fn the_recorded_put_sends_the_recorded_frames_from_a_source() {
    let script = scripts::fixture("program/put_7-10_overwrite.script");
    let section = script
        .sections
        .iter()
        .find(|s| s.intent.as_deref() == Some(PUT))
        .expect("the script records the put");
    let file = std::fs::read(scripts::fixtures().join("program/prog_8-14.ne5p")).unwrap();
    let write = (Location::from_user(7, 10), "prog-8-14", 0x6a89_f433);
    let unit = unit(ObjectClass::Program, 1);

    let mut served = Served::new(&file);
    for (how, source) in [("a slice", None), ("a source", Some(&mut served))] {
        let r = run(
            ObjectClass::Program,
            unit,
            section.steps.clone(),
            write,
            &file,
            source,
        );
        r.result.unwrap_or_else(|e| panic!("from {how}: {e}"));
        assert_eq!(r.exhausted, None, "from {how}");
    }
}

/// A body of many chunks sends the same frames from a slice and from a source, and the
/// source is asked for the header, then the body twice, never more than a chunk at once.
#[test]
fn a_body_of_many_chunks_streams_a_chunk_at_a_time() {
    let at = Location { bank: 0, slot: 41 };
    let body = body(40 * CHUNK + 1234);
    let file = envelope::wrap("nsmp", at, 1, &body).unwrap();
    let write = (at, "Long", 1_787_522_949);
    let steps = sample_write(at, &body, write.1, write.2);
    let unit = unit(ObjectClass::Sample, 131_064);

    let sliced = run(ObjectClass::Sample, unit, steps.clone(), write, &file, None);
    let mut served = Served::new(&file);
    let streamed = run(
        ObjectClass::Sample,
        unit,
        steps,
        write,
        &file,
        Some(&mut served),
    );

    sliced.result.expect("from a slice");
    streamed.result.expect("from a source");
    assert_eq!(sliced.exhausted, None, "from a slice");
    assert_eq!(streamed.exhausted, None, "from a source");
    assert!(
        sliced.sent == streamed.sent,
        "the two writes sent different frames"
    );

    assert_eq!(
        served.reads[0],
        (0, BODY_START as usize),
        "the header comes first"
    );
    let biggest = served.reads.iter().map(|&(_, len)| len).max().unwrap();
    assert!(biggest <= CHUNK, "one read asked for {biggest} bytes");
    let read: usize = served.reads.iter().map(|&(_, len)| len).sum();
    assert_eq!(
        read,
        BODY_START as usize + 2 * body.len(),
        "{:?}",
        served.reads
    );
}

/// A transfer of three chunks to program 3:6.
fn three_chunks() -> (Vec<u8>, Vec<u8>, (Location, &'static str, u32)) {
    let at = Location { bank: 2, slot: 5 };
    let body = body(2 * CHUNK + 10);
    let file = envelope::wrap("ne5p", at, 4, &body).unwrap();
    (body, file, (at, "Three", 1_787_428_287))
}

/// A source that fails partway through the transfer leaves the session as a send that
/// fails there does: in step, closed in full by the caller.
#[test]
fn a_source_failing_mid_transfer_is_cleaned_up_as_a_failed_send_is() {
    let (body, file, write) = three_chunks();
    let unit = unit(ObjectClass::Program, 1);
    let sent = transfer(write.0, b"ne5p", &body, write.1, write.2);
    // The label, BEGIN_WRITE and its reply, the first chunk and its progress.
    let before = |tail: Vec<Step>| -> Vec<Step> {
        let mut steps = session_open(ObjectClass::Program);
        steps.extend(sent[..5].iter().cloned());
        steps.extend(tail);
        steps.extend(session_close());
        steps
    };

    let send_fails = run(
        ObjectClass::Program,
        unit,
        before(vec![failed(sent[5].clone())]),
        write,
        &file,
        None,
    );
    let mut served = Served::faulting(&file, chunk_at(1), 2, Fault::Fail);
    let read_fails = run(
        ObjectClass::Program,
        unit,
        before(Vec::new()),
        write,
        &file,
        Some(&mut served),
    );

    let send = send_fails
        .result
        .expect_err("the second chunk was never sent");
    let read = read_fails
        .result
        .expect_err("the second chunk was never read");
    assert!(matches!(send, Error::Transport(_)), "{send}");
    assert!(
        matches!(&read, Error::Io(e) if e.to_string() == "the file went away"),
        "{read}"
    );
    assert_eq!(read.expect_kind(), ErrKind::Transport);
    assert_eq!(send_fails.exhausted, None, "after the failed send");
    assert_eq!(read_fails.exhausted, None, "after the failed read");
}

/// A file that cannot be read whole, or does not match its checksum, is refused before
/// any frame, as a damaged slice is.
#[test]
fn a_file_that_fails_its_check_sends_nothing() {
    let (_, file, write) = three_chunks();
    let unit = unit(ObjectClass::Program, 1);
    let idle = || {
        let mut steps = session_open(ObjectClass::Program);
        steps.extend(session_close());
        steps
    };
    let mut damaged = file.clone();
    *damaged.last_mut().unwrap() ^= 1;

    let unreadable = Served::faulting(&file, chunk_at(1), 1, Fault::Fail);
    for (what, mut source) in [
        ("unreadable", unreadable),
        ("damaged", Served::new(&damaged)),
    ] {
        let r = run(
            ObjectClass::Program,
            unit,
            idle(),
            write,
            &file,
            Some(&mut source),
        );
        r.result.expect_err(what);
        assert_eq!(r.exhausted, None, "a {what} file sent a frame");
    }

    let r = run(ObjectClass::Program, unit, idle(), write, &damaged, None);
    assert!(
        matches!(r.result, Err(Error::Envelope(_))),
        "{:?}",
        r.result
    );
    assert_eq!(r.exhausted, None, "a damaged slice sent a frame");
}

/// Bytes that change between the check and the send keep the last chunk back, so the
/// write never completes with a file nothing checked.
#[test]
fn a_file_that_changes_during_the_send_withholds_its_last_chunk() {
    let (body, file, write) = three_chunks();
    let mut changed = body.clone();
    changed[CHUNK] ^= 0xff;
    let sent = transfer(write.0, b"ne5p", &changed, write.1, write.2);
    // Through the second chunk, as changed, and its progress.
    let mut steps = session_open(ObjectClass::Program);
    steps.extend(sent[..7].iter().cloned());
    steps.extend(session_close());

    let mut served = Served::faulting(&file, chunk_at(1), 2, Fault::Change);
    let r = run(
        ObjectClass::Program,
        unit(ObjectClass::Program, 1),
        steps,
        write,
        &file,
        Some(&mut served),
    );
    assert!(
        matches!(r.result, Err(Error::Envelope(_))),
        "{:?}",
        r.result
    );
    assert_eq!(r.exhausted, None);
}
