//! The Nord Stage 4 effects chain: one section's modulation, amp sim / EQ,
//! compressor, delay and reverb, 52 bytes of it.
//!
//! Every section carries its own. A program holds six — the organ's, which its two
//! layers share, plus one per piano and synth layer — and the preset banks carry
//! theirs, so this one declaration is placed twelve times across the Stage 4
//! formats.
//!
//! Values are raw, as everywhere in [the Stage 4 modules](super).

use crate::components::{
    CompressorResponse, DelayCharacter, EqBand, Frequency, Level, MorphTarget, Rate, Selector, Time,
};
#[nord_bits_derive::bitbody(52)]
pub struct FxChain {
    #[bits(0..=0)]
    pub mod_1_enabled: bool,
    #[bits(1..=1)]
    pub mod_1_master_clock_enabled: bool,
    #[bits(2..=8)]
    pub mod_1_rate: Rate,
    #[bits(9..=16)]
    pub mod_1_rate_wheel: MorphTarget,
    #[bits(17..=24)]
    pub mod_1_rate_aftertouch: MorphTarget,
    #[bits(25..=32)]
    pub mod_1_rate_ctrl_pedal: MorphTarget,
    #[bits(33..=39)]
    pub mod_1_amount: Level,
    #[bits(40..=47)]
    pub mod_1_amount_wheel: MorphTarget,
    #[bits(48..=55)]
    pub mod_1_amount_aftertouch: MorphTarget,
    #[bits(56..=63)]
    pub mod_1_amount_ctrl_pedal: MorphTarget,
    #[bits(64..=67)]
    pub mod_1_mode: Selector<4>,
    #[bits(68..=68)]
    pub mod_2_enabled: bool,
    #[bits(69..=75)]
    pub mod_2_rate: Rate,
    #[bits(76..=83)]
    pub mod_2_rate_wheel: MorphTarget,
    #[bits(84..=91)]
    pub mod_2_rate_aftertouch: MorphTarget,
    #[bits(92..=99)]
    pub mod_2_rate_ctrl_pedal: MorphTarget,
    #[bits(100..=106)]
    pub mod_2_amount: Level,
    #[bits(107..=114)]
    pub mod_2_amount_wheel: MorphTarget,
    #[bits(115..=122)]
    pub mod_2_amount_aftertouch: MorphTarget,
    #[bits(123..=130)]
    pub mod_2_amount_ctrl_pedal: MorphTarget,
    #[bits(131..=134)]
    pub mod_2_mode: Selector<4>,
    #[bits(135..=135)]
    pub amp_sim_eq_enabled: bool,
    #[bits(136..=142)]
    pub amp_sim_eq_treb: EqBand,
    #[bits(143..=149)]
    pub amp_sim_eq_mid: EqBand,
    #[bits(150..=156)]
    pub amp_sim_eq_bass: EqBand,
    #[bits(157..=163)]
    pub amp_sim_eq_freq: Frequency,
    #[bits(164..=171)]
    pub amp_sim_eq_freq_wheel: MorphTarget,
    #[bits(172..=179)]
    pub amp_sim_eq_freq_aftertouch: MorphTarget,
    #[bits(180..=187)]
    pub amp_sim_eq_freq_ctrl_pedal: MorphTarget,
    #[bits(188..=194)]
    pub amp_sim_eq_drive: Level,
    #[bits(195..=202)]
    pub amp_sim_eq_drive_wheel: MorphTarget,
    #[bits(203..=210)]
    pub amp_sim_eq_drive_aftertouch: MorphTarget,
    #[bits(211..=218)]
    pub amp_sim_eq_drive_ctrl_pedal: MorphTarget,
    #[bits(223..=223)]
    pub comp_enabled: bool,
    #[bits(224..=230)]
    pub comp_amount: Level,
    #[bits(231..=231)]
    pub comp_response: CompressorResponse,
    #[bits(232..=232)]
    pub delay_enabled: bool,
    #[bits(233..=233)]
    pub delay_tempo_master_clock_enabled: bool,
    #[bits(234..=240)]
    pub delay_tempo: Time,
    #[bits(248..=255)]
    pub delay_tempo_wheel: MorphTarget,
    #[bits(256..=263)]
    pub delay_tempo_aftertouch: MorphTarget,
    #[bits(264..=271)]
    pub delay_tempo_ctrl_pedal: MorphTarget,
    #[bits(293..=299)]
    pub delay_mix: Level,
    #[bits(300..=307)]
    pub delay_mix_wheel: MorphTarget,
    #[bits(308..=315)]
    pub delay_mix_aftertouch: MorphTarget,
    #[bits(316..=323)]
    pub delay_mix_ctrl_pedal: MorphTarget,
    #[bits(324..=324)]
    pub delay_normal_analog: DelayCharacter,
    #[bits(325..=325)]
    pub delay_ping_pong_enabled: bool,
    #[bits(326..=327)]
    pub delay_filter_type: Selector<2>,
    #[bits(328..=334)]
    pub delay_feedback: Level,
    #[bits(335..=342)]
    pub delay_feedback_wheel: MorphTarget,
    #[bits(343..=350)]
    pub delay_feedback_aftertouch: MorphTarget,
    #[bits(351..=358)]
    pub delay_feedback_ctrl_pedal: MorphTarget,
    #[bits(359..=362)]
    pub delay_effects: Selector<4>,
    #[bits(363..=363)]
    pub reverb_enabled: bool,
    #[bits(364..=370)]
    pub reverb_amount: Level,
    #[bits(371..=378)]
    pub reverb_amount_wheel: MorphTarget,
    #[bits(379..=386)]
    pub reverb_amount_aftertouch: MorphTarget,
    #[bits(387..=394)]
    pub reverb_amount_ctrl_pedal: MorphTarget,
    #[bits(395..=396)]
    pub reverb_dark_bright: Selector<2>,
    #[bits(397..=400)]
    pub reverb_type: Selector<4>,
    #[bits(405..=408)]
    pub amp_sim_eq_mode: Selector<4>,
}
