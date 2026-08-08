//! The sample panel.

use crate::formats::ne5::Level;
use crate::types::RangedU8;
use nord_bits_derive::bitbody;

// 0x46..0x4d — the sample panel.

#[bitbody(8)]
#[derive(Default)]
pub struct SamplePanel {
    #[bits(0..=6)]
    pub attack: Level,
    #[bits(7..=13)]
    pub decay_release: Level,
    /// Zero-based slot in the instrument's Samp Lib — the panel number minus one. A
    /// position, not an identity: adding or deleting samples renumbers it. Use
    /// [`id`](Self::id) to resolve the dependency.
    #[bits(14..=21)]
    pub number: u8,
    /// The sample (`.nsmp`) this program depends on, laid out exactly as
    /// [`PianoPanel::id`].
    #[bits(22..=53)]
    pub id: u32,
    #[bits(54..=55)]
    pub dynamics: RangedU8<3>,
    #[bits(56..=56)]
    pub filter: bool,
}
