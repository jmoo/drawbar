use binrw::{binrw, BinRead, BinReaderExt, BinWriterExt};
use std::io;

use crate::common;
use crate::common::bank::Item;
use crate::crc::{CrcReader, CrcWriter};

use crate::common::bank;

use crate::electro5::program;

use crate::types::RangedU16Pair;

pub const FORMAT: &str = "ne5t";
pub const PROGRAM_COUNT: usize = 4;
pub const BANK_COUNT: u16 = 4;
pub const SLOT_COUNT: u16 = 50;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
pub type Header = common::Header<Location>;
pub type Bank = bank::Bank<Song, Location>;
pub type Song = common::song::Song<PROGRAM_COUNT, Location, program::Location>;

#[binrw]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0x3d - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0x3d - 0x2c))]
struct Schema {
    pub header: Header,

    pub version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    #[brw(big, pad_before = 16)]
    #[bw(calc = (
    ((* a).as_u16() as u64) << 39
    | ((* b).as_u16() as u64) << 30
    | ((* c).as_u16() as u64) << 21
    | ((* d).as_u16() as u64) << 12)
    | 0x01000000000000
    )]
    map: u64,

    /// These bytes are part of the crc check so they cannot be skipped with the pad_after directive
    #[bw(calc = [0; 10])]
    pad: [u8; 10],

    #[br(try_calc = ((map >> 39 & 0b111111111) as u16).try_into())]
    #[bw(ignore)]
    pub a: program::Location,

    #[br(try_calc = ((map >> 30 & 0b111111111) as u16).try_into())]
    #[bw(ignore)]
    pub b: program::Location,

    #[br(try_calc = ((map >> 21 & 0b111111111) as u16).try_into())]
    #[bw(ignore)]
    pub c: program::Location,

    #[br(try_calc = ((map >> 12 & 0b111111111) as u16).try_into())]
    #[bw(ignore)]
    pub d: program::Location,
}

impl Schema {
    pub fn new(
        location: Location,
        a: program::Location,
        b: program::Location,
        c: program::Location,
        d: program::Location,
    ) -> Schema {
        Schema {
            header: Header::new(1, FORMAT, location),
            version: 1,
            a,
            b,
            c,
            d,
        }
    }
}

impl Song {
    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Song, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Song::new(
            schema.header.location,
            [schema.a, schema.b, schema.c, schema.d],
        ))
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        let mut schema = Schema::new(
            self.location(),
            self.programs()[0],
            self.programs()[1],
            self.programs()[2],
            self.programs()[3],
        );

        match writer.write_be(&mut schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Song;
    use crate::common::bank::Item;
    use crate::error::Error;
    use std::io::Cursor;

    #[test]
    fn read_write_new_song() -> Result<(), Error> {
        let mut song = Song::new(
            (0, 1).try_into()?,
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

        let result = Song::read_from(&mut Cursor::new(&mut write_result)).unwrap();

        // Assert those values are the same after writing and reading
        assert_eq!(song.location(), result.location());
        assert_eq!(song.get(0), result.get(0));
        assert_eq!(song.get(1), result.get(1));
        assert_eq!(song.get(2), result.get(2));
        assert_eq!(song.get(3), result.get(3));

        Ok(())
    }

    #[test]
    fn update_song_program() -> Result<(), Error> {
        let mut song = Song::new(
            (0, 1).try_into()?,
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

        let result = Song::read_from(&mut Cursor::new(&mut write_result)).unwrap();

        // Assert those values are the same after writing and reading
        assert_eq!(song.location(), result.location());
        assert_eq!(song.get(0), result.get(0));
        assert_eq!(song.get(1), result.get(1));
        assert_eq!(song.get(2), result.get(2));
        assert_eq!(song.get(3), result.get(3));

        Ok(())
    }
}
