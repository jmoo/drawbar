use thiserror::Error as ThisError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(ThisError, Debug)]
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

    #[error("transport: {0}")]
    Transport(String),

    #[error("{0}")]
    Format(#[from] nord_format::error::Error),

    #[error("{0}")]
    Io(#[from] std::io::Error),
}
