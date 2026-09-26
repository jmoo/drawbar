//! Raw SysEx dumps (`.syx`), the form in which the Lead 1, 2, 2X and 3 ship their
//! banks.
//!
//! The dump is kept verbatim; only the envelope is read. There are two envelope
//! shapes, one per model line:
//!
//! * Lead 1/2/2X messages open `F0 33 0F 04`
//! * Lead 3 messages open `F0 33 {01,7F} 09`
//!
//! Inferred from specimens; not confirmed on hardware.
//!
//! `0x33` is Clavia's manufacturer id; the fourth byte is the discriminator (the
//! third varies within the Lead 3 dumps). The message layout (parameter numbers,
//! bank framing, any checksum) is unmapped.

use crate::error::{Error, ParseError};
use std::io::{Read, Write};

/// The status byte every dump opens with.
pub const SYSEX_START: u8 = 0xf0;
const SYSEX_END: u8 = 0xf7;
const CLAVIA_ID: u8 = 0x33;

/// Which Lead family wrote a dump, by its envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Lead 1, 2 or 2X; the dump alone cannot tell the three apart.
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
    /// Verifies the envelope and nothing else: a dump opens with `F0` and ends
    /// with `F7`.
    pub fn read_from(reader: &mut impl Read) -> Result<Sysex, Error> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        if data.first() != Some(&SYSEX_START) || data.last() != Some(&SYSEX_END) {
            return Err(ParseError::UnknownFileType(
                "a SysEx dump opens with F0 and ends with F7".to_string(),
            )
            .into());
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

    /// The dump in order, split after each `F7`. A read guarantees the first byte
    /// is `F0` and the last `F7`, so no piece is unterminated; interior `F0` bytes
    /// are not validated, so a piece holding two of them is yielded as one message.
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
    fn a_dump_is_kept_verbatim() {
        let bytes = [0xf0u8, 0x33, 0x0f, 0x04, 0x01, 0x02, 0xf7];
        let dump = Sysex::read_from(&mut bytes.as_slice()).unwrap();
        assert_eq!(dump.data, bytes);
        let mut out = Vec::new();
        dump.write_to(&mut out).unwrap();
        assert_eq!(out, bytes);
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(
            Sysex::read_from(&mut b"".as_slice()).is_err(),
            "an empty file"
        );
        assert!(
            Sysex::read_from(&mut [0x00u8].as_slice()).is_err(),
            "no F0 to open with"
        );
        assert!(
            Sysex::read_from(&mut b"MThd".as_slice()).is_err(),
            "a MIDI file"
        );
        assert!(
            Sysex::read_from(&mut [0xf0u8, 0x33, 0x0f, 0x04, 0x01].as_slice()).is_err(),
            "an envelope with no F7 to close it"
        );
    }

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
    }

    #[test]
    fn another_manufacturers_dump_classifies_as_unknown() {
        let other = Sysex {
            data: vec![0xf0, 0x00, 0x0f, 0x04, 0x00, 0xf7],
        };
        assert_eq!(other.family(), Family::Unknown);
    }

    #[test]
    fn messages_split_on_the_end_byte() {
        let one = Sysex {
            data: vec![0xf0, 0x33, 0x7f, 0x09, 0x00, 0xf7],
        };
        assert_eq!(one.messages().count(), 1);

        let two = Sysex {
            data: vec![0xf0, 0x33, 0xf7, 0xf0, 0x44, 0xf7],
        };
        let m: Vec<_> = two.messages().collect();
        assert_eq!(m, [&[0xf0, 0x33, 0xf7][..], &[0xf0, 0x44, 0xf7][..]]);
    }
}
