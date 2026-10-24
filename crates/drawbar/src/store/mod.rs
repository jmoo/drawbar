//! "This computer", kept as a folder of real files.
//!
//! The library is a directory tree: a folder in the browser is a directory, an asset is a
//! file, and a move is a rename. What a file cannot say (its tags) is kept by toshokan in
//! a hidden `.drawbar/library/` folder at the root, as facts about the entities that say
//! it; every other file is known by its listing alone. Every running drawbar writes its
//! own log there, so several may write one library at once.
//!
//! ```text
//! <root>/
//!   .drawbar/
//!     library/        toshokan's root: each writer's log, pending effects and trash
//!   Grand.npno
//!   Cello/c3.wav
//! ```
//!
//! Opening writes nothing in the folder: `.drawbar/library/` is made at the first fact
//! said. An older build's `library.ron` beside it is neither read nor written.
//!
//! The app talks to a backend in [`Cmd`]s and hears back in [`Event`]s, in order, and
//! never waits for one: the desktop runs them on a thread, and a browser's storage
//! answers only asynchronously. Every change the user makes to the files or the facts
//! is an [`Intent`], one undo step, which toshokan checks against the files before it
//! writes them. Only reading a file and making or removing an empty folder are not.
//! [`Store`] is the app's side of that conversation: it turns what the user asked of
//! the [`crate::workspace::Workspace`] into intents, and projects the library's view
//! onto the workspace and the browser's tags.

mod cache;
mod counted;
mod diff;
#[cfg(any(target_arch = "wasm32", test))]
mod dom;
mod drafts;
mod exec;
mod ids;
mod keeper;
mod keys;
mod mirror;
pub mod names;
mod older;
pub mod shown;

#[cfg(not(target_arch = "wasm32"))]
mod native;
mod slots;
mod storage;
pub(crate) mod wire;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{default_root, local_root, openable, sync_dir, Backend};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
mod worker;
#[cfg(target_arch = "wasm32")]
pub use web::{default_root, permission, Backend, Picked, Root};
#[cfg(target_arch = "wasm32")]
pub(crate) use web::{remove_private, settle};
#[cfg(target_arch = "wasm32")]
pub use worker::{Unrun, Worker};

/// The folder of this install's own state for a library that holds the slots' former
/// occupants a write to the instrument leaves while the library is open.
pub const RESCUED: &str = "rescued";

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

/// A file outside the library, read by range.
#[cfg(not(target_arch = "wasm32"))]
pub fn outside_source(from: &Outside) -> std::io::Result<impl nord_usb::FileSource> {
    nord_usb::envelope::Positional::new(std::fs::File::open(from)?)
}

/// A file outside the library, read by range.
#[cfg(target_arch = "wasm32")]
pub fn outside_source(from: &Outside) -> std::io::Result<impl nord_usb::FileSource> {
    Ok(crate::ondisk::Sliced::new(from.clone()))
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
pub use exec::{opens, MOST_BYTES};
pub use ids::Key;
pub use keys::identity_of_bytes;
pub use mirror::{Pass, Store};

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use toshokan::EntityId;

use crate::ondisk::OnDisk;
use crate::rewrite::Pieces;

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

/// Enough about a file to tell whether it changed.
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
    /// The file's identity as the library takes it, once drawbar has read or written it.
    pub identity: Option<toshokan::Identity>,
}

impl Fingerprint {
    /// What `bytes`, written at `path` and found there with `stat`, are known by.
    pub fn of(path: &LibPath, stat: Stat, bytes: &[u8]) -> Fingerprint {
        Fingerprint {
            crc: Some(nord_format::crc::crc32(bytes)),
            identity: keys::identity_of_bytes(path, bytes),
            ..Fingerprint::unread(stat)
        }
    }

    /// A file's fingerprint before its contents are read.
    pub fn unread(stat: Stat) -> Fingerprint {
        Fingerprint {
            len: stat.len,
            modified: stat.modified,
            crc: None,
            identity: None,
        }
    }

    /// What a change to the file expects to find: the file drawbar last read or wrote,
    /// by its identity, or else by the length and time a listing found.
    pub fn expect(&self) -> toshokan::Expect {
        match self.identity {
            Some(identity) => toshokan::Expect::Holds(identity),
            None => toshokan::Expect::Listed {
                len: self.len,
                modified: self.modified,
            },
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Found {
    pub path: LibPath,
    pub stat: Stat,
    /// The contents, where they were read: a file asked for, a file drawbar holds whose
    /// [`Stat`] moved, and the file under a working copy at open. Never sent: only the
    /// backend that reads files holds them.
    #[serde(skip)]
    pub bytes: Option<Vec<u8>>,
    /// The file left on disk and indexed in place of `bytes`, where the backend reads a
    /// piano or sample instrument by range. Never sent, as `bytes`.
    #[serde(skip)]
    pub file: Option<Arc<OnDisk>>,
    /// CRC-32 over the whole file, taken only where its contents decide something: its
    /// [`Stat`] moved from one whose CRC is known, or it may be a known file moved.
    pub crc: Option<u32>,
    /// The file's identity, where its contents were read.
    pub identity: Option<toshokan::Identity>,
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
            identity: None,
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
            identity: self.identity,
            ..Fingerprint::unread(self.stat)
        }
    }
}

/// A slot's former occupant that a write to the instrument left on this computer when
/// neither the write nor putting the occupant back finished. It may be the only copy of
/// what the slot held, so nothing sweeps it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rescue {
    /// Its file's name: [`nord_usb::envelope::rescue_name`], numbered where taken.
    pub name: String,
    /// The slot it came off, as its name says.
    pub slot: Option<nord_usb::Location>,
    /// Where it is, outside every library: on the desktop, a path; in the browser, its
    /// path in the browser's storage for drawbar.
    pub at: std::path::PathBuf,
    /// The file, as the browser gives it.
    #[cfg(target_arch = "wasm32")]
    pub file: web_sys::File,
    /// Why it does not read as one whole Nord file, where it does not. It is then no copy
    /// of what the slot held, and is offered only to be discarded.
    pub broken: Option<String>,
}

/// Why `file` does not read as one whole Nord file, where it does not.
async fn unwhole(file: std::io::Result<impl nord_usb::FileSource>) -> Option<String> {
    let verified = match file {
        Ok(mut file) => nord_usb::envelope::verify(&mut file).await.map(drop),
        Err(e) => Err(e.into()),
    };
    verified.err().map(|e| e.to_string())
}

impl Rescue {
    /// The slot a rescue named `name` came off: [`nord_usb::envelope::rescue_name`] numbers
    /// its bank and slot from one.
    pub fn slot_of(name: &str) -> Option<nord_usb::Location> {
        let rest = name.strip_prefix(nord_usb::envelope::RESCUED)?;
        let (bank, rest) = rest.split_once('-')?;
        let slot: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let (bank, slot) = (bank.parse().ok()?, slot.parse().ok()?);
        (bank >= 1 && slot >= 1).then(|| nord_usb::Location::from_user(bank, slot))
    }

    /// Whether the system's file manager can show it: only on the desktop.
    pub fn shows(&self) -> bool {
        cfg!(not(target_arch = "wasm32"))
    }
}

/// Why a save did not land.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    /// The file is not what drawbar last read: something else wrote it, moved it, or put
    /// a file where a new one was to go.
    Moved,
    /// A read that would have taken what drawbar holds whole past the room it was given:
    /// the file's length.
    Room(u64),
    Io(String),
    /// The answer that would say whether it was done was lost: a look at the folder
    /// finds out.
    Unheard,
}

/// Some bytes of a file outside the library, and the CRC-32 they must have: a member of
/// a bundle. A copy whose bytes do not match fails.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Part {
    #[serde(with = "wire::outside")]
    pub from: Outside,
    pub bytes: std::ops::Range<u64>,
    pub crc32: u32,
}

impl Part {
    /// The error for bytes that are short or do not match.
    pub fn mismatch(&self) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the bundle's copy of it is damaged",
        )
    }
}

/// What a file copied into the library is a copy of, as the app asks for it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CopyOf {
    /// A file outside the library.
    Outside(#[serde(with = "wire::outside")] Outside),
    /// A member of a bundle outside the library.
    Part(Part),
    /// The file the asset with this id rests in, byte for byte, from wherever it is when
    /// the copy is sent.
    Asset(u64),
    /// The piano or sample instrument the asset with this id rests in, from wherever it
    /// is when the copy is sent, as these pieces of it lay out an edit of it.
    Edited(u64, Arc<Pieces>),
}

/// What an [`Intent::Import`] copies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Source {
    /// A file outside the library.
    Outside(#[serde(with = "wire::outside")] Outside),
    /// A member of a bundle outside the library.
    Part(Part),
    /// A file of the library, byte for byte, which must still hold what its fingerprint
    /// says.
    Library(LibPath, Fingerprint),
    /// A piano or sample instrument of the library, which must still hold what its
    /// fingerprint says, as these pieces of it lay out an edit of it.
    Edited(LibPath, Fingerprint, Arc<Pieces>),
}

/// A draft as this install keeps it: of the file `of` names, by this instance's writer,
/// or by the idle writer of the install's pool that kept it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Kept {
    pub of: Key,
    pub pooled: Option<toshokan::WriterId>,
}

impl Kept {
    /// A draft this instance's writer keeps.
    pub fn own(of: Key) -> Kept {
        Kept { of, pooled: None }
    }
}

/// A slot of one instrument, which an unsaved edit of its view is kept by: the class's
/// code and the bank and slot from zero, as the wire numbers them.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Slot {
    /// What tells the instrument from others: its serial number, or its product name
    /// where it reports none.
    pub instrument: String,
    pub class: u32,
    pub bank: u32,
    pub slot: u32,
}

/// What the app asks a backend to do. Each runs after the one sent before it.
#[derive(Debug, Serialize, Deserialize)]
pub enum Cmd {
    /// Open the library as a writer of its own, from the view it kept. Answered by
    /// [`Event::Opened`], which lists no file: a [`Cmd::Rescan`] lists them.
    Open,
    /// Read what other writers logged since, and scan every file for what changed
    /// outside. Answered by [`Event::Scanned`].
    Rescan,
    /// Read what other writers logged since, scanning only the files their entries
    /// moved. Answered by [`Event::Scanned`].
    Refresh,
    /// Answered by [`Event::Committed`], after the answer of a change to a file.
    Intent(Intent),
    /// Undo this session's last intent not undone. Answered by [`Event::Committed`].
    Undo,
    /// Redo the intent undone last. Answered by [`Event::Committed`].
    Redo,
    /// Keep an unsaved edit of the file `of` names, made over the file of identity
    /// `base`, in this install's own state, never in the folder, and once it is kept,
    /// let go of the drafts it `replaces`. Answered by [`Event::Drafted`].
    Draft {
        of: Key,
        base: toshokan::Identity,
        #[serde(with = "wire::bytes")]
        bytes: Vec<u8>,
        replaces: Vec<Kept>,
    },
    /// Let go of a draft. Answered only on failure.
    Discard(Kept),
    /// Keep the unsaved edit of a slot's view, made over the slot's bytes of hash
    /// `base`, in this install's own state, never in the folder. Answered only on
    /// failure, by [`Event::Unkept`].
    KeepSlot {
        slot: Slot,
        base: blake3::Hash,
        bytes: Vec<u8>,
    },
    /// Let go of the edit kept of a slot's view. Answered only on failure.
    DiscardSlot(Slot),
    /// Read the edit kept of a slot's view. Answered by [`Event::SlotKept`].
    ReadSlot(Slot),
    /// Settle as `how` says a change another drawbar left unfinished. Answered by
    /// [`Event::Committed`].
    Settle {
        orphan: toshokan::report::Orphan,
        how: toshokan::log::Settlement,
    },
    /// Read each of these files for the asset whose id comes with it, reading at most
    /// `room` bytes whole between them. A piano or sample instrument may be left resting
    /// in its file instead, which takes none of `room`. The fingerprint is what drawbar
    /// knew of the file. Answered by [`Event::Read`].
    Read {
        files: Vec<(u64, LibPath, Option<Fingerprint>)>,
        room: u64,
    },
}

impl Cmd {
    /// Whether it writes the folder.
    pub fn writes(&self) -> bool {
        match self {
            Cmd::Open
            | Cmd::Rescan
            | Cmd::Refresh
            | Cmd::Draft { .. }
            | Cmd::Discard(_)
            | Cmd::KeepSlot { .. }
            | Cmd::DiscardSlot(_)
            | Cmd::ReadSlot(_)
            | Cmd::Read { .. } => false,
            Cmd::Intent(_) | Cmd::Undo | Cmd::Redo | Cmd::Settle { .. } => true,
        }
    }
}

/// A change the user asked for, committed as one undo step. Each names the entities and
/// files it changes as the view it was made from showed them. A change to a file names
/// it by its path: the backend finds the entity bound to it, if any, in the view it
/// commits against, and checks the file holds what `expect` says before it writes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Intent {
    /// Put a tag on each of these files, making an entity for each one without, and
    /// where `make` says, add the tag to the library's list of tags as
    /// [`Intent::MakeTag`] does.
    Tag {
        files: Vec<Key>,
        name: String,
        make: bool,
    },
    /// Take a tag off each of these entities.
    Untag {
        entities: Vec<EntityId>,
        name: String,
    },
    /// Add a tag to the library's list of tags.
    MakeTag { name: String },
    /// Rename a tag on each of these entities, which wear it, and in the list of tags.
    RenameTag {
        entities: Vec<EntityId>,
        from: String,
        to: String,
    },
    /// Take a tag off each of these entities, which wear it, and out of the list of tags.
    RemoveTag {
        entities: Vec<EntityId>,
        name: String,
    },
    /// End an entity whose file is gone.
    Forget { entity: EntityId },
    /// Take changes another branch of a writer's history made, which are kept apart
    /// from the library ([`KeptApart`]): each of `changes`, or every one it can take
    /// where none is named.
    Take {
        run: toshokan::log::Run,
        changes: Vec<KeptKey>,
    },
    /// Never show or merge the changes of `run` again, nor those after it.
    Dismiss { run: toshokan::log::Run },
    /// Write a file whole: a new one where `expect` is [`toshokan::Expect::Absent`],
    /// with an entity saying where it came from where `came` says. Answered by
    /// [`Event::Saved`].
    Save {
        id: u64,
        path: LibPath,
        #[serde(with = "wire::bytes")]
        bytes: Vec<u8>,
        expect: toshokan::Expect,
        came: Option<keys::Came>,
    },
    /// Write over the piano or sample instrument at `path` the edit `pieces` lay out of
    /// it, reading what it keeps of the file by range. Nothing is read or written whole.
    /// Answered by [`Event::Rewritten`].
    Rewrite {
        id: u64,
        path: LibPath,
        pieces: Arc<Pieces>,
        expect: toshokan::Expect,
    },
    /// Copy a file to `path`, as [`Intent::Save`] writes one. Nothing is read whole: a
    /// piano or sample instrument is left resting in the copy, and any other file is
    /// left unread. Answered by [`Event::Imported`].
    Import {
        id: u64,
        path: LibPath,
        from: Source,
        expect: toshokan::Expect,
        came: Option<keys::Came>,
    },
    /// Rename each of these files, all or none. Refused where a `to` already exists.
    /// Answered by an [`Event::Moved`] for each.
    Move { moves: Vec<Moving> },
    /// Rename a folder with everything in it. Refused where `to` already exists.
    /// Answered by [`Event::Moved`].
    MoveFolder { from: LibPath, to: LibPath },
    /// Move each of these files to this writer's trash, all or none, and end the entity
    /// bound to each.
    Delete {
        files: Vec<(LibPath, toshokan::Expect)>,
    },
    /// Make an empty folder where nothing is.
    MakeDir { path: LibPath },
    /// Remove a folder, empty but for the files the changes before it in its batch
    /// move out.
    RemoveDir { path: LibPath },
    /// The changes of one gesture, in order, as one change: each is answered as it
    /// would be alone, and those that did not land answer how they failed.
    Batch(Vec<Intent>),
}

/// The part of a batch that names the gesture: the folder it made or removed, around
/// which the rest moves, or else its first.
fn gesture(parts: &[Intent]) -> Option<&Intent> {
    let folder = |part: &&Intent| matches!(part, Intent::MakeDir { .. } | Intent::RemoveDir { .. });
    parts.iter().find(folder).or(parts.first())
}

/// One rename of an [`Intent::Move`]: the file at `from`, which holds what `expect`
/// says, to `to`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Moving {
    pub from: LibPath,
    pub to: LibPath,
    pub expect: toshokan::Expect,
}

impl Intent {
    /// What the Edit menu calls it: its label and what it names.
    pub fn describe(&self) -> String {
        let named = |path: &LibPath| format!("{} “{}”", self.label(), path.leaf());
        let tag = |name: &str| format!("{} “{name}”", self.label());
        let files = |paths: Vec<&LibPath>| match paths[..] {
            [path] => named(path),
            _ => format!("{} {} files", self.label(), paths.len()),
        };
        match self {
            Intent::Batch(parts) => {
                let label = self.label();
                let files: usize = parts.iter().map(Intent::files).sum();
                match parts.iter().all(|part| part.label() == label) && files > 1 {
                    true => format!("{label} {files} files"),
                    false => gesture(parts).map(Intent::describe).unwrap_or_default(),
                }
            }
            Intent::Move { moves } => files(moves.iter().map(|moving| &moving.from).collect()),
            Intent::Delete { files: deleted } => {
                files(deleted.iter().map(|(path, _)| path).collect())
            }
            Intent::Tag { name, .. }
            | Intent::Untag { name, .. }
            | Intent::MakeTag { name }
            | Intent::RemoveTag { name, .. }
            | Intent::RenameTag { from: name, .. } => tag(name),
            Intent::Save { path, .. }
            | Intent::Rewrite { path, .. }
            | Intent::Import { path, .. }
            | Intent::MakeDir { path }
            | Intent::RemoveDir { path }
            | Intent::MoveFolder { from: path, .. } => named(path),
            Intent::Forget { .. } | Intent::Take { .. } | Intent::Dismiss { .. } => {
                self.label().to_string()
            }
        }
    }

    /// What the history and other writers call it.
    pub fn label(&self) -> &'static str {
        match self {
            Intent::Tag { make: true, .. } => "New tag",
            Intent::Tag { .. } => "Tag",
            Intent::Untag { .. } => "Untag",
            Intent::MakeTag { .. } => "New tag",
            Intent::RenameTag { .. } => "Rename tag",
            Intent::RemoveTag { .. } => "Remove tag",
            Intent::Forget { .. } => "Forget",
            Intent::Take { .. } => "Take kept-apart changes",
            Intent::Dismiss { .. } => "Dismiss kept-apart changes",
            Intent::Save {
                expect: toshokan::Expect::Absent,
                ..
            } => "New",
            Intent::Save { .. } | Intent::Rewrite { .. } => "Save",
            Intent::Import { .. } => "Copy in",
            Intent::Move { moves } => {
                let renames = |moving: &Moving| moving.from.parent() == moving.to.parent();
                match moves.iter().all(renames) {
                    true => "Rename",
                    false => "Move",
                }
            }
            Intent::MoveFolder { from, to } => match from.parent() == to.parent() {
                true => "Rename",
                false => "Move",
            },
            Intent::Delete { .. } => "Delete",
            Intent::MakeDir { .. } => "New folder",
            Intent::RemoveDir { .. } => "Remove folder",
            Intent::Batch(parts) => gesture(parts).map_or("Change", Intent::label),
        }
    }

    /// Whether what it writes is filled from a file, whose read may fail before
    /// anything is written.
    pub fn reads(&self) -> bool {
        match self {
            Intent::Rewrite { .. } | Intent::Import { .. } => true,
            Intent::Batch(parts) => parts.iter().any(Intent::reads),
            _ => false,
        }
    }

    /// How many files or folders it changes, for the name of a batch of them.
    fn files(&self) -> usize {
        match self {
            Intent::Move { moves } => moves.len(),
            Intent::Delete { files } => files.len(),
            Intent::Batch(parts) => parts.iter().map(Intent::files).sum(),
            _ => 1,
        }
    }
}

/// Changes a reader shows apart from the library rather than merging them: a branch of
/// a writer's history two computers or a copied install continued, entries waiting for
/// the one before them, or a version a sync client dropped.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct KeptApart {
    pub kind: KeptKind,
    pub run: toshokan::log::Run,
    pub by: toshokan::report::By,
    /// When its entries were made, earliest and latest, in milliseconds since the Unix
    /// epoch.
    pub span: (u64, u64),
    /// What taking them would change; none once the library holds all they say.
    pub changes: Vec<KeptChange>,
    /// They may be their writer's own older changes, which it replaced since.
    pub uncertain: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum KeptKind {
    /// A branch of a writer's history: its changes may be taken, or it dismissed.
    Run,
    /// Entries waiting for an earlier one to sync, merged once it does.
    Gap,
    /// A version of a writer's history a sync client dropped: its changes may be
    /// taken.
    Unplaced,
}

/// What a change kept apart does to its field.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum KeptDo {
    Gain(String),
    Lose(String),
    Set(String),
    Clear,
    Delete,
    Revive,
    /// Binds the entity to the library file at this path.
    File(String),
}

impl KeptDo {
    fn of(change: &toshokan::plan::FactChange) -> KeptDo {
        use toshokan::plan::FactChange;
        let text = |raw: &toshokan::Raw| {
            serde_json::from_str::<String>(raw.as_str()).unwrap_or_else(|_| raw.as_str().into())
        };
        match change {
            FactChange::Add { value, .. } => KeptDo::Gain(text(value)),
            FactChange::Remove { value, .. } => KeptDo::Lose(text(value)),
            FactChange::Set { value, .. } => KeptDo::Set(text(value)),
            FactChange::Clear { .. } => KeptDo::Clear,
            FactChange::Delete { .. } => KeptDo::Delete,
            FactChange::Revive { .. } => KeptDo::Revive,
            FactChange::File { file, .. } => KeptDo::File(file.path.to_string()),
        }
    }
}

/// Which change of a [`KeptApart`] an [`Intent::Take`] takes.
pub type KeptKey = (EntityId, toshokan::report::What);

/// One change a [`KeptApart`] would make.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct KeptChange {
    pub entity: EntityId,
    pub what: toshokan::report::What,
    /// What taking it does; `None` where it left the entity without a file, which only
    /// trashing the file does.
    pub takes: Option<KeptDo>,
    /// Shown whatever the library holds: it replaced a value no reader holds any more,
    /// so only taking it settles it.
    pub forgotten: bool,
}

impl KeptApart {
    pub fn of(item: &toshokan::report::Item) -> KeptApart {
        use toshokan::report::ItemKind;
        KeptApart {
            kind: match item.kind {
                ItemKind::Run => KeptKind::Run,
                ItemKind::Gap => KeptKind::Gap,
                ItemKind::Unplaced => KeptKind::Unplaced,
            },
            run: item.run,
            by: item.by.clone(),
            span: (item.span.0.wall_ms, item.span.1.wall_ms),
            changes: item
                .changes
                .iter()
                .map(|change| KeptChange {
                    entity: change.entity,
                    what: change.what.clone(),
                    takes: change.change.as_ref().map(KeptDo::of),
                    forgotten: change.forgotten,
                })
                .collect(),
            uncertain: item.uncertain,
        }
    }
}

/// What opening found.
#[derive(Debug, Serialize, Deserialize)]
pub struct Opened {
    /// `Err` with the reason when nothing may be written here. The library still opens,
    /// and every edit stays in memory.
    pub writable: Result<(), String>,
    /// What the library kept of its view, which lists no file until a
    /// [`Cmd::Rescan`] has scanned every one.
    pub viewed: Viewed,
    /// The unsaved edits an earlier session kept, each with whether its file still holds
    /// what the edit was made over.
    pub drafts: Vec<toshokan::report::DraftStatus>,
    /// How many pending records of other writers opening could not read.
    pub ignored: usize,
    /// What an older drawbar kept in the folder, the first time this install opens it.
    pub older: Vec<older::Older>,
    /// What opening tried to tidy and could not: finishing a change a crash or
    /// another window left, or emptying the trash.
    pub trouble: Vec<String>,
}

/// The library as one answer left it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Viewed {
    /// What the answer changed of what the app shows.
    pub shown: shown::Change,
    /// Every folder below the root, in path order, where the answer listed them.
    pub dirs: Option<Vec<LibPath>>,
    /// What changed since the view before, and who changed it.
    pub changes: Vec<toshokan::report::Change>,
    /// The files the answer's scan found moved outside drawbar, from and to.
    pub moves: Vec<(LibPath, LibPath)>,
    /// What this session's undo or redo did, which the workspace follows without a
    /// word: each tree it moved, then each file no entity is bound to that it made,
    /// moved or trashed, from and to.
    pub undone: Option<Vec<(Option<LibPath>, Option<LibPath>)>>,
    /// The folders a scan of every file could not list, where the answer scanned every
    /// file. What is in them is not known.
    pub unlisted: Option<Vec<LibPath>>,
    /// The changes other installs' drawbars left unfinished as they stand now, which the
    /// user settles, where the answer read them.
    pub unfinished: Option<Vec<toshokan::report::Orphan>>,
    /// The changes the library keeps apart, as the view after the answer shows them.
    pub kept: Vec<KeptApart>,
    /// Whether the answer found this install's history forked, as by a copy of its own
    /// data used elsewhere too: it goes on writing a history of its own.
    pub retired: bool,
}

/// A fault a test makes the library's thread or worker show.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub enum Fault {
    /// It stops, as a bug that panics or traps makes it.
    Trap,
    /// It never finishes the command it takes next.
    Stall,
    /// The page cannot read the answer to the command the worker takes next.
    #[cfg(target_arch = "wasm32")]
    Unread,
}

/// What a backend answers.
#[derive(Debug, Serialize, Deserialize)]
pub enum Event {
    Opened(Result<Opened, String>),
    /// What a [`Cmd::Rescan`] or a [`Cmd::Refresh`] found.
    Scanned(Result<Viewed, String>),
    /// Whether an intent, an undo or a redo was made, as a step this session's undo
    /// takes back, by the entry that logged it, and the view after it, or why not. A change to a file that failed is
    /// told by its own answer, and is answered here with the view as it stands.
    Committed {
        label: String,
        made: Option<toshokan::EntryHash>,
        result: Result<Viewed, String>,
    },
    /// The answers to the change `label` were lost: whether it was made is not known.
    Unheard {
        label: String,
    },
    /// The trash grew past what it holds in a session, and its oldest items went: the
    /// entries whose undo needed them.
    Trimmed(Vec<toshokan::EntryHash>),
    /// What [`Cmd::Read`] read, each with its asset's id. A file not where it was asked
    /// for answers [`Failure::Moved`].
    Read(Vec<(u64, Result<Found, Failure>)>),
    Saved {
        id: u64,
        path: LibPath,
        result: Result<Fingerprint, Failure>,
    },
    /// What an [`Intent::Rewrite`] wrote, as a listing finds it.
    Rewritten {
        id: u64,
        path: LibPath,
        result: Result<Found, Failure>,
    },
    /// What an [`Intent::Import`] copied, as a listing finds it.
    Imported {
        id: u64,
        path: LibPath,
        result: Result<Found, Failure>,
    },
    /// Whether an [`Intent::Move`] or an [`Intent::MoveFolder`] moved anything, and why
    /// not.
    Moved {
        from: LibPath,
        to: LibPath,
        result: Result<(), String>,
    },
    /// A command other than a save, a copy or a move failed: what it was doing, and
    /// why.
    Failed(String),
    /// Whether [`Cmd::Draft`] kept the unsaved edit of the file `of` names.
    Drafted {
        of: Key,
        kept: Result<(), String>,
    },
    /// An unsaved edit could not be let go of, or a slot's edit kept: why.
    Unkept(String),
    /// The edit [`Cmd::ReadSlot`] found kept of a slot's view, with the hash of the
    /// slot's bytes it was made over, or why it could not be read.
    SlotKept {
        slot: Slot,
        kept: Result<Option<(blake3::Hash, Vec<u8>)>, String>,
    },
}

impl Event {
    /// The codes this answer's failures are counted under, each once. Never a path or a
    /// message, which name files.
    pub fn faults(&self) -> std::collections::BTreeSet<&'static str> {
        let one = |code| std::iter::once(code).collect();
        match self {
            Event::Opened(Err(_)) => one("open"),
            Event::Opened(Ok(opened)) if opened.writable.is_err() => one("read-only"),
            Event::Scanned(Err(_)) => one("rescan"),
            Event::Committed { result: Err(_), .. } => one("intent"),
            Event::Moved { result: Err(_), .. } => one("move"),
            Event::Failed(_) => one("write"),
            Event::Unkept(_)
            | Event::Drafted { kept: Err(_), .. }
            | Event::SlotKept { kept: Err(_), .. } => one("draft"),
            Event::Read(answers) => answers
                .iter()
                .filter_map(|(_, result)| result.as_ref().err())
                .map(|failure| READ.of(failure))
                .collect(),
            Event::Saved { result, .. } => SAVE.failed(result),
            Event::Imported { result, .. } => IMPORT.failed(result),
            Event::Rewritten { result, .. } => REWRITE.failed(result),
            Event::Unheard { .. } => one("unheard"),
            Event::Opened(Ok(_))
            | Event::Drafted { kept: Ok(()), .. }
            | Event::SlotKept { kept: Ok(_), .. }
            | Event::Trimmed(_)
            | Event::Scanned(Ok(_))
            | Event::Committed { result: Ok(_), .. }
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
            Failure::Unheard => "unheard",
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
