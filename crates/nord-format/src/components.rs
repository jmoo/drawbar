//! Typed values shared across models.
//!
//! A component owns its encoding, its validation and its `Display`, and knows nothing
//! about which panel or offset holds it — so the same impl serves every `#[bits(...)]`
//! placement of that value.
//!
//! Only what more than one model uses belongs here. A component with a single consumer
//! lives beside that consumer, in the panel module that names it.

use std::fmt::{self, Debug, Display, Formatter};

use crate::bits::{bits_for, Packed};
use crate::error::ParseError;
use crate::types::RangedI8;

/// Octave shift. The range and the storage bias are the model's business, so each
/// names its own alias.
pub type OctaveShift<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;

/// Half-step transposition. As with [`OctaveShift`], the model fixes the parameters.
pub type Transpose<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;

/// A 0..=127 continuous control — rate, level, compression, gain, tone. The panel shows
/// most of these as 0..10.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level {
    inner: u8,
}

impl Level {
    pub const MAX: u8 = 127;

    pub fn new(value: u8) -> Result<Self, ParseError> {
        value.try_into()
    }

    /// The stored value, 0..=127.
    pub fn as_u8(&self) -> u8 {
        self.inner
    }

    /// The panel's 0..10 reading.
    ///
    /// Confirmed on hardware: reverb wet reads `43` in the file and the panel shows
    /// 3.4, and `43 / 127 * 10 = 3.39`.
    ///
    /// ⚠️ Not every 0..127 field is on this scale — fx2's rate is in Hz and delay tempo
    /// runs backwards in milliseconds, so those print the stored value.
    pub fn as_panel(&self) -> f32 {
        f32::from(self.inner) / 127.0 * 10.0
    }
}

impl TryFrom<u8> for Level {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, ParseError> {
        if value > Self::MAX {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: format!("0..={}", Self::MAX),
            });
        }
        Ok(Level { inner: value })
    }
}

impl Packed for Level {
    const MAX_BITS: u32 = bits_for(Level::MAX as u64);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u8).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl Display for Level {
    /// Stored byte and panel reading: `96 (7.6)`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:.1})", self.inner, self.as_panel())
    }
}

impl Debug for Level {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl PartialEq<u8> for Level {
    fn eq(&self, other: &u8) -> bool {
        self.inner == *other
    }
}

/// The balance between a split's lower and upper parts, as a 0..=127 crossfade.
///
/// ⚠️ Each side is clamped at 50, so the pair does not sum to 100 — a stored 16 reads
/// as `50.0/12.6`.
#[derive(Copy, Default, Clone, PartialEq, Eq)]
pub struct PartMix {
    inner: u8,
}

impl PartMix {
    pub fn inner(&self) -> u8 {
        self.inner
    }

    pub fn lower(&self) -> f32 {
        let lower = 100_f32 - ((self.inner() as f32) / 127.0) * 100_f32;

        if lower > 50_f32 {
            50_f32
        } else {
            lower
        }
    }

    pub fn upper(&self) -> f32 {
        let upper = ((self.inner() as f32) / 127.0) * 100_f32;

        if upper > 50_f32 {
            50_f32
        } else {
            upper
        }
    }

    pub fn as_string(&self) -> String {
        format!("{:.1}/{:.1}", self.lower(), self.upper())
    }

    pub fn as_tuple(&self) -> (f32, f32) {
        (self.lower(), self.upper())
    }
}

impl Debug for PartMix {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_string())
    }
}

impl Packed for PartMix {
    const MAX_BITS: u32 = 7;
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u8).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.inner() as u64
    }
}

impl TryFrom<u8> for PartMix {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value > 127 {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: "0..=127".to_string(),
            });
        }

        Ok(PartMix { inner: value })
    }
}

/// Percussion decay speed. How it is stored is per-model; the Electro 5's B3 does not
/// store it in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PercSpeed {
    Off,
    Soft,
    Fast,
    Both,
}

/// A keyboard split point as the 73-key models store it: one of six keys, or
/// the whole keyboard as Upper / Lower.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub enum SplitPoint73 {
    #[default]
    C3,
    F3,
    C4,
    F4,
    C5,
    F5,
    Upper,
    Lower,
}

impl TryFrom<u8> for SplitPoint73 {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<SplitPoint73, Self::Error> {
        match value {
            0 => Ok(SplitPoint73::C3),
            1 => Ok(SplitPoint73::F3),
            2 => Ok(SplitPoint73::C4),
            3 => Ok(SplitPoint73::F4),
            4 => Ok(SplitPoint73::C5),
            5 => Ok(SplitPoint73::F5),
            6 => Ok(SplitPoint73::Upper),
            7 => Ok(SplitPoint73::Lower),
            _ => Err("Value is out of range for split point"),
        }
    }
}

impl Packed for SplitPoint73 {
    const MAX_BITS: u32 = 3;
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        SplitPoint73::try_from(bits as u8).map_err(|_| ParseError::OutOfBounds {
            value: format!("{bits}"),
            bound: "0..=7 (SplitPoint73)".to_string(),
        })
    }

    fn to_bits(&self) -> u64 {
        *self as u64
    }
}

/// A vibrato (`V`) or chorus (`C`) organ modulation at one of three depths.
///
/// Which subset an organ offers is the model's business, and so is the index each sits
/// at — see the per-model tables beside the organ panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VibChorus {
    V1,
    C1,
    V2,
    C2,
    V3,
    C3,
}

/// A Stage program's transpose slot: stored `0..=12`, biased by 6, reading
/// `-6..=+6` semitones.
///
/// ⚠️ Not a [`RangedI8`]: the Stage 2 EX factory live
/// buffers hold 15 in this slot — an untouched buffer stores an out-of-table
/// pattern — so the unknown patterns are preserved rather than refused.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StageTranspose {
    raw: u8,
}

impl StageTranspose {
    /// The stored 4-bit pattern.
    pub fn raw(&self) -> u8 {
        self.raw
    }

    /// The semitone reading, or `None` for a pattern past the panel's `+6`.
    pub fn semitones(&self) -> Option<i8> {
        (self.raw <= 12).then(|| self.raw as i8 - 6)
    }
}

impl Packed for StageTranspose {
    const MAX_BITS: u32 = 4;
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        Ok(StageTranspose { raw: bits as u8 })
    }

    fn to_bits(&self) -> u64 {
        self.raw as u64
    }
}

impl Debug for StageTranspose {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.semitones() {
            Some(s) => write!(f, "{s}"),
            None => write!(f, "unknown ({})", self.raw),
        }
    }
}

/// The master clock rate the Stage 2 and 3 store in a program: `stored + 30` BPM.
///
/// Inferred from the Nord User Forum's ns3-program-viewer documentation
/// (github.com/Chris55/ns3-program-viewer); not confirmed on hardware.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MasterTempo {
    inner: u8,
}

impl MasterTempo {
    /// The stored byte.
    pub fn as_u8(&self) -> u8 {
        self.inner
    }

    /// The panel's BPM reading.
    pub fn bpm(&self) -> u16 {
        self.inner as u16 + 30
    }
}

impl Packed for MasterTempo {
    const MAX_BITS: u32 = 8;
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        Ok(MasterTempo { inner: bits as u8 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl Debug for MasterTempo {
    /// The BPM reading — the stored byte is recoverable as `bpm - 30`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.bpm())
    }
}

/// Declare a sparse enumeration: known values, plus `Unknown` for the rest of the slot.
///
/// The slot is wider than the set of values we have names for, so anything unrecognized
/// decodes to `Unknown`, round-trips byte-exactly, and displays as `unknown (9)` — never
/// coerced to the nearest label. Match on it, or call `is_unknown()`, to find them.
macro_rules! sparse_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $bits:expr, { $($value:expr => $variant:ident, $label:expr;)+ }
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($variant,)+
            /// A stored value with no known meaning.
            Unknown(u8),
        }

        /// Named variants as their names; an unknown as `unknown (raw)`. ⚠️ The corpus
        /// tripwires match the lowercase spelling — a derived `Unknown(raw)` slips past
        /// them.
        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    $($name::$variant => f.write_str(stringify!($variant)),)+
                    $name::Unknown(raw) => write!(f, "unknown ({raw})"),
                }
            }
        }

        impl $name {
            /// The label, or `None` for a value with no known meaning.
            pub fn label(&self) -> Option<&'static str> {
                match self {
                    $($name::$variant => Some($label),)+
                    $name::Unknown(_) => None,
                }
            }

            /// Whether the stored value has no known meaning.
            pub fn is_unknown(&self) -> bool {
                matches!(self, $name::Unknown(_))
            }

            /// The stored value, named or not.
            pub fn raw(&self) -> u8 {
                <Self as $crate::bits::Packed>::to_bits(self) as u8
            }
        }

        impl Default for $name {
            fn default() -> Self {
                <Self as $crate::bits::Packed>::from_bits(0).expect("decoding is total")
            }
        }

        impl $crate::bits::Packed for $name {
            const MAX_BITS: u32 = $bits;
            type Error = ::core::convert::Infallible;

            fn from_bits(bits: u64) -> Result<Self, Self::Error> {
                Ok(match bits as u8 {
                    $($value => $name::$variant,)+
                    other => $name::Unknown(other),
                })
            }

            fn to_bits(&self) -> u64 {
                match self {
                    $($name::$variant => $value as u64,)+
                    $name::Unknown(raw) => *raw as u64,
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self.label() {
                    Some(label) => f.write_str(label),
                    None => write!(f, "unknown ({})", self.raw()),
                }
            }
        }
    };
}

pub(crate) use sparse_enum;

sparse_enum!(
    /// A Stage split boundary, one of the ten notes the panel offers.
    ///
    /// The Stage 2 and 3 store the same ten-note table. Inferred from the Nord User
    /// Forum's ns3-program-viewer documentation (github.com/Chris55/ns3-program-viewer);
    /// not confirmed on hardware.
    SplitNote, 4, {
        0 => F2, "F2";
        1 => C3, "C3";
        2 => F3, "F3";
        3 => C4, "C4";
        4 => F4, "F4";
        5 => C5, "C5";
        6 => F5, "F5";
        7 => C6, "C6";
        8 => F6, "F6";
        9 => C7, "C7";
    }
);

sparse_enum!(
    /// A Stage 3 split crossfade width, in semitones.
    ///
    /// Inferred from the ns3-program-viewer documentation; not confirmed on hardware.
    SplitWidth, 2, {
        0 => One, "1";
        1 => Six, "6";
        2 => Twelve, "12";
    }
);

sparse_enum!(
    /// The program category byte the Stage 2 and 3 keep in the header's `aux` word.
    ///
    /// Inferred from the ns3-program-viewer documentation; not confirmed on hardware.
    /// The gaps are real: no name is known for the values between these.
    ProgramCategory, 8, {
        0x00 => Acoustic, "Acoustic";
        0x01 => Bass, "Bass";
        0x02 => Wind, "Wind";
        0x04 => Fantasy, "Fantasy";
        0x05 => Fx, "FX";
        0x06 => Lead, "Lead";
        0x07 => Organ, "Organ";
        0x08 => Pad, "Pad";
        0x0a => Pluck, "Pluck";
        0x0b => String, "String";
        0x0c => Synth, "Synth";
        0x0d => Vocal, "Vocal";
        0x0e => User, "User";
        0x11 => None_, "None";
        0x15 => Grand, "Grand";
        0x16 => Upright, "Upright";
        0x17 => EPiano1, "EPiano1";
        0x18 => EPiano2, "EPiano2";
        0x1b => Clavinet, "Clavinet";
        0x1c => Harpsi, "Harpsi";
        0x1e => Arpeggio, "Arpeggio";
        0xff => Undefined, "Undefined";
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_carries_the_panel_transform() {
        assert_eq!(Level::new(0).unwrap().to_string(), "0 (0.0)");
        assert_eq!(Level::new(127).unwrap().to_string(), "127 (10.0)");
        assert_eq!(Level::new(96).unwrap().to_string(), "96 (7.6)");
        assert!(Level::new(128).is_err(), "128 does not fit seven bits");
    }
}
