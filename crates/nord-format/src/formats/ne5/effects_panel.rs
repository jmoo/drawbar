//! The effects panel: the four effect slots, the reverb, the rotary and the EQ.

use crate::bits::Packed;
use crate::components::sparse_enum;
use crate::components::{EqBand, Frequency, Rate, RotorSpeed, Time};
use crate::formats::ne5::Level;
use crate::types::RangedU8;
use nord_bits_derive::bitbody;

use std::fmt::{self, Display, Formatter};

// File offsets 0x93..=0xa4.

/// The effects panel: the four effect slots, the reverb, the rotary and the
/// EQ.
#[bitbody(18)]
#[derive(Default)]
pub struct EffectsPanel {
    #[bits(0..=1)]
    pub fx1: Routing,
    #[bits(2..=5)]
    pub fx1_type: Fx1Type,
    /// The effect's rate, in hertz. ⚠️ Not a [`Level`]: the panel does not show it on the
    /// 0..10 scale.
    #[bits(6..=12)]
    pub fx1_rate: Rate,
    #[bits(13..=14)]
    pub fx2: Routing,
    #[bits(15..=18)]
    pub fx2_type: Fx2Type,
    #[bits(19..=25)]
    pub fx2_rate: Rate,
    #[bits(26..=27)]
    pub fx4: Routing,
    #[bits(28..=29)]
    pub fx4_feedback: RangedU8<3>,
    /// Delay time. ⚠️ Runs backward: the panel reads 750 ms at 0 and 20 ms at 127, so it
    /// is not on the 0..10 scale.
    #[bits(30..=36)]
    pub fx4_tempo: Time,
    /// Delay wet/dry.
    #[bits(37..=43)]
    pub fx4_moisture: Level,
    #[bits(44..=44)]
    pub fx4_ping_pong: bool,
    /// EQ engaged.
    #[bits(45..=45)]
    pub equalizer_on: bool,
    /// Which part the equalizer applies to. [`equalizer_on`](Self::equalizer_on) says
    /// whether it is engaged.
    #[bits(117..=118)]
    pub equalizer_part: EqualizerPart,
    /// The sweepable mid frequency, in hertz.
    #[bits(47..=53)]
    pub equalizer_freq: Frequency,
    #[bits(54..=60)]
    pub equalizer_treble: EqBand,
    /// The mid band's boost or cut. ⚠️ Bipolar: zero is the center of the range, so a
    /// [`Level`]'s 0..10 reading would show a cut as a small boost.
    #[bits(61..=67)]
    pub equalizer_freq_gain: EqBand,
    #[bits(68..=74)]
    pub equalizer_bass: EqBand,
    #[bits(75..=76)]
    pub fx3: Routing,
    #[bits(77..=79)]
    pub fx3_type: Fx3Type,
    #[bits(80..=86)]
    pub fx3_compression: Level,
    #[bits(87..=87)]
    pub fx5: bool,
    #[bits(88..=90)]
    pub fx5_type: Fx5Type,
    #[bits(91..=97)]
    pub fx5_moisture: Level,
    #[bits(98..=98)]
    pub rotary_stop: bool,
    #[bits(99..=99)]
    pub rotary_speed: RotorSpeed,
    /// fx1 control pedal.
    #[bits(115..=115)]
    pub fx1_control: bool,
    /// fx2 deep.
    #[bits(116..=116)]
    pub fx2_deep: bool,
}

/// Which part an effect is routed to.
///
/// Off is stored as `0`, and the two engaged positions as `2` and `3`.
///
/// | stored | 0 | 1 | 2 | 3 |
/// |---|---|---|---|---|
/// | | off | [`Unknown`](Self::Unknown) | lower | upper |
///
/// Total over two bits, so decoding cannot fail.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Routing {
    #[default]
    Off = 0,
    /// Off, as older firmware wrote it.
    ///
    /// It behaves as off (no light, no effect), and a front-panel store preserves it,
    /// while the panel itself writes off as `0`. Confirmed on hardware. That it is the
    /// encoding before firmware 2.04: Inferred from specimens; not confirmed on hardware.
    /// Every program holding it is a factory program or predates the 2.04 update.
    Unknown = 1,
    Lower = 2,
    Upper = 3,
}

impl Routing {
    /// Which part the effect actually reaches, or `None` when it is not engaged.
    pub fn part(&self) -> Option<&'static str> {
        match self {
            Routing::Lower => Some("lower"),
            Routing::Upper => Some("upper"),
            Routing::Off | Routing::Unknown => None,
        }
    }

    /// Whether this is the value with no known meaning. It occurs in real programs.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Routing::Unknown)
    }
}

impl Packed for Routing {
    const MAX_BITS: u32 = 2;
    const DECODE_BITS: u32 = 2;
    const CONTROL: crate::fields::ControlKind = crate::fields::ControlKind::Selector;
    type Error = std::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(match bits & 0b11 {
            0 => Routing::Off,
            1 => Routing::Unknown,
            2 => Routing::Lower,
            _ => Routing::Upper,
        })
    }

    fn to_bits(&self) -> u64 {
        *self as u64
    }
}

impl Display for Routing {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Routing::Off => f.write_str("off"),
            Routing::Unknown => f.write_str("unknown (1)"),
            Routing::Lower => f.write_str("lower"),
            Routing::Upper => f.write_str("upper"),
        }
    }
}

sparse_enum!(
    /// Effect 1's modulation type.
    ///
    /// Values are as stored, which is rotated from the panel's order: stored 0 is trem 1,
    /// not pan 1. Inferred from specimens; not confirmed on hardware. Each specimen is
    /// named for the panel setting it was stored from.
    Fx1Type, 4, {
        0 => Trem1, "trem 1";
        1 => Trem2, "trem 2";
        2 => Trem1And2, "trem 1&2";
        3 => Pan1, "pan 1";
        4 => Pan2, "pan 2";
        5 => Pan1And2, "pan 1&2";
        6 => Wah, "wah";
        7 => Rm, "rm";
    }
);

sparse_enum!(
    /// Effect 2's modulation type.
    Fx2Type, 4, {
        0 => Phaser1, "phaser 1";
        1 => Phaser2, "phaser 2";
        2 => Flanger, "flanger";
        3 => Chorus1, "chorus 1";
        4 => Chorus2, "chorus 2";
        5 => Vibe, "vibe";
    }
);

sparse_enum!(
    /// The speaker or amp simulation.
    Fx3Type, 3, {
        0 => None_, "none";
        1 => Small, "small";
        2 => Jc, "jc";
        3 => Twin, "twin";
        4 => Rotary, "rotary";
        5 => Comp, "comp";
    }
);

sparse_enum!(
    /// The reverb algorithm.
    Fx5Type, 3, {
        0 => Room, "room";
        1 => StageSoft, "stage soft";
        2 => Stage, "stage";
        3 => HallSoft, "hall soft";
        4 => Hall, "hall";
    }
);

sparse_enum!(
    /// Which part the equalizer applies to. Whether it is engaged is a separate bit, so
    /// `Lower` means lower, not off.
    EqualizerPart, 2, {
        0 => Lower, "lower";
        1 => Upper, "upper";
        2 => Both, "lower+upper";
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecognized_value_survives_and_announces_itself() {
        let unknown = Fx1Type::from_bits(9).unwrap();
        assert_eq!(unknown, Fx1Type::Unknown(9));
        assert!(unknown.is_unknown());
        assert_eq!(unknown.label(), None);
        assert_eq!(unknown.to_string(), "unknown (9)");
        assert_eq!(unknown.to_bits(), 9, "an unknown value must round-trip");
    }

    #[test]
    fn recovered_values_round_trip() {
        for bits in 0..8u64 {
            let t = Fx1Type::from_bits(bits).unwrap();
            assert!(!t.is_unknown(), "{bits} should be recovered");
            assert_eq!(t.to_bits(), bits);
        }
        for bits in 0..3u64 {
            assert_eq!(EqualizerPart::from_bits(bits).unwrap().to_bits(), bits);
        }
    }

    #[test]
    fn routing_matches_what_the_instrument_stores() {
        assert_eq!(Routing::from_bits(0).unwrap(), Routing::Off);
        assert_eq!(Routing::from_bits(1).unwrap(), Routing::Unknown);
        assert_eq!(Routing::from_bits(2).unwrap(), Routing::Lower);
        assert_eq!(Routing::from_bits(3).unwrap(), Routing::Upper);

        for bits in 0..4u64 {
            assert_eq!(Routing::from_bits(bits).unwrap().to_bits(), bits);
        }

        // The panel's three positions are 0, 1 and 2; the engaged two store 2 and 3.
        assert_eq!(Routing::Lower.to_bits(), 2);
        assert_eq!(Routing::Upper.to_bits(), 3);

        // The unknown state must not render as `off`.
        assert_eq!(Routing::Off.to_string(), "off");
        assert_eq!(Routing::Unknown.to_string(), "unknown (1)");
        assert!(Routing::Unknown.is_unknown());
        assert!(!Routing::Off.is_unknown());
        assert_eq!(Routing::Unknown.part(), None);
    }
}
