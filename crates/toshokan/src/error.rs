use thiserror::Error as ThisError;

use crate::effects::Precondition;
use crate::fs::{Capability, Fingerprint, RelPath};
use crate::ids::{EntityId, WriterId};
use crate::undo::Refusal;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(ThisError, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("{path} does not exist")]
    NotFound { path: RelPath },

    #[error("{path} already exists")]
    AlreadyExists { path: RelPath },

    #[error("{path} is a directory")]
    IsDirectory { path: RelPath },

    #[error("{path} is not a directory")]
    NotDirectory { path: RelPath },

    #[error("directory {path} is not empty")]
    DirectoryNotEmpty { path: RelPath },

    #[error("no space is left for {path}")]
    NoSpace { path: RelPath },

    /// The backend does not declare the capability the operation needs. Callers plan
    /// from [`crate::fs::Capabilities`]; reaching this is a planning bug.
    #[error("the file system cannot {0}")]
    Unsupported(Capability),

    /// An in-memory file system was told to crash. Every later operation on it fails
    /// the same way.
    #[error("the file system crashed")]
    Crashed,

    #[error("{path}: {source}")]
    Io {
        path: RelPath,
        #[source]
        source: std::io::Error,
    },

    #[error("{path:?} is not a valid library path: {reason}")]
    InvalidPath { path: String, reason: &'static str },

    #[error("{text:?} is not a valid {what}")]
    InvalidId { what: &'static str, text: String },

    /// A file toshokan owns cannot be read as its format requires. A torn log tail
    /// is not corruption; it ends the readable log.
    #[error("{path} is corrupt: {reason}")]
    Corrupt { path: RelPath, reason: String },

    /// This writer's own log holds entries this build does not understand, so it
    /// cannot append without misrepresenting its history.
    #[error("writer {writer} is read-only: {reason}")]
    ReadOnly { writer: WriterId, reason: String },

    /// The log given for a writer was read before that writer's latest append, so
    /// folding it would lose entries.
    #[error("the log of writer {writer} was read before its latest append")]
    StaleLog { writer: WriterId },

    /// A file effect found the file other than the writer expected. Nothing was
    /// changed.
    #[error("{} is not as expected", .0.path)]
    Changed(Box<Mismatch>),

    /// A file was not as expected partway through an intent's file effects. The
    /// effects before it were made and logged; the files in `stayed` were left where
    /// and as they were.
    #[error("{error}, so the intent was applied in part")]
    Partial {
        error: Box<Error>,
        stayed: Vec<RelPath>,
    },

    #[error("cannot undo or redo: {0}")]
    Refused(Box<Refusal>),

    /// An intent the merged state does not allow on this entity. Nothing was written.
    #[error("{entity} {reason}")]
    Entity {
        entity: EntityId,
        reason: &'static str,
    },
}

/// What the writer expected at a path, and the file found there; `None` is no file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mismatch {
    pub path: RelPath,
    pub expected: Precondition,
    pub found: Option<Fingerprint>,
}
