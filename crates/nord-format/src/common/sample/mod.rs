//! Sample instruments (`.nsmp`) — the Nord Sample Library format.
//!
//! Shared across the Nord line rather than specific to one model, which is why this sits
//! in [`crate::common`]. A file is the usual 44-byte CBIN header followed by a chain of
//! tagged [`section`]s: an `hdr` carrying the name, a `cat` of category strings, a `map`
//! ending in the [`zone`] table, one [`stroke`] per zone, and a trailing `sty`.
//!
//! **The audio is encoded and stays that way.** Strokes are kept verbatim, so this reads
//! and rewrites instruments byte-exactly and can retune, rename and remap them — but it
//! cannot decode or synthesise the audio itself.

pub mod section;
pub mod stroke;
pub mod zone;

pub use section::Section;
pub use stroke::Stroke;
pub use zone::Zone;

use crate::crc::crc32;
use crate::error::{Error, ParseError};
use std::fmt;
use std::io::{Read, Seek, Write};

pub const FORMAT: &str = "nsmp";

/// The fixed CBIN header; the section chain starts here. Also the first byte the
/// checksum covers.
pub const BODY_AT: usize = 0x2c;

/// Offset of the instrument name within the `hdr` payload.
const NAME_AT: usize = 12;

/// Longest name this writer will emit.
///
/// The field is fixed-width and NUL-padded — a 4-character and a 14-character name give
/// the same file length — but only 14 bytes have ever been observed in use, and what
/// follows the name inside `hdr` is unmapped. Writing a longer one risks overwriting a
/// field we cannot see, so refuse instead. Reading is unrestricted.
pub const MAX_NAME_LEN: usize = 14;

/// A sample instrument.
///
/// Sections are held in file order, including repeats — `stk` appears once per zone.
pub struct Sample {
    pub header: Header,
    pub sections: Vec<Section>,
}

/// The CBIN header of a sample instrument.
///
/// ⚠️ Not [`crate::common::Header`]. Where other formats keep a bank/slot pair and an
/// `0xFFFFFFFF` trailer, a sample file carries `0xFFFFFFFF` in the location and a
/// different constant after it — a library sample has no slot until an instrument gives
/// it one. Both are preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// CBIN header type. 1 on everything with a checksum.
    pub header_type: u32,
    pub format: String,
    /// Where other formats hold bank and slot.
    pub location: u32,
    /// Where other formats hold the `0xFFFFFFFF` trailer. `0x000f0000` on every
    /// specimen; meaning unknown.
    pub unknown_0x10: u32,
    /// Content version as `format * 100 + revision`, so a 2.0 instrument reads 200.
    /// Not a schema revision, and not gated: it moves with the library's own content.
    pub version: u32,
    pub crc32: u32,
}

impl Header {
    fn read(bytes: &[u8]) -> Result<Header, ParseError> {
        if bytes.len() < BODY_AT {
            return Err(ParseError::AssertFail(format!(
                "{FORMAT}: {} bytes is shorter than the {BODY_AT}-byte header",
                bytes.len()
            )));
        }
        if &bytes[0..4] != b"CBIN" {
            return Err(ParseError::UnknownFormat(
                String::from_utf8_lossy(&bytes[0..4]).into_owned(),
            ));
        }
        let format = String::from_utf8_lossy(&bytes[8..12]).into_owned();
        if format != FORMAT {
            return Err(ParseError::WrongFormat {
                expected: FORMAT,
                got: format,
            });
        }
        let le = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Ok(Header {
            header_type: le(0x04),
            format,
            location: le(0x0c),
            unknown_0x10: le(0x10),
            version: le(0x14),
            crc32: le(0x18),
        })
    }

    fn write_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"CBIN");
        out.extend_from_slice(&self.header_type.to_le_bytes());
        out.extend_from_slice(self.format.as_bytes());
        out.extend_from_slice(&self.location.to_le_bytes());
        out.extend_from_slice(&self.unknown_0x10.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.crc32.to_le_bytes());
        out.resize(BODY_AT, 0);
    }
}

impl Sample {
    /// Reads a whole instrument, verifying its checksum.
    ///
    /// Unlike the fixed-length formats this does not drive the CRC through binrw's
    /// `map_stream`: the checksummed region runs from [`BODY_AT`] to end of file, whose
    /// length is not known until the file has been read.
    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Sample, Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Sample::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Sample, Error> {
        let header = Header::read(bytes)?;
        let computed = crc32(&bytes[BODY_AT..]);
        if computed != header.crc32 {
            return Err(ParseError::AssertFail(format!(
                "{FORMAT}: stored checksum {:#010x} does not match the body's {computed:#010x}",
                header.crc32
            ))
            .into());
        }
        let sections = section::read_chain(bytes, BODY_AT)?;
        Ok(Sample { header, sections })
    }

    pub fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<(), Error> {
        writer.write_all(&self.to_bytes())?;
        Ok(())
    }

    /// Serialises, recomputing the checksum over the body it just produced.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for s in &self.sections {
            s.write_into(&mut body);
        }
        let mut header = self.header.clone();
        header.crc32 = crc32(&body);

        let mut out = Vec::with_capacity(BODY_AT + body.len());
        header.write_into(&mut out);
        out.extend_from_slice(&body);
        out
    }

    /// Instrument name, as the Nord display shows it.
    ///
    /// The editor composes this from separate Main, Sub and Aux fields joined with `_`,
    /// so an empty Sub shows up as a doubled underscore rather than a typo.
    pub fn name(&self) -> Result<String, Error> {
        let hdr = self.hdr()?;
        let from = hdr.payload.get(NAME_AT..).ok_or_else(|| {
            ParseError::AssertFail(format!("hdr section is {} bytes", hdr.payload.len()))
        })?;
        let end = from.iter().position(|&b| b == 0).unwrap_or(from.len());
        Ok(String::from_utf8_lossy(&from[..end]).into_owned())
    }

    /// Renames in place, NUL-padding the rest of the field.
    pub fn set_name(&mut self, name: &str) -> Result<(), Error> {
        if name.len() > MAX_NAME_LEN {
            return Err(ParseError::OutOfBounds {
                value: format!("{name:?} ({} bytes)", name.len()),
                bound: format!("a name of at most {MAX_NAME_LEN} bytes"),
            }
            .into());
        }
        let hdr = section::find_mut(&mut self.sections, section::HDR)
            .ok_or_else(|| ParseError::AssertFail("no hdr section".into()))?;
        let field = hdr
            .payload
            .get_mut(NAME_AT..NAME_AT + MAX_NAME_LEN)
            .ok_or_else(|| ParseError::AssertFail("hdr section is too short for a name".into()))?;
        field.fill(0);
        field[..name.len()].copy_from_slice(name.as_bytes());
        Ok(())
    }

    /// Keyboard zones, high to low.
    pub fn zones(&self) -> Result<Vec<Zone>, Error> {
        Ok(zone::read(&self.map()?.payload)?)
    }

    /// Sets one zone's top note. The strokes are untouched.
    pub fn set_zone_top_note(&mut self, index: usize, note: u8) -> Result<(), Error> {
        let map = section::find_mut(&mut self.sections, section::MAP)
            .ok_or_else(|| ParseError::AssertFail("no map section".into()))?;
        zone::set_top_note(&mut map.payload, index, note)?;
        Ok(())
    }

    /// One stroke per zone, in the same high-to-low order as [`Sample::zones`].
    pub fn strokes(&self) -> Result<Vec<Stroke>, Error> {
        let zones = zone::count(&self.map()?.payload)?;
        self.stroke_sections()
            .enumerate()
            .map(|(i, s)| Ok(stroke::read(&s.payload, i, zones)?))
            .collect()
    }

    /// Retunes one zone by moving the note its sample plays untransposed at.
    pub fn set_root_key(&mut self, index: usize, note: u8) -> Result<(), Error> {
        let section = self
            .sections
            .iter_mut()
            .filter(|s| s.is(section::STK))
            .nth(index)
            .ok_or_else(|| ParseError::AssertFail(format!("no stroke {index}")))?;
        stroke::set_root_key(&mut section.payload, note)?;
        Ok(())
    }

    /// Category labels, as stored in `cat`: length-prefixed strings.
    pub fn categories(&self) -> Vec<String> {
        let Some(cat) = section::find(&self.sections, section::CAT) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < cat.payload.len() {
            let len = cat.payload[i] as usize;
            let from = i + 1;
            // A length running past the end means this is not a string here; the
            // section holds a few leading bytes before the labels start.
            match cat.payload.get(from..from + len) {
                Some(s) if len > 0 && s.iter().all(|&b| (0x20..0x7f).contains(&b)) => {
                    out.push(String::from_utf8_lossy(s).into_owned());
                    i = from + len;
                }
                _ => i += 1,
            }
        }
        out
    }

    fn stroke_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|s| s.is(section::STK))
    }

    fn hdr(&self) -> Result<&Section, Error> {
        section::find(&self.sections, section::HDR)
            .ok_or_else(|| ParseError::AssertFail("no hdr section".into()).into())
    }

    fn map(&self) -> Result<&Section, Error> {
        section::find(&self.sections, section::MAP)
            .ok_or_else(|| ParseError::AssertFail("no map section".into()).into())
    }
}

impl fmt::Debug for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sample")
            .field("name", &self.name().unwrap_or_default())
            .field("version", &self.header.version)
            .field("zones", &self.zones().map(|z| z.len()).unwrap_or(0))
            .field("sections", &self.sections)
            .finish()
    }
}
