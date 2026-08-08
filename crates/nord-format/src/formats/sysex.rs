//! Raw SysEx dumps (`.syx`) — how the Lead 1, 2, 2X and 3 ship their banks.
//!
//! The dump is kept verbatim; only the envelope is read. Two envelope shapes
//! exist, split down the model line — confirmed across every corpus dump, not on
//! hardware:
//!
//! * Lead 1/2/2X messages open `F0 33 0F 04`
//! * Lead 3 messages open `F0 33 {01,7F} 09`
//!
//! `0x33` is Clavia's manufacturer id; the fourth byte is the discriminator (the
//! third varies within the Lead 3 dumps). Message-level layout — parameter
//! numbers, bank framing, any checksum — is unmapped.

use crate::error::{Error, ParseError};
use std::io::{Read, Write};

/// The status byte every dump opens with.
pub const SYSEX_START: u8 = 0xf0;
const SYSEX_END: u8 = 0xf7;
const CLAVIA_ID: u8 = 0x33;

/// Which Lead family wrote a dump, by its envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Lead 1, 2 or 2X — the three are indistinguishable from the dump alone.
    Lead2Family,
    Lead3,
    Unknown,
}

/// One `.syx` file, verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sysex {
    pub data: Vec<u8>,
}

impl Sysex {
    pub fn read_from(reader: &mut impl Read) -> Result<Sysex, Error> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        if data.first() != Some(&SYSEX_START) {
            return Err(ParseError::UnknownFileType("not sysex".to_string()).into());
        }
        Ok(Sysex { data })
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), Error> {
        writer.write_all(&self.data)?;
        Ok(())
    }

    pub fn family(&self) -> Family {
        match self.data.as_slice() {
            [SYSEX_START, CLAVIA_ID, _, 0x04, ..] => Family::Lead2Family,
            [SYSEX_START, CLAVIA_ID, _, 0x09, ..] => Family::Lead3,
            _ => Family::Unknown,
        }
    }

    /// The messages in dump order, each spanning its `F0..F7` inclusive. A
    /// truncated final message is yielded as-is rather than dropped.
    pub fn messages(&self) -> impl Iterator<Item = &[u8]> {
        self.data
            .split_inclusive(|&b| b == SYSEX_END)
            .filter(|m| !m.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_lead_envelopes_classify() {
        let lead2 = Sysex {
            data: vec![0xf0, 0x33, 0x0f, 0x04, 0x00, 0xf7],
        };
        assert_eq!(lead2.family(), Family::Lead2Family);

        let lead3 = Sysex {
            data: vec![0xf0, 0x33, 0x7f, 0x09, 0x00, 0xf7],
        };
        assert_eq!(lead3.family(), Family::Lead3);
        assert_eq!(lead3.messages().count(), 1);
    }

    #[test]
    fn messages_split_on_the_end_byte() {
        let two = Sysex {
            data: vec![0xf0, 0x33, 0xf7, 0xf0, 0x44, 0xf7],
        };
        let m: Vec<_> = two.messages().collect();
        assert_eq!(m, [&[0xf0, 0x33, 0xf7][..], &[0xf0, 0x44, 0xf7][..]]);
    }
}
