use std::io;
use thiserror::Error as ThisError;

pub type Result<T> = std::result::Result<T, Error>;

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

    /// A re-encode that did not produce the length the format declares. Guards against
    /// a writer silently emitting a truncated file.
    #[error(
        "{format}: re-encoded to {got} bytes but the format is {expected}; \
             refusing to emit a truncated file"
    )]
    BadEncodedLength {
        format: &'static str,
        got: usize,
        expected: usize,
    },
}

/// Lets an infallible decode sit alongside fallible ones behind the same `?`.
impl From<std::convert::Infallible> for ParseError {
    fn from(never: std::convert::Infallible) -> Self {
        match never {}
    }
}

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

impl From<binrw::Error> for Error {
    fn from(value: binrw::Error) -> Self {
        match value {
            binrw::Error::Io(e) => Error::Io(e),
            // A `try_map`/`try_calc` that failed with one of this crate's own errors:
            // recover the typed value instead of flattening it to a string, so a
            // caller can still match on `OutOfBounds` etc.
            binrw::Error::Custom { err, .. } => match err.downcast::<ParseError>() {
                Ok(parse) => Error::Parse(*parse),
                Err(other) => Error::Parse(ParseError::AssertFail(format!("{other:?}"))),
            },
            // binrw wraps nested errors in a backtrace; the cause is inside.
            binrw::Error::Backtrace(bt) => Error::from(*bt.error),
            e => Error::Parse(ParseError::AssertFail(e.to_string())),
        }
    }
}
