use crate::common;
use crate::common::bank::Location;
use crate::common::crc::{CrcReader, CrcWriter};
use crate::common::Header;
use crate::electro5::BANK_SIZE;
use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};


use std::io;
use std::io::Error;



pub const FORMAT: &str = "ne5t";

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

    // These bytes are part of the crc check so they cannot be skipped with the pad_after directive
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
    pub bank: u16,
    pub slot: u16,
    pub a: (u16, u16),
    pub b: (u16, u16),
    pub c: (u16, u16),
    pub d: (u16, u16),
}

impl Song {
    pub fn new(
        location: (u16, u16),
        a: (u16, u16),
        b: (u16, u16),
        c: (u16, u16),
        d: (u16, u16),
    ) -> Song {
        Song {
            schema: Schema::new(0, 0, 0, 0, 0, 0),
            bank: location.0,
            slot: location.1,
            a,
            b,
            c,
            d,
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Song, Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Song {
            bank: schema.header.bank,
            slot: schema.header.slot,
            a: Location::from_value(BANK_SIZE, schema.a).coords(),
            b: Location::from_value(BANK_SIZE, schema.b).coords(),
            c: Location::from_value(BANK_SIZE, schema.c).coords(),
            d: Location::from_value(BANK_SIZE, schema.d).coords(),
            schema,
        })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), Error> {
        self.schema.header.bank = self.bank;
        self.schema.header.slot = self.slot;
        self.schema.a = Location::from_coords(BANK_SIZE, self.a.0, self.a.1).value();
        self.schema.b = Location::from_coords(BANK_SIZE, self.b.0, self.b.1).value();
        self.schema.c = Location::from_coords(BANK_SIZE, self.c.0, self.c.1).value();
        self.schema.d = Location::from_coords(BANK_SIZE, self.d.0, self.d.1).value();

        match writer.write_be(&mut self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

impl common::song::Song for Song {
    fn location(&self) -> Location {
        Location::from_coords(BANK_SIZE, self.bank, self.slot)
    }

    fn programs(&self) -> Vec<Location> {
        vec![
            Location::from_coords(BANK_SIZE, self.a.0, self.a.1),
            Location::from_coords(BANK_SIZE, self.b.0, self.b.1),
            Location::from_coords(BANK_SIZE, self.c.0, self.c.1),
            Location::from_coords(BANK_SIZE, self.d.0, self.d.1),
        ]
    }
}
