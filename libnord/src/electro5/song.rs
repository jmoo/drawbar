use std::fmt::Debug;
use crate::{common, NordResult};
use crate::common::bank::{Item};
use crate::common::bank;
use crate::common::crc::{CrcReader, CrcWriter};
use crate::common::Header;
use crate::common::Error;
use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};

use std::io;
use crate::electro5::program;
use crate::electro5::program::BANK_SIZE as PROGRAM_BANK_SIZE;
use crate::electro5::program::BANK_COUNT as PROGRAM_BANK_COUNT;

pub const FORMAT: &str = "ne5t";

pub const BANK_COUNT: u16 = 4;
pub const BANK_SIZE: u16 = 50;

pub type Coordinates = bank::Coordinates<BANK_COUNT, BANK_SIZE>;

#[binrw]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0x3d - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0x3d - 0x2c))]
struct Schema {
    pub header: Header,

    pub version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    #[brw(big, pad_before = 16)]
    #[bw(calc = ((*a as u64) << 39 | (*b as u64) << 30 | (*c as u64) << 21 | (*d as u64) << 12) | 0x01000000000000)]
    map: u64,

    /// These bytes are part of the crc check so they cannot be skipped with the pad_after directive
    #[bw(calc = [0; 10])]
    pad: [u8; 10],

    #[br(calc = (map >> 39 & 0b111111111) as u16)]
    #[bw(ignore)]
    pub a: u16,

    #[br(calc = (map >> 30 & 0b111111111) as u16)]
    #[bw(ignore)]
    pub b: u16,

    #[br(calc = (map >> 21 & 0b111111111) as u16)]
    #[bw(ignore)]
    pub c: u16,

    #[br(calc = (map >> 12 & 0b111111111) as u16)]
    #[bw(ignore)]
    pub d: u16,
}

impl Schema {
    pub fn new(bank: u16, location: u16, a: u16, b: u16, c: u16, d: u16) -> Schema {
        Schema {
            header: Header::new(FORMAT, bank, location),
            version: 1,
            a,
            b,
            c,
            d,
        }
    }
}

pub struct Song {
    schema: Schema,
    coordinates: Coordinates,
    programs: [program::Coordinates; 4],
}

impl Song {
    pub fn new(
        coords: Coordinates,
        a: program::Coordinates,
        b: program::Coordinates,
        c: program::Coordinates,
        d: program::Coordinates,
    ) -> Song {
        Song {
            schema: Schema::new(0, 0, 0, 0, 0, 0),
            coordinates: coords,
            programs: [a, b, c, d],
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Song, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Song {
            coordinates: Coordinates::from_coords((schema.header.bank, schema.header.slot)),
            programs: [
                program::Coordinates::from_value(schema.a),
                program::Coordinates::from_value(schema.b),
                program::Coordinates::from_value(schema.c),
                program::Coordinates::from_value(schema.d),
            ],
            schema,
        })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        self.schema.header.bank = self.coordinates.bank();
        self.schema.header.slot = self.coordinates.slot();
        self.schema.a = self.programs[0].value();
        self.schema.b = self.programs[1].value();
        self.schema.c = self.programs[2].value();
        self.schema.d = self.programs[3].value();

        match writer.write_be(&mut self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

impl bank::Item<BANK_COUNT, BANK_SIZE> for Song {
    fn location(&self) -> Coordinates {
        self.coordinates
    }

    fn set_location(&mut self, location: Coordinates) -> () {
        self.coordinates = location;
    }
}

impl common::song::Song<PROGRAM_BANK_COUNT, PROGRAM_BANK_SIZE> for Song {
    fn get(&self, slot: u16) -> program::Coordinates {
       self.programs[slot as usize]
    }

    fn set(&mut self, slot: u16, coords: program::Coordinates) -> () {
        self.programs[slot as usize] = coords;
    }
}

impl Debug for Song {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Song")
            .field("location", &self.coordinates)
            .field("programs", &self.programs)
            .finish()
    }
}
