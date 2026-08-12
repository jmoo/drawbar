//! One piano layer's stored state: model, keyboard zone, octave, timbre and the
//! acoustic options.
//!
//! ⚠️ A layer's **enable and volume are not in here** — the file packs those with
//! the other layers', a bit and 31 bits apart respectively, so they stay on the
//! owning body. This block is the part that repeats at a whole-byte stride.

use crate::components::{KbZone4, LibraryRef, OctaveShiftNibble, Selector};
#[nord_bits_derive::bitbody(9)]
pub struct PianoLayer {
    #[bits(0..=3)]
    pub kb_zones: KbZone4,
    #[bits(4..=7)]
    pub octave_shift: OctaveShiftNibble,
    #[bits(8..=8)]
    pub pitch_stick_enabled: bool,
    #[bits(9..=9)]
    pub sustain_pedal_enabled: bool,
    #[bits(10..=12)]
    pub piano_type: Selector<3>,
    #[bits(13..=17)]
    pub model_slot: Selector<5>,
    #[bits(18..=19)]
    pub model_variation: Selector<2>,
    #[bits(20..=51)]
    pub model_id: LibraryRef,
    #[bits(52..=52)]
    pub soft_rel_enabled: bool,
    #[bits(53..=53)]
    pub string_res_enabled: bool,
    #[bits(54..=54)]
    pub pedal_noise_enabled: bool,
    #[bits(55..=56)]
    pub touch: Selector<2>,
    #[bits(57..=58)]
    pub unison_level: Selector<2>,
    #[bits(59..=60)]
    pub dyn_comp: Selector<2>,
    #[bits(62..=64)]
    pub timbre: Selector<3>,
}
