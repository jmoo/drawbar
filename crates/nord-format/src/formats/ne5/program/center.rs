//! The center panel — part selection, split, transpose, gain, and the organ selector.

use crate::components::sparse_enum;
use crate::components::PartMix;
use crate::formats::ne5::{Instrument, Level, OctaveShift, SplitPoint, Transpose};
use nord_bits_derive::bitbody;

// 0x2e..0x34 — the center panel.

#[bitbody(7)]
#[derive(Default)]
pub struct CenterPanel {
    #[bits(0..=2)]
    pub lower_part: Instrument,
    #[bits(3..=5)]
    pub upper_part: Instrument,
    #[bits(6..=9)]
    pub lower_octave_shift: OctaveShift,
    #[bits(10..=13)]
    pub upper_octave_shift: OctaveShift,
    #[bits(14..=14)]
    pub lower_sustain: bool,
    #[bits(15..=15)]
    pub upper_sustain: bool,
    #[bits(16..=16)]
    pub lower_control: bool,
    #[bits(17..=17)]
    pub upper_control: bool,
    /// Zero in every specimen; not confirmed on hardware.
    #[bits(18..=18)]
    pub unknown_boolean1: bool,
    #[bits(19..=19)]
    pub split: bool,
    #[bits(20..=22)]
    pub split_point: SplitPoint,
    /// Sticky: the instrument sets this the first time transposition is changed and never
    /// clears it again, so it stays true after the value is put back to 0. It marks that
    /// transposition has been *touched*, not that the program is transposed.
    ///
    /// The transpose light is on for `transpose_enabled && transpose != 0`. Neither field
    /// answers on its own — a caller reporting or editing transposition must read both.
    /// Confirmed on hardware.
    #[bits(23..=23)]
    pub transpose_enabled: bool,

    /// Half-step transposition, `-6..=6`, stored biased by 6.
    ///
    /// Carries no meaning while [`transpose_enabled`](Self::transpose_enabled) is clear: an
    /// untouched program stores `+1` there rather than `0`. Inferred from specimens; every
    /// specimen with the enable clear holds `+1`.
    #[bits(24..=27)]
    pub transpose: Transpose,
    #[bits(28..=34)]
    pub part_mix: PartMix,
    #[bits(35..=41)]
    pub gain: Level,
    #[bits(42..=44)]
    pub organ_type: OrganType,
    #[bits(45..=45)]
    pub lower_enabled: bool,
    #[bits(46..=46)]
    pub upper_enabled: bool,
    #[bits(47..=47)]
    pub drawbar_live: bool,
}

sparse_enum!(
    /// Which organ the program has selected.
    ///
    /// b3+bass shares the B3's storage rather than being a fifth model, and its preset 1
    /// is the bass manual — see [`Self::is_b3_bass`].
    OrganType, 3, {
        0 => B3, "b3";
        1 => B3Bass, "b3+bass";
        2 => Pipe, "pipe";
        3 => Vox, "vox";
        4 => Farfisa, "farfisa";
    }
);

impl OrganType {
    /// Which model's storage slots this selection reads from. b3 and b3+bass share
    /// [`OrganModel::B3`](crate::formats::ne5::program::OrganModel::B3); `None` for an
    /// unknown selection.
    pub fn storage(&self) -> Option<crate::formats::ne5::program::OrganModel> {
        use crate::formats::ne5::program::OrganModel;
        match self {
            OrganType::B3 | OrganType::B3Bass => Some(OrganModel::B3),
            OrganType::Pipe => Some(OrganModel::Pipe),
            OrganType::Vox => Some(OrganModel::Vox),
            OrganType::Farfisa => Some(OrganModel::Farfisa),
            OrganType::Unknown(_) => None,
        }
    }

    /// Whether preset 1 is the bass manual.
    ///
    /// **b3+bass is a selection, not a fifth model.** It shares the B3's storage, but its
    /// two presets are different instruments: preset 1 is the bass manual, where only
    /// drawbars 1-2 do anything and they live outside the nine-nibble block, and preset 2
    /// is an ordinary B3. Reading preset 1's nine nibbles in that mode shows stale
    /// values —
    /// [`OrganPanel::b3_bass_drawbars`](crate::formats::ne5::program::OrganPanel::b3_bass_drawbars)
    /// is the only correct source for bars 1-2.
    pub fn is_b3_bass(&self) -> bool {
        matches!(self, OrganType::B3Bass)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{self as program, OrganModel, FILE_LEN};
    use super::*;
    use crate::bits::Packed;
    use crate::types::RangedU8;
    use std::io::Cursor;

    /// An out-of-range value is not something a field can hold, so the refusal happens
    /// at construction rather than part-way through a write.
    #[test]
    fn an_out_of_range_value_is_refused_where_it_is_written() {
        // `panel.gain = 200;` does not compile: 200 is not a `RangedU8<127>`.
        let too_wide: Result<RangedU8<127>, _> = 200u8.try_into();
        assert!(too_wide.is_err(), "200 must not be a valid seven-bit gain");
        assert!(
            RangedU8::<127>::new(127).is_ok(),
            "127 is the largest that fits"
        );

        let mut program = program::new((0, 0).try_into().unwrap());
        program.center_panel.gain = 96u8.try_into().unwrap();

        let mut bytes = Vec::new();
        program
            .write_to(&mut Cursor::new(&mut bytes))
            .expect("a panel built from ranged values always writes");
        assert_eq!(bytes.len(), FILE_LEN);
    }

    /// A default panel has to encode, which zeroed bytes alone would not: an octave
    /// shift of zero is stored as 7, so all-zero bits decode as -7 — out of range.
    #[test]
    fn the_default_panel_encodes_and_decodes() {
        let panel = CenterPanel::default();
        let raw = <[u8; 7]>::from(&panel);
        let back = CenterPanel::try_from(raw).expect("default panel must decode");

        assert_eq!(back.lower_octave_shift, 0);
        assert_eq!(back.upper_octave_shift, 0);
        assert_eq!(back.transpose, 0);
        assert_eq!(back.lower_part, Instrument::Organ);
    }

    /// Every organ the panel can select round-trips through its stored value.
    #[test]
    fn organ_type_values_round_trip() {
        for bits in 0..5u64 {
            assert_eq!(OrganType::from_bits(bits).unwrap().to_bits(), bits);
        }
        assert!(OrganType::from_bits(6).unwrap().is_unknown());
        assert_eq!(OrganType::B3Bass.storage(), Some(OrganModel::B3));
        assert!(OrganType::B3Bass.is_b3_bass());
        assert!(!OrganType::B3.is_b3_bass());
    }
}
