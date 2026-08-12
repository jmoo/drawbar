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
pub use zone::ZoneV3;

use crate::cbin::{self, BodyReader, BodyWriter, Cbin, Header};
use crate::error::{Error, ParseError};
use std::fmt;
use std::io::{Read, Seek, Write};

pub const FORMAT: &str = "nsmp";

/// The content version at which the body leaves the `NWS` chain for the wide
/// `NSMP` chain. All generations share the `nsmp` tag; the u32 at `0x14` is the
/// generation marker, running `format × 100 + revision` — `.nsmp3` content
/// stores 300 and up, `.nsmp4` 400 and up.
pub const V3_FROM_VERSION: u32 = 300;

/// Content version of the first Sample Library whose v2 layout this reader decodes.
///
/// The number tracks the *library release*, not the codec, so the versions below this
/// are older libraries rather than older codecs — 8 above all, plus a 4/5/100/140/150
/// tail we hold no specimen of. They are still `NWS`-chain files; only what sits
/// inside the sections differs.
pub const LIBRARY_2_VERSION: u32 = 200;

/// A body decoded by generation: v2 in full, v3/v4 as a section chain with
/// strokes verbatim.
///
/// ⚠️ The v2 pool also holds versions that are not `2xx` — 8 (the original
/// Sample Library) and 200 (Sample Library 2.0; independent interop projects
/// report the number tracks the library release, not the codec) — so the gate
/// is "at least 300", not "exactly 2xx".
#[derive(Debug)]
pub enum AnyBody {
    V2(Sample),
    V3(SampleV3),
}

impl cbin::Body for AnyBody {
    fn read<R: Read + Seek>(r: &mut BodyReader<'_, R>, header: &Header) -> Result<Self, Error> {
        if header.version >= V3_FROM_VERSION {
            Ok(AnyBody::V3(<SampleV3 as cbin::Body>::read(r, header)?))
        } else {
            Ok(AnyBody::V2(<Sample as cbin::Body>::read(r, header)?))
        }
    }

    fn write<W: Write + Seek>(&self, w: &mut BodyWriter<'_, W>) -> Result<(), Error> {
        match self {
            AnyBody::V2(s) => <Sample as cbin::Body>::write(s, w),
            AnyBody::V3(s) => <SampleV3 as cbin::Body>::write(s, w),
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

/// A v3/v4 body: the wide-section (`NSMP`) chain, held in file order including
/// repeats — `stk` appears once per stroke. Sections are preserved verbatim, so
/// a file round-trips byte-exactly; nothing edits one yet.
///
/// Every corpus specimen chains `NSMP`, `hdr`, `cat`, `map`, N × `stk`, `sty`,
/// `meta`, in that order, in both container generations. Inferred from
/// specimens; not confirmed on hardware.
///
/// ⚠️ The stroke payload past the id fields is the encoded audio, and it stays
/// verbatim deliberately. The block codec independent interop projects describe
/// (a u32 header of count/order/width fields, fixed binomial predictors, a stop
/// sentinel) was tested against this corpus and **did not reproduce**: the
/// layout frames a handful of strokes exactly and fails the rest, and where it
/// frames, the predictor arithmetic diverges on real data. Do not implement
/// audio decode from that description without new evidence — a matched
/// WAV-in/nsmp-out differential pair is the missing oracle.
#[derive(Debug)]
pub struct SampleV3 {
    pub sections: Vec<section::Section4>,
}

impl cbin::Body for SampleV3 {
    fn read<R: Read + Seek>(r: &mut BodyReader<'_, R>, _: &Header) -> Result<Self, Error> {
        Ok(SampleV3 {
            sections: section::read_chain4(r)?,
        })
    }

    fn write<W: Write + Seek>(&self, w: &mut BodyWriter<'_, W>) -> Result<(), Error> {
        for s in &self.sections {
            s.write_to(w)?;
        }
        Ok(())
    }
}

/// Offset of the main name within the v3/v4 `hdr` payload.
const NAME_V3_AT: usize = 10;

/// End of the main-name field: the sub-name field starts here. The two fields
/// are what the filename convention joins — `Bass Clarinet 2` + `KG  mono` →
/// `Bass Clarinet 2_KG  mono 3.11`. Inferred from specimens; not confirmed on
/// hardware.
const NAME_V3_SUB_AT: usize = 76;

impl Cbin<SampleV3> {
    fn hdr(&self) -> Result<&section::Section4, Error> {
        section::find4(&self.body.sections, section::HDR4)
            .ok_or_else(|| ParseError::AssertFail("no hdr section".into()).into())
    }

    fn hdr_field(&self, from: usize, to: Option<usize>) -> Result<String, Error> {
        let hdr = self.hdr()?;
        let field = match to {
            Some(to) => hdr.payload.get(from..to),
            None => hdr.payload.get(from..),
        }
        .ok_or_else(|| {
            ParseError::AssertFail(format!("hdr section is {} bytes", hdr.payload.len()))
        })?;
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        Ok(String::from_utf8_lossy(&field[..end]).into_owned())
    }

    /// The instrument's main name.
    pub fn name(&self) -> Result<String, Error> {
        self.hdr_field(NAME_V3_AT, Some(NAME_V3_SUB_AT))
    }

    /// The sub name — the string after the `_` in the vendor's filenames.
    /// Empty on files that carry none.
    pub fn sub_name(&self) -> Result<String, Error> {
        self.hdr_field(NAME_V3_SUB_AT, None)
    }

    /// How many strokes the body carries — one `stk` section each.
    pub fn stroke_count(&self) -> usize {
        self.body
            .sections
            .iter()
            .filter(|s| s.is(section::STK4))
            .count()
    }

    /// Each stroke's `(global id, root key)` — the u32 its payload leads with,
    /// and the byte at offset 5. Inferred from specimens; not confirmed on
    /// hardware.
    fn stroke_ids(&self) -> Result<Vec<(u32, u8)>, Error> {
        self.body
            .sections
            .iter()
            .filter(|s| s.is(section::STK4))
            .map(|s| match (s.payload.get(0..4), s.payload.get(5)) {
                (Some(gid), Some(&root)) => Ok((u32::from_be_bytes(gid.try_into().unwrap()), root)),
                _ => Err(ParseError::AssertFail(format!(
                    "stroke payload is {} bytes, too short for its id fields",
                    s.payload.len()
                ))
                .into()),
            })
            .collect()
    }

    /// Keyboard zones, in stored order — high to low except `map` v14, which
    /// stores low to high. Each zone is verified against the stroke it names.
    pub fn zones(&self) -> Result<Vec<ZoneV3>, Error> {
        let map = section::find4(&self.body.sections, section::MAP4)
            .ok_or_else(|| ParseError::AssertFail("no map section".into()))?;
        Ok(zone::read_v3(
            map.version,
            &map.payload,
            &self.stroke_ids()?,
        )?)
    }
}

pub fn from_bytes(bytes: &[u8]) -> Result<Cbin<Sample>, Error> {
    read_from(&mut std::io::Cursor::new(bytes))
}

impl Cbin<Sample> {
    /// Serializes, recomputing the checksum over the body it just produced.
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

    /// Refuses the pre-2.0 library layout by name rather than by symptom.
    ///
    /// Version 8 is the original Sample Library, and its `map` section does not follow
    /// `801 + 15·(zones−1)` — the two specimens hold ten zones in 906 bytes and one in
    /// 798, against 936 and 801. The zone table is somewhere else, or shaped
    /// differently; either way the 2.0 reader walks off into the wrong bytes and
    /// reports a length complaint that says nothing about the real cause. The section
    /// chain, name and checksum are unaffected and still read.
    fn require_known_layout(&self) -> Result<(), Error> {
        if self.header.version < LIBRARY_2_VERSION {
            return Err(ParseError::AssertFail(format!(
                "content version {} predates Sample Library 2.0 and lays out its zone \
                 table differently; only the section chain and name are decoded",
                self.header.version
            ))
            .into());
        }
        Ok(())
    }

    /// Keyboard zones, high to low.
    pub fn zones(&self) -> Result<Vec<Zone>, Error> {
        self.require_known_layout()?;
        Ok(zone::read(&self.map()?.payload)?)
    }

    /// Sets one zone's top note. The strokes are untouched.
    pub fn set_zone_top_note(&mut self, index: usize, note: u8) -> Result<(), Error> {
        let map = section::find_mut(&mut self.body.sections, section::MAP)
            .ok_or_else(|| ParseError::AssertFail("no map section".into()))?;
        zone::set_top_note(&mut map.payload, index, note)?;
        Ok(())
    }

    /// One stroke per zone, **in [`Self::zones`] order** — which is not file order.
    ///
    /// Each zone names its stroke by id, and only instruments built in a single editor
    /// pass have those ids running parallel to the sections. Zipping this against
    /// `zones()` is therefore safe; indexing it as "the nth `stk` section" is not.
    pub fn strokes(&self) -> Result<Vec<Stroke>, Error> {
        self.require_known_layout()?;
        let zones = self.zones()?;
        let by_id = self.strokes_in_file_order()?;
        zones
            .iter()
            .map(|z| {
                by_id
                    .iter()
                    .find(|(id, _)| *id == u32::from(z.stroke_id))
                    .map(|(_, s)| *s)
                    .ok_or_else(|| {
                        ParseError::AssertFail(format!(
                            "zone reaching up to note {} names stroke {}, which the file \
                             does not contain",
                            z.top_note, z.stroke_id
                        ))
                        .into()
                    })
            })
            .collect()
    }

    /// Every stroke with the global id it carries, in the order the sections appear.
    ///
    /// The header length depends on a stroke's *position in the file*, so the read has
    /// to happen here, before anything reorders them.
    fn strokes_in_file_order(&self) -> Result<Vec<(u32, Stroke)>, Error> {
        let zones = zone::count(&self.map()?.payload)?;
        self.stroke_sections()
            .enumerate()
            .map(|(i, s)| {
                let id = s
                    .payload
                    .get(0..4)
                    .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                    .ok_or_else(|| {
                        ParseError::AssertFail(format!(
                            "stroke {i} is {} bytes, too short for its id",
                            s.payload.len()
                        ))
                    })?;
                Ok((id, stroke::read(&s.payload, i, zones)?))
            })
            .collect()
    }

    /// Retunes one zone by moving the note its sample plays untransposed at.
    ///
    /// `index` is into [`Self::zones`], matching [`Self::set_zone_top_note`] — so the
    /// stroke it reaches is the one that zone names, not the nth section. The two are
    /// the same file order only for instruments the editor built in a single pass.
    pub fn set_root_key(&mut self, index: usize, note: u8) -> Result<(), Error> {
        let zones = self.zones()?;
        let zone = zones
            .get(index)
            .ok_or_else(|| ParseError::AssertFail(format!("no zone {index}")))?;
        let wanted = u32::from(zone.stroke_id);
        let section = self
            .body
            .sections
            .iter_mut()
            .filter(|s| s.is(section::STK))
            .find(|s| {
                s.payload
                    .get(0..4)
                    .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                    == Some(wanted)
            })
            .ok_or_else(|| {
                ParseError::AssertFail(format!(
                    "zone {index} names stroke {wanted}, which the file does not contain"
                ))
            })?;
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
