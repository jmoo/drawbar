use crate::common;
use crate::crc::{CrcReader, CrcWriter};
use crate::types::RangedU16Pair;
use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use std::fmt::Debug;
use std::io;

pub const FORMAT: &str = "ne5s";
pub type Location = RangedU16Pair<0, 0>;
pub type Header = common::Header<Location>;

#[binrw]
#[derive(Debug)]
#[brw(assert(header.preamble.format == FORMAT))]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0x4e - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0x4e - 0x2c))]
struct Schema {
    header: Header,

    pub version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    #[brw(big, pad_before = 16)]
    body: [u8; (0x4e - 0x2c) as usize],
}

#[derive(Debug)]
pub struct Settings {
    schema: Schema,
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            schema: Schema {
                header: Header::new(1, FORMAT, (0, 0).try_into().unwrap()),
                body: [0; (0x4e - 0x2c) as usize],
                version: 0,
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Settings, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Settings { schema })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        match writer.write_be(&mut self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

impl common::settings::Settings for Settings {}
