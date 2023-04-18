use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use crate::common::crc::{CrcReader, CrcWriter};
use std::{fmt, io};
use std::fmt::Debug;
use crate::common::Preamble;

pub const FORMAT: &str = "nsmp";

#[binrw]
#[brw(assert(preamble.format == FORMAT))]
struct Schema {
    preamble: Preamble,
}

pub struct Sample {
    schema: Schema,
}

impl Sample {
    pub fn new() -> Sample {
        Sample {
            schema: Schema {
                preamble: Preamble { format: FORMAT.to_string(), version: 0 }
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Sample, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Sample {
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

impl Debug for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sample")
            .finish()
    }
}
