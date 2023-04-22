use crate::common;
use crate::common::{bank, Header};
use crate::crc::{CrcReader, CrcWriter};
use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use std::fmt::Debug;
use std::io;
use modular_bitfield::prelude::*;

pub const FORMAT: &str = "ne5p";

pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Coordinates = bank::Coordinates<BANK_COUNT, SLOT_COUNT>;
pub type Bank = bank::Bank<BANK_COUNT, SLOT_COUNT, Program>;

// Program naming convention:
// abcdabcdz e y
// lower - upper - split - - z  y
// ---------------------
// a = part (n, o, p, s)
// b = sustain (0,1)
// c = control (0,1)
// d = octave (-1,0,1)
// ---------------------
// z = transpose (-6..6)
// e = split (0:off, 1:c3, 2:f3, 3:c4, 4:f4, 5c5, 6:f5, 7:upper)
// y - part volume (-50..50)

// 31 - 32
#[derive(Copy, Clone)]
#[bitfield]
#[binrw]
#[br(map = Self::from_bytes)]
#[bw(map = |&x| Self::into_bytes(x))]
pub struct PanelSettings31 {
    // 11111
    pub b16: B1,
    pub b15: B1,
    pub b14: B1,
    pub b13: B1,
    pub b12: B1,

    // split volume
    pub b11: B1,
    pub b10: B1,
    pub b9: B1,
    pub b8: B1,
    pub b7: B1,
    pub b6: B1,

    // ?????
    pub b5: B1,

    // transpose (0 to 12  big endian = -6 to -6 half steps transposition)
    // 0111 1100  12
    // 0111 10111 11
    pub b4: B1,
    pub b3: B1,
    pub b2: B1,
    pub b1: B1,
}

// 30
#[derive(Copy, Clone)]
#[bitfield]
#[binrw]
#[br(map = Self::from_bytes)]
#[bw(map = |&x| Self::into_bytes(x))]
pub struct PanelSettings30 {
    // split enabled ????
    pub b8: B1,

    // keyboard split
    // 0011  0  off (confirmed)
    // 1000  1  c3 (confirmed)
    // 1001  2  f3
    // 1010  3  c4
    // 1011  4
    // 1100  5  c5 (confirmed)
    // 1101  6  f5 (confirmed)
    // 1110  7  upper (confirmed)
    pub b7: B1,
    pub b6: B1,
    pub b5: B1,
    pub b4: B1,

    // ???
    pub b3: B1,

    // right control
    pub b2: B1,

    // left control
    pub b1: B1,
}

// 2e-2f
#[derive(Copy, Clone)]
#[bitfield]
#[binrw]
#[br(map = Self::from_bytes)]
#[bw(map = |&x| Self::into_bytes(x))]
pub struct PanelSettings2e {
    // right sustain
    pub b16: B1,

    // left sustain
    pub b15: B1,

    // left and right octave
    // 0111 0111   0 <-> 0
    // 1000 0111   1 <-> 0
    // 0111 1000   0 <-> 1

    // 0110  -1 ????
    // 0111   0
    // 1000   1

    // right octave shift
    pub b14: B1,
    pub b13: B1,
    pub b12: B1,
    pub b11: B1,

    // left octave shift
    pub b10: B1,
    pub b9: B1,
    pub b8: B1,
    pub b7: B1,

    // 000 -> right organ
    // 001 -> right piano
    // 010 -> right sample
    pub b6: B1,
    pub b5: B1,
    pub b4: B1,

    // 000 -> left organ
    // 001 -> left piano
    // 010 -> left sample
    pub b3: B1,
    pub b2: B1,
    pub b1: B1,
}

#[binrw]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0xa5 - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0xa5 - 0x2c))]
pub struct Schema {
    pub header: Header,

    pub version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    #[brw(big, pad_before = 16)]
    zeros: u8, // 0x2c

    // version
    four: u8, // 0x2d

    #[brw(big)]
    pub panel_settings_2e: u16, // 0x2e - 2f

    #[brw(big)]
    pub panel_settings_30: u8, // 0x30

    pub panel_settings_31: u16, // 0x31 - 32

    body: [u8; (0xa5 - 0x33) as usize],

    #[br(calc = ((panel_settings_2e & 0b1110000000000000) >> 13) as u8)]
    #[bw(ignore)]
    pub left_part: u8,

    #[br(calc = ((panel_settings_2e & 0b0001110000000000) >> 10) as u8)]
    #[bw(ignore)]
    pub right_part: u8,

    #[br(calc = ((panel_settings_2e & 0b0000001111000000) >> 6) as u8)]
    #[bw(ignore)]
    pub left_octave_shift: u8,

    #[br(calc = ((panel_settings_2e & 0b0000000000111100) >> 2) as u8)]
    #[bw(ignore)]
    pub right_octave_shift: u8,

    #[br(calc = (panel_settings_2e & 0b0000000000000010 >> 1) != 0)]
    #[bw(ignore)]
    pub left_sustain: bool,

    #[br(calc = (panel_settings_2e & 0b0000000000000001) != 0)]
    #[bw(ignore)]
    pub right_sustain: bool,
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
                version: 4,
                body: [0; (0xa5 - 0x33) as usize],
                four: 4,
                zeros: 0,
                panel_settings_2e: 0,
                panel_settings_30: 0,
                panel_settings_31: 0,
                left_octave_shift: 0,
                right_octave_shift: 0,
                left_part: 0,
                right_part: 0,
                left_sustain: false,
                right_sustain: false,
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

    pub fn left_part(&self) -> &str {
        match self.schema.left_part {
            0 => "organ",
            1 => "piano",
            2 => "sample",
            _ => "unknown",
        }
    }

    pub fn right_part(&self) -> &str {
        match self.schema.right_part {
            0 => "organ",
            1 => "piano",
            2 => "sample",
            _ => "unknown",
        }
    }

    pub fn left_octave_shift(&self) -> u8 {
        self.schema.left_octave_shift
        // match self.schema.left_octave_shift {
        //     0 => -1,
        //     1 => 0,
        //     2 => 1,
        //     _ => 0,
        // }
    }

    pub fn right_octave_shift(&self) -> u8 {
        self.schema.right_octave_shift
        // match self.schema.right_octave_shift {
        //     0 => -1,
        //     1 => 0,
        //     2 => 1,
        //     _ => 0,
        // }
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
            //.field("body", &self.schema.body)
            .field("left_part", &self.left_part())
            .field("right_part", &self.right_part())
            .field("left_octave_shift", &self.left_octave_shift())
            .field("right_octave_shift", &self.right_octave_shift())
            .field("left_sustain", &self.schema.left_sustain)
            .field("right_sustain", &self.schema.right_sustain)
            .finish()
    }
}
