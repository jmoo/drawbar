//! One module per file format. A CBIN module is named for the four-character tag it
//! carries; where a model family shares a prefix across several tags, for that prefix;
//! and where the tags share no usable prefix (`nsclassic`, `np`), for the model. The
//! formats outside CBIN (`cn3`, `midi`, `sysex`, `nsmpproj`) are named for their file
//! type.
//!
//! # How far each format goes
//!
//! Writable formats round-trip byte-exactly (`to_bytes(from_stream(x)) == x`), and a
//! read verifies its container: the CBIN header and checksum for the CBIN formats, and
//! the envelope for the SysEx and MIDI carriers. Archives are read-only. What differs is
//! how much of the body decodes, in three tiers:
//!
//! - **Decoded.** The body is a bit-mapped struct of named fields. The struct's doc
//!   carries its byte map, a read gates on the schema versions the offsets are
//!   validated against and range-checks every field, and bits no field claims survive
//!   a re-encode unchanged. These are the Electro 5 program, live slot, song and
//!   settings ([`ne5`]); the Stage 2, 3 and 4 programs and live slots ([`ns2`],
//!   [`ns3`], [`ns4`]); the Stage 3 synth preset; and the Stage 4 synth, piano and
//!   organ presets.
//! - **Structurally decoded.** The body's framing decodes and is editable, and decoded
//!   audio is available on request. These are sample instruments ([`nsmp`]: section
//!   chain, zones, stroke metadata, and encoded audio) and piano libraries ([`npno`]:
//!   the CNSP prefix, the stroke directory, and audio spans, which transforms can drop
//!   and the writer lays out again). Sample Editor projects ([`nsmpproj`]) are text,
//!   read and written as a key-value tree with typed views.
//! - **Container-verified stubs.** Everything else. The body is kept as stored until
//!   it is mapped, and each stub module's doc records what is known of it.
//!
//! Provenance is marked where each fact is stated, in four phrases: "Confirmed on
//! hardware", "Inferred from specimens", "Reported by public documentation", and
//! "Unexplained". Broadly, the Electro 5 bodies are pinned by hardware sweeps that
//! change one setting at a time, and the Stage bodies come from community byte maps and
//! specimen measurement, not confirmed on hardware. Each module says which.

pub(crate) mod predictor;
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

/// Refuse a schema version the field offsets have not been validated against;
/// decoding it could produce plausible but wrong values.
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

/// One ZIP member, read under the length its directory entry declares.
///
/// ⚠️ A member's decompressed length is chosen by the archive's author and can far
/// exceed the archive's size. Memory grows with the bytes the entry yields, capped by
/// the declaration, so neither the declaration nor the entry alone sizes an
/// allocation. An entry that yields a length other than the one it declares is
/// refused.
#[cfg(feature = "bundle")]
pub(crate) fn zip_member_bytes(file: &mut zip::read::ZipFile<'_>) -> Result<Vec<u8>, Error> {
    use std::cmp::Ordering;
    use std::io::Read;

    let refuse = |value: String| -> Error {
        ParseError::OutOfBounds {
            value,
            bound: "the length its directory entry declares".into(),
        }
        .into()
    };

    let declared = file.size();
    let one_past_declared = declared
        .checked_add(1)
        .ok_or_else(|| refuse(format!("a member declaring {declared} bytes")))?;
    let mut bytes = Vec::new();
    file.take(one_past_declared).read_to_end(&mut bytes)?;

    let yielded = bytes.len() as u64;
    match yielded.cmp(&declared) {
        Ordering::Greater => Err(refuse(format!(
            "a member yielding more than the {declared} bytes it declares"
        ))),
        Ordering::Less => Err(refuse(format!(
            "a member yielding {yielded} of the {declared} bytes it declares"
        ))),
        Ordering::Equal => Ok(bytes),
    }
}

/// Every member of a ZIP archive, each parsed as a CBIN file of `format`.
///
/// For the drum banks, whose archives hold nothing else. A member that is not a
/// `format` file fails the read; it is not skipped.
#[cfg(feature = "bundle")]
pub(crate) fn zip_members(
    reader: &mut (impl std::io::Read + std::io::Seek),
    format: &'static str,
) -> Result<Vec<(String, crate::cbin::Cbin<crate::cbin::RawBody>)>, Error> {
    let mut zip = zip::ZipArchive::new(reader)?;
    let mut members = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let buffer = zip_member_bytes(&mut file)?;
        let member = crate::cbin::read(&mut std::io::Cursor::new(buffer), format)?;
        members.push((name, member));
    }
    Ok(members)
}

/// Every member of a ZIP archive as a container-verified CBIN file, tags mixed.
///
/// The shape every model's bundles and backups take: a plain ZIP of ordinary
/// program files, the member path encoding the slot. Reported by public
/// documentation; not confirmed on hardware. Members stay raw and are not dispatched
/// to their format modules. A member that is not a CBIN file fails the read, which is
/// how an unrecognized ZIP is judged not to be a bundle.
#[cfg(feature = "bundle")]
pub(crate) fn zip_raw_members(
    reader: &mut (impl std::io::Read + std::io::Seek),
) -> Result<Vec<(String, crate::cbin::Cbin<crate::cbin::RawBody>)>, Error> {
    let mut zip = zip::ZipArchive::new(reader)?;
    let mut members = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        // A backup manifest describes the archive; it is not a member entity.
        if file.is_dir() || file.name().ends_with("meta.xml") {
            continue;
        }
        let name = file.name().to_string();
        let buffer = zip_member_bytes(&mut file)?;
        let member = crate::cbin::read_raw(&mut std::io::Cursor::new(buffer))?;
        members.push((name, member));
    }
    Ok(members)
}

#[cfg(all(test, feature = "bundle"))]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const MEMBER: &[u8; 16] = b"0123456789abcdef";

    /// A one-member stored archive whose headers declare `declared` uncompressed
    /// bytes while the entry still holds all of [`MEMBER`]: `zip` caps a stored
    /// read at the compressed size, so patching only the uncompressed size
    /// leaves an entry that yields a length other than the one it declares.
    fn archive_declaring(declared: u32) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "member.ne5p",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(MEMBER).unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();

        let at = |signature: &[u8; 4]| {
            bytes
                .windows(4)
                .position(|window| window == signature)
                .expect("a ZIP signature")
        };
        let (local, central) = (at(b"PK\x03\x04"), at(b"PK\x01\x02"));
        bytes[local + 22..local + 26].copy_from_slice(&declared.to_le_bytes());
        bytes[central + 24..central + 28].copy_from_slice(&declared.to_le_bytes());
        bytes
    }

    fn member_bytes(archive: &[u8]) -> Result<Vec<u8>, Error> {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut file = zip.by_index(0).unwrap();
        zip_member_bytes(&mut file)
    }

    #[test]
    fn a_member_is_read_to_its_declared_length() {
        let bytes = member_bytes(&archive_declaring(MEMBER.len() as u32)).unwrap();
        assert_eq!(bytes, MEMBER);
    }

    #[test]
    fn a_member_yielding_more_than_it_declares_is_refused() {
        let archive = archive_declaring(MEMBER.len() as u32 - 1);
        let err = member_bytes(&archive).unwrap_err();
        assert!(
            err.to_string()
                .contains("more than the 15 bytes it declares"),
            "expected a refusal naming the declared length, got {err}"
        );
    }

    #[test]
    fn a_member_yielding_fewer_bytes_than_it_declares_is_refused() {
        let archive = archive_declaring(MEMBER.len() as u32 + 1);
        let err = member_bytes(&archive).unwrap_err();
        assert!(
            err.to_string().contains("16 of the 17 bytes it declares"),
            "expected a refusal naming both lengths, got {err}"
        );
    }

    #[test]
    fn a_member_declaring_more_than_the_archive_holds_is_refused() {
        let archive = archive_declaring(64 * 1024 * 1024);
        assert!(
            archive.len() < 1024,
            "a declaration of 64 MiB should sit in a tiny archive, which is {} bytes",
            archive.len()
        );
        let err = member_bytes(&archive).unwrap_err();
        assert!(
            err.to_string()
                .contains("16 of the 67108864 bytes it declares"),
            "expected a refusal naming the bytes the member yielded, got {err}"
        );
    }
}
