//! What can go wrong: [`ParseError`] for a file that violates its format,
//! [`Error`] folding that together with I/O (and ZIP, under `bundle`).

use std::io;
use thiserror::Error as ThisError;

/// Shorthand for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

/// A file that violates its format: unknown tags or lengths, checksum
/// mismatches, out-of-range values, unsupported schema versions.
#[derive(ThisError, Debug)]
#[non_exhaustive]
pub enum ParseError {
    #[error("value {value} is outside {bound}")]
    OutOfBounds { value: String, bound: String },

    #[error("unknown format: {0}")]
    UnknownFormat(String),

    #[error("unknown filetype: {0}")]
    UnknownFileType(String),

    /// A CBIN tag other than the one the reader was asked for. Formats sharing a body
    /// layout decode each other's files without complaint, so the tag is the only thing
    /// that tells them apart.
    #[error("expected a {expected} file, got {got}")]
    WrongFormat { expected: &'static str, got: String },

    #[error("{0}")]
    AssertFail(String),

    /// A file whose schema version this build has never been validated against.
    ///
    /// Field offsets are only known to be right for the versions in the corpus.
    /// Decoding a newer one would produce plausible-looking but wrong values, and
    /// writing it back would then persist them — so refuse instead.
    #[error(
        "{format}: schema version {version} is not supported (known: {supported:?}); \
             refusing to decode rather than risk misreading fields"
    )]
    UnsupportedVersion {
        format: &'static str,
        version: u32,
        supported: &'static [u32],
    },

    /// A body whose length is not the one the format declares — a truncated or
    /// padded file on read, a miscounting writer on write.
    #[error(
        "{format}: the body is {got} bytes where the format holds {expected}; \
             refusing rather than misread fields"
    )]
    WrongBodyLength {
        format: String,
        got: u64,
        expected: u64,
    },
}

/// Lets an infallible decode sit alongside fallible ones behind the same `?`.
impl From<std::convert::Infallible> for ParseError {
    fn from(never: std::convert::Infallible) -> Self {
        match never {}
    }
}

/// Everything a read or write can fail with: I/O, a format violation, or
/// (under `bundle`) a ZIP error.
#[derive(ThisError, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Parse(#[from] ParseError),

    #[cfg(feature = "bundle")]
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}
