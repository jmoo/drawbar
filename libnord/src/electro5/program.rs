use crate::common;
use crate::common::{bank, PartMix};
use crate::crc::{CrcReader, CrcWriter};
use crate::types::RangedU16Pair;
use binrw::{binrw, BinRead, BinReaderExt, BinWrite, BinWriterExt};
use std::fmt::Debug;
use std::io;
use crate::electro5::{Instrument, OctaveShift, SplitPoint, Transpose};

pub const FORMAT: &str = "ne5p";
pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
pub type Header = common::Header<Location>;
pub type Bank = bank::Bank<Program, Location>;

// 0x2e-0x32
#[binrw]
#[derive(Debug)]
pub struct CenterPanel {
    // 0x2e-0x2f
    #[brw(big)]
    #[bw(calc =
    (*left_part as u16) << 13
    | (*right_part as u16) << 10
    | ((((*left_octave_shift).as_u8()) as u16) << 6)
    | ((((*right_octave_shift).as_u8()) as u16) << 2)
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

    #[br(try_calc = (((settings & 0b0000001111000000) >> 6) as u8).try_into())]
    #[bw(ignore)]
    pub left_octave_shift: OctaveShift,

    #[br(try_calc = (((settings & 0b0000000000111100) >> 2) as u8).try_into())]
    #[bw(ignore)]
    pub right_octave_shift: OctaveShift,

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
    | (*transpose_enabled as u8)
    )]
    pub settings2: u8,

    // 0x30
    #[br(calc = ((settings2 & 0b10000000) >> 7) != 0)]
    #[bw(ignore)]
    pub left_control: bool,

    // 0x30
    #[br(calc = ((settings2 & 0b01000000) >> 6) != 0)]
    #[bw(ignore)]
    pub right_control: bool,

    // 0x30
    #[br(calc = ((settings2 & 0b00100000) >> 5) != 0)]
    #[bw(ignore)]
    pub unknown_boolean1: bool,

    // 0x30
    #[br(calc = ((settings2 & 0b00010000) >> 4) != 0)]
    #[bw(ignore)]
    pub split: bool,

    // 0x30
    #[br(try_calc = ((settings2 & 0b00001110) >> 1).try_into())]
    #[bw(ignore)]
    pub split_point: SplitPoint,

    // 0x30
    // NOTE: Sometimes the electro 5 leaves this as true even when the transpose is 0. It will
    // not show a transpose light when this happens
    #[br(calc = (settings2 & 0b00000001) != 0)]
    #[bw(ignore)]
    pub transpose_enabled: bool,

    // 0x31
    #[brw(big)]
    pub settings3: u16,

    // transpose (0 to 12  big endian = -6 to -6 half steps transposition)
    #[br(try_calc = ((settings3 & 0b1111000000000000) >> 12).try_into())]
    #[bw(ignore)]
    pub transpose: Transpose,

    #[br(try_calc = ((settings3 & 0b0000111111100000) >> 5).try_into())]
    #[bw(ignore)]
    pub part_mix: PartMix,

    // #[br(calc = ((settings3 & 0b0000000000011111)) as u8)]
    // #[bw(ignore)]
    // pub ??????: bool,
}

#[binrw]
#[derive(Debug)]
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

#[derive(Debug)]
pub struct Program {
    schema: Schema,
    location: Location,
    name: Option<String>,
}

impl Program {
    pub fn new(location: Location) -> Program {
        Program {
            location: location,
            name: None,
            schema: Schema {
                header: Header::new(1, FORMAT, location),
                version: 4,
                body: [0; (0xa4 - 0x32) as usize],
                program_version: 4,
                center_panel: CenterPanel {
                    left_octave_shift: (0_i8).try_into().unwrap(),
                    right_octave_shift: (0_i8).try_into().unwrap(),
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
                    transpose: (1_i8).try_into().unwrap(),
                    transpose_enabled: false,
                    part_mix: (0_u8).try_into().unwrap(),
                },
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Program, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };

        Ok(Program {
            location: schema.header.location,
            name: None,
            schema,
        })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        self.schema.header.location = self.location;

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

    pub fn left_octave_shift(&self) -> OctaveShift {
        self.schema.center_panel.left_octave_shift
    }

    pub fn right_octave_shift(&self) -> OctaveShift {
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

    pub fn transpose(&self) -> Transpose {
        self.schema.center_panel.transpose
    }

    pub fn transpose_enabled(&self) -> bool {
        self.schema.center_panel.transpose_enabled
    }

    pub fn part_mix(&self) -> PartMix {
        self.schema.center_panel.part_mix
    }
}

impl bank::Item<Location> for Program {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn set_name(&mut self, name: String) -> () {
        self.name = Some(name);
    }

    fn location(&self) -> Location {
        self.location
    }

    fn set_location(&mut self, location: Location) -> () {
        self.location = location;
    }
}

impl common::program::Program for Program {}
