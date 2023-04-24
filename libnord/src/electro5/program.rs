use crate::common;
use crate::common::{bank, Header};
use crate::crc::{CrcReader, CrcWriter};
use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use std::fmt::Debug;
use std::io;

pub const FORMAT: &str = "ne5p";

pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Coordinates = bank::Coordinates<BANK_COUNT, SLOT_COUNT>;
pub type Bank = bank::Bank<BANK_COUNT, SLOT_COUNT, Program>;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Instrument {
    Organ,
    Piano,
    Sample
}

impl TryFrom<u8> for Instrument {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Instrument, Self::Error> {
        match value {
            0 => Ok(Instrument::Organ),
            1 => Ok(Instrument::Piano),
            2 => Ok(Instrument::Sample),
            _ => Err(&"Value is out of range for instrument"),
        }
    }
}

impl Instrument {
    fn as_str(&self) -> &'static str {
        match self {
            Instrument::Organ => "organ",
            Instrument::Piano => "piano",
            Instrument::Sample => "sample",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SplitPoint {
    C3,
    F3,
    C4,
    F4,
    C5,
    F5,
    Upper,
    Lower
}

impl TryFrom<u8> for SplitPoint {
    type Error =  &'static str;

    fn try_from(value: u8) -> Result<SplitPoint, Self::Error> {
        match value {
            0 => Ok(SplitPoint::C3),
            1 => Ok(SplitPoint::F3),
            2 => Ok(SplitPoint::C4),
            3 => Ok(SplitPoint::F4),
            4 => Ok(SplitPoint::C5),
            5 => Ok(SplitPoint::F5),
            6 => Ok(SplitPoint::Upper),
            7 => Ok(SplitPoint::Lower),
            _ => Err(&"Value is out of range for split point")
        }
    }
}

impl SplitPoint {
    fn as_str(&self) -> &'static str {
        match self {
            SplitPoint::C3 => "c3",
            SplitPoint::F3 => "f3",
            SplitPoint::C4 => "c4",
            SplitPoint::F4 => "f4",
            SplitPoint::C5 => "c5",
            SplitPoint::F5 => "f5",
            SplitPoint::Upper => "upper",
            SplitPoint::Lower => "lower",
        }
    }
}

// 0x2e-0x32
#[binrw]
pub struct CenterPanel {
    // 0x2e-0x2f
    #[brw(big)]
    #[bw(calc =
    (*left_part as u16) << 13
    | (*right_part as u16) << 10
    | (((*left_octave_shift + 7) as u16) << 6)
    | (((*right_octave_shift + 7) as u16) << 2)
    | ((*left_sustain as u16) << 1)
    | (*right_sustain as u16)
    )]
    settings: u16,

    #[br(try_calc = (((settings & 0b1110000000000000) >> 13) as u8).try_into())]
    #[bw(ignore)]
    pub left_part: Instrument,

    #[br(try_calc = (((settings & 0b0001110000000000) >> 10) as u8).try_into())]
    #[bw(ignore)]
    pub right_part: Instrument,

    #[br(calc = (((settings & 0b0000001111000000) >> 6) as i8) - 7)]
    #[bw(ignore)]
    pub left_octave_shift: i8,

    #[br(calc = (((settings & 0b0000000000111100) >> 2) as i8) - 7)]
    #[bw(ignore)]
    pub right_octave_shift: i8,

    #[br(calc = (settings & 0b0000000000000010 >> 1) != 0)]
    #[bw(ignore)]
    pub left_sustain: bool,

    #[br(calc = (settings & 0b0000000000000001) != 0)]
    #[bw(ignore)]
    pub right_sustain: bool,

    /// 0x30
    #[brw(big)]
    #[bw(calc =
    (*left_control as u8) << 7
    | (*right_control as u8) << 6
    | ((*unknown_boolean1 as u8) << 5)
    | ((*split as u8) << 4)
    | ((*split_point as u8) << 1)
    | (*unknown_boolean2 as u8)
    )]
    pub settings2: u8,

    #[br(calc = ((settings2 & 0b10000000) >> 7) != 0)]
    #[bw(ignore)]
    pub left_control: bool,

    #[br(calc = ((settings2 & 0b01000000) >> 6) != 0)]
    #[bw(ignore)]
    pub right_control: bool,

    #[br(calc = ((settings2 & 0b00100000) >> 5) != 0)]
    #[bw(ignore)]
    pub unknown_boolean1: bool,

    #[br(calc = ((settings2 & 0b00010000) >> 4) != 0)]
    #[bw(ignore)]
    pub split: bool,

    #[br(try_calc = ((settings2 & 0b00001110) >> 1).try_into())]
    #[bw(ignore)]
    pub split_point: SplitPoint,


    // Pretty sure this boolean is either a part of transpose or just signals that the transpose
    // has been set to something other than default. This bit is not set on any default programs,
    // only programs that have has the transpose edited. It is even set on programs that have
    // transpose set to 0. It seems that default programs might have their transpose set to 1 (off)
    // instead of 0 (off)
    #[br(calc = (settings2 & 0b00000001) != 0)]
    #[bw(ignore)]
    pub unknown_boolean2: bool,

    #[brw(big)]
    pub settings3: u16,

    // transpose (0 to 12  big endian = -6 to -6 half steps transposition)
    // 0111 1100  12
    // 0111 10111 11
    // #[br(calc = ((settings3 & 0b1111000000000000) >> 12) as u8)]
    // #[bw(ignore)]
    // pub transpose: u8,

    // #[br(calc = ((settings3 & 0b0000100000000000) >> 11) != 0)]
    // #[bw(ignore)]
    // pub ??????: bool,

    // #[br(calc = ((settings3 & 0b0000011111100000) >> 5) as u8)]
    // #[bw(ignore)]
    // pub part_volume??: bool,

    // #[br(calc = ((settings3 & 0b0000000000011111)) as u8)]
    // #[bw(ignore)]
    // pub ??????: bool,
}

#[binrw]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0xa4 - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0xa4 - 0x2c))]
pub struct Schema {
    pub header: Header,

    pub version: u32,

    /// 0x18-0x1a
    #[bw(try_calc = w.checksum())]
    crc32: u32,

    /// 0x2c-0x2d
    #[brw(big, pad_before = 16)]
    program_version: u16,

    /// 0x2e-0x32
    center_panel: CenterPanel,

    body: [u8; (0xa4 - 0x32) as usize],
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
                body: [0; (0xa4 - 0x32) as usize],
                program_version: 4,
                center_panel: CenterPanel {
                    left_octave_shift: 0,
                    right_octave_shift: 0,
                    left_part: Instrument::Organ,
                    right_part: Instrument::Organ,
                    left_sustain: false,
                    right_sustain: false,
                    settings3: 0,
                    left_control: false,
                    right_control: false,
                    split_point: SplitPoint::C4,
                    split: false,
                    unknown_boolean1: false,
                    unknown_boolean2: false
                }
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

    pub fn left_part(&self) -> Instrument {
        self.schema.center_panel.left_part
    }

    pub fn right_part(&self) -> Instrument {
        self.schema.center_panel.right_part
    }

    pub fn left_octave_shift(&self) -> i8 {
        self.schema.center_panel.left_octave_shift
    }

    pub fn right_octave_shift(&self) -> i8 {
        self.schema.center_panel.right_octave_shift
    }

    pub fn left_sustain(&self) -> bool {
        self.schema.center_panel.left_sustain
    }

    pub fn right_sustain(&self) -> bool {
        self.schema.center_panel.right_sustain
    }

    pub fn left_control(&self) -> bool {
        self.schema.center_panel.left_control
    }

    pub fn right_control(&self) -> bool {
        self.schema.center_panel.right_control
    }

    pub fn split_point(&self) -> SplitPoint {
        self.schema.center_panel.split_point
    }

    pub fn split(&self) -> bool {
        self.schema.center_panel.split
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
            .field("left_part", &self.left_part())
            .field("right_part", &self.right_part())
            .field("left_octave_shift", &self.left_octave_shift())
            .field("right_octave_shift", &self.right_octave_shift())
            .field("left_sustain", &self.left_sustain())
            .field("right_sustain", &self.right_sustain())
            .field("left_control", &self.left_control())
            .field("right_control", &self.right_control())
            .field("split", &self.split())
            .field("split_point", &self.split_point())
            .finish()
    }
}
