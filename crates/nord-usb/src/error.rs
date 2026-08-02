use thiserror::Error as ThisError;

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
