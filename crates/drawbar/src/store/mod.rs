//! "This computer", kept as a folder of real files.
//!
//! The library is a directory tree: a folder in the browser is a directory, an asset is a
//! file, and a move is a rename. What a file cannot say (an asset's id, its tags, the slot
//! it came off, an edit not yet saved) lives in a hidden `.drawbar/` folder at the root:
//!
//! ```text
//! <root>/
//!   .drawbar/
//!     library.ron     the index: ids, paths, fingerprints, tags, origins
//!     lock            held while a drawbar writes this library
//!     tmp/            writes in flight; emptied on open
//!     working/        <id>-<generation>: an unsaved edit's bytes, never rewritten
//!   Grand.npno
//!   Cello/c3.wav
//! ```
//!
//! `.drawbar/` is made at the first write, so any folder can be opened as a library and
//! stays as it was until something in it is changed.
//!
//! The app talks to a backend in [`Cmd`]s and hears back in [`Event`]s, in order, and
//! never waits for one: the desktop runs them on a thread, and a browser's storage
//! answers only asynchronously. [`Store`] is the app's side of that conversation: it
//! mirrors the [`crate::workspace::Workspace`] onto the files and folds what it hears back
//! into it.

mod diff;
mod exec;
mod mirror;
pub mod names;
mod sidecar;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{default_root, Backend};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{default_root, Backend};

pub use mirror::{Pass, Store};
pub use sidecar::{Row, Sidecar, Stored};

use serde::{Deserialize, Serialize};

/// Where an entry sits in a library: its names from the root down, joined by `/`. The
/// root itself is the empty path.
///
/// ⚠️ Every component is a real name: never empty, `.` or `..`, and never holding a `/`,
/// a `\` or a NUL. A path read from the index is checked on the way in, because the index
/// is a file anyone can edit, and a `..` in it would reach outside the library.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LibPath(String);

impl LibPath {
    pub fn root() -> LibPath {
        LibPath(String::new())
    }

    /// A path from its `/`-joined text, or `None` if a component is not a real name.
    pub fn parse(text: &str) -> Option<LibPath> {
        if text.is_empty() {
            return Some(LibPath::root());
        }
        text.split('/')
            .all(is_component)
            .then(|| LibPath(text.to_string()))
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The entry named `leaf` inside this folder.
    ///
    /// ⚠️ `leaf` must be a real name: a caller joins a name the rule in [`names`] allows,
    /// or one read from the disk.
    pub fn join(&self, leaf: &str) -> LibPath {
        debug_assert!(is_component(leaf), "{leaf:?}");
        match self.is_root() {
            true => LibPath(leaf.to_string()),
            false => LibPath(format!("{}/{leaf}", self.0)),
        }
    }

    /// The folder this sits in. The root's parent is the root.
    pub fn parent(&self) -> LibPath {
        match self.0.rsplit_once('/') {
            Some((parent, _)) => LibPath(parent.to_string()),
            None => LibPath::root(),
        }
    }

    /// The last name, or the empty string for the root.
    pub fn leaf(&self) -> &str {
        self.0
            .rsplit_once('/')
            .map_or(self.0.as_str(), |(_, leaf)| leaf)
    }

    /// How many folders down this is: 0 for an entry in the root.
    pub fn depth(&self) -> usize {
        self.0.matches('/').count()
    }

    /// Whether this is `dir` or somewhere inside it.
    pub fn is_in(&self, dir: &LibPath) -> bool {
        dir.is_root()
            || self.0 == dir.0
            || self
                .0
                .strip_prefix(&dir.0)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    /// Where this ends up when `from` is renamed to `to`: moved with it if it is `from` or
    /// inside it, and `None` otherwise.
    pub fn moved(&self, from: &LibPath, to: &LibPath) -> Option<LibPath> {
        if from.is_root() || !self.is_in(from) {
            return None;
        }
        let rest = &self.0[from.0.len()..];
        Some(LibPath(format!("{}{rest}", to.0)))
    }

    /// The names from the root down.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|part| !part.is_empty())
    }
}

fn is_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

impl TryFrom<String> for LibPath {
    type Error = String;

    fn try_from(text: String) -> Result<LibPath, String> {
        LibPath::parse(&text).ok_or_else(|| format!("{text:?} is not a path inside a library"))
    }
}

impl From<LibPath> for String {
    fn from(path: LibPath) -> String {
        path.0
    }
}

impl std::fmt::Display for LibPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a directory listing says about a file without opening it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Stat {
    pub len: u64,
    /// Nanoseconds since the Unix epoch, where the system reports a time at all.
    pub modified: Option<u64>,
}

/// Enough about a file's contents to tell whether it changed, and to recognize it at
/// another path.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Fingerprint {
    pub len: u64,
    pub modified: Option<u64>,
    /// CRC-32 over the whole file.
    pub crc: u32,
}

impl Fingerprint {
    pub fn of(stat: Stat, bytes: &[u8]) -> Fingerprint {
        Fingerprint {
            len: stat.len,
            modified: stat.modified,
            crc: nord_format::crc::crc32(bytes),
        }
    }

    pub fn stat(&self) -> Stat {
        Stat {
            len: self.len,
            modified: self.modified,
        }
    }

    /// Whether `bytes` are the contents this fingerprint was taken of, whenever they were
    /// written.
    pub fn holds(&self, bytes: &[u8]) -> bool {
        self.len == bytes.len() as u64 && self.crc == nord_format::crc::crc32(bytes)
    }

    /// Whether two fingerprints were taken of the same contents.
    pub fn same_contents(&self, other: &Fingerprint) -> bool {
        (self.len, self.crc) == (other.len, other.crc)
    }
}

/// One file a listing found.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: LibPath,
    pub stat: Stat,
    /// The contents, when the listing was asked to read them: always on open, and on a
    /// rescan for a file whose [`Stat`] is not the one the app knew.
    pub bytes: Option<Vec<u8>>,
}

/// The library's tree as the disk has it.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Every folder below the root, parents before children.
    pub dirs: Vec<LibPath>,
    /// Every file below the root, in path order. `.drawbar/` and names starting with a
    /// dot are left out.
    pub files: Vec<Found>,
    /// Files that could not be read, and why. They are left out of `files`.
    pub unread: Vec<(LibPath, String)>,
}

/// What opening a library found.
#[derive(Debug)]
pub struct Opened {
    /// `Err` with the reason when nothing may be written here. The library still opens,
    /// and every edit stays in memory.
    pub writable: Result<(), String>,
    /// `.drawbar/` was there already. Where it was not, nothing has been written, and
    /// the library's lock is taken at the first write.
    pub indexed: bool,
    /// The index, or an empty one for a library that has none or one this build must not
    /// read.
    pub sidecar: Sidecar,
    pub listing: Listing,
    /// The working copies the index names, by asset id. One that did not read is left
    /// out.
    pub working: std::collections::BTreeMap<u64, Vec<u8>>,
    /// How many leftovers of interrupted writes were removed.
    pub swept: usize,
}

/// Why a save did not land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The file is not what drawbar last read: something else wrote it, moved it, or put
    /// a file where a new one was to go.
    Moved,
    Io(String),
}

/// What the app asks a backend to do. Each runs after the one sent before it.
///
/// Every command that writes first makes `.drawbar/` where there is none and takes the
/// library's lock. Where it cannot, it runs no further and is answered by
/// [`Event::ReadOnly`].
#[derive(Debug)]
pub enum Cmd {
    /// Read the index, and list and read every file. Where `.drawbar/` exists, take the
    /// lock and sweep interrupted writes first; where it does not, write nothing. Answered
    /// by [`Event::Opened`].
    Open,
    /// List the tree again, reading every file whose [`Stat`] is not in `known` under its
    /// path. Answered by [`Event::Scanned`].
    Scan {
        known: std::collections::BTreeMap<LibPath, Stat>,
    },
    /// Write the `working` copies, then the index, then delete the working copies in
    /// `drop`. Working copies are named `<id>-<generation>`. Answered only on failure.
    Commit {
        sidecar: Sidecar,
        working: Vec<(String, Vec<u8>)>,
        drop: Vec<String>,
    },
    /// Write a file whole: a new one when `expect` is `None`, otherwise over a file that
    /// must still hold what `expect` says. Answered by [`Event::Saved`].
    Save {
        id: u64,
        path: LibPath,
        bytes: Vec<u8>,
        expect: Option<Fingerprint>,
    },
    /// Rename a file or folder. Refused where `to` already exists. Answered only on
    /// failure.
    Move { from: LibPath, to: LibPath },
    /// Answered only on failure.
    MakeDir(LibPath),
    /// Delete a file that must still hold what `expect` says. Answered only on failure.
    RemoveFile { path: LibPath, expect: Fingerprint },
    /// Delete an empty folder. Answered only on failure.
    RemoveDir(LibPath),
}

/// What a backend answers.
#[derive(Debug)]
pub enum Event {
    Opened(Result<Opened, String>),
    Scanned(Result<Listing, String>),
    Saved {
        id: u64,
        path: LibPath,
        result: Result<Fingerprint, Failure>,
    },
    /// A command other than a save failed: what it was doing, and why.
    Failed(String),
    /// A write found that nothing may be written after all, and why: another drawbar
    /// took the lock first, or the folder refused the sidecar. The write did not run.
    ReadOnly(String),
}

/// The keys of eframe's store that held the library before it was a folder. Nothing
/// reads them.
const LEFT_BEHIND: [&str; 3] = ["drawbar.this_computer", "drawbar.folders", "drawbar.tags"];

/// What the user is told, once, when a library kept the old way is left behind.
pub const STARTS_EMPTY: &str = "This computer starts empty in this version of drawbar. \
     What the previous version kept is not carried over.";

/// Whether eframe's store still holds a library kept the old way.
pub fn left_behind(storage: &dyn eframe::Storage) -> bool {
    LEFT_BEHIND
        .iter()
        .any(|key| storage.get_string(key).is_some_and(|held| !held.is_empty()))
}

/// Empty the keys that held a library kept the old way.
///
/// ⚠️ eframe's store has no remove, and on the desktop it writes back whatever it holds
/// in memory, so a key is emptied rather than left alone.
pub fn leave_behind(storage: &mut dyn eframe::Storage) {
    for key in LEFT_BEHIND {
        if storage.get_string(key).is_some_and(|held| !held.is_empty()) {
            storage.set_string(key, String::new());
        }
    }
}

#[cfg(test)]
mod tests;
