//! One Nord Stage 3 panel: a complete organ, piano, synth, extern and effects setup in
//! 263 bytes.
//!
//! A program holds two of these, Panel A and Panel B, and the Panel buttons switch
//! between them or layer both. They share one layout, so this type is placed twice;
//! see [`super::program::Program`].
//!
//! The synth block at panel byte 0x39 is a [`SynthPreset`], the body the Stage 3
//! synth preset (`ns3y`) stores under its own tag.

use super::program::*;
use super::synth::SynthPreset;
use crate::components::{
    CompressorResponse, DelayCharacter, Drawbar, DrawbarMorph, Effect1Type, Effect2Type, EqBand,
    Frequency, KbZone4, Level, MorphTarget, PianoRef, Rate, ReverbType, Selector, Time,
};
use crate::types::{RangedU16, RangedU8};

/// The panel's parameters. Bits are MSB-first from panel byte 0,
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
    pub piano_clavinet_model: ClavinetModel,
    /// The piano model's library id.
    #[bits(60..=91)]
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
    #[at(57..115)]
    pub synth: SynthPreset,
    #[bits(928..=928)]
    pub organ_on: bool,
    #[bits(929..=932)]
    pub organ_kb_zone: KbZone4,
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
    #[at(124..151)]
    pub organ_preset_1: OrganPreset,
    #[at(151..178)]
    pub organ_preset_2: OrganPreset,
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
    pub extern_midi_cc_number: RangedU8<127>,
    #[bits(1455..=1461)]
    pub extern_midi_cc: Level,
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
    pub extern_midi_program: RangedU8<127>,
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
    pub delay_tempo_lsw: RangedU8<127>,
    #[bits(1742..=1749)]
    pub delay_tempo_wheel: MorphTarget,
    #[bits(1750..=1756)]
    pub delay_tempo_wheel_lsw: RangedU8<127>,
    #[bits(1757..=1764)]
    pub delay_tempo_aftertouch: MorphTarget,
    #[bits(1765..=1771)]
    pub delay_tempo_aftertouch_lsw: RangedU8<127>,
    #[bits(1772..=1779)]
    pub delay_tempo_ctrl_pedal: MorphTarget,
    #[bits(1780..=1786)]
    pub delay_tempo_ctrl_pedal_lsw: RangedU8<127>,
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

/// One organ preset: nine drawbars with their morph slots, then the vibrato and
/// percussion switches. Bits are MSB-first from preset byte 0, which is panel byte 124
/// for preset 1 and 151 for preset 2.
#[nord_bits_derive::bitbody(27)]
pub struct OrganPreset {
    #[bits(0..=3)]
    pub drawbar_1: Drawbar,
    #[bits(4..=8)]
    pub drawbar_1_wheel: DrawbarMorph,
    #[bits(9..=13)]
    pub drawbar_1_aftertouch: DrawbarMorph,
    #[bits(14..=18)]
    pub drawbar_1_ctrl_pedal: DrawbarMorph,
    #[bits(19..=22)]
    pub drawbar_2: Drawbar,
    #[bits(23..=27)]
    pub drawbar_2_wheel: DrawbarMorph,
    #[bits(28..=32)]
    pub drawbar_2_aftertouch: DrawbarMorph,
    #[bits(33..=37)]
    pub drawbar_2_ctrl_pedal: DrawbarMorph,
    #[bits(38..=41)]
    pub drawbar_3: Drawbar,
    #[bits(42..=46)]
    pub drawbar_3_wheel: DrawbarMorph,
    #[bits(47..=51)]
    pub drawbar_3_aftertouch: DrawbarMorph,
    #[bits(52..=56)]
    pub drawbar_3_ctrl_pedal: DrawbarMorph,
    #[bits(57..=60)]
    pub drawbar_4: Drawbar,
    #[bits(61..=65)]
    pub drawbar_4_wheel: DrawbarMorph,
    #[bits(66..=70)]
    pub drawbar_4_aftertouch: DrawbarMorph,
    #[bits(71..=75)]
    pub drawbar_4_ctrl_pedal: DrawbarMorph,
    #[bits(76..=79)]
    pub drawbar_5: Drawbar,
    #[bits(80..=84)]
    pub drawbar_5_wheel: DrawbarMorph,
    #[bits(85..=89)]
    pub drawbar_5_aftertouch: DrawbarMorph,
    #[bits(90..=94)]
    pub drawbar_5_ctrl_pedal: DrawbarMorph,
    #[bits(95..=98)]
    pub drawbar_6: Drawbar,
    #[bits(99..=103)]
    pub drawbar_6_wheel: DrawbarMorph,
    #[bits(104..=108)]
    pub drawbar_6_aftertouch: DrawbarMorph,
    #[bits(109..=113)]
    pub drawbar_6_ctrl_pedal: DrawbarMorph,
    #[bits(114..=117)]
    pub drawbar_7: Drawbar,
    #[bits(118..=122)]
    pub drawbar_7_wheel: DrawbarMorph,
    #[bits(123..=127)]
    pub drawbar_7_aftertouch: DrawbarMorph,
    #[bits(128..=132)]
    pub drawbar_7_ctrl_pedal: DrawbarMorph,
    #[bits(133..=136)]
    pub drawbar_8: Drawbar,
    #[bits(137..=141)]
    pub drawbar_8_wheel: DrawbarMorph,
    #[bits(142..=146)]
    pub drawbar_8_aftertouch: DrawbarMorph,
    #[bits(147..=151)]
    pub drawbar_8_ctrl_pedal: DrawbarMorph,
    #[bits(152..=155)]
    pub drawbar_9: Drawbar,
    #[bits(156..=160)]
    pub drawbar_9_wheel: DrawbarMorph,
    #[bits(161..=165)]
    pub drawbar_9_aftertouch: DrawbarMorph,
    #[bits(166..=170)]
    pub drawbar_9_ctrl_pedal: DrawbarMorph,
    #[bits(171..=171)]
    pub vibrato_on: bool,
    #[bits(172..=172)]
    pub percussion_on: bool,
    #[bits(173..=173)]
    pub percussion_harmonic_third: bool,
    #[bits(174..=174)]
    pub percussion_decay_fast: bool,
    #[bits(175..=175)]
    pub percussion_volume_soft: bool,
}
