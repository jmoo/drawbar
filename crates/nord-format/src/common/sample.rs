use crate::common::Preamble;

use crate::error::Error;
use binrw::{binrw, BinRead, BinWriterExt};
use std::fmt;
use std::fmt::Debug;
use std::io::{Read, Seek, Write};

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

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Sample, Error> {
        let schema = Schema::read_be(reader)?;
        Ok(Sample { schema })
    }

    pub fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<(), Error> {
        writer.write_be(&self.schema)?;
        Ok(())
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
