use crate::common::crc::{CrcReader, CrcWriter};
use crate::schema::header::Header;

use binrw::{binrw, BinRead, BinWrite};
use std::fmt;
use std::fmt::Formatter;
use std::io::Write;

#[derive(BinRead, BinWrite, Clone)]
#[brw(little)]
pub struct ProgramMapV1 {
    #[brw(big)]
    map: u64,

    pad_after: [u8; 10],

    #[br(calc = (map >> 39 & 0b111111111) as u16)]
    #[bw(ignore)]
    a: u16,

    #[br(calc = (map >> 30 & 0b111111111) as u16)]
    #[bw(ignore)]
    b: u16,

    #[br(calc = (map >> 21 & 0b111111111) as u16)]
    #[bw(ignore)]
    c: u16,

    #[br(calc = (map >> 12 & 0b111111111) as u16)]
    #[bw(ignore)]
    d: u16,
}

impl ProgramMapV1 {
    pub fn new(a: u16, b: u16, c: u16, d: u16) -> ProgramMapV1 {
        ProgramMapV1 {
            map: (a as u64) << 39 | (b as u64) << 30 | (c as u64) << 21 | (d as u64) << 12,
            pad_after: [0; 10],
            a,
            b,
            c,
            d,
        }
    }

    pub fn a(&self) -> u16 {
        self.a
    }

    pub fn b(&self) -> u16 {
        self.b
    }

    pub fn c(&self) -> u16 {
        self.c
    }

    pub fn d(&self) -> u16 {
        self.d
    }
}

impl fmt::Debug for ProgramMapV1 {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "\n      ProgramMap: (\n        \
              A: {:?}\n        \
              B: {:?}\n        \
              C: {:?}\n        \
              D: {:?}\n      \
           )",
            self.a(),
            self.b(),
            self.c(),
            self.d()
        )
    }
}

#[binrw]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0x3d - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0x3d - 0x2c))]
pub struct SongV1 {
    header: Header,

    #[brw(little)]
    version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    #[br(pad_before = 16)]
    programs: ProgramMapV1,
}

impl SongV1 {
    pub fn new(schema: &str, bank: u16, location: u16) -> SongV1 {
        SongV1 {
            header: Header::new(schema, bank, location),
            version: 0,
            programs: ProgramMapV1::new(0, 0, 0, 0),
        }
    }

    pub fn bank(&self) -> u16 {
        self.header.bank()
    }

    pub fn location(&self) -> u16 {
        self.header.location()
    }

    pub fn schema(&self) -> String {
        self.header.schema()
    }

    pub fn programs(&self) -> ProgramMapV1 {
        self.programs.clone()
    }
}

impl fmt::Debug for SongV1 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "\n  Song: (\n    \
              Schema: {}\n    \
              Bank: {}\n    \
              Location: {}\n    \
              Version: {}\n    \
              Programs: {:?}\n  \
           )\n",
            self.schema(),
            self.bank(),
            self.location(),
            self.version,
            self.programs
        )
    }
}
