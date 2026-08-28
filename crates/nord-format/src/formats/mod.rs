//! One module per file format, named for the four-character CBIN tag it carries — or,
//! where a model family shares a prefix across several tags, for that prefix; or,
//! where the tags share no usable prefix (`nsclassic`, `np`), for the model.
//!
//! # How far each format goes
//!
//! Every format reads and writes byte-exactly — `to_bytes(from_stream(x)) == x` —
//! and a read verifies its container: CBIN header and checksum for the CBIN
//! formats, the envelope for the SysEx/MIDI carriers. What differs is how much of
//! the body decodes, in three tiers:
//!
//! - **Decoded** — the body is a bit-mapped struct of named fields. The struct's
//!   own doc carries its byte map, a read gates on the schema versions the
//!   offsets are validated against and range-checks every field, and bits no
//!   field claims survive a re-encode verbatim. These are the Electro 5 program,
//!   live slot, song and settings ([`ne5`]); the Stage 2, 3 and 4 programs and
//!   live slots ([`ns2`], [`ns3`], [`ns4`]); the Stage 3 synth preset; and the
//!   Stage 4 synth, piano and organ presets.
//! - **Structurally decoded** — the body's framing decodes and is editable, but
//!   the payloads stay verbatim: sample instruments ([`nsmp`] — section chain,
//!   zones and stroke metadata, never the audio) and piano libraries ([`npno`] —
//!   the CNSP prefix over a verbatim body).
//! - **Container-verified stubs** — everything else: body kept verbatim, waiting
//!   to be reverse-engineered. Each stub module's doc records what is known of it.
//!
//! Provenance is marked where each fact is stated, in three phrases: *confirmed
//! on hardware*, *inferred from specimens*, *unexplained*. Broadly, the Electro 5
//! bodies are pinned by change-one-setting hardware sweeps; the Stage bodies come
//! from community byte maps and corpus measurement, not confirmed on hardware —
//! each module says which.

pub(crate) mod raw;

pub mod cn3;
pub mod midi;
pub mod nc2;
pub mod nc2d;
pub mod nd2;
pub mod nd3;
pub mod ne3;
pub mod ne4;
pub mod ne5;
pub mod ne6;
pub mod ne7;
pub mod ng2;
pub mod nl4;
pub mod nla1;
pub mod no3;
pub mod np;
pub mod np2;
pub mod np3;
pub mod np4;
pub mod np5;
pub mod npip;
pub mod npno;
pub mod ns2;
pub mod ns3;
pub mod ns4;
pub mod nsclassic;
pub mod nsmp;
pub mod nsmpproj;
pub mod nw;
pub mod nw2;
pub mod sysex;

use crate::error::{Error, ParseError};

/// Refuse a schema version the build's field offsets have never been validated
/// against — decoding it would produce plausible-looking but wrong values.
pub(crate) fn known_version(
    format: &'static str,
    version: u32,
    supported: &'static [u32],
) -> Result<(), Error> {
    if supported.contains(&version) {
        Ok(())
    } else {
        Err(ParseError::UnsupportedVersion {
            format,
            version,
            supported,
        }
        .into())
    }
}

/// Every member of a ZIP archive, each parsed as a CBIN file of `format`.
///
/// For the drum banks, whose archives hold nothing else — a member that is not a
/// `format` file fails the read rather than being skipped.
#[cfg(feature = "bundle")]
pub(crate) fn zip_members(
    reader: &mut (impl std::io::Read + std::io::Seek),
    format: &'static str,
) -> Result<Vec<(String, crate::cbin::Cbin<crate::cbin::RawBody>)>, Error> {
    use std::io::Read;

    let mut zip = zip::ZipArchive::new(reader)?;
    let mut members = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        let member = crate::cbin::read(&mut std::io::Cursor::new(buffer), format)?;
        members.push((name, member));
    }
    Ok(members)
}

/// Every member of a ZIP archive as a container-verified CBIN file, tags mixed.
///
/// The shape independent interop projects report for every model's bundles and
/// backups: a plain ZIP of ordinary program files, the member path encoding the
/// slot. No such archive is in the corpus, so members stay raw rather than
/// dispatching to their format modules. A member that is not a CBIN file fails
/// the read — this is the arbiter of whether an unrecognised ZIP is a bundle.
#[cfg(feature = "bundle")]
pub(crate) fn zip_raw_members(
    reader: &mut (impl std::io::Read + std::io::Seek),
) -> Result<Vec<(String, crate::cbin::Cbin<crate::cbin::RawBody>)>, Error> {
    use std::io::Read;

    let mut zip = zip::ZipArchive::new(reader)?;
    let mut members = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        // A backup manifest describes the archive; it is not a member entity.
        if file.is_dir() || file.name().ends_with("meta.xml") {
            continue;
        }
        let name = file.name().to_string();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        let member = crate::cbin::read_raw(&mut std::io::Cursor::new(buffer))?;
        members.push((name, member));
    }
    Ok(members)
}
