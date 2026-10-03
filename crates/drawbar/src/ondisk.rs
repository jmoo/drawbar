//! An asset left in its file: a piano or sample instrument, which runs to hundreds of
//! megabytes, is read by range through its index rather than held.
//!
//! The desktop reads the file by position, through a handle held open. The browser reads
//! slices of the `File` the page took of it, and a slice answers only later: a read on
//! the frame of a range not fetched yet answers [`io::ErrorKind::WouldBlock`] and fetches
//! it, and a pass over the whole file runs as a task of its own.

use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use eframe::egui;
use nord_format::cbin::{self, Header, Verifier};
use nord_format::crc::Crc32Stream;
use nord_format::formats::{npno, nsmp};

use crate::work::{Job, Progress};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(unix)]
pub use native::raise_open_files;
#[cfg(not(target_arch = "wasm32"))]
use native::Source;
#[cfg(target_arch = "wasm32")]
pub use web::repaint_with;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::slice;
#[cfg(target_arch = "wasm32")]
use web::Source;

/// Where each stroke or zone's audio sits in the file.
#[derive(Debug)]
pub enum Index {
    Piano(npno::Index),
    Sample(nsmp::Index),
}

impl Index {
    /// How much of a file's start says whether it may be indexed: the magic and the tag.
    pub const HEAD: usize = 12;

    /// The format a file starting `head` would be indexed as, where it is a piano or
    /// sample instrument.
    fn tagged(head: &[u8]) -> Option<&'static str> {
        if head.get(..4)? != &cbin::MAGIC[..] {
            return None;
        }
        let tag = head.get(8..Index::HEAD)?;
        [npno::FORMAT, nsmp::FORMAT]
            .into_iter()
            .find(|format| format.as_bytes() == tag)
    }

    pub fn header(&self) -> &Header {
        match self {
            Index::Piano(index) => &index.library().header,
            Index::Sample(index) => index.header(),
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Index::Piano(_) => npno::FORMAT,
            Index::Sample(_) => nsmp::FORMAT,
        }
    }

    /// The index of the file `r` reads, when it is a piano or sample instrument. `None`
    /// for any other file, and for one whose index does not read, which is then read
    /// whole so that its decode says why. It fails only where a read of the file does:
    /// over [`Slices`], at a range not fetched yet.
    pub fn read(r: &mut (impl Read + Seek), len: u64) -> io::Result<Option<Index>> {
        let mut head = [0u8; Index::HEAD];
        if len < head.len() as u64 {
            return Ok(None);
        }
        r.seek(SeekFrom::Start(0))?;
        r.read_exact(&mut head)?;
        r.seek(SeekFrom::Start(0))?;
        let index = match Index::tagged(&head) {
            Some(npno::FORMAT) => npno::Index::read_from(r).map(Index::Piano),
            Some(_) => nsmp::Index::read_from(r).map(Index::Sample),
            None => return Ok(None),
        };
        match index {
            Ok(index) => Ok(Some(index)),
            Err(nord_format::error::Error::Io(e)) if Missing::of(&e).is_some() => Err(e),
            Err(_) => Ok(None),
        }
    }
}

/// A piano or sample instrument's file, left where it is, with its index.
///
/// ⚠️ On the desktop the handle follows the file through a rename, and reads the
/// contents it was indexed over until the file is replaced. A file rewritten in place
/// under it reads as whatever is there now; a rescan notices the change and indexes the
/// file again. In the browser a read of a file changed since it was indexed fails.
#[derive(Debug)]
pub struct OnDisk {
    source: Source,
    /// The file's length when it was indexed.
    pub len: u64,
    /// CRC-32 over every byte of the file, once a pass over all of it has taken it.
    crc: OnceLock<u32>,
    /// Distinct for every file indexed in this run, so two indexings of one path are told
    /// apart.
    pub serial: u64,
    pub index: Index,
    /// Every range [`OnDisk::read`] was asked for.
    #[cfg(test)]
    reads: std::sync::Mutex<Vec<Range<u64>>>,
}

impl OnDisk {
    fn indexed(source: Source, len: u64, crc: Option<u32>, index: Index) -> OnDisk {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        OnDisk {
            source,
            len,
            crc: crc.map(OnceLock::from).unwrap_or_default(),
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            index,
            #[cfg(test)]
            reads: Default::default(),
        }
    }

    /// The bytes at `range`.
    ///
    /// ⚠️ In the browser a range not fetched yet answers [`io::ErrorKind::WouldBlock`] and
    /// is fetched, and the repaint once it lands is the time to ask again. A range is
    /// handed to the read that asks for it once.
    pub fn read(&self, range: Range<u64>) -> io::Result<Vec<u8>> {
        if range.start > range.end || range.end > self.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{range:?} is not within the {}-byte file", self.len),
            ));
        }
        #[cfg(test)]
        self.reads.lock().expect("unpoisoned").push(range.clone());
        self.source.read(self.serial, range)
    }

    /// Every byte of the file, read on this thread.
    ///
    /// ⚠️ On the desktop that is hundreds of megabytes for a piano library. The browser
    /// cannot wait for a read, and refuses: [`OnDisk::whole_then`] reads it off the frame.
    pub fn whole(&self) -> io::Result<Vec<u8>> {
        #[cfg(test)]
        self.reads.lock().expect("unpoisoned").push(0..self.len);
        self.source.whole(self.len)
    }

    /// Read the whole file off the frame and hand it to `then`.
    pub fn whole_then<T: Send + 'static>(
        self: &Arc<Self>,
        ctx: &egui::Context,
        then: impl FnOnce(&Progress, Vec<u8>) -> Result<T, String> + Send + 'static,
    ) -> Job<Result<T, String>> {
        let len = self.len;
        let gathered = Gathered::with(len);
        self.pass(
            ctx,
            gathered,
            |gathered, chunk| gathered.take(chunk),
            move |progress, gathered| match gathered {
                Ok(Gathered(Ok(bytes))) if bytes.len() as u64 == len => then(progress, bytes),
                Ok(Gathered(Ok(_))) => Err("the file changed while it was read".to_string()),
                Ok(Gathered(Err(why))) | Err(why) => Err(why),
            },
        )
    }

    /// Check the file's stored checksum in one streaming pass off the frame, taking the
    /// file's own CRC-32 on the way.
    pub fn verify(self: &Arc<Self>, ctx: &egui::Context) -> Job<Result<Sums, String>> {
        let file = self.clone();
        let summing = Summing::new(self.index.header(), self.len);
        self.pass(
            ctx,
            summing,
            |summing, chunk| summing.feed(chunk),
            move |_, summed| {
                let sums = summed?.finish()?;
                let _ = file.crc.set(sums.crc);
                Ok(sums)
            },
        )
    }

    /// Feed every byte of the file to `each`, in order, a chunk at a time, off the frame,
    /// then hand what it built to `then`, or why the file did not read. The job's progress
    /// says it is reading the file until `then` says otherwise.
    fn pass<S: Send + 'static, T: Send + 'static>(
        self: &Arc<Self>,
        ctx: &egui::Context,
        state: S,
        each: impl FnMut(&mut S, &[u8]) -> Result<(), String> + Send + 'static,
        then: impl FnOnce(&Progress, Result<S, String>) -> Result<T, String> + Send + 'static,
    ) -> Job<Result<T, String>> {
        Source::pass(self, ctx, state, each, then)
    }

    /// CRC-32 over every byte of the file, taken in one streaming pass the first time it
    /// is asked for.
    ///
    /// ⚠️ On the desktop that pass reads the whole file, hundreds of megabytes for a piano
    /// library, on the calling thread. The browser cannot wait for it, and answers only a
    /// CRC already taken: [`OnDisk::crc_now`] takes one.
    pub fn crc(&self) -> io::Result<u32> {
        if let Some(crc) = self.crc.get() {
            return Ok(*crc);
        }
        let crc = self.source.crc(self.len)?;
        Ok(*self.crc.get_or_init(|| crc))
    }

    /// [`OnDisk::crc`], in a task that may wait for the browser's slices.
    pub async fn crc_now(&self) -> io::Result<u32> {
        if let Some(crc) = self.crc.get() {
            return Ok(*crc);
        }
        let crc = self.source.crc_now(self.serial, self.len).await?;
        Ok(*self.crc.get_or_init(|| crc))
    }

    /// The CRC, where a pass has taken it already.
    pub fn known_crc(&self) -> Option<u32> {
        self.crc.get().copied()
    }

    /// Whether `bytes` are what the file held when it was indexed. Takes the file's CRC
    /// if nothing has yet, where this target can on the calling thread.
    pub fn holds(&self, bytes: &[u8]) -> bool {
        self.len == bytes.len() as u64
            && self
                .crc()
                .is_ok_and(|crc| crc == nord_format::crc::crc32(bytes))
    }

    /// The ranges read so far, a read of the whole file among them, emptied.
    #[cfg(test)]
    pub fn take_reads(&self) -> Vec<Range<u64>> {
        std::mem::take(&mut self.reads.lock().expect("unpoisoned"))
    }
}

/// How much a streaming pass reads at a time.
const CHUNK: usize = 4 << 20;

/// The bytes of a whole read, gathered chunk by chunk, or why there is no room for them.
struct Gathered(Result<Vec<u8>, String>);

impl Gathered {
    fn with(len: u64) -> Gathered {
        let unfit = || format!("its {len} bytes do not fit in memory");
        let mut bytes = Vec::new();
        let room = usize::try_from(len)
            .ok()
            .and_then(|len| bytes.try_reserve_exact(len).ok());
        Gathered(room.map(|_| bytes).ok_or_else(unfit))
    }

    fn take(&mut self, chunk: &[u8]) -> Result<(), String> {
        match &mut self.0 {
            Ok(bytes) if bytes.len() + chunk.len() <= bytes.capacity() => {
                bytes.extend_from_slice(chunk);
                Ok(())
            }
            Ok(_) => Err("the file changed while it was read".to_string()),
            Err(why) => Err(why.clone()),
        }
    }
}

/// What a checksum pass found: the file's own CRC-32, and what the container says of its
/// body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sums {
    /// CRC-32 over every byte of the file.
    pub crc: u32,
    /// Where the body sits in the file, a type-0 container's trailing checksum left out.
    pub body: Range<u64>,
    /// The checksum the container stores: the type-1 CRC-32, or the type-0 CRC-16 widened.
    pub stored: u32,
    /// Whether `stored` is the checksum of what it covers.
    pub checksum_ok: bool,
    /// CRC-32 over the body, which the instrument reports for a slot.
    pub body_crc32: u32,
}

/// A checksum pass in flight, fed the file's bytes in order: [`cbin::Verifier`]'s
/// verdict, with the CRC-32s of the whole file and of its body.
struct Summing {
    len: u64,
    /// How many bytes have been fed.
    at: u64,
    body: Range<u64>,
    file: Crc32Stream<'static>,
    body_crc: Crc32Stream<'static>,
    verifier: Verifier,
}

impl Summing {
    fn new(header: &Header, len: u64) -> Summing {
        let generation = header.generation;
        let start = generation.body_start().min(len);
        // A checksum stored past the body's start trails it.
        let end = usize::try_from(len)
            .ok()
            .and_then(|len| generation.checksum_range(len))
            .map(|stored| stored.start as u64)
            .filter(|&stored| stored >= start)
            .unwrap_or(len);
        Summing {
            len,
            at: 0,
            body: start..end,
            file: Crc32Stream::new(),
            body_crc: Crc32Stream::new(),
            verifier: Verifier::new(),
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), String> {
        let span = self.at..self.at + chunk.len() as u64;
        let start = self.body.start.clamp(span.start, span.end) - span.start;
        let end = self.body.end.clamp(span.start, span.end) - span.start;
        self.file.update(chunk);
        self.body_crc.update(&chunk[start as usize..end as usize]);
        self.verifier.update(chunk).map_err(|e| e.to_string())?;
        self.at = span.end;
        Ok(())
    }

    fn finish(self) -> Result<Sums, String> {
        if self.at != self.len {
            return Err("the file changed while it was read".to_string());
        }
        let info = self.verifier.finish().map_err(|e| e.to_string())?;
        if self.body.end - self.body.start != info.body_len {
            return Err(format!(
                "the {}-byte body was taken as {:?}",
                info.body_len, self.body
            ));
        }
        Ok(Sums {
            crc: self.file.value(),
            body: self.body,
            stored: info.stored_checksum,
            checksum_ok: info.checksum_ok,
            body_crc32: self.body_crc.value(),
        })
    }
}

/// CRC-32 over everything `r` yields.
pub fn crc_of(r: &mut impl Read) -> io::Result<u32> {
    let mut crc = Crc32Stream::new();
    let mut chunk = vec![0u8; CHUNK];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => return Ok(crc.value()),
            Ok(n) => crc.update(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// A range of a file a read over [`Slices`] needed and was not fetched: from the first
/// byte missing to the end of the read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing(pub Range<u64>);

impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bytes {}..{} are not read yet", self.0.start, self.0.end)
    }
}

impl std::error::Error for Missing {}

impl Missing {
    fn error(range: Range<u64>) -> io::Error {
        io::Error::new(io::ErrorKind::WouldBlock, Missing(range))
    }

    /// The range an error names, where it is a range not fetched yet.
    pub fn of(e: &io::Error) -> Option<Range<u64>> {
        let missing = e.get_ref()?.downcast_ref::<Missing>()?;
        Some(missing.0.clone())
    }
}

/// The least a fetch for an index takes. A piano's container header, prefix and stroke
/// directory sit together at its start, and a sample's section header beside the
/// opening of its stroke, so a fetch this size usually holds all of a piano's index
/// and one stroke's part of a sample's.
pub const FETCH: u64 = 64 << 10;

/// The parts of a file fetched so far, read as the file itself: a reader over them
/// reports the file's whole length, and a read reaching a byte not fetched fails with
/// [`Missing`], naming the range it needed. Fetching that range and reading again
/// converges on whatever the reads need.
#[derive(Debug)]
pub struct Slices {
    len: u64,
    held: BTreeMap<u64, Vec<u8>>,
}

impl Slices {
    pub fn new(len: u64) -> Slices {
        Slices {
            len,
            held: BTreeMap::new(),
        }
    }

    /// The range to fetch for a read that needed `missing`: at least [`FETCH`] bytes
    /// from its start, within the file.
    pub fn widen(&self, missing: &Range<u64>) -> Range<u64> {
        let end = missing.end.max(missing.start.saturating_add(FETCH));
        missing.start..end.min(self.len)
    }

    /// Hold the bytes fetched at `range`. Bytes of another length are refused, since the
    /// file is then not the one the reads are over.
    pub fn hold(&mut self, range: Range<u64>, bytes: Vec<u8>) -> io::Result<()> {
        if range.end > self.len || range.end - range.start != bytes.len() as u64 {
            return Err(io::Error::other("the file changed while it was read"));
        }
        self.held.insert(range.start, bytes);
        Ok(())
    }

    pub fn reader(&self) -> SliceReader<'_> {
        SliceReader {
            slices: self,
            pos: 0,
        }
    }

    /// Read `f` over the slices, fetching each range it is missing with `fetch` and
    /// reading again, until it answers. A fetch that fails ends it.
    #[cfg(test)]
    pub fn converge<T>(
        &mut self,
        mut fetch: impl FnMut(Range<u64>) -> io::Result<Vec<u8>>,
        mut f: impl FnMut(&mut SliceReader<'_>) -> io::Result<T>,
    ) -> io::Result<T> {
        loop {
            let missing = match f(&mut self.reader()) {
                Err(e) => Missing::of(&e).ok_or(e)?,
                answer => return answer,
            };
            let range = self.widen(&missing);
            let bytes = fetch(range.clone())?;
            self.hold(range, bytes)?;
        }
    }
}

/// A reader over [`Slices`].
pub struct SliceReader<'a> {
    slices: &'a Slices,
    pos: u64,
}

impl Read for SliceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.slices.len;
        if self.pos >= len || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(len - self.pos);
        let held = self.slices.held.range(..=self.pos).next_back();
        let Some((&at, bytes)) = held.filter(|(&at, bytes)| self.pos < at + bytes.len() as u64)
        else {
            return Err(Missing::error(self.pos..self.pos + want));
        };
        let from = (self.pos - at) as usize;
        let n = (bytes.len() - from).min(want as usize);
        buf[..n].copy_from_slice(&bytes[from..from + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SliceReader<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = seek(self.pos, self.slices.len, to)?;
        Ok(self.pos)
    }
}

/// Where a seek from `pos` in a file of `len` bytes lands.
fn seek(pos: u64, len: u64, to: SeekFrom) -> io::Result<u64> {
    let landed = match to {
        SeekFrom::Start(at) => Some(at),
        SeekFrom::End(delta) => len.checked_add_signed(delta),
        SeekFrom::Current(delta) => pos.checked_add_signed(delta),
    };
    landed.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a seek before the file's start",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Index `bytes` over slices fetched as the reads need them, and count the fetches.
    fn converged(bytes: &[u8]) -> (Option<Index>, usize, u64) {
        let mut slices = Slices::new(bytes.len() as u64);
        let mut fetches = 0;
        let index = slices
            .converge(
                |range| {
                    fetches += 1;
                    Ok(bytes[range.start as usize..range.end as usize].to_vec())
                },
                |r| Index::read(r, bytes.len() as u64),
            )
            .expect("every fetch answers");
        let fetched = slices.held.values().map(|held| held.len() as u64).sum();
        (index, fetches, fetched)
    }

    fn fixture(path: &str) -> Vec<u8> {
        let at = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../nord-format/tests/fixtures/"
        );
        std::fs::read(format!("{at}{path}")).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    #[test]
    fn an_index_read_over_fetched_slices_is_the_index_of_the_whole_file() {
        for path in [
            "npno/tone.npno",
            "demo/drawbar-tine.npno",
            "nsmp/tone.nsmp3",
            "demo/drawbar-pad.nsmp",
            "demo/drawbar-pad.nsmp4",
        ] {
            let bytes = fixture(path);
            let whole = Index::read(&mut Cursor::new(&bytes), bytes.len() as u64)
                .unwrap()
                .unwrap_or_else(|| panic!("{path} indexes"));
            let (sliced, _, _) = converged(&bytes);
            let sliced = sliced.unwrap_or_else(|| panic!("{path} indexes over slices"));
            match (whole, sliced) {
                (Index::Piano(whole), Index::Piano(sliced)) => {
                    assert_eq!(whole.audio_ranges(), sliced.audio_ranges(), "{path}")
                }
                (Index::Sample(whole), Index::Sample(sliced)) => {
                    assert_eq!(whole.zones(), sliced.zones(), "{path}")
                }
                _ => panic!("{path} indexed as two kinds"),
            }
        }
    }

    /// The index takes only the bytes around its own reads, never the audio between
    /// them.
    #[test]
    fn a_piano_index_over_slices_fetches_its_directory_and_no_audio() {
        use nord_format::formats::npno::synthetic::{take, Build};
        use nord_format::formats::npno::Bank;

        let bytes = Build {
            takes: vec![
                take(60, Bank::Attack, 0, 4000),
                take(72, Bank::Attack, 0, 4000),
            ],
            map: vec![(60, 60), (72, 72)],
            ..Build::new()
        }
        .bytes()
        .expect("the builder lays out a library");
        let Some(Index::Piano(index)) =
            Index::read(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap()
        else {
            panic!("a piano")
        };
        let first_audio = index.audio_ranges()[0].start;
        assert!(
            bytes.len() as u64 > first_audio + 4 * FETCH,
            "audio that would show"
        );
        let (_, fetches, fetched) = converged(&bytes);
        assert!(fetches <= 3, "{fetches} fetches");
        assert!(
            fetched <= first_audio + FETCH,
            "{fetched} bytes fetched, audio starting at {first_audio}"
        );
    }

    #[test]
    fn a_read_past_what_was_fetched_names_the_range_it_needs() {
        let mut slices = Slices::new(100);
        slices.hold(10..20, vec![7; 10]).unwrap();
        let mut reader = slices.reader();
        reader.seek(SeekFrom::Start(15)).unwrap();
        let mut buf = [0u8; 10];
        let e = reader.read_exact(&mut buf).unwrap_err();
        assert_eq!(Missing::of(&e), Some(20..25));
        assert_eq!(
            reader.seek(SeekFrom::End(0)).unwrap(),
            100,
            "the file's length"
        );
        assert!(slices.hold(90..101, vec![0; 11]).is_err(), "past the end");
        assert!(slices.hold(0..10, vec![0; 9]).is_err(), "short");
    }

    /// The pass checks what `cbin::inspect` checks, of a type-1 file and a type-0 one,
    /// whole and with a byte of the body changed.
    #[test]
    fn a_checksum_pass_agrees_with_inspect() {
        let sample = crate::testing::sample_bytes();
        let type_0 = crate::workspace::as_type_0(&sample);
        for (name, bytes) in [("type 1", sample), ("type 0", type_0)] {
            let mut broken = bytes.clone();
            let at = broken.len() - 3;
            broken[at] ^= 0x40;
            for bytes in [bytes, broken] {
                let info = cbin::inspect(&mut Cursor::new(&bytes)).unwrap();
                let mut summing = Summing::new(&info.header, bytes.len() as u64);
                for chunk in bytes.chunks(7) {
                    summing.feed(chunk).unwrap();
                }
                let sums = summing.finish().unwrap();
                assert_eq!(sums.body.end - sums.body.start, info.body_len, "{name}");
                assert_eq!(sums.checksum_ok, info.checksum_ok, "{name}");
                assert_eq!(sums.stored, info.stored_checksum, "{name}");
                assert_eq!(sums.crc, nord_format::crc::crc32(&bytes), "{name}");
                let body = info.header.generation.body_start() as usize
                    ..info.header.generation.body_start() as usize + info.body_len as usize;
                let body = nord_format::crc::crc32(&bytes[body]);
                assert_eq!(sums.body_crc32, body, "{name}");
            }
        }
    }
}
