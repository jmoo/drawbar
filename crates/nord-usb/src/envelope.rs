//! Conversion between the entity body on the wire and an on-disk `CBIN` file.
//!
//! The device transfers an entity body without its `CBIN` header. The format tag,
//! schema version, and slot reported by the device, plus the body, determine that
//! header. The container codec and checksum live in `nord_format::cbin`.

use crate::error::{Error, Result};
use crate::wire::{Location, ProgramInfo};
use nord_format::cbin::{self, Cbin, Generation, Header, RawBody, Verifier};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::ops::Range;

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
    file.get(8..12).and_then(tag_text)
}

/// Four bytes as a format tag, where they are one.
fn tag_text(tag: &[u8]) -> Option<String> {
    (tag.len() == 4 && tag.iter().all(|b| b.is_ascii_alphanumeric()))
        .then(|| String::from_utf8_lossy(tag).into_owned())
}

/// Filename for the bytes rescued from the slot `info` describes, named before any of
/// them are read: [`RESCUED`], the location as the instrument labels it, the occupant's
/// name in parentheses, and its format tag, or `bin` without one, so the file can be
/// written straight back under its own name, as `nord-rescued-7-50 (Grand).ne5p`.
///
/// The name is left out unless every character is a letter, a digit, a space or one of
/// `-_.,'&+!`, and it is at most 160 bytes, so every supported system can hold the file
/// and [`Rescued::parse`] reads back exactly the name it holds.
pub fn rescue_name_for(info: &ProgramInfo) -> String {
    let name = &info.name;
    let named = match name.is_empty() || name.len() > LONGEST_NAME || !name.chars().all(plain) {
        true => String::new(),
        false => format!(" ({name})"),
    };
    let format = tag_text(info.format.as_bytes());
    format!(
        "{RESCUED}{}-{}{named}.{}",
        info.location.user_bank(),
        info.location.user_slot(),
        format.as_deref().unwrap_or("bin")
    )
}

/// The longest occupant's name, in bytes, a rescue's file name carries. It
/// leaves room under the 255 bytes of a file name for the rest of it, a number that makes
/// it free, and [`PARTIAL`].
const LONGEST_NAME: usize = 160;

/// What a rescue's file name says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rescued {
    /// The slot it came off.
    pub at: Location,
    /// What the slot was called, empty where its file name carries no name.
    pub name: String,
}

impl Rescued {
    /// What `file_name` says, where it is a rescue's as [`rescue_name_for`] names one.
    /// Whatever follows the location or the parenthesized name, such as a number that made
    /// the name free, is ignored.
    pub fn parse(file_name: &str) -> Option<Rescued> {
        let rest = file_name.strip_prefix(RESCUED)?;
        let (bank, rest) = rest.split_once('-')?;
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (slot, rest) = rest.split_at(digits);
        let (bank, slot): (u32, u32) = (bank.parse().ok()?, slot.parse().ok()?);
        if bank == 0 || slot == 0 {
            return None;
        }
        let name = rest
            .strip_prefix(" (")
            .and_then(|rest| rest.split_once(')'))
            .map_or_else(String::new, |(name, _)| name.to_string());
        Some(Rescued {
            at: Location::from_user(bank, slot),
            name,
        })
    }
}

/// Whether a rescue's file name can carry a slot name holding `c`.
fn plain(c: char) -> bool {
    c.is_alphanumeric() || " -_.,'&+!".contains(c)
}

/// Every rescued file's name starts with this.
pub const RESCUED: &str = "nord-rescued-";

/// What a rescue is called while it is being written: its own name, then this. Nothing is
/// deleted until it is whole and renamed, so a file so named is never a slot's only copy.
pub const PARTIAL: &str = ".partial";

/// Whether `name` is a rescue's, written whole: a file that may be a slot's only copy.
pub fn is_rescue(name: &str) -> bool {
    name.starts_with(RESCUED) && !name.ends_with(PARTIAL)
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

/// A file read by position through [`Seek`], such as a [`std::fs::File`]. Its length is
/// taken once, when it is wrapped.
pub struct Positional<R> {
    inner: R,
    len: u64,
}

impl<R: Seek> Positional<R> {
    pub fn new(mut inner: R) -> io::Result<Positional<R>> {
        let len = inner.seek(SeekFrom::End(0))?;
        Ok(Positional { inner, len })
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read + Seek> FileSource for Positional<R> {
    fn len(&self) -> u64 {
        self.len
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.inner.seek(SeekFrom::Start(offset))?;
        self.inner.read_exact(buf)
    }
}

/// Where the bytes of a file read off the instrument go: a file on disk, a file in a
/// browser's storage, memory, or nowhere when only their checksum is wanted. A read hands
/// it one transfer chunk at a time, so the whole file is never held at once.
///
/// No `Send` bound, for the reason [`Transport`](crate::transport::Transport) gives.
#[allow(async_fn_in_trait)]
pub trait FileSink {
    /// Write `buf` at `offset`, growing the file as needed. A gap a write leaves before
    /// `offset` is filled by a later write.
    async fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()>;
}

impl FileSink for Vec<u8> {
    async fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::OutOfMemory)?;
        let end = start
            .checked_add(buf.len())
            .ok_or(io::ErrorKind::OutOfMemory)?;
        if self.len() < end {
            self.resize(end, 0);
        }
        self[start..end].copy_from_slice(buf);
        Ok(())
    }
}

impl FileSink for io::Sink {
    async fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> io::Result<()> {
        Ok(())
    }
}

impl FileSink for std::fs::File {
    async fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.seek(SeekFrom::Start(offset))?;
        self.write_all(buf)
    }
}

/// [`unwrap`] for a file read through a [`FileSource`], a transfer chunk at a time: the
/// header, once every byte has been checked against the stored checksum.
///
/// A write runs this check before its first frame. Call it alone to know the file is
/// whole before touching the instrument.
pub async fn verify(file: &mut impl FileSource) -> Result<Header> {
    Ok(open(file, crate::op::WRITE_CHUNK).await?.header)
}

/// The header [`wrap`] would put ahead of a body whose CRC-32 is `body_crc32`, for a body
/// written apart from it.
pub(crate) fn header(format: &str, at: Location, version: u32, body_crc32: u32) -> Result<Vec<u8>> {
    let mut head = wrap(format, at, version, &[])?;
    let stored = Generation::V1
        .checksum_range(head.len())
        .ok_or_else(|| Error::Envelope("a type-1 header has no room for its checksum".into()))?;
    head[stored].copy_from_slice(&body_crc32.to_le_bytes());
    Ok(head)
}

/// A file opened through a [`FileSource`] and verified as [`unwrap`] verifies one: its
/// header, and where in the file the body the wire carries lies.
pub(crate) struct Opened {
    pub header: Header,
    pub body: Range<usize>,
    /// The file's bytes before the body and after it, as they were verified.
    head: Vec<u8>,
    tail: Vec<u8>,
    stored: u32,
}

fn envelope(e: nord_format::error::Error) -> Error {
    Error::Envelope(e.to_string())
}

impl Opened {
    /// A check that has seen the file up to its body.
    pub fn verifier(&self) -> Result<Verifier> {
        let mut verifier = Verifier::new();
        verifier.update(&self.head).map_err(envelope)?;
        Ok(verifier)
    }

    /// Whether `verifier`, having seen the whole body, finds the checksum the file
    /// stored when it was opened.
    pub fn matches(&self, mut verifier: Verifier) -> Result<bool> {
        verifier.update(&self.tail).map_err(envelope)?;
        let info = verifier.finish().map_err(envelope)?;
        Ok(info.checksum_ok && info.stored_checksum == self.stored)
    }
}

/// [`unwrap`] through a [`FileSource`]: the header is read alone, and the rest in
/// pieces of `chunk` bytes, which are checked against the stored checksum and dropped.
pub(crate) async fn open(file: &mut impl FileSource, chunk: usize) -> Result<Opened> {
    let len = usize::try_from(file.len()).map_err(|_| {
        Error::InvalidArgument("the file is larger than this platform can address".into())
    })?;
    let mut head = vec![0; len.min(Generation::V1.body_start() as usize)];
    file.read_at(0, &mut head).await?;
    let header = Header::from_prefix(&head).map_err(envelope)?;
    head.truncate(header.generation.body_start() as usize);

    let mut verifier = Verifier::new();
    verifier.update(&head).map_err(envelope)?;
    let mut buf = vec![0; chunk.min(len - head.len())];
    for at in (head.len()..len).step_by(chunk) {
        let piece = &mut buf[..chunk.min(len - at)];
        file.read_at(at as u64, piece).await?;
        verifier.update(piece).map_err(envelope)?;
    }
    let info = verifier.finish().map_err(envelope)?;
    if !info.checksum_ok {
        return Err(Error::Envelope(format!(
            "{}: stored checksum {:#x} does not match the file",
            tag(&info.header),
            info.stored_checksum
        )));
    }

    // The body lies within a file whose length is a `usize`.
    let body = head.len()..head.len() + info.body_len as usize;
    if body.is_empty() {
        return Err(Error::Envelope(BARE_HEADER.into()));
    }
    let mut tail = vec![0; len - body.end];
    file.read_at(body.end as u64, &mut tail).await?;
    Ok(Opened {
        header: info.header,
        body,
        head,
        tail,
        stored: info.stored_checksum,
    })
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

    /// A read writes the body behind the header's room and the header last, so memory
    /// takes a write past its end and keeps what was written there.
    #[test]
    fn a_vec_takes_a_write_past_its_end_and_one_into_its_gap() {
        let mut file = Vec::new();
        pollster::block_on(file.write_at(3, &[4, 5])).unwrap();
        assert_eq!(file, [0, 0, 0, 4, 5]);
        pollster::block_on(file.write_at(0, &[1, 2, 3])).unwrap();
        assert_eq!(file, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_positional_file_reads_by_offset_and_not_past_its_end() {
        let mut file = Positional::new(Cursor::new(vec![1u8, 2, 3, 4])).unwrap();
        assert_eq!(FileSource::len(&file), 4);
        let mut buf = [0; 2];
        pollster::block_on(file.read_at(2, &mut buf)).unwrap();
        assert_eq!(buf, [3, 4]);
        let err = pollster::block_on(file.read_at(3, &mut buf)).expect_err("one byte short");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    fn occupant(at: Location, format: &str, name: &str) -> ProgramInfo {
        ProgramInfo {
            location: at,
            body_len: 1,
            format: format.into(),
            version: 4,
            crc32: None,
            modified: None,
            name: name.into(),
        }
    }

    /// A rescue file is the last copy of an object that no longer exists on the
    /// instrument, so its name must say where it came from, what it is, and what the
    /// slot was called.
    #[test]
    fn a_rescue_is_named_for_its_slot_its_occupant_and_its_format() {
        let at = Location { bank: 6, slot: 49 };
        let named = |format, name| rescue_name_for(&occupant(at, format, name));
        assert_eq!(named("ne5p", "Grand"), "nord-rescued-7-50 (Grand).ne5p");
        assert_eq!(named("ne5t", "Gig"), "nord-rescued-7-50 (Gig).ne5t");
        assert_eq!(named("npno", ""), "nord-rescued-7-50.npno");
        assert_eq!(named("nsp\0", "Pad"), "nord-rescued-7-50 (Pad).bin");
    }

    /// `put` restores the slot's name from its rescue, so a name the file carries is
    /// read back exactly.
    #[test]
    fn a_rescue_names_its_slot_and_occupant_back_exactly() {
        let at = Location::from_user(3, 12);
        for name in [
            "",
            "Grand",
            "Royal Grand 3D YaS6 XL 5.4",
            "Café",
            "CON",
            " . ",
        ] {
            let file = rescue_name_for(&occupant(at, "ne5p", name));
            let back = Rescued::parse(&file);
            assert_eq!(
                back,
                Some(Rescued {
                    at,
                    name: name.into()
                }),
                "{file:?}"
            );
        }
    }

    /// A name a file name could not hold on every system, or that would not read back
    /// as written, is left out, as an empty one is.
    #[test]
    fn a_name_with_a_character_that_is_not_plain_is_left_out() {
        let at = Location::from_user(3, 12);
        for name in [
            "A/B: mix?",
            "C:\\new <\"x\"> |*",
            "100% (live)",
            "tab\there",
        ] {
            let file = rescue_name_for(&occupant(at, "ne5p", name));
            assert_eq!(file, "nord-rescued-3-12.ne5p", "{name:?}");
        }
    }

    #[test]
    fn a_name_too_long_for_a_file_name_is_left_out() {
        let at = Location::from_user(1, 1);
        let name = "a".repeat(LONGEST_NAME + 1);
        let file = rescue_name_for(&occupant(at, "ne5p", &name));
        assert_eq!(file, "nord-rescued-1-1.ne5p");
        let fits = "a".repeat(LONGEST_NAME);
        let file = rescue_name_for(&occupant(at, "ne5p", &fits));
        assert!(file.len() + "-100".len() + PARTIAL.len() <= 255, "{file}");
        assert_eq!(Rescued::parse(&file).map(|r| r.name), Some(fits));
    }

    /// A copy numbered to make its name free, by `nord` or by a library, still says
    /// where it came from.
    #[test]
    fn a_numbered_rescue_still_names_its_slot_and_occupant() {
        let rescued = |name: &str| Rescued::parse(name).map(|r| (r.at, r.name));
        let at = Location::from_user(1, 4);
        let grand = Some((at, "Grand".to_string()));
        assert_eq!(rescued("nord-rescued-1-4 (Grand)-2.npno"), grand);
        assert_eq!(rescued("nord-rescued-1-4 (Grand) 2.npno"), grand);
        assert_eq!(
            rescued("nord-rescued-1-4 2.npno"),
            Some((at, String::new()))
        );
        assert_eq!(
            rescued("nord-rescued-1-4-2.npno"),
            Some((at, String::new()))
        );
        assert_eq!(
            rescued("nord-rescued-3-12.ne5p"),
            Some((Location::from_user(3, 12), String::new()))
        );
        assert_eq!(rescued("nord-rescued-0-4.npno"), None);
        assert_eq!(rescued("nord-rescued-notes.npno"), None);
        assert_eq!(rescued("Grand.npno"), None);
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

    #[test]
    fn a_rescue_still_being_written_is_not_a_rescue() {
        let name = rescue_name_for(&occupant(Location::from_user(1, 159), "nsmp", "Pad"));
        assert!(is_rescue(&name), "{name}");
        assert!(!is_rescue(&format!("{name}{PARTIAL}")), "{name}{PARTIAL}");
        assert!(!is_rescue("fetched-0-158.nsmp"));
    }

    #[test]
    fn a_tag_is_four_letters_or_digits() {
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
