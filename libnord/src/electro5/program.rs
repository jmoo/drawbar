use crate::common;
use crate::common::{bank, Header};
use crate::crc::{CrcReader, CrcWriter};
use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use std::fmt::Debug;
use std::io;

pub const FORMAT: &str = "ne5p";

pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Coordinates = bank::Coordinates<BANK_COUNT, SLOT_COUNT>;
pub type Bank = bank::Bank<BANK_COUNT, SLOT_COUNT, Program>;

#[binrw]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0xa5 - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0xa5 - 0x2c))]
pub struct Schema {
    pub header: Header,

    pub version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    #[brw(big, pad_before = 16)]
    body: [u8; (0xa5 - 0x2c) as usize],
}

pub struct Program {
    schema: Schema,
    coordinates: Coordinates,
    name: Option<String>,
}

impl Program {
    pub fn new(location: Coordinates) -> Program {
        Program {
            coordinates: location,
            name: None,
            schema: Schema {
                header: Header::new(FORMAT, location.bank(), location.slot()),
                version: 1,
                body: [0; (0xa5 - 0x2c) as usize],
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Program, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Program {
            coordinates: Coordinates::from_coords((schema.header.bank, schema.header.slot)),
            name: None,
            schema,
        })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        self.schema.header.bank = self.coordinates.bank();
        self.schema.header.slot = self.coordinates.slot();

        match writer.write_be(&mut self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

impl bank::Item<BANK_COUNT, SLOT_COUNT> for Program {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn set_name(&mut self, name: String) -> () {
        self.name = Some(name);
    }

    fn location(&self) -> Coordinates {
        self.coordinates
    }

    fn set_location(&mut self, location: Coordinates) -> () {
        self.coordinates = location;
    }
}

impl common::program::Program for Program {}

impl Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("electro5::Program")
            .field("schema", &self.schema.header.preamble.format)
            .field("coordinates", &self.coordinates)
            .field("name", &self.name)
            .field("body", &self.schema.body)
            .finish()
    }
}
