//! The Stage 4 program body (`.ns4p`, `.ns4l`): 824 bytes, every parameter placed.
//!
//! All three sections and the globals that route them. Placements, naming and
//! provenance are the [module docs](super); values are raw.

use super::fx::FxChain;
use super::organ_layers::OrganLayer;
use super::piano_layers::PianoLayer;
use super::synth_performance::SynthPerformance;
use super::synth_voice::SynthVoice;
use crate::cbin::{self, Cbin};
use crate::error::Error;
use crate::types::RangedU8;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns4p";
/// Schema versions this build's field offsets have been validated against,
/// stored ×100. The corpus holds 3.13 throughout; ns4decode reports the same
/// offsets back to 3.04, changing only how three values are *read*, which this
/// decode does not do.
pub const KNOWN_VERSIONS: &[u32] = &[304, 305, 306, 307, 308, 309, 310, 311, 312, 313];
pub const BODY_LEN: usize = 824;

#[nord_bits_derive::bitbody(824)]
pub struct Program {
    #[bits(24..=31)]
    pub version_echo: u8,
    #[bits(40..=40)]
    pub split_enabled: bool,
    #[bits(41..=41)]
    pub kb_zones_1_2_split_point_enabled: bool,
    #[bits(42..=42)]
    pub kb_zones_2_3_split_point_enabled: bool,
    #[bits(43..=43)]
    pub kb_zones_3_4_split_point_enabled: bool,
    #[bits(44..=47)]
    pub kb_zones_1_2_split_point: RangedU8<15>,
    #[bits(48..=51)]
    pub kb_zones_2_3_split_point: RangedU8<15>,
    #[bits(52..=55)]
    pub kb_zones_3_4_split_point: RangedU8<15>,
    #[bits(56..=57)]
    pub kb_zones_1_2_split_point_xfade: RangedU8<3>,
    #[bits(58..=59)]
    pub kb_zones_2_3_split_point_xfade: RangedU8<3>,
    #[bits(60..=61)]
    pub kb_zones_3_4_split_point_xfade: RangedU8<3>,
    #[bits(62..=62)]
    pub program_transpose_enabled: bool,
    #[bits(63..=66)]
    pub program_transpose_amount: RangedU8<15>,
    #[bits(79..=79)]
    pub fx_comp_global_enabled: bool,
    #[bits(80..=80)]
    pub fx_delay_global_enabled: bool,
    #[bits(81..=81)]
    pub fx_reverb_global_enabled: bool,
    #[bits(316..=316)]
    pub organ_section_enabled: bool,
    #[bits(317..=317)]
    pub piano_section_enabled: bool,
    #[bits(318..=318)]
    pub synth_section_enabled: bool,
    #[bits(319..=319)]
    pub organ_section_enabled_scene_2: bool,
    #[bits(320..=320)]
    pub piano_section_enabled_scene_2: bool,
    #[bits(321..=321)]
    pub synth_section_enabled_scene_2: bool,
    #[bits(322..=322)]
    pub active_layer_scene: bool,
    #[bits(347..=347)]
    pub fx_enabled: bool,
    #[bits(401..=401)]
    pub organ_b_layer_enabled: bool,
    #[bits(402..=402)]
    pub organ_a_layer_enabled: bool,
    #[bits(404..=404)]
    pub organ_b_layer_enabled_scene_2: bool,
    #[bits(405..=405)]
    pub organ_a_layer_enabled_scene_2: bool,
    #[bits(406..=412)]
    pub organ_a_volume: RangedU8<127>,
    #[bits(413..=420)]
    pub organ_a_volume_wheel: u8,
    #[bits(421..=428)]
    pub organ_a_volume_aftertouch: u8,
    #[bits(429..=436)]
    pub organ_a_volume_ctrl_pedal: u8,
    #[bits(437..=443)]
    pub organ_b_volume: RangedU8<127>,
    #[bits(444..=451)]
    pub organ_b_volume_wheel: u8,
    #[bits(452..=459)]
    pub organ_b_volume_aftertouch: u8,
    #[bits(460..=467)]
    pub organ_b_volume_ctrl_pedal: u8,
    #[bits(468..=468)]
    pub organ_pitch_stick_enabled: bool,
    #[bits(472..=472)]
    pub organ_rotary_speaker_enabled: bool,
    #[bits(484..=490)]
    pub rotary_speaker_drive: RangedU8<127>,
    #[bits(508..=510)]
    pub rotary_speaker_stop_position: RangedU8<7>,
    #[bits(514..=514)]
    pub rotary_speaker_stop_enabled: bool,
    #[bits(515..=515)]
    pub rotary_speaker_slow_fast: bool,
    #[bits(522..=528)]
    pub organ_vib_chorus_type: RangedU8<127>,
    #[bits(1481..=1481)]
    pub piano_b_layer_enabled: bool,
    #[bits(1482..=1482)]
    pub piano_a_layer_enabled: bool,
    #[bits(1484..=1484)]
    pub piano_b_layer_enabled_scene_2: bool,
    #[bits(1485..=1485)]
    pub piano_a_layer_enabled_scene_2: bool,
    #[bits(1486..=1492)]
    pub piano_a_volume: RangedU8<127>,
    #[bits(1493..=1500)]
    pub piano_a_volume_wheel: u8,
    #[bits(1501..=1508)]
    pub piano_a_volume_aftertouch: u8,
    #[bits(1509..=1516)]
    pub piano_a_volume_ctrl_pedal: u8,
    #[bits(1517..=1523)]
    pub piano_b_volume: RangedU8<127>,
    #[bits(1524..=1531)]
    pub piano_b_volume_wheel: u8,
    #[bits(1532..=1539)]
    pub piano_b_volume_aftertouch: u8,
    #[bits(1540..=1547)]
    pub piano_b_volume_ctrl_pedal: u8,
    #[bits(2658..=2658)]
    pub synth_c_layer_enabled: bool,
    #[bits(2659..=2659)]
    pub synth_b_layer_enabled: bool,
    #[bits(2660..=2660)]
    pub synth_a_layer_enabled: bool,
    #[bits(2663..=2663)]
    pub synth_c_layer_enabled_scene_2: bool,
    #[bits(2664..=2664)]
    pub synth_b_layer_enabled_scene_2: bool,
    #[bits(2665..=2665)]
    pub synth_a_layer_enabled_scene_2: bool,
    #[bits(2666..=2672)]
    pub synth_a_volume: RangedU8<127>,
    #[bits(2673..=2680)]
    pub synth_a_volume_wheel: u8,
    #[bits(2681..=2688)]
    pub synth_a_volume_aftertouch: u8,
    #[bits(2689..=2696)]
    pub synth_a_volume_ctrl_pedal: u8,
    #[bits(2697..=2703)]
    pub synth_b_volume: RangedU8<127>,
    #[bits(2704..=2711)]
    pub synth_b_volume_wheel: u8,
    #[bits(2712..=2719)]
    pub synth_b_volume_aftertouch: u8,
    #[bits(2720..=2727)]
    pub synth_b_volume_ctrl_pedal: u8,
    #[bits(2728..=2734)]
    pub synth_c_volume: RangedU8<127>,
    #[bits(2735..=2742)]
    pub synth_c_volume_wheel: u8,
    #[bits(2743..=2750)]
    pub synth_c_volume_aftertouch: u8,
    #[bits(2751..=2758)]
    pub synth_c_volume_ctrl_pedal: u8,
    #[bits(2759..=2764)]
    pub synth_a_pan: RangedU8<63>,
    #[bits(2790..=2795)]
    pub synth_b_pan: RangedU8<63>,
    #[bits(2821..=2826)]
    pub synth_c_pan: RangedU8<63>,
    #[bits(2853..=2853)]
    pub synth_arp_group_enabled: bool,
    #[bits(2855..=2855)]
    pub synth_kb_hold_enabled: bool,

    #[at(68..97)]
    pub organ_a: OrganLayer,
    #[at(99..128)]
    pub organ_b: OrganLayer,
    /// The organ section's effects chain.
    #[at(130..182)]
    pub organ_fx: FxChain,
    #[at(198..207)]
    pub piano_a: PianoLayer,
    #[at(210..219)]
    pub piano_b: PianoLayer,
    /// The piano a section's effects chain.
    #[at(222..274)]
    pub piano_a_fx: FxChain,
    /// The piano b section's effects chain.
    #[at(277..329)]
    pub piano_b_fx: FxChain,
    #[at(363..410)]
    pub synth_a_performance: SynthPerformance,
    #[at(414..461)]
    pub synth_b_performance: SynthPerformance,
    #[at(465..512)]
    pub synth_c_performance: SynthPerformance,
    #[at(516..560)]
    pub synth_a_voice: SynthVoice,
    #[at(564..608)]
    pub synth_b_voice: SynthVoice,
    #[at(612..656)]
    pub synth_c_voice: SynthVoice,
    /// The synth a section's effects chain.
    #[at(660..712)]
    pub synth_a_fx: FxChain,
    /// The synth b section's effects chain.
    #[at(715..767)]
    pub synth_b_fx: FxChain,
    /// The synth c section's effects chain.
    #[at(770..822)]
    pub synth_c_fx: FxChain,
}

/// The `(bank, location)` pair from the header, uninterpreted: bank 0..=5 for
/// the six program banks and location 0..=63 on current exports. Not validated
/// — see the Stage 3's note on out-of-range locations in old files.
pub fn location(file: &Cbin<Program>) -> (u16, u16) {
    file.header.slot()
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
    let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
