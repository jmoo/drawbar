//! The Stage 4 piano preset body (`.ns4n`): 151 bytes.
//!
//! One piano section as a program stores it, moved down 180 bytes, without the
//! keyboard zone — that belongs to the program that loads the preset.

use super::fx::FxChain;
use crate::cbin::{self, Cbin};
use crate::components::{KbZone4, Level, LibraryRef, MorphTarget, OctaveShiftNibble, Selector};
use crate::error::Error;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns4n";
/// Stored ×100. The corpus holds 2.03; ns4decode was tested on 2.01.
pub const KNOWN_VERSIONS: &[u32] = &[201, 202, 203];
pub const BODY_LEN: usize = 151;

#[nord_bits_derive::bitbody(151)]
pub struct PianoPreset {
    #[bits(41..=41)]
    pub piano_b_layer_enabled: bool,
    #[bits(42..=42)]
    pub piano_a_layer_enabled: bool,
    #[bits(44..=44)]
    pub piano_b_layer_enabled_scene_2: bool,
    #[bits(45..=45)]
    pub piano_a_layer_enabled_scene_2: bool,
    #[bits(46..=52)]
    pub piano_a_volume: Level,
    #[bits(53..=60)]
    pub piano_a_volume_wheel: MorphTarget,
    #[bits(61..=68)]
    pub piano_a_volume_aftertouch: MorphTarget,
    #[bits(69..=76)]
    pub piano_a_volume_ctrl_pedal: MorphTarget,
    #[bits(77..=83)]
    pub piano_b_volume: Level,
    #[bits(84..=91)]
    pub piano_b_volume_wheel: MorphTarget,
    #[bits(92..=99)]
    pub piano_b_volume_aftertouch: MorphTarget,
    #[bits(100..=107)]
    pub piano_b_volume_ctrl_pedal: MorphTarget,
    #[bits(148..=151)]
    pub piano_a_octave_shift: OctaveShiftNibble,
    #[bits(152..=152)]
    pub piano_a_pitch_stick_enabled: bool,
    #[bits(153..=153)]
    pub piano_a_sustain_pedal_enabled: bool,
    #[bits(154..=156)]
    pub piano_a_type: Selector<3>,
    #[bits(157..=161)]
    pub piano_a_model_slot: Selector<5>,
    #[bits(162..=163)]
    pub piano_a_model_variation: Selector<2>,
    #[bits(164..=195)]
    pub piano_a_model_id: LibraryRef,
    #[bits(196..=196)]
    pub piano_a_soft_rel_enabled: bool,
    #[bits(197..=197)]
    pub piano_a_string_res_enabled: bool,
    #[bits(198..=198)]
    pub piano_a_pedal_noise_enabled: bool,
    #[bits(199..=200)]
    pub piano_a_touch: Selector<2>,
    #[bits(201..=202)]
    pub piano_a_unison_level: Selector<2>,
    #[bits(203..=204)]
    pub piano_a_dyn_comp: Selector<2>,
    #[bits(206..=208)]
    pub piano_a_timbre: Selector<3>,
    #[bits(240..=243)]
    pub piano_b_kb_zones: KbZone4,
    #[bits(244..=247)]
    pub piano_b_octave_shift: OctaveShiftNibble,
    #[bits(248..=248)]
    pub piano_b_pitch_stick_enabled: bool,
    #[bits(249..=249)]
    pub piano_b_sustain_pedal_enabled: bool,
    #[bits(250..=252)]
    pub piano_b_type: Selector<3>,
    #[bits(253..=257)]
    pub piano_b_model_slot: Selector<5>,
    #[bits(258..=259)]
    pub piano_b_model_variation: Selector<2>,
    #[bits(260..=291)]
    pub piano_b_model_id: LibraryRef,
    #[bits(292..=292)]
    pub piano_b_soft_rel_enabled: bool,
    #[bits(293..=293)]
    pub piano_b_string_res_enabled: bool,
    #[bits(294..=294)]
    pub piano_b_pedal_noise_enabled: bool,
    #[bits(295..=296)]
    pub piano_b_touch: Selector<2>,
    #[bits(297..=298)]
    pub piano_b_unison_level: Selector<2>,
    #[bits(299..=300)]
    pub piano_b_dyn_comp: Selector<2>,
    #[bits(302..=304)]
    pub piano_b_timbre: Selector<3>,

    /// The piano a section's effects chain.
    #[at(42..94)]
    pub piano_a_fx: FxChain,
    /// The piano b section's effects chain.
    #[at(97..149)]
    pub piano_b_fx: FxChain,
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<PianoPreset>, Error> {
    let file: Cbin<PianoPreset> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
