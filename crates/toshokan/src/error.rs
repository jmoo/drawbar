use thiserror::Error as ThisError;

use crate::ids::{EntityId, EntryHash, Identity, WriterId};
use crate::io::{Capability, IoError, Root};
use crate::path::RelPath;
use crate::plan::Expect;
use crate::report::Rekey;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(ThisError, Debug)]
pub enum Error {
    #[error("{root:?} {path}: {error}")]
    Io {
        root: Root,
        path: RelPath,
        error: IoError,
    },

    #[error("{path:?} is not a valid path: {reason}")]
    InvalidPath { path: String, reason: &'static str },

    #[error("{text:?} is not a valid {what}")]
    InvalidId { what: &'static str, text: String },

    #[error("the schema declares {name:?} twice")]
    DuplicateKey { name: &'static str },

    #[error("{name:?} is not a valid key name")]
    InvalidKey { name: &'static str },

    /// The library is open read-only, so nothing can be written.
    #[error("the library is read-only: {0}")]
    ReadOnly(Why),

    /// An intent, undo or redo was refused before anything was written.
    #[error("refused: {0}")]
    Refused(Refusal),

    /// This writer cannot write again: appending would continue a history the
    /// folder no longer holds, or that another instance also continues. Nothing
    /// was written; the library continues as a new writer.
    #[error("writer {writer} cannot continue its history: {why:?}")]
    Rekey { writer: WriterId, why: Rekey },
}

/// Why a library is read-only.
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
pub enum Why {
    /// This writer's own history holds entries this build does not understand, so
    /// it cannot append without misrepresenting it.
    #[error("this writer's history was written by a newer version")]
    NewerOwnHistory { entry: EntryHash },
    /// The folder cannot append to files, which writing needs.
    #[error("the folder cannot be written")]
    FolderNotWritable,
    /// The clock cannot advance past this writer's last entry: it holds the last
    /// reading there is.
    #[error("this writer's clock is exhausted")]
    ClockExhausted,
    /// A change this writer began stopped where the folder refused the rest at
    /// `path`. It stays open, and opening the library again tries the rest again.
    #[error("{label:?} could not be finished: {path}: {error}")]
    Unfinished {
        label: String,
        path: RelPath,
        error: IoError,
    },
}

/// Why an intent, undo or redo was refused. A refusal changes nothing.
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// A file was not as the intent expected.
    #[error("{} is not as expected", .0.path)]
    Changed(Box<Mismatch>),
    /// A directory is where the intent expected a file or nothing.
    #[error("{0} is a directory")]
    Directory(RelPath),
    /// The intent is not valid against the current view.
    #[error("{0}")]
    Invalid(Invalid),
    /// Another writer changed what the undo or redo would change since this writer
    /// did.
    #[error("{by} changed it since")]
    ChangedSince { by: WriterId, entry: EntryHash },
    /// There is nothing to undo, redo, settle, compact or adopt.
    #[error("there is nothing to do")]
    Nothing,
    /// The snapshot a compaction would write is longer than a reader holds.
    #[error("the snapshot is longer than a reader holds")]
    SnapshotTooLong,
    /// The bytes an undo would bring back have left the trash.
    #[error("the trash no longer holds what undo needs")]
    Emptied,
    /// The folder cannot do what settling another writer's effect needs, as
    /// rename a directory that writer's folder renamed whole.
    #[error("the folder cannot {0}")]
    Unsupported(Capability),
    /// What settling an effect would put at this library path is gone from the
    /// folder, so the effect cannot be carried that way.
    #[error("what goes at {0} is gone")]
    Gone(RelPath),
}

/// What the intent expected at a path, and what was found; `None` is nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mismatch {
    pub path: RelPath,
    pub expected: Expect,
    pub found: Option<Identity>,
}

#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
pub enum Invalid {
    #[error("{0} does not exist")]
    NoEntity(EntityId),
    #[error("{0} has no file")]
    NoFile(EntityId),
    #[error("{0} is an entity's file")]
    Bound(RelPath),
    #[error("{0} is not declared in the schema")]
    UndeclaredKey(String),
    #[error("a value for {key} cannot be written as JSON: {reason}")]
    Unencodable { key: String, reason: String },
    #[error("{0} is toshokan's own")]
    ReservedPath(RelPath),
    #[error("two effects of one intent touch {0}")]
    Overlapping(RelPath),
    #[error("{0} holds files")]
    NotEmpty(RelPath),
    #[error("the intent changes nothing")]
    Empty,
    #[error("the intent is too long to log as one entry")]
    TooLong,
    /// A file change taken from an item names a file the entity cannot be bound
    /// to now.
    #[error("{entity} cannot be bound to {path}: {why}")]
    Unbindable {
        entity: EntityId,
        path: RelPath,
        why: Unbindable,
    },
}

/// Why an entity cannot be bound to the file a run names, as the last scan
/// found the library's files.
#[derive(ThisError, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unbindable {
    #[error("no file is there")]
    Absent,
    #[error("the file there holds other contents")]
    Changed,
    /// No scan of the path has succeeded since the library opened, or since a
    /// scan of it failed.
    #[error("the file there is not scanned yet")]
    Unscanned,
    #[error("the file there is bound to {0}")]
    Bound(EntityId),
}
