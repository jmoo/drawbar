//! Electro 2 sample libraries (`.cn3`) — magic `CNE3`, not CBIN.
//!
//! All fourteen known files open `CNE3` (never `CNE2`); why the 3 is unexplained.
//! It is not a version field — the four bytes after it read `2c 01`, i.e. 300,
//! which is where a version *would* sit. Nothing about the CBIN core generalises
//! here, so the whole file is kept verbatim.

use crate::error::{Error, ParseError};
use std::io::{Read, Write};

pub const MAGIC: &[u8; 4] = b"CNE3";

/// One `.cn3` library, verbatim. ⚠️ Real libraries run to megabytes and this
/// allocates them whole.
#[derive(Clone, PartialEq, Eq)]
pub struct Cne3 {
    pub data: Vec<u8>,
}

impl Cne3 {
    pub fn read_from(reader: &mut impl Read) -> Result<Cne3, Error> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        if data.len() < 4 || &data[0..4] != MAGIC {
            return Err(ParseError::UnknownFileType("not a CNE3 library".to_string()).into());
        }
        Ok(Cne3 { data })
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), Error> {
        writer.write_all(&self.data)?;
        Ok(())
    }
}

impl std::fmt::Debug for Cne3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cne3")
            .field("len", &self.data.len())
            .finish()
    }
}
