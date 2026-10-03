//! Conversion between the entity body on the wire and an on-disk `CBIN` file.
//!
//! The device transfers an entity body without its `CBIN` header. The format tag,
//! schema version, and slot reported by the device, plus the body, determine that
//! header. The container codec and checksum live in `nord_format::cbin`.

use crate::error::{Error, Result};
use crate::wire::Location;
use nord_format::cbin::{self, Cbin, Generation, Header, RawBody};
use nord_format::crc::{Crc16Stream, Crc32Stream};
use std::io::{self, Cursor};
use std::ops::Range;

/// CRC-32/ISO-HDLC over a wire body. The type-1 container carries the same checksum,
/// and the device reports it in `0x1e` object info.
pub fn crc32(data: &[u8]) -> u32 {
    nord_format::crc::crc32(data)
}

/// A wire slot as the header's `(bank, slot)` pair. Both are zero-indexed, one below
/// the display.
fn slot(at: Location) -> Result<(u16, u16)> {
    let bank = u16::try_from(at.bank)
        .map_err(|_| Error::InvalidArgument(format!("bank {} does not fit in CBIN", at.bank)))?;
    let slot = u16::try_from(at.slot)
        .map_err(|_| Error::InvalidArgument(format!("slot {} does not fit in CBIN", at.slot)))?;
    Ok((bank, slot))
}

/// The slot a header addresses, as the wire spells it.
pub fn location(header: &Header) -> Location {
    let (bank, slot) = header.slot();
    Location {
        bank: bank as u32,
        slot: slot as u32,
    }
}

/// The header's format tag as text.
pub fn tag(header: &Header) -> String {
    String::from_utf8_lossy(&header.tag).into_owned()
}

/// The four-character format tag in file bytes, if those bytes are one.
///
/// ⚠️ Read from the header bytes without parsing: this must work for a file whose
/// checksum is bad, because that file may be a slot's last remaining copy.
pub fn unchecked_tag(file: &[u8]) -> Option<String> {
    file.get(8..12)
        .filter(|tag| tag.iter().all(|b| b.is_ascii_alphanumeric()))
        .map(|tag| String::from_utf8_lossy(tag).into_owned())
}

/// Filename for the bytes rescued from a slot: the location as the instrument labels
/// it, and the object's own format tag, or `bin` without one, so the file can be
/// written straight back. The checksum is not verified.
pub fn rescue_name(at: Location, backup: &[u8]) -> String {
    let format = unchecked_tag(backup).unwrap_or_else(|| "bin".to_string());
    format!(
        "nord-rescued-{}-{}.{format}",
        at.user_bank(),
        at.user_slot()
    )
}

/// Wrap a wire body in a `CBIN` header, producing the bytes of a `.ne5p`-style file.
///
/// `format` and `version` are the tag and schema version the device reported for the
/// slot in `0x1e` object info. `version` is per format tag, so passing a program's 4
/// for a set list writes a header `nord-format` will refuse to read.
pub fn wrap(format: &str, at: Location, version: u32, body: &[u8]) -> Result<Vec<u8>> {
    if format.len() != 4 {
        return Err(Error::Envelope(format!(
            "format tag {format:?} is not 4 characters"
        )));
    }

    let file = Cbin {
        header: Header::new(format, slot(at)?, version),
        body: RawBody(body.to_vec()),
    };
    let mut out = Cursor::new(Vec::new());
    file.write_to(&mut out)
        .map_err(|e| Error::Envelope(e.to_string()))?;
    Ok(out.into_inner())
}

const BARE_HEADER: &str = "the file is a bare CBIN header with no body to send";

/// The inverse of [`wrap`]: split file bytes into the header and the body the wire
/// carries. The checksum is verified.
pub fn unwrap(file: &[u8]) -> Result<Cbin<RawBody>> {
    let read =
        cbin::read_raw(&mut Cursor::new(file)).map_err(|e| Error::Envelope(e.to_string()))?;
    // A container may hold an empty body, but the body is the whole payload of a write.
    if read.body.0.is_empty() {
        return Err(Error::Envelope(BARE_HEADER.into()));
    }
    Ok(read)
}

/// Where the bytes of a file to send come from: a file on disk, a browser `File`, or
/// memory. A write reads it in pieces no larger than one transfer chunk, so the whole
/// file is never held at once.
///
/// No `Send` bound, for the reason [`Transport`](crate::transport::Transport) gives.
#[allow(async_fn_in_trait, clippy::len_without_is_empty)]
pub trait FileSource {
    /// The file's length in bytes, taken once before the first read.
    fn len(&self) -> u64;

    /// Fill `buf` with the bytes starting at `offset`. Bytes past the end are an error,
    /// such as [`io::ErrorKind::UnexpectedEof`], never a short read.
    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl FileSource for &[u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let bytes = usize::try_from(offset)
            .ok()
            .and_then(|start| self.get(start..start.checked_add(buf.len())?))
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

/// A file opened through a [`FileSource`] and verified as [`unwrap`] verifies one: its
/// header, and where in the file the body the wire carries lies.
pub(crate) struct Opened {
    pub header: Header,
    pub body: Range<usize>,
    stored: u32,
    /// The file's first bytes, which a type-0 checksum covers ahead of the body.
    head: Vec<u8>,
}

/// The checksum a file's generation stores, accumulated over its body.
pub(crate) enum Hash {
    V0(Crc16Stream<'static>),
    V1(Crc32Stream<'static>),
}

impl Hash {
    pub fn update(&mut self, bytes: &[u8]) {
        match self {
            Hash::V0(h) => h.update(bytes),
            Hash::V1(h) => h.update(bytes),
        }
    }

    fn value(&self) -> u32 {
        match self {
            Hash::V0(h) => h.value().into(),
            Hash::V1(h) => h.value(),
        }
    }
}

impl Opened {
    /// An accumulator that has seen what the checksum covers before the body.
    pub fn hash(&self) -> Hash {
        match self.header.generation {
            Generation::V0 => {
                let mut hash = Crc16Stream::new();
                hash.update(&self.head[..Generation::V0.body_start() as usize]);
                Hash::V0(hash)
            }
            Generation::V1 => Hash::V1(Crc32Stream::new()),
        }
    }

    /// Whether `hash`, having seen the whole body, matches the checksum the file stores.
    pub fn matches(&self, hash: &Hash) -> bool {
        hash.value() == self.stored
    }

    /// One pass over the body in pieces of `chunk` bytes, checking it against the
    /// stored checksum.
    async fn verify(&self, file: &mut impl FileSource, chunk: usize) -> Result<()> {
        let mut hash = self.hash();
        let mut buf = vec![0; chunk.min(self.body.len())];
        for at in self.body.clone().step_by(chunk) {
            let piece = &mut buf[..chunk.min(self.body.end - at)];
            file.read_at(at as u64, piece).await?;
            hash.update(piece);
        }
        if self.matches(&hash) {
            return Ok(());
        }
        Err(Error::Envelope(format!(
            "{}: stored checksum {:#x} does not match the file's {:#x}",
            tag(&self.header),
            self.stored,
            hash.value()
        )))
    }
}

/// [`unwrap`] through a [`FileSource`]: the header is read alone, and the body in
/// pieces of `chunk` bytes, which are checked against the stored checksum and dropped.
pub(crate) async fn open(file: &mut impl FileSource, chunk: usize) -> Result<Opened> {
    let len = usize::try_from(file.len()).map_err(|_| {
        Error::InvalidArgument("the file is larger than this platform can address".into())
    })?;
    let mut head = vec![0; len.min(Generation::V1.body_start() as usize)];
    file.read_at(0, &mut head).await?;
    // The header checks are nord-format's: the longer header's worth of bytes, or the
    // whole of a shorter file, is a container it can inspect without the body.
    let info =
        cbin::inspect(&mut Cursor::new(&head)).map_err(|e| Error::Envelope(e.to_string()))?;

    let start = info.header.generation.body_start() as usize;
    let (body, stored) = match info.header.generation {
        Generation::V1 => (start..len, info.stored_checksum),
        // The crc16 trails the file, out of the header's reach. Inspecting has refused a
        // file too short to hold it.
        Generation::V0 => {
            let mut trailer = [0u8; 2];
            let end = len - trailer.len();
            file.read_at(end as u64, &mut trailer).await?;
            (start..end, u16::from_le_bytes(trailer).into())
        }
    };
    let opened = Opened {
        header: info.header,
        body,
        stored,
        head,
    };
    opened.verify(file, chunk).await?;
    if opened.body.is_empty() {
        return Err(Error::Envelope(BARE_HEADER.into()));
    }
    Ok(opened)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `.ne5p` read off bank 8 slot 14, split at the header boundary.
    const HEADER: &str =
        "4342494e010000006e65357007000d00ffffffff04000000b65d46a500000000000000000000000000000000";
    const BODY: &str = "000401df06781fc60000000000000000000000000000000000000100000000000000000000400000000000000002200000000000022000400000008888000008008888000008000000000080000000000080000000000000000000800000000800800000000800020010060401020408140010000000000000";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn wrap_rebuilds_the_original_file() {
        let body = hex(BODY);
        let file = [hex(HEADER), body.clone()].concat();
        // Bank 8 slot 14 on the instrument; 7 and 13 on the wire.
        let built = wrap("ne5p", Location::from_user(8, 14), 4, &body).unwrap();
        assert_eq!(built, file, "rebuilt header differs from the real file");
    }

    /// A set list is version 0 or 1 where a program is 4, and a constant version makes
    /// `nord-format` refuse the file.
    #[test]
    fn wrap_writes_the_version_it_is_given() {
        let body = hex(BODY);
        for version in [0u32, 1, 4, 540] {
            let file = wrap("ne5t", Location::from_user(1, 1), version, &body).unwrap();
            assert_eq!(
                u32::from_le_bytes(file[0x14..0x18].try_into().unwrap()),
                version
            );
        }
    }

    #[test]
    fn unwrap_is_the_inverse() {
        let body = hex(BODY);
        let file = wrap("ne5p", Location::from_user(8, 14), 4, &body).unwrap();
        let got = unwrap(&file).unwrap();
        assert_eq!(tag(&got.header), "ne5p");
        assert_eq!(location(&got.header), Location::from_user(8, 14));
        assert_eq!(got.header.version, 4);
        assert_eq!(got.body.0, body);
    }

    /// A well-formed header with nothing behind it passes every container check, and
    /// still has nothing to transfer.
    #[test]
    fn unwrap_rejects_a_headers_worth_of_file() {
        let file = Cbin {
            header: Header::new("ne5p", (0, 0), 4),
            body: RawBody(Vec::new()),
        };
        let mut bytes = Cursor::new(Vec::new());
        file.write_to(&mut bytes).unwrap();
        let bytes = bytes.into_inner();
        assert!(cbin::read_raw(&mut Cursor::new(&bytes)).is_ok());
        assert!(unwrap(&bytes).is_err(), "an empty body has nothing to send");
    }

    fn file(generation: Generation, tag: &str, body: &[u8]) -> Vec<u8> {
        let file = Cbin {
            header: Header {
                generation,
                ..Header::new(tag, (6, 9), 4)
            },
            body: RawBody(body.to_vec()),
        };
        let mut bytes = Cursor::new(Vec::new());
        file.write_to(&mut bytes).unwrap();
        bytes.into_inner()
    }

    /// For every truncation and every flipped bit of type-1 and type-0 files, `open`
    /// accepts exactly what `unwrap` accepts, and finds the same header and body.
    #[test]
    fn opening_through_a_source_accepts_exactly_what_unwrap_accepts() {
        let body = hex(BODY);
        let files = [
            [hex(HEADER), body.clone()].concat(),
            file(Generation::V0, "nspg", &body),
            // Shorter than a type-1 header, so the header read is the whole file.
            file(Generation::V0, "nspg", &body[..5]),
            file(Generation::V1, "ne5p", &[]),
        ];
        let mut cases = Vec::new();
        for file in &files {
            cases.extend((0..=file.len()).map(|len| file[..len].to_vec()));
            for at in 0..file.len() {
                for bit in 0..8 {
                    let mut flipped = file.clone();
                    flipped[at] ^= 1 << bit;
                    cases.push(flipped);
                }
            }
        }

        let (mut accepted, mut refused) = (0, 0);
        for (i, case) in cases.iter().enumerate() {
            let whole = unwrap(case);
            for chunk in [1, 7, 4096] {
                let opened = pollster::block_on(open(&mut case.as_slice(), chunk));
                match (&whole, opened) {
                    (Ok(whole), Ok(opened)) => {
                        assert_eq!(opened.header, whole.header, "case {i}");
                        assert_eq!(case[opened.body], whole.body.0[..], "case {i}");
                        accepted += 1;
                    }
                    (Err(_), Err(_)) => refused += 1,
                    (whole, opened) => panic!(
                        "case {i}, chunk {chunk}: unwrap {:?} but open {:?}",
                        whole.as_ref().map(|_| ()),
                        opened.map(|_| ())
                    ),
                }
            }
        }
        assert!(
            accepted > 0 && refused > 0,
            "{accepted} accepted, {refused} refused"
        );
    }

    #[test]
    fn a_slice_refuses_a_read_past_its_end() {
        let mut file: &[u8] = &[1, 2, 3];
        let mut buf = [0; 2];
        pollster::block_on(file.read_at(1, &mut buf)).unwrap();
        assert_eq!(buf, [2, 3]);
        let err = pollster::block_on(file.read_at(2, &mut buf)).expect_err("one byte short");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let err = pollster::block_on(file.read_at(u64::MAX, &mut buf)).expect_err("past usize");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn unwrap_rejects_a_corrupted_body() {
        let body = hex(BODY);
        let mut file = wrap("ne5p", Location::from_user(8, 14), 4, &body).unwrap();
        *file.last_mut().unwrap() ^= 0xFF;
        assert!(
            unwrap(&file).is_err(),
            "a corrupted body should fail the checksum"
        );
    }

    /// A rescue file is the last copy of an object that no longer exists on the
    /// instrument, so its name must say where it came from and what it is.
    #[test]
    fn a_rescued_slot_is_named_for_its_location_and_format() {
        // A minimal CBIN: magic, header type, tag. The checksum is left wrong, because
        // naming must not depend on the backup being intact.
        let mut file = vec![0u8; 45];
        file[0..4].copy_from_slice(b"CBIN");
        file[4..8].copy_from_slice(&1u32.to_le_bytes());
        file[8..12].copy_from_slice(b"ne5p");
        let at = Location { bank: 6, slot: 49 };
        assert_eq!(rescue_name(at, &file), "nord-rescued-7-50.ne5p");
    }

    /// A set list must not land with a program's extension.
    #[test]
    fn a_rescued_slot_takes_its_format_tag_from_the_bytes() {
        let mut file = vec![0u8; 45];
        file[8..12].copy_from_slice(b"ne5t");
        let at = Location { bank: 0, slot: 3 };
        assert_eq!(rescue_name(at, &file), "nord-rescued-1-4.ne5t");
    }

    /// Bytes that do not parse are still the only copy, so they must still get a name.
    #[test]
    fn unparseable_bytes_are_rescued_as_bin() {
        let at = Location { bank: 0, slot: 0 };
        assert_eq!(rescue_name(at, b"nonsense"), "nord-rescued-1-1.bin");
        let mut short = vec![0u8; 12];
        short[8..12].copy_from_slice(b"ne5\0");
        assert_eq!(unchecked_tag(&short), None, "a NUL is not part of a tag");
        assert_eq!(unchecked_tag(&short[..11]), None, "a tag cut short");
    }

    #[test]
    fn wrap_rejects_an_address_the_container_cannot_represent() {
        let at = Location {
            bank: u16::MAX as u32 + 1,
            slot: 0,
        };
        assert!(matches!(
            wrap("ne5p", at, 4, &[1]),
            Err(Error::InvalidArgument(_))
        ));
    }
}
