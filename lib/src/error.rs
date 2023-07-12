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
