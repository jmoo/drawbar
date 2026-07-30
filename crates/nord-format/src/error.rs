use std::io;
use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum ParseError {
    #[error("value `{0}` exceeds bound `{1}`")]
    OutOfBounds(String, String),

    #[error("unknown format: {0}")]
    UnknownFormat(String),

    #[error("unknown filetype: {0}")]
    UnknownFileType(String),

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
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),

    #[error("{0}")]
    ParseError(#[from] ParseError),
}

impl From<binrw::Error> for Error {
    fn from(value: binrw::Error) -> Self {
        match value {
            binrw::Error::Io(e) => Error::Io(e),
            e => Error::ParseError(ParseError::AssertFail(format!("{:?}", e))),
        }
    }
}
