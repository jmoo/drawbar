//! Nord bundles, which drawbar holds only on the way in and out: an import unpacks one
//! into a folder of the library, and an export writes one from a selection.

use std::io;
use std::ops::Range;

use nord_format::bundle::archive::Directory;
#[cfg(target_arch = "wasm32")]
use nord_format::bundle::archive::{Tail, TAIL_MAX};
use nord_format::bundle::manifest;

use crate::store::Outside;

/// The extensions of the bundles Nord Sound Manager writes for an Electro 5.
const EXTENSIONS: [&str; 2] = ["ne5pbundle", "ne5tbundle"];

/// Whether a file of this name is a bundle to import.
pub fn is_bundle(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// One file a bundle holds: its archive path and where its bytes lie in the bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub path: String,
    pub bytes: Range<u64>,
}

impl Member {
    /// The member's own name, the last part of its archive path.
    pub fn leaf(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// The files a bundle holds, its manifest left out, read from its directory without
/// reading any member.
pub async fn members(from: &Outside) -> io::Result<Vec<Member>> {
    let directory = directory(from).await?;
    Ok(directory
        .members
        .into_iter()
        .filter(|member| member.entry.name != manifest::PATH)
        .map(|member| Member {
            path: member.entry.name,
            bytes: member.body,
        })
        .collect())
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
async fn directory(from: &Outside) -> io::Result<Directory> {
    let mut file = std::fs::File::open(from)?;
    Directory::read_from(&mut file).map_err(invalid)
}

#[cfg(target_arch = "wasm32")]
async fn directory(from: &Outside) -> io::Result<Directory> {
    use crate::ondisk::slice;
    let len = from.size() as u64;
    let tail = slice(from, len - len.min(TAIL_MAX)..len).await?;
    let tail = Tail::find(&tail, len).map_err(invalid)?;
    let bytes = slice(from, tail.directory.clone()).await?;
    let mut directory = tail.directory(&bytes).map_err(invalid)?;
    for member in &mut directory.members {
        let header = slice(from, member.header.clone()).await?;
        member.check_header(&header).map_err(invalid)?;
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_is_named_by_its_extension() {
        assert!(is_bundle("B3 Split.ne5tbundle"));
        assert!(is_bundle("foo.NE5PBUNDLE"));
        assert!(!is_bundle("foo.ne5p"));
        assert!(!is_bundle("ne5pbundle"));
    }
}
