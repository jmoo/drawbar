//! The Stage 4 synth preset body (`.ns4y`): 497 bytes.
//!
//! One synth section as a program stores it, moved down 327 bytes. The keyboard
//! zone and the extern CC values ride along for every layer, though a preset has
//! no use for them. Layers are 408 bits apart; layer A's zone is confirmed
//! against the corpus (it varies exactly as B's and C's do), its extern CC block
//! is placed by the stride alone, since no specimen sets any layer's.

use super::fx::FxChain;
use super::synth_performance::SynthPerformance;
use super::synth_voice::SynthVoice;
use crate::cbin::{self, Cbin};
use crate::components::{Level, MorphTarget, Pan};
use crate::error::Error;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns4y";
/// Stored ×100. The corpus holds 2.08; ns4decode was tested from 2.03.
pub const KNOWN_VERSIONS: &[u32] = &[203, 204, 205, 206, 207, 208];
pub const BODY_LEN: usize = 497;

/// The 497-byte preset body: one synth section as a program stores it.
///
/// Reads and writes byte-exactly. A read verifies the container checksum, gates
/// on [`KNOWN_VERSIONS`], and range-checks every field; unclaimed bits survive a
/// re-encode verbatim. Placements derived from ns4decode's published tables;
/// values raw. Inferred from specimens; not confirmed on hardware.
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

    #[at(36..83)]
    pub synth_a_performance: SynthPerformance,
    #[at(87..134)]
    pub synth_b_performance: SynthPerformance,
    #[at(138..185)]
    pub synth_c_performance: SynthPerformance,
    #[at(189..233)]
    pub synth_a_voice: SynthVoice,
    #[at(237..281)]
    pub synth_b_voice: SynthVoice,
    #[at(285..329)]
    pub synth_c_voice: SynthVoice,
    #[at(333..385)]
    pub synth_a_fx: FxChain,
    #[at(388..440)]
    pub synth_b_fx: FxChain,
    #[at(443..495)]
    pub synth_c_fx: FxChain,
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<SynthPreset>, Error> {
    let file: Cbin<SynthPreset> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
