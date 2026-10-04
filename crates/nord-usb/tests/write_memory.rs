//! A streamed write holds a few chunks at most, however large the file.
//!
//! ⚠️ The counting allocator serves this whole binary, so it holds this one test: a
//! second running alongside would count against the first.

use nord_format::cbin::Generation;
use nord_format::crc::Crc32Stream;
use nord_usb::transport::Transport;
use nord_usb::wire::{AllocationUnit, Message, ObjectClass, Partition};
use nord_usb::{envelope, op, FileSource, Location, Result, Session};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` unchanged; the counters only observe.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const CHUNK: usize = 32720;

/// A type-1 file that exists nowhere: its header is held, and each body byte is
/// computed from its offset when read.
struct Synthetic {
    header: Vec<u8>,
    body_len: usize,
}

fn byte(i: usize) -> u8 {
    ((i as u32).wrapping_mul(2_654_435_761) >> 24) as u8
}

impl Synthetic {
    fn new(at: Location, body_len: usize) -> Synthetic {
        let mut crc = Crc32Stream::new();
        let mut piece = vec![0; CHUNK];
        for start in (0..body_len).step_by(CHUNK) {
            let piece = &mut piece[..CHUNK.min(body_len - start)];
            piece
                .iter_mut()
                .enumerate()
                .for_each(|(i, b)| *b = byte(start + i));
            crc.update(piece);
        }
        let mut header = envelope::wrap("ne5p", at, 4, &[]).unwrap();
        let word = Generation::V1.checksum_range(header.len()).unwrap();
        header[word].copy_from_slice(&crc.value().to_le_bytes());
        Synthetic { header, body_len }
    }
}

impl FileSource for Synthetic {
    fn len(&self) -> u64 {
        (self.header.len() + self.body_len) as u64
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let offset = offset as usize;
        if offset + buf.len() > self.len() as usize {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        for (i, b) in buf.iter_mut().enumerate() {
            let at = offset + i;
            *b = match at.checked_sub(self.header.len()) {
                None => self.header[at],
                Some(body) => byte(body),
            };
        }
        Ok(())
    }
}

/// An instrument that accepts everything and keeps nothing: each read answers the last
/// frame written with success.
#[derive(Default)]
struct Sink {
    last: Option<Message>,
}

impl Transport for Sink {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.last = Some(Message::decode(buf)?);
        Ok(())
    }

    async fn read(&mut self, _max: usize) -> Result<Vec<u8>> {
        let last = self.last.as_ref().expect("a read follows a request");
        let status = 0u32.to_be_bytes().to_vec();
        Ok(Message::new(last.service, last.subsystem, last.command + 1, status).encode())
    }
}

fn byte_unit() -> AllocationUnit {
    let mut fields = 1u32.to_be_bytes().to_vec();
    fields.resize(29, 0);
    Partition {
        index: ObjectClass::Program.to_raw(),
        name: "Program".into(),
        native: false,
        fields,
    }
    .allocation_unit()
    .unwrap()
}

#[test]
fn a_streamed_write_holds_a_few_chunks_whatever_the_files_size() {
    let at = Location { bank: 0, slot: 0 };
    let body_len = 128 * CHUNK;
    let mut file = Synthetic::new(at, body_len);
    let mut sink = Sink::default();

    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    pollster::block_on(async {
        let mut s = Session::open(&mut sink, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::write_from(&mut s, byte_unit(), at, &mut file, "Big", 0)
            .await
            .expect("the sink accepts the write");
        s.commit().await.unwrap();
    });
    let held = PEAK.load(Ordering::SeqCst) - before;

    assert!(
        held <= 8 * CHUNK,
        "writing a {body_len}-byte body held {held} bytes at once"
    );
}
