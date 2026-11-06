//! The stub-format machinery: a module per tag whose body is not yet mapped.
//!
//! A stub reads and writes the container (header parsed, checksum verified, body
//! kept verbatim), so every file round-trips byte-exactly while its body is unmapped.

/// Declare one stub format module: its tag, and the body length its specimens share
/// (omit the length for variable-length library formats).
macro_rules! raw_format {
    ($(#[$meta:meta])* $name:ident, $tag:literal $(, $body_len:literal)?) => {
        $(#[$meta])*
        #[doc = ""]
        #[doc = "Container-verified stub: reads and writes byte-exactly, header \
                 parsed and checksum verified, body kept verbatim and undecoded."]
        pub mod $name {
            use crate::cbin::{self, Cbin, RawBody};
            use crate::error::Error;
            use std::io::{Read, Seek};

            pub const FORMAT: &str = $tag;

            const _: () = assert!(FORMAT.len() == 4, "a CBIN tag is four bytes");

            $(
                /// The body length every specimen holds. It is not
                /// enforced on read: a raw body cannot be misread, so a file of
                /// another length is preserved.
                pub const BODY_LEN: usize = $body_len;
            )?

            pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<RawBody>, Error> {
                cbin::read(reader, FORMAT)
            }
        }
    };
}

pub(crate) use raw_format;
