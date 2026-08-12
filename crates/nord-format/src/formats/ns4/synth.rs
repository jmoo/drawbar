//! The Stage 4 synth preset body (`.ns4y`): 497 bytes.
//!
//! One synth section as a program stores it, moved down 327 bytes, without the
//! two placements a preset has no use for — the keyboard zone, which belongs to
//! the program that loads it, and the extern CC values.

use super::fx::FxChain;
use super::synth_voice::SynthVoice;
use crate::cbin::{self, Cbin};
use crate::components::{
    ArpPattern, KbZone4, Level, LibraryRef, MorphTarget, OctaveShiftNibble, Pan, Selector, Time,
};
use crate::error::Error;
use crate::types::{RangedU16, RangedU8};
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns4y";
/// Stored ×100. The corpus holds 2.08; ns4decode was tested from 2.03.
pub const KNOWN_VERSIONS: &[u32] = &[203, 204, 205, 206, 207, 208];
pub const BODY_LEN: usize = 497;

#[nord_bits_derive::bitbody(497)]
pub struct SynthPreset {
    #[bits(42..=42)]
    pub synth_c_layer_enabled: bool,
    #[bits(43..=43)]
    pub synth_b_layer_enabled: bool,
    #[bits(44..=44)]
    pub synth_a_layer_enabled: bool,
    #[bits(47..=47)]
    pub synth_c_layer_enabled_scene_2: bool,
    #[bits(48..=48)]
    pub synth_b_layer_enabled_scene_2: bool,
    #[bits(49..=49)]
    pub synth_a_layer_enabled_scene_2: bool,
    #[bits(50..=56)]
    pub synth_a_volume: Level,
    #[bits(57..=64)]
    pub synth_a_volume_wheel: MorphTarget,
    #[bits(65..=72)]
    pub synth_a_volume_aftertouch: MorphTarget,
    #[bits(73..=80)]
    pub synth_a_volume_ctrl_pedal: MorphTarget,
    #[bits(81..=87)]
    pub synth_b_volume: Level,
    #[bits(88..=95)]
    pub synth_b_volume_wheel: MorphTarget,
    #[bits(96..=103)]
    pub synth_b_volume_aftertouch: MorphTarget,
    #[bits(104..=111)]
    pub synth_b_volume_ctrl_pedal: MorphTarget,
    #[bits(112..=118)]
    pub synth_c_volume: Level,
    #[bits(119..=126)]
    pub synth_c_volume_wheel: MorphTarget,
    #[bits(127..=134)]
    pub synth_c_volume_aftertouch: MorphTarget,
    #[bits(135..=142)]
    pub synth_c_volume_ctrl_pedal: MorphTarget,
    #[bits(143..=148)]
    pub synth_a_pan: Pan,
    #[bits(174..=179)]
    pub synth_b_pan: Pan,
    #[bits(205..=210)]
    pub synth_c_pan: Pan,
    #[bits(288..=288)]
    pub synth_a_samples_analog: bool,
    #[bits(294..=305)]
    pub synth_a_sample_slot: RangedU16<4095>,
    #[bits(306..=337)]
    pub synth_a_sample_id: LibraryRef,
    #[bits(342..=345)]
    pub synth_a_octave_shift: OctaveShiftNibble,
    #[bits(346..=346)]
    pub synth_a_pitch_stick_enabled: bool,
    #[bits(347..=350)]
    pub synth_a_pitch_stick_range: Selector<4>,
    #[bits(351..=351)]
    pub synth_a_sustain_pedal_enabled: bool,
    #[bits(352..=354)]
    pub synth_a_vibrato_mode: Selector<3>,
    #[bits(357..=357)]
    pub synth_a_legato_enabled: bool,
    #[bits(358..=358)]
    pub synth_a_mono_enabled: bool,
    #[bits(359..=360)]
    pub synth_a_voice_priority: Selector<2>,
    #[bits(361..=367)]
    pub synth_a_glide: Time,
    #[bits(368..=368)]
    pub synth_a_extern_enabled: bool,
    #[bits(399..=405)]
    pub synth_a_extern_program: Level,
    #[bits(414..=414)]
    pub synth_a_kb_hold: bool,
    #[bits(415..=415)]
    pub synth_a_arpeggiator_run_enabled: bool,
    #[bits(416..=417)]
    pub synth_a_arpeggiator_mode: Selector<2>,
    #[bits(418..=418)]
    pub synth_a_arp_pattern_enabled: bool,
    #[bits(419..=419)]
    pub synth_a_kb_sync_enabled: bool,
    #[bits(420..=426)]
    pub synth_a_arp_range_env: Level,
    #[bits(427..=434)]
    pub synth_a_arp_range_env_wheel: MorphTarget,
    #[bits(435..=442)]
    pub synth_a_arp_range_env_aftertouch: MorphTarget,
    #[bits(443..=450)]
    pub synth_a_arp_range_env_ctrl_pedal: MorphTarget,
    #[bits(451..=452)]
    pub synth_a_arp_direction: Selector<2>,
    #[bits(453..=453)]
    pub synth_a_arp_zigzag_enabled: bool,
    #[bits(454..=454)]
    pub synth_a_arp_master_clock_enabled: bool,
    #[bits(455..=461)]
    pub synth_a_arp_rate_time: Time,
    #[bits(462..=469)]
    pub synth_a_arp_rate_time_wheel: MorphTarget,
    #[bits(470..=477)]
    pub synth_a_arp_rate_time_aftertouch: MorphTarget,
    #[bits(478..=485)]
    pub synth_a_arp_rate_time_ctrl_pedal: MorphTarget,
    #[bits(486..=489)]
    pub synth_a_arp_pattern_length: RangedU8<15>,
    #[bits(490..=521)]
    pub synth_a_arpeggiator_accent: ArpPattern,
    #[bits(522..=553)]
    pub synth_a_arpeggiator_gate: ArpPattern,
    #[bits(554..=585)]
    pub synth_a_arpeggiator_pan: ArpPattern,
    #[bits(586..=587)]
    pub synth_a_unison_level: Selector<2>,
    #[bits(652..=656)]
    pub synth_a_vibrato_delay: Selector<5>,
    #[bits(696..=696)]
    pub synth_b_samples_analog: bool,
    #[bits(702..=713)]
    pub synth_b_sample_slot: RangedU16<4095>,
    #[bits(714..=745)]
    pub synth_b_sample_id: LibraryRef,
    #[bits(746..=749)]
    pub synth_b_kb_zones: KbZone4,
    #[bits(750..=753)]
    pub synth_b_octave_shift: OctaveShiftNibble,
    #[bits(754..=754)]
    pub synth_b_pitch_stick_enabled: bool,
    #[bits(755..=758)]
    pub synth_b_pitch_stick_range: Selector<4>,
    #[bits(759..=759)]
    pub synth_b_sustain_pedal_enabled: bool,
    #[bits(760..=762)]
    pub synth_b_vibrato_mode: Selector<3>,
    #[bits(765..=765)]
    pub synth_b_legato_enabled: bool,
    #[bits(766..=766)]
    pub synth_b_mono_enabled: bool,
    #[bits(767..=768)]
    pub synth_b_voice_priority: Selector<2>,
    #[bits(769..=775)]
    pub synth_b_glide: Time,
    #[bits(776..=776)]
    pub synth_b_extern_enabled: bool,
    #[bits(807..=813)]
    pub synth_b_extern_program: Level,
    #[bits(822..=822)]
    pub synth_b_kb_hold: bool,
    #[bits(823..=823)]
    pub synth_b_arpeggiator_run_enabled: bool,
    #[bits(824..=825)]
    pub synth_b_arpeggiator_mode: Selector<2>,
    #[bits(826..=826)]
    pub synth_b_arp_pattern_enabled: bool,
    #[bits(827..=827)]
    pub synth_b_kb_sync_enabled: bool,
    #[bits(828..=834)]
    pub synth_b_arp_range_env: Level,
    #[bits(835..=842)]
    pub synth_b_arp_range_env_wheel: MorphTarget,
    #[bits(843..=850)]
    pub synth_b_arp_range_env_aftertouch: MorphTarget,
    #[bits(851..=858)]
    pub synth_b_arp_range_env_ctrl_pedal: MorphTarget,
    #[bits(859..=860)]
    pub synth_b_arp_direction: Selector<2>,
    #[bits(861..=861)]
    pub synth_b_arp_zigzag_enabled: bool,
    #[bits(862..=862)]
    pub synth_b_arp_master_clock_enabled: bool,
    #[bits(863..=869)]
    pub synth_b_arp_rate_time: Time,
    #[bits(870..=877)]
    pub synth_b_arp_rate_time_wheel: MorphTarget,
    #[bits(878..=885)]
    pub synth_b_arp_rate_time_aftertouch: MorphTarget,
    #[bits(886..=893)]
    pub synth_b_arp_rate_time_ctrl_pedal: MorphTarget,
    #[bits(894..=897)]
    pub synth_b_arp_pattern_length: RangedU8<15>,
    #[bits(898..=929)]
    pub synth_b_arpeggiator_accent: ArpPattern,
    #[bits(930..=961)]
    pub synth_b_arpeggiator_gate: ArpPattern,
    #[bits(962..=993)]
    pub synth_b_arpeggiator_pan: ArpPattern,
    #[bits(994..=995)]
    pub synth_b_unison_level: Selector<2>,
    #[bits(996..=1002)]
    pub synth_b_extern_cc_val1: Level,
    #[bits(1003..=1010)]
    pub synth_b_extern_cc_val1_wheel: MorphTarget,
    #[bits(1011..=1018)]
    pub synth_b_extern_cc_val1_aftertouch: MorphTarget,
    #[bits(1019..=1026)]
    pub synth_b_extern_cc_val1_ctrl_pedal: MorphTarget,
    #[bits(1027..=1033)]
    pub synth_b_extern_cc_val2: Level,
    #[bits(1034..=1041)]
    pub synth_b_extern_cc_val2_wheel: MorphTarget,
    #[bits(1042..=1049)]
    pub synth_b_extern_cc_val2_aftertouch: MorphTarget,
    #[bits(1050..=1057)]
    pub synth_b_extern_cc_val2_ctrl_pedal: MorphTarget,
    #[bits(1060..=1064)]
    pub synth_b_vibrato_delay: Selector<5>,
    #[bits(1104..=1104)]
    pub synth_c_samples_analog: bool,
    #[bits(1110..=1121)]
    pub synth_c_sample_slot: RangedU16<4095>,
    #[bits(1122..=1153)]
    pub synth_c_sample_id: LibraryRef,
    #[bits(1154..=1157)]
    pub synth_c_kb_zones: KbZone4,
    #[bits(1158..=1161)]
    pub synth_c_octave_shift: OctaveShiftNibble,
    #[bits(1162..=1162)]
    pub synth_c_pitch_stick_enabled: bool,
    #[bits(1163..=1166)]
    pub synth_c_pitch_stick_range: Selector<4>,
    #[bits(1167..=1167)]
    pub synth_c_sustain_pedal_enabled: bool,
    #[bits(1168..=1170)]
    pub synth_c_vibrato_mode: Selector<3>,
    #[bits(1173..=1173)]
    pub synth_c_legato_enabled: bool,
    #[bits(1174..=1174)]
    pub synth_c_mono_enabled: bool,
    #[bits(1175..=1176)]
    pub synth_c_voice_priority: Selector<2>,
    #[bits(1177..=1183)]
    pub synth_c_glide: Time,
    #[bits(1184..=1184)]
    pub synth_c_extern_enabled: bool,
    #[bits(1215..=1221)]
    pub synth_c_extern_program: Level,
    #[bits(1230..=1230)]
    pub synth_c_kb_hold: bool,
    #[bits(1231..=1231)]
    pub synth_c_arpeggiator_run_enabled: bool,
    #[bits(1232..=1233)]
    pub synth_c_arpeggiator_mode: Selector<2>,
    #[bits(1234..=1234)]
    pub synth_c_arp_pattern_enabled: bool,
    #[bits(1235..=1235)]
    pub synth_c_kb_sync_enabled: bool,
    #[bits(1236..=1242)]
    pub synth_c_arp_range_env: Level,
    #[bits(1243..=1250)]
    pub synth_c_arp_range_env_wheel: MorphTarget,
    #[bits(1251..=1258)]
    pub synth_c_arp_range_env_aftertouch: MorphTarget,
    #[bits(1259..=1266)]
    pub synth_c_arp_range_env_ctrl_pedal: MorphTarget,
    #[bits(1267..=1268)]
    pub synth_c_arp_direction: Selector<2>,
    #[bits(1269..=1269)]
    pub synth_c_arp_zigzag_enabled: bool,
    #[bits(1270..=1270)]
    pub synth_c_arp_master_clock_enabled: bool,
    #[bits(1271..=1277)]
    pub synth_c_arp_rate_time: Time,
    #[bits(1278..=1285)]
    pub synth_c_arp_rate_time_wheel: MorphTarget,
    #[bits(1286..=1293)]
    pub synth_c_arp_rate_time_aftertouch: MorphTarget,
    #[bits(1294..=1301)]
    pub synth_c_arp_rate_time_ctrl_pedal: MorphTarget,
    #[bits(1302..=1305)]
    pub synth_c_arp_pattern_length: RangedU8<15>,
    #[bits(1306..=1337)]
    pub synth_c_arpeggiator_accent: ArpPattern,
    #[bits(1338..=1369)]
    pub synth_c_arpeggiator_gate: ArpPattern,
    #[bits(1370..=1401)]
    pub synth_c_arpeggiator_pan: ArpPattern,
    #[bits(1402..=1403)]
    pub synth_c_unison_level: Selector<2>,
    #[bits(1404..=1410)]
    pub synth_c_extern_cc_val1: Level,
    #[bits(1411..=1418)]
    pub synth_c_extern_cc_val1_wheel: MorphTarget,
    #[bits(1419..=1426)]
    pub synth_c_extern_cc_val1_aftertouch: MorphTarget,
    #[bits(1427..=1434)]
    pub synth_c_extern_cc_val1_ctrl_pedal: MorphTarget,
    #[bits(1435..=1441)]
    pub synth_c_extern_cc_val2: Level,
    #[bits(1442..=1449)]
    pub synth_c_extern_cc_val2_wheel: MorphTarget,
    #[bits(1450..=1457)]
    pub synth_c_extern_cc_val2_aftertouch: MorphTarget,
    #[bits(1458..=1465)]
    pub synth_c_extern_cc_val2_ctrl_pedal: MorphTarget,
    #[bits(1468..=1472)]
    pub synth_c_vibrato_delay: Selector<5>,

    #[at(189..233)]
    pub synth_a_voice: SynthVoice,
    #[at(237..281)]
    pub synth_b_voice: SynthVoice,
    #[at(285..329)]
    pub synth_c_voice: SynthVoice,
    /// The synth a section's effects chain.
    #[at(333..385)]
    pub synth_a_fx: FxChain,
    /// The synth b section's effects chain.
    #[at(388..440)]
    pub synth_b_fx: FxChain,
    /// The synth c section's effects chain.
    #[at(443..495)]
    pub synth_c_fx: FxChain,
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<SynthPreset>, Error> {
    let file: Cbin<SynthPreset> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
