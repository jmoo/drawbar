//! The Nord Stage 4 effects chain: one section's modulation, amp sim / EQ,
//! compressor, delay and reverb, 52 bytes of it.
//!
//! Every section carries its own. A program holds six — the organ's, which its two
//! layers share, plus one per piano and synth layer — and the preset banks carry
//! theirs, so this one declaration is placed twelve times across the Stage 4
//! formats.
//!
//! Values are raw, as everywhere in [the Stage 4 modules](super).

use crate::types::RangedU8;

#[nord_bits_derive::bitbody(52)]
pub struct FxChain {
    #[bits(0..=0)]
    pub mod_1_enabled: bool,
    #[bits(1..=1)]
    pub mod_1_master_clock_enabled: bool,
    #[bits(2..=8)]
    pub mod_1_rate: RangedU8<127>,
    #[bits(9..=16)]
    pub mod_1_rate_wheel: u8,
    #[bits(17..=24)]
    pub mod_1_rate_aftertouch: u8,
    #[bits(25..=32)]
    pub mod_1_rate_ctrl_pedal: u8,
    #[bits(33..=39)]
    pub mod_1_amount: RangedU8<127>,
    #[bits(40..=47)]
    pub mod_1_amount_wheel: u8,
    #[bits(48..=55)]
    pub mod_1_amount_aftertouch: u8,
    #[bits(56..=63)]
    pub mod_1_amount_ctrl_pedal: u8,
    #[bits(64..=67)]
    pub mod_1_mode: RangedU8<15>,
    #[bits(68..=68)]
    pub mod_2_enabled: bool,
    #[bits(69..=75)]
    pub mod_2_rate: RangedU8<127>,
    #[bits(76..=83)]
    pub mod_2_rate_wheel: u8,
    #[bits(84..=91)]
    pub mod_2_rate_aftertouch: u8,
    #[bits(92..=99)]
    pub mod_2_rate_ctrl_pedal: u8,
    #[bits(100..=106)]
    pub mod_2_amount: RangedU8<127>,
    #[bits(107..=114)]
    pub mod_2_amount_wheel: u8,
    #[bits(115..=122)]
    pub mod_2_amount_aftertouch: u8,
    #[bits(123..=130)]
    pub mod_2_amount_ctrl_pedal: u8,
    #[bits(131..=134)]
    pub mod_2_mode: RangedU8<15>,
    #[bits(135..=135)]
    pub amp_sim_eq_enabled: bool,
    #[bits(136..=142)]
    pub amp_sim_eq_treb: RangedU8<127>,
    #[bits(143..=149)]
    pub amp_sim_eq_mid: RangedU8<127>,
    #[bits(150..=156)]
    pub amp_sim_eq_bass: RangedU8<127>,
    #[bits(157..=163)]
    pub amp_sim_eq_freq: RangedU8<127>,
    #[bits(164..=171)]
    pub amp_sim_eq_freq_wheel: u8,
    #[bits(172..=179)]
    pub amp_sim_eq_freq_aftertouch: u8,
    #[bits(180..=187)]
    pub amp_sim_eq_freq_ctrl_pedal: u8,
    #[bits(188..=194)]
    pub amp_sim_eq_drive: RangedU8<127>,
    #[bits(195..=202)]
    pub amp_sim_eq_drive_wheel: u8,
    #[bits(203..=210)]
    pub amp_sim_eq_drive_aftertouch: u8,
    #[bits(211..=218)]
    pub amp_sim_eq_drive_ctrl_pedal: u8,
    #[bits(223..=223)]
    pub comp_enabled: bool,
    #[bits(224..=230)]
    pub comp_amount: RangedU8<127>,
    #[bits(231..=231)]
    pub comp_response: bool,
    #[bits(232..=232)]
    pub delay_enabled: bool,
    #[bits(233..=233)]
    pub delay_tempo_master_clock_enabled: bool,
    #[bits(234..=240)]
    pub delay_tempo: RangedU8<127>,
    #[bits(248..=255)]
    pub delay_tempo_wheel: u8,
    #[bits(256..=263)]
    pub delay_tempo_aftertouch: u8,
    #[bits(264..=271)]
    pub delay_tempo_ctrl_pedal: u8,
    #[bits(293..=299)]
    pub delay_mix: RangedU8<127>,
    #[bits(300..=307)]
    pub delay_mix_wheel: u8,
    #[bits(308..=315)]
    pub delay_mix_aftertouch: u8,
    #[bits(316..=323)]
    pub delay_mix_ctrl_pedal: u8,
    #[bits(324..=324)]
    pub delay_normal_analog: bool,
    #[bits(325..=325)]
    pub delay_ping_pong_enabled: bool,
    #[bits(326..=327)]
    pub delay_filter_type: RangedU8<3>,
    #[bits(328..=334)]
    pub delay_feedback: RangedU8<127>,
    #[bits(335..=342)]
    pub delay_feedback_wheel: u8,
    #[bits(343..=350)]
    pub delay_feedback_aftertouch: u8,
    #[bits(351..=358)]
    pub delay_feedback_ctrl_pedal: u8,
    #[bits(359..=362)]
    pub delay_effects: RangedU8<15>,
    #[bits(363..=363)]
    pub reverb_enabled: bool,
    #[bits(364..=370)]
    pub reverb_amount: RangedU8<127>,
    #[bits(371..=378)]
    pub reverb_amount_wheel: u8,
    #[bits(379..=386)]
    pub reverb_amount_aftertouch: u8,
    #[bits(387..=394)]
    pub reverb_amount_ctrl_pedal: u8,
    #[bits(395..=396)]
    pub reverb_dark_bright: RangedU8<3>,
    #[bits(397..=400)]
    pub reverb_type: RangedU8<15>,
    #[bits(405..=408)]
    pub amp_sim_eq_mode: RangedU8<15>,
}
