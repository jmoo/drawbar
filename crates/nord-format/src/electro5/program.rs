use crate::common;
use crate::common::{bank, PartMix};
use crate::crc::{CrcReader, CrcWriter};
use crate::electro5::{Instrument, OctaveShift, SplitPoint, Transpose};
use crate::types::RangedU16Pair;
use binrw::{binrw, BinRead, BinReaderExt, BinWriterExt};

use std::fmt::Debug;
use std::io;

pub const FORMAT: &str = "ne5p";
pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
pub type Header = common::Header<Location>;
pub type Bank = bank::Bank<Program, Location>;

// 0x2e-0x32
#[binrw]
#[derive(Debug, Default)]
pub struct CenterPanel {
    // 0x2e..0x2f                 0x2e     0x2f
    #[brw(big)]
    #[bw(calc =
    (* left_part as u16) << 13
    | (* right_part as u16) << 10
    | ((((* left_octave_shift).as_u8()) as u16) << 6)
    | ((((* right_octave_shift).as_u8()) as u16) << 2)
    | ((* left_sustain as u16) << 1)
    | (* right_sustain as u16)
    )]
    settings: u16,

    #[br(try_calc = (((settings & 0b11100000_00000000) >> ((8*1)+5)) as u8).try_into())]
    #[bw(ignore)]
    pub left_part: Instrument,

    #[br(try_calc = (((settings & 0b00011100_00000000) >> ((8*1)+2)) as u8).try_into())]
    #[bw(ignore)]
    pub right_part: Instrument,

    #[br(try_calc = (((settings & 0b00000011_11000000) >> ((8*0)+6)) as u8).try_into())]
    #[bw(ignore)]
    pub left_octave_shift: OctaveShift,

    #[br(try_calc = (((settings & 0b00000000_00111100) >> ((8*0)+2)) as u8).try_into())]
    #[bw(ignore)]
    pub right_octave_shift: OctaveShift,

    #[br(calc = ((settings & 0b00000000_00000010) >> ((8*0)+1)) != 0)]
    #[bw(ignore)]
    pub left_sustain: bool,

    #[br(calc = ((settings & 0b00000000_00000001) >> ((8*0)+0)) != 0)]
    #[bw(ignore)]
    pub right_sustain: bool,

    // 0x30                    0x30
    #[brw(big)]
    #[bw(calc =
    (* left_control as u8) << 7
    | (* right_control as u8) << 6
    | ((* unknown_boolean1 as u8) << 5)
    | ((* split as u8) << 4)
    | ((* split_point as u8) << 1)
    | (* transpose_enabled as u8)
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

    // always zero
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

    // 0x31..34                     0x31      0x32      0x33     0x34
    #[brw(big)]
    pub settings3: u32,

    // -6, 5, 4, 3, 2, 1, 0, 1 111
    // transpose (0 to 12  big endian = -6 to -6 half steps transposition)
    #[br(try_calc = (((settings3 & 0b11110000_00000000_00000000_00000000) >> ((8 * 2) + 12)) as u16).try_into())]
    #[bw(ignore)]
    pub transpose: Transpose,

    #[br(try_calc = (((settings3 & 0b00001111_11100000_00000000_00000000) >> ((8 * 2) + 5)) as u16).try_into())]
    #[bw(ignore)]
    pub part_mix: PartMix,

    // 0..127 (0..10)
    #[br(calc = ((settings3 & 0b00000000_00011111_11000000_00000000) >> ((8 * 1) + 6)) as u8)]
    #[bw(ignore)]
    pub gain: u8,

    #[br(calc = ((settings3 & 0b00000000_00000000_00111000_00000000) >> ((8 * 1) + 3)) as u8)]
    #[bw(ignore)]
    pub organ_type: u8,

    #[br(calc = ((settings3 & 0b00000000_00000000_00000100_00000000) >> ((8 * 1) + 2)) != 0)]
    #[bw(ignore)]
    pub lower_enabled: bool,

    #[br(calc = ((settings3 & 0b00000000_00000000_00000010_00000000) >> ((8 * 1) + 1)) != 0)]
    #[bw(ignore)]
    pub upper_enabled: bool,

    #[br(calc = ((settings3 & 0b00000000_00000000_00000001_00000000) >> ((8 * 1) + 0)) != 0)]
    #[bw(ignore)]
    pub drawbar_live: bool,
}

// 0x3a..0x41
#[binrw]
#[derive(Debug, Default)]
pub struct PianoPanel {
    // 0x3a..0x41               0x3a      0x3b     0x3c     0x3d     0x3e     0x3f     0x40    0x41
    #[brw(big)]
    settings: u64,

    // 5 == 0, 6 == 1, 1 == 2, 2 == 3, 3 == 4, 4 == 5
    #[br(calc = ((settings & 0b11100000_00000000_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 7) + 5)) as u8)]
    #[bw(ignore)]
    pub category: u8,

    #[br(calc = ((settings & 0b00000111_11000000_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 6) + 0)) as u8)]
    #[bw(ignore)]
    pub piano_model: u8,

    #[br(calc = ((settings & 0b00000000_00000001_10000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 5) + 7)) as u8)]
    #[bw(ignore)]
    pub clav_model: u8,

    #[br(calc = ((settings & 0b00000000_00000000_01100000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 5) + 5)) as u8)]
    #[bw(ignore)]
    pub acoustics: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00011000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 5) + 3)) as u8)]
    #[bw(ignore)]
    pub touch: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00000100_00000000_00000000_00000000_00000000_00000000) >> ((8 * 5) + 2)) != 0)]
    #[bw(ignore)]
    pub mono: bool,

    #[br(calc = ((settings & 0b00000000_00000000_00000011_11111111_11111111_11111111_11111100_00000000) >> ((8 * 1) + 2)) != 0)]
    #[bw(ignore)]
    pub id: bool,
}

// 0x46..0x4d
#[binrw]
#[derive(Debug, Default)]
pub struct SamplePanel {
    // 0x46..0x4d               0x46      0x47     0x48     0x49     0x4a     0x4b     0x4c    0x4d
    #[brw(big)]
    settings: u64,

    #[br(calc = ((settings & 0b11111110_00000000_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 7) + 1)) as u8)]
    #[bw(ignore)]
    pub attack: u8,

    #[br(calc = ((settings & 0b00000001_11111100_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 6) + 2)) as u8)]
    #[bw(ignore)]
    pub decay_release: u8,

    #[br(calc = ((settings & 0b00000000_00000011_11111100_00000000_00000000_00000000_00000000_00000000) >> ((8 * 0) + 0)) as u8)]
    #[bw(ignore)]
    pub number: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00000011_11111111_11111111_11111111_11111100_00000000) >> ((8 * 0) + 0)) as u32)]
    #[bw(ignore)]
    pub id: u32,

    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000000_00000000_00000000_00000011_00000000) >> ((8 * 1) + 0)) as u8)]
    #[bw(ignore)]
    pub dynamics: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000000_00000000_00000000_00000000_10000000) >> ((8 * 0) + 7)) != 0)]
    #[bw(ignore)]
    pub filter: bool,
}

// 0x4e..0x92 — the organ panel. The Electro 5 stores the full drawbar +
// vib/perc state for *every* organ model (B3, Vox, Farfisa, Pipe) and both
// presets, so switching model/preset on the instrument is lossless too.
//
//   * Drawbars = 9 nibbles, physical position 0..=8, packed high-nibble first,
//     at these panel offsets per model + preset (B3-bass shares the B3 slots):
//         B3   p1 0x55  p2 0x5c      Vox  p1 0x67  p2 0x6d
//         Farf p1 0x77  p2 0x7d      Pipe p1 0x87  p2 0x8d
//     Every model stores the *physical* bar position on disk; the per-model
//     "real" value (Farf's >=5 on/off, Vox's ignored 8th bar, B3-bass's remapped
//     bass bars) is a display transform layered on top — NOT decoded here yet.
//   * Preset selection = bit 0x40 of one byte per model group
//         B3 0x53, Vox 0x65, Farf 0x75, Pipe 0x85   (0 = preset 1, 1 = preset 2)
//
// STILL RAW (byte map retained below): the vib/chorus on-off + type and perc
// on/third/speed toggles. They round-trip byte-exact through `raw`; decoding
// them semantically is the next organ increment.

/// Length of the organ panel block, 0x4e..=0x92 (69 bytes).
const ORGAN_LEN: usize = 0x92 - 0x4d;

/// Panel-relative index of the byte at absolute Electro 5 file offset `abs`
/// (the organ panel begins at 0x4e).
const fn org(abs: usize) -> usize {
    abs - 0x4e
}

/// Nine drawbar positions (physical, 0..=8), nibble-packed high-nibble first,
/// starting at panel-relative byte `at`. This is the on-disk form shared by all
/// organ models; per-model display transforms are applied elsewhere.
fn read_drawbars(raw: &[u8], at: usize) -> [u8; 9] {
    let mut bars = [0u8; 9];
    for (n, bar) in bars.iter_mut().enumerate() {
        let byte = raw[at + n / 2];
        *bar = if n % 2 == 0 { byte >> 4 } else { byte & 0x0f };
    }
    bars
}

/// The Electro 5's four organ models. (B3-bass shares the B3 storage slots, so
/// it isn't a separate model here.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganModel {
    B3,
    Vox,
    Farfisa,
    Pipe,
}

#[binrw]
#[derive(Debug)]
pub struct OrganPanel {
    /// The whole 0x4e..=0x92 block, kept verbatim so the panel always
    /// round-trips byte-exact. Decoded values are exposed via the methods below.
    raw: [u8; ORGAN_LEN],
    // // 0x4e..0x50
    // pad: B24,
    //
    // // 0x51 0b11100000
    // pub b3_vib_type: B3,
    //
    // // 0x51 0b00010000
    // pub b3_perc_third: bool,
    //
    // // 0x51 0b00001100
    // pub b3_perc_speed: B3,
    //
    // // 0x52
    // pad2: u8,
    //
    // // 0x53 0b01000000
    // pub b3_bass_preset: bool,
    //
    // // 0x54
    // pub unknown_byte: u8,
    //
    // // 0x55 0b11111111_11111111_11111111_11111111_11110000
    // pub preset1_b3_drawbars: Drawbars,

    // Drawbars: 9 with 4 bits each representing a value of 0..8
    // 0x4e..0x50      - pad
    // 0x51 0b11100000 - preset 1/2 b3/b3-bass vib selection (010: 0, 101: 3)
    // 0x51 0b00010000 - preset 1/2 b3/b3-bass perc third (0,1)
    // 0x51 0b00001100 - preset 1/2 b3/b3-bass perc speed (10: 1, 01: 2, 11: 3)
    // 0x52 0b00000000 - pad
    // 0x53 0b01000000 - b3/b3-bass preset selection
    // 0x54 0b00000000 - ?
    // 0x55 0b11111111_11111111_11111111_11111111_11110000 - preset1 drawbars (b3 normal, b3-bass inverted for first two and then normal for the rest except their value is ignored)
    // 0x59 0b00001000 - preset 1 b3/b3-bass vib on/off (0,1)
    // 0x59 0b00000100 - preset 1 b3/b3-bass perc on/off (0,1)
    // 0x59 0b00000010 - ?
    // 0x59 0b00000001 - ?
    // 0x5a 0b00000000 - ?
    // 0x5b 0b00000000 - pad
    // 0x5c 0b11111111_11111111_11111111_11111111_11110000 - preset2 drawbars (b3 normal, b3-bass normal)
    // 0x60 0b00000100 - preset 2 b3/b3-bass vib on/off (0,1)
    // 0x60 0b00000100 - preset 2 b3/b3-bass perc on/off (0,1)
    // 0x60 0b00000010 - unknown boolean (true on all programs i have created, false on a bunch of random presets)
    // 0x61 0b00100000 - unknown boolean (true on all programs i have created, false on a bunch of random presets)
    // 0x62 0b00000000 - pad
    // 0x63 0b11100000 - preset 1/2 vox vib selection (000: 4, 010: 2, 001: 0)
    // 0x64 0b00000000 - pad
    // 0x65 0b01000000 - vox preset selection
    // 0x66 0b00000000 - pad
    // 0x67 0b11111111_11111111_11111111_11111111_11110000 - preset1 drawbars (vox normal but 8th drawbar value is ignored)
    // 0x6b 0b00001000 - preset 1 vox vib on/off
    // 0x6c 0b00000000 - pad
    // 0x6d 0b11111111_11111111_11111111_11111111_11110000 - preset1 drawbars (vox normal but 8th drawbar value is ignored)
    // 0x71 0b00001000 - preset 2 vox vib on/off
    // 0x72 0b00000000 - pad
    // 0x73 0b11100000 - preset 1/2 farfisa vib selection (000: 4, 011: 3, 010: 1, 001: 0)
    // 0x74 0b00000000 - pad
    // 0x75 0b01000000 - farf preset selection
    // 0x76 0b00000000 - pad
    // 0x77 0b11111111_11111111_11111111_11111111_11110000 - preset1 drawbars (farfisa normal values but >= 5 is interpreted as 1 and anything else is interpreted as 0)
    // 0x7b 0b00001000 - preset 1 farfisa vib on/off
    // 0x7c 0b00000000 - pad
    // 0x7d 0b11111111_11111111_11111111_11111111_11110000 - preset2 drawbars (farfisa normal values but >= 5 is interpreted as 1 and anything else is interpreted as 0)
    // 0x81 0b00001000 - preset 2 farfisa vib on/off
    // 0x82 0b00000000 - pad
    // 0x83 0b00000000 - pad
    // 0x84 - pad
    // 0x85 0b01000000 - pipe preset selection
    // 0x86 - pad
    // 0x87 0b11111111_11111111_11111111_11111111_11110000 - preset1 drawbars (pipe normal)
    // 0x8b 0b00001000 - unknown boolean (always true except for included preset 'Sunday')
    // 0x8c 0b00000000 - pad
    // 0x8d 0b11111111_11111111_11111111_11111111_11110000 - preset2 drawbars (pipe, normal)
}

/// A vibrato (`V`) or chorus (`C`) organ modulation at one of three depths.
/// Which subset is available depends on the model (see [`OrganPanel::vib_type`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VibChorus {
    V1,
    C1,
    V2,
    C2,
    V3,
    C3,
}

/// B3 percussion decay speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PercSpeed {
    Off,
    Soft,
    Fast,
    Both,
}

impl OrganPanel {
    /// Panel-relative drawbar-block offset for a model + preset (1 or 2).
    fn drawbar_offset(model: OrganModel, preset: u8) -> usize {
        let (p1, p2) = match model {
            OrganModel::B3 => (0x55, 0x5c),
            OrganModel::Vox => (0x67, 0x6d),
            OrganModel::Farfisa => (0x77, 0x7d),
            OrganModel::Pipe => (0x87, 0x8d),
        };
        org(if preset == 2 { p2 } else { p1 })
    }

    /// Panel-relative index of a model's preset-selection byte (bit 0x40).
    fn preset_byte(model: OrganModel) -> usize {
        org(match model {
            OrganModel::B3 => 0x53,
            OrganModel::Vox => 0x65,
            OrganModel::Farfisa => 0x75,
            OrganModel::Pipe => 0x85,
        })
    }

    /// The selected preset (1 or 2) for `model`.
    pub fn preset(&self, model: OrganModel) -> u8 {
        if self.raw[Self::preset_byte(model)] & 0x40 != 0 {
            2
        } else {
            1
        }
    }

    /// The nine drawbar positions (physical, 0..=8) stored for `model`'s
    /// `preset`. This is the on-disk value; per-model display transforms
    /// (Farfisa on/off, Vox's ignored 8th bar, B3-bass bass-bar remap) are not
    /// applied.
    pub fn drawbars(&self, model: OrganModel, preset: u8) -> [u8; 9] {
        read_drawbars(&self.raw, Self::drawbar_offset(model, preset))
    }

    /// Panel-relative index of the per-preset vib/perc byte for `model`, or
    /// `None` for Pipe (no vib/perc). For B3 this byte also holds percussion.
    fn effect_byte(model: OrganModel, preset: u8) -> Option<usize> {
        let (p1, p2) = match model {
            OrganModel::B3 => (0x59, 0x60),
            OrganModel::Vox => (0x6b, 0x71),
            OrganModel::Farfisa => (0x7b, 0x81),
            OrganModel::Pipe => return None,
        };
        Some(org(if preset == 2 { p2 } else { p1 }))
    }

    /// Whether vibrato/chorus is on for `model`'s `preset`.
    pub fn vib_on(&self, model: OrganModel, preset: u8) -> bool {
        match Self::effect_byte(model, preset) {
            Some(i) => self.raw[i] & 0x08 != 0,
            None => false,
        }
    }

    /// The vibrato/chorus mode selected for `model` (shared across presets), or
    /// `None` for Pipe. Each model exposes a different subset of the six modes,
    /// so the stored 3-bit value indexes into a per-model table.
    pub fn vib_type(&self, model: OrganModel) -> Option<VibChorus> {
        use VibChorus::*;
        let (byte, table): (usize, &[VibChorus]) = match model {
            OrganModel::B3 => (org(0x51), &[V1, C1, V2, C2, V3, C3]),
            OrganModel::Vox => (org(0x63), &[V1, V2, V3]),
            OrganModel::Farfisa => (org(0x73), &[V1, V2, C2, C3]),
            OrganModel::Pipe => return None,
        };
        table.get((self.raw[byte] >> 5) as usize).copied()
    }

    /// Whether B3 percussion is on for `preset` (B3 only).
    pub fn b3_perc_on(&self, preset: u8) -> bool {
        self.raw[org(if preset == 2 { 0x60 } else { 0x59 })] & 0x04 != 0
    }

    /// Whether B3 percussion uses the third harmonic (shared across presets).
    pub fn b3_perc_third(&self) -> bool {
        self.raw[org(0x51)] & 0x10 != 0
    }

    /// B3 percussion decay speed (shared across presets). Note the on-disk
    /// encoding is not monotonic — the two speed bits store 2/1/3 for
    /// soft/fast/both.
    pub fn b3_perc_speed(&self) -> PercSpeed {
        match (self.raw[org(0x51)] >> 2) & 0x03 {
            0 => PercSpeed::Off,
            2 => PercSpeed::Soft,
            1 => PercSpeed::Fast,
            _ => PercSpeed::Both,
        }
    }
}

// 0x93..0x9F
#[binrw]
#[derive(Debug, Default)]
pub struct EffectsPanel {
    // 0x93..0x9a               0x93      0x94     0x95     0x96     0x97     0x98     0x99    0x9a
    #[brw(big)]
    settings: u64,

    // fx1 (0: off, 1: lower, 2: upper)
    #[br(calc = ((settings & 0b11000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 7) + 6)) as u8)]
    #[bw(ignore)]
    pub fx1: u8,

    // fx1 type (1: pan1, pan2, pan1&2, 2: wah, rm, trem1, trem2, trem1&2)
    #[br(calc = ((settings & 0b00111100_00000000_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 7) + 2)) as u8)]
    #[bw(ignore)]
    pub fx1_type: u8,

    // fx1 rate 0..127 (0..10)
    #[br(calc = ((settings & 0b00000011_11111000_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 6) + 3)) as u8)]
    #[bw(ignore)]
    pub fx1_rate: u8,

    // fx2 (0: off, 1: lower, 2: upper
    #[br(calc = ((settings & 0b00000000_00000110_00000000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 6) + 1)) as u8)]
    #[bw(ignore)]
    pub fx2: u8,

    // fx2 type (flang, choir1, choir2, vibe, phas1, phas2)
    #[br(calc = ((settings & 0b00000000_00000001_11100000_00000000_00000000_00000000_00000000_00000000) >> ((8 * 5) + 5)) as u8)]
    #[bw(ignore)]
    pub fx2_type: u8,

    // fx2 rate 0..127 (0..10)
    #[br(calc = ((settings & 0b00000000_00000000_00011111_11000000_00000000_00000000_00000000_00000000) >> ((8 * 4) + 6)) as u8)]
    #[bw(ignore)]
    pub fx2_rate: u8,

    // fx4 (0: off, 1: lower, 2: upper)
    #[br(calc = ((settings & 0b00000000_00000000_00000000_00110000_00000000_00000000_00000000_00000000) >> ((8 * 4) + 4)) as u8)]
    #[bw(ignore)]
    pub fx4: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00000000_00001100_00000000_00000000_00000000_00000000) >> ((8 * 4) + 2)) as u8)]
    #[bw(ignore)]
    pub fx4_feedback: u8,

    // fx4 rate 0..127 (750ms..20ms)
    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000011_11111000_00000000_00000000_00000000) >> ((8 * 3) + 3)) as u8)]
    #[bw(ignore)]
    pub fx4_tempo: u8,

    // fx4 wet/dry 0..127 (0..10)
    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000000_00000111_11110000_00000000_00000000) >> ((8 * 2) + 4)) as u8)]
    #[bw(ignore)]
    pub fx4_moisture: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000000_00000000_00001000_00000000_00000000) >> ((8 * 2) + 3)) != 0)]
    #[bw(ignore)]
    pub fx4_ping_pong: bool,

    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000000_00000000_00000110_00000000_00000000) >> ((8 * 2) + 1)) as u8)]
    #[bw(ignore)]
    pub equalizer_part_select: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000000_00000000_00000001_11111100_00000000) >> ((8 * 1) + 2)) as u8)]
    #[bw(ignore)]
    pub equalizer_freq: u8,

    #[br(calc = ((settings & 0b00000000_00000000_00000000_00000000_00000000_00000000_00000011_11111000) >> ((8 * 0) + 3)) as u8)]
    #[bw(ignore)]
    pub equalizer_treble: u8,

    // 0x9b..0x9e
    #[brw(big)]
    settings2: u32,

    //                           0x9a                                     0x9b      0x9c     0x9d      0x9e
    #[br(calc = (((settings & 0b00000111) << 4) as u8) + (((settings2 & 0b11110000_00000000_00000000_00000000) >> ((8 * 3) + 4)) as u8))]
    #[bw(ignore)]
    pub equalizer_freq_gain: u8,

    //                           0x9b      0x9c      0x9d     0x9e
    #[br(calc = ((settings2 & 0b00001111_11100000_00000000_00000000) >> ((8 * 2) + 5)) as u8)]
    #[bw(ignore)]
    pub equalizer_bass: u8,

    // fx3 (0: off, 1: lower, 2: upper)
    #[br(calc = ((settings2 & 0b00000000_00011000_00000000_00000000) >> ((8 * 2) + 3)) as u8)]
    #[bw(ignore)]
    pub fx3: u8,

    // fx3 type (none, twin, rotary, comp, small, jc)
    #[br(calc = ((settings2 & 0b00000000_00000111_00000000_00000000) >> ((8 * 2) + 0)) as u8)]
    #[bw(ignore)]
    pub fx3_type: u8,

    // fx3 rate 0..127 (0..10)
    #[br(calc = ((settings2 & 0b00000000_00000000_11111110_00000000) >> ((8 * 1) + 1)) as u8)]
    #[bw(ignore)]
    pub fx3_compression: u8,

    #[br(calc = ((settings2 & 0b00000000_00000000_00000001_00000000) >> ((8 * 1) + 0)) != 0)]
    #[bw(ignore)]
    pub fx5: bool,

    #[br(calc = ((settings2 & 0b00000000_00000000_00000000_11100000) >> ((8 * 0) + 5)) as u8)]
    #[bw(ignore)]
    pub fx5_type: u8,

    // 0x9f
    #[brw(big)]
    settings3: u8,

    // 0x9b..0x9f                  0x9b      0x9c      0x9d     0x9e                                   0x9f
    #[br(calc = ((((settings2 & 0b00000000_00000000_00000000_00011111)) << 2) as u8) + ((settings3 & 0b11000000) >> 6))]
    #[bw(ignore)]
    pub fx5_moisture: u8,

    // 0 = off, 1 = on
    #[br(calc = ((settings3 & 0b00100000) >> ((8 * 0) + 5)) as u8)]
    #[bw(ignore)]
    pub rotary_stop: u8,

    // 0 = slow, 1 = fast
    #[br(calc = ((settings3 & 0b00010000) >> ((8 * 0) + 4)) as u8)]
    #[bw(ignore)]
    pub rotary_speed: u8,
}

// 0xa1..0xa4
#[binrw]
#[derive(Debug, Default)]
pub struct Extra {
    #[brw(big)]
    settings: u32,

    // fx1 control pedal (0: off, 1: on)
    #[br(calc = ((settings & 0b00010000_00000000_00000000_00000000) >> ((8 * 3) + 4)) != 0)]
    #[bw(ignore)]
    pub fx1_control: bool,

    // fx1 deep (0: off, 1: on)
    #[br(calc = ((settings & 0b00001000_00000000_00000000_00000000) >> ((8 * 3) + 3)) != 0)]
    #[bw(ignore)]
    pub fx2_deep: bool,
}

#[binrw]
#[derive(Debug)]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0xa4 - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0xa4 - 0x2c))]
pub struct Schema {
    pub header: Header,

    pub version: u32,

    // 0x18..0x1a
    #[bw(try_calc = w.checksum())]
    crc32: u32,

    // 0x2c..0x2d
    #[brw(big, pad_before = 16)]
    program_version: u16,

    // 0x2e..0x34
    pub center_panel: CenterPanel,

    // 0x35..0x3b
    pad1: [u8; (0x39 - 0x34) as usize],

    // 0x3a..0x41
    pub piano_panel: PianoPanel,

    // 0x42..0x45
    pad2: [u8; (0x45 - 0x41) as usize],

    // 0x46..0x4d
    pub sample_panel: SamplePanel,

    // 0x4e..0x92
    pub organ_panel: OrganPanel,

    // 0x93..0x9f
    pub effects_panel: EffectsPanel,

    // 0xa0
    todo: u8,

    // 0xa1..0xa4
    pub extra: Extra,
}

#[derive(Debug)]
pub struct Program {
    pub schema: Schema,
    location: Location,
    name: Option<String>,
}

impl Program {
    pub fn new(location: Location) -> Program {
        Program {
            location,
            name: None,
            schema: Schema {
                header: Header::new(1, FORMAT, location),
                version: 4,
                pad1: [0; (0x39 - 0x34) as usize],
                pad2: [0; (0x45 - 0x41) as usize],
                todo: 0,
                program_version: 4,
                center_panel: CenterPanel::default(),
                piano_panel: PianoPanel::default(),
                sample_panel: SamplePanel::default(),
                organ_panel: OrganPanel {
                    raw: [0; ORGAN_LEN],
                },
                effects_panel: EffectsPanel::default(),
                extra: Extra::default(),
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

    pub fn lower_part(&self) -> Instrument {
        self.schema.center_panel.left_part
    }

    pub fn upper_part(&self) -> Instrument {
        self.schema.center_panel.right_part
    }

    pub fn lower_octave_shift(&self) -> OctaveShift {
        self.schema.center_panel.left_octave_shift
    }

    pub fn upper_octave_shift(&self) -> OctaveShift {
        self.schema.center_panel.right_octave_shift
    }

    pub fn lower_sustain(&self) -> bool {
        self.schema.center_panel.left_sustain
    }

    pub fn upper_sustain(&self) -> bool {
        self.schema.center_panel.right_sustain
    }

    pub fn lower_control(&self) -> bool {
        self.schema.center_panel.left_control
    }

    pub fn upper_control(&self) -> bool {
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

    pub fn gain(&self) -> u8 {
        self.schema.center_panel.gain
    }

    pub fn fx_panel(&self) -> &EffectsPanel {
        &self.schema.effects_panel
    }

    pub fn extra(&self) -> &Extra {
        &self.schema.extra
    }

    pub fn organ(&self) -> &OrganPanel {
        &self.schema.organ_panel
    }
}

impl bank::Item<Location> for Program {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn set_name(&mut self, name: String) {
        self.name = Some(name);
    }

    fn location(&self) -> Location {
        self.location
    }

    fn set_location(&mut self, location: Location) {
        self.location = location;
    }
}

impl common::program::Program for Program {}
