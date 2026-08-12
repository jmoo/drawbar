//! The Stage 4 organ preset body (`.ns4o`): 139 bytes.
//!
//! One organ section as a program stores it, moved down 45 bytes, without the
//! keyboard zone — that belongs to the program that loads the preset. The two
//! layers still share one effects chain, so its fields carry no layer.

use super::fx::FxChain;
use crate::cbin::{self, Cbin};
use crate::error::Error;
use crate::types::RangedU8;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns4o";
/// Stored ×100. The corpus holds 2.05; ns4decode was tested on 2.01.
pub const KNOWN_VERSIONS: &[u32] = &[201, 202, 203, 204, 205];
pub const BODY_LEN: usize = 139;

#[nord_bits_derive::bitbody(139)]
pub struct OrganPreset {
    #[bits(41..=41)]
    pub organ_b_layer_enabled: bool,
    #[bits(42..=42)]
    pub organ_a_layer_enabled: bool,
    #[bits(44..=44)]
    pub organ_b_layer_enabled_scene_2: bool,
    #[bits(45..=45)]
    pub organ_a_layer_enabled_scene_2: bool,
    #[bits(46..=52)]
    pub organ_a_volume: RangedU8<127>,
    #[bits(53..=60)]
    pub organ_a_volume_wheel: u8,
    #[bits(61..=68)]
    pub organ_a_volume_aftertouch: u8,
    #[bits(69..=76)]
    pub organ_a_volume_ctrl_pedal: u8,
    #[bits(77..=83)]
    pub organ_b_volume: RangedU8<127>,
    #[bits(84..=91)]
    pub organ_b_volume_wheel: u8,
    #[bits(92..=99)]
    pub organ_b_volume_aftertouch: u8,
    #[bits(100..=107)]
    pub organ_b_volume_ctrl_pedal: u8,
    #[bits(188..=191)]
    pub organ_a_octave_shift: RangedU8<15>,
    #[bits(192..=192)]
    pub organ_a_sustain_pedal_enabled: bool,
    #[bits(193..=195)]
    pub organ_a_model: RangedU8<7>,
    #[bits(196..=196)]
    pub organ_a_preset_enabled: bool,
    #[bits(216..=219)]
    pub organ_a_drawbar_1: RangedU8<15>,
    #[bits(220..=224)]
    pub organ_a_drawbar_1_wheel: RangedU8<31>,
    #[bits(225..=229)]
    pub organ_a_drawbar_1_aftertouch: RangedU8<31>,
    #[bits(230..=234)]
    pub organ_a_drawbar_1_ctrl_pedal: RangedU8<31>,
    #[bits(235..=238)]
    pub organ_a_drawbar_2: RangedU8<15>,
    #[bits(239..=243)]
    pub organ_a_drawbar_2_wheel: RangedU8<31>,
    #[bits(244..=248)]
    pub organ_a_drawbar_2_aftertouch: RangedU8<31>,
    #[bits(249..=253)]
    pub organ_a_drawbar_2_ctrl_pedal: RangedU8<31>,
    #[bits(254..=257)]
    pub organ_a_drawbar_3: RangedU8<15>,
    #[bits(258..=262)]
    pub organ_a_drawbar_3_wheel: RangedU8<31>,
    #[bits(263..=267)]
    pub organ_a_drawbar_3_aftertouch: RangedU8<31>,
    #[bits(268..=272)]
    pub organ_a_drawbar_3_ctrl_pedal: RangedU8<31>,
    #[bits(273..=276)]
    pub organ_a_drawbar_4: RangedU8<15>,
    #[bits(277..=281)]
    pub organ_a_drawbar_4_wheel: RangedU8<31>,
    #[bits(282..=286)]
    pub organ_a_drawbar_4_aftertouch: RangedU8<31>,
    #[bits(287..=291)]
    pub organ_a_drawbar_4_ctrl_pedal: RangedU8<31>,
    #[bits(292..=295)]
    pub organ_a_drawbar_5: RangedU8<15>,
    #[bits(296..=300)]
    pub organ_a_drawbar_5_wheel: RangedU8<31>,
    #[bits(301..=305)]
    pub organ_a_drawbar_5_aftertouch: RangedU8<31>,
    #[bits(306..=310)]
    pub organ_a_drawbar_5_ctrl_pedal: RangedU8<31>,
    #[bits(311..=314)]
    pub organ_a_drawbar_6: RangedU8<15>,
    #[bits(315..=319)]
    pub organ_a_drawbar_6_wheel: RangedU8<31>,
    #[bits(320..=324)]
    pub organ_a_drawbar_6_aftertouch: RangedU8<31>,
    #[bits(325..=329)]
    pub organ_a_drawbar_6_ctrl_pedal: RangedU8<31>,
    #[bits(330..=333)]
    pub organ_a_drawbar_7: RangedU8<15>,
    #[bits(334..=338)]
    pub organ_a_drawbar_7_wheel: RangedU8<31>,
    #[bits(339..=343)]
    pub organ_a_drawbar_7_aftertouch: RangedU8<31>,
    #[bits(344..=348)]
    pub organ_a_drawbar_7_ctrl_pedal: RangedU8<31>,
    #[bits(349..=352)]
    pub organ_a_drawbar_8: RangedU8<15>,
    #[bits(353..=357)]
    pub organ_a_drawbar_8_wheel: RangedU8<31>,
    #[bits(358..=362)]
    pub organ_a_drawbar_8_aftertouch: RangedU8<31>,
    #[bits(363..=367)]
    pub organ_a_drawbar_8_ctrl_pedal: RangedU8<31>,
    #[bits(368..=371)]
    pub organ_a_drawbar_9: RangedU8<15>,
    #[bits(372..=376)]
    pub organ_a_drawbar_9_wheel: RangedU8<31>,
    #[bits(377..=381)]
    pub organ_a_drawbar_9_aftertouch: RangedU8<31>,
    #[bits(382..=386)]
    pub organ_a_drawbar_9_ctrl_pedal: RangedU8<31>,
    #[bits(408..=408)]
    pub organ_a_vib_chorus_enabled: bool,
    #[bits(409..=409)]
    pub organ_a_percussion_enabled: bool,
    #[bits(410..=410)]
    pub organ_a_percussion_harmonic_3rd_enabled: bool,
    #[bits(411..=411)]
    pub organ_a_percussion_decay_fast_enabled: bool,
    #[bits(412..=412)]
    pub organ_a_percussion_volume_soft_enabled: bool,
    #[bits(432..=435)]
    pub organ_b_kb_zones: RangedU8<15>,
    #[bits(436..=439)]
    pub organ_b_octave_shift: RangedU8<15>,
    #[bits(440..=440)]
    pub organ_b_sustain_pedal_enabled: bool,
    #[bits(441..=443)]
    pub organ_b_model: RangedU8<7>,
    #[bits(444..=444)]
    pub organ_b_preset_enabled: bool,
    #[bits(464..=467)]
    pub organ_b_drawbar_1: RangedU8<15>,
    #[bits(468..=472)]
    pub organ_b_drawbar_1_wheel: RangedU8<31>,
    #[bits(473..=477)]
    pub organ_b_drawbar_1_aftertouch: RangedU8<31>,
    #[bits(478..=482)]
    pub organ_b_drawbar_1_ctrl_pedal: RangedU8<31>,
    #[bits(483..=486)]
    pub organ_b_drawbar_2: RangedU8<15>,
    #[bits(487..=491)]
    pub organ_b_drawbar_2_wheel: RangedU8<31>,
    #[bits(492..=496)]
    pub organ_b_drawbar_2_aftertouch: RangedU8<31>,
    #[bits(497..=501)]
    pub organ_b_drawbar_2_ctrl_pedal: RangedU8<31>,
    #[bits(502..=505)]
    pub organ_b_drawbar_3: RangedU8<15>,
    #[bits(506..=510)]
    pub organ_b_drawbar_3_wheel: RangedU8<31>,
    #[bits(511..=515)]
    pub organ_b_drawbar_3_aftertouch: RangedU8<31>,
    #[bits(516..=520)]
    pub organ_b_drawbar_3_ctrl_pedal: RangedU8<31>,
    #[bits(521..=524)]
    pub organ_b_drawbar_4: RangedU8<15>,
    #[bits(525..=529)]
    pub organ_b_drawbar_4_wheel: RangedU8<31>,
    #[bits(530..=534)]
    pub organ_b_drawbar_4_aftertouch: RangedU8<31>,
    #[bits(535..=539)]
    pub organ_b_drawbar_4_ctrl_pedal: RangedU8<31>,
    #[bits(540..=543)]
    pub organ_b_drawbar_5: RangedU8<15>,
    #[bits(544..=548)]
    pub organ_b_drawbar_5_wheel: RangedU8<31>,
    #[bits(549..=553)]
    pub organ_b_drawbar_5_aftertouch: RangedU8<31>,
    #[bits(554..=558)]
    pub organ_b_drawbar_5_ctrl_pedal: RangedU8<31>,
    #[bits(559..=562)]
    pub organ_b_drawbar_6: RangedU8<15>,
    #[bits(563..=567)]
    pub organ_b_drawbar_6_wheel: RangedU8<31>,
    #[bits(568..=572)]
    pub organ_b_drawbar_6_aftertouch: RangedU8<31>,
    #[bits(573..=577)]
    pub organ_b_drawbar_6_ctrl_pedal: RangedU8<31>,
    #[bits(578..=581)]
    pub organ_b_drawbar_7: RangedU8<15>,
    #[bits(582..=586)]
    pub organ_b_drawbar_7_wheel: RangedU8<31>,
    #[bits(587..=591)]
    pub organ_b_drawbar_7_aftertouch: RangedU8<31>,
    #[bits(592..=596)]
    pub organ_b_drawbar_7_ctrl_pedal: RangedU8<31>,
    #[bits(597..=600)]
    pub organ_b_drawbar_8: RangedU8<15>,
    #[bits(601..=605)]
    pub organ_b_drawbar_8_wheel: RangedU8<31>,
    #[bits(606..=610)]
    pub organ_b_drawbar_8_aftertouch: RangedU8<31>,
    #[bits(611..=615)]
    pub organ_b_drawbar_8_ctrl_pedal: RangedU8<31>,
    #[bits(616..=619)]
    pub organ_b_drawbar_9: RangedU8<15>,
    #[bits(620..=624)]
    pub organ_b_drawbar_9_wheel: RangedU8<31>,
    #[bits(625..=629)]
    pub organ_b_drawbar_9_aftertouch: RangedU8<31>,
    #[bits(630..=634)]
    pub organ_b_drawbar_9_ctrl_pedal: RangedU8<31>,
    #[bits(656..=656)]
    pub organ_b_vib_chorus_enabled: bool,
    #[bits(657..=657)]
    pub organ_b_percussion_enabled: bool,
    #[bits(658..=658)]
    pub organ_b_percussion_harmonic_3rd_enabled: bool,
    #[bits(659..=659)]
    pub organ_b_percussion_decay_fast_enabled: bool,
    #[bits(660..=660)]
    pub organ_b_percussion_volume_soft_enabled: bool,

    /// The organ section's effects chain.
    #[at(85..137)]
    pub organ_fx: FxChain,
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<OrganPreset>, Error> {
    let file: Cbin<OrganPreset> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
