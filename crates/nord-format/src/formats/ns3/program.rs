//! The Stage 3 program body (`.ns3f`, `.ns3l`): 548 bytes, every documented
//! parameter placed.
//!
//! The program-wide globals were decoded first and by hand; everything else — the
//! organ's two presets and their drawbars, the piano, synth, extern and the whole
//! effects chain — comes from the byte maps. Where the two disagree the hand
//! decode wins: the map has one 22-bit `split` run where the companion doc, and
//! this module, break it into eight fields.
//!
//! The body is 22 bytes of globals and then two [`Panel`]s — the program's two
//! complete setups — so the panel is declared once and placed twice rather than
//! spelled out either side. Registry paths follow: `panel_a.organ_type`.
//!
//! Values are raw except where the documentation enumerates them; see [the module
//! docs](super) for what that ceiling is and why.

use super::panel::Panel;
use crate::cbin::{self, Cbin, Header};
use crate::components::{
    sparse_enum, MasterTempo, ProgramCategory, SplitNote, SplitWidth, StageTranspose,
};
use crate::error::Error;
use crate::types::RangedU8;
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
    #[bits(65..=66)]
    pub piano_layer_detune: PianoLayerDetune,
    #[bits(67..=67)]
    pub organ_pitch_stick: bool,
    #[bits(68..=70)]
    pub organ_vibrato_mode: OrganVibratoMode,
    #[bits(71..=71)]
    pub rotary_speaker_speed: bool,
    #[bits(72..=72)]
    pub rotary_speaker_stop_mode: bool,
    #[bits(73..=75)]
    pub rotary_speaker_speed_wheel: RangedU8<7>,
    #[bits(76..=78)]
    pub rotary_speaker_speed_aftertouch: RangedU8<7>,
    #[bits(79..=81)]
    pub rotary_speaker_speed_ctrl_pedal: RangedU8<7>,
    #[bits(96..=96)]
    pub transpose_enabled: bool,
    #[bits(97..=100)]
    pub transpose: StageTranspose,
    #[bits(101..=108)]
    pub master_clock: MasterTempo,
    #[bits(109..=115)]
    pub rotary_speaker_drive: RangedU8<127>,
    #[bits(116..=116)]
    pub dual_keyboard: bool,
    #[bits(118..=119)]
    pub dual_keyboard_style: DualKeyboardStyle,
    #[bits(120..=123)]
    pub synth_pitch_stick_range: RangedU8<15>,

    /// Panel A — the first of the program's two complete setups.
    #[at(22..285)]
    pub panel_a: Panel,

    /// Panel B. Same type: the two are the same layout, and neither is
    /// a copy of the other — `panel_enable` says which sound.
    #[at(285..548)]
    pub panel_b: Panel,
}

/// The preset name, 22 bytes of ASCII padded with NULs.
pub fn synth_preset_name(body: &Program) -> String {
    let raw = <[u8; BODY_LEN]>::from(body);
    String::from_utf8_lossy(&raw[44..66])
        .trim_end_matches('\0')
        .trim_end()
        .to_string()
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

sparse_enum!(
    /// From the `ns3-amp-sim-eq-amp-type` table in the Stage byte-map docs.
    AmpSimEqAmpType, 3, {
        0 => Clean, "Clean";
        1 => Twin, "Twin";
        2 => Jc, "JC";
        3 => Small, "Small";
        4 => Lp24, "LP24";
        5 => Hp24, "HP24";
    }
);

sparse_enum!(
    /// From the `ns3-clavinet-model` table in the Stage byte-map docs.
    ClavinetModel, 2, {
        0 => Ca, "CA";
        1 => Cb, "CB";
        2 => Da, "DA";
        3 => Db, "DB";
    }
);

sparse_enum!(
    /// From the `ns3-organ-kb-zone` table in the Stage byte-map docs.
    OrganKbZone, 4, {
        0 => V0, "o---";
        1 => V1, "-o--";
        2 => V2, "--o-";
        3 => V3, "---o";
        4 => V4, "oo--";
        5 => V5, "-oo-";
        6 => V6, "--oo";
        7 => V7, "ooo-";
        8 => V8, "-ooo";
        9 => V9, "oooo";
    }
);

sparse_enum!(
    /// From the `ns3-organ-type` table in the Stage byte-map docs.
    OrganType, 3, {
        0 => B3, "B3";
        1 => Vox, "Vox";
        2 => Farfisa, "Farfisa";
        3 => Pipe1, "Pipe1";
        4 => Pipe2, "Pipe2";
    }
);

sparse_enum!(
    /// From the `ns3-organ-vibrato-mode` table in the Stage byte-map docs.
    OrganVibratoMode, 3, {
        0 => V0, "V1";
        1 => V1, "C1";
        2 => V2, "V2";
        3 => V3, "C2";
        4 => V4, "V3";
        5 => V5, "C3";
    }
);

sparse_enum!(
    /// From the `ns3-piano-kb-touch` table in the Stage byte-map docs.
    PianoKbTouch, 2, {
        0 => Normal, "Normal";
        1 => KbTouch1, "KB Touch 1";
        2 => Touch2, "Touch 2";
        3 => Touch3, "Touch 3";
    }
);

sparse_enum!(
    /// From the `ns3-piano-layer-detune` table in the Stage byte-map docs.
    PianoLayerDetune, 2, {
        0 => V0, "Off";
        1 => V1, "1";
        2 => V2, "2";
        3 => V3, "3";
    }
);

sparse_enum!(
    /// From the `ns3-piano-timbre` table in the Stage byte-map docs.
    PianoTimbre, 3, {
        0 => None, "None";
        1 => Soft, "Soft";
        2 => Treble, "Treble";
        3 => SoftTreble, "Soft+Treble";
        4 => Brilliant, "Brilliant";
        5 => SoftBrill, "Soft+Brill";
        6 => TrebleBrill, "Treble+Brill";
        7 => SoftTrbBrill, "Soft+Trb+Brill";
    }
);

sparse_enum!(
    /// From the `ns3-piano-type` table in the Stage byte-map docs.
    PianoType, 3, {
        0 => Grand, "Grand";
        1 => Upright, "Upright";
        2 => Electric, "Electric";
        3 => Clav, "Clav";
        4 => Digital, "Digital";
        5 => Misc, "Misc";
    }
);

sparse_enum!(
    /// From the `ns3-synth-amp-env-velocity` table in the Stage byte-map docs.
    SynthAmpEnvVelocity, 2, {
        0 => V0, "Off";
        1 => V1, "1";
        2 => V2, "2";
        3 => V3, "3";
    }
);

sparse_enum!(
    /// From the `ns3-synth-arp-pattern` table in the Stage byte-map docs.
    SynthArpPattern, 2, {
        0 => Up, "Up";
        1 => Down, "Down";
        2 => UpDown, "Up/Down";
        3 => Random, "Random";
    }
);

sparse_enum!(
    /// From the `ns3-synth-arp-range` table in the Stage byte-map docs.
    SynthArpRange, 2, {
        0 => V1Octave, "1 Octave";
        1 => V2Octaves, "2 Octaves";
        2 => V3Octaves, "3 Octaves";
        3 => V4Octaves, "4 Octaves";
    }
);

sparse_enum!(
    /// From the `ns3-synth-filter-drive` table in the Stage byte-map docs.
    SynthFilterDrive, 2, {
        0 => V0, "Off";
        1 => V1, "1";
        2 => V2, "2";
        3 => V3, "3";
    }
);

sparse_enum!(
    /// From the `ns3-synth-filter-kb-track` table in the Stage byte-map docs.
    SynthFilterKbTrack, 2, {
        0 => V0, "Off";
        1 => V1, "1/3";
        2 => V2, "2/3";
        3 => V3, "1";
    }
);

sparse_enum!(
    /// From the `ns3-synth-filter-type` table in the Stage byte-map docs.
    SynthFilterType, 3, {
        0 => Lp12, "LP12";
        1 => Lp24, "LP24";
        2 => MiniMoog, "Mini Moog";
        3 => LpHp, "LP+HP";
        4 => Bp24, "BP24";
        5 => Hp24, "HP24";
    }
);

sparse_enum!(
    /// From the `ns3-synth-lfo-wave` table in the Stage byte-map docs.
    SynthLfoWave, 3, {
        0 => Triangle, "Triangle";
        1 => Saw, "Saw";
        2 => NegSaw, "Neg Saw";
        3 => Square, "Square";
        4 => SH, "S/H";
    }
);

sparse_enum!(
    /// From the `ns3-synth-oscillator-config` table in the Stage byte-map docs.
    SynthOscillatorConfig, 4, {
        0 => None, "None";
        1 => Pitch, "Pitch";
        2 => Shape, "Shape";
        3 => Sync, "Sync";
        4 => Detune, "Detune";
        5 => Mixsin, "MixSin";
        6 => Mixtri, "MixTri";
        7 => Mixsaw, "MixSaw";
        8 => Mixsqr, "MixSqr";
        9 => Mixbell, "MixBell";
        10 => Mixns1, "MixNs1";
        11 => Mixns2, "MixNs2";
        12 => Fm1, "FM1";
        13 => Fm2, "FM2";
        14 => Rm, "RM";
    }
);

sparse_enum!(
    /// From the `ns3-synth-oscillator-type` table in the Stage byte-map docs.
    SynthOscillatorType, 3, {
        0 => Classic, "Classic";
        1 => Wave, "Wave";
        2 => Formant, "Formant";
        3 => Super, "Super";
        4 => Sample, "Sample";
    }
);

sparse_enum!(
    /// From the `ns3-synth-unison` table in the Stage byte-map docs.
    SynthUnison, 2, {
        0 => V0, "Off";
        1 => V1, "1";
        2 => V2, "2";
        3 => V3, "3";
    }
);

sparse_enum!(
    /// From the `ns3-synth-vibrato` table in the Stage byte-map docs.
    SynthVibrato, 3, {
        0 => Off, "Off";
        1 => Delay1, "Delay 1";
        2 => Delay2, "Delay 2";
        3 => Delay3, "Delay 3";
        4 => Wheel, "Wheel";
        5 => AfterTouch, "After Touch";
    }
);

sparse_enum!(
    /// From the `ns3-synth-voice` table in the Stage byte-map docs.
    SynthVoice, 2, {
        0 => Poly, "Poly";
        1 => Legato, "Legato";
        2 => Mono, "Mono";
    }
);
