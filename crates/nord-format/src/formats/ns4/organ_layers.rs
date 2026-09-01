//! One organ layer's stored state: drawbars and their morph targets, percussion,
//! vibrato, model and octave.
//!
//! ⚠️ A layer's **enable and volume are not in here** — the file packs those with
//! the other layers', a bit and 31 bits apart respectively, so they stay on the
//! owning body. This block is the part that repeats at a whole-byte stride.

use crate::components::{Drawbar, DrawbarMorph, KbZone4, OctaveShiftNibble, Selector};
#[nord_bits_derive::bitbody(29)]
pub struct OrganLayer {
    #[bits(0..=3)]
    pub kb_zones: KbZone4,
    #[bits(4..=7)]
    pub octave_shift: OctaveShiftNibble,
    #[bits(8..=8)]
    pub sustain_pedal_enabled: bool,
    #[bits(9..=11)]
    pub model: Selector<3>,
    #[bits(12..=12)]
    pub preset_enabled: bool,
    #[bits(32..=35)]
    pub drawbar_1: Drawbar,
    #[bits(36..=40)]
    pub drawbar_1_wheel: DrawbarMorph,
    #[bits(41..=45)]
    pub drawbar_1_aftertouch: DrawbarMorph,
    #[bits(46..=50)]
    pub drawbar_1_ctrl_pedal: DrawbarMorph,
    #[bits(51..=54)]
    pub drawbar_2: Drawbar,
    #[bits(55..=59)]
    pub drawbar_2_wheel: DrawbarMorph,
    #[bits(60..=64)]
    pub drawbar_2_aftertouch: DrawbarMorph,
    #[bits(65..=69)]
    pub drawbar_2_ctrl_pedal: DrawbarMorph,
    #[bits(70..=73)]
    pub drawbar_3: Drawbar,
    #[bits(74..=78)]
    pub drawbar_3_wheel: DrawbarMorph,
    #[bits(79..=83)]
    pub drawbar_3_aftertouch: DrawbarMorph,
    #[bits(84..=88)]
    pub drawbar_3_ctrl_pedal: DrawbarMorph,
    #[bits(89..=92)]
    pub drawbar_4: Drawbar,
    #[bits(93..=97)]
    pub drawbar_4_wheel: DrawbarMorph,
    #[bits(98..=102)]
    pub drawbar_4_aftertouch: DrawbarMorph,
    #[bits(103..=107)]
    pub drawbar_4_ctrl_pedal: DrawbarMorph,
    #[bits(108..=111)]
    pub drawbar_5: Drawbar,
    #[bits(112..=116)]
    pub drawbar_5_wheel: DrawbarMorph,
    #[bits(117..=121)]
    pub drawbar_5_aftertouch: DrawbarMorph,
    #[bits(122..=126)]
    pub drawbar_5_ctrl_pedal: DrawbarMorph,
    #[bits(127..=130)]
    pub drawbar_6: Drawbar,
    #[bits(131..=135)]
    pub drawbar_6_wheel: DrawbarMorph,
    #[bits(136..=140)]
    pub drawbar_6_aftertouch: DrawbarMorph,
    #[bits(141..=145)]
    pub drawbar_6_ctrl_pedal: DrawbarMorph,
    #[bits(146..=149)]
    pub drawbar_7: Drawbar,
    #[bits(150..=154)]
    pub drawbar_7_wheel: DrawbarMorph,
    #[bits(155..=159)]
    pub drawbar_7_aftertouch: DrawbarMorph,
    #[bits(160..=164)]
    pub drawbar_7_ctrl_pedal: DrawbarMorph,
    #[bits(165..=168)]
    pub drawbar_8: Drawbar,
    #[bits(169..=173)]
    pub drawbar_8_wheel: DrawbarMorph,
    #[bits(174..=178)]
    pub drawbar_8_aftertouch: DrawbarMorph,
    #[bits(179..=183)]
    pub drawbar_8_ctrl_pedal: DrawbarMorph,
    #[bits(184..=187)]
    pub drawbar_9: Drawbar,
    #[bits(188..=192)]
    pub drawbar_9_wheel: DrawbarMorph,
    #[bits(193..=197)]
    pub drawbar_9_aftertouch: DrawbarMorph,
    #[bits(198..=202)]
    pub drawbar_9_ctrl_pedal: DrawbarMorph,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::ControlKind;

    /// Nine bars, each declaring which one it is, and each with its three morph slots
    /// bound to it — the two relations a caller would otherwise have to read out of the
    /// field names itself.
    #[test]
    fn every_bar_declares_its_rank_and_owns_its_morph_slots() {
        let specs = OrganLayer::field_specs();
        let spec = |name: &str| {
            specs
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no field {name}"))
        };

        for rank in 1..=9u8 {
            assert!(
                matches!(
                    spec(&format!("drawbar_{rank}")).control,
                    ControlKind::Drawbar {
                        bars: 1,
                        rank: Some(declared),
                        ..
                    } if declared == rank,
                ),
                "drawbar_{rank}",
            );
            for control in ["wheel", "aftertouch", "ctrl_pedal"] {
                let slot = spec(&format!("drawbar_{rank}_{control}"));
                assert_eq!(
                    slot.morph_parent(),
                    Some(format!("drawbar_{rank}")),
                    "{}",
                    slot.name,
                );
            }
        }

        // A field that is not a drawbar keeps what its type said, whatever its name ends
        // in.
        assert_eq!(spec("kb_zones").control, ControlKind::Selector);
    }
}
