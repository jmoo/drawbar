//! The stub-format machinery: a module per tag whose body is not yet mapped.
//!
//! A stub reads and writes the container — header parsed, checksum verified, body
//! kept verbatim — so every file round-trips byte-exactly while its body waits to
//! be reverse-engineered. The observed body length is recorded as data for tests,
//! never enforced on read: a raw body cannot misread, so an unexpected length is
//! preserved rather than refused.

/// Declare one stub format module: its tag, and the body length its corpus
/// specimens share (omit the length for variable-length library formats).
macro_rules! raw_format {
    ($(#[$meta:meta])* $name:ident, $tag:literal $(, $body_len:literal)?) => {
        $(#[$meta])*
        pub mod $name {
            use crate::cbin::{self, Cbin, RawBody};
            use crate::error::Error;
            use std::io::{Read, Seek};

            pub const FORMAT: &str = $tag;

            $(
                /// The body length every corpus specimen holds. Observed, not
                /// enforced — see [`crate::formats::raw`].
                pub const BODY_LEN: u64 = $body_len;
            )?

            pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<RawBody>, Error> {
                cbin::read(reader, FORMAT)
            }
        }
    };
}

pub(crate) use raw_format;
