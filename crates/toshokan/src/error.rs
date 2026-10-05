use thiserror::Error as ThisError;

use crate::fs::{Capability, Fingerprint, RelPath};
use crate::ids::WriterId;
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

    /// A file effect found the file no longer matching what the writer last read.
    /// Nothing was changed.
    #[error("{} changed since it was last read", .0.path)]
    Changed(Box<Mismatch>),

    #[error("cannot undo or redo: {0}")]
    Refused(Box<Refusal>),
}

/// A file as the writer expected it and as it was found; `None` is no file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mismatch {
    pub path: RelPath,
    pub expected: Option<Fingerprint>,
    pub found: Option<Fingerprint>,
}
