//! The `CBIN` container: one type owning what every Nord file format shares — the
//! header (both generations), the checksum policy, and the length bookkeeping.
//!
//! A format module contributes a [`Body`]: the bytes after the header, decoded from
//! a [`BodyReader`] scoped so that position 0 is the first body byte. Bodies never
//! see the header layout, the checksum, or the generation — which is what makes a
//! type-0 format the same amount of work as a type-1 format.
//!
//! The layouts, inferred from specimens across the corpus and not confirmed on
//! hardware:
//!
//! | offset | type 0 | type 1 |
//! |---|---|---|
//! | `0x00` | `"CBIN"` | `"CBIN"` |
//! | `0x04` | 0, LE u32 | 1 |
//! | `0x08` | tag | tag |
//! | `0x0c` | location | location |
//! | `0x10` | aux | aux |
//! | `0x14` | version | version |
//! | `0x18` | body… | crc32 over the body, 16 zero bytes, body at `0x2c` |
//! | EOF−2 | crc16 over every byte before it, LE | — |
//!
//! The container reads and writes in one forward pass and holds O(1) state, so a
//! 42MB library costs the same memory as a 165-byte program. Whether a body
//! allocates is the body's own choice.

use crate::crc::{Crc16Stream, Crc32Stream};
use crate::error::{Error, ParseError};
use std::io::{self, Read, Seek, SeekFrom, Write};

pub const MAGIC: &[u8; 4] = b"CBIN";

/// Bytes of the fields both generations share: magic through version.
const HEAD_LEN: usize = 0x18;

/// The two header layouts, named by the u32 at `0x04`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    V0,
    V1,
}

impl Generation {
    /// Offset of the first body byte.
    pub fn body_start(self) -> u64 {
        match self {
            Generation::V0 => 0x18,
            Generation::V1 => 0x2c,
        }
    }

    /// Bytes of checksum after the body: the type-0 crc16 trails the file.
    fn trailer_len(self) -> u64 {
        match self {
            Generation::V0 => 2,
            Generation::V1 => 0,
        }
    }
}

/// The tag at `0x08`, kept as raw bytes.
///
/// ⚠️ Three tags are three characters plus a NUL (`nsp\0`, `nss\0`, `nwp\0`), so a
/// format spells all four bytes and nothing pads implicitly.
pub type Tag = [u8; 4];

/// `format` as its 4-byte tag. A format constant of any other length is a bug in
/// the format module, not a file condition, hence the panic.
fn tag(format: &str) -> Tag {
    format
        .as_bytes()
        .try_into()
        .unwrap_or_else(|_| panic!("format tag {format:?} is not 4 bytes"))
}

fn tag_str(tag: &Tag) -> String {
    String::from_utf8_lossy(tag).into_owned()
}

/// The five fields both generations carry, verbatim.
///
/// No asserts live here: what `location` and `aux` mean is per format — a
/// bank/slot pair on programs, a library location on samples, `0xFFFFFFFF` where
/// unset — so the container preserves them and the format modules interpret them.
#[derive(Clone, PartialEq, Eq)]
pub struct Header {
    pub generation: Generation,
    pub tag: Tag,
    /// u32 at `0x0c`. On slot-addressed formats the low u16 is the bank and the
    /// high u16 the slot; see [`Header::slot`].
    pub location: u32,
    /// u32 at `0x10`. `0xFFFFFFFF` on every slot-addressed specimen; other values
    /// on sample libraries. Meaning unknown; preserved verbatim.
    pub aux: u32,
    /// u32 at `0x14`: a format's schema version (`ne5p` holds 4) or a library's
    /// content version (`nsmp` holds format×100 + revision).
    pub version: u32,
}

impl Header {
    /// A fresh type-1 header — the generation every current device writes — with
    /// `aux` at the `0xFFFFFFFF` the slot-addressed formats hold.
    pub fn new(format: &str, location: (u16, u16), version: u32) -> Header {
        Header {
            generation: Generation::V1,
            tag: tag(format),
            location: (location.0 as u32) | ((location.1 as u32) << 16),
            aux: 0xFFFF_FFFF,
            version,
        }
    }

    /// The location as the (bank, slot) pair slot-addressed formats store there.
    pub fn slot(&self) -> (u16, u16) {
        (self.location as u16, (self.location >> 16) as u16)
    }

    pub fn set_slot(&mut self, (bank, slot): (u16, u16)) {
        self.location = (bank as u32) | ((slot as u32) << 16);
    }

    /// The shared `0x18` header bytes, as they appear on disk.
    fn head_bytes(&self) -> [u8; HEAD_LEN] {
        let generation: u32 = match self.generation {
            Generation::V0 => 0,
            Generation::V1 => 1,
        };
        let mut out = [0u8; HEAD_LEN];
        out[0..4].copy_from_slice(MAGIC);
        out[4..8].copy_from_slice(&generation.to_le_bytes());
        out[8..12].copy_from_slice(&self.tag);
        out[12..16].copy_from_slice(&self.location.to_le_bytes());
        out[16..20].copy_from_slice(&self.aux.to_le_bytes());
        out[20..24].copy_from_slice(&self.version.to_le_bytes());
        out
    }
}

impl std::fmt::Debug for Header {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Header")
            .field("generation", &self.generation)
            .field("tag", &tag_str(&self.tag))
            .field("location", &format_args!("{:#010x}", self.location))
            .field("aux", &format_args!("{:#010x}", self.aux))
            .field("version", &self.version)
            .finish()
    }
}

/// The bytes after the header, as one format decodes them.
pub trait Body: Sized {
    /// Fixed body length, when the format has one; checked on read and write.
    const LEN: Option<u64> = None;

    /// Decode from `r`, which is scoped to the body: position 0 is the first body
    /// byte and [`BodyReader::len`] is known. `header` is for version gating and
    /// `location`/`aux` interpretation, not for layout — the container already
    /// consumed the header bytes.
    fn read<R: Read + Seek>(r: &mut BodyReader<'_, R>, header: &Header) -> Result<Self, Error>;

    /// Encode to `w` in one forward pass.
    fn write<W: Write + Seek>(&self, w: &mut BodyWriter<'_, W>) -> Result<(), Error>;
}

/// One decoded file: its header and its body.
///
/// Derefs to the body: the body *is* the entity, and the container is its file
/// identity, so `file.center_panel` reads as the entity access it is.
#[derive(Debug)]
pub struct Cbin<B> {
    pub header: Header,
    pub body: B,
}

impl<B> std::ops::Deref for Cbin<B> {
    type Target = B;

    fn deref(&self) -> &B {
        &self.body
    }
}

impl<B> std::ops::DerefMut for Cbin<B> {
    fn deref_mut(&mut self) -> &mut B {
        &mut self.body
    }
}

/// One checksum accumulator, whichever the generation uses.
enum Hash {
    V0(Crc16Stream<'static>),
    V1(Crc32Stream<'static>),
}

impl Hash {
    fn new(generation: Generation) -> Hash {
        match generation {
            Generation::V0 => Hash::V0(Crc16Stream::new()),
            Generation::V1 => Hash::V1(Crc32Stream::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Hash::V0(h) => h.update(bytes),
            Hash::V1(h) => h.update(bytes),
        }
    }
}

/// Parse the header; for a type-1 file also consume the crc32 word and the pad,
/// leaving the stream at the first body byte.
fn read_header(r: &mut impl Read) -> Result<(Header, u32), Error> {
    let mut head = [0u8; HEAD_LEN];
    r.read_exact(&mut head)?;
    if &head[0..4] != MAGIC {
        return Err(ParseError::UnknownFileType(tag_str(&head[0..4].try_into().unwrap())).into());
    }
    let le = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
    let generation = match le(4) {
        0 => Generation::V0,
        1 => Generation::V1,
        other => {
            return Err(ParseError::UnknownFormat(format!("CBIN header type {other}")).into());
        }
    };
    let header = Header {
        generation,
        tag: head[8..12].try_into().unwrap(),
        location: le(12),
        aux: le(16),
        version: le(20),
    };

    let mut stored_crc32 = 0;
    if generation == Generation::V1 {
        let mut rest = [0u8; 20];
        r.read_exact(&mut rest)?;
        stored_crc32 = u32::from_le_bytes(rest[0..4].try_into().unwrap());
        // Zero on every specimen. A file that used these bytes would round-trip
        // wrong silently, so refuse it loudly instead.
        if rest[4..] != [0u8; 16] {
            return Err(ParseError::AssertFail(
                "nonzero bytes in the 0x1c..0x2c header pad".into(),
            )
            .into());
        }
    }
    Ok((header, stored_crc32))
}

/// The end of the stream, with the position restored.
fn stream_end(r: &mut impl Seek) -> io::Result<u64> {
    let pos = r.stream_position()?;
    let end = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(pos))?;
    Ok(end)
}

/// Read one container from the whole rest of the stream, expecting `format`'s tag.
pub fn read<B: Body>(r: &mut (impl Read + Seek), format: &'static str) -> Result<Cbin<B>, Error> {
    read_inner(r, Some(format))
}

/// Read one container of any tag, its body kept verbatim.
pub fn read_raw(r: &mut (impl Read + Seek)) -> Result<Cbin<RawBody>, Error> {
    read_inner(r, None)
}

fn read_inner<B: Body>(
    r: &mut (impl Read + Seek),
    format: Option<&'static str>,
) -> Result<Cbin<B>, Error> {
    let start = r.stream_position()?;
    let (header, stored_crc32) = read_header(r)?;
    if let Some(expected) = format {
        if header.tag != tag(expected) {
            return Err(ParseError::WrongFormat {
                expected,
                got: tag_str(&header.tag),
            }
            .into());
        }
    }
    // In errors below, name the format by its expected tag when one was asked
    // for, and by the file's own tag on a raw read.
    let format = format.map_or_else(|| tag_str(&header.tag), str::to_string);

    let end = stream_end(r)?;
    let overhead = header.generation.body_start() + header.generation.trailer_len();
    if end < start + overhead {
        return Err(ParseError::AssertFail(format!(
            "{format}: {} bytes is shorter than the {overhead}-byte container",
            end - start,
        ))
        .into());
    }
    let body_start = start + header.generation.body_start();
    let body_len = end - body_start - header.generation.trailer_len();
    if let Some(expected) = B::LEN {
        if body_len != expected {
            return Err(ParseError::WrongBodyLength {
                format,
                got: body_len,
                expected,
            }
            .into());
        }
    }

    let mut hash = Hash::new(header.generation);
    if header.generation == Generation::V0 {
        // The crc16 covers the header too. Re-encoding is exact: every one of the
        // 0x18 bytes is either verified (magic) or held verbatim in `header`.
        hash.update(&header.head_bytes());
    }
    let mut reader = BodyReader {
        inner: r,
        start: body_start,
        len: body_len,
        pos: 0,
        hashed: 0,
        hash,
    };
    let body = B::read(&mut reader, &header)?;
    reader.verify(stored_crc32, &format)?;
    Ok(Cbin { header, body })
}

impl<B: Body> Cbin<B> {
    pub fn write_to(&self, w: &mut (impl Write + Seek)) -> Result<(), Error> {
        let start = w.stream_position()?;
        let head = self.header.head_bytes();
        let mut hash = Hash::new(self.header.generation);
        w.write_all(&head)?;
        match self.header.generation {
            // The crc32 is not known yet; a placeholder holds its word until the
            // body has streamed past, then one seek patches it.
            Generation::V1 => w.write_all(&[0u8; 20])?,
            Generation::V0 => hash.update(&head),
        }

        let body_start = start + self.header.generation.body_start();
        let mut writer = BodyWriter {
            inner: w,
            pos: 0,
            hash,
        };
        self.body.write(&mut writer)?;
        let BodyWriter {
            pos: written, hash, ..
        } = writer;

        if let Some(expected) = B::LEN {
            if written != expected {
                return Err(ParseError::WrongBodyLength {
                    format: tag_str(&self.header.tag),
                    got: written,
                    expected,
                }
                .into());
            }
        }

        match hash {
            Hash::V1(h) => {
                w.seek(SeekFrom::Start(start + 0x18))?;
                w.write_all(&h.value().to_le_bytes())?;
                w.seek(SeekFrom::Start(body_start + written))?;
            }
            Hash::V0(h) => w.write_all(&h.value().to_le_bytes())?,
        }
        Ok(())
    }
}

/// A body kept verbatim: bytes in, bytes out, checksum verified, nothing decoded.
///
/// For formats whose body is not yet mapped (`npno`) and for wire code that moves
/// bodies whole. ⚠️ Allocates the body — a library-sized file wants [`inspect`],
/// which holds O(1), not a `RawBody`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBody(pub Vec<u8>);

impl Body for RawBody {
    fn read<R: Read + Seek>(r: &mut BodyReader<'_, R>, _: &Header) -> Result<RawBody, Error> {
        let mut bytes = vec![0u8; r.len() as usize];
        r.read_exact(&mut bytes)?;
        Ok(RawBody(bytes))
    }

    fn write<W: Write + Seek>(&self, w: &mut BodyWriter<'_, W>) -> Result<(), Error> {
        w.write_all(&self.0)?;
        Ok(())
    }
}

/// Container-level facts about one file: header, length, checksum verdict.
#[derive(Debug, Clone)]
pub struct Info {
    pub header: Header,
    pub body_len: u64,
    /// Whether the stored checksum matches the bytes. A mismatch is a fact to
    /// report, not an error: reporting bad files is this function's job.
    pub checksum_ok: bool,
}

/// One streaming pass over any CBIN file, no body knowledge needed. O(1) memory,
/// so it serves the formats too large or too unmapped to decode.
pub fn inspect(r: &mut (impl Read + Seek)) -> Result<Info, Error> {
    let start = r.stream_position()?;
    let (header, stored_crc32) = read_header(r)?;
    let end = stream_end(r)?;
    let overhead = header.generation.body_start() + header.generation.trailer_len();
    if end < start + overhead {
        return Err(ParseError::AssertFail(format!(
            "{}: {} bytes is shorter than the {overhead}-byte container",
            tag_str(&header.tag),
            end - start,
        ))
        .into());
    }
    let body_len = end - start - overhead;

    let mut hash = Hash::new(header.generation);
    if header.generation == Generation::V0 {
        hash.update(&header.head_bytes());
    }
    let mut remaining = body_len;
    let mut scratch = [0u8; 8192];
    while remaining > 0 {
        let take = remaining.min(scratch.len() as u64) as usize;
        r.read_exact(&mut scratch[..take])?;
        hash.update(&scratch[..take]);
        remaining -= take as u64;
    }
    let checksum_ok = match hash {
        Hash::V1(h) => h.value() == stored_crc32,
        Hash::V0(h) => {
            let mut trailer = [0u8; 2];
            r.read_exact(&mut trailer)?;
            h.value() == u16::from_le_bytes(trailer)
        }
    };
    Ok(Info {
        header,
        body_len,
        checksum_ok,
    })
}

/// A read view scoped to the body: position 0 is the first body byte, [`len`] is
/// the body length (the type-0 trailer already excluded), and every byte is
/// checksummed on its way past.
///
/// Seeking forward reads through the gap in bounded chunks so skipped bytes still
/// reach the checksum; seeking backward and re-reading hashes nothing twice.
///
/// [`len`]: BodyReader::len
pub struct BodyReader<'a, R: Read + Seek> {
    inner: &'a mut R,
    /// Absolute offset of body byte 0.
    start: u64,
    len: u64,
    /// Body-relative position.
    pos: u64,
    /// High-water mark of hashed bytes. Never exceeded by `pos` outside `seek`,
    /// so the hash covers `[0, hashed)` exactly once.
    hashed: u64,
    hash: Hash,
}

impl<R: Read + Seek> BodyReader<'_, R> {
    /// Body length in bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Body bytes between the current position and the end.
    pub fn remaining(&self) -> u64 {
        self.len - self.pos
    }

    /// Drain to the end, then check the checksum against the stored one.
    fn verify(mut self, stored_crc32: u32, format: &str) -> Result<(), Error> {
        self.seek(SeekFrom::Start(self.len))?;
        match self.hash {
            Hash::V1(h) => {
                let computed = h.value();
                if computed != stored_crc32 {
                    return Err(ParseError::AssertFail(format!(
                        "{format}: stored checksum {stored_crc32:#010x} does not match the \
                         body's {computed:#010x}"
                    ))
                    .into());
                }
            }
            Hash::V0(h) => {
                let mut trailer = [0u8; 2];
                self.inner.read_exact(&mut trailer)?;
                let stored = u16::from_le_bytes(trailer);
                let computed = h.value();
                if computed != stored {
                    return Err(ParseError::AssertFail(format!(
                        "{format}: stored checksum {stored:#06x} does not match the \
                         file's {computed:#06x}"
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }
}

impl<R: Read + Seek> Read for BodyReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len - self.pos;
        if remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(remaining) as usize;
        let n = self.inner.read(&mut buf[..want])?;
        let end = self.pos + n as u64;
        if end > self.hashed {
            // Only the tail past the high-water mark: a re-read after a backward
            // seek must not reach the accumulator twice.
            let from = (self.hashed - self.pos) as usize;
            self.hash.update(&buf[from..n]);
            self.hashed = end;
        }
        self.pos = end;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for BodyReader<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
        };
        if target < 0 || target > self.len as i128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("seek to {target} outside the {}-byte body", self.len),
            ));
        }
        let target = target as u64;

        if target <= self.hashed {
            self.inner.seek(SeekFrom::Start(self.start + target))?;
        } else {
            // Read through the gap so the skipped bytes still reach the checksum.
            self.inner.seek(SeekFrom::Start(self.start + self.hashed))?;
            let mut scratch = [0u8; 8192];
            while self.hashed < target {
                let take = (target - self.hashed).min(scratch.len() as u64) as usize;
                self.inner.read_exact(&mut scratch[..take])?;
                self.hash.update(&scratch[..take]);
                self.hashed += take as u64;
            }
        }
        self.pos = target;
        Ok(target)
    }
}

/// The write half: hashes bytes as they stream out, in one forward pass.
///
/// ⚠️ This must never implement `Seek`. The checksum is accumulated as bytes go
/// past, so it is only correct if every body byte is written exactly once, in
/// order; a rewind would hash a byte twice and stamp a checksum matching no file.
pub struct BodyWriter<'a, W: Write + Seek> {
    inner: &'a mut W,
    /// Body-relative position.
    pos: u64,
    hash: Hash,
}

impl<W: Write + Seek> Write for BodyWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        // Only the bytes accepted — hashing past `n` would checksum bytes the
        // caller will retry, counting them twice.
        self.hash.update(&buf[..n]);
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc::{crc16, crc32};
    use std::io::Cursor;

    /// A 5-byte body under a fixed length, to exercise `B::LEN`.
    #[derive(Debug)]
    struct Five([u8; 5]);

    impl Body for Five {
        const LEN: Option<u64> = Some(5);

        fn read<R: Read + Seek>(r: &mut BodyReader<'_, R>, _: &Header) -> Result<Five, Error> {
            let mut b = [0u8; 5];
            r.read_exact(&mut b)?;
            Ok(Five(b))
        }

        fn write<W: Write + Seek>(&self, w: &mut BodyWriter<'_, W>) -> Result<(), Error> {
            w.write_all(&self.0)?;
            Ok(())
        }
    }

    fn v1_file(body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"CBIN");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(b"test");
        out.extend_from_slice(&0x0007_0003u32.to_le_bytes());
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&crc32(body).to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(body);
        out
    }

    fn v0_file(body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"CBIN");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"test");
        out.extend_from_slice(&0x0007_0003u32.to_le_bytes());
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(body);
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Both generations of the same body round-trip byte-exactly, and the type-0
    /// file is 18 bytes shorter — the corpus's measured delta.
    #[test]
    fn both_generations_round_trip_and_differ_by_18_bytes() {
        let body = [0xaa, 0xbb, 0xcc, 0xdd, 0xee];
        for bytes in [v1_file(&body), v0_file(&body)] {
            let file: Cbin<Five> = read(&mut Cursor::new(&bytes), "test").unwrap();
            assert_eq!(file.body.0, body);
            assert_eq!(file.header.slot(), (3, 7));

            let mut out = Cursor::new(Vec::new());
            file.write_to(&mut out).unwrap();
            assert_eq!(out.into_inner(), bytes, "round trip changed the bytes");
        }
        assert_eq!(v1_file(&body).len() - v0_file(&body).len(), 18);
    }

    #[test]
    fn a_corrupted_byte_fails_either_checksum() {
        let body = [1, 2, 3, 4, 5];
        for mut bytes in [v1_file(&body), v0_file(&body)] {
            let at = bytes.len() - 3;
            bytes[at] ^= 0xff;
            assert!(
                read::<Five>(&mut Cursor::new(&bytes), "test").is_err(),
                "a corrupted body must not verify"
            );
        }
    }

    /// The type-0 crc16 covers the header: corrupting a header byte must fail even
    /// though the body is intact.
    #[test]
    fn the_v0_checksum_covers_the_header() {
        let mut bytes = v0_file(&[1, 2, 3, 4, 5]);
        bytes[0x0c] ^= 0xff;
        assert!(read::<Five>(&mut Cursor::new(&bytes), "test").is_err());

        // ...and the type-1 crc32 does not: the same header corruption decodes,
        // which is how Clavia files one sample variant per device by editing the
        // location alone.
        let mut bytes = v1_file(&[1, 2, 3, 4, 5]);
        bytes[0x0c] ^= 0xff;
        assert!(read::<Five>(&mut Cursor::new(&bytes), "test").is_ok());
    }

    #[test]
    fn the_wrong_tag_is_refused_by_name() {
        let bytes = v1_file(&[1, 2, 3, 4, 5]);
        let err = read::<Five>(&mut Cursor::new(&bytes), "ne5p").unwrap_err();
        assert!(
            matches!(
                err,
                Error::Parse(ParseError::WrongFormat {
                    expected: "ne5p",
                    ..
                })
            ),
            "refused for the wrong reason: {err}",
        );
    }

    #[test]
    fn the_wrong_length_is_refused_before_the_body_decodes() {
        let bytes = v1_file(&[1, 2, 3]);
        let err = read::<Five>(&mut Cursor::new(&bytes), "test").unwrap_err();
        assert!(
            matches!(
                err,
                Error::Parse(ParseError::WrongBodyLength {
                    got: 3,
                    expected: 5,
                    ..
                })
            ),
            "refused for the wrong reason: {err}",
        );
    }

    /// An unread body tail still reaches the checksum: the container drains what
    /// the body left behind, so verification never silently narrows.
    #[test]
    fn an_unread_tail_is_still_verified() {
        struct TwoOfFive;
        impl Body for TwoOfFive {
            fn read<R: Read + Seek>(
                r: &mut BodyReader<'_, R>,
                _: &Header,
            ) -> Result<TwoOfFive, Error> {
                let mut b = [0u8; 2];
                r.read_exact(&mut b)?;
                Ok(TwoOfFive)
            }
            fn write<W: Write + Seek>(&self, _: &mut BodyWriter<'_, W>) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut bytes = v1_file(&[1, 2, 3, 4, 5]);
        assert!(read::<TwoOfFive>(&mut Cursor::new(&bytes), "test").is_ok());
        *bytes.last_mut().unwrap() ^= 0xff;
        assert!(
            read::<TwoOfFive>(&mut Cursor::new(&bytes), "test").is_err(),
            "a corrupt byte the body never read must still fail verification",
        );
    }

    /// Forward seeks hash the skipped bytes; backward seeks and re-reads hash
    /// nothing twice.
    #[test]
    fn seeking_bodies_keep_the_checksum_exact() {
        struct Skipper;
        impl Body for Skipper {
            fn read<R: Read + Seek>(
                r: &mut BodyReader<'_, R>,
                _: &Header,
            ) -> Result<Skipper, Error> {
                r.seek(SeekFrom::Start(4))?; // skip forward over unread bytes
                let mut b = [0u8; 1];
                r.read_exact(&mut b)?;
                r.seek(SeekFrom::Start(0))?; // back to the start
                r.read_exact(&mut b)?; // re-read an already-hashed byte
                Ok(Skipper)
            }
            fn write<W: Write + Seek>(&self, _: &mut BodyWriter<'_, W>) -> Result<(), Error> {
                Ok(())
            }
        }

        let bytes = v1_file(&[9, 8, 7, 6, 5]);
        assert!(read::<Skipper>(&mut Cursor::new(&bytes), "test").is_ok());
    }

    #[test]
    fn inspect_reports_both_generations_without_a_body() {
        for (bytes, generation) in [
            (v1_file(&[1, 2, 3]), Generation::V1),
            (v0_file(&[1, 2, 3]), Generation::V0),
        ] {
            let info = inspect(&mut Cursor::new(&bytes)).unwrap();
            assert_eq!(info.header.generation, generation);
            assert_eq!(info.body_len, 3);
            assert!(info.checksum_ok);

            let mut corrupt = bytes.clone();
            let at = corrupt.len() - 3;
            corrupt[at] ^= 0xff;
            let info = inspect(&mut Cursor::new(&corrupt)).unwrap();
            assert!(!info.checksum_ok, "inspect reports, it does not refuse");
        }
    }

    #[test]
    fn raw_bodies_round_trip_any_tag() {
        let bytes = v0_file(&[1, 2, 3, 4, 5, 6, 7]);
        let file = read_raw(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(file.body.0, [1, 2, 3, 4, 5, 6, 7]);

        let mut out = Cursor::new(Vec::new());
        file.write_to(&mut out).unwrap();
        assert_eq!(out.into_inner(), bytes);
    }
}
