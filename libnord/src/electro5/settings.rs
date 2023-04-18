use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use crate::common::crc::{CrcReader, CrcWriter};
use std::{fmt, io};
use std::fmt::Debug;
use crate::common;
use crate::common::Header;

pub const FORMAT: &str = "ne5s";

#[binrw]
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

impl Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schema")
            .field("header", &self.header)
            .field("version", &self.version)
            .field("body", &self.body)
            .finish()
    }
}

pub struct Settings {
    schema: Schema,
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            schema: Schema {
                header: Header::new(FORMAT, 0, 0),
                body: [0; (0x4e - 0x2c) as usize],
                version: 0
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Settings, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Settings {
            schema,
        })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        match writer.write_be(&mut self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

impl common::settings::Settings for Settings {}

impl Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("schema", &self.schema)
            .finish()
    }
}