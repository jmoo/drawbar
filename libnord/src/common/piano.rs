use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt, until_eof};
use crate::crc::{CrcReader, CrcWriter};
use std::{fmt, io};
use std::fmt::Debug;
use crate::common::Header;

pub const FORMAT: &str = "npno";

#[binrw]
#[brw(assert(header.preamble.format == FORMAT))]
struct Schema {
    header: Header,

    // #[br(parse_with = until_eof)]
    // body: Vec<u8>
}

pub struct Piano {
    schema: Schema,
}

impl Piano {
    pub fn new() -> Piano {
        Piano {
            schema: Schema {
                header: Header::new(FORMAT, 0, 0),
                // body: Vec::new(),
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

impl Debug for Piano {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("common::Piano")
            .field("schema", &self.schema.header.preamble.format)
            .finish()
    }
}
