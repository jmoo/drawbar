use thiserror::Error as ThisError;

use crate::ids::{EntityId, EntryHash, Identity, WriterId};
use crate::io::{IoError, Root};
use crate::path::RelPath;
use crate::plan::Expect;
use crate::report::Rekey;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(ThisError, Debug)]
#[non_exhaustive]
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

    /// A file in this install's local root cannot be read as its format requires.
    /// Files in the folder never cause this: what cannot be read there is reported.
    #[error("{path} in the local root is corrupt: {reason}")]
    Corrupt { path: RelPath, reason: String },

    /// The library is open read-only, so nothing can be written.
    #[error("the library is read-only: {0}")]
    ReadOnly(Why),

    /// An intent, undo or redo was refused before anything was written.
    #[error("refused: {0}")]
    Refused(Refusal),

    /// This instance has not written to the library yet, so it has no directory
    /// in the local root to keep drafts in.
    #[error("this instance has not committed anything yet")]
    NoWriter,

    /// This writer cannot write again: appending would continue a history the
    /// folder no longer holds, or that another instance also continues. Nothing
    /// was written; the library continues as a new writer.
    #[error("writer {writer} cannot continue its history: {why:?}")]
    Rekey { writer: WriterId, why: Rekey },
}

/// Why a library opened read-only.
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Why {
    /// This writer's own history holds entries this build does not understand, so
    /// it cannot append without misrepresenting it.
    #[error("this writer's history was written by a newer version")]
    NewerOwnHistory { entry: EntryHash },
    /// The folder cannot append to files or rename them, which writing needs.
    #[error("the folder cannot be written")]
    FolderNotWritable,
    /// The clock cannot advance past this writer's last entry.
    #[error("this writer's clock is exhausted")]
    ClockExhausted,
}

/// Why an intent, undo or redo was refused. A refusal changes nothing.
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Refusal {
    /// A file was not as the intent expected.
    #[error("{} is not as expected", .0.path)]
    Changed(Box<Mismatch>),
    /// The intent is not valid against the current view.
    #[error("{0}")]
    Invalid(Invalid),
    /// Another writer changed what the undo or redo would change since this writer
    /// did.
    #[error("{by} changed it since")]
    ChangedSince { by: WriterId, entry: EntryHash },
    /// There is nothing to undo or redo.
    #[error("there is nothing to undo or redo")]
    Nothing,
    /// The bytes an undo would bring back have left the trash.
    #[error("the trash no longer holds what undo needs")]
    Emptied,
}

/// What the intent expected at a path, and what was found; `None` is nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mismatch {
    pub path: RelPath,
    pub expected: Expect,
    pub found: Option<Identity>,
}

#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Invalid {
    #[error("{0} does not exist")]
    NoEntity(EntityId),
    #[error("{0} has no file")]
    NoFile(EntityId),
    #[error("{0} is not declared in the schema")]
    UndeclaredKey(String),
    #[error("a value for {key} cannot be written as JSON: {reason}")]
    Unencodable { key: String, reason: String },
    #[error("{0} is toshokan's own")]
    ReservedPath(RelPath),
    #[error("two effects of one intent touch {0}")]
    Overlapping(RelPath),
    #[error("the intent changes nothing")]
    Empty,
    #[error("the intent is too long to log as one entry")]
    TooLong,
}
