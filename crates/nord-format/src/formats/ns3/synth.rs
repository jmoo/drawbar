//! The Stage 3 synth preset body (`.ns3y`): 58 bytes.
//!
//! The program's synth block under its own tag. ⚠️ The community docs call it a
//! subset at program `0x0080..0x00AC`; the corpus puts it at program **body**
//! `0x4f`, found by locating preset content inside programs and confirmed by the
//! selector check. Panel B's copy sits 263 bytes further on, which is how the
//! program's two panels were found.
//!
//! Field names match [`super::program::Program`]'s, so the same parameter reads
//! the same either side of the tag.

use super::program::{
    SynthAmpEnvVelocity, SynthArpPattern, SynthArpRange, SynthFilterDrive, SynthFilterKbTrack,
    SynthFilterType, SynthLfoWave, SynthOscillatorConfig, SynthOscillatorType, SynthUnison,
    SynthVibrato, SynthVoice,
};
use crate::cbin::{self, Cbin};
use crate::components::{Frequency, Level, LibraryRef, MorphTarget, Rate, Time};
use crate::error::Error;
use crate::types::{RangedU16, RangedU8};
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns3y";
/// Stored ×100; every corpus specimen holds 3.00.
pub const KNOWN_VERSIONS: &[u32] = &[300];
pub const BODY_LEN: usize = 58;

#[nord_bits_derive::bitbody(58)]
pub struct SynthPreset {
    #[bits(40..=40)]
    pub synth_kb_hold: bool,
    #[bits(41..=41)]
    pub synth_arp_on: bool,
    #[bits(42..=42)]
    pub synth_arp_kb_sync: bool,
    #[bits(43..=44)]
    pub synth_arp_range: SynthArpRange,
    #[bits(45..=46)]
    pub synth_arp_pattern: SynthArpPattern,
    #[bits(47..=47)]
    pub synth_arp_master_clock: bool,
    #[bits(48..=54)]
    pub synth_arp_rate: Time,
    #[bits(55..=62)]
    pub synth_arp_rate_wheel: MorphTarget,
    #[bits(63..=70)]
    pub synth_arp_rate_aftertouch: MorphTarget,
    #[bits(71..=78)]
    pub synth_arp_rate_ctrl_pedal: MorphTarget,
    #[bits(79..=80)]
    pub synth_voice: SynthVoice,
    #[bits(81..=87)]
    pub synth_glide: Time,
    #[bits(88..=89)]
    pub synth_unison: SynthUnison,
    #[bits(90..=92)]
    pub synth_vibrato: SynthVibrato,
    #[bits(93..=95)]
    pub synth_lfo_wave: SynthLfoWave,
    #[bits(96..=96)]
    pub synth_lfo_master_clock: bool,
    #[bits(97..=103)]
    pub synth_lfo_rate: Rate,
    #[bits(104..=111)]
    pub synth_lfo_rate_wheel: MorphTarget,
    #[bits(112..=119)]
    pub synth_lfo_rate_aftertouch: MorphTarget,
    #[bits(120..=127)]
    pub synth_lfo_rate_ctrl_pedal: MorphTarget,
    #[bits(128..=134)]
    pub synth_mod_env_attack: Time,
    #[bits(135..=141)]
    pub synth_mod_env_decay: Time,
    #[bits(142..=148)]
    pub synth_mod_env_release: Time,
    #[bits(149..=149)]
    pub synth_mod_env_velocity: bool,
    #[bits(150..=152)]
    pub synth_oscillator_type: SynthOscillatorType,
    #[bits(153..=161)]
    pub synth_oscillator_1_wave_form: RangedU16<511>,
    #[bits(163..=166)]
    pub synth_oscillator_config: SynthOscillatorConfig,
    #[bits(167..=172)]
    pub synth_pitch: RangedU8<63>,
    #[bits(173..=179)]
    pub synth_oscillator_control: Level,
    #[bits(180..=187)]
    pub synth_oscillator_control_wheel: MorphTarget,
    #[bits(188..=195)]
    pub synth_oscillator_control_aftertouch: MorphTarget,
    #[bits(196..=203)]
    pub synth_oscillator_control_ctrl_pedal: MorphTarget,
    #[bits(204..=210)]
    pub synth_oscillator_mod: Level,
    #[bits(211..=218)]
    pub synth_oscillator_mod_wheel: MorphTarget,
    #[bits(219..=226)]
    pub synth_oscillator_mod_aftertouch: MorphTarget,
    #[bits(227..=234)]
    pub synth_oscillator_mod_ctrl_pedal: MorphTarget,
    #[bits(235..=237)]
    pub synth_filter_type: SynthFilterType,
    #[bits(238..=244)]
    pub synth_filter_freq: Frequency,
    #[bits(245..=252)]
    pub synth_filter_freq_wheel: MorphTarget,
    #[bits(253..=260)]
    pub synth_filter_freq_aftertouch: MorphTarget,
    #[bits(261..=268)]
    pub synth_filter_freq_ctrl_pedal: MorphTarget,
    #[bits(269..=275)]
    pub synth_filter_hp_freq_res: Level,
    #[bits(276..=283)]
    pub synth_filter_hp_freq_res_wheel: MorphTarget,
    #[bits(284..=291)]
    pub synth_filter_hp_freq_res_aftertouch: MorphTarget,
    #[bits(292..=299)]
    pub synth_filter_hp_freq_res_ctrl_pedal: MorphTarget,
    #[bits(300..=306)]
    pub synth_filter_lfo_amount: Level,
    #[bits(307..=314)]
    pub synth_filter_lfo_amount_wheel: MorphTarget,
    #[bits(315..=322)]
    pub synth_filter_lfo_amount_aftertouch: MorphTarget,
    #[bits(323..=330)]
    pub synth_filter_lfo_amount_ctrl_pedal: MorphTarget,
    #[bits(331..=337)]
    pub synth_filter_vel_mod_env_amount: Level,
    #[bits(338..=339)]
    pub synth_filter_kb_track: SynthFilterKbTrack,
    #[bits(340..=341)]
    pub synth_filter_drive: SynthFilterDrive,
    #[bits(342..=348)]
    pub synth_amp_env_attack: Time,
    #[bits(349..=355)]
    pub synth_amp_env_decay: Time,
    #[bits(356..=362)]
    pub synth_amp_env_release: Time,
    #[bits(363..=364)]
    pub synth_amp_env_velocity: SynthAmpEnvVelocity,
    #[bits(365..=396)]
    pub synth_sample_id: LibraryRef,
    #[bits(397..=397)]
    pub synth_fast_attack: bool,
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<SynthPreset>, Error> {
    let file: Cbin<SynthPreset> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
