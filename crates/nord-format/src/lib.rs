//! Parse and write Clavia / Nord keyboard binary file formats.
//!
//! > This is an unofficial, community project: **not affiliated with, endorsed
//! > by, or supported by Clavia DMI AB**. "Nord" and the instrument names are
//! > Clavia's trademarks, used here only to identify which files this crate
//! > reads.
//!
//! The formats — programs, live slots, songs, settings, presets, synth
//! patches, sample and piano libraries, across the Nord keyboard range — are
//! reverse engineered from specimen files and hardware observation, never
//! from Clavia's software, and are in varying states of completion: some
//! bodies decode to named fields, others are container-verified and kept
//! verbatim. [`formats`] is the map of what exists and how far each format's
//! decoding goes.
//!
//! Completeness never gates I/O. Every supported file reads and writes
//! whether its body decodes fully, partially, or not at all: decoded values
//! are views over a verbatim body, bits no field claims survive untouched,
//! and `to_bytes(from_stream(x)) == x` bit-for-bit (archives are read-only).
//! That invariant is tested against a private corpus of more than 10,000 real
//! files.
//!
//! [`from_path`] / [`from_stream`] sniff any supported file and decode it
//! into an [`Entity`]; [`to_bytes`] is the inverse.
//!
//! Runtime dependencies are `crcxx` and `thiserror` (plus `zip` behind the
//! `bundle` feature), and no I/O happens beyond `Read`/`Seek`/`Write`, so the
//! crate runs anywhere `std` does — wasm included. Device access lives in the
//! companion `nord-usb` crate, in the same repository.

pub mod bank;
pub mod bits;
pub mod cbin;
pub mod components;
pub mod crc;
pub mod error;
pub mod fields;
pub mod formats;
pub mod layout;
pub mod types;
pub mod util;

use crate::cbin::{Cbin, RawBody};
use crate::formats::{
    cn3, midi, nc2, nc2d, nd2, nd3, ne3, ne4, ne5, ne6, ne7, ng2, nl4, nla1, no3, np, np2, np3,
    np4, np5, npip, npno, ns2, ns3, ns4, nsclassic, nsmp, nw, nw2, sysex,
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
    /// A ZIP of CBIN files under any mix of tags — every model's bundle/backup
    /// shape, verified against real factory restores (`.no3b`, `.nc2b`,
    /// `.nl4b`). Members are kept container-verified and raw, under their
    /// archive paths — which encode the slot, uninterpreted here.
    Members(Vec<(String, Cbin<RawBody>)>),
}

/// A stored program, one variant per model. Only the Electro 5 and the three
/// Stages decode anything of the body; the rest are container-verified stubs.
///
/// Left unboxed for the reason [`Entity`] gives.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Program {
    C2(Cbin<RawBody>),
    C2D(Cbin<RawBody>),
    /// A Nord Drum 2 program (`nd2p`), usually met inside a bank archive.
    Drum2(Cbin<RawBody>),
    /// A Nord Drum 3P kit (`nd3k`) — the model's program-equivalent.
    Drum3(Cbin<RawBody>),
    /// Electro 3 and 3HP — the file does not say which.
    Electro3(Cbin<RawBody>),
    /// Electro 4 and 4D — likewise.
    Electro4(Cbin<RawBody>),
    Electro5(Cbin<ne5::Program>),
    Electro6(Cbin<RawBody>),
    Electro7(Cbin<RawBody>),
    Grand(Cbin<RawBody>),
    Lead4(Cbin<RawBody>),
    LeadA1(Cbin<RawBody>),
    Organ3(Cbin<RawBody>),
    Piano1(Cbin<RawBody>),
    Piano2(Cbin<RawBody>),
    Piano3(Cbin<RawBody>),
    Piano4(Cbin<RawBody>),
    Piano5(Cbin<RawBody>),
    /// Stage 2 and 2 EX.
    Stage2(Cbin<ns2::Program>),
    Stage3(Cbin<ns3::Program>),
    Stage4(Cbin<ns4::Program>),
    /// Stage Classic and Stage EX.
    StageClassic(Cbin<RawBody>),
    Wave(Cbin<RawBody>),
    Wave2(Cbin<RawBody>),
}

/// The live buffer — the panel as it stands, not a saved program. Same body as
/// [`Program`], under its own format tag.
///
/// Left unboxed for the reason [`Entity`] gives.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Live {
    Electro4(Cbin<RawBody>),
    Electro5(Cbin<ne5::Program>),
    Electro6(Cbin<RawBody>),
    Electro7(Cbin<RawBody>),
    Grand(Cbin<RawBody>),
    Piano1(Cbin<RawBody>),
    Piano2(Cbin<RawBody>),
    Piano3(Cbin<RawBody>),
    Piano4(Cbin<RawBody>),
    Piano5(Cbin<RawBody>),
    Stage2(Cbin<ns2::Program>),
    Stage3(Cbin<ns3::Program>),
    Stage4(Cbin<ns4::Program>),
    Wave2(Cbin<RawBody>),
}

/// A stored song / set list, one variant per model that has them. Only the
/// Electro 5 body decodes; the Stage 3 is container-verified verbatim.
#[derive(Debug)]
pub enum Song {
    Electro5(Cbin<ne5::Song>),
    Stage3(Cbin<RawBody>),
}

/// The instrument's global settings, one variant per model. Only the Electro 5
/// body decodes; the rest are container-verified stubs.
#[derive(Debug)]
pub enum Settings {
    C2(Cbin<RawBody>),
    C2D(Cbin<RawBody>),
    Electro4(Cbin<RawBody>),
    Electro5(Cbin<ne5::Settings>),
    Electro6(Cbin<RawBody>),
    Electro7(Cbin<RawBody>),
    Grand(Cbin<RawBody>),
    Lead4(Cbin<RawBody>),
    LeadA1(Cbin<RawBody>),
    Organ3(Cbin<RawBody>),
    Piano1(Cbin<RawBody>),
    Piano2(Cbin<RawBody>),
    Piano3(Cbin<RawBody>),
    Piano4(Cbin<RawBody>),
    Piano5(Cbin<RawBody>),
    Stage2(Cbin<RawBody>),
    Stage3(Cbin<RawBody>),
    Stage4(Cbin<RawBody>),
    Wave(Cbin<RawBody>),
    Wave2(Cbin<RawBody>),
}

/// A synth patch, on the models that bank them separately from programs. Only
/// the Stage 4's decodes.
///
/// Left unboxed for the reason [`Entity`] gives.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Synth {
    Stage2(Cbin<RawBody>),
    Stage3(Cbin<ns3::SynthPreset>),
    Stage4(Cbin<ns4::synth::SynthPreset>),
    StageClassic(Cbin<RawBody>),
}

/// A Lead performance — the multi-slot layer above that family's programs.
#[derive(Debug)]
pub enum Performance {
    Lead4(Cbin<RawBody>),
    LeadA1(Cbin<RawBody>),
}

/// A stored organ preset, on the models that keep them as files.
///
/// Left unboxed for the reason [`Entity`] gives.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum OrganPreset {
    /// Electro 3 and 3HP (`neop`).
    Electro3(Cbin<RawBody>),
    /// Stage 4 (`ns4o`).
    Stage4(Cbin<ns4::organ_preset::OrganPreset>),
}

/// A stored piano preset, on the models that keep them as files.
#[derive(Debug)]
pub enum PianoPreset {
    /// Stage 4 (`ns4n`).
    Stage4(Cbin<ns4::piano_preset::PianoPreset>),
}

/// A sample instrument, decoded by generation: all three share the `nsmp` tag,
/// and the header version says which schema the body holds.
#[derive(Debug)]
pub enum Sample {
    V2(Cbin<nsmp::Sample>),
    /// The nsmp3/nsmp4 generations: section chain decoded, strokes verbatim.
    V3(Cbin<nsmp::SampleV3>),
}

/// One decoded file.
///
/// The decoded program variants are much the largest: a decoded panel holds its
/// fields *and* the bytes it came from. Left unboxed — one of these exists per
/// file being read, never in a collection.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Entity {
    /// An Electro 2 sample library — the one non-CBIN library format.
    Cne3(cn3::Cne3),
    Live(Live),
    /// A MIDI carrier for a Lead SysEx bank, verbatim.
    Midi(midi::Midi),
    OrganPreset(OrganPreset),
    /// A piano library (`npno`).
    Piano(npno::Piano),
    /// A Stage Classic piano library (`nsp`). ⚠️ Megabytes, allocated whole —
    /// [`cbin::inspect`] answers container questions in O(1).
    PianoLibrary(Cbin<RawBody>),
    PianoPreset(PianoPreset),
    /// A C2 pipe-organ library (`npip`). Same caution as [`Entity::PianoLibrary`].
    PipeLibrary(Cbin<RawBody>),
    Performance(Performance),
    Program(Program),
    Sample(Sample),
    Settings(Settings),
    Song(Song),
    Synth(Synth),
    /// A Lead 1/2/2X/3 SysEx dump, verbatim.
    Sysex(sysex::Sysex),
    #[cfg(feature = "bundle")]
    Bundle(Bundle),
}

/// Sniff `reader` and decode one supported file into an [`Entity`] — the
/// counterpart to [`to_bytes`]. The container class comes from the leading
/// bytes; a CBIN body is then dispatched on the format tag at offset 8.
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
        FileType::Cbin => read_cbin(reader, header.format.as_str()),
        e => Err(ParseError::UnknownFileType(e.as_str().to_string()).into()),
    }
}

/// One CBIN file, dispatched by the tag at offset 8.
fn read_cbin(reader: &mut (impl Read + Seek), tag: &str) -> Result<Entity, Error> {
    use Entity as E;

    Ok(match tag {
        // The shared library formats.
        nsmp::FORMAT => {
            let file: Cbin<nsmp::AnyBody> = cbin::read(reader, nsmp::FORMAT)?;
            let header = file.header;
            E::Sample(match file.body {
                nsmp::AnyBody::V2(body) => Sample::V2(Cbin { header, body }),
                nsmp::AnyBody::V3(body) => Sample::V3(Cbin { header, body }),
            })
        }
        npno::FORMAT => E::Piano(npno::Piano::read_from(reader)?),
        npip::pipe_library::FORMAT => E::PipeLibrary(npip::pipe_library::read_from(reader)?),
        nsclassic::piano_library::FORMAT => {
            E::PianoLibrary(nsclassic::piano_library::read_from(reader)?)
        }

        // Electro.
        ne3::program::FORMAT => E::Program(Program::Electro3(ne3::program::read_from(reader)?)),
        ne3::organ_preset::FORMAT => {
            E::OrganPreset(OrganPreset::Electro3(ne3::organ_preset::read_from(reader)?))
        }
        ne4::program::FORMAT => E::Program(Program::Electro4(ne4::program::read_from(reader)?)),
        ne4::live::FORMAT => E::Live(Live::Electro4(ne4::live::read_from(reader)?)),
        ne4::settings::FORMAT => E::Settings(Settings::Electro4(ne4::settings::read_from(reader)?)),
        ne5::program::FORMAT => E::Program(Program::Electro5(ne5::program::read_from(reader)?)),
        ne5::live::FORMAT => E::Live(Live::Electro5(ne5::live::read_from(reader)?)),
        ne5::song::FORMAT => E::Song(Song::Electro5(ne5::song::read_from(reader)?)),
        ne5::settings::FORMAT => E::Settings(Settings::Electro5(ne5::settings::read_from(reader)?)),
        ne6::program::FORMAT => E::Program(Program::Electro6(ne6::program::read_from(reader)?)),
        ne6::live::FORMAT => E::Live(Live::Electro6(ne6::live::read_from(reader)?)),
        ne6::settings::FORMAT => E::Settings(Settings::Electro6(ne6::settings::read_from(reader)?)),
        ne7::program::FORMAT => E::Program(Program::Electro7(ne7::program::read_from(reader)?)),
        ne7::live::FORMAT => E::Live(Live::Electro7(ne7::live::read_from(reader)?)),
        ne7::settings::FORMAT => E::Settings(Settings::Electro7(ne7::settings::read_from(reader)?)),

        // Stage.
        nsclassic::program::FORMAT => E::Program(Program::StageClassic(
            nsclassic::program::read_from(reader)?,
        )),
        nsclassic::synth::FORMAT => {
            E::Synth(Synth::StageClassic(nsclassic::synth::read_from(reader)?))
        }
        ns2::program::FORMAT => E::Program(Program::Stage2(ns2::program::read_from(reader)?)),
        ns2::live::FORMAT => E::Live(Live::Stage2(ns2::live::read_from(reader)?)),
        ns2::synth::FORMAT => E::Synth(Synth::Stage2(ns2::synth::read_from(reader)?)),
        ns2::settings::FORMAT => E::Settings(Settings::Stage2(ns2::settings::read_from(reader)?)),
        ns3::program::FORMAT => E::Program(Program::Stage3(ns3::program::read_from(reader)?)),
        ns3::live::FORMAT => E::Live(Live::Stage3(ns3::live::read_from(reader)?)),
        ns3::song::FORMAT => E::Song(Song::Stage3(ns3::song::read_from(reader)?)),
        ns3::synth::FORMAT => E::Synth(Synth::Stage3(ns3::synth::read_from(reader)?)),
        ns3::settings::FORMAT => E::Settings(Settings::Stage3(ns3::settings::read_from(reader)?)),
        ns4::program::FORMAT => E::Program(Program::Stage4(ns4::program::read_from(reader)?)),
        ns4::live::FORMAT => E::Live(Live::Stage4(ns4::live::read_from(reader)?)),
        ns4::synth::FORMAT => E::Synth(Synth::Stage4(ns4::synth::read_from(reader)?)),
        ns4::piano_preset::FORMAT => {
            E::PianoPreset(PianoPreset::Stage4(ns4::piano_preset::read_from(reader)?))
        }
        ns4::organ_preset::FORMAT => {
            E::OrganPreset(OrganPreset::Stage4(ns4::organ_preset::read_from(reader)?))
        }
        ns4::settings::FORMAT => E::Settings(Settings::Stage4(ns4::settings::read_from(reader)?)),

        // Piano and Grand.
        np::program::FORMAT => E::Program(Program::Piano1(np::program::read_from(reader)?)),
        np::live::FORMAT => E::Live(Live::Piano1(np::live::read_from(reader)?)),
        np::settings::FORMAT => E::Settings(Settings::Piano1(np::settings::read_from(reader)?)),
        np2::program::FORMAT => E::Program(Program::Piano2(np2::program::read_from(reader)?)),
        np2::live::FORMAT => E::Live(Live::Piano2(np2::live::read_from(reader)?)),
        np2::settings::FORMAT => E::Settings(Settings::Piano2(np2::settings::read_from(reader)?)),
        np3::program::FORMAT => E::Program(Program::Piano3(np3::program::read_from(reader)?)),
        np3::live::FORMAT => E::Live(Live::Piano3(np3::live::read_from(reader)?)),
        np3::settings::FORMAT => E::Settings(Settings::Piano3(np3::settings::read_from(reader)?)),
        np4::program::FORMAT => E::Program(Program::Piano4(np4::program::read_from(reader)?)),
        np4::live::FORMAT => E::Live(Live::Piano4(np4::live::read_from(reader)?)),
        np4::settings::FORMAT => E::Settings(Settings::Piano4(np4::settings::read_from(reader)?)),
        np5::program::FORMAT => E::Program(Program::Piano5(np5::program::read_from(reader)?)),
        np5::live::FORMAT => E::Live(Live::Piano5(np5::live::read_from(reader)?)),
        np5::settings::FORMAT => E::Settings(Settings::Piano5(np5::settings::read_from(reader)?)),
        ng2::program::FORMAT => E::Program(Program::Grand(ng2::program::read_from(reader)?)),
        ng2::live::FORMAT => E::Live(Live::Grand(ng2::live::read_from(reader)?)),
        ng2::settings::FORMAT => E::Settings(Settings::Grand(ng2::settings::read_from(reader)?)),

        // Wave.
        nw::program::FORMAT => E::Program(Program::Wave(nw::program::read_from(reader)?)),
        nw::settings::FORMAT => E::Settings(Settings::Wave(nw::settings::read_from(reader)?)),
        nw2::program::FORMAT => E::Program(Program::Wave2(nw2::program::read_from(reader)?)),
        nw2::live::FORMAT => E::Live(Live::Wave2(nw2::live::read_from(reader)?)),
        nw2::settings::FORMAT => E::Settings(Settings::Wave2(nw2::settings::read_from(reader)?)),

        // Organs.
        nc2::program::FORMAT => E::Program(Program::C2(nc2::program::read_from(reader)?)),
        nc2::settings::FORMAT => E::Settings(Settings::C2(nc2::settings::read_from(reader)?)),
        nc2d::program::FORMAT => E::Program(Program::C2D(nc2d::program::read_from(reader)?)),
        nc2d::settings::FORMAT => E::Settings(Settings::C2D(nc2d::settings::read_from(reader)?)),
        no3::program::FORMAT => E::Program(Program::Organ3(no3::program::read_from(reader)?)),
        no3::settings::FORMAT => E::Settings(Settings::Organ3(no3::settings::read_from(reader)?)),

        // Leads (the CBIN generation; the older Leads ship SysEx).
        nl4::program::FORMAT => E::Program(Program::Lead4(nl4::program::read_from(reader)?)),
        nl4::performance::FORMAT => {
            E::Performance(Performance::Lead4(nl4::performance::read_from(reader)?))
        }
        nl4::settings::FORMAT => E::Settings(Settings::Lead4(nl4::settings::read_from(reader)?)),
        nla1::program::FORMAT => E::Program(Program::LeadA1(nla1::program::read_from(reader)?)),
        nla1::performance::FORMAT => {
            E::Performance(Performance::LeadA1(nla1::performance::read_from(reader)?))
        }
        nla1::settings::FORMAT => E::Settings(Settings::LeadA1(nla1::settings::read_from(reader)?)),

        // Drums.
        nd2::program::FORMAT => E::Program(Program::Drum2(nd2::program::read_from(reader)?)),
        nd3::kit::FORMAT => E::Program(Program::Drum3(nd3::kit::read_from(reader)?)),

        e => return Err(ParseError::UnknownFormat(e.to_string()).into()),
    })
}

/// One ZIP file: an Electro 5 bundle or backup (it carries a `meta.xml`
/// manifest), or a Drum bank (members are all one CBIN format).
#[cfg(feature = "bundle")]
fn read_zip(reader: &mut (impl Read + Seek)) -> Result<Entity, Error> {
    let kind = {
        let zip = zip::ZipArchive::new(&mut *reader)?;
        let names: Vec<&str> = zip.file_names().collect();
        // ⚠️ meta.xml alone does not say Electro 5: the whole family's backups
        // carry one (verified against real Piano/Organ 3/C2D/Lead 4 factory
        // restores). Only .ne5* members make it an Electro 5 bundle.
        if names.iter().any(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e.to_string_lossy().starts_with("ne5"))
        }) {
            "bundle"
        } else if names.iter().all(|n| n.ends_with(".nd2p")) {
            "nd2"
        } else if names.iter().all(|n| n.ends_with(".nd3k")) {
            "nd3"
        } else {
            // Anything else — a bundle only if every member is a CBIN file,
            // which `zip_raw_members` decides below.
            "members"
        }
    };
    reader.seek(std::io::SeekFrom::Start(0))?;

    Ok(Entity::Bundle(match kind {
        "nd2" => Bundle::Drum2Bank(nd2::bank::read_from(reader)?),
        "nd3" => Bundle::Drum3KitBank(nd3::kit_bank::read_from(reader)?),
        "members" => Bundle::Members(formats::zip_raw_members(reader)?),
        _ => Bundle::Electro5(ne5::Bundle::read_from(reader)?),
    }))
}

/// [`from_stream`] over a buffered read of the file at `path`.
pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Entity, Error> {
    from_stream(&mut BufReader::new(File::open(path)?))
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

    fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in members {
            zip.start_file(name.to_string(), stored).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// A ZIP of mixed CBIN members — the reported family bundle shape — reads
    /// as [`Bundle::Members`] with paths preserved.
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

    /// A ZIP holding anything that is not a CBIN file is not a bundle.
    #[test]
    fn a_zip_with_a_non_cbin_member_is_refused() {
        let a = member("ns3f");
        let bytes = archive(&[("One.ns3f", &a), ("readme.txt", b"hello")]);
        assert!(from_stream(&mut Cursor::new(bytes)).is_err());
    }
}

/// Serialize an [`Entity`] back to the bytes of its file — the counterpart to
/// [`from_stream`].
///
/// For every format this crate reads, `to_bytes(from_stream(x)) == x` byte-for-byte,
/// whichever header generation `x` carries. That is the crate's central invariant —
/// decoded values are read-only views over a verbatim body, so a re-emit cannot
/// drift — and `nord verify` exists to check it against real specimens. Fixed-length
/// formats declare their body length on their [`cbin::Body`] impl, and the container
/// refuses to emit a wrong-sized file.
///
/// Bundles are unsupported: a bundle is a ZIP walk over other entities, not a
/// re-emittable structure.
pub fn to_bytes(entity: &Entity) -> Result<Vec<u8>, Error> {
    use std::io::Cursor;

    let mut out = Cursor::new(Vec::new());
    entity.write_to(&mut out)?;
    Ok(out.into_inner())
}

/// What an entity is: a human label and the format tag its file carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// `"Electro 6 program"` — model then role, as the summary prints it.
    pub kind: &'static str,
    /// The CBIN tag, or the carrier name (`zip`, `syx`, `mid`, `cn3`).
    pub format: &'static str,
}

impl Entity {
    /// The container of a stub-backed entity — every variant whose body is
    /// container-verified but undecoded. `None` for the decoded formats and the
    /// non-CBIN carriers.
    pub fn raw(&self) -> Option<&Cbin<RawBody>> {
        use {Live as L, OrganPreset as OP, Program as P, Settings as St, Synth as Sy};
        match self {
            Entity::Program(
                P::C2(f)
                | P::C2D(f)
                | P::Drum2(f)
                | P::Drum3(f)
                | P::Electro3(f)
                | P::Electro4(f)
                | P::Electro6(f)
                | P::Electro7(f)
                | P::Grand(f)
                | P::Lead4(f)
                | P::LeadA1(f)
                | P::Organ3(f)
                | P::Piano1(f)
                | P::Piano2(f)
                | P::Piano3(f)
                | P::Piano4(f)
                | P::Piano5(f)
                | P::StageClassic(f)
                | P::Wave(f)
                | P::Wave2(f),
            )
            | Entity::Live(
                L::Electro4(f)
                | L::Electro6(f)
                | L::Electro7(f)
                | L::Grand(f)
                | L::Piano1(f)
                | L::Piano2(f)
                | L::Piano3(f)
                | L::Piano4(f)
                | L::Piano5(f)
                | L::Wave2(f),
            )
            | Entity::Settings(
                St::C2(f)
                | St::C2D(f)
                | St::Electro4(f)
                | St::Electro6(f)
                | St::Electro7(f)
                | St::Grand(f)
                | St::Lead4(f)
                | St::LeadA1(f)
                | St::Organ3(f)
                | St::Piano1(f)
                | St::Piano2(f)
                | St::Piano3(f)
                | St::Piano4(f)
                | St::Piano5(f)
                | St::Stage2(f)
                | St::Stage3(f)
                | St::Stage4(f)
                | St::Wave(f)
                | St::Wave2(f),
            )
            | Entity::Song(Song::Stage3(f))
            | Entity::Synth(Sy::Stage2(f) | Sy::StageClassic(f))
            | Entity::Performance(Performance::Lead4(f) | Performance::LeadA1(f))
            | Entity::OrganPreset(OP::Electro3(f))
            | Entity::PianoLibrary(f)
            | Entity::PipeLibrary(f) => Some(f),
            _ => None,
        }
    }

    /// The entity's [`Identity`]: its human label and the tag its file carries.
    pub fn identity(&self) -> Identity {
        use {Live as L, Program as P, Settings as St};
        let id = |kind, format| Identity { kind, format };
        match self {
            Entity::Program(p) => match p {
                P::C2(_) => id("C2 program", nc2::program::FORMAT),
                P::C2D(_) => id("C2D program", nc2d::program::FORMAT),
                P::Drum2(_) => id("Drum 2 program", nd2::program::FORMAT),
                P::Drum3(_) => id("Drum 3P kit", nd3::kit::FORMAT),
                P::Electro3(_) => id("Electro 3 program", ne3::program::FORMAT),
                P::Electro4(_) => id("Electro 4 program", ne4::program::FORMAT),
                P::Electro5(_) => id("Electro 5 program", ne5::program::FORMAT),
                P::Electro6(_) => id("Electro 6 program", ne6::program::FORMAT),
                P::Electro7(_) => id("Electro 7 program", ne7::program::FORMAT),
                P::Grand(_) => id("Grand program", ng2::program::FORMAT),
                P::Lead4(_) => id("Lead 4 program", nl4::program::FORMAT),
                P::LeadA1(_) => id("Lead A1 program", nla1::program::FORMAT),
                P::Organ3(_) => id("no3 organ program", no3::program::FORMAT),
                P::Piano1(_) => id("Piano program", np::program::FORMAT),
                P::Piano2(_) => id("Piano 2 program", np2::program::FORMAT),
                P::Piano3(_) => id("Piano 3 program", np3::program::FORMAT),
                P::Piano4(_) => id("Piano 4 program", np4::program::FORMAT),
                P::Piano5(_) => id("Piano 5 program", np5::program::FORMAT),
                P::Stage2(_) => id("Stage 2 program", ns2::program::FORMAT),
                P::Stage3(_) => id("Stage 3 program", ns3::program::FORMAT),
                P::Stage4(_) => id("Stage 4 program", ns4::program::FORMAT),
                P::StageClassic(_) => id("Stage Classic program", nsclassic::program::FORMAT),
                P::Wave(_) => id("Wave program", nw::program::FORMAT),
                P::Wave2(_) => id("Wave 2 program", nw2::program::FORMAT),
            },
            Entity::Live(l) => match l {
                L::Electro4(_) => id("Electro 4 live slot", ne4::live::FORMAT),
                L::Electro5(_) => id("Electro 5 live slot", ne5::live::FORMAT),
                L::Electro6(_) => id("Electro 6 live slot", ne6::live::FORMAT),
                L::Electro7(_) => id("Electro 7 live slot", ne7::live::FORMAT),
                L::Grand(_) => id("Grand live slot", ng2::live::FORMAT),
                L::Piano1(_) => id("Piano live slot", np::live::FORMAT),
                L::Piano2(_) => id("Piano 2 live slot", np2::live::FORMAT),
                L::Piano3(_) => id("Piano 3 live slot", np3::live::FORMAT),
                L::Piano4(_) => id("Piano 4 live slot", np4::live::FORMAT),
                L::Piano5(_) => id("Piano 5 live slot", np5::live::FORMAT),
                L::Stage2(_) => id("Stage 2 live slot", ns2::live::FORMAT),
                L::Stage3(_) => id("Stage 3 live slot", ns3::live::FORMAT),
                L::Stage4(_) => id("Stage 4 live slot", ns4::live::FORMAT),
                L::Wave2(_) => id("Wave 2 live slot", nw2::live::FORMAT),
            },
            Entity::Settings(s) => match s {
                St::C2(_) => id("C2 settings", nc2::settings::FORMAT),
                St::C2D(_) => id("C2D settings", nc2d::settings::FORMAT),
                St::Electro4(_) => id("Electro 4 settings", ne4::settings::FORMAT),
                St::Electro5(_) => id("Electro 5 settings", ne5::settings::FORMAT),
                St::Electro6(_) => id("Electro 6 settings", ne6::settings::FORMAT),
                St::Electro7(_) => id("Electro 7 settings", ne7::settings::FORMAT),
                St::Grand(_) => id("Grand settings", ng2::settings::FORMAT),
                St::Lead4(_) => id("Lead 4 settings", nl4::settings::FORMAT),
                St::LeadA1(_) => id("Lead A1 settings", nla1::settings::FORMAT),
                St::Organ3(_) => id("no3 organ settings", no3::settings::FORMAT),
                St::Piano1(_) => id("Piano settings", np::settings::FORMAT),
                St::Piano2(_) => id("Piano 2 settings", np2::settings::FORMAT),
                St::Piano3(_) => id("Piano 3 settings", np3::settings::FORMAT),
                St::Piano4(_) => id("Piano 4 settings", np4::settings::FORMAT),
                St::Piano5(_) => id("Piano 5 settings", np5::settings::FORMAT),
                St::Stage2(_) => id("Stage 2 settings", ns2::settings::FORMAT),
                St::Stage3(_) => id("Stage 3 settings", ns3::settings::FORMAT),
                St::Stage4(_) => id("Stage 4 settings", ns4::settings::FORMAT),
                St::Wave(_) => id("Wave settings", nw::settings::FORMAT),
                St::Wave2(_) => id("Wave 2 settings", nw2::settings::FORMAT),
            },
            Entity::Song(Song::Electro5(_)) => id("Electro 5 song / set", ne5::song::FORMAT),
            Entity::Song(Song::Stage3(_)) => id("Stage 3 song", ns3::song::FORMAT),
            Entity::Synth(Synth::Stage2(_)) => id("Stage 2 synth patch", ns2::synth::FORMAT),
            Entity::Synth(Synth::Stage3(_)) => id("Stage 3 synth patch", ns3::synth::FORMAT),
            Entity::Synth(Synth::Stage4(_)) => id("Stage 4 synth preset", ns4::synth::FORMAT),
            Entity::Synth(Synth::StageClassic(_)) => {
                id("Stage Classic synth patch", nsclassic::synth::FORMAT)
            }
            Entity::Performance(Performance::Lead4(_)) => {
                id("Lead 4 performance", nl4::performance::FORMAT)
            }
            Entity::Performance(Performance::LeadA1(_)) => {
                id("Lead A1 performance", nla1::performance::FORMAT)
            }
            Entity::OrganPreset(OrganPreset::Electro3(_)) => {
                id("Electro 3 organ preset", ne3::organ_preset::FORMAT)
            }
            Entity::OrganPreset(OrganPreset::Stage4(_)) => {
                id("Stage 4 organ preset", ns4::organ_preset::FORMAT)
            }
            Entity::PianoPreset(PianoPreset::Stage4(_)) => {
                id("Stage 4 piano preset", ns4::piano_preset::FORMAT)
            }
            Entity::Piano(_) => id("piano library", npno::FORMAT),
            Entity::PianoLibrary(_) => id(
                "Stage Classic piano library",
                nsclassic::piano_library::FORMAT,
            ),
            Entity::PipeLibrary(_) => id("C2 pipe library", npip::pipe_library::FORMAT),
            Entity::Sample(Sample::V2(_)) => id("sample instrument", nsmp::FORMAT),
            Entity::Sample(Sample::V3(_)) => id("sample instrument (nsmp3/nsmp4)", nsmp::FORMAT),
            Entity::Sysex(_) => id("SysEx dump", "syx"),
            Entity::Midi(_) => id("MIDI file", "mid"),
            Entity::Cne3(_) => id("Electro 2 library", "cn3"),
            #[cfg(feature = "bundle")]
            Entity::Bundle(_) => id("bundle", "zip"),
        }
    }

    /// Re-encode to `w`, byte-exact for anything read and unedited.
    ///
    /// Bundles are the one exception: the archive layer does not re-encode, so a
    /// bundle refuses rather than writing something almost like its source.
    pub fn write_to(&self, w: &mut (impl std::io::Write + Seek)) -> Result<(), Error> {
        match self {
            Entity::Cne3(f) => f.write_to(w),
            Entity::Live(l) => match l {
                Live::Electro4(f)
                | Live::Electro6(f)
                | Live::Electro7(f)
                | Live::Grand(f)
                | Live::Piano1(f)
                | Live::Piano2(f)
                | Live::Piano3(f)
                | Live::Piano4(f)
                | Live::Piano5(f)
                | Live::Wave2(f) => f.write_to(w),
                Live::Electro5(f) => f.write_to(w),
                Live::Stage4(f) => f.write_to(w),
                Live::Stage2(f) => f.write_to(w),
                Live::Stage3(f) => f.write_to(w),
            },
            Entity::Midi(f) => f.write_to(w),
            Entity::OrganPreset(OrganPreset::Electro3(f))
            | Entity::PianoLibrary(f)
            | Entity::PipeLibrary(f) => f.write_to(w),
            Entity::OrganPreset(OrganPreset::Stage4(f)) => f.write_to(w),
            Entity::PianoPreset(PianoPreset::Stage4(f)) => f.write_to(w),
            Entity::Piano(f) => f.write_to(w),
            Entity::Performance(Performance::Lead4(f))
            | Entity::Performance(Performance::LeadA1(f)) => f.write_to(w),
            Entity::Program(p) => match p {
                Program::C2(f)
                | Program::C2D(f)
                | Program::Drum2(f)
                | Program::Drum3(f)
                | Program::Electro3(f)
                | Program::Electro4(f)
                | Program::Electro6(f)
                | Program::Electro7(f)
                | Program::Grand(f)
                | Program::Lead4(f)
                | Program::LeadA1(f)
                | Program::Organ3(f)
                | Program::Piano1(f)
                | Program::Piano2(f)
                | Program::Piano3(f)
                | Program::Piano4(f)
                | Program::Piano5(f)
                | Program::StageClassic(f)
                | Program::Wave(f)
                | Program::Wave2(f) => f.write_to(w),
                Program::Electro5(f) => f.write_to(w),
                Program::Stage2(f) => f.write_to(w),
                Program::Stage3(f) => f.write_to(w),
                Program::Stage4(f) => f.write_to(w),
            },
            Entity::Sample(Sample::V2(f)) => f.write_to(w),
            Entity::Sample(Sample::V3(f)) => f.write_to(w),
            Entity::Settings(s) => match s {
                Settings::C2(f)
                | Settings::C2D(f)
                | Settings::Electro4(f)
                | Settings::Electro6(f)
                | Settings::Electro7(f)
                | Settings::Grand(f)
                | Settings::Lead4(f)
                | Settings::LeadA1(f)
                | Settings::Organ3(f)
                | Settings::Piano1(f)
                | Settings::Piano2(f)
                | Settings::Piano3(f)
                | Settings::Piano4(f)
                | Settings::Piano5(f)
                | Settings::Stage2(f)
                | Settings::Stage3(f)
                | Settings::Stage4(f)
                | Settings::Wave(f)
                | Settings::Wave2(f) => f.write_to(w),
                Settings::Electro5(f) => f.write_to(w),
            },
            Entity::Song(Song::Electro5(f)) => f.write_to(w),
            Entity::Song(Song::Stage3(f)) => f.write_to(w),
            Entity::Synth(Synth::Stage2(f)) | Entity::Synth(Synth::StageClassic(f)) => {
                f.write_to(w)
            }
            Entity::Synth(Synth::Stage3(f)) => f.write_to(w),
            Entity::Synth(Synth::Stage4(f)) => f.write_to(w),
            Entity::Sysex(f) => f.write_to(w),
            #[cfg(feature = "bundle")]
            Entity::Bundle(_) => Err(ParseError::AssertFail(
                "bundles are archives; re-encoding one is not supported".into(),
            )
            .into()),
        }
    }
}
