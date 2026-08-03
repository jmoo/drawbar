//! The Electro 5 live buffer (`.ne5l`) — the panel as it stands, in three slots.
//!
//! Confirmed on hardware: the live buffer is the `ne5p` program body under another tag.
//! The same panel state read as object class 6 slot `1:1` and as program `5:40` gives
//! byte-identical 121-byte bodies, so this module is [`program::Schema`] in the live slot
//! space and every program field applies here unchanged.

use binrw::{BinRead, BinWriterExt};
use std::io::{Read, Seek, Write};

use crate::common;
use crate::common::bank;
use crate::electro5::program;
use crate::error::{Error, ParseError};
use crate::types::RangedU16Pair;

pub const FORMAT: &str = "ne5l";
/// Schema versions this build's field offsets have been validated against. Every corpus
/// live slot reports 4, as every program does.
pub const KNOWN_VERSIONS: &[u32] = &[4];
/// Total file length: 44-byte CBIN header + 121-byte body — the program length, because
/// it is the program body.
pub const FILE_LEN: usize = program::FILE_LEN;

/// Highest bank: the live buffer is one bank, wire-addressed `0`.
pub const BANK_MAX: u16 = 0;
/// Highest slot: three live slots, wire-addressed `0..=2` and shown as `1:1..1:3`.
pub const SLOT_MAX: u16 = 2;

pub type Location = RangedU16Pair<BANK_MAX, SLOT_MAX>;
pub type Header = common::Header<Location>;
pub type Schema = program::Schema<Location>;

/// One of the three live slots.
#[derive(Debug)]
pub struct Live {
    pub schema: Schema,
    name: Option<String>,
}

impl Live {
    pub fn new(location: Location) -> Live {
        Live {
            name: None,
            schema: Schema::new(FORMAT, location),
        }
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Live, Error> {
        let schema = Schema::read_be(reader)?;
        program::check_tag(FORMAT, &schema)?;
        if !KNOWN_VERSIONS.contains(&schema.version) {
            return Err(ParseError::UnsupportedVersion {
                format: FORMAT,
                version: schema.version,
                supported: KNOWN_VERSIONS,
            }
            .into());
        }

        Ok(Live { name: None, schema })
    }

    pub fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<(), Error> {
        writer.write_be(&self.schema)?;
        Ok(())
    }
}

impl bank::Item<Location> for Live {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn set_name(&mut self, name: String) {
        self.name = Some(name);
    }

    fn location(&self) -> Location {
        self.schema.header.location
    }

    fn set_location(&mut self, location: Location) {
        self.schema.header.location = location;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::bank::Item;
    use std::io::Cursor;

    /// A live slot writes out at the program's length and reads back where it was.
    #[test]
    fn a_live_slot_round_trips_at_the_program_length() {
        let live = Live::new((0, 2).try_into().unwrap());
        let mut bytes = Vec::new();
        live.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);
        assert_eq!(&bytes[0x08..0x0c], FORMAT.as_bytes());

        let back = Live::read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(back.location(), (0, 2));
    }

    /// There are three live slots, and the type will not name a fourth.
    #[test]
    fn the_live_slot_space_stops_at_three() {
        for slot in 0..=SLOT_MAX {
            assert!(Location::try_from((0, slot)).is_ok(), "slot {slot}");
        }
        assert!(Location::try_from((0, SLOT_MAX + 1)).is_err());
        assert!(Location::try_from((1, 0)).is_err());
    }

    /// The bodies are interchangeable, so only the tag says which format a file is —
    /// and a reader that ignored it would re-emit the file as the other one.
    #[test]
    fn a_program_is_not_accepted_as_a_live_slot() {
        let program = program::Program::new((0, 1).try_into().unwrap());
        let mut bytes = Vec::new();
        program.write_to(&mut Cursor::new(&mut bytes)).unwrap();

        let err = Live::read_from(&mut Cursor::new(&mut bytes))
            .expect_err("a ne5p file must not read as a live slot");
        assert!(
            matches!(
                err,
                Error::Parse(ParseError::WrongFormat {
                    expected: FORMAT,
                    ..
                })
            ),
            "refused for the wrong reason: {err}",
        );

        let mut live = Vec::new();
        Live::new((0, 1).try_into().unwrap())
            .write_to(&mut Cursor::new(&mut live))
            .unwrap();
        let err = program::Program::read_from(&mut Cursor::new(&mut live))
            .expect_err("a ne5l file must not read as a program");
        assert!(
            matches!(
                err,
                Error::Parse(ParseError::WrongFormat {
                    expected: program::FORMAT,
                    ..
                })
            ),
            "refused for the wrong reason: {err}",
        );
    }

    /// The two formats are one body: a live slot and a program at the same location
    /// differ in the one tag byte that spells them apart, and nothing else.
    #[test]
    fn a_live_body_is_a_program_body() {
        let mut live = Vec::new();
        Live::new((0, 1).try_into().unwrap())
            .write_to(&mut Cursor::new(&mut live))
            .unwrap();

        let mut program = Vec::new();
        program::Program::new((0, 1).try_into().unwrap())
            .write_to(&mut Cursor::new(&mut program))
            .unwrap();

        let differing: Vec<usize> = (0..live.len()).filter(|&i| live[i] != program[i]).collect();
        assert_eq!(differing, vec![0x0b], "{differing:x?}");
    }
}
