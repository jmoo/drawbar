//! One synth layer's voice half: oscillator, filter, envelopes and LFO.
//!
//! ⚠️ A layer's **enable and volume are not in here** — the file packs those with
//! the other layers', a bit and 31 bits apart respectively, so they stay on the
//! owning body. This block is the part that repeats at a whole-byte stride.

use crate::types::RangedU8;

#[nord_bits_derive::bitbody(44)]
pub struct SynthVoice {
    #[bits(2..=3)]
    pub analog_type_knob_1: RangedU8<3>,
    #[bits(7..=9)]
    pub analog_cat_knob_2: RangedU8<7>,
    #[bits(11..=16)]
    pub analog_wave_partial_knob_3: RangedU8<63>,
    #[bits(17..=23)]
    pub osc_ctrl: RangedU8<127>,
    #[bits(24..=31)]
    pub osc_ctrl_wheel: u8,
    #[bits(32..=39)]
    pub osc_ctrl_aftertouch: u8,
    #[bits(40..=47)]
    pub osc_ctrl_ctrl_pedal: u8,
    #[bits(48..=54)]
    pub pitch_fine: RangedU8<127>,
    #[bits(55..=60)]
    pub pitch_coarse: RangedU8<63>,
    #[bits(63..=69)]
    pub osc_env_attack: RangedU8<127>,
    #[bits(70..=76)]
    pub osc_env_decay: RangedU8<127>,
    #[bits(77..=83)]
    pub osc_env_release: RangedU8<127>,
    #[bits(84..=90)]
    pub osc_env_amount: RangedU8<127>,
    #[bits(91..=98)]
    pub osc_env_amount_wheel: u8,
    #[bits(99..=106)]
    pub osc_env_amount_aftertouch: u8,
    #[bits(107..=114)]
    pub osc_env_amount_ctrl_pedal: u8,
    #[bits(115..=115)]
    pub osc_env_to_pitch_enabled: bool,
    #[bits(116..=116)]
    pub osc_env_velocity_enabled: bool,
    #[bits(117..=118)]
    pub sample_options: RangedU8<3>,
    #[bits(119..=120)]
    pub lfo_target: RangedU8<3>,
    #[bits(121..=123)]
    pub lfo_shape: RangedU8<7>,
    #[bits(124..=124)]
    pub lfo_master_clock_enabled: bool,
    #[bits(125..=131)]
    pub lfo_rate_time: RangedU8<127>,
    #[bits(132..=139)]
    pub lfo_rate_time_wheel: u8,
    #[bits(140..=147)]
    pub lfo_rate_time_aftertouch: u8,
    #[bits(148..=155)]
    pub lfo_rate_time_ctrl_pedal: u8,
    #[bits(156..=162)]
    pub lfo_mod_amount: RangedU8<127>,
    #[bits(163..=170)]
    pub lfo_mod_amount_wheel: u8,
    #[bits(171..=178)]
    pub lfo_mod_amount_aftertouch: u8,
    #[bits(179..=186)]
    pub lfo_mod_amount_ctrl_pedal: u8,
    #[bits(187..=193)]
    pub amp_env_attack: RangedU8<127>,
    #[bits(194..=200)]
    pub amp_env_decay: RangedU8<127>,
    #[bits(201..=207)]
    pub amp_env_release: RangedU8<127>,
    #[bits(208..=209)]
    pub amp_env_velocity: RangedU8<3>,
    #[bits(210..=212)]
    pub filter_type: RangedU8<7>,
    #[bits(213..=219)]
    pub filter_freq: RangedU8<127>,
    #[bits(220..=227)]
    pub filter_freq_wheel: u8,
    #[bits(228..=235)]
    pub filter_freq_aftertouch: u8,
    #[bits(236..=243)]
    pub filter_freq_ctrl_pedal: u8,
    #[bits(244..=250)]
    pub filter_resonance_freq_hp: RangedU8<127>,
    #[bits(251..=258)]
    pub filter_resonance_wheel: u8,
    #[bits(259..=266)]
    pub filter_resonance_aftertouch: u8,
    #[bits(267..=274)]
    pub filter_resonance_ctrl_pedal: u8,
    #[bits(275..=276)]
    pub filter_track: RangedU8<3>,
    #[bits(277..=278)]
    pub filter_drive: RangedU8<3>,
    #[bits(279..=285)]
    pub filter_env_amount: RangedU8<127>,
    #[bits(286..=293)]
    pub filter_env_amount_wheel: u8,
    #[bits(294..=301)]
    pub filter_env_amount_aftertouch: u8,
    #[bits(302..=309)]
    pub filter_env_amount_ctrl_pedal: u8,
    #[bits(310..=316)]
    pub filter_env_attack: RangedU8<127>,
    #[bits(317..=323)]
    pub filter_env_decay: RangedU8<127>,
    #[bits(324..=330)]
    pub filter_env_release: RangedU8<127>,
    #[bits(331..=331)]
    pub filter_velocity_enabled: bool,
    #[bits(332..=332)]
    pub filter_enabled: bool,
    #[bits(333..=339)]
    pub vibrato_rate: RangedU8<127>,
    #[bits(340..=345)]
    pub vibrato_amount: RangedU8<63>,
    #[bits(346..=346)]
    pub sample_bright_enabled: bool,
}
