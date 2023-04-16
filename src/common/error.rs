use std::fmt::{Debug, Formatter};

pub enum Error {
    Io(std::io::Error),
    InvalidSchema,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "IO error: {}", e),
            Error::InvalidSchema => write!(f, "Invalid schema"),
        }
    }
}

impl Debug for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        <Error as std::fmt::Display>::fmt(self, f)
    }
}

impl std::error::Error for Error {}
