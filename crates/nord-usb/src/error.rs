use thiserror::Error as ThisError;

use crate::wire::{Location, ObjectClass};

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

    /// The device refused `SESSION_OPEN` for this class: it does not serve the class.
    /// A refusal of any other command stays a [`Error::DeviceStatus`].
    #[error("the device refused a session for {class:?} with status {status:#x}")]
    ClassRefused { class: ObjectClass, status: u32 },

    #[error("expected a response to command {expected:#x}, got {got:#x}")]
    UnexpectedResponse { expected: u32, got: u32 },

    #[error("device reported location {reported:?} for the requested location {requested:?}")]
    UnexpectedLocation {
        requested: Location,
        reported: Location,
    },

    #[error("device reported partition {reported} for the requested partition {requested}")]
    UnexpectedPartition { requested: u32, reported: u32 },

    /// An inventory walk contradicted the geometry declared by the instrument.
    #[error(
        "walking bank {bank}, which declares {slots} slots, became inconsistent at \
         {answered:?}"
    )]
    Enumeration {
        bank: u32,
        answered: Location,
        slots: u32,
    },

    #[error("bank {bank} cannot be scanned completely within {limit} slots")]
    ScanLimit { bank: u32, limit: u32 },

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
    Io(#[from] std::io::Error),
}

impl Error {
    /// This failure's kind in a replay script's `expect: err <kind>` header.
    ///
    /// The vocabulary is short on purpose — it exists to tell one *expected* refusal
    /// from another — so a failure it does not name is reported as the nearest kind
    /// rather than left out, where the script would claim the operation succeeded. A
    /// script that names the wrong kind fails the sweep, which is the report; a script
    /// that names none passes silently, which is not.
    ///
    /// The match is exhaustive so that a new variant is given a kind rather than
    /// defaulting into one, and [`ErrKind::matches`] is its inverse.
    pub fn expect_kind(&self) -> ErrKind {
        match self {
            Error::DeviceStatus(code) => ErrKind::DeviceStatus(*code),
            Error::ClassRefused { status, .. } => ErrKind::ClassRefused(*status),
            Error::UnexpectedResponse { .. } => ErrKind::UnexpectedResponse,
            Error::UnexpectedLocation { .. } => ErrKind::UnexpectedLocation,
            Error::UnexpectedPartition { .. } => ErrKind::UnexpectedPartition,
            Error::Enumeration { .. } | Error::ScanLimit { .. } => ErrKind::Enumeration,
            Error::Replay(_) => ErrKind::Replay,
            Error::Truncated { .. }
            | Error::LengthMismatch { .. }
            | Error::BadCrc { .. }
            | Error::Transport(_)
            | Error::Envelope(_)
            | Error::InvalidArgument(_)
            | Error::Io(_) => ErrKind::Transport,
        }
    }
}

/// The failures a replay script may name, spelled in kebab-case after the [`Error`]
/// variant.
///
/// Deliberately a short list: it exists to tell one *expected* refusal from another, not
/// to mirror the error type. A device refusal carries its status code, because the code
/// is the finding — `0x15` (the library classes refusing a rename) and `0x1` (nothing
/// loaded) are different results, not two spellings of one.
///
/// [`Error::expect_kind`] is the one table: what a recorder writes, what a script
/// parses, and what the sweep judges are the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    DeviceStatus(u32),
    ClassRefused(u32),
    UnexpectedResponse,
    UnexpectedLocation,
    UnexpectedPartition,
    Enumeration,
    Transport,
    Replay,
}

impl ErrKind {
    /// Whether an error is the one this names.
    pub fn matches(&self, e: &Error) -> bool {
        *self == e.expect_kind()
    }

    /// The inverse of [`Display`](std::fmt::Display): read a kind as a script spells it.
    #[cfg(feature = "replay")]
    pub(crate) fn parse(value: &str) -> std::result::Result<Self, String> {
        let (kind, arg) = match value.split_once(char::is_whitespace) {
            Some((kind, arg)) => (kind, arg.trim()),
            None => (value, ""),
        };
        match (kind, arg) {
            ("device-status", "") => Err("device-status needs its code, e.g. \
                                          'err device-status 0x15'"
                .into()),
            ("device-status", code) => parse_u32(code)
                .map(ErrKind::DeviceStatus)
                .ok_or_else(|| format!("bad device status {code:?}")),
            ("class-refused", "") => Err("class-refused needs its code, e.g. \
                                         'err class-refused 0x5'"
                .into()),
            ("class-refused", code) => parse_u32(code)
                .map(ErrKind::ClassRefused)
                .ok_or_else(|| format!("bad class refusal status {code:?}")),
            ("unexpected-response", "") => Ok(ErrKind::UnexpectedResponse),
            ("unexpected-location", "") => Ok(ErrKind::UnexpectedLocation),
            ("unexpected-partition", "") => Ok(ErrKind::UnexpectedPartition),
            ("enumeration", "") => Ok(ErrKind::Enumeration),
            ("transport", "") => Ok(ErrKind::Transport),
            ("replay", "") => Ok(ErrKind::Replay),
            (kind, _) => Err(format!(
                "unknown failure {kind:?}; the vocabulary is device-status <code>, \
                class-refused <code>, unexpected-response, unexpected-location, \
                unexpected-partition, enumeration, \
                transport, replay"
            )),
        }
    }
}

impl std::fmt::Display for ErrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrKind::DeviceStatus(code) => write!(f, "device-status {code:#x}"),
            ErrKind::ClassRefused(code) => write!(f, "class-refused {code:#x}"),
            ErrKind::UnexpectedResponse => f.write_str("unexpected-response"),
            ErrKind::UnexpectedLocation => f.write_str("unexpected-location"),
            ErrKind::UnexpectedPartition => f.write_str("unexpected-partition"),
            ErrKind::Enumeration => f.write_str("enumeration"),
            ErrKind::Transport => f.write_str("transport"),
            ErrKind::Replay => f.write_str("replay"),
        }
    }
}

/// `0x`-prefixed hex or decimal — status codes are quoted both ways.
#[cfg(feature = "replay")]
fn parse_u32(s: &str) -> Option<u32> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}
