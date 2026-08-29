use thiserror::Error as ThisError;

use crate::wire::Location;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(ThisError, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("message truncated: got {got} bytes, need at least {need}")]
    Truncated { got: usize, need: usize },

    #[error("length field says {declared} bytes but the message is {actual}")]
    LengthMismatch { declared: usize, actual: usize },

    #[error("crc mismatch: message carries {expected:#06x}, computed {actual:#06x}")]
    BadCrc { expected: u16, actual: u16 },

    #[error("device reported status {0:#x}")]
    DeviceStatus(u32),

    #[error("expected a response to command {expected:#x}, got {got:#x}")]
    UnexpectedResponse { expected: u32, got: u32 },

    #[error("device reported location {reported:?} for the requested location {requested:?}")]
    UnexpectedLocation {
        requested: Location,
        reported: Location,
    },

    /// The byte pipe itself failed — a USB transfer error, a missing device, a claim
    /// refusal. Nothing about message *content* belongs here.
    #[error("transport: {0}")]
    Transport(String),

    /// The `CBIN` header around an entity body is wrong: bad magic, a checksum that
    /// does not match the body, a malformed format tag.
    #[error("envelope: {0}")]
    Envelope(String),

    /// A replay script that could not be parsed or was contradicted by the code under
    /// test. Only produced by the `replay` feature's transport.
    #[error("replay: {0}")]
    Replay(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error(transparent)]
    Format(#[from] nord_format::error::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// This failure's name in a replay script's `expect: err <kind>` header.
    ///
    /// The vocabulary is short on purpose — it exists to tell one *expected* refusal
    /// from another — so a failure it does not name is reported as the nearest kind
    /// rather than left out, where the script would claim the operation succeeded. A
    /// script that names the wrong kind fails the sweep, which is the report; a script
    /// that names none passes silently, which is not.
    pub fn expect_kind(&self) -> String {
        match self {
            Error::DeviceStatus(code) => format!("device-status {code:#x}"),
            Error::UnexpectedResponse { .. } => "unexpected-response".into(),
            Error::UnexpectedLocation { .. } => "unexpected-location".into(),
            Error::Replay(_) => "replay".into(),
            _ => "transport".into(),
        }
    }
}
