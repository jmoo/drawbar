//! The piano panel.

use crate::components::sparse_enum;
use crate::types::RangedU8;
use nord_bits_derive::bitbody;

// 0x3a..0x41 — the piano panel.

#[bitbody(8)]
#[derive(Default)]
pub struct PianoPanel {
    #[bits(0..=2)]
    pub category: PianoCategory,
    /// Zero-based model slot *within* [`category`](Self::category) — the panel's
    /// Model dial. A slot coordinate, not an identity; see [`id`](Self::id).
    #[bits(5..=9)]
    pub piano_model: RangedU8<31>,
    #[bits(15..=16)]
    pub clav_model: RangedU8<3>,
    #[bits(17..=18)]
    pub acoustics: RangedU8<3>,
    #[bits(19..=20)]
    pub touch: RangedU8<3>,
    #[bits(21..=21)]
    pub mono: bool,
    /// The piano (`.npno`) this program depends on: a stable id, independent of where
    /// the piano sits in the library, and `0` when none is referenced. Use this — not
    /// [`category`](Self::category)/[`piano_model`](Self::piano_model), which are slot
    /// coordinates — to resolve the song → program → piano chain.
    ///
    /// It is the same id the instrument reports for this program over USB in a
    /// `DEPENDENCIES` reply, which is what lets a file on disk be matched to the library
    /// content it needs. The wire carries the piano's *name* too; the file does not, so
    /// resolving one to the other needs the device or a bundle manifest.
    #[bits(22..=53)]
    pub id: u32,
}
sparse_enum!(
    /// The piano panel's Type dial — which library category the model comes from.
    PianoCategory, 3, {
        0 => Grand, "grand";
        1 => Upright, "upright";
        2 => EPiano1, "epiano1";
        3 => EPiano2, "epiano2";
        4 => Clavinet, "clavinet";
        5 => Harpsichord, "harps";
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::array;

    /// Bits no field names survive a re-encode, because each panel keeps the bytes it
    /// was decoded from.
    #[test]
    fn unnamed_bits_survive_a_re_encode() {
        // `PianoPanel` bits 3..=4 and 10..=14 are named by nothing.
        const GAPS: [u8; 8] = [0b0001_1000, 0b0011_1110, 0, 0, 0, 0, 0, 0];
        fn gap_bits(raw: [u8; 8]) -> [u8; 8] {
            array::from_fn(|i| raw[i] & GAPS[i])
        }

        let mut panel = PianoPanel::try_from(GAPS).unwrap();
        panel.category = PianoCategory::EPiano1;
        panel.id = 0xdead_beef;
        let out = <[u8; 8]>::from(&panel);
        assert_eq!(gap_bits(out), GAPS, "a re-encode cleared a gap bit");

        let mut panel = PianoPanel::try_from([0; 8]).unwrap();
        panel.category = PianoCategory::Unknown(7);
        panel.piano_model = 31u8.try_into().unwrap();
        let out = <[u8; 8]>::from(&panel);
        assert_eq!(gap_bits(out), [0; 8], "a re-encode set a gap bit");
    }
}
