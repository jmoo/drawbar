//! The Stage 3 program body (`.ns3f`): 548 bytes, globals decoded, panels not.
//!
//! Every placement here is inferred from the Nord User Forum's ns3-program-viewer
//! documentation (github.com/Chris55/ns3-program-viewer); not confirmed on
//! hardware. The two 263-byte panel blocks — organ, piano, synth, extern and the
//! effects chains — are documented there too but not yet decoded; their bits ride
//! along verbatim.

use crate::cbin::{self, Cbin, Header};
use crate::components::{
    sparse_enum, MasterTempo, ProgramCategory, SplitNote, SplitWidth, StageTranspose,
};
use crate::error::Error;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns3f";
/// Schema versions this build's field offsets have been validated against:
/// program v3.00 (OS v0.92) through v3.04 (OS v2.10 and later), stored ×100.
pub const KNOWN_VERSIONS: &[u32] = &[300, 301, 302, 303, 304];
pub const BODY_LEN: usize = 548;

sparse_enum!(
    /// Which panels the program enables.
    PanelEnable, 2, {
        0 => AOnly, "A only";
        1 => BOnly, "B only";
        2 => Both, "A & B";
    }
);

sparse_enum!(
    /// What the second keyboard plays when Dual Keyboard is on.
    DualKeyboardStyle, 2, {
        0 => Panel, "Panel";
        1 => Organ, "Organ";
        2 => Piano, "Piano";
        3 => Synth, "Synth";
    }
);

/// The program-wide globals at the head of the body. Bits are MSB-first from body
/// byte 0 (`0x2c` in a type-1 file), so byte 0x05 bit 7 is bit 40.
///
/// ⚠️ The three split notes can be stored out of order — the panel reorders them on
/// display (documented with specimens in the ns3-program-viewer sources). The
/// decode reports what is stored.
#[nord_bits_derive::bitbody(548)]
pub struct Program {
    /// Body byte 0x05 bit 7: which panel was selected when the program was stored.
    #[bits(40..=40)]
    pub panel_b_selected: bool,
    #[bits(41..=42)]
    pub panel_enable: PanelEnable,
    #[bits(43..=43)]
    pub split_enabled: bool,
    #[bits(44..=44)]
    pub split_low_enabled: bool,
    #[bits(45..=45)]
    pub split_mid_enabled: bool,
    #[bits(46..=46)]
    pub split_high_enabled: bool,
    #[bits(47..=50)]
    pub split_low_note: SplitNote,
    #[bits(51..=54)]
    pub split_mid_note: SplitNote,
    #[bits(55..=58)]
    pub split_high_note: SplitNote,
    #[bits(59..=60)]
    pub split_low_width: SplitWidth,
    #[bits(61..=62)]
    pub split_mid_width: SplitWidth,
    #[bits(63..=64)]
    pub split_high_width: SplitWidth,
    /// Touched at least once, not active: the untouched default stores 6 (= 0).
    #[bits(96..=96)]
    pub transpose_enabled: bool,
    #[bits(97..=100)]
    pub transpose: StageTranspose,
    #[bits(101..=108)]
    pub master_clock: MasterTempo,
    #[bits(116..=116)]
    pub dual_keyboard: bool,
    #[bits(118..=119)]
    pub dual_keyboard_style: DualKeyboardStyle,
}

/// The category byte the header's `aux` word carries; the three bytes above it
/// are zero on every corpus specimen.
pub fn category(header: &Header) -> ProgramCategory {
    use crate::bits::Packed;
    ProgramCategory::from_bits((header.aux & 0xff) as u64).expect("decoding is total")
}

/// The `(bank, location)` pair from the header, uninterpreted.
///
/// Not validated: current exports hold bank 0..=15 and location 0..=24, but v3.00
/// files in the wild hold out-of-range locations (norduserforum.com t=14414), so
/// gating on them would refuse real files.
pub fn location(file: &Cbin<Program>) -> (u16, u16) {
    file.header.slot()
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
    let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
