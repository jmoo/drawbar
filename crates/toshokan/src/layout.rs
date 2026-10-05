//! Where toshokan keeps its files: everything lives under one hidden root directory
//! inside the library folder, and nothing outside it belongs to toshokan.
//!
//! ```text
//! <root>/writers/<writer>/log-<n>.jsonl         a log segment
//! <root>/writers/<writer>/snapshot-<hash>.json  a snapshot, named by its BLAKE3 hash
//! <root>/blobs/<hash>                           a blob, named by its BLAKE3 hash
//! <root>/journal/<writer>/                      a writer's journal of unfinished effects
//! <root>/tmp/<writer>/                          a writer's files before they are renamed into place
//! <root>/quarantine/<writer>/<hash>             a store file whose bytes were not its name
//! ```

use crate::error::{Error, Result};
use crate::fs::RelPath;
use crate::ids::{canonical_u64, WriterId};
use crate::value::BlobId;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    root: RelPath,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            root: RelPath::new(Self::DEFAULT_ROOT).expect("the default root is one component"),
        }
    }
}

impl Layout {
    pub const DEFAULT_ROOT: &'static str = ".toshokan";

    /// A layout rooted at `root`, which must not be the library folder itself.
    pub fn new(root: RelPath) -> Result<Self> {
        if root.is_root() {
            return Err(Error::InvalidPath {
                path: String::new(),
                reason: "toshokan's root must be a directory inside the library folder",
            });
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &RelPath {
        &self.root
    }

    /// Whether `path` is toshokan's own: the root or anything under it.
    pub fn owns(&self, path: &RelPath) -> bool {
        path.starts_with(&self.root)
    }

    /// Refuse toshokan's root, anything inside it, and any directory holding it,
    /// none of which is a library file.
    pub fn check_library_path(&self, path: &RelPath) -> Result<()> {
        match self.owns(path) || self.root.starts_with(path) {
            true => Err(Error::InvalidPath {
                path: path.as_str().to_owned(),
                reason: "toshokan's own files are not library files",
            }),
            false => Ok(()),
        }
    }

    pub fn writers(&self) -> RelPath {
        child(&self.root, "writers")
    }

    pub fn writer(&self, writer: WriterId) -> RelPath {
        child(&self.writers(), &writer.to_string())
    }

    pub fn segment(&self, writer: WriterId, number: u64) -> RelPath {
        child(&self.writer(writer), &LogFile::Segment(number).name())
    }

    pub fn snapshot(&self, writer: WriterId, hash: BlobId) -> RelPath {
        child(&self.writer(writer), &LogFile::Snapshot(hash).name())
    }

    pub fn blobs(&self) -> RelPath {
        child(&self.root, "blobs")
    }

    pub fn blob(&self, blob: BlobId) -> RelPath {
        child(&self.blobs(), &blob.to_string())
    }

    pub fn journal(&self, writer: WriterId) -> RelPath {
        child(&child(&self.root, "journal"), &writer.to_string())
    }

    pub fn tmp(&self, writer: WriterId) -> RelPath {
        child(&child(&self.root, "tmp"), &writer.to_string())
    }

    pub fn quarantine(&self, writer: WriterId) -> RelPath {
        child(&child(&self.root, "quarantine"), &writer.to_string())
    }
}

fn child(parent: &RelPath, name: &str) -> RelPath {
    parent
        .join(name)
        .expect("layout names are single components")
}

/// A file in a writer's log directory.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LogFile {
    /// `log-<n>.jsonl`, with `n` in decimal without leading zeros.
    Segment(u64),
    /// `snapshot-<hash>.json`, with the BLAKE3 hash of the file's own bytes.
    Snapshot(BlobId),
}

impl LogFile {
    /// `None` for a name that is neither, which readers leave alone.
    pub fn parse(name: &str) -> Option<Self> {
        if let Some(number) = name
            .strip_prefix("log-")
            .and_then(|rest| rest.strip_suffix(".jsonl"))
        {
            return canonical_u64(number).map(Self::Segment);
        }
        name.strip_prefix("snapshot-")
            .and_then(|rest| rest.strip_suffix(".json"))
            .and_then(|hash| hash.parse().ok())
            .map(Self::Snapshot)
    }

    pub fn name(&self) -> String {
        match self {
            Self::Segment(number) => format!("log-{number}.jsonl"),
            Self::Snapshot(hash) => format!("snapshot-{hash}.json"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_lives_under_the_root() {
        let layout = Layout::default();
        let writer = WriterId::from_u128(0xab);
        let blob = BlobId::of(b"");
        let expected = [
            (layout.writer(writer), format!(".toshokan/writers/{writer}")),
            (
                layout.segment(writer, 7),
                format!(".toshokan/writers/{writer}/log-7.jsonl"),
            ),
            (
                layout.snapshot(writer, blob),
                format!(".toshokan/writers/{writer}/snapshot-{blob}.json"),
            ),
            (layout.blob(blob), format!(".toshokan/blobs/{blob}")),
            (
                layout.journal(writer),
                format!(".toshokan/journal/{writer}"),
            ),
            (layout.tmp(writer), format!(".toshokan/tmp/{writer}")),
            (
                layout.quarantine(writer),
                format!(".toshokan/quarantine/{writer}"),
            ),
        ];
        for (path, text) in expected {
            assert_eq!(path.as_str(), text);
            assert!(layout.owns(&path), "{path}");
        }
        assert!(!layout.owns(&RelPath::new(".toshokanx/a").unwrap()));
    }

    #[test]
    fn the_library_folder_cannot_be_the_root() {
        assert!(Layout::new(RelPath::ROOT).is_err());
    }

    #[test]
    fn log_file_names_parse_only_in_their_canonical_form() {
        let blob = BlobId::of(b"");
        for file in [
            LogFile::Segment(0),
            LogFile::Segment(u64::MAX),
            LogFile::Snapshot(blob),
        ] {
            assert_eq!(LogFile::parse(&file.name()), Some(file));
        }
        for name in [
            "log-.jsonl",
            "log-01.jsonl",
            "log-+1.jsonl",
            "log-1.json",
            "log-18446744073709551616.jsonl",
            "snapshot-abc.json",
            &format!("snapshot-{}.json", blob.to_string().to_uppercase()),
            "journal",
        ] {
            assert_eq!(LogFile::parse(name), None, "{name}");
        }
    }
}
