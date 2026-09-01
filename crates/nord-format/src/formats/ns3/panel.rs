//! One Nord Stage 3 panel: a complete organ / piano / synth / extern /
//! effects setup, 263 bytes of it.
//!
//! A program holds two of these — Panel A and Panel B — and the Panel buttons
//! switch between them or layer both. They are the same layout, so this is one
//! type placed twice; see [`super::program::Program`].
//!
//! The Stage 3 synth preset (`ns3y`) is this panel's synth block under its own
//! tag, at panel byte 0x39.

use super::program::*;
use crate::components::{
    CompressorResponse, DelayCharacter, Drawbar, DrawbarMorph, Effect1Type, Effect2Type, EqBand,
    Frequency, Interval, KbZone4, Level, MorphTarget, PianoRef, Rate, ReverbType, SampleRef,
    Selector, Time, WideSelector,
};
use crate::types::{RangedU16, RangedU8};

/// The panel's 281 parameters. Bits are MSB-first from panel byte 0,
/// which is body byte 0x16 for A and 0x11d for B.
#[nord_bits_derive::bitbody(263)]
pub struct Panel {
    #[bits(8..=8)]
    pub piano_on: bool,
    #[bits(9..=12)]
    pub piano_kb_zone: KbZone4,
    #[bits(13..=19)]
    pub piano_volume: Level,
    #[bits(20..=27)]
    pub piano_volume_wheel: MorphTarget,
    #[bits(28..=35)]
    pub piano_volume_aftertouch: MorphTarget,
    #[bits(36..=43)]
    pub piano_volume_ctrl_pedal: MorphTarget,
    #[bits(44..=47)]
    pub piano_octave_shift: OctaveShift,
    #[bits(48..=48)]
    pub piano_pitch_stick: bool,
    #[bits(49..=49)]
    pub piano_sustain_pedal: bool,
    #[bits(50..=52)]
    pub piano_type: PianoType,
    #[bits(53..=57)]
    pub piano_model: Selector<5>,
    #[bits(58..=59)]
    pub clavinet_model: ClavinetModel,
    #[bits(60..=91)]
    /// The piano model's library id. ⚠️ Renamed from `piano_sample_name`, which it
    /// never was — the Stage 4 calls the same 32-bit reference `model_id`. The preset
    /// *name* is ASCII elsewhere in the body, read by
    /// [`super::program::synth_preset_name`].
    pub piano_model_id: PianoRef,
    #[bits(92..=92)]
    pub piano_soft_release: bool,
    #[bits(93..=93)]
    pub piano_string_resonance: bool,
    #[bits(94..=94)]
    pub piano_pedal_noise: bool,
    #[bits(95..=96)]
    pub piano_kb_touch: PianoKbTouch,
    #[bits(98..=100)]
    pub piano_timbre: PianoTimbre,
    #[bits(128..=128)]
    pub synth_on: bool,
    #[bits(129..=132)]
    pub synth_kb_zone: KbZone4,
    #[bits(133..=139)]
    pub synth_volume: Level,
    #[bits(140..=147)]
    pub synth_volume_wheel: MorphTarget,
    #[bits(148..=155)]
    pub synth_volume_aftertouch: MorphTarget,
    #[bits(156..=163)]
    pub synth_volume_ctrl_pedal: MorphTarget,
    #[bits(164..=167)]
    pub synth_octave_shift: OctaveShift,
    #[bits(168..=168)]
    pub synth_pitch_stick: bool,
    #[bits(169..=169)]
    pub synth_sustain_pedal: bool,
    #[bits(170..=179)]
    pub synth_preset_location: RangedU16<1023>,
    #[bits(496..=496)]
    pub synth_kb_hold: bool,
    #[bits(497..=497)]
    pub synth_arp_on: bool,
    #[bits(498..=498)]
    pub synth_arp_kb_sync: bool,
    #[bits(499..=500)]
    pub synth_arp_range: SynthArpRange,
    #[bits(501..=502)]
    pub synth_arp_pattern: SynthArpPattern,
    #[bits(503..=503)]
    pub synth_arp_master_clock: bool,
    #[bits(504..=510)]
    pub synth_arp_rate: Time,
    #[bits(511..=518)]
    pub synth_arp_rate_wheel: MorphTarget,
    #[bits(519..=526)]
    pub synth_arp_rate_aftertouch: MorphTarget,
    #[bits(527..=534)]
    pub synth_arp_rate_ctrl_pedal: MorphTarget,
    #[bits(535..=536)]
    pub synth_voice: SynthVoice,
    #[bits(537..=543)]
    pub synth_glide: Time,
    #[bits(544..=545)]
    pub synth_unison: SynthUnison,
    #[bits(546..=548)]
    pub synth_vibrato: SynthVibrato,
    #[bits(549..=551)]
    pub synth_lfo_wave: SynthLfoWave,
    #[bits(552..=552)]
    pub synth_lfo_master_clock: bool,
    #[bits(553..=559)]
    pub synth_lfo_rate: Rate,
    #[bits(560..=567)]
    pub synth_lfo_rate_wheel: MorphTarget,
    #[bits(568..=575)]
    pub synth_lfo_rate_aftertouch: MorphTarget,
    #[bits(576..=583)]
    pub synth_lfo_rate_ctrl_pedal: MorphTarget,
    #[bits(584..=590)]
    pub synth_mod_env_attack: Time,
    #[bits(591..=597)]
    pub synth_mod_env_decay: Time,
    #[bits(598..=604)]
    pub synth_mod_env_release: Time,
    #[bits(605..=605)]
    pub synth_mod_env_velocity: bool,
    #[bits(606..=608)]
    pub synth_oscillator_type: SynthOscillatorType,
    #[bits(609..=617)]
    pub synth_oscillator_1_wave_form: WideSelector<9>,
    #[bits(619..=622)]
    pub synth_oscillator_config: SynthOscillatorConfig,
    #[bits(623..=628)]
    pub synth_pitch: Interval,
    #[bits(629..=635)]
    pub synth_oscillator_control: Level,
    #[bits(636..=643)]
    pub synth_oscillator_control_wheel: MorphTarget,
    #[bits(644..=651)]
    pub synth_oscillator_control_aftertouch: MorphTarget,
    #[bits(652..=659)]
    pub synth_oscillator_control_ctrl_pedal: MorphTarget,
    #[bits(660..=666)]
    pub synth_oscillator_mod: Level,
    #[bits(667..=674)]
    pub synth_oscillator_mod_wheel: MorphTarget,
    #[bits(675..=682)]
    pub synth_oscillator_mod_aftertouch: MorphTarget,
    #[bits(683..=690)]
    pub synth_oscillator_mod_ctrl_pedal: MorphTarget,
    #[bits(691..=693)]
    pub synth_filter_type: SynthFilterType,
    #[bits(694..=700)]
    pub synth_filter_freq: Frequency,
    #[bits(701..=708)]
    pub synth_filter_freq_wheel: MorphTarget,
    #[bits(709..=716)]
    pub synth_filter_freq_aftertouch: MorphTarget,
    #[bits(717..=724)]
    pub synth_filter_freq_ctrl_pedal: MorphTarget,
    #[bits(725..=731)]
    pub synth_filter_hp_freq_res: Level,
    #[bits(732..=739)]
    pub synth_filter_hp_freq_res_wheel: MorphTarget,
    #[bits(740..=747)]
    pub synth_filter_hp_freq_res_aftertouch: MorphTarget,
    #[bits(748..=755)]
    pub synth_filter_hp_freq_res_ctrl_pedal: MorphTarget,
    #[bits(756..=762)]
    pub synth_filter_lfo_amount: Level,
    #[bits(763..=770)]
    pub synth_filter_lfo_amount_wheel: MorphTarget,
    #[bits(771..=778)]
    pub synth_filter_lfo_amount_aftertouch: MorphTarget,
    #[bits(779..=786)]
    pub synth_filter_lfo_amount_ctrl_pedal: MorphTarget,
    #[bits(787..=793)]
    pub synth_filter_vel_mod_env_amount: Level,
    #[bits(794..=795)]
    pub synth_filter_kb_track: SynthFilterKbTrack,
    #[bits(796..=797)]
    pub synth_filter_drive: SynthFilterDrive,
    #[bits(798..=804)]
    pub synth_amp_env_attack: Time,
    #[bits(805..=811)]
    pub synth_amp_env_decay: Time,
    #[bits(812..=818)]
    pub synth_amp_env_release: Time,
    #[bits(819..=820)]
    pub synth_amp_env_velocity: SynthAmpEnvVelocity,
    #[bits(821..=852)]
    pub synth_sample_id: SampleRef,
    #[bits(853..=853)]
    pub synth_fast_attack: bool,
    #[bits(928..=928)]
    pub organ_on: bool,
    #[bits(929..=932)]
    pub organ_kb_zone: OrganKbZone,
    #[bits(933..=939)]
    pub organ_volume: Level,
    #[bits(940..=947)]
    pub organ_volume_wheel: MorphTarget,
    #[bits(948..=955)]
    pub organ_volume_aftertouch: MorphTarget,
    #[bits(956..=963)]
    pub organ_volume_ctrl_pedal: MorphTarget,
    #[bits(964..=967)]
    pub organ_octave_shift: OctaveShift,
    #[bits(968..=968)]
    pub organ_sustain_pedal: bool,
    #[bits(969..=971)]
    pub organ_type: OrganType,
    #[bits(972..=972)]
    pub organ_live_mode: bool,
    #[bits(973..=973)]
    pub organ_preset_2_on: bool,
    #[bits(992..=995)]
    pub organ_preset_1_drawbar_1: Drawbar,
    #[bits(996..=1000)]
    pub organ_preset_1_drawbar_1_wheel: DrawbarMorph,
    #[bits(1001..=1005)]
    pub organ_preset_1_drawbar_1_aftertouch: DrawbarMorph,
    #[bits(1006..=1010)]
    pub organ_preset_1_drawbar_1_ctrl_pedal: DrawbarMorph,
    #[bits(1011..=1014)]
    pub organ_preset_1_drawbar_2: Drawbar,
    #[bits(1015..=1019)]
    pub organ_preset_1_drawbar_2_wheel: DrawbarMorph,
    #[bits(1020..=1024)]
    pub organ_preset_1_drawbar_2_aftertouch: DrawbarMorph,
    #[bits(1025..=1029)]
    pub organ_preset_1_drawbar_2_ctrl_pedal: DrawbarMorph,
    #[bits(1030..=1033)]
    pub organ_preset_1_drawbar_3: Drawbar,
    #[bits(1034..=1038)]
    pub organ_preset_1_drawbar_3_wheel: DrawbarMorph,
    #[bits(1039..=1043)]
    pub organ_preset_1_drawbar_3_aftertouch: DrawbarMorph,
    #[bits(1044..=1048)]
    pub organ_preset_1_drawbar_3_ctrl_pedal: DrawbarMorph,
    #[bits(1049..=1052)]
    pub organ_preset_1_drawbar_4: Drawbar,
    #[bits(1053..=1057)]
    pub organ_preset_1_drawbar_4_wheel: DrawbarMorph,
    #[bits(1058..=1062)]
    pub organ_preset_1_drawbar_4_aftertouch: DrawbarMorph,
    #[bits(1063..=1067)]
    pub organ_preset_1_drawbar_4_ctrl_pedal: DrawbarMorph,
    #[bits(1068..=1071)]
    pub organ_preset_1_drawbar_5: Drawbar,
    #[bits(1072..=1076)]
    pub organ_preset_1_drawbar_5_wheel: DrawbarMorph,
    #[bits(1077..=1081)]
    pub organ_preset_1_drawbar_5_aftertouch: DrawbarMorph,
    #[bits(1082..=1086)]
    pub organ_preset_1_drawbar_5_ctrl_pedal: DrawbarMorph,
    #[bits(1087..=1090)]
    pub organ_preset_1_drawbar_6: Drawbar,
    #[bits(1091..=1095)]
    pub organ_preset_1_drawbar_6_wheel: DrawbarMorph,
    #[bits(1096..=1100)]
    pub organ_preset_1_drawbar_6_aftertouch: DrawbarMorph,
    #[bits(1101..=1105)]
    pub organ_preset_1_drawbar_6_ctrl_pedal: DrawbarMorph,
    #[bits(1106..=1109)]
    pub organ_preset_1_drawbar_7: Drawbar,
    #[bits(1110..=1114)]
    pub organ_preset_1_drawbar_7_wheel: DrawbarMorph,
    #[bits(1115..=1119)]
    pub organ_preset_1_drawbar_7_aftertouch: DrawbarMorph,
    #[bits(1120..=1124)]
    pub organ_preset_1_drawbar_7_ctrl_pedal: DrawbarMorph,
    #[bits(1125..=1128)]
    pub organ_preset_1_drawbar_8: Drawbar,
    #[bits(1129..=1133)]
    pub organ_preset_1_drawbar_8_wheel: DrawbarMorph,
    #[bits(1134..=1138)]
    pub organ_preset_1_drawbar_8_aftertouch: DrawbarMorph,
    #[bits(1139..=1143)]
    pub organ_preset_1_drawbar_8_ctrl_pedal: DrawbarMorph,
    #[bits(1144..=1147)]
    pub organ_preset_1_drawbar_9: Drawbar,
    #[bits(1148..=1152)]
    pub organ_preset_1_drawbar_9_wheel: DrawbarMorph,
    #[bits(1153..=1157)]
    pub organ_preset_1_drawbar_9_aftertouch: DrawbarMorph,
    #[bits(1158..=1162)]
    pub organ_preset_1_drawbar_9_ctrl_pedal: DrawbarMorph,
    #[bits(1163..=1163)]
    pub organ_vibrato_on: bool,
    #[bits(1164..=1164)]
    pub organ_percussion_on: bool,
    #[bits(1165..=1165)]
    pub organ_percussion_harmonic_third: bool,
    #[bits(1166..=1166)]
    pub organ_percussion_decay_fast: bool,
    #[bits(1167..=1167)]
    pub organ_percussion_volume_soft: bool,
    #[bits(1208..=1211)]
    pub organ_preset_2_drawbar_1: Drawbar,
    #[bits(1212..=1216)]
    pub organ_preset_2_drawbar_1_wheel: DrawbarMorph,
    #[bits(1217..=1221)]
    pub organ_preset_2_drawbar_1_aftertouch: DrawbarMorph,
    #[bits(1222..=1226)]
    pub organ_preset_2_drawbar_1_ctrl_pedal: DrawbarMorph,
    #[bits(1227..=1230)]
    pub organ_preset_2_drawbar_2: Drawbar,
    #[bits(1231..=1235)]
    pub organ_preset_2_drawbar_2_wheel: DrawbarMorph,
    #[bits(1236..=1240)]
    pub organ_preset_2_drawbar_2_aftertouch: DrawbarMorph,
    #[bits(1241..=1245)]
    pub organ_preset_2_drawbar_2_ctrl_pedal: DrawbarMorph,
    #[bits(1246..=1249)]
    pub organ_preset_2_drawbar_3: Drawbar,
    #[bits(1250..=1254)]
    pub organ_preset_2_drawbar_3_wheel: DrawbarMorph,
    #[bits(1255..=1259)]
    pub organ_preset_2_drawbar_3_aftertouch: DrawbarMorph,
    #[bits(1260..=1264)]
    pub organ_preset_2_drawbar_3_ctrl_pedal: DrawbarMorph,
    #[bits(1265..=1268)]
    pub organ_preset_2_drawbar_4: Drawbar,
    #[bits(1269..=1273)]
    pub organ_preset_2_drawbar_4_wheel: DrawbarMorph,
    #[bits(1274..=1278)]
    pub organ_preset_2_drawbar_4_aftertouch: DrawbarMorph,
    #[bits(1279..=1283)]
    pub organ_preset_2_drawbar_4_ctrl_pedal: DrawbarMorph,
    #[bits(1284..=1287)]
    pub organ_preset_2_drawbar_5: Drawbar,
    #[bits(1288..=1292)]
    pub organ_preset_2_drawbar_5_wheel: DrawbarMorph,
    #[bits(1293..=1297)]
    pub organ_preset_2_drawbar_5_aftertouch: DrawbarMorph,
    #[bits(1298..=1302)]
    pub organ_preset_2_drawbar_5_ctrl_pedal: DrawbarMorph,
    #[bits(1303..=1306)]
    pub organ_preset_2_drawbar_6: Drawbar,
    #[bits(1307..=1311)]
    pub organ_preset_2_drawbar_6_wheel: DrawbarMorph,
    #[bits(1312..=1316)]
    pub organ_preset_2_drawbar_6_aftertouch: DrawbarMorph,
    #[bits(1317..=1321)]
    pub organ_preset_2_drawbar_6_ctrl_pedal: DrawbarMorph,
    #[bits(1322..=1325)]
    pub organ_preset_2_drawbar_7: Drawbar,
    #[bits(1326..=1330)]
    pub organ_preset_2_drawbar_7_wheel: DrawbarMorph,
    #[bits(1331..=1335)]
    pub organ_preset_2_drawbar_7_aftertouch: DrawbarMorph,
    #[bits(1336..=1340)]
    pub organ_preset_2_drawbar_7_ctrl_pedal: DrawbarMorph,
    #[bits(1341..=1344)]
    pub organ_preset_2_drawbar_8: Drawbar,
    #[bits(1345..=1349)]
    pub organ_preset_2_drawbar_8_wheel: DrawbarMorph,
    #[bits(1350..=1354)]
    pub organ_preset_2_drawbar_8_aftertouch: DrawbarMorph,
    #[bits(1355..=1359)]
    pub organ_preset_2_drawbar_8_ctrl_pedal: DrawbarMorph,
    #[bits(1360..=1363)]
    pub organ_preset_2_drawbar_9: Drawbar,
    #[bits(1364..=1368)]
    pub organ_preset_2_drawbar_9_wheel: DrawbarMorph,
    #[bits(1369..=1373)]
    pub organ_preset_2_drawbar_9_aftertouch: DrawbarMorph,
    #[bits(1374..=1378)]
    pub organ_preset_2_drawbar_9_ctrl_pedal: DrawbarMorph,
    #[bits(1379..=1379)]
    pub organ_preset_2_vibrato_on: bool,
    #[bits(1380..=1380)]
    pub organ_preset_2_percussion_on: bool,
    #[bits(1381..=1381)]
    pub organ_preset_2_percussion_harmonic_third: bool,
    #[bits(1382..=1382)]
    pub organ_preset_2_percussion_decay_fast: bool,
    #[bits(1383..=1383)]
    pub organ_preset_2_percussion_volume_soft: bool,
    #[bits(1424..=1424)]
    pub extern_on: bool,
    #[bits(1425..=1427)]
    pub extern_kb_zone: Selector<3>,
    #[bits(1430..=1432)]
    pub extern_octave_shift: RangedU8<7>,
    #[bits(1433..=1434)]
    pub extern_midi_velocity_curve: Selector<2>,
    #[bits(1435..=1439)]
    pub extern_midi_channel: Selector<5>,
    #[bits(1440..=1440)]
    pub extern_pitch_stick: bool,
    #[bits(1441..=1441)]
    pub extern_sustain_pedal: bool,
    #[bits(1442..=1442)]
    pub extern_midi_send_wheel: bool,
    #[bits(1443..=1443)]
    pub extern_midi_send_aftertouch: bool,
    #[bits(1444..=1444)]
    pub extern_midi_send_ctrl_pedal: bool,
    #[bits(1445..=1445)]
    pub extern_midi_send_swell: bool,
    #[bits(1446..=1447)]
    pub extern_midi_control: Selector<2>,
    #[bits(1448..=1454)]
    pub extern_midi_cc_number: Level,
    #[bits(1455..=1461)]
    pub extern_midi_cc_value: Level,
    /// ⚠️ The three morph slots below name an `extern_midi_cc` this body does not declare,
    /// so they bind to nothing. `extern_midi_cc_value` is the parameter beside them and
    /// the likely one they move; inferred from placement, not confirmed on hardware.
    #[bits(1462..=1469)]
    pub extern_midi_cc_wheel: MorphTarget,
    #[bits(1470..=1477)]
    pub extern_midi_cc_aftertouch: MorphTarget,
    #[bits(1478..=1485)]
    pub extern_midi_cc_ctrl_pedal: MorphTarget,
    #[bits(1486..=1486)]
    pub extern_midi_send_user_cc_on_load: bool,
    #[bits(1487..=1494)]
    pub extern_midi_bank_select_cc32: u8,
    #[bits(1495..=1502)]
    pub extern_midi_bank_select_cc00: u8,
    #[bits(1503..=1509)]
    pub extern_midi_program: Level,
    #[bits(1510..=1517)]
    pub extern_midi_program_wheel: MorphTarget,
    #[bits(1518..=1525)]
    pub extern_midi_program_aftertouch: MorphTarget,
    #[bits(1526..=1533)]
    pub extern_midi_program_ctrl_pedal: MorphTarget,
    #[bits(1534..=1534)]
    pub extern_midi_send_program_on_load: bool,
    #[bits(1535..=1541)]
    pub extern_volume: Level,
    #[bits(1542..=1549)]
    pub extern_volume_wheel: MorphTarget,
    #[bits(1550..=1557)]
    pub extern_volume_aftertouch: MorphTarget,
    #[bits(1558..=1565)]
    pub extern_volume_ctrl_pedal: MorphTarget,
    #[bits(1566..=1566)]
    pub extern_midi_send_volume_on_load: bool,
    #[bits(1567..=1567)]
    pub extern_midi_send_volume: bool,
    #[bits(1608..=1608)]
    pub rotary_speaker_on: bool,
    #[bits(1609..=1610)]
    pub rotary_speaker_source: Selector<2>,
    #[bits(1611..=1611)]
    pub effect_1_on: bool,
    #[bits(1612..=1613)]
    pub effect_1_source: Selector<2>,
    #[bits(1614..=1616)]
    pub effect_1_type: Effect1Type,
    #[bits(1617..=1617)]
    pub effect_1_master_clock: bool,
    #[bits(1618..=1624)]
    pub effect_1_rate: Rate,
    #[bits(1625..=1632)]
    pub effect_1_rate_wheel: MorphTarget,
    #[bits(1633..=1640)]
    pub effect_1_rate_aftertouch: MorphTarget,
    #[bits(1641..=1648)]
    pub effect_1_rate_ctrl_pedal: MorphTarget,
    #[bits(1649..=1655)]
    pub effect_1_amount: Level,
    #[bits(1656..=1663)]
    pub effect_1_amount_wheel: MorphTarget,
    #[bits(1664..=1671)]
    pub effect_1_amount_aftertouch: MorphTarget,
    #[bits(1672..=1679)]
    pub effect_1_amount_ctrl_pedal: MorphTarget,
    #[bits(1680..=1680)]
    pub effect_2_on: bool,
    #[bits(1681..=1682)]
    pub effect_2_source: Selector<2>,
    #[bits(1683..=1685)]
    pub effect_2_type: Effect2Type,
    #[bits(1686..=1692)]
    pub effect_2_rate: Rate,
    #[bits(1693..=1699)]
    pub effect_2_amount: Level,
    #[bits(1700..=1707)]
    pub effect_2_amount_wheel: MorphTarget,
    #[bits(1708..=1715)]
    pub effect_2_amount_aftertouch: MorphTarget,
    #[bits(1716..=1723)]
    pub effect_2_amount_ctrl_pedal: MorphTarget,
    #[bits(1724..=1724)]
    pub delay_on: bool,
    #[bits(1725..=1726)]
    pub delay_source: Selector<2>,
    #[bits(1727..=1727)]
    pub delay_master_clock: bool,
    #[bits(1728..=1734)]
    pub delay_tempo: Time,
    #[bits(1735..=1741)]
    pub delay_tempo_lsw: Time,
    #[bits(1742..=1749)]
    pub delay_tempo_wheel: MorphTarget,
    #[bits(1750..=1756)]
    pub delay_tempo_wheel_lsw: Level,
    #[bits(1757..=1764)]
    pub delay_tempo_aftertouch: MorphTarget,
    #[bits(1765..=1771)]
    pub delay_tempo_aftertouch_lsw: Level,
    #[bits(1772..=1779)]
    pub delay_tempo_ctrl_pedal: MorphTarget,
    #[bits(1780..=1786)]
    pub delay_tempo_ctrl_pedal_lsw: Level,
    #[bits(1787..=1793)]
    pub delay_mix: Level,
    #[bits(1794..=1801)]
    pub delay_mix_wheel: MorphTarget,
    #[bits(1802..=1809)]
    pub delay_mix_aftertouch: MorphTarget,
    #[bits(1810..=1817)]
    pub delay_mix_ctrl_pedal: MorphTarget,
    #[bits(1818..=1818)]
    pub delay_ping_pong: bool,
    #[bits(1819..=1820)]
    pub delay_filter: Selector<2>,
    #[bits(1821..=1827)]
    pub delay_feedback: Level,
    #[bits(1828..=1835)]
    pub delay_feedback_wheel: MorphTarget,
    #[bits(1836..=1843)]
    pub delay_feedback_aftertouch: MorphTarget,
    #[bits(1844..=1851)]
    pub delay_feedback_ctrl_pedal: MorphTarget,
    #[bits(1852..=1852)]
    pub delay_analog_mode: DelayCharacter,
    #[bits(1853..=1853)]
    pub amp_sim_eq_on: bool,
    #[bits(1854..=1855)]
    pub amp_sim_eq_source: Selector<2>,
    #[bits(1856..=1858)]
    pub amp_sim_eq_amp_type: AmpSimEqAmpType,
    #[bits(1859..=1865)]
    pub amp_sim_eq_treble: EqBand,
    #[bits(1866..=1872)]
    pub amp_sim_eq_mid_res: EqBand,
    #[bits(1873..=1879)]
    pub amp_sim_eq_bass_dry_wet: EqBand,
    #[bits(1880..=1886)]
    pub amp_sim_eq_mid_flt_freq: Frequency,
    #[bits(1887..=1894)]
    pub amp_sim_eq_mid_flt_freq_wheel: MorphTarget,
    #[bits(1895..=1902)]
    pub amp_sim_eq_mid_flt_freq_aftertouch: MorphTarget,
    #[bits(1903..=1910)]
    pub amp_sim_eq_mid_flt_freq_ctrl_pedal: MorphTarget,
    #[bits(1911..=1917)]
    pub amp_sim_eq_drive: Level,
    #[bits(1918..=1925)]
    pub amp_sim_eq_drive_wheel: MorphTarget,
    #[bits(1926..=1933)]
    pub amp_sim_eq_drive_aftertouch: MorphTarget,
    #[bits(1934..=1941)]
    pub amp_sim_eq_drive_ctrl_pedal: MorphTarget,
    #[bits(1942..=1942)]
    pub reverb_on: bool,
    #[bits(1943..=1945)]
    pub reverb_type: ReverbType,
    #[bits(1946..=1946)]
    pub reverb_bright: bool,
    #[bits(1947..=1953)]
    pub reverb_amount: Level,
    #[bits(1954..=1961)]
    pub reverb_amount_wheel: MorphTarget,
    #[bits(1962..=1969)]
    pub reverb_amount_aftertouch: MorphTarget,
    #[bits(1970..=1977)]
    pub reverb_amount_ctrl_pedal: MorphTarget,
    #[bits(1978..=1978)]
    pub compressor_on: bool,
    #[bits(1979..=1985)]
    pub compressor_amount: Level,
    #[bits(1986..=1986)]
    pub compressor_fast: CompressorResponse,
    #[bits(2064..=2066)]
    pub program_output_main: Selector<3>,
    #[bits(2067..=2068)]
    pub program_output_sub_source: Selector<2>,
    #[bits(2069..=2070)]
    pub program_output_sub_destination: Selector<2>,
}
