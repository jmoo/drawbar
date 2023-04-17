use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use crate::common::crc::{CrcReader, CrcWriter};
use std::{fmt, io};
use crate::common::Header;

pub const FORMAT: &str = "npno";

#[binrw]
#[brw(assert(header.preamble.format == FORMAT))]
struct Schema {
    header: Header,
}

pub struct Piano {
    schema: Schema,
}

impl Piano {
    pub fn new() -> Piano {
        Piano {
            schema: Schema {
                header: Header::new(FORMAT, 0, 0),
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Piano, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Piano {
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
