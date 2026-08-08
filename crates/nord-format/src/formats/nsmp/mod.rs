//! Sample instruments (`.nsmp`) — the Nord Sample Library format.
//!
//! Shared across the Nord line rather than specific to one model, so it carries its own
//! tag rather than a model's. A file is the CBIN header followed by a chain of tagged
//! [`section`]s: an `hdr` carrying the name, a `cat` of category strings, a `map`
//! ending in the [`zone`] table, one [`stroke`] per zone, and a trailing `sty`.
//!
//! Both container generations occur: across the corpus every v2 specimen is type 0 and
//! every v4 is type 1, while v3 is split. The container handles the difference; the
//! chain is the same.
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

use crate::cbin::{self, BodyReader, BodyWriter, Cbin, Header, RawBody};
use crate::error::{Error, ParseError};
use std::fmt;
use std::io::{Read, Seek, Write};

pub const FORMAT: &str = "nsmp";

/// The content version at which the schema leaves v2. All three generations
/// share the `nsmp` tag; the u32 at `0x14` is the generation marker, running
/// `format × 100 + revision` — `.nsmp3` content stores 300 and up, `.nsmp4`
/// 400 and up, and their bodies decode a different schema than v2's.
pub const RAW_FROM_VERSION: u32 = 300;

/// A body decoded by generation: v2 in full, the later schemas kept verbatim.
///
/// ⚠️ The v2 pool also holds versions that are not `2xx` (8, 100, 140 … —
/// unexplained), so the gate is "at least 300", not "exactly 2xx".
#[derive(Debug)]
pub enum AnyBody {
    V2(Sample),
    Raw(RawBody),
}

impl cbin::Body for AnyBody {
    fn read<R: Read + Seek>(r: &mut BodyReader<'_, R>, header: &Header) -> Result<Self, Error> {
        if header.version >= RAW_FROM_VERSION {
            Ok(AnyBody::Raw(<RawBody as cbin::Body>::read(r, header)?))
        } else {
            Ok(AnyBody::V2(<Sample as cbin::Body>::read(r, header)?))
        }
    }

    fn write<W: Write + Seek>(&self, w: &mut BodyWriter<'_, W>) -> Result<(), Error> {
        match self {
            AnyBody::V2(s) => <Sample as cbin::Body>::write(s, w),
            AnyBody::Raw(r) => <RawBody as cbin::Body>::write(r, w),
        }
    }
}

/// Offset of the instrument name within the `hdr` payload.
const NAME_AT: usize = 12;

/// Longest name this writer will emit.
///
/// The field is fixed-width and NUL-padded — a 4-character and a 14-character name give
/// the same file length — but only 14 bytes have ever been observed in use, and what
/// follows the name inside `hdr` is unmapped. Writing a longer one risks overwriting a
/// field we cannot see, so refuse instead. Reading is unrestricted.
pub const MAX_NAME_LEN: usize = 14;

/// A sample instrument's body: the section chain, held in file order including
/// repeats — `stk` appears once per zone. A file is a `Cbin<Sample>`.
pub struct Sample {
    pub sections: Vec<Section>,
}

impl cbin::Body for Sample {
    fn read<R: Read + Seek>(r: &mut BodyReader<'_, R>, _: &Header) -> Result<Self, Error> {
        Ok(Sample {
            sections: section::read_chain(r)?,
        })
    }

    fn write<W: Write + Seek>(&self, w: &mut BodyWriter<'_, W>) -> Result<(), Error> {
        for s in &self.sections {
            s.write_to(w)?;
        }
        Ok(())
    }
}

/// Reads a whole instrument, verifying its checksum.
pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Sample>, Error> {
    cbin::read(reader, FORMAT)
}

pub fn from_bytes(bytes: &[u8]) -> Result<Cbin<Sample>, Error> {
    read_from(&mut std::io::Cursor::new(bytes))
}

impl Cbin<Sample> {
    /// Serialises, recomputing the checksum over the body it just produced.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut out = std::io::Cursor::new(Vec::new());
        self.write_to(&mut out)?;
        Ok(out.into_inner())
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
        let hdr = section::find_mut(&mut self.body.sections, section::HDR)
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
        let map = section::find_mut(&mut self.body.sections, section::MAP)
            .ok_or_else(|| ParseError::AssertFail("no map section".into()))?;
        zone::set_top_note(&mut map.payload, index, note)?;
        Ok(())
    }

    /// One stroke per zone, in the same high-to-low order as [`Self::zones`].
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
            .body
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
        let Some(cat) = section::find(&self.body.sections, section::CAT) else {
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
        self.body.sections.iter().filter(|s| s.is(section::STK))
    }

    fn hdr(&self) -> Result<&Section, Error> {
        section::find(&self.body.sections, section::HDR)
            .ok_or_else(|| ParseError::AssertFail("no hdr section".into()).into())
    }

    fn map(&self) -> Result<&Section, Error> {
        section::find(&self.body.sections, section::MAP)
            .ok_or_else(|| ParseError::AssertFail("no map section".into()).into())
    }
}

impl fmt::Debug for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sample")
            .field("sections", &self.sections)
            .finish()
    }
}
