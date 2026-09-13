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
/// Reads and writes byte-exactly. A read verifies the container checksum, gates
/// on [`KNOWN_VERSIONS`] and the aux word, and validates the slot.
///
/// The container header is never transmitted over USB — the device sends only
/// this body — so the version is echoed into bits the wire side can see. ⚠️ It
/// must be the *read* version, never a constant: the eight factory demo songs
/// are version 0, and stamping 1 here silently rewrites them.
#[nord_bits_derive::bitbody(18)]
pub struct Song {
    #[bits(0..=15)]
    pub version: u16,
    #[bits(16..=24)]
    pub a: program::Location,
    #[bits(25..=33)]
    pub b: program::Location,
    #[bits(34..=42)]
    pub c: program::Location,
    #[bits(43..=51)]
    pub d: program::Location,
}

/// Which of the four programs a song plays — the entries in panel order.
///
/// A song holds exactly these four, so naming one is total: [`Song::get`] and
/// [`Song::set`] cannot be asked for a fifth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
    C,
    D,
}

impl Slot {
    /// The four entries in panel order.
    pub const ALL: [Slot; PROGRAM_COUNT] = [Slot::A, Slot::B, Slot::C, Slot::D];

    /// The entry at a zero-based index, or `None` past the fourth.
    pub fn at(index: usize) -> Option<Slot> {
        Self::ALL.get(index).copied()
    }
}

impl Song {
    /// The four programs the song plays, in panel order.
    pub fn programs(&self) -> [program::Location; PROGRAM_COUNT] {
        [self.a, self.b, self.c, self.d]
    }

    pub fn get(&self, slot: Slot) -> program::Location {
        match slot {
            Slot::A => self.a,
            Slot::B => self.b,
            Slot::C => self.c,
            Slot::D => self.d,
        }
    }

    pub fn set(&mut self, slot: Slot, location: program::Location) {
        *match slot {
            Slot::A => &mut self.a,
            Slot::B => &mut self.b,
            Slot::C => &mut self.c,
            Slot::D => &mut self.d,
        } = location;
    }
}

/// The set list slot the file claims.
pub fn location(file: &Cbin<Song>) -> Result<Location, Error> {
    program::slot(&file.header)
}

/// A song at `location` playing `programs`, written as schema `version`.
///
/// ⚠️ The version is the caller's to state: the header and the body's echo must agree,
/// and they only do because both are set from this one argument. A version
/// [`read_from`] would refuse is refused here too, rather than written and then
/// unreadable.
pub fn new(
    location: Location,
    version: u32,
    programs: [program::Location; PROGRAM_COUNT],
) -> Result<Cbin<Song>, Error> {
    program::known_version(FORMAT, version, KNOWN_VERSIONS)?;
    let echo = u16::try_from(version).map_err(|_| {
        crate::error::ParseError::OutOfBounds {
            value: format!("version {version}"),
            bound: "a version the body's 16-bit echo can hold".into(),
        }
    })?;
    let [a, b, c, d] = programs;
    Ok(Cbin {
        header: Header::new(FORMAT, location.inner(), version),
        body: Song {
            raw: [0; BODY_LEN],
            version: echo,
            a,
            b,
            c,
            d,
        },
    })
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

    fn song_of(programs: [(u16, u16); PROGRAM_COUNT]) -> Result<Cbin<Song>, Error> {
        let mut at = [program::Location::default(); PROGRAM_COUNT];
        for (slot, pair) in at.iter_mut().zip(programs) {
            *slot = pair.try_into()?;
        }
        new((0, 1).try_into()?, DEFAULT_VERSION, at)
    }

    #[test]
    fn a_songs_four_programs_survive_a_round_trip() -> Result<(), Error> {
        let song = song_of([(1, 2), (2, 3), (3, 4), (4, 5)])?;

        assert_eq!(song.location(), (0, 1));
        for (slot, want) in Slot::ALL.into_iter().zip([(1, 2), (2, 3), (3, 4), (4, 5)]) {
            assert_eq!(song.get(slot), want, "{slot:?}");
        }

        let mut bytes = Vec::new();
        song.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();

        assert_eq!(song.location(), back.location());
        for slot in Slot::ALL {
            assert_eq!(song.get(slot), back.get(slot), "{slot:?}");
        }

        Ok(())
    }

    /// The body echoes the header's version, and both come from the one argument — so a
    /// version the read would refuse cannot be written in the first place.
    #[test]
    fn a_version_no_read_accepts_is_not_written() -> Result<(), Error> {
        let at = [program::Location::default(); PROGRAM_COUNT];
        for version in KNOWN_VERSIONS {
            assert!(new((0, 0).try_into()?, *version, at).is_ok(), "v{version}");
        }
        let err = new((0, 0).try_into()?, 2, at).expect_err("version 2 must not be written");
        assert!(
            matches!(
                err,
                Error::Parse(crate::error::ParseError::UnsupportedVersion { version: 2, .. })
            ),
            "refused for the wrong reason: {err}",
        );
        // The echo is 16 bits wide, and `as` would have written 0 for this one.
        assert!(new((0, 0).try_into()?, 0x1_0000, at).is_err());
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
            )?;

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
            assert_eq!(back.get(Slot::A), song.get(Slot::A));
        }
        Ok(())
    }

    /// Writing one entry moves that entry and leaves the other three where they were.
    #[test]
    fn setting_one_entry_leaves_the_others_alone() -> Result<(), Error> {
        let mut song = song_of([(1, 2), (2, 3), (3, 4), (4, 5)])?;

        song.set(Slot::B, (5, 20).try_into()?);

        assert_eq!(song.location(), (0, 1));
        for (slot, want) in Slot::ALL
            .into_iter()
            .zip([(1, 2), (5, 20), (3, 4), (4, 5)])
        {
            assert_eq!(song.get(slot), want, "{slot:?}");
        }

        let mut bytes = Vec::new();
        song.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();

        assert_eq!(song.location(), back.location());
        for slot in Slot::ALL {
            assert_eq!(song.get(slot), back.get(slot), "{slot:?}");
        }

        Ok(())
    }

    /// Four entries, and the index that names them stops there.
    #[test]
    fn a_song_holds_four_entries_and_no_fifth() {
        assert_eq!(Slot::ALL.len(), PROGRAM_COUNT);
        assert_eq!(Slot::at(0), Some(Slot::A));
        assert_eq!(Slot::at(PROGRAM_COUNT - 1), Some(Slot::D));
        assert_eq!(Slot::at(PROGRAM_COUNT), None);
    }
}
