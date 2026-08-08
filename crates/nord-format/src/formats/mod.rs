//! One module per file format, named for the four-character CBIN tag it carries — or,
//! where a model family shares a prefix across several tags, for that prefix; or,
//! where the tags share no usable prefix (`nsclassic`, `np`), for the model.
//!
//! Most modules are [`raw`] stubs: container read and verified, body kept verbatim.

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
