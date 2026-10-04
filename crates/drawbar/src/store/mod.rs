//! "This computer", kept as a folder of real files.
//!
//! The library is a directory tree: a folder in the browser is a directory, an asset is a
//! file, and a move is a rename. What a file cannot say (its tags, the slot it came off,
//! an edit not yet saved) lives in a hidden `.drawbar/` folder at the root, and so does
//! the id of each asset with any of those. Every other file is known by its listing.
//!
//! ```text
//! <root>/
//!   .drawbar/
//!     library.ron     the index: a row for each asset with something a file cannot say
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

mod cache;
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
pub(crate) use web::{buffer, private_root, settle, Writer};
#[cfg(target_arch = "wasm32")]
pub use web::{default_root, permission, Backend, Picked, Root};

/// Where a library is: a folder on the desktop.
#[cfg(not(target_arch = "wasm32"))]
pub type Root = std::path::PathBuf;

/// A file outside the library, to be copied into it: a path on the desktop.
#[cfg(not(target_arch = "wasm32"))]
pub type Outside = std::path::PathBuf;

/// A file outside the library, to be copied into it: a file the browser handed the page.
#[cfg(target_arch = "wasm32")]
pub type Outside = web_sys::File;

/// The name a file outside the library goes by.
#[cfg(not(target_arch = "wasm32"))]
pub fn outside_name(from: &Outside) -> String {
    from.file_name().map_or_else(
        || from.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The name a file outside the library goes by.
#[cfg(target_arch = "wasm32")]
pub fn outside_name(from: &Outside) -> String {
    from.name()
}

/// How many bytes a file outside the library holds, where that can be told.
#[cfg(not(target_arch = "wasm32"))]
pub fn outside_len(from: &Outside) -> Option<u64> {
    std::fs::metadata(from).ok().map(|meta| meta.len())
}

/// How many bytes a file outside the library holds.
#[cfg(target_arch = "wasm32")]
pub fn outside_len(from: &Outside) -> Option<u64> {
    Some(from.size() as u64)
}

#[cfg(target_arch = "wasm32")]
pub use cache::keep_libraries;
pub use cache::Cache;
pub(crate) use exec::TMP;
pub use exec::{opens, MOST_BYTES, MOST_ENTRIES};
pub use mirror::{Pass, Store};
pub use sidecar::{Keeps, Row, Sidecar, Stored, Working};

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ondisk::OnDisk;
use crate::rewrite::Rewrite;

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

/// Enough about a file to tell whether it changed, and to recognize it at another path.
///
/// Its [`Stat`] is trusted: a file whose length and time are the ones taken holds what it
/// held. Where they moved, the CRC decides, and a fingerprint without one says only that
/// the file is not known to be the same.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Fingerprint {
    pub len: u64,
    pub modified: Option<u64>,
    /// CRC-32 over the whole file, once something has read all of it.
    pub crc: Option<u32>,
}

impl Fingerprint {
    pub fn of(stat: Stat, bytes: &[u8]) -> Fingerprint {
        Fingerprint {
            crc: Some(nord_format::crc::crc32(bytes)),
            ..Fingerprint::unread(stat)
        }
    }

    /// A file's fingerprint before its contents are read.
    pub fn unread(stat: Stat) -> Fingerprint {
        Fingerprint {
            len: stat.len,
            modified: stat.modified,
            crc: None,
        }
    }

    pub fn stat(&self) -> Stat {
        Stat {
            len: self.len,
            modified: self.modified,
        }
    }

    /// Whether `bytes` are the contents this fingerprint was taken of, whenever they were
    /// written. Never, for a fingerprint without a CRC.
    pub fn holds(&self, bytes: &[u8]) -> bool {
        self.len == bytes.len() as u64 && self.crc == Some(nord_format::crc::crc32(bytes))
    }

    /// The length and CRC that recognize these contents at another path.
    pub fn contents(&self) -> Option<(u64, u32)> {
        Some((self.len, self.crc?))
    }
}

/// How drawbar holds the contents of a file it knows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Holds {
    /// Read whole, into memory.
    Whole,
    /// Left in its file and read by range, as a piano or sample instrument is.
    Resting,
    /// Not read: drawbar knows its name, length and time.
    Unread,
}

/// One file a listing found.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: LibPath,
    pub stat: Stat,
    /// The contents, where they were read: a file asked for, a file drawbar holds whose
    /// [`Stat`] moved, and the file under a working copy at open.
    pub bytes: Option<Vec<u8>>,
    /// The file left on disk and indexed in place of `bytes`, where the backend reads a
    /// piano or sample instrument by range.
    pub file: Option<Arc<OnDisk>>,
    /// CRC-32 over the whole file, taken only where its contents decide something: its
    /// [`Stat`] moved from one whose CRC is known, or it may be a known file moved.
    pub crc: Option<u32>,
}

impl Found {
    /// A file as a listing first finds it: its name, length and time, and nothing read.
    pub fn unread(path: LibPath, stat: Stat) -> Found {
        Found {
            path,
            stat,
            bytes: None,
            file: None,
            crc: None,
        }
    }

    /// Whether the contents were read.
    pub fn read(&self) -> bool {
        self.bytes.is_some() || self.file.is_some()
    }

    /// How drawbar holds what was found.
    pub fn holds(&self) -> Holds {
        match (&self.bytes, &self.file) {
            (Some(_), _) => Holds::Whole,
            (None, Some(_)) => Holds::Resting,
            (None, None) => Holds::Unread,
        }
    }

    /// The file's fingerprint, with the CRC where the listing took one.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint {
            crc: self.crc,
            ..Fingerprint::unread(self.stat)
        }
    }
}

/// The library's tree as the disk has it, or one part of it. `.drawbar/` and names
/// starting with a dot are left out.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Every folder below the root, in path order.
    pub dirs: Vec<LibPath>,
    /// Every file drawbar holds or opens, in path order.
    pub files: Vec<Found>,
    /// Files drawbar would have held that were not read, and why, in path order. They
    /// are left out of `files`.
    pub unread: Vec<(LibPath, String)>,
    /// Files drawbar does not open, in path order, listed by name only.
    pub others: Vec<LibPath>,
    /// Folders whose contents were not all listed, because the library holds more
    /// entries than a listing looks at or the folder could not be read. The root among
    /// them means some of its own were left out.
    pub unwalked: Vec<LibPath>,
}

impl Listing {
    /// Take in another part of the same listing.
    pub fn extend(&mut self, part: Listing) {
        self.dirs.extend(part.dirs);
        self.files.extend(part.files);
        self.unread.extend(part.unread);
        self.others.extend(part.others);
        self.unwalked.extend(part.unwalked);
    }

    /// Put every list in path order.
    pub fn sort(&mut self) {
        self.dirs.sort();
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        self.unread.sort();
        self.others.sort();
        self.unwalked.sort();
    }
}

/// What opening a library found before listing it.
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
    /// The working copies the index names, by asset id. One that is not there is left
    /// out, and one that did not read leaves the library read-only.
    pub working: std::collections::BTreeMap<u64, Vec<u8>>,
    /// How many leftovers of interrupted writes were removed.
    pub swept: usize,
    /// Folders an interrupted rename left under the name it moved them through, that
    /// could not be put back because another entry has their name.
    pub stranded: Vec<LibPath>,
    /// The slots' former occupants in `.drawbar/tmp/`, by name, where the library may be
    /// written: see [`Rescue`].
    pub rescued: Vec<(String, Stat)>,
}

/// A slot's former occupant that a write to the instrument left on this computer when
/// neither the write nor putting the occupant back finished. It may be the only copy of
/// what the slot held, so nothing sweeps it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rescue {
    /// Its file's name: [`nord_usb::envelope::rescue_name`], numbered where taken.
    pub name: String,
    pub at: Left,
}

/// Where a [`Rescue`] was left.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Left {
    /// In the open library's `.drawbar/tmp/`, holding this.
    Library(Stat),
    /// At this path in drawbar's own data, where no library could take it.
    #[cfg(not(target_arch = "wasm32"))]
    Shelf(std::path::PathBuf),
}

impl Rescue {
    /// Whether the system's file manager can show it: only on the desktop.
    pub fn shows(&self) -> bool {
        cfg!(not(target_arch = "wasm32"))
    }

    /// Where it is in the library, for one left there.
    fn path(&self) -> Option<LibPath> {
        match self.at {
            Left::Library(_) => LibPath::parse(&format!("{TMP}/{}", self.name)),
            #[cfg(not(target_arch = "wasm32"))]
            Left::Shelf(_) => None,
        }
    }
}

/// The end of an open's listing.
#[derive(Debug)]
pub struct Complete {
    /// Files listed at paths the index does not name, each the length of a file it
    /// names that is not where it says, and so maybe that file moved. Each comes with its
    /// CRC, unread.
    pub strangers: Vec<Found>,
    /// Files of that kind that were gone by the time their CRC was to be taken.
    pub gone: Vec<LibPath>,
    /// How many temporary siblings of interrupted saves were removed.
    pub swept: usize,
    /// How many commands sent since the open had run when the listing ended.
    pub ran: u64,
}

/// Why a save did not land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The file is not what drawbar last read: something else wrote it, moved it, or put
    /// a file where a new one was to go.
    Moved,
    /// A read that would have taken what drawbar holds whole past the room it was given:
    /// the file's length.
    Room(u64),
    Io(String),
}

/// What a file copied into the library is a copy of, as the app asks for it.
#[derive(Clone, Debug)]
pub enum CopyOf {
    /// A file outside the library.
    Outside(Outside),
    /// The bytes `range` of a file outside the library: a member of a bundle.
    Part(Outside, std::ops::Range<u64>),
    /// The file the asset with this id rests in, byte for byte, from wherever it is when
    /// the copy is sent.
    Asset(u64),
    /// A piano or sample instrument resting in the library, with an edit written through
    /// it as it is copied.
    Edited(Arc<OnDisk>, Arc<Rewrite>),
}

/// What a [`Cmd::Import`] copies.
#[derive(Debug)]
pub enum Source {
    /// A file outside the library.
    Outside(Outside),
    /// The bytes `range` of a file outside the library.
    Part(Outside, std::ops::Range<u64>),
    /// A file of the library, byte for byte, which must still hold what its fingerprint
    /// says.
    Library(LibPath, Fingerprint),
    /// A piano or sample instrument resting in the library, with an edit written through
    /// it as it is copied.
    Edited(Arc<OnDisk>, Arc<Rewrite>),
}

/// What the app asks a backend to do. Each runs after the one sent before it.
///
/// Every command that writes first makes `.drawbar/` where there is none and takes the
/// library's lock. Where it cannot, it runs no further and is answered by
/// [`Event::ReadOnly`].
#[derive(Debug)]
pub enum Cmd {
    /// Read the index, and list every file by its name, length and time. Nothing is read
    /// but the file under each working copy the index names. Where `.drawbar/` exists,
    /// take the lock and sweep interrupted writes first; where it does not, write
    /// nothing. Answered by [`Event::Opened`], then the files the index names in
    /// [`Event::Listed`] parts, then the rest of the listing in parts, breadth first,
    /// then [`Event::Complete`]. Only [`Event::Opened`] answers an open that failed.
    ///
    /// Every command sent while the listing is in flight runs between two of its folders,
    /// in the order sent, and the rest of the listing follows what it moved, made or
    /// removed. A file a command wrote is not listed again.
    Open,
    /// List the tree again. A file `known` holds whole or resting is read again where its
    /// [`Stat`] is not the known one; no other file is read. Answered by
    /// [`Event::Scanned`].
    Scan {
        known: std::collections::BTreeMap<LibPath, (Fingerprint, Holds)>,
    },
    /// Look again at the files `known` names, as a rescan would, without listing the
    /// tree: a file whose [`Stat`] is not the known one is read. Answered by
    /// [`Event::Checked`].
    Check {
        known: std::collections::BTreeMap<LibPath, Fingerprint>,
    },
    /// List this folder and everything below it ahead of the rest of an open's listing.
    /// Answered by [`Event::Walked`] once its parts have been sent.
    Walk(LibPath),
    /// Read each of these files for the asset whose id comes with it, reading at most
    /// `room` bytes whole between them. A piano or sample instrument may be left resting
    /// in its file instead, which takes none of `room`. The fingerprint is what drawbar
    /// knew of the file. Answered by [`Event::Read`].
    Read {
        files: Vec<(u64, LibPath, Option<Fingerprint>)>,
        room: u64,
    },
    /// Take the CRC of each of these files, for the asset whose id comes with it, where
    /// its [`Stat`] is still the one its fingerprint gives. Answered by
    /// [`Event::Fingerprinted`].
    Fingerprint(Vec<(u64, LibPath, Fingerprint)>),
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
    /// Rename a file or folder. Refused where `to` already exists. Answered by
    /// [`Event::Moved`].
    Move { from: LibPath, to: LibPath },
    /// Write over the file at `path`, which must still hold what `expect` says, the file
    /// `edit` makes of `from`, the piano or sample instrument resting there, read by range.
    /// Nothing is read or written whole. Answered by [`Event::Rewritten`].
    Rewrite {
        id: u64,
        path: LibPath,
        from: Arc<OnDisk>,
        edit: Arc<Rewrite>,
        expect: Fingerprint,
    },
    /// Copy a file to `path`, as [`Cmd::Save`] writes one: a new file when `expect` is
    /// `None`, otherwise over a file that must still hold what `expect` says. Nothing is
    /// read whole: a piano or sample instrument is left resting in the copy, and any other
    /// file is left unread. Answered by [`Event::Imported`].
    Import {
        id: u64,
        path: LibPath,
        from: Source,
        expect: Option<Fingerprint>,
    },
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
    /// One part of an open's listing, gathered after the first `ran` commands sent since
    /// the open had run, and before the next.
    Listed {
        part: Listing,
        ran: u64,
    },
    Complete(Complete),
    Scanned(Result<Listing, String>),
    /// The files a [`Cmd::Check`] found where it looked.
    Checked(Result<Vec<Found>, String>),
    /// The folder a [`Cmd::Walk`] asked for, as it was after `ran` commands, is listed
    /// whole.
    Walked {
        dir: LibPath,
        ran: u64,
    },
    /// What [`Cmd::Read`] read, each with its asset's id. A file not where it was asked
    /// for answers [`Failure::Moved`].
    Read(Vec<(u64, Result<Found, Failure>)>),
    /// The files a [`Cmd::Fingerprint`] found as their fingerprints said, each with its
    /// asset's id and its fingerprint, now with its CRC. A file that moved, is gone or
    /// did not read is left out.
    Fingerprinted(Vec<(u64, LibPath, Fingerprint)>),
    Saved {
        id: u64,
        path: LibPath,
        result: Result<Fingerprint, Failure>,
    },
    /// What a [`Cmd::Rewrite`] wrote, as a listing finds it.
    Rewritten {
        id: u64,
        path: LibPath,
        result: Result<Found, Failure>,
    },
    /// What a [`Cmd::Import`] copied, as a listing finds it.
    Imported {
        id: u64,
        path: LibPath,
        result: Result<Found, Failure>,
    },
    /// Whether a [`Cmd::Move`] moved anything, and why not.
    Moved {
        from: LibPath,
        to: LibPath,
        result: Result<(), String>,
    },
    /// A command other than a save or a move failed: what it was doing, and why.
    Failed(String),
    /// A write found that nothing may be written after all, and why: another drawbar
    /// took the lock first, or the folder refused the sidecar. The write did not run.
    ReadOnly(String),
}

impl Event {
    /// The codes this answer's failures are counted under, each once. Never a path or a
    /// message, which name files.
    pub fn faults(&self) -> std::collections::BTreeSet<&'static str> {
        let one = |code| std::iter::once(code).collect();
        match self {
            Event::Opened(Err(_)) => one("open"),
            Event::Opened(Ok(opened)) if opened.writable.is_err() => one("read-only"),
            Event::ReadOnly(_) => one("read-only"),
            Event::Scanned(Err(_)) | Event::Checked(Err(_)) => one("rescan"),
            Event::Moved { result: Err(_), .. } => one("move"),
            Event::Failed(_) => one("write"),
            Event::Read(answers) => answers
                .iter()
                .filter_map(|(_, result)| result.as_ref().err())
                .map(|failure| READ.of(failure))
                .collect(),
            Event::Saved { result, .. } => SAVE.failed(result),
            Event::Imported { result, .. } => IMPORT.failed(result),
            Event::Rewritten { result, .. } => REWRITE.failed(result),
            Event::Opened(Ok(_))
            | Event::Listed { .. }
            | Event::Complete(_)
            | Event::Scanned(Ok(_))
            | Event::Checked(Ok(_))
            | Event::Walked { .. }
            | Event::Fingerprinted(_)
            | Event::Moved { result: Ok(()), .. } => Default::default(),
        }
    }
}

/// The codes a step on one file fails under.
struct Codes {
    changed: &'static str,
    room: &'static str,
    io: &'static str,
}

impl Codes {
    fn of(&self, failure: &Failure) -> &'static str {
        match failure {
            Failure::Moved => self.changed,
            Failure::Room(_) => self.room,
            Failure::Io(_) => self.io,
        }
    }

    fn failed<T>(&self, result: &Result<T, Failure>) -> std::collections::BTreeSet<&'static str> {
        result
            .as_ref()
            .err()
            .map(|failure| self.of(failure))
            .into_iter()
            .collect()
    }
}

const READ: Codes = Codes {
    changed: "read-changed",
    room: "read-room",
    io: "read-io",
};
const SAVE: Codes = Codes {
    changed: "save-changed",
    room: "save-room",
    io: "save-io",
};
const IMPORT: Codes = Codes {
    changed: "import-changed",
    room: "import-room",
    io: "import-io",
};
const REWRITE: Codes = Codes {
    changed: "rewrite-changed",
    room: "rewrite-room",
    io: "rewrite-io",
};

/// The keys of eframe's store that held the library before it was a folder. Nothing
/// reads them.
const LEFT_BEHIND: [&str; 3] = ["drawbar.this_computer", "drawbar.folders", "drawbar.tags"];

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

// The tests run the desktop's backend over folders on disk.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
