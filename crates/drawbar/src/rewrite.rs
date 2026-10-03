//! An edit of a piano or sample instrument resting in its file, held as what it changes
//! and written by copying the file through with the edit in place, so neither the file
//! nor the edited copy is ever held whole.
//!
//! The desktop copies through a handle that reads by position. The browser cannot wait
//! for a read, and a writer that reads as it writes must wait for each, so there the
//! copy is laid out as [`Pieces`] first, and [`stream`] fetches each kept range as the
//! writer takes it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::{AsyncFnMut, Range};

use nord_format::cbin::Verifier;
use nord_format::formats::{npno, nsmp};

use crate::ondisk::{self, OnDisk};

/// What an edit changes of a file resting in the library.
#[derive(Debug)]
pub enum Rewrite {
    /// A sample's fields outside its audio. The sections they changed are spliced into
    /// a copy of the file, with the checksum restated.
    Sample(nsmp::Outline),
    /// A piano library laid out again from its index. Each stroke it keeps is read from
    /// the file by its range, and the checksum is taken as the copy is written.
    Piano(npno::Library<'static>),
}

impl Rewrite {
    /// Write the file this edit makes of `from` to `out`, reading what it keeps of `from`
    /// by range.
    ///
    /// ⚠️ `from` must be the file the edit was made over. A file whose bytes are not the
    /// ones its index read is refused with [`changed`], though `out` holds the copy.
    pub fn write_from(
        &self,
        from: &mut (impl Read + Seek),
        index: &ondisk::Index,
        out: &mut (impl Write + Seek),
    ) -> io::Result<()> {
        match (self, index) {
            (Rewrite::Sample(outline), ondisk::Index::Sample(index)) => {
                let patch = index.patch(outline).map_err(io::Error::other)?;
                patch.copy(from, out).map_err(|e| match e {
                    nord_format::error::Error::Io(e) => e,
                    e => changed(e.to_string()),
                })
            }
            (Rewrite::Piano(library), ondisk::Index::Piano(_)) => {
                library.write_from(out, from).map_err(|e| match e {
                    nord_format::error::Error::Io(e) => e,
                    e => io::Error::other(e.to_string()),
                })
            }
            _ => Err(not_ours()),
        }
    }

    /// [`Rewrite::write_from`] for a file resting on the desktop, read through its handle.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write(&self, from: &OnDisk, out: &mut (impl Write + Seek)) -> io::Result<()> {
        self.write_from(&mut from.at(), &from.index, out)
    }

    /// The file this edit makes of `from`, as the bytes it holds and the ranges of `from`
    /// it keeps, in order.
    pub fn pieces(&self, from: &OnDisk) -> io::Result<Pieces> {
        match (self, &from.index) {
            (Rewrite::Sample(outline), ondisk::Index::Sample(index)) => {
                let patch = index.patch(outline).map_err(io::Error::other)?;
                let mut parts = Vec::new();
                let mut at = 0;
                for splice in patch.splices() {
                    parts.push(Piece::Kept(at..splice.at));
                    parts.push(Piece::Held(splice.bytes.clone()));
                    at = splice.at + splice.bytes.len() as u64;
                }
                parts.push(Piece::Kept(at..from.len));
                parts.retain(|part| !part.is_empty());
                Ok(Pieces {
                    parts,
                    sealed: true,
                })
            }
            (Rewrite::Piano(library), ondisk::Index::Piano(_)) => laid_out(library),
            _ => Err(not_ours()),
        }
    }
}

/// The pieces of the file [`npno::Library::write_from`] writes: everything it writes but
/// the audio as it comes, and each stroke's audio as the range of the source it is read
/// from. The checksum it stores is the one of a copy with no audio, and [`stream`] seals
/// the copy again.
fn laid_out(library: &npno::Library<'static>) -> io::Result<Pieces> {
    let strokes = RefCell::new(VecDeque::new());
    let mut recorder = Recorder {
        strokes: &strokes,
        parts: Vec::new(),
        at: 0,
        end: 0,
    };
    library
        .write_from(&mut recorder, &mut Strokes(&strokes))
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(Pieces {
        parts: recorder.parts,
        sealed: false,
    })
}

/// The ranges [`npno::Library::write_from`] reads its strokes' audio from, in the order it
/// writes them. Nothing is read.
struct Strokes<'a>(&'a RefCell<VecDeque<Range<u64>>>);

impl npno::AudioSource for Strokes<'_> {
    fn read_audio(&mut self, at: u64, buf: &mut [u8]) -> io::Result<()> {
        let range = at..at + buf.len() as u64;
        self.0.borrow_mut().push_back(range);
        Ok(())
    }
}

/// A writer that keeps what it is given as [`Piece`]s: the bytes written after a stroke
/// is read, up to its length, as that stroke's range, and anything else as it is.
struct Recorder<'a> {
    strokes: &'a RefCell<VecDeque<Range<u64>>>,
    parts: Vec<Piece>,
    /// Where the next write lands.
    at: u64,
    /// How many bytes have been written.
    end: u64,
}

impl Recorder<'_> {
    /// Write `bytes` over what was written at `self.at`, which they must not run past.
    fn over(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut start = 0;
        for part in &mut self.parts {
            let len = part.len();
            let (from, to) = (
                self.at.max(start),
                (self.at + bytes.len() as u64).min(start + len),
            );
            if from < to {
                let Piece::Held(held) = part else {
                    return Err(io::Error::other("a write back over a stroke's audio"));
                };
                let (from, to) = ((from - start) as usize, (to - start) as usize);
                let given = (start + from as u64 - self.at) as usize;
                held[from..to].copy_from_slice(&bytes[given..given + (to - from)]);
            }
            start += len;
        }
        Ok(())
    }

    fn push(&mut self, piece: Piece) {
        match (self.parts.last_mut(), piece) {
            (Some(Piece::Held(held)), Piece::Held(bytes)) => held.extend_from_slice(&bytes),
            (Some(Piece::Kept(kept)), Piece::Kept(range)) if kept.end == range.start => {
                kept.end = range.end
            }
            (_, piece) => self.parts.push(piece),
        }
    }
}

impl Write for Recorder<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.at < self.end {
            if self.at + bytes.len() as u64 > self.end {
                return Err(io::Error::other(
                    "a write back past the end of what was written",
                ));
            }
            self.over(bytes)?;
            self.at += bytes.len() as u64;
            return Ok(bytes.len());
        }
        let mut rest = bytes;
        while !rest.is_empty() {
            let mut strokes = self.strokes.borrow_mut();
            let Some(stroke) = strokes.front_mut() else {
                drop(strokes);
                self.push(Piece::Held(rest.to_vec()));
                break;
            };
            let n = rest.len().min((stroke.end - stroke.start) as usize);
            let kept = stroke.start..stroke.start + n as u64;
            stroke.start = kept.end;
            if stroke.start == stroke.end {
                strokes.pop_front();
            }
            drop(strokes);
            self.push(Piece::Kept(kept));
            rest = &rest[n..];
        }
        self.at += bytes.len() as u64;
        self.end = self.at;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Recorder<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let at = match to {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(delta) => self.end.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.at.checked_add_signed(delta),
        };
        self.at = at
            .filter(|at| *at <= self.end)
            .ok_or_else(|| io::Error::other("a seek outside what was written"))?;
        Ok(self.at)
    }
}

fn not_ours() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "the edit is of another kind of file",
    )
}

/// A file laid out as the bytes an edit holds and the ranges of its source it keeps.
#[derive(Debug)]
pub struct Pieces {
    parts: Vec<Piece>,
    /// Whether the checksum the pieces store is the copy's own. Otherwise [`stream`]
    /// takes it as the copy passes and writes it last.
    sealed: bool,
}

#[derive(Debug)]
enum Piece {
    Held(Vec<u8>),
    Kept(Range<u64>),
}

impl Piece {
    fn len(&self) -> u64 {
        match self {
            Piece::Held(bytes) => bytes.len() as u64,
            Piece::Kept(range) => range.end.saturating_sub(range.start),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// How much of a kept range is read at once.
const CHUNK: u64 = 4 << 20;

/// Write the file `pieces` lay out through `write`, in order and a chunk at a time,
/// reading each kept range with `read`. Each call to `write` names where its bytes go.
///
/// Every byte written is checked as a container as it passes. Where the pieces carry
/// their checksum, a copy that does not match it is refused with [`changed`] once the
/// last byte is written: the source is not the file the edit was made over. Otherwise
/// the checksum the copy calls for is written last, over the one it stores.
pub async fn stream(
    pieces: &Pieces,
    mut read: impl AsyncFnMut(Range<u64>) -> io::Result<Vec<u8>>,
    mut write: impl AsyncFnMut(u64, Vec<u8>) -> io::Result<()>,
) -> io::Result<()> {
    let mut verifier = Verifier::new();
    let mut at = 0;
    for part in &pieces.parts {
        match part {
            Piece::Held(bytes) => {
                verifier.update(bytes).map_err(io::Error::other)?;
                write(at, bytes.clone()).await?;
                at += bytes.len() as u64;
            }
            Piece::Kept(range) => {
                let mut from = range.start;
                while from < range.end {
                    let until = range.end.min(from + CHUNK);
                    let bytes = read(from..until).await?;
                    if bytes.len() as u64 != until - from {
                        return Err(changed("the file changed while it was read".into()));
                    }
                    verifier.update(&bytes).map_err(io::Error::other)?;
                    let len = bytes.len() as u64;
                    write(at, bytes).await?;
                    at += len;
                    from = until;
                }
            }
        }
    }
    if !pieces.sealed {
        let seal = verifier.seal().map_err(io::Error::other)?;
        return write(seal.at, seal.bytes).await;
    }
    let info = verifier.finish().map_err(io::Error::other)?;
    match info.checksum_ok {
        true => Ok(()),
        false => Err(changed(
            "the copy does not match its checksum: the file is not the one the edit was made \
             over"
                .into(),
        )),
    }
}

/// Why a copy was refused: the file it read is not the one the edit was made over.
#[derive(Debug)]
struct Changed(String);

impl std::fmt::Display for Changed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Changed {}

/// An error saying the file a copy read changed since its index was read.
pub fn changed(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, Changed(why))
}

/// Whether `e` says the file a copy read changed since its index was read.
pub fn is_changed(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<Changed>())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::io::{Cursor, SeekFrom};

    use nord_format::formats::nsmp::codec::Layout;

    use super::*;
    use crate::testing::{on_disk, zoned_sample, Temp};

    /// A reader over bytes that records the span of every read.
    struct Recording<'a> {
        inner: Cursor<&'a [u8]>,
        reads: Vec<Range<u64>>,
    }

    impl Read for Recording<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let at = self.inner.position();
            let n = self.inner.read(buf)?;
            self.reads.push(at..at + n as u64);
            Ok(n)
        }
    }

    impl Seek for Recording<'_> {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.inner.seek(to)
        }
    }

    /// Every generation of a three-zone instrument, and the same instrument renamed and
    /// with its middle zone's root moved, as a whole decode writes it.
    fn edited() -> Vec<(Vec<u8>, Rewrite, Vec<u8>)> {
        let sets = [
            ("name".to_string(), "Vibes".to_string()),
            ("zone2.root_key".to_string(), "D4".to_string()),
        ];
        [Layout::V2, Layout::V3, Layout::V4]
            .into_iter()
            .map(|layout| {
                let bytes = zoned_sample(layout, 4 * 92);
                let index =
                    nsmp::Index::read_from(&mut Cursor::new(&bytes)).expect("the file indexes");
                let mut outline = index.outline().clone();
                outline.set_name("Vibes").unwrap();
                outline.set_root_key(1, 62).unwrap();
                let whole = crate::document::sample::apply(&bytes, &sets).unwrap();
                (bytes, Rewrite::Sample(outline), whole)
            })
            .collect()
    }

    /// The bytes [`stream`] writes of `pieces` over `source`, and the ranges it read.
    fn streamed(pieces: &Pieces, source: &[u8]) -> (io::Result<Vec<u8>>, Vec<Range<u64>>) {
        let mut out = Vec::new();
        let mut reads = Vec::new();
        let read = async |range: Range<u64>| {
            reads.push(range.clone());
            Ok(source[range.start as usize..range.end as usize].to_vec())
        };
        let write = async |at: u64, bytes: Vec<u8>| {
            let at = at as usize;
            out.resize(out.len().max(at + bytes.len()), 0);
            out[at..at + bytes.len()].copy_from_slice(&bytes);
            Ok(())
        };
        let wrote = nord_usb::block_on(stream(pieces, read, write));
        (wrote.map(|()| out), reads)
    }

    /// A sample's edit, written through its file by range on the desktop or laid out in
    /// pieces for the browser, makes the bytes a whole decode, the same sets and a whole
    /// write make, reading what it keeps a chunk at a time.
    #[test]
    fn an_edit_written_through_its_file_is_the_file_a_whole_edit_writes() {
        for (bytes, edit, whole) in edited() {
            let dir = Temp::new();
            let file = on_disk(&dir, "Zoned.nsmp", &bytes);
            let mut source = Recording {
                inner: Cursor::new(&bytes),
                reads: Vec::new(),
            };
            let mut out = Cursor::new(Vec::new());
            edit.write_from(&mut source, &file.index, &mut out).unwrap();
            assert!(out.into_inner() == whole, "the desktop's copy");
            let most = source.reads.iter().map(|read| read.end - read.start).max();
            assert!(most <= Some(64 << 10), "reads of at most {most:?} bytes");

            let pieces = edit.pieces(&file).unwrap();
            let (out, reads) = streamed(&pieces, &bytes);
            assert!(out.unwrap() == whole, "the browser's copy");
            let kept: u64 = reads.iter().map(|read| read.end - read.start).sum();
            assert!(kept < bytes.len() as u64, "it holds what the edit changed");
        }
    }

    /// A file changed under its index since it was read is refused, however the copy
    /// reads it: the copy's checksum is not the one the edit restated.
    #[test]
    fn a_file_changed_since_its_index_was_read_is_refused() {
        for (bytes, edit, _) in edited() {
            let dir = Temp::new();
            let file = on_disk(&dir, "Zoned.nsmp", &bytes);
            let mut changed = bytes.clone();
            let last = changed.len() - 3;
            changed[last] ^= 0x40;

            let e = edit
                .write_from(
                    &mut Cursor::new(&changed),
                    &file.index,
                    &mut Cursor::new(Vec::new()),
                )
                .unwrap_err();
            assert!(is_changed(&e), "{e}");
            let (e, _) = streamed(&edit.pieces(&file).unwrap(), &changed);
            let e = e.unwrap_err();
            assert!(is_changed(&e), "{e}");
        }
    }

    /// A piano library of three roots with two layers each, every stroke a different
    /// length.
    fn piano() -> Vec<u8> {
        use nord_format::formats::npno::synthetic::{take, Build};
        use nord_format::formats::npno::Bank;

        let mut takes = Vec::new();
        for (i, root) in [48u8, 60, 72].into_iter().enumerate() {
            let blocks = 3 + i as u16;
            takes.push(take(root, Bank::Attack, 0, blocks));
            takes.push(take(root, Bank::Attack, 1, blocks + 4));
            takes.push(take(root, Bank::Release, 0, 2));
        }
        let map = (40..=80)
            .map(|key: u8| {
                (
                    key,
                    [48u8, 60, 72][usize::from(key.saturating_sub(40) / 14)],
                )
            })
            .collect();
        Build {
            takes,
            map,
            ..Build::new()
        }
        .bytes()
        .expect("the builder lays out a library")
    }

    /// A plan's edits, made of a library however it was read.
    fn planned(library: &mut npno::Library<'_>) {
        library.set_name("Trimmed").unwrap();
        library.drop_bank(npno::Bank::Release);
        library.retain_strokes(|stroke| stroke.root != 60 || stroke.layer() == 0);
        library.set_trim(1, 6).unwrap();
    }

    /// A piano plan written through its file reads each kept stroke by its range, once,
    /// and writes the library a whole read and a whole layout write, on the desktop and
    /// laid out in pieces for the browser alike. The browser's copy takes its checksum as
    /// it streams.
    #[test]
    fn a_piano_plan_written_through_its_file_is_the_library_a_whole_layout_writes() {
        let bytes = piano();
        let mut whole = npno::Library::borrow(&bytes).unwrap();
        planned(&mut whole);
        let whole =
            nord_format::to_bytes(&nord_format::Entity::Piano(whole.to_piano().unwrap())).unwrap();

        let dir = Temp::new();
        let file = on_disk(&dir, "Grand.npno", &bytes);
        let ondisk::Index::Piano(index) = &file.index else {
            panic!("a piano library")
        };
        let mut library = index.library().clone();
        planned(&mut library);
        let kept: Vec<Range<u64>> = library
            .strokes()
            .iter()
            .map(|stroke| {
                let at = index
                    .library()
                    .strokes()
                    .iter()
                    .position(|held| held.id() == stroke.id());
                index.audio_ranges()[at.expect("a kept stroke")].clone()
            })
            .collect();
        let edit = Rewrite::Piano(library);

        let mut source = Recording {
            inner: Cursor::new(&bytes),
            reads: Vec::new(),
        };
        let mut out = Cursor::new(Vec::new());
        edit.write_from(&mut source, &file.index, &mut out).unwrap();
        assert!(out.into_inner() == whole, "the desktop's copy");
        assert_eq!(source.reads, kept, "one read of each kept stroke");

        let pieces = edit.pieces(&file).unwrap();
        let (out, reads) = streamed(&pieces, &bytes);
        assert!(out.unwrap() == whole, "the browser's copy");
        let mut runs: Vec<Range<u64>> = Vec::new();
        for range in kept {
            match runs.last_mut() {
                Some(run) if run.end == range.start => run.end = range.end,
                _ => runs.push(range),
            }
        }
        assert_eq!(reads, runs, "one read of each run of kept strokes");
    }
}
