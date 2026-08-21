//! Standard MIDI files (`.mid`) — carriers for the Lead SysEx banks.
//!
//! Kept verbatim: the interesting bytes are the embedded SysEx messages, and
//! extracting them means walking MTrk events, which is unimplemented. ⚠️ The
//! `.mid` and `.syx` editions of a bank are not interchangeable on every model:
//! the Lead 2X pairs carry the same messages in the same order, the Lead 3 pairs
//! the same set in a different order.

use crate::error::{Error, ParseError};
use std::io::{Read, Write};

pub const MAGIC: &[u8; 4] = b"MThd";

/// One `.mid` file, verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Midi {
    pub data: Vec<u8>,
}

impl Midi {
    pub fn read_from(reader: &mut impl Read) -> Result<Midi, Error> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        if data.len() < 4 || &data[0..4] != MAGIC {
            return Err(ParseError::UnknownFileType("not a MIDI file".to_string()).into());
        }
        Ok(Midi { data })
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), Error> {
        writer.write_all(&self.data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_kept_verbatim() {
        let bytes = [b"MThd".as_slice(), &[0, 0, 0, 6, 0, 0, 0, 1, 0, 96]].concat();
        let midi = Midi::read_from(&mut bytes.as_slice()).unwrap();
        assert_eq!(midi.data, bytes);
        let mut out = Vec::new();
        midi.write_to(&mut out).unwrap();
        assert_eq!(out, bytes);
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(Midi::read_from(&mut b"MTrk....".as_slice()).is_err());
        assert!(
            Midi::read_from(&mut b"MT".as_slice()).is_err(),
            "shorter than the magic"
        );
    }
}
