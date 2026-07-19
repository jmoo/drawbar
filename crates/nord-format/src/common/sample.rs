use crate::common::Preamble;

use binrw::{binrw, BinRead, BinReaderExt, BinWriterExt};
use std::fmt::Debug;
use std::{fmt, io};

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
                preamble: Preamble {
                    format: FORMAT.to_string(),
                    version: 0,
                },
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Sample, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Sample { schema })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        match writer.write_be(&mut self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

impl Default for Sample {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("common::Sample")
            .field("schema", &self.schema.preamble.format)
            .finish()
    }
}
