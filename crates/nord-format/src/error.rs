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

    #[error("unknown file type: {0}")]
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
    /// Decoding a newer one could produce plausible but wrong values, and writing it
    /// back would persist them.
    #[error(
        "{format}: schema version {version} is not supported (known: {supported:?}); \
             decoding it could misread fields"
    )]
    UnsupportedVersion {
        format: &'static str,
        version: u32,
        supported: &'static [u32],
    },

    /// A body whose length is not the one the format declares: a truncated or padded
    /// file on read, or a miscounting writer on write.
    #[error("{format}: the body is {got} bytes, but the format holds {expected}")]
    WrongBodyLength {
        format: String,
        got: u64,
        expected: u64,
    },

    /// A bundle's ZIP container in a shape [`crate::bundle::archive`] does not read.
    #[error(transparent)]
    Archive(#[from] crate::bundle::archive::ArchiveError),
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

/// An empty vector with room for `len` elements, reporting an allocation the platform
/// cannot make instead of aborting the process on it. The error counts `len` in `unit`.
pub(crate) fn try_with_capacity<T>(
    len: usize,
    unit: &str,
) -> std::result::Result<Vec<T>, ParseError> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| ParseError::OutOfBounds {
            value: format!("{len} {unit}"),
            bound: "an allocation that fits memory".into(),
        })?;
    Ok(out)
}

/// `len` zeroed elements, failing as [`try_with_capacity`] does.
pub(crate) fn try_zeroed<T: Clone + Default>(
    len: usize,
    unit: &str,
) -> std::result::Result<Vec<T>, ParseError> {
    let mut out = try_with_capacity(len, unit)?;
    out.resize(len, T::default());
    Ok(out)
}

/// A zeroed buffer of `len` bytes, failing as [`try_with_capacity`] does.
pub(crate) fn try_vec(len: usize) -> std::result::Result<Vec<u8>, ParseError> {
    try_zeroed(len, "bytes")
}
