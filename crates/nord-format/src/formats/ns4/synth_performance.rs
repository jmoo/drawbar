//! One synth layer's performance half: keyboard zone, octave, voicing, glide, the
//! arpeggiator and the extern controls.
//!
//! ⚠️ A layer's **enable and volume are not in here** — the file packs those with
//! the other layers', a bit and 31 bits apart respectively, so they stay on the
//! owning body. This block is the part that repeats at a whole-byte stride.

use crate::types::{RangedU16, RangedU8};

#[nord_bits_derive::bitbody(47)]
pub struct SynthPerformance {
    #[bits(0..=0)]
    pub samples_analog: bool,
    #[bits(6..=17)]
    pub sample_slot: RangedU16<4095>,
    #[bits(18..=49)]
    pub sample_id: u32,
    #[bits(50..=53)]
    pub kb_zones: RangedU8<15>,
    #[bits(54..=57)]
    pub octave_shift: RangedU8<15>,
    #[bits(58..=58)]
    pub pitch_stick_enabled: bool,
    #[bits(59..=62)]
    pub pitch_stick_range: RangedU8<15>,
    #[bits(63..=63)]
    pub sustain_pedal_enabled: bool,
    #[bits(64..=66)]
    pub vibrato_mode: RangedU8<7>,
    #[bits(69..=69)]
    pub legato_enabled: bool,
    #[bits(70..=70)]
    pub mono_enabled: bool,
    #[bits(71..=72)]
    pub voice_priority: RangedU8<3>,
    #[bits(73..=79)]
    pub glide: RangedU8<127>,
    #[bits(80..=80)]
    pub extern_enabled: bool,
    #[bits(111..=117)]
    pub extern_program: RangedU8<127>,
    #[bits(126..=126)]
    pub kb_hold: bool,
    #[bits(127..=127)]
    pub arpeggiator_run_enabled: bool,
    #[bits(128..=129)]
    pub arpeggiator_mode: RangedU8<3>,
    #[bits(130..=130)]
    pub arp_pattern_enabled: bool,
    #[bits(131..=131)]
    pub kb_sync_enabled: bool,
    #[bits(132..=138)]
    pub arp_range_env: RangedU8<127>,
    #[bits(139..=146)]
    pub arp_range_env_wheel: u8,
    #[bits(147..=154)]
    pub arp_range_env_aftertouch: u8,
    #[bits(155..=162)]
    pub arp_range_env_ctrl_pedal: u8,
    #[bits(163..=164)]
    pub arp_direction: RangedU8<3>,
    #[bits(165..=165)]
    pub arp_zigzag_enabled: bool,
    #[bits(166..=166)]
    pub arp_master_clock_enabled: bool,
    #[bits(167..=173)]
    pub arp_rate_time: RangedU8<127>,
    #[bits(174..=181)]
    pub arp_rate_time_wheel: u8,
    #[bits(182..=189)]
    pub arp_rate_time_aftertouch: u8,
    #[bits(190..=197)]
    pub arp_rate_time_ctrl_pedal: u8,
    #[bits(198..=201)]
    pub arp_pattern_length: RangedU8<15>,
    #[bits(202..=233)]
    pub arpeggiator_accent: u32,
    #[bits(234..=265)]
    pub arpeggiator_gate: u32,
    #[bits(266..=297)]
    pub arpeggiator_pan: u32,
    #[bits(298..=299)]
    pub unison_level: RangedU8<3>,
    #[bits(300..=306)]
    pub extern_cc_val1: RangedU8<127>,
    #[bits(307..=314)]
    pub extern_cc_val1_wheel: u8,
    #[bits(315..=322)]
    pub extern_cc_val1_aftertouch: u8,
    #[bits(323..=330)]
    pub extern_cc_val1_ctrl_pedal: u8,
    #[bits(331..=337)]
    pub extern_cc_val2: RangedU8<127>,
    #[bits(338..=345)]
    pub extern_cc_val2_wheel: u8,
    #[bits(346..=353)]
    pub extern_cc_val2_aftertouch: u8,
    #[bits(354..=361)]
    pub extern_cc_val2_ctrl_pedal: u8,
    #[bits(364..=368)]
    pub vibrato_delay: RangedU8<31>,
}
