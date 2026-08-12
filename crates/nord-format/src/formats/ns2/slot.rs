//! One Nord Stage 2 slot: a complete organ / piano / synth / extern / effects
//! setup, 249 bytes of it.
//!
//! A program holds two — Slot A and Slot B — switched or layered by the Slot
//! buttons. Same layout both times, so this is one type placed twice; see
//! [`super::program::Program`].

use super::program::*;
use crate::components::{Effect1Type, Effect2Type};
use crate::types::{RangedU16, RangedU8};

/// The slot's 403 parameters. Bits are MSB-first from slot byte 0,
/// which is body byte 0x17 for A and 0x110 for B.
#[nord_bits_derive::bitbody(249)]
pub struct Slot {
    #[bits(0..=0)]
    pub organ_on: bool,
    #[bits(1..=8)]
    pub organ_volume_wheel: u8,
    #[bits(9..=16)]
    pub organ_volume_aftertouch: u8,
    #[bits(17..=24)]
    pub organ_volume_ctrl_pedal: u8,
    #[bits(25..=31)]
    pub organ_volume: RangedU8<127>,
    #[bits(32..=34)]
    pub organ_kb_zone: OrganKbZone,
    #[bits(35..=38)]
    pub organ_octave_shift: RangedU8<15>,
    #[bits(39..=39)]
    pub organ_sustain_pedal: bool,
    #[bits(40..=40)]
    pub piano_on: bool,
    #[bits(41..=48)]
    pub piano_volume_wheel: u8,
    #[bits(49..=56)]
    pub piano_volume_aftertouch: u8,
    #[bits(57..=64)]
    pub piano_volume_ctrl_pedal: u8,
    #[bits(65..=71)]
    pub piano_volume: RangedU8<127>,
    #[bits(72..=74)]
    pub piano_split_zones: PianoKbZone,
    #[bits(75..=78)]
    pub piano_octave_shift: RangedU8<15>,
    #[bits(79..=79)]
    pub piano_pitch_stick: bool,
    #[bits(80..=80)]
    pub piano_sustain_pedal: bool,
    #[bits(81..=81)]
    pub synth_on: bool,
    #[bits(82..=89)]
    pub synth_volume_wheel: u8,
    #[bits(90..=97)]
    pub synth_volume_aftertouch: u8,
    #[bits(98..=105)]
    pub synth_volume_ctrl_pedal: u8,
    #[bits(106..=112)]
    pub synth_volume: RangedU8<127>,
    #[bits(113..=115)]
    pub synth_kb_zone: SynthKbZone,
    #[bits(116..=119)]
    pub synth_octave_shift: RangedU8<15>,
    #[bits(120..=120)]
    pub synth_pitch_stick: bool,
    #[bits(121..=121)]
    pub synth_sustain_pedal: bool,
    #[bits(122..=122)]
    pub extern_on: bool,
    #[bits(154..=156)]
    pub extern_kb_zone: RangedU8<7>,
    #[bits(157..=160)]
    pub extern_octave_shift: RangedU8<15>,
    #[bits(161..=161)]
    pub extern_pitch_stick: bool,
    #[bits(162..=162)]
    pub extern_sustain_pedal: bool,
    #[bits(174..=175)]
    pub piano_program_output: RangedU8<3>,
    #[bits(177..=178)]
    pub synth_program_output: RangedU8<3>,
    #[bits(180..=181)]
    pub organ_program_output: RangedU8<3>,
    #[bits(182..=182)]
    pub organ_latch_pedal: bool,
    #[bits(183..=183)]
    pub organ_kb_gate: bool,
    #[bits(184..=184)]
    pub piano_latch_pedal: bool,
    #[bits(185..=185)]
    pub piano_kb_gate: bool,
    #[bits(186..=186)]
    pub synth_latch_pedal: bool,
    #[bits(187..=187)]
    pub synth_kb_gate: bool,
    #[bits(200..=200)]
    pub organ_b3_preset_2: bool,
    #[bits(208..=208)]
    pub organ_vox_vox_ii: bool,
    #[bits(216..=216)]
    pub organ_farfisa_preset_2: bool,
    #[bits(224..=228)]
    pub organ_b3_preset_1_drawbar_1_wheel: RangedU8<31>,
    #[bits(229..=233)]
    pub organ_b3_preset_1_drawbar_1_aftertouch: RangedU8<31>,
    #[bits(234..=238)]
    pub organ_b3_preset_1_drawbar_1_ctrl_pedal: RangedU8<31>,
    #[bits(239..=242)]
    pub organ_b3_preset_1_drawbar_1: RangedU8<15>,
    #[bits(243..=247)]
    pub organ_b3_preset_1_drawbar_2_wheel: RangedU8<31>,
    #[bits(248..=252)]
    pub organ_b3_preset_1_drawbar_2_aftertouch: RangedU8<31>,
    #[bits(253..=257)]
    pub organ_b3_preset_1_drawbar_2_ctrl_pedal: RangedU8<31>,
    #[bits(258..=261)]
    pub organ_b3_preset_1_drawbar_2: RangedU8<15>,
    #[bits(262..=266)]
    pub organ_b3_preset_1_drawbar_3_wheel: RangedU8<31>,
    #[bits(267..=271)]
    pub organ_b3_preset_1_drawbar_3_aftertouch: RangedU8<31>,
    #[bits(272..=276)]
    pub organ_b3_preset_1_drawbar_3_ctrl_pedal: RangedU8<31>,
    #[bits(277..=280)]
    pub organ_b3_preset_1_drawbar_3: RangedU8<15>,
    #[bits(281..=285)]
    pub organ_b3_preset_1_drawbar_4_wheel: RangedU8<31>,
    #[bits(286..=290)]
    pub organ_b3_preset_1_drawbar_4_aftertouch: RangedU8<31>,
    #[bits(291..=295)]
    pub organ_b3_preset_1_drawbar_4_ctrl_pedal: RangedU8<31>,
    #[bits(296..=299)]
    pub organ_b3_preset_1_drawbar_4: RangedU8<15>,
    #[bits(300..=304)]
    pub organ_b3_preset_1_drawbar_5_wheel: RangedU8<31>,
    #[bits(305..=309)]
    pub organ_b3_preset_1_drawbar_5_aftertouch: RangedU8<31>,
    #[bits(310..=314)]
    pub organ_b3_preset_1_drawbar_5_ctrl_pedal: RangedU8<31>,
    #[bits(315..=318)]
    pub organ_b3_preset_1_drawbar_5: RangedU8<15>,
    #[bits(319..=323)]
    pub organ_b3_preset_1_drawbar_6_wheel: RangedU8<31>,
    #[bits(324..=328)]
    pub organ_b3_preset_1_drawbar_6_aftertouch: RangedU8<31>,
    #[bits(329..=333)]
    pub organ_b3_preset_1_drawbar_6_ctrl_pedal: RangedU8<31>,
    #[bits(334..=337)]
    pub organ_b3_preset_1_drawbar_6: RangedU8<15>,
    #[bits(338..=342)]
    pub organ_b3_preset_1_drawbar_7_wheel: RangedU8<31>,
    #[bits(343..=347)]
    pub organ_b3_preset_1_drawbar_7_aftertouch: RangedU8<31>,
    #[bits(348..=352)]
    pub organ_b3_preset_1_drawbar_7_ctrl_pedal: RangedU8<31>,
    #[bits(353..=356)]
    pub organ_b3_preset_1_drawbar_7: RangedU8<15>,
    #[bits(357..=361)]
    pub organ_b3_preset_1_drawbar_8_wheel: RangedU8<31>,
    #[bits(362..=366)]
    pub organ_b3_preset_1_drawbar_8_aftertouch: RangedU8<31>,
    #[bits(367..=371)]
    pub organ_b3_preset_1_drawbar_8_ctrl_pedal: RangedU8<31>,
    #[bits(372..=375)]
    pub organ_b3_preset_1_drawbar_8: RangedU8<15>,
    #[bits(376..=380)]
    pub organ_b3_preset_1_drawbar_9_wheel: RangedU8<31>,
    #[bits(381..=385)]
    pub organ_b3_preset_1_drawbar_9_aftertouch: RangedU8<31>,
    #[bits(386..=390)]
    pub organ_b3_preset_1_drawbar_9_ctrl_pedal: RangedU8<31>,
    #[bits(391..=394)]
    pub organ_b3_preset_1_drawbar_9: RangedU8<15>,
    #[bits(395..=395)]
    pub organ_b3_preset_1_vibrato_chorus: bool,
    #[bits(396..=396)]
    pub organ_b3_preset_1_percussion: bool,
    #[bits(408..=412)]
    pub organ_vox_preset_1_drawbar_1_wheel: RangedU8<31>,
    #[bits(413..=417)]
    pub organ_vox_preset_1_drawbar_1_aftertouch: RangedU8<31>,
    #[bits(418..=422)]
    pub organ_vox_preset_1_drawbar_1_ctrl_pedal: RangedU8<31>,
    #[bits(423..=426)]
    pub organ_vox_preset_1_drawbar_1: RangedU8<15>,
    #[bits(427..=431)]
    pub organ_vox_preset_1_drawbar_2_wheel: RangedU8<31>,
    #[bits(432..=436)]
    pub organ_vox_preset_1_drawbar_2_aftertouch: RangedU8<31>,
    #[bits(437..=441)]
    pub organ_vox_preset_1_drawbar_2_ctrl_pedal: RangedU8<31>,
    #[bits(442..=445)]
    pub organ_vox_preset_1_drawbar_2: RangedU8<15>,
    #[bits(446..=450)]
    pub organ_vox_preset_1_drawbar_3_wheel: RangedU8<31>,
    #[bits(451..=455)]
    pub organ_vox_preset_1_drawbar_3_aftertouch: RangedU8<31>,
    #[bits(456..=460)]
    pub organ_vox_preset_1_drawbar_3_ctrl_pedal: RangedU8<31>,
    #[bits(461..=464)]
    pub organ_vox_preset_1_drawbar_3: RangedU8<15>,
    #[bits(465..=469)]
    pub organ_vox_preset_1_drawbar_4_wheel: RangedU8<31>,
    #[bits(470..=474)]
    pub organ_vox_preset_1_drawbar_4_aftertouch: RangedU8<31>,
    #[bits(475..=479)]
    pub organ_vox_preset_1_drawbar_4_ctrl_pedal: RangedU8<31>,
    #[bits(480..=483)]
    pub organ_vox_preset_1_drawbar_4: RangedU8<15>,
    #[bits(484..=488)]
    pub organ_vox_preset_1_drawbar_5_wheel: RangedU8<31>,
    #[bits(489..=493)]
    pub organ_vox_preset_1_drawbar_5_aftertouch: RangedU8<31>,
    #[bits(494..=498)]
    pub organ_vox_preset_1_drawbar_5_ctrl_pedal: RangedU8<31>,
    #[bits(499..=502)]
    pub organ_vox_preset_1_drawbar_5: RangedU8<15>,
    #[bits(503..=507)]
    pub organ_vox_preset_1_drawbar_6_wheel: RangedU8<31>,
    #[bits(508..=512)]
    pub organ_vox_preset_1_drawbar_6_aftertouch: RangedU8<31>,
    #[bits(513..=517)]
    pub organ_vox_preset_1_drawbar_6_ctrl_pedal: RangedU8<31>,
    #[bits(518..=521)]
    pub organ_vox_preset_1_drawbar_6: RangedU8<15>,
    #[bits(522..=526)]
    pub organ_vox_preset_1_drawbar_7_wheel: RangedU8<31>,
    #[bits(527..=531)]
    pub organ_vox_preset_1_drawbar_7_aftertouch: RangedU8<31>,
    #[bits(532..=536)]
    pub organ_vox_preset_1_drawbar_7_ctrl_pedal: RangedU8<31>,
    #[bits(537..=540)]
    pub organ_vox_preset_1_drawbar_7: RangedU8<15>,
    #[bits(541..=545)]
    pub organ_vox_preset_1_drawbar_8_wheel: RangedU8<31>,
    #[bits(546..=550)]
    pub organ_vox_preset_1_drawbar_8_aftertouch: RangedU8<31>,
    #[bits(551..=555)]
    pub organ_vox_preset_1_drawbar_8_ctrl_pedal: RangedU8<31>,
    #[bits(556..=559)]
    pub organ_vox_preset_1_drawbar_8: RangedU8<15>,
    #[bits(560..=564)]
    pub organ_vox_preset_1_drawbar_9_wheel: RangedU8<31>,
    #[bits(565..=569)]
    pub organ_vox_preset_1_drawbar_9_aftertouch: RangedU8<31>,
    #[bits(570..=574)]
    pub organ_vox_preset_1_drawbar_9_ctrl_pedal: RangedU8<31>,
    #[bits(575..=578)]
    pub organ_vox_preset_1_drawbar_9: RangedU8<15>,
    #[bits(592..=593)]
    pub organ_farfisa_preset_1_drawbar_1_wheel: RangedU8<3>,
    #[bits(594..=595)]
    pub organ_farfisa_preset_1_drawbar_1_aftertouch: RangedU8<3>,
    #[bits(596..=597)]
    pub organ_farfisa_preset_1_drawbar_1_ctrl_pedal: RangedU8<3>,
    #[bits(598..=598)]
    pub organ_farfisa_preset_1_drawbar_1: bool,
    #[bits(599..=600)]
    pub organ_farfisa_preset_1_drawbar_2_wheel: RangedU8<3>,
    #[bits(601..=602)]
    pub organ_farfisa_preset_1_drawbar_2_aftertouch: RangedU8<3>,
    #[bits(603..=604)]
    pub organ_farfisa_preset_1_drawbar_2_ctrl_pedal: RangedU8<3>,
    #[bits(605..=605)]
    pub organ_farfisa_preset_1_drawbar_2: bool,
    #[bits(606..=607)]
    pub organ_farfisa_preset_1_drawbar_3_wheel: RangedU8<3>,
    #[bits(608..=609)]
    pub organ_farfisa_preset_1_drawbar_3_aftertouch: RangedU8<3>,
    #[bits(610..=611)]
    pub organ_farfisa_preset_1_drawbar_3_ctrl_pedal: RangedU8<3>,
    #[bits(612..=612)]
    pub organ_farfisa_preset_1_drawbar_3: bool,
    #[bits(613..=614)]
    pub organ_farfisa_preset_1_drawbar_4_wheel: RangedU8<3>,
    #[bits(615..=616)]
    pub organ_farfisa_preset_1_drawbar_4_aftertouch: RangedU8<3>,
    #[bits(617..=618)]
    pub organ_farfisa_preset_1_drawbar_4_ctrl_pedal: RangedU8<3>,
    #[bits(619..=619)]
    pub organ_farfisa_preset_1_drawbar_4: bool,
    #[bits(620..=621)]
    pub organ_farfisa_preset_1_drawbar_5_wheel: RangedU8<3>,
    #[bits(622..=623)]
    pub organ_farfisa_preset_1_drawbar_5_aftertouch: RangedU8<3>,
    #[bits(624..=625)]
    pub organ_farfisa_preset_1_drawbar_5_ctrl_pedal: RangedU8<3>,
    #[bits(626..=626)]
    pub organ_farfisa_preset_1_drawbar_5: bool,
    #[bits(627..=628)]
    pub organ_farfisa_preset_1_drawbar_6_wheel: RangedU8<3>,
    #[bits(629..=630)]
    pub organ_farfisa_preset_1_drawbar_6_aftertouch: RangedU8<3>,
    #[bits(631..=632)]
    pub organ_farfisa_preset_1_drawbar_6_ctrl_pedal: RangedU8<3>,
    #[bits(633..=633)]
    pub organ_farfisa_preset_1_drawbar_6: bool,
    #[bits(634..=635)]
    pub organ_farfisa_preset_1_drawbar_7_wheel: RangedU8<3>,
    #[bits(636..=637)]
    pub organ_farfisa_preset_1_drawbar_7_aftertouch: RangedU8<3>,
    #[bits(638..=639)]
    pub organ_farfisa_preset_1_drawbar_7_ctrl_pedal: RangedU8<3>,
    #[bits(640..=640)]
    pub organ_farfisa_preset_1_drawbar_7: bool,
    #[bits(641..=642)]
    pub organ_farfisa_preset_1_drawbar_8_wheel: RangedU8<3>,
    #[bits(643..=644)]
    pub organ_farfisa_preset_1_drawbar_8_aftertouch: RangedU8<3>,
    #[bits(645..=646)]
    pub organ_farfisa_preset_1_drawbar_8_ctrl_pedal: RangedU8<3>,
    #[bits(647..=647)]
    pub organ_farfisa_preset_1_drawbar_8: bool,
    #[bits(648..=649)]
    pub organ_farfisa_preset_1_drawbar_9_wheel: RangedU8<3>,
    #[bits(650..=651)]
    pub organ_farfisa_preset_1_drawbar_9_aftertouch: RangedU8<3>,
    #[bits(652..=653)]
    pub organ_farfisa_preset_1_drawbar_9_ctrl_pedal: RangedU8<3>,
    #[bits(654..=654)]
    pub organ_farfisa_preset_1_drawbar_9: bool,
    #[bits(664..=668)]
    pub organ_b3_preset_2_drawbar_1_wheel: RangedU8<31>,
    #[bits(669..=673)]
    pub organ_b3_preset_2_drawbar_1_aftertouch: RangedU8<31>,
    #[bits(674..=678)]
    pub organ_b3_preset_2_drawbar_1_ctrl_pedal: RangedU8<31>,
    #[bits(679..=682)]
    pub organ_b3_preset_2_drawbar_1: RangedU8<15>,
    #[bits(683..=687)]
    pub organ_b3_preset_2_drawbar_2_wheel: RangedU8<31>,
    #[bits(688..=692)]
    pub organ_b3_preset_2_drawbar_2_aftertouch: RangedU8<31>,
    #[bits(693..=697)]
    pub organ_b3_preset_2_drawbar_2_ctrl_pedal: RangedU8<31>,
    #[bits(698..=701)]
    pub organ_b3_preset_2_drawbar_2: RangedU8<15>,
    #[bits(702..=706)]
    pub organ_b3_preset_2_drawbar_3_wheel: RangedU8<31>,
    #[bits(707..=711)]
    pub organ_b3_preset_2_drawbar_3_aftertouch: RangedU8<31>,
    #[bits(712..=716)]
    pub organ_b3_preset_2_drawbar_3_ctrl_pedal: RangedU8<31>,
    #[bits(717..=720)]
    pub organ_farfisa_preset_1_drawbar_4_3: RangedU8<15>,
    #[bits(721..=725)]
    pub organ_b3_preset_2_drawbar_4_wheel: RangedU8<31>,
    #[bits(726..=730)]
    pub organ_b3_preset_2_drawbar_4_aftertouch: RangedU8<31>,
    #[bits(731..=735)]
    pub organ_b3_preset_2_drawbar_4_ctrl_pedal: RangedU8<31>,
    #[bits(736..=739)]
    pub organ_b3_preset_2_drawbar_4: RangedU8<15>,
    #[bits(740..=744)]
    pub organ_b3_preset_2_drawbar_5_wheel: RangedU8<31>,
    #[bits(745..=749)]
    pub organ_b3_preset_2_drawbar_5_aftertouch: RangedU8<31>,
    #[bits(750..=754)]
    pub organ_b3_preset_2_drawbar_5_ctrl_pedal: RangedU8<31>,
    #[bits(755..=758)]
    pub organ_b3_preset_2_drawbar_5: RangedU8<15>,
    #[bits(759..=763)]
    pub organ_b3_preset_2_drawbar_6_wheel: RangedU8<31>,
    #[bits(764..=768)]
    pub organ_b3_preset_2_drawbar_6_aftertouch: RangedU8<31>,
    #[bits(769..=773)]
    pub organ_b3_preset_2_drawbar_6_ctrl_pedal: RangedU8<31>,
    #[bits(774..=777)]
    pub organ_b3_preset_2_drawbar_6: RangedU8<15>,
    #[bits(778..=782)]
    pub organ_b3_preset_2_drawbar_7_wheel: RangedU8<31>,
    #[bits(783..=787)]
    pub organ_b3_preset_2_drawbar_7_aftertouch: RangedU8<31>,
    #[bits(788..=792)]
    pub organ_b3_preset_2_drawbar_7_ctrl_pedal: RangedU8<31>,
    #[bits(793..=796)]
    pub organ_b3_preset_2_drawbar_7: RangedU8<15>,
    #[bits(797..=801)]
    pub organ_b3_preset_2_drawbar_8_wheel: RangedU8<31>,
    #[bits(802..=806)]
    pub organ_b3_preset_2_drawbar_8_aftertouch: RangedU8<31>,
    #[bits(807..=811)]
    pub organ_b3_preset_2_drawbar_8_ctrl_pedal: RangedU8<31>,
    #[bits(812..=815)]
    pub organ_b3_preset_2_drawbar_8: RangedU8<15>,
    #[bits(816..=820)]
    pub organ_b3_preset_2_drawbar_9_wheel: RangedU8<31>,
    #[bits(821..=825)]
    pub organ_b3_preset_2_drawbar_9_aftertouch: RangedU8<31>,
    #[bits(826..=830)]
    pub organ_b3_preset_2_drawbar_9_ctrl_pedal: RangedU8<31>,
    #[bits(831..=834)]
    pub organ_b3_preset_2_drawbar_9: RangedU8<15>,
    #[bits(835..=835)]
    pub organ_b3_preset_2_vibrato_chorus: bool,
    #[bits(836..=836)]
    pub organ_b3_preset_2_percussion: bool,
    #[bits(848..=852)]
    pub organ_vox_preset_2_drawbar_1_wheel: RangedU8<31>,
    #[bits(853..=857)]
    pub organ_vox_preset_2_drawbar_1_aftertouch: RangedU8<31>,
    #[bits(858..=862)]
    pub organ_vox_preset_2_drawbar_1_ctrl_pedal: RangedU8<31>,
    #[bits(863..=866)]
    pub organ_vox_preset_2_drawbar_1: RangedU8<15>,
    #[bits(867..=871)]
    pub organ_vox_preset_2_drawbar_2_wheel: RangedU8<31>,
    #[bits(872..=876)]
    pub organ_vox_preset_2_drawbar_2_aftertouch: RangedU8<31>,
    #[bits(877..=881)]
    pub organ_vox_preset_2_drawbar_2_ctrl_pedal: RangedU8<31>,
    #[bits(882..=885)]
    pub organ_vox_preset_2_drawbar_2: RangedU8<15>,
    #[bits(886..=890)]
    pub organ_vox_preset_2_drawbar_3_wheel: RangedU8<31>,
    #[bits(891..=895)]
    pub organ_vox_preset_2_drawbar_3_aftertouch: RangedU8<31>,
    #[bits(896..=900)]
    pub organ_vox_preset_2_drawbar_3_ctrl_pedal: RangedU8<31>,
    #[bits(901..=904)]
    pub organ_b3_preset_2_drawbar_3: RangedU8<15>,
    #[bits(905..=909)]
    pub organ_vox_preset_2_drawbar_4_wheel: RangedU8<31>,
    #[bits(910..=914)]
    pub organ_vox_preset_2_drawbar_4_aftertouch: RangedU8<31>,
    #[bits(915..=919)]
    pub organ_vox_preset_2_drawbar_4_ctrl_pedal: RangedU8<31>,
    #[bits(920..=923)]
    pub organ_vox_preset_2_drawbar_4: RangedU8<15>,
    #[bits(924..=928)]
    pub organ_vox_preset_2_drawbar_5_wheel: RangedU8<31>,
    #[bits(929..=933)]
    pub organ_vox_preset_2_drawbar_5_aftertouch: RangedU8<31>,
    #[bits(934..=938)]
    pub organ_vox_preset_2_drawbar_5_ctrl_pedal: RangedU8<31>,
    #[bits(939..=942)]
    pub organ_vox_preset_2_drawbar_5: RangedU8<15>,
    #[bits(943..=947)]
    pub organ_vox_preset_2_drawbar_6_wheel: RangedU8<31>,
    #[bits(948..=952)]
    pub organ_vox_preset_2_drawbar_6_aftertouch: RangedU8<31>,
    #[bits(953..=957)]
    pub organ_vox_preset_2_drawbar_6_ctrl_pedal: RangedU8<31>,
    #[bits(958..=961)]
    pub organ_vox_preset_2_drawbar_6: RangedU8<15>,
    #[bits(962..=966)]
    pub organ_vox_preset_2_drawbar_7_wheel: RangedU8<31>,
    #[bits(967..=971)]
    pub organ_vox_preset_2_drawbar_7_aftertouch: RangedU8<31>,
    #[bits(972..=976)]
    pub organ_vox_preset_2_drawbar_7_ctrl_pedal: RangedU8<31>,
    #[bits(977..=980)]
    pub organ_vox_preset_2_drawbar_7: RangedU8<15>,
    #[bits(981..=985)]
    pub organ_vox_preset_2_drawbar_8_wheel: RangedU8<31>,
    #[bits(986..=990)]
    pub organ_vox_preset_2_drawbar_8_aftertouch: RangedU8<31>,
    #[bits(991..=995)]
    pub organ_vox_preset_2_drawbar_8_ctrl_pedal: RangedU8<31>,
    #[bits(996..=999)]
    pub organ_vox_preset_2_drawbar_8: RangedU8<15>,
    #[bits(1000..=1004)]
    pub organ_vox_preset_2_drawbar_9_wheel: RangedU8<31>,
    #[bits(1005..=1009)]
    pub organ_vox_preset_2_drawbar_9_aftertouch: RangedU8<31>,
    #[bits(1010..=1014)]
    pub organ_vox_preset_2_drawbar_9_ctrl_pedal: RangedU8<31>,
    #[bits(1015..=1018)]
    pub organ_vox_preset_2_drawbar_9: RangedU8<15>,
    #[bits(1032..=1033)]
    pub organ_farfisa_preset_2_drawbar_1_wheel: RangedU8<3>,
    #[bits(1034..=1035)]
    pub organ_farfisa_preset_2_drawbar_1_aftertouch: RangedU8<3>,
    #[bits(1036..=1037)]
    pub organ_farfisa_preset_2_drawbar_1_ctrl_pedal: RangedU8<3>,
    #[bits(1038..=1038)]
    pub organ_farfisa_preset_2_drawbar_1: bool,
    #[bits(1039..=1040)]
    pub organ_farfisa_preset_2_drawbar_2_wheel: RangedU8<3>,
    #[bits(1041..=1042)]
    pub organ_farfisa_preset_2_drawbar_2_aftertouch: RangedU8<3>,
    #[bits(1043..=1044)]
    pub organ_farfisa_preset_2_drawbar_2_ctrl_pedal: RangedU8<3>,
    #[bits(1045..=1045)]
    pub organ_farfisa_preset_2_drawbar_2: bool,
    #[bits(1046..=1047)]
    pub organ_farfisa_preset_2_drawbar_3_wheel: RangedU8<3>,
    #[bits(1048..=1049)]
    pub organ_farfisa_preset_2_drawbar_3_aftertouch: RangedU8<3>,
    #[bits(1050..=1051)]
    pub organ_farfisa_preset_2_drawbar_3_ctrl_pedal: RangedU8<3>,
    #[bits(1052..=1052)]
    pub organ_vox_preset_2_drawbar_3: bool,
    #[bits(1053..=1054)]
    pub organ_farfisa_preset_2_drawbar_4_wheel: RangedU8<3>,
    #[bits(1055..=1056)]
    pub organ_farfisa_preset_2_drawbar_4_aftertouch: RangedU8<3>,
    #[bits(1057..=1058)]
    pub organ_farfisa_preset_2_drawbar_4_ctrl_pedal: RangedU8<3>,
    #[bits(1059..=1059)]
    pub organ_farfisa_preset_2_drawbar_4: bool,
    #[bits(1060..=1061)]
    pub organ_farfisa_preset_2_drawbar_5_wheel: RangedU8<3>,
    #[bits(1062..=1063)]
    pub organ_farfisa_preset_2_drawbar_5_aftertouch: RangedU8<3>,
    #[bits(1064..=1065)]
    pub organ_farfisa_preset_2_drawbar_5_ctrl_pedal: RangedU8<3>,
    #[bits(1066..=1066)]
    pub organ_farfisa_preset_2_drawbar_5: bool,
    #[bits(1067..=1068)]
    pub organ_farfisa_preset_2_drawbar_6_wheel: RangedU8<3>,
    #[bits(1069..=1070)]
    pub organ_farfisa_preset_2_drawbar_6_aftertouch: RangedU8<3>,
    #[bits(1071..=1072)]
    pub organ_farfisa_preset_2_drawbar_6_ctrl_pedal: RangedU8<3>,
    #[bits(1073..=1073)]
    pub organ_farfisa_preset_2_drawbar_6: bool,
    #[bits(1074..=1075)]
    pub organ_farfisa_preset_2_drawbar_7_wheel: RangedU8<3>,
    #[bits(1076..=1077)]
    pub organ_farfisa_preset_2_drawbar_7_aftertouch: RangedU8<3>,
    #[bits(1078..=1079)]
    pub organ_farfisa_preset_2_drawbar_7_ctrl_pedal: RangedU8<3>,
    #[bits(1080..=1080)]
    pub organ_farfisa_preset_2_drawbar_7: bool,
    #[bits(1081..=1082)]
    pub organ_farfisa_preset_2_drawbar_8_wheel: RangedU8<3>,
    #[bits(1083..=1084)]
    pub organ_farfisa_preset_2_drawbar_8_aftertouch: RangedU8<3>,
    #[bits(1085..=1086)]
    pub organ_farfisa_preset_2_drawbar_8_ctrl_pedal: RangedU8<3>,
    #[bits(1087..=1087)]
    pub organ_farfisa_preset_2_drawbar_8: bool,
    #[bits(1088..=1089)]
    pub organ_farfisa_preset_2_drawbar_9_wheel: RangedU8<3>,
    #[bits(1090..=1091)]
    pub organ_farfisa_preset_2_drawbar_9_aftertouch: RangedU8<3>,
    #[bits(1092..=1093)]
    pub organ_farfisa_preset_2_drawbar_9_ctrl_pedal: RangedU8<3>,
    #[bits(1094..=1094)]
    pub organ_farfisa_preset_2_drawbar_9: bool,
    #[bits(1104..=1106)]
    pub piano_type: RangedU8<7>,
    #[bits(1119..=1120)]
    pub piano_clavinet_model: RangedU8<3>,
    #[bits(1121..=1121)]
    pub piano_long_release: bool,
    #[bits(1122..=1122)]
    pub piano_string_resonance: bool,
    #[bits(1123..=1123)]
    pub piano_pedal_noise: bool,
    #[bits(1124..=1125)]
    pub piano_dynamics: RangedU8<3>,
    #[bits(1126..=1127)]
    pub piano_clav_eq_hi: RangedU8<3>,
    #[bits(1128..=1129)]
    pub piano_clav_eq: RangedU8<3>,
    #[bits(1130..=1161)]
    pub piano_sample_id: u32,
    #[bits(1207..=1207)]
    pub synth_arp_on: bool,
    #[bits(1208..=1208)]
    pub synth_arp_master_clock: bool,
    #[bits(1209..=1212)]
    pub synth_arp_master_clock_divisor: RangedU8<15>,
    #[bits(1214..=1220)]
    pub synth_arp_rate: RangedU8<127>,
    #[bits(1221..=1222)]
    pub synth_arp_pattern: RangedU8<3>,
    #[bits(1223..=1224)]
    pub synth_arp_master_range: RangedU8<3>,
    #[bits(1225..=1225)]
    pub synth_lfo_master_clock: bool,
    #[bits(1226..=1229)]
    pub synth_lfo_rate_clock_divisor: RangedU8<15>,
    #[bits(1230..=1230)]
    pub synth_kb_hold: bool,
    #[bits(1248..=1254)]
    pub synth_mod_env_attack: RangedU8<127>,
    #[bits(1255..=1261)]
    pub synth_mod_env_decay: RangedU8<127>,
    #[bits(1262..=1268)]
    pub synth_mod_env_release: RangedU8<127>,
    #[bits(1269..=1269)]
    pub synth_mod_env_velocity: bool,
    #[bits(1270..=1272)]
    pub synth_osc_mode: RangedU8<7>,
    #[bits(1273..=1282)]
    pub synth_osc_waveform: RangedU16<1023>,
    #[bits(1283..=1290)]
    pub synth_shape_wheel: u8,
    #[bits(1291..=1298)]
    pub synth_shape_aftertouch: u8,
    #[bits(1299..=1306)]
    pub synth_shape_ctrl_pedal: u8,
    #[bits(1307..=1313)]
    pub synth_shape: RangedU8<127>,
    #[bits(1314..=1320)]
    pub synth_shape_mod: RangedU8<127>,
    #[bits(1321..=1328)]
    pub synth_shape_detune_wheel: u8,
    #[bits(1329..=1336)]
    pub synth_shape_detune_aftertouch: u8,
    #[bits(1337..=1344)]
    pub synth_shape_detune_ctrl_pedal: u8,
    #[bits(1345..=1351)]
    pub synth_shape_detune: RangedU8<127>,
    #[bits(1352..=1353)]
    pub synth_skip_sample_attack_wheel: RangedU8<3>,
    #[bits(1354..=1355)]
    pub synth_skip_sample_attack_aftertouch: RangedU8<3>,
    #[bits(1356..=1357)]
    pub synth_skip_sample_attack_ctrl_pedal: RangedU8<3>,
    #[bits(1358..=1358)]
    pub synth_skip_sample_attack: bool,
    #[bits(1359..=1366)]
    pub synth_filter_freq_wheel: u8,
    #[bits(1367..=1374)]
    pub synth_filter_freq_aftertouch: u8,
    #[bits(1375..=1382)]
    pub synth_filter_freq_ctrl_pedal: u8,
    #[bits(1383..=1389)]
    pub synth_filter_freq: RangedU8<127>,
    #[bits(1390..=1396)]
    pub synth_filter_resonance: RangedU8<127>,
    #[bits(1397..=1403)]
    pub synth_filter_mod_2: RangedU8<127>,
    #[bits(1404..=1410)]
    pub synth_filter_mod_1: RangedU8<127>,
    #[bits(1411..=1411)]
    pub synth_filter_kb_track: bool,
    #[bits(1412..=1414)]
    pub synth_filter_type: RangedU8<7>,
    #[bits(1415..=1421)]
    pub synth_amp_env_attack: RangedU8<127>,
    #[bits(1422..=1428)]
    pub synth_amp_env_decay: RangedU8<127>,
    #[bits(1429..=1435)]
    pub synth_amp_env_release: RangedU8<127>,
    #[bits(1436..=1436)]
    pub synth_amp_env_velocity: bool,
    #[bits(1437..=1443)]
    pub synth_lfo_rate: RangedU8<127>,
    #[bits(1444..=1445)]
    pub synth_lfo_waveform: RangedU8<3>,
    #[bits(1446..=1477)]
    pub synth_sample_id: u32,
    #[bits(1478..=1484)]
    pub synth_glide_rate: RangedU8<127>,
    #[bits(1485..=1486)]
    pub synth_glide_voice_mode: RangedU8<3>,
    #[bits(1487..=1489)]
    pub synth_unison: RangedU8<7>,
    #[bits(1490..=1492)]
    pub synth_vibrato: RangedU8<7>,
    #[bits(1504..=1505)]
    pub extern_midi_control: RangedU8<3>,
    #[bits(1506..=1512)]
    pub extern_midi_cc_number: RangedU8<127>,
    #[bits(1513..=1520)]
    pub extern_midi_cc_wheel: u8,
    #[bits(1521..=1528)]
    pub extern_midi_cc_aftertouch: u8,
    #[bits(1529..=1536)]
    pub extern_midi_cc_ctrl_pedal: u8,
    #[bits(1537..=1543)]
    pub extern_midi_cc: RangedU8<127>,
    #[bits(1544..=1544)]
    pub extern_midi_cc_on: bool,
    #[bits(1545..=1551)]
    pub extern_midi_bank_select_cc32: RangedU8<127>,
    #[bits(1552..=1552)]
    pub extern_midi_bank_select_cc32_enabled: bool,
    #[bits(1553..=1559)]
    pub extern_midi_bank_select_cc00: RangedU8<127>,
    #[bits(1560..=1560)]
    pub extern_midi_bank_select_cc00_enabled: bool,
    #[bits(1561..=1567)]
    pub extern_midi_program: RangedU8<127>,
    #[bits(1568..=1568)]
    pub extern_midi_program_on: bool,
    #[bits(1569..=1572)]
    pub extern_midi_channel: RangedU8<15>,
    #[bits(1574..=1574)]
    pub extern_midi_channel_type: bool,
    #[bits(1575..=1582)]
    pub extern_volume_wheel: u8,
    #[bits(1583..=1590)]
    pub extern_volume_aftertouch: u8,
    #[bits(1591..=1598)]
    pub extern_volume_ctrl_pedal: u8,
    #[bits(1599..=1605)]
    pub extern_volume: RangedU8<127>,
    #[bits(1606..=1606)]
    pub extern_midi_volume_on: bool,
    #[bits(1607..=1607)]
    pub extern_midi_send_wheel: bool,
    #[bits(1608..=1608)]
    pub extern_midi_send_aftertouch: bool,
    #[bits(1609..=1609)]
    pub extern_midi_send_control_pedal: bool,
    #[bits(1611..=1612)]
    pub extern_midi_velocity_curve: RangedU8<3>,
    #[bits(1613..=1613)]
    pub extern_midi_send_swell: bool,
    #[bits(1632..=1633)]
    pub effect_focus: RangedU8<3>,
    #[bits(1634..=1634)]
    pub effect_1_on: bool,
    #[bits(1635..=1636)]
    pub effect_1_source: RangedU8<3>,
    #[bits(1637..=1639)]
    pub effect_1_type: Effect1Type,
    #[bits(1640..=1640)]
    pub effect_1_master_clock: bool,
    #[bits(1641..=1645)]
    pub effect_1_rate_mst_clock_divisor_wheel: RangedU8<31>,
    #[bits(1646..=1650)]
    pub effect_1_rate_mst_clock_divisor_aftertouch: RangedU8<31>,
    #[bits(1651..=1655)]
    pub effect_1_rate_mst_clock_divisor_ctrl_pedal: RangedU8<31>,
    #[bits(1656..=1659)]
    pub effect_1_rate_mst_clock_divisor: RangedU8<15>,
    #[bits(1660..=1667)]
    pub effect_1_rate_wheel: u8,
    #[bits(1668..=1675)]
    pub effect_1_rate_aftertouch: u8,
    #[bits(1676..=1683)]
    pub effect_1_rate_ctrl_pedal: u8,
    #[bits(1684..=1690)]
    pub effect_1_rate: RangedU8<127>,
    #[bits(1691..=1698)]
    pub effect_1_amount_wheel: u8,
    #[bits(1699..=1706)]
    pub effect_1_amount_aftertouch: u8,
    #[bits(1707..=1714)]
    pub effect_1_amount_ctrl_pedal: u8,
    #[bits(1715..=1721)]
    pub effect_1_amount: RangedU8<127>,
    #[bits(1722..=1722)]
    pub effect_2_on: bool,
    #[bits(1723..=1724)]
    pub effect_2_source: RangedU8<3>,
    #[bits(1725..=1727)]
    pub effect_2_type: Effect2Type,
    #[bits(1728..=1728)]
    pub effect_2_master_clock: bool,
    #[bits(1729..=1733)]
    pub effect_2_rate_mst_clock_divisor_wheel: RangedU8<31>,
    #[bits(1734..=1738)]
    pub effect_2_rate_mst_clock_divisor_aftertouch: RangedU8<31>,
    #[bits(1739..=1743)]
    pub effect_2_rate_mst_clock_divisor_ctrl_pedal: RangedU8<31>,
    #[bits(1744..=1747)]
    pub effect_2_rate_mst_clock_divisor: RangedU8<15>,
    #[bits(1748..=1755)]
    pub effect_2_rate_wheel: u8,
    #[bits(1756..=1763)]
    pub effect_2_rate_aftertouch: u8,
    #[bits(1764..=1771)]
    pub effect_2_rate_ctrl_pedal: u8,
    #[bits(1772..=1778)]
    pub effect_2_rate: RangedU8<127>,
    #[bits(1779..=1786)]
    pub effect_2_amount_wheel: u8,
    #[bits(1787..=1794)]
    pub effect_2_amount_aftertouch: u8,
    #[bits(1795..=1802)]
    pub effect_2_amount_ctrl_pedal: u8,
    #[bits(1803..=1809)]
    pub effect_2_amount: RangedU8<127>,
    #[bits(1810..=1810)]
    pub delay_on: bool,
    #[bits(1811..=1812)]
    pub delay_source: RangedU8<3>,
    #[bits(1813..=1813)]
    pub delay_ping_pong: bool,
    #[bits(1814..=1814)]
    pub delay_master_clock: bool,
    #[bits(1815..=1819)]
    pub delay_tempo_master_clock_divisor_wheel_o_delay_on: RangedU8<31>,
    #[bits(1820..=1824)]
    pub delay_tempo_master_clock_divisor_aftertouch: RangedU8<31>,
    #[bits(1825..=1829)]
    pub delay_tempo_master_clock_divisor_ctrl_pedal: RangedU8<31>,
    #[bits(1830..=1833)]
    pub delay_tempo_master_clock_divisor: RangedU8<15>,
    #[bits(1834..=1846)]
    pub delay_tempo_master_clock_divisor_wheel: RangedU16<8191>,
    #[bits(1847..=1859)]
    pub delay_tempo_aftertouch: RangedU16<8191>,
    #[bits(1860..=1872)]
    pub delay_tempo_ctrl_pedal: RangedU16<8191>,
    #[bits(1873..=1884)]
    pub delay_tempo: RangedU16<4095>,
    #[bits(1885..=1892)]
    pub delay_tempo_wheel: u8,
    #[bits(1893..=1900)]
    pub delay_amount_aftertouch: u8,
    #[bits(1901..=1908)]
    pub delay_amount_ctrl_pedal: u8,
    #[bits(1909..=1915)]
    pub delay_amount: RangedU8<127>,
    #[bits(1916..=1922)]
    pub delay_feedback: RangedU8<127>,
    #[bits(1923..=1923)]
    pub amp_sim_eq_on: bool,
    #[bits(1924..=1925)]
    pub amp_sim_eq_source: RangedU8<3>,
    #[bits(1926..=1927)]
    pub amp_type: RangedU8<3>,
    #[bits(1928..=1934)]
    pub amp_sim_drive: RangedU8<127>,
    #[bits(1935..=1941)]
    pub eq_treble: RangedU8<127>,
    #[bits(1942..=1948)]
    pub eq_mid: RangedU8<127>,
    #[bits(1949..=1955)]
    pub eq_bass: RangedU8<127>,
    #[bits(1956..=1962)]
    pub eq_mid_flt_freq: RangedU8<127>,
}
