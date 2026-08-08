//! The Electro 5 set list format (`.ne5t`).
//!
//! A file is a `Cbin<Song>`: the container header carries the slot, the schema version
//! and the generation, and the 18-byte body carries the four programs the song plays.

use std::io::{Read, Seek};

use crate::bank;
use crate::cbin::{self, Cbin, Header};
use crate::error::Error;
use crate::formats::ne5::program;
use crate::types::RangedU16Pair;

pub const FORMAT: &str = "ne5t";
/// Schema versions this build's field offsets have been validated against: 0 is the
/// eight factory demo songs, 1 is everything user-written.
pub const KNOWN_VERSIONS: &[u32] = &[0, 1];
/// The body after the container header: the 8-byte program map and 10 zero bytes.
pub const BODY_LEN: usize = 18;
/// Type-1 file length: 44-byte CBIN header + 18-byte body.
pub const FILE_LEN: usize = 0x2c + BODY_LEN;
pub const PROGRAM_COUNT: usize = 4;
pub const BANK_COUNT: u16 = 4;
pub const SLOT_COUNT: u16 = 50;
/// What a newly authored song is written as; a song read from a file carries whatever
/// version that file held.
pub const DEFAULT_VERSION: u32 = 1;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
pub type Bank = bank::Bank<Cbin<Song>, Location>;

/// The 18-byte body: four 9-bit program references behind a version echo.
///
/// The container header is never transmitted over USB — the device sends only
/// this body — so the version is echoed into bits the wire side can see. ⚠️ It
/// must be the *read* version, never a constant: the eight factory demo songs
/// are version 0, and stamping 1 here silently rewrites them.
#[nord_bits_derive::bitbody(18)]
pub struct Song {
    #[bits(0..=15)]
    version: u16,
    #[bits(16..=24)]
    a: program::Location,
    #[bits(25..=33)]
    b: program::Location,
    #[bits(34..=42)]
    c: program::Location,
    #[bits(43..=51)]
    d: program::Location,
}

impl Song {
    /// The four programs the song plays, in panel order.
    pub fn programs(&self) -> [program::Location; PROGRAM_COUNT] {
        [self.a, self.b, self.c, self.d]
    }

    pub fn get(&self, slot: u16) -> program::Location {
        self.programs()[slot as usize]
    }

    pub fn set(&mut self, slot: u16, location: program::Location) {
        match slot {
            0 => self.a = location,
            1 => self.b = location,
            2 => self.c = location,
            3 => self.d = location,
            _ => panic!("no slot {slot}: a song holds {PROGRAM_COUNT} programs"),
        }
    }
}

/// The set list slot the file claims.
pub fn location(file: &Cbin<Song>) -> Result<Location, Error> {
    program::slot(&file.header)
}

/// A song at `location` playing `programs`, written as schema `version`.
///
/// ⚠️ The version is the caller's to state: the header and the body's echo must agree,
/// and they only do because both are set from this one argument.
pub fn new(
    location: Location,
    version: u32,
    programs: [program::Location; PROGRAM_COUNT],
) -> Cbin<Song> {
    let [a, b, c, d] = programs;
    Cbin {
        header: Header::new(FORMAT, location.inner(), version),
        body: Song {
            raw: [0; BODY_LEN],
            version: version as u16,
            a,
            b,
            c,
            d,
        },
    }
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Song>, Error> {
    let file: Cbin<Song> = cbin::read(reader, FORMAT)?;
    program::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    program::unset_aux(FORMAT, &file.header)?;
    location(&file)?;
    Ok(file)
}

impl bank::Item<Location> for Cbin<Song> {
    fn location(&self) -> Location {
        // Validated at `read_from` and `new`, and only `Header::set_slot` writes it.
        location(self).expect("a song's location is validated at construction")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::Item;
    use crate::error::Error;
    use std::io::Cursor;

    #[test]
    fn read_write_new_song() -> Result<(), Error> {
        let song = new(
            (0, 1).try_into()?,
            DEFAULT_VERSION,
            [
                (1, 2).try_into()?,
                (2, 3).try_into()?,
                (3, 4).try_into()?,
                (4, 5).try_into()?,
            ],
        );

        // Assert song was created with correct values
        assert_eq!(song.location(), (0, 1));
        assert_eq!(song.get(0), (1, 2));
        assert_eq!(song.get(1), (2, 3));
        assert_eq!(song.get(2), (3, 4));
        assert_eq!(song.get(3), (4, 5));

        // Read/Write song to result
        let mut write_result = Vec::new();
        song.write_to(&mut Cursor::new(&mut write_result)).unwrap();

        let result = read_from(&mut Cursor::new(&mut write_result)).unwrap();

        // Assert those values are the same after writing and reading
        assert_eq!(song.location(), result.location());
        assert_eq!(song.get(0), result.get(0));
        assert_eq!(song.get(1), result.get(1));
        assert_eq!(song.get(2), result.get(2));
        assert_eq!(song.get(3), result.get(3));

        Ok(())
    }

    /// A version-0 song must come back out as version 0.
    ///
    /// The eight factory demo songs are version 0 and everything user-written is
    /// version 1. A writer stamping a constant into the header or the map's echo
    /// silently promotes them — a real difference at offset `0x14` and again in the
    /// body, on every one of the eight.
    #[test]
    fn version_survives_a_round_trip() -> Result<(), Error> {
        for version in [0u32, 1] {
            let song = new(
                (0, 5).try_into()?,
                version,
                [
                    (1, 2).try_into()?,
                    (2, 3).try_into()?,
                    (3, 4).try_into()?,
                    (4, 5).try_into()?,
                ],
            );

            let mut bytes = Vec::new();
            song.write_to(&mut Cursor::new(&mut bytes)).unwrap();

            // Header field at 0x14, little-endian.
            assert_eq!(
                u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap()),
                version,
                "header version for v{version}",
            );
            // ...and the echo in the top bits of the big-endian map word at 0x2c, which
            // is the only copy the device ever sees.
            assert_eq!(
                u16::from_be_bytes(bytes[0x2c..0x2e].try_into().unwrap()) as u32,
                version,
                "body version echo for v{version}",
            );

            let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();
            assert_eq!(back.header.version, version);
            assert_eq!(back.get(0), song.get(0));
        }
        Ok(())
    }

    #[test]
    fn update_song_program() -> Result<(), Error> {
        let mut song = new(
            (0, 1).try_into()?,
            DEFAULT_VERSION,
            [
                (1, 2).try_into()?,
                (2, 3).try_into()?,
                (3, 4).try_into()?,
                (4, 5).try_into()?,
            ],
        );

        // Update program 1
        song.set(1, (5, 20).try_into()?);

        // Assert song was updated with correct values
        assert_eq!(song.location(), (0, 1));
        assert_eq!(song.get(0), (1, 2));
        assert_eq!(song.get(1), (5, 20));
        assert_eq!(song.get(2), (3, 4));
        assert_eq!(song.get(3), (4, 5));

        // Read/Write song to result
        let mut write_result = Vec::new();
        song.write_to(&mut Cursor::new(&mut write_result)).unwrap();

        let result = read_from(&mut Cursor::new(&mut write_result)).unwrap();

        // Assert those values are the same after writing and reading
        assert_eq!(song.location(), result.location());
        assert_eq!(song.get(0), result.get(0));
        assert_eq!(song.get(1), result.get(1));
        assert_eq!(song.get(2), result.get(2));
        assert_eq!(song.get(3), result.get(3));

        Ok(())
    }
}
