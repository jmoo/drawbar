//! Piano libraries (`.npno`).
//!
//! The body is a `CNSP` stream: a header and metadata block, a 128-entry
//! per-note key map, then the stroke directory and encoded audio, which stay
//! unmapped. A `Piano` keeps the body verbatim — it round-trips byte-exactly
//! and verifies the checksum — and reads the mapped prefix in place.
//!
//! The prefix layout, derived from factory specimens spanning v5.0–v6.1 in
//! both container generations (independent interop projects report the same
//! shape); not confirmed on hardware:
//!
//! | body offset | field |
//! |---|---|
//! | `0x00` | `"CNSP"` |
//! | `0x04` | u16 BE stream version — `0x450` on v5.x files, `0x464` on v6.1 |
//! | `0x06` | u32, unique per file; meaning open |
//! | `0x16` | metadata block: the version echoed, then category-like bytes |
//! | `0x1c` | name field: `Name#Variant`, space-padded (`Electric Grand 1#CP80`) |
//! | `0x40` | on `0x464` streams, the bare name again, NUL-terminated |
//! | `0x8c` | 128-entry per-note key map, monotonic; `0xFF` = unused note |
//!
//! ⚠️ Real libraries are tens of megabytes and this allocates the body —
//! [`crate::cbin::inspect`] answers container questions in O(1) instead.
//!
//! ⚠️ The header's `location` and `aux` are unchecked here on purpose: this is a
//! library format, where those words hold something other than a bank/slot pair, and
//! no local specimen says what. Gating on them would refuse real files.

use crate::cbin::{self, Cbin, Header, RawBody};
use crate::error::{Error, ParseError};
use std::fmt;
use std::fmt::Debug;
use std::io::{Read, Seek, Write};

pub const FORMAT: &str = "npno";

/// The body's stream magic.
pub const CNSP_MAGIC: &[u8; 4] = b"CNSP";

/// Offset of the `Name#Variant` field within the body.
const NAME_AT: usize = 0x1c;
/// End of the name field — the byte past the longest observed content.
const NAME_END: usize = 0x40;
/// Offset of the 128-entry per-note key map.
const KEY_MAP_AT: usize = 0x8c;

/// A piano library (`npno`): the CNSP prefix read in place, body verbatim.
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

impl Piano {
    /// The body bytes, after checking they open with the `CNSP` magic.
    fn cnsp(&self) -> Result<&[u8], Error> {
        let body = &self.file.body.0;
        if body.get(..4) != Some(CNSP_MAGIC.as_slice()) {
            return Err(ParseError::AssertFail(format!(
                "body opens {:02x?}, not the CNSP stream",
                body.get(..4).unwrap_or_default()
            ))
            .into());
        }
        Ok(body)
    }

    /// The stream version at body `0x04` — `0x450` on v5.x specimens, `0x464`
    /// on v6.1.
    pub fn stream_version(&self) -> Result<u16, Error> {
        let body = self.cnsp()?;
        let bytes = body.get(4..6).ok_or_else(|| {
            ParseError::AssertFail("body ends inside the CNSP header".to_string())
        })?;
        Ok(u16::from_be_bytes(bytes.try_into().unwrap()))
    }

    /// The `(name, variant)` pair from the `Name#Variant` field — for
    /// *Electric Grand 1 CP80*, `("Electric Grand 1", "CP80")`. The variant is
    /// empty when the field carries none.
    pub fn name(&self) -> Result<(String, String), Error> {
        let body = self.cnsp()?;
        let field = body
            .get(NAME_AT..NAME_END)
            .ok_or_else(|| ParseError::AssertFail("body ends inside the name field".to_string()))?;
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        let text = String::from_utf8_lossy(&field[..end]);
        let (name, variant) = text.split_once('#').unwrap_or((&text, ""));
        Ok((name.trim().to_owned(), variant.trim().to_owned()))
    }

    /// The 128-entry per-note key map: one byte per MIDI note, monotonic on
    /// every specimen, `0xFF` for a note the library does not cover. What the
    /// value indexes is not yet mapped; exposed verbatim.
    pub fn key_map(&self) -> Result<&[u8], Error> {
        self.cnsp()?
            .get(KEY_MAP_AT..KEY_MAP_AT + 128)
            .ok_or_else(|| {
                ParseError::AssertFail("body ends inside the key map".to_string()).into()
            })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal body shaped like the real prefix: magic, version, the
    /// `Name#Variant` field, and a key map with the bottom octave unused.
    fn piano(name_field: &[u8]) -> Piano {
        let mut body = vec![0u8; KEY_MAP_AT + 128];
        body[..4].copy_from_slice(CNSP_MAGIC);
        body[4..6].copy_from_slice(&0x0450u16.to_be_bytes());
        body[NAME_AT..NAME_AT + name_field.len()].copy_from_slice(name_field);
        for (i, slot) in body[KEY_MAP_AT..].iter_mut().enumerate() {
            *slot = if i < 12 { 0xFF } else { 0x19 };
        }
        Piano {
            file: Cbin {
                header: Header::new(FORMAT, (0, 0), 530),
                body: RawBody(body),
            },
        }
    }

    #[test]
    fn the_name_field_splits_on_the_separator() {
        let p = piano(b"Electric Grand 1#CP80     ");
        assert_eq!(p.stream_version().unwrap(), 0x450);
        let (name, variant) = p.name().unwrap();
        assert_eq!(name, "Electric Grand 1");
        assert_eq!(variant, "CP80");

        let p = piano(b"Clavinet D6#     ");
        assert_eq!(p.name().unwrap(), ("Clavinet D6".into(), "".into()));
    }

    #[test]
    fn the_key_map_is_read_verbatim() {
        let p = piano(b"X#");
        let map = p.key_map().unwrap();
        assert_eq!(map.len(), 128);
        assert_eq!(map[0], 0xFF);
        assert_eq!(map[127], 0x19);
    }

    #[test]
    fn a_body_without_the_magic_is_refused() {
        let mut p = piano(b"X#");
        p.file.body.0[0] = b'Q';
        assert!(p.name().is_err(), "a non-CNSP body has no name to read");
    }
}
