//! One organ layer's stored state: drawbars and their morph targets, percussion,
//! vibrato, model and octave.
//!
//! ⚠️ A layer's **enable and volume are not in here** — the file packs those with
//! the other layers', a bit and 31 bits apart respectively, so they stay on the
//! owning body. This block is the part that repeats at a whole-byte stride.

use crate::types::RangedU8;

#[nord_bits_derive::bitbody(29)]
pub struct OrganLayer {
    #[bits(0..=3)]
    pub kb_zones: RangedU8<15>,
    #[bits(4..=7)]
    pub octave_shift: RangedU8<15>,
    #[bits(8..=8)]
    pub sustain_pedal_enabled: bool,
    #[bits(9..=11)]
    pub model: RangedU8<7>,
    #[bits(12..=12)]
    pub preset_enabled: bool,
    #[bits(32..=35)]
    pub drawbar_1: RangedU8<15>,
    #[bits(36..=40)]
    pub drawbar_1_wheel: RangedU8<31>,
    #[bits(41..=45)]
    pub drawbar_1_aftertouch: RangedU8<31>,
    #[bits(46..=50)]
    pub drawbar_1_ctrl_pedal: RangedU8<31>,
    #[bits(51..=54)]
    pub drawbar_2: RangedU8<15>,
    #[bits(55..=59)]
    pub drawbar_2_wheel: RangedU8<31>,
    #[bits(60..=64)]
    pub drawbar_2_aftertouch: RangedU8<31>,
    #[bits(65..=69)]
    pub drawbar_2_ctrl_pedal: RangedU8<31>,
    #[bits(70..=73)]
    pub drawbar_3: RangedU8<15>,
    #[bits(74..=78)]
    pub drawbar_3_wheel: RangedU8<31>,
    #[bits(79..=83)]
    pub drawbar_3_aftertouch: RangedU8<31>,
    #[bits(84..=88)]
    pub drawbar_3_ctrl_pedal: RangedU8<31>,
    #[bits(89..=92)]
    pub drawbar_4: RangedU8<15>,
    #[bits(93..=97)]
    pub drawbar_4_wheel: RangedU8<31>,
    #[bits(98..=102)]
    pub drawbar_4_aftertouch: RangedU8<31>,
    #[bits(103..=107)]
    pub drawbar_4_ctrl_pedal: RangedU8<31>,
    #[bits(108..=111)]
    pub drawbar_5: RangedU8<15>,
    #[bits(112..=116)]
    pub drawbar_5_wheel: RangedU8<31>,
    #[bits(117..=121)]
    pub drawbar_5_aftertouch: RangedU8<31>,
    #[bits(122..=126)]
    pub drawbar_5_ctrl_pedal: RangedU8<31>,
    #[bits(127..=130)]
    pub drawbar_6: RangedU8<15>,
    #[bits(131..=135)]
    pub drawbar_6_wheel: RangedU8<31>,
    #[bits(136..=140)]
    pub drawbar_6_aftertouch: RangedU8<31>,
    #[bits(141..=145)]
    pub drawbar_6_ctrl_pedal: RangedU8<31>,
    #[bits(146..=149)]
    pub drawbar_7: RangedU8<15>,
    #[bits(150..=154)]
    pub drawbar_7_wheel: RangedU8<31>,
    #[bits(155..=159)]
    pub drawbar_7_aftertouch: RangedU8<31>,
    #[bits(160..=164)]
    pub drawbar_7_ctrl_pedal: RangedU8<31>,
    #[bits(165..=168)]
    pub drawbar_8: RangedU8<15>,
    #[bits(169..=173)]
    pub drawbar_8_wheel: RangedU8<31>,
    #[bits(174..=178)]
    pub drawbar_8_aftertouch: RangedU8<31>,
    #[bits(179..=183)]
    pub drawbar_8_ctrl_pedal: RangedU8<31>,
    #[bits(184..=187)]
    pub drawbar_9: RangedU8<15>,
    #[bits(188..=192)]
    pub drawbar_9_wheel: RangedU8<31>,
    #[bits(193..=197)]
    pub drawbar_9_aftertouch: RangedU8<31>,
    #[bits(198..=202)]
    pub drawbar_9_ctrl_pedal: RangedU8<31>,
    #[bits(224..=224)]
    pub vib_chorus_enabled: bool,
    #[bits(225..=225)]
    pub percussion_enabled: bool,
    #[bits(226..=226)]
    pub percussion_harmonic_3rd_enabled: bool,
    #[bits(227..=227)]
    pub percussion_decay_fast_enabled: bool,
    #[bits(228..=228)]
    pub percussion_volume_soft_enabled: bool,
}
