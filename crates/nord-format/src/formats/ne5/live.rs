//! The Electro 5 live buffer (`.ne5l`) — the panel as it stands, in three slots.
//!
//! Confirmed on hardware: the live buffer is the `ne5p` program body under another tag.
//! The same panel state read as object class 6 slot `1:1` and as program `5:40` gives
//! byte-identical 121-byte bodies, so this module is [`program::Program`] in the live
//! slot space and every program field applies here unchanged.

use std::io::{Read, Seek};

use crate::cbin::{self, Cbin, Header};
use crate::error::Error;
use crate::formats::ne5::program::{self, Program};
use crate::types::RangedU16Pair;

pub const FORMAT: &str = "ne5l";
/// Schema versions this build's field offsets have been validated against. Every corpus
/// live slot reports 4, as every program does.
pub const KNOWN_VERSIONS: &[u32] = &[4];
/// Type-1 file length — the program length, because it is the program body.
pub const FILE_LEN: usize = program::FILE_LEN;

/// The live buffer is one bank, wire-addressed `0`.
pub const BANK_COUNT: u16 = 1;
/// Three live slots, wire-addressed `0..=2` and shown as `1:1..1:3`.
pub const SLOT_COUNT: u16 = 3;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;

/// The live slot the file claims — the same header word a program reads as a bank and
/// slot, in the three-slot space instead.
pub fn location(file: &Cbin<Program>) -> Result<Location, Error> {
    program::slot(&file.header)
}

/// A default live buffer addressed to `location`.
pub fn new(location: Location) -> Cbin<Program> {
    Cbin {
        header: Header::new(FORMAT, location.inner(), 4),
        body: Program::default(),
    }
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
    let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
    program::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    program::unset_aux(FORMAT, &file.header)?;
    location(&file)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ParseError;
    use std::io::Cursor;

    /// A live slot writes out at the program's length and reads back where it was.
    #[test]
    fn a_live_slot_round_trips_at_the_program_length() {
        let live = new((0, 2).try_into().unwrap());
        let mut bytes = Vec::new();
        live.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);
        assert_eq!(&bytes[0x08..0x0c], FORMAT.as_bytes());

        let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(location(&back).unwrap(), (0, 2));
    }

    /// There are three live slots, and the type will not name a fourth.
    #[test]
    fn the_live_slot_space_stops_at_three() {
        for slot in 0..SLOT_COUNT {
            assert!(Location::try_from((0, slot)).is_ok(), "slot {slot}");
        }
        assert!(Location::try_from((0, SLOT_COUNT)).is_err());
        assert!(Location::try_from((1, 0)).is_err());
    }

    /// The bodies are interchangeable, so only the tag says which format a file is —
    /// and a reader that ignored it would re-emit the file as the other one.
    #[test]
    fn a_program_is_not_accepted_as_a_live_slot() {
        let program = program::new((0, 1).try_into().unwrap());
        let mut bytes = Vec::new();
        program.write_to(&mut Cursor::new(&mut bytes)).unwrap();

        let err = read_from(&mut Cursor::new(&mut bytes))
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
        new((0, 1).try_into().unwrap())
            .write_to(&mut Cursor::new(&mut live))
            .unwrap();
        let err = program::read_from(&mut Cursor::new(&mut live))
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
        new((0, 1).try_into().unwrap())
            .write_to(&mut Cursor::new(&mut live))
            .unwrap();

        let mut program = Vec::new();
        program::new((0, 1).try_into().unwrap())
            .write_to(&mut Cursor::new(&mut program))
            .unwrap();

        let differing: Vec<usize> = (0..live.len()).filter(|&i| live[i] != program[i]).collect();
        assert_eq!(differing, vec![0x0b], "{differing:x?}");
    }
}
