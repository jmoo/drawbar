//! Read and write Clavia Nord keyboard files.
//!
//! > This is an unofficial community project. It is **not affiliated with, endorsed
//! > by, or supported by Clavia DMI AB**. "Nord" and the instrument names are
//! > Clavia's trademarks, used here only to identify which files this crate reads.
//!
//! The crate reads programs, live slots, songs, settings, presets, synth patches, and
//! sample and piano libraries across the Nord keyboard range. The formats are reverse
//! engineered from specimen files and hardware observation, never from Clavia's
//! software, and each is decoded to a different depth: some bodies decode to named
//! fields, and others are only container-verified. [`formats`] lists every format and
//! how far its decoding goes.
//!
//! # Reading and writing
//!
//! [`from_path`] and [`from_stream`] sniff any supported file and decode it into an
//! [`Entity`]. [`to_bytes`] and [`Entity::write_to`] serialize it again.
//! [`cbin_formats`] lists the CBIN format tags the reader dispatches.
//!
//! Every supported file reads and writes however much of its body decodes. Decoded
//! values are views over the stored body, bits that no field claims survive untouched,
//! and `to_bytes(from_stream(x)) == x` byte for byte. Archives are read-only. This
//! invariant is tested against a private corpus of real files.
//!
//! [`Entity::registry`] and [`Entity::registry_mut`] list and set the named fields of a
//! decoded body by dotted path, such as `center_panel.transpose`.
//!
//! # Modules
//!
//! - [`cbin`] is the container most formats share: header, checksum and body length.
//! - [`formats`] has one module per file format.
//! - [`fields`] and [`layout`] describe a decoded body's fields at runtime, and
//!   [`panel`] groups them the way the instrument's panel does.
//! - [`components`] and [`types`] are the typed values those fields hold.
//! - [`accept`] says which format tags each instrument family takes.
//! - [`wav`] moves decoded audio in and out of WAV files.
//!
//! # Features and dependencies
//!
//! The `bundle` feature reads ZIP archives: bundles, backups and Drum banks. It is off
//! by default and pulls in `zip`. The other runtime dependencies are `crcxx` and
//! `thiserror`. The crate does no I/O beyond `Read`, `Seek` and `Write`, so it runs
//! anywhere `std` does, including wasm. Device access lives in the companion `nord-usb`
//! crate.

pub mod accept;
pub mod bank;
pub mod bits;
pub mod cbin;
pub mod components;
pub mod crc;
pub mod error;
pub mod fields;
pub mod formats;
pub mod layout;
pub mod note;
pub mod panel;
pub mod types;
pub mod util;
pub mod wav;

use crate::cbin::{Cbin, RawBody};
use crate::formats::{
    cn3, midi, nc2, nc2d, nd2, nd3, ne3, ne4, ne5, ne6, ne7, ng2, nl4, nla1, no3, np, np2, np3,
    np4, np5, npip, npno, ns2, ns3, ns4, nsclassic, nsmp, nsmpproj, nw, nw2, sysex,
};
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use util::{peek, FileType};

use crate::error::{Error, ParseError};

/// A ZIP archive: an Electro 5 bundle or backup, or a Drum-family bank.
#[cfg(feature = "bundle")]
#[derive(Debug)]
pub enum Bundle {
    Drum2Bank(nd2::bank::Bank),
    Drum3KitBank(nd3::kit_bank::KitBank),
    Electro5(ne5::Bundle),
    /// A ZIP of CBIN files under any mix of tags, the shape of every model's bundle and
    /// backup. Reported by public documentation; not confirmed on hardware.
    /// Members are container-verified and kept raw under their archive paths. The paths
    /// encode the slot, which this crate does not interpret.
    Members(Vec<(String, Cbin<RawBody>)>),
}

/// Declare the per-role entity enums, one row per CBIN format: the variant, its body,
/// the module that reads and tags it, and the label [`Identity::kind`] prints.
macro_rules! roles {
    ($(
        $(#[$meta:meta])*
        $role:ident {
            $(
                $(#[$doc:meta])*
                $variant:ident($body:ty) = $($module:ident)::+, $kind:literal;
            )*
        }
    )*) => {
        $(
            $(#[$meta])*
            #[derive(Debug)]
            pub enum $role {
                $($(#[$doc])* $variant(Cbin<$body>),)*
            }

            impl Role for $role {
                fn identity(&self) -> Identity {
                    match self {
                        $($role::$variant(_) => Identity {
                            kind: $kind,
                            format: $($module)::+::FORMAT,
                        },)*
                    }
                }

                fn raw(&self) -> Option<&Cbin<RawBody>> {
                    match self {
                        $($role::$variant(f) => f.as_raw(),)*
                    }
                }

                fn write_to(&self, w: &mut (impl std::io::Write + Seek)) -> Result<(), Error> {
                    match self {
                        $($role::$variant(f) => f.write_to(w),)*
                    }
                }
            }
        )*

        /// Every CBIN tag a role enum holds, with the reader it dispatches to.
        const ROLE_READERS: &[(&str, ReadCbin)] = &[
            $($(($($module)::+::FORMAT, |mut r| {
                Ok(Entity::$role($role::$variant($($module)::+::read_from(&mut r)?)))
            }),)*)*
        ];
    };
}

/// What the role enums answer for each of their variants.
trait Role {
    fn identity(&self) -> Identity;
    fn raw(&self) -> Option<&Cbin<RawBody>>;
    fn write_to(&self, w: &mut (impl std::io::Write + Seek)) -> Result<(), Error>;
}

/// The container of an undecoded body, or `None` for a decoded one.
trait AsRaw {
    fn as_raw(&self) -> Option<&Cbin<RawBody>> {
        None
    }
}

impl AsRaw for Cbin<RawBody> {
    fn as_raw(&self) -> Option<&Cbin<RawBody>> {
        Some(self)
    }
}

macro_rules! decoded_bodies {
    ($($body:ty),* $(,)?) => {$(
        impl AsRaw for Cbin<$body> {}
    )*};
}

decoded_bodies!(
    ne5::Program,
    ne5::Settings,
    ne5::Song,
    ns2::Program,
    ns3::Program,
    ns3::SynthPreset,
    ns4::Program,
    ns4::organ_preset::OrganPreset,
    ns4::piano_preset::PianoPreset,
    ns4::synth::SynthPreset,
);

roles! {
    /// A stored program, one variant per model. Only the Electro 5 and Stage 2, 3 and 4
    /// bodies decode; the rest are container-verified stubs.
    ///
    /// Left unboxed for the reason [`Entity`] gives.
    #[allow(clippy::large_enum_variant)]
    Program {
        C2(RawBody) = nc2::program, "C2 program";
        C2D(RawBody) = nc2d::program, "C2D program";
        /// A Nord Drum 2 program (`nd2p`), usually met inside a bank archive.
        Drum2(RawBody) = nd2::program, "Drum 2 program";
        /// A Nord Drum 3P kit (`nd3k`), the model's equivalent of a program.
        Drum3(RawBody) = nd3::kit, "Drum 3P kit";
        /// Electro 3 and 3HP. The file does not say which.
        Electro3(RawBody) = ne3::program, "Electro 3 program";
        /// Electro 4 and 4D. The file does not say which.
        Electro4(RawBody) = ne4::program, "Electro 4 program";
        Electro5(ne5::Program) = ne5::program, "Electro 5 program";
        Electro6(RawBody) = ne6::program, "Electro 6 program";
        Electro7(RawBody) = ne7::program, "Electro 7 program";
        Grand(RawBody) = ng2::program, "Grand program";
        Lead4(RawBody) = nl4::program, "Lead 4 program";
        LeadA1(RawBody) = nla1::program, "Lead A1 program";
        Organ3(RawBody) = no3::program, "no3 organ program";
        Piano1(RawBody) = np::program, "Piano program";
        Piano2(RawBody) = np2::program, "Piano 2 program";
        Piano3(RawBody) = np3::program, "Piano 3 program";
        Piano4(RawBody) = np4::program, "Piano 4 program";
        Piano5(RawBody) = np5::program, "Piano 5 program";
        /// Stage 2 and 2 EX.
        Stage2(ns2::Program) = ns2::program, "Stage 2 program";
        Stage3(ns3::Program) = ns3::program, "Stage 3 program";
        Stage4(ns4::Program) = ns4::program, "Stage 4 program";
        /// Stage Classic and Stage EX.
        StageClassic(RawBody) = nsclassic::program, "Stage Classic program";
        Wave(RawBody) = nw::program, "Wave program";
        Wave2(RawBody) = nw2::program, "Wave 2 program";
    }

    /// The live buffer: the panel's current state, as opposed to a saved program. It has the
    /// same body as [`Program`] under its own format tag.
    ///
    /// Left unboxed for the reason [`Entity`] gives.
    #[allow(clippy::large_enum_variant)]
    Live {
        Electro4(RawBody) = ne4::live, "Electro 4 live slot";
        Electro5(ne5::Program) = ne5::live, "Electro 5 live slot";
        Electro6(RawBody) = ne6::live, "Electro 6 live slot";
        Electro7(RawBody) = ne7::live, "Electro 7 live slot";
        Grand(RawBody) = ng2::live, "Grand live slot";
        Piano1(RawBody) = np::live, "Piano live slot";
        Piano2(RawBody) = np2::live, "Piano 2 live slot";
        Piano3(RawBody) = np3::live, "Piano 3 live slot";
        Piano4(RawBody) = np4::live, "Piano 4 live slot";
        Piano5(RawBody) = np5::live, "Piano 5 live slot";
        Stage2(ns2::Program) = ns2::live, "Stage 2 live slot";
        Stage3(ns3::Program) = ns3::live, "Stage 3 live slot";
        Stage4(ns4::Program) = ns4::live, "Stage 4 live slot";
        Wave2(RawBody) = nw2::live, "Wave 2 live slot";
    }

    /// A stored song or set list, one variant per model that has them. Only the Electro 5
    /// body decodes; the Stage 3 body is container-verified and kept raw.
    Song {
        Electro5(ne5::Song) = ne5::song, "Electro 5 song / set";
        Stage3(RawBody) = ns3::song, "Stage 3 song";
    }

    /// The instrument's global settings, one variant per model. Only the Electro 5
    /// body decodes; the rest are container-verified stubs.
    Settings {
        C2(RawBody) = nc2::settings, "C2 settings";
        C2D(RawBody) = nc2d::settings, "C2D settings";
        Electro4(RawBody) = ne4::settings, "Electro 4 settings";
        Electro5(ne5::Settings) = ne5::settings, "Electro 5 settings";
        Electro6(RawBody) = ne6::settings, "Electro 6 settings";
        Electro7(RawBody) = ne7::settings, "Electro 7 settings";
        Grand(RawBody) = ng2::settings, "Grand settings";
        Lead4(RawBody) = nl4::settings, "Lead 4 settings";
        LeadA1(RawBody) = nla1::settings, "Lead A1 settings";
        Organ3(RawBody) = no3::settings, "no3 organ settings";
        Piano1(RawBody) = np::settings, "Piano settings";
        Piano2(RawBody) = np2::settings, "Piano 2 settings";
        Piano3(RawBody) = np3::settings, "Piano 3 settings";
        Piano4(RawBody) = np4::settings, "Piano 4 settings";
        Piano5(RawBody) = np5::settings, "Piano 5 settings";
        Stage2(RawBody) = ns2::settings, "Stage 2 settings";
        Stage3(RawBody) = ns3::settings, "Stage 3 settings";
        Stage4(RawBody) = ns4::settings, "Stage 4 settings";
        Wave(RawBody) = nw::settings, "Wave settings";
        Wave2(RawBody) = nw2::settings, "Wave 2 settings";
    }

    /// A synth patch, on the models that bank them separately from programs. The Stage 3
    /// and Stage 4 bodies decode.
    ///
    /// Left unboxed for the reason [`Entity`] gives.
    #[allow(clippy::large_enum_variant)]
    Synth {
        Stage2(RawBody) = ns2::synth, "Stage 2 synth patch";
        Stage3(ns3::SynthPreset) = ns3::synth, "Stage 3 synth patch";
        Stage4(ns4::synth::SynthPreset) = ns4::synth, "Stage 4 synth preset";
        StageClassic(RawBody) = nsclassic::synth, "Stage Classic synth patch";
    }

    /// A Lead performance, the multi-slot layer above that family's programs.
    Performance {
        Lead4(RawBody) = nl4::performance, "Lead 4 performance";
        LeadA1(RawBody) = nla1::performance, "Lead A1 performance";
    }

    /// A stored organ preset, on the models that keep them as files.
    ///
    /// Left unboxed for the reason [`Entity`] gives.
    #[allow(clippy::large_enum_variant)]
    OrganPreset {
        /// Electro 3 and 3HP (`neop`).
        Electro3(RawBody) = ne3::organ_preset, "Electro 3 organ preset";
        /// Stage 4 (`ns4o`).
        Stage4(ns4::organ_preset::OrganPreset) = ns4::organ_preset, "Stage 4 organ preset";
    }

    /// A stored piano preset, on the models that keep them as files.
    PianoPreset {
        /// Stage 4 (`ns4n`).
        Stage4(ns4::piano_preset::PianoPreset) = ns4::piano_preset, "Stage 4 piano preset";
    }
}

/// A sample instrument, decoded by generation. All three generations share the `nsmp`
/// tag, and the header version says which schema the body holds.
#[derive(Debug)]
pub enum Sample {
    V2(Cbin<nsmp::Sample>),
    /// The nsmp3 and nsmp4 generations: the section chain decodes, and the strokes are
    /// kept as stored and decode through [`nsmp::codec`].
    V3(Cbin<nsmp::SampleV3>),
}

impl Sample {
    pub fn name(&self) -> Result<String, Error> {
        match self {
            Sample::V2(s) => s.name(),
            Sample::V3(s) => s.name(),
        }
    }

    /// Longest name this generation's field takes.
    pub fn max_name_len(&self) -> usize {
        match self {
            Sample::V2(_) => nsmp::MAX_NAME_LEN,
            Sample::V3(_) => nsmp::MAX_NAME_V3_LEN,
        }
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), Error> {
        match self {
            Sample::V2(s) => s.set_name(name),
            Sample::V3(s) => s.set_name(name),
        }
    }

    /// Move the note a zone's sample plays untransposed at.
    pub fn set_root_key(&mut self, index: usize, note: u8) -> Result<(), Error> {
        match self {
            Sample::V2(s) => s.set_root_key(index, note),
            Sample::V3(s) => s.set_root_key(index, note),
        }
    }

    pub fn set_zone_top_note(&mut self, index: usize, note: u8) -> Result<(), Error> {
        match self {
            Sample::V2(s) => s.set_zone_top_note(index, note),
            Sample::V3(s) => s.set_zone_top_note(index, note),
        }
    }

    /// Whether this generation stores a zone's lowest note.
    ///
    /// False where zones tile: a zone reaches down to one above the next-lower zone's
    /// top, so only the top note is stored. [`Self::set_zone_low_note`] refuses there.
    pub fn has_low_note(&self) -> bool {
        matches!(self, Sample::V3(_))
    }

    /// Move a zone's lowest note, on the generations that store one. See
    /// [`Self::has_low_note`].
    pub fn set_zone_low_note(&mut self, index: usize, note: u8) -> Result<(), Error> {
        match self {
            Sample::V2(_) => Err(ParseError::AssertFail("v2 stores no low note".into()).into()),
            Sample::V3(s) => s.set_zone_low_note(index, note),
        }
    }

    /// Whether this instrument's zones can be retuned and remapped. Its name
    /// always can.
    ///
    /// False where the zone table does not read, or where a `map` that also
    /// describes the keyboard note by note cannot be recomputed from the layout.
    /// The setters' errors say which.
    pub fn zones_are_editable(&self) -> bool {
        match self {
            Sample::V2(s) => s.zones().is_ok() && s.strokes().is_ok(),
            Sample::V3(s) => s.zones_are_editable(),
        }
    }

    /// Which section chain this body's sections form. A narrow body whose `map` version
    /// names no chain with a known specimen is an error.
    pub fn chain(&self) -> Result<nsmp::Chain, Error> {
        match self {
            Sample::V2(s) => s.chain(),
            Sample::V3(_) => Ok(nsmp::Chain::Wide),
        }
    }

    /// Which generation's units this body's stroke streams are in. A content version
    /// past the generations the codec describes is refused.
    pub fn layout(&self) -> Result<nsmp::codec::Layout, Error> {
        match self {
            Sample::V2(_) => Ok(nsmp::codec::Layout::V2),
            Sample::V3(s) => nsmp::codec::Layout::from_version(s.header.version).ok_or_else(|| {
                ParseError::OutOfBounds {
                    value: format!("content version {}", s.header.version),
                    bound: format!(
                        "the generations this codec describes, below {}",
                        nsmp::codec::V5_FROM_VERSION
                    ),
                }
                .into()
            }),
        }
    }

    /// The generation to name in a report, taken from the content version, not the
    /// file name.
    pub fn generation(&self) -> &'static str {
        match self {
            Sample::V2(_) => "v2",
            Sample::V3(s) if s.header.version >= nsmp::V4_FROM_VERSION => "v4",
            Sample::V3(_) => "v3",
        }
    }

    /// Every zone in stored order, paired with the stream that plays it.
    ///
    /// One codec reads all three generations. Only the accessors that reach the zones
    /// and their streams differ.
    pub fn zones(&self) -> Result<Vec<nsmp::ZoneAudio<'_>>, Error> {
        match self {
            Sample::V2(s) => {
                let zones = s.zones()?;
                let strokes = s.strokes()?;
                zones
                    .iter()
                    .zip(&strokes)
                    .enumerate()
                    .map(|(i, (zone, stroke))| {
                        let (at, stream) = s.zone_stream(i)?;
                        Ok(nsmp::ZoneAudio {
                            root_key: stroke.root_key,
                            top_note: zone.top_note,
                            low_note: None,
                            at,
                            stream,
                        })
                    })
                    .collect()
            }
            Sample::V3(s) => {
                let zones = s.zones()?;
                zones
                    .iter()
                    .enumerate()
                    .map(|(i, zone)| {
                        let (at, stream) = s.zone_stream(i)?;
                        Ok(nsmp::ZoneAudio {
                            root_key: zone.root_key,
                            top_note: zone.top_note,
                            low_note: zone.low_note,
                            at,
                            stream,
                        })
                    })
                    .collect()
            }
        }
    }

    /// Every stroke's stream in file order, whether or not a zone names it.
    pub fn stroke_streams(&self) -> Vec<(usize, &[u8])> {
        match self {
            Sample::V2(s) => s.stroke_streams(),
            Sample::V3(s) => s.stroke_streams(),
        }
    }

    /// Serializes, recomputing the checksum over the body it just produced.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut out = std::io::Cursor::new(Vec::new());
        match self {
            Sample::V2(s) => s.write_to(&mut out),
            Sample::V3(s) => s.write_to(&mut out),
        }?;
        Ok(out.into_inner())
    }
}

/// One decoded file.
///
/// The decoded program variants are much the largest, because a decoded panel holds its
/// fields and the bytes it came from. They are left unboxed because one of these exists
/// per file being read, never in a collection.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Entity {
    /// An Electro 2 sample library, the only library format outside CBIN.
    Cne3(cn3::Cne3),
    Live(Live),
    /// A MIDI file carrying a Lead SysEx bank, kept raw.
    Midi(midi::Midi),
    OrganPreset(OrganPreset),
    /// A piano library (`npno`).
    Piano(npno::Piano),
    /// A Stage Classic piano library (`nsp`). ⚠️ Megabytes, read into memory whole.
    /// [`cbin::inspect`] answers container questions in O(1).
    PianoLibrary(Cbin<RawBody>),
    PianoPreset(PianoPreset),
    /// A C2 pipe-organ library (`npip`). Same caution as [`Entity::PianoLibrary`].
    PipeLibrary(Cbin<RawBody>),
    Performance(Performance),
    Program(Program),
    Sample(Sample),
    /// A Nord Sample Editor project (`.nsmpproj`), the file the editor saves and
    /// generates an `nsmp` from.
    SampleProject(nsmpproj::Project),
    Settings(Settings),
    Song(Song),
    Synth(Synth),
    /// A Lead 1, 2, 2X or 3 SysEx dump, kept raw.
    Sysex(sysex::Sysex),
    #[cfg(feature = "bundle")]
    Bundle(Bundle),
}

/// Sniff `reader` and decode one supported file into an [`Entity`]. The inverse is
/// [`to_bytes`]. The leading bytes identify the container, and a CBIN body is then
/// dispatched on the format tag at offset 8.
pub fn from_stream(reader: &mut (impl Read + Seek + Sized)) -> Result<Entity, Error> {
    let header = peek(reader)?;

    match header.file_type {
        #[cfg(feature = "bundle")]
        FileType::Zip => read_zip(reader),
        #[cfg(not(feature = "bundle"))]
        FileType::Zip => {
            Err(ParseError::UnknownFileType("zip (bundle feature disabled)".to_string()).into())
        }
        FileType::Sysex => Ok(Entity::Sysex(sysex::Sysex::read_from(reader)?)),
        FileType::Midi => Ok(Entity::Midi(midi::Midi::read_from(reader)?)),
        FileType::Cne3 => Ok(Entity::Cne3(cn3::Cne3::read_from(reader)?)),
        FileType::SampleProject => Ok(Entity::SampleProject(nsmpproj::Project::read_from(reader)?)),
        FileType::Cbin => read_cbin(reader, header.format.as_str()),
        e => Err(ParseError::UnknownFileType(e.as_str().to_string()).into()),
    }
}

/// Reads one CBIN file whose tag the table has already matched.
type ReadCbin = fn(&mut dyn ReadSeek) -> Result<Entity, Error>;

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek + ?Sized> ReadSeek for T {}

/// Every CBIN tag outside the role enums, with the reader it dispatches to.
const LIBRARY_READERS: &[(&str, ReadCbin)] = &[
    (nsmp::FORMAT, |mut r| {
        let file: Cbin<nsmp::AnyBody> = cbin::read(&mut r, nsmp::FORMAT)?;
        let header = file.header;
        Ok(Entity::Sample(match file.body {
            nsmp::AnyBody::V2(body) => Sample::V2(Cbin { header, body }),
            nsmp::AnyBody::V3(body) => Sample::V3(Cbin { header, body }),
        }))
    }),
    (npno::FORMAT, |mut r| {
        Ok(Entity::Piano(npno::Piano::read_from(&mut r)?))
    }),
    (npip::pipe_library::FORMAT, |mut r| {
        Ok(Entity::PipeLibrary(npip::pipe_library::read_from(&mut r)?))
    }),
    (nsclassic::piano_library::FORMAT, |mut r| {
        Ok(Entity::PianoLibrary(nsclassic::piano_library::read_from(
            &mut r,
        )?))
    }),
];

/// Every CBIN tag [`from_stream`] reads, with the reader it dispatches to.
fn cbin_readers() -> impl Iterator<Item = &'static (&'static str, ReadCbin)> {
    LIBRARY_READERS.iter().chain(ROLE_READERS)
}

/// Every CBIN format tag [`from_stream`] reads, NULs preserved.
pub fn cbin_formats() -> impl Iterator<Item = &'static str> {
    cbin_readers().map(|(format, _)| *format)
}

/// One CBIN file, dispatched by the tag at offset 8.
fn read_cbin(reader: &mut (impl Read + Seek), tag: &str) -> Result<Entity, Error> {
    let (_, read) = cbin_readers()
        .find(|(format, _)| *format == tag)
        .ok_or_else(|| ParseError::UnknownFormat(tag.to_string()))?;
    read(reader)
}

/// Which archive a ZIP is, from the members the walks below will see.
#[cfg(feature = "bundle")]
enum ZipKind {
    Electro5,
    Drum2,
    Drum3,
    Members,
}

/// One ZIP file: an Electro 5 bundle or backup (it carries a `meta.xml`
/// manifest), or a Drum bank (members are all one CBIN format).
#[cfg(feature = "bundle")]
fn read_zip(reader: &mut (impl Read + Seek)) -> Result<Entity, Error> {
    let start = reader.stream_position()?;
    let kind = {
        let zip = zip::ZipArchive::new(&mut *reader)?;
        // Counting a directory or the manifest would make an archive of directories an
        // empty bundle, and a `kits/` entry would stop a drum bank from classifying as one.
        let names: Vec<&str> = zip
            .file_names()
            .filter(|name| formats::is_member(name))
            .collect();
        // An archive with nothing in it would satisfy the all-members checks below
        // vacuously and read as a drum bank holding no programs.
        if names.is_empty() {
            return Err(ParseError::AssertFail("the archive holds no members".into()).into());
        }
        // ⚠️ `meta.xml` is shared across product families; only `.ne5*` members identify
        // an Electro 5 bundle.
        if names.iter().any(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e.to_string_lossy().starts_with("ne5"))
        }) {
            ZipKind::Electro5
        } else if names.iter().all(|n| n.ends_with(".nd2p")) {
            ZipKind::Drum2
        } else if names.iter().all(|n| n.ends_with(".nd3k")) {
            ZipKind::Drum3
        } else {
            // A bundle only if every member is a CBIN file, which `zip_raw_members`
            // checks.
            ZipKind::Members
        }
    };
    reader.seek(std::io::SeekFrom::Start(start))?;

    Ok(Entity::Bundle(match kind {
        ZipKind::Drum2 => Bundle::Drum2Bank(nd2::bank::read_from(reader)?),
        ZipKind::Drum3 => Bundle::Drum3KitBank(nd3::kit_bank::read_from(reader)?),
        ZipKind::Members => Bundle::Members(formats::zip_raw_members(reader)?),
        ZipKind::Electro5 => Bundle::Electro5(ne5::Bundle::read_from(reader)?),
    }))
}

/// [`from_stream`] over a buffered read of the file at `path`.
pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Entity, Error> {
    from_stream(&mut BufReader::new(File::open(path)?))
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    /// A registry read and written through the entity lands on the same field the
    /// body's own accessors reach, so neither consumer needs to name the body type.
    #[test]
    fn the_entity_registry_reads_and_writes_the_body() {
        let mut entity = Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().unwrap(),
        )));

        let before = entity.registry().unwrap().fields();
        assert!(before.iter().any(|f| f.path == "center_panel.transpose"));

        entity
            .registry_mut()
            .unwrap()
            .set_field("center_panel.transpose", "-5")
            .unwrap();
        let after = entity.registry().unwrap().fields();
        let transpose = after
            .iter()
            .find(|f| f.path == "center_panel.transpose")
            .unwrap();
        assert_eq!(transpose.value, "-5");
    }

    /// Each class the Electro 5 keeps a default for makes a file under that class's tag,
    /// and a set list points at the programs it is given.
    #[test]
    fn a_new_electro_5_file_has_its_class_and_its_set_list() {
        use accept::{Family, Slot};
        let programs = [0, 1, 2, 3].map(|slot| ne5::program::Location::new(0, slot).unwrap());
        for class in Slot::ALL {
            let made = Entity::electro5(class, programs).map(Result::unwrap);
            let tag = Family::Electro5.tag(class);
            match class {
                Slot::Piano | Slot::Sample => assert!(made.is_none(), "{class:?}"),
                _ => assert_eq!(made.map(|e| e.identity().format), tag, "{class:?}"),
            }
        }
        let Some(Ok(Entity::Song(Song::Electro5(song)))) =
            Entity::electro5(Slot::SetList, programs)
        else {
            panic!("a set list is a song");
        };
        assert_eq!(song.programs(), programs);
    }
}

#[cfg(all(test, feature = "bundle"))]
mod bundle_tests {
    use super::*;
    use crate::cbin::{Cbin, Header, RawBody};
    use std::io::{Cursor, Write};

    fn member(tag: &str) -> Vec<u8> {
        let file = Cbin {
            header: Header::new(tag, (0, 0), 4),
            body: RawBody(vec![0x5A; 16]),
        };
        let mut out = Cursor::new(Vec::new());
        file.write_to(&mut out).unwrap();
        out.into_inner()
    }

    /// A stored archive of `members`; a name ending in `/` becomes a directory entry.
    fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in members {
            match name.strip_suffix('/') {
                Some(directory) => zip.add_directory(directory, stored).unwrap(),
                None => {
                    zip.start_file(name.to_string(), stored).unwrap();
                    zip.write_all(bytes).unwrap();
                }
            }
        }
        zip.finish().unwrap().into_inner()
    }

    /// A ZIP of mixed CBIN members, the reported bundle shape, reads as
    /// [`Bundle::Members`] with paths preserved.
    #[test]
    fn a_zip_of_mixed_cbin_members_is_a_bundle() {
        let a = member("ns3f");
        let b = member("ns3y");
        let bytes = archive(&[("Bank A/One.ns3f", &a), ("presets/Two.ns3y", &b)]);

        let entity = from_stream(&mut Cursor::new(bytes)).unwrap();
        let Entity::Bundle(Bundle::Members(members)) = entity else {
            panic!("decoded to something other than a member bundle");
        };
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].0, "Bank A/One.ns3f");
        assert_eq!(&members[0].1.header.tag, b"ns3f");
        assert_eq!(&members[1].1.header.tag, b"ns3y");
    }

    /// An empty archive would pass every all-members check and read as a drum bank
    /// holding no programs.
    #[test]
    fn an_empty_zip_is_refused() {
        let bytes = archive(&[]);
        assert!(from_stream(&mut Cursor::new(bytes)).is_err());
    }

    /// Directory entries and a manifest are not members, so an archive of only those is
    /// refused like an empty one.
    #[test]
    fn a_zip_of_directories_and_a_manifest_is_refused() {
        let bytes = archive(&[("kits/", b""), ("meta.xml", b"<meta/>")]);
        let err = from_stream(&mut Cursor::new(bytes)).unwrap_err();
        assert!(
            err.to_string().contains("no members"),
            "refused for the wrong reason: {err}"
        );
    }

    /// A backup's directory entries do not stop a bank whose files are all one CBIN
    /// format from reading as that bank.
    #[test]
    fn a_directory_entry_does_not_hide_a_drum_bank() {
        let program = member("nd2p");
        let bytes = archive(&[("kits/", b""), ("kits/One.nd2p", &program)]);
        let entity = from_stream(&mut Cursor::new(bytes)).unwrap();
        assert!(
            matches!(entity, Entity::Bundle(Bundle::Drum2Bank(_))),
            "a `kits/` entry left it classified as {}",
            entity.identity().kind,
        );
    }

    /// A drum bank carrying a backup manifest reads as the bank; the manifest is not
    /// read as one of its programs.
    #[test]
    fn a_manifest_does_not_break_a_drum_bank() {
        let program = member("nd2p");
        let bytes = archive(&[("meta.xml", b"<meta/>"), ("One.nd2p", &program)]);
        let entity = from_stream(&mut Cursor::new(bytes)).unwrap();
        let Entity::Bundle(Bundle::Drum2Bank(bank)) = entity else {
            panic!("classified as {}", entity.identity().kind);
        };
        assert_eq!(bank.programs.len(), 1);
        assert_eq!(bank.programs[0].0, "One.nd2p");
    }

    /// A ZIP holding anything that is not a CBIN file is not a bundle.
    #[test]
    fn a_zip_with_a_non_cbin_member_is_refused() {
        let a = member("ns3f");
        let bytes = archive(&[("One.ns3f", &a), ("readme.txt", b"hello")]);
        assert!(from_stream(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn a_zip_is_read_from_the_callers_current_position() {
        let member = member("ns3f");
        let bytes = archive(&[("Bank A/One.ns3f", &member)]);
        let prefix_len = 7;
        let mut prefixed = vec![0xa5; prefix_len];
        prefixed.extend(bytes);
        let mut reader = Cursor::new(prefixed);
        reader.set_position(prefix_len as u64);

        let entity = from_stream(&mut reader).unwrap();
        assert!(matches!(entity, Entity::Bundle(Bundle::Members(_))));
    }
}

/// Serialize an [`Entity`] back to the bytes of its file. The inverse is
/// [`from_stream`].
///
/// For every format this crate reads, `to_bytes(from_stream(x)) == x` byte for byte,
/// whichever header generation `x` carries. Decoded values are views over the stored
/// body, so an unedited file re-emits unchanged; `nord verify` checks this against real
/// files. Fixed-length formats declare their body length on their [`cbin::Body`] impl,
/// and the container refuses to emit a file of the wrong size.
///
/// Bundles are an error: a bundle is a ZIP walk over other entities, which this crate
/// does not re-encode.
pub fn to_bytes(entity: &Entity) -> Result<Vec<u8>, Error> {
    use std::io::Cursor;

    let mut out = Cursor::new(Vec::new());
    entity.write_to(&mut out)?;
    Ok(out.into_inner())
}

/// What an entity is: a human label and the format tag its file carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// Model then role, as the summary prints it: `"Electro 6 program"`.
    pub kind: &'static str,
    /// The CBIN tag, or the carrier name (`zip`, `syx`, `mid`, `cn3`).
    pub format: &'static str,
}

macro_rules! registry_bodies {
    ($($body:ty),* $(,)?) => {$(
        impl fields::Registry for Cbin<$body> {
            fn fields(&self) -> Vec<fields::Field> {
                self.body.fields()
            }
            fn field_values(&self) -> Vec<fields::FieldValue> {
                self.body.field_values()
            }
            fn set_field(&mut self, path: &str, value: &str) -> Result<(), fields::FieldError> {
                self.body.set_field(path, value)
            }
        }
    )*};
}

registry_bodies!(
    ne5::Program,
    ne5::Settings,
    ns2::Program,
    ns3::Program,
    ns3::SynthPreset,
    ns4::Program,
    ns4::organ_preset::OrganPreset,
    ns4::piano_preset::PianoPreset,
    ns4::synth::SynthPreset,
);

/// The entities that declare a registry, borrowed through `&` or `&mut`, so one list
/// serves both directions. The live buffer is the program body under another tag, so
/// the two share an arm. `ne5::Song` declares no public fields, so it has no registry.
macro_rules! with_registry {
    ($entity:expr, $($reference:tt)*) => {
        match $entity {
            Entity::Program(Program::Electro5(f)) | Entity::Live(Live::Electro5(f)) => {
                Some(f as $($reference)* dyn fields::Registry)
            }
            Entity::Program(Program::Stage2(f)) | Entity::Live(Live::Stage2(f)) => {
                Some(f as $($reference)* dyn fields::Registry)
            }
            Entity::Program(Program::Stage3(f)) | Entity::Live(Live::Stage3(f)) => {
                Some(f as $($reference)* dyn fields::Registry)
            }
            Entity::Program(Program::Stage4(f)) | Entity::Live(Live::Stage4(f)) => {
                Some(f as $($reference)* dyn fields::Registry)
            }
            Entity::Settings(Settings::Electro5(f)) => Some(f as $($reference)* dyn fields::Registry),
            Entity::Synth(Synth::Stage3(f)) => Some(f as $($reference)* dyn fields::Registry),
            Entity::Synth(Synth::Stage4(f)) => Some(f as $($reference)* dyn fields::Registry),
            Entity::OrganPreset(OrganPreset::Stage4(f)) => {
                Some(f as $($reference)* dyn fields::Registry)
            }
            Entity::PianoPreset(PianoPreset::Stage4(f)) => {
                Some(f as $($reference)* dyn fields::Registry)
            }
            _ => None,
        }
    };
}

impl Entity {
    /// The container of a stub-backed entity, one whose body is container-verified but
    /// undecoded. `None` for the decoded formats and the non-CBIN carriers.
    pub fn raw(&self) -> Option<&Cbin<RawBody>> {
        match self {
            Entity::Live(e) => e.raw(),
            Entity::OrganPreset(e) => e.raw(),
            Entity::Performance(e) => e.raw(),
            Entity::PianoPreset(e) => e.raw(),
            Entity::Program(e) => e.raw(),
            Entity::Settings(e) => e.raw(),
            Entity::Song(e) => e.raw(),
            Entity::Synth(e) => e.raw(),
            Entity::PianoLibrary(f) | Entity::PipeLibrary(f) => Some(f),
            _ => None,
        }
    }

    /// A new Electro 5 file of one class: the default body, addressed to the first slot. A
    /// set list points its four entries at `set_list`. `None` for the piano and sample
    /// libraries, which have no default.
    pub fn electro5(
        class: accept::Slot,
        set_list: [ne5::program::Location; ne5::song::PROGRAM_COUNT],
    ) -> Option<Result<Entity, Error>> {
        use accept::Slot;
        Some(match class {
            Slot::Program => Ok(Entity::Program(Program::Electro5(ne5::program::new(
                Default::default(),
            )))),
            Slot::Live => Ok(Entity::Live(Live::Electro5(ne5::live::new(
                Default::default(),
            )))),
            Slot::Settings => Ok(Entity::Settings(Settings::Electro5(ne5::settings::new()))),
            Slot::SetList => {
                ne5::song::new(Default::default(), ne5::song::DEFAULT_VERSION, set_list)
                    .map(|song| Entity::Song(Song::Electro5(song)))
            }
            Slot::Piano | Slot::Sample => return None,
        })
    }

    /// The generated field registry behind this entity, for reading.
    /// `None` for the container-verified stubs and the non-panel carriers.
    pub fn registry(&self) -> Option<&dyn fields::Registry> {
        with_registry!(self, &)
    }

    /// The registry, for setting fields. Every body with a registry supports both
    /// reading and setting.
    pub fn registry_mut(&mut self) -> Option<&mut dyn fields::Registry> {
        with_registry!(self, &mut)
    }

    /// The entity's [`Identity`]: its human label and the tag its file carries.
    pub fn identity(&self) -> Identity {
        let id = |kind, format| Identity { kind, format };
        match self {
            Entity::Live(e) => e.identity(),
            Entity::OrganPreset(e) => e.identity(),
            Entity::Performance(e) => e.identity(),
            Entity::PianoPreset(e) => e.identity(),
            Entity::Program(e) => e.identity(),
            Entity::Settings(e) => e.identity(),
            Entity::Song(e) => e.identity(),
            Entity::Synth(e) => e.identity(),
            Entity::Piano(_) => id("piano library", npno::FORMAT),
            Entity::PianoLibrary(_) => id(
                "Stage Classic piano library",
                nsclassic::piano_library::FORMAT,
            ),
            Entity::PipeLibrary(_) => id("C2 pipe library", npip::pipe_library::FORMAT),
            Entity::Sample(Sample::V2(_)) => id("sample instrument", nsmp::FORMAT),
            Entity::Sample(Sample::V3(_)) => id("sample instrument (nsmp3/nsmp4)", nsmp::FORMAT),
            Entity::SampleProject(_) => id("Sample Editor project", nsmpproj::FORMAT),
            Entity::Sysex(_) => id("SysEx dump", "syx"),
            Entity::Midi(_) => id("MIDI file", "mid"),
            Entity::Cne3(_) => id("Electro 2 library", "cn3"),
            #[cfg(feature = "bundle")]
            Entity::Bundle(_) => id("bundle", "zip"),
        }
    }

    /// Re-encode to `w`, byte-exact for anything read and unedited.
    ///
    /// Bundles are an error: the archive layer does not re-encode, and an approximate
    /// archive would not match its source.
    pub fn write_to(&self, w: &mut (impl std::io::Write + Seek)) -> Result<(), Error> {
        match self {
            Entity::Live(e) => e.write_to(w),
            Entity::OrganPreset(e) => e.write_to(w),
            Entity::Performance(e) => e.write_to(w),
            Entity::PianoPreset(e) => e.write_to(w),
            Entity::Program(e) => e.write_to(w),
            Entity::Settings(e) => e.write_to(w),
            Entity::Song(e) => e.write_to(w),
            Entity::Synth(e) => e.write_to(w),
            Entity::Cne3(f) => f.write_to(w),
            Entity::Midi(f) => f.write_to(w),
            Entity::PianoLibrary(f) | Entity::PipeLibrary(f) => f.write_to(w),
            Entity::Piano(f) => f.write_to(w),
            Entity::Sample(Sample::V2(f)) => f.write_to(w),
            Entity::Sample(Sample::V3(f)) => f.write_to(w),
            Entity::SampleProject(f) => f.write_to(w),
            Entity::Sysex(f) => f.write_to(w),
            #[cfg(feature = "bundle")]
            Entity::Bundle(_) => Err(ParseError::AssertFail(
                "bundles are archives; re-encoding one is not supported".into(),
            )
            .into()),
        }
    }
}
