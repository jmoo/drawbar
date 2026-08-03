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

/// Percussion decay speed. How it is stored is per-model; the Electro 5's B3 does not
/// store it in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PercSpeed {
    Off,
    Soft,
    Fast,
    Both,
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
