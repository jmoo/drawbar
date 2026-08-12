//! The Stage 2 program body (`.ns2p`, `.ns2l`): 521 bytes, every documented
//! parameter placed.
//!
//! The program-wide globals were decoded first and by hand; everything else — the
//! organ's B3, Vox and Farfisa drawbar banks, the piano, synth, extern and the
//! effects chain — comes from the byte maps.
//!
//! ⚠️ Stage 2 files are **type-0** containers, where the Stage 3's are type-1. The
//! byte maps number both in the type-1 layout; since type-0 differs only by
//! omitting `0x18..0x2b`, the body is the same either way and a documented offset
//! is `doc - 0x2c` in both.
//!
//! The body is 23 bytes of globals and then two [`Slot`]s — the program's two
//! complete setups — so the slot is declared once and placed twice. Registry paths
//! follow: `slot_a.organ_type`.
//!
//! Values are raw except where the documentation enumerates them; see [the module
//! docs](super) for what that ceiling is and why.

use super::slot::Slot;
use crate::cbin::{self, Cbin, Header};
use crate::components::{
    sparse_enum, MasterTempo, ProgramCategory, ReverbType, SplitNote, StageTranspose,
};
use crate::error::Error;
use crate::types::RangedU8;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns2p";
/// Schema versions this build's field offsets have been validated against. The
/// corpus factory banks hold 6 and 7; the ns3-program-viewer accepts 2 through 7
/// with the same offsets.
pub const KNOWN_VERSIONS: &[u32] = &[2, 3, 4, 5, 6, 7];
pub const BODY_LEN: usize = 521;

/// The program-wide globals at the head of the body. Bits are MSB-first from body
/// byte 0 (`0x2c` in a type-1 file), so byte 0x02 bit 5 is bit 18.
#[nord_bits_derive::bitbody(521)]
pub struct Program {
    #[bits(16..=17)]
    pub slot_selection: RangedU8<3>,
    #[bits(18..=18)]
    pub dual_keyboard: bool,
    #[bits(20..=23)]
    pub split_low_note: SplitNote,
    #[bits(24..=27)]
    pub split_high_note: SplitNote,
    #[bits(28..=28)]
    pub split_three_zones: bool,
    #[bits(29..=29)]
    pub split_two_zones: bool,
    #[bits(33..=33)]
    pub organ_pitch_stick: bool,
    #[bits(34..=34)]
    pub transpose_enabled: bool,
    #[bits(35..=38)]
    pub transpose: StageTranspose,
    #[bits(43..=50)]
    pub master_clock: MasterTempo,
    #[bits(64..=65)]
    pub organ_model: RangedU8<3>,
    #[bits(72..=74)]
    pub organ_b3_vibrato_mode: RangedU8<7>,
    #[bits(75..=75)]
    pub organ_b3_harmonic_third: bool,
    #[bits(76..=76)]
    pub organ_b3_decay_fast: bool,
    #[bits(77..=77)]
    pub organ_b3_volume_soft: bool,
    #[bits(89..=90)]
    pub organ_vox_vibrato_mode: RangedU8<3>,
    #[bits(91..=91)]
    pub organ_vox_vibrato_on: bool,
    #[bits(105..=106)]
    pub organ_farfisa_vibrato_mode: RangedU8<3>,
    #[bits(107..=107)]
    pub organ_farfisa_vibrato_on: bool,
    #[bits(120..=122)]
    pub piano_slot_detune: RangedU8<7>,
    #[bits(136..=136)]
    pub reverb_on: bool,
    #[bits(137..=139)]
    pub reverb_type: ReverbType,
    #[bits(140..=146)]
    pub reverb_amount: RangedU8<127>,
    #[bits(147..=147)]
    pub compressor_on: bool,
    #[bits(148..=154)]
    pub compressor_amount: RangedU8<127>,
    #[bits(155..=155)]
    pub rotary_speaker_on: bool,
    #[bits(156..=157)]
    pub rotary_speaker_source: RangedU8<3>,
    #[bits(158..=164)]
    pub rotary_speaker_drive: RangedU8<127>,
    #[bits(165..=165)]
    pub rotary_speaker_stop_mode: bool,
    #[bits(166..=166)]
    pub rotary_speaker_speed: bool,
    #[bits(167..=167)]
    pub rotary_speaker_speed_wheel: bool,
    #[bits(168..=168)]
    pub rotary_speaker_speed_aftertouch: bool,
    #[bits(169..=169)]
    pub rotary_speaker_speed_ctrl_pedal: bool,

    /// Slot A — the first of the program's two complete setups.
    #[at(23..272)]
    pub slot_a: Slot,

    /// Slot B. Same type: the two are the same layout, and neither is
    /// a copy of the other — `slot_enabled_and_selection` says which sound.
    #[at(272..521)]
    pub slot_b: Slot,
}

impl Program {
    /// Whether any split is active.
    pub fn split_enabled(&self) -> bool {
        self.split_two_zones || self.split_three_zones
    }
}

/// The category byte the header's `aux` word carries; the three bytes above it
/// are zero on every corpus specimen.
pub fn category(header: &Header) -> ProgramCategory {
    use crate::bits::Packed;
    ProgramCategory::from_bits((header.aux & 0xff) as u64).expect("decoding is total")
}

/// The `(bank, location)` pair from the header, uninterpreted: bank 0..=3,
/// location 0..=99 on current exports. Not validated — see the Stage 3's note on
/// out-of-range locations in old files.
pub fn location(file: &Cbin<Program>) -> (u16, u16) {
    file.header.slot()
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
    let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}

sparse_enum!(
    /// From the `ns2-organ-kb-zone` table in the Stage byte-map docs.
    OrganKbZone, 3, {
        0 => Lo, "LO";
        1 => LoUp, "LO UP";
        2 => Up, "UP";
        3 => UpHi, "UP HI";
        4 => Hi, "HI";
        5 => LoUpHi, "LO UP HI";
    }
);

sparse_enum!(
    /// From the `ns2-piano-kb-zone` table in the Stage byte-map docs.
    PianoKbZone, 3, {
        0 => Lo, "LO";
        1 => LoUp, "LO UP";
        2 => Up, "UP";
        3 => UpHi, "UP HI";
        4 => Hi, "HI";
        5 => LoUpHi, "LO UP HI";
    }
);

sparse_enum!(
    /// From the `ns2-synth-kb-zone` table in the Stage byte-map docs.
    SynthKbZone, 3, {
        0 => Lo, "LO";
        1 => LoUp, "LO UP";
        2 => Up, "UP";
        3 => UpHi, "UP HI";
        4 => Hi, "HI";
        5 => LoUpHi, "LO UP HI";
    }
);
