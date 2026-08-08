//! Piano libraries (`.npno`).
//!
//! The body is not yet mapped, so a `Piano` is the container's facts plus the body
//! kept verbatim: it round-trips byte-exactly and verifies the checksum, and that
//! is all. ⚠️ Real libraries are tens of megabytes and this allocates the body —
//! [`crate::cbin::inspect`] answers container questions in O(1) instead.
//!
//! ⚠️ The header's `location` and `aux` are unchecked here on purpose: this is a
//! library format, where those words hold something other than a bank/slot pair, and
//! no local specimen says what. Gating on them would refuse real files.

use crate::cbin::{self, Cbin, Header, RawBody};
use crate::error::Error;
use std::fmt;
use std::fmt::Debug;
use std::io::{Read, Seek, Write};

pub const FORMAT: &str = "npno";

pub struct Piano {
    pub file: Cbin<RawBody>,
}

impl Piano {
    pub fn new() -> Piano {
        Piano {
            file: Cbin {
                header: Header::new(FORMAT, (0, 0), 0),
                body: RawBody(Vec::new()),
            },
        }
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Piano, Error> {
        Ok(Piano {
            file: cbin::read(reader, FORMAT)?,
        })
    }

    pub fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<(), Error> {
        self.file.write_to(writer)
    }
}

impl Default for Piano {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for Piano {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("npno::Piano")
            .field("header", &self.file.header)
            .field("body_len", &self.file.body.0.len())
            .finish()
    }
}
