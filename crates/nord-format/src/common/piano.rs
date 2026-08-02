use crate::common;

use crate::error::Error;
use crate::types::RangedU16Pair;
use binrw::{binrw, BinRead, BinWriterExt};
use std::fmt;
use std::fmt::Debug;
use std::io::{Read, Seek, Write};

pub const FORMAT: &str = "npno";
pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
pub type Header = common::Header<Location>;

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
                header: Header::new(0, FORMAT, (0, 0).try_into().unwrap()),
            },
        }
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Piano, Error> {
        let schema = Schema::read_be(reader)?;
        Ok(Piano { schema })
    }

    pub fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<(), Error> {
        writer.write_be(&self.schema)?;
        Ok(())
    }
}

impl Default for Piano {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for Piano {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("common::Piano")
            .field("schema", &self.schema.header.preamble.format)
            .finish()
    }
}
