//! Range-checked integers for bit-packed fields — an out-of-range stored
//! value is a decode error, never a silent wrap.

use std::fmt::{Debug, Formatter};

use crate::bank::Location;
use crate::bits::{bits_for, Packed};
use crate::error::ParseError;
use crate::fields::{ControlKind, Unit};

/// An i8 value that is bounded by MIN and MAX and can be converted to a u8 by adding OFFSET.
#[derive(Copy, Default, Clone, PartialEq, Eq, Hash)]
pub struct RangedI8<const OFFSET: u8, const MIN: i8, const MAX: i8> {
    inner: i8,
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> RangedI8<OFFSET, MIN, MAX> {
    pub fn as_u8(&self) -> u8 {
        (self.inner + OFFSET as i8) as u8
    }

    pub fn inner(&self) -> i8 {
        self.inner
    }
}

/// Stored biased by `OFFSET`, so the widest encoding is `MAX + OFFSET`.
impl<const OFFSET: u8, const MIN: i8, const MAX: i8> Packed for RangedI8<OFFSET, MIN, MAX> {
    const MAX_BITS: u32 = bits_for((MAX as i16 + OFFSET as i16) as u64);
    /// A signed shift. ⚠️ The unit is the model's — octaves for an octave shift,
    /// semitones for a transpose — and the alias does not carry it, so the kind says
    /// only that the control is signed.
    const CONTROL: ControlKind = ControlKind::Shift(Unit::None);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u8).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.as_u8() as u64
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> Debug for RangedI8<OFFSET, MIN, MAX> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> TryFrom<u8> for RangedI8<OFFSET, MIN, MAX> {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        ((value as i8) - (OFFSET as i8)).try_into()
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> TryFrom<i8> for RangedI8<OFFSET, MIN, MAX> {
    type Error = ParseError;

    fn try_from(value: i8) -> Result<Self, Self::Error> {
        if value < MIN || value > MAX {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: format!("{MIN}..={MAX}"),
            });
        }

        Ok(RangedI8 { inner: value })
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> PartialEq<u8> for RangedI8<OFFSET, MIN, MAX> {
    fn eq(&self, other: &u8) -> bool {
        self.as_u8() == *other
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> PartialEq<i8> for RangedI8<OFFSET, MIN, MAX> {
    fn eq(&self, other: &i8) -> bool {
        self.inner == *other
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> PartialEq<i32> for RangedI8<OFFSET, MIN, MAX> {
    fn eq(&self, other: &i32) -> bool {
        (self.inner as i32) == *other
    }
}

/// An unsigned value constrained to `0..=MAX`.
///
/// The counterpart to [`RangedI8`] for fields that are still plain integers — knob
/// positions, model slots, selectors. Expressing the bound in the type means the value
/// cannot be built too wide for its slot, so encoding it can never fail.
///
/// `MAX` is what the *slot* holds, not what the instrument uses: tightening it to the
/// real range would reject files this decoder currently accepts.
#[derive(Copy, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RangedU8<const MAX: u8> {
    inner: u8,
}

impl<const MAX: u8> RangedU8<MAX> {
    /// The largest value this type can hold.
    pub const MAX: u8 = MAX;

    pub fn new(value: u8) -> Result<Self, ParseError> {
        value.try_into()
    }

    pub fn as_u8(&self) -> u8 {
        self.inner
    }

    pub fn inner(&self) -> u8 {
        self.inner
    }
}

impl<const MAX: u8> Packed for RangedU8<MAX> {
    const MAX_BITS: u32 = bits_for(MAX as u64);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u8).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl<const MAX: u8> TryFrom<u8> for RangedU8<MAX> {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, ParseError> {
        if value > MAX {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: format!("0..={MAX}"),
            });
        }
        Ok(RangedU8 { inner: value })
    }
}

impl<const MAX: u8> From<RangedU8<MAX>> for u8 {
    fn from(value: RangedU8<MAX>) -> u8 {
        value.inner
    }
}

impl<const MAX: u8> Debug for RangedU8<MAX> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const MAX: u8> std::fmt::Display for RangedU8<MAX> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const MAX: u8> PartialEq<u8> for RangedU8<MAX> {
    fn eq(&self, other: &u8) -> bool {
        self.inner == *other
    }
}

/// An unsigned value constrained to `0..=MAX`, for a slot wider than a byte.
///
/// [`RangedU8`] with a wider inner type, and the same rule about `MAX`: it is the
/// slot's bound, not the instrument's.
#[derive(Copy, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RangedU16<const MAX: u16> {
    inner: u16,
}

impl<const MAX: u16> RangedU16<MAX> {
    /// The largest value this type can hold.
    pub const MAX: u16 = MAX;

    pub fn new(value: u16) -> Result<Self, ParseError> {
        value.try_into()
    }

    pub fn as_u16(&self) -> u16 {
        self.inner
    }

    pub fn inner(&self) -> u16 {
        self.inner
    }
}

impl<const MAX: u16> Packed for RangedU16<MAX> {
    const MAX_BITS: u32 = bits_for(MAX as u64);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u16).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl<const MAX: u16> TryFrom<u16> for RangedU16<MAX> {
    type Error = ParseError;

    fn try_from(value: u16) -> Result<Self, ParseError> {
        if value > MAX {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: format!("0..={MAX}"),
            });
        }
        Ok(RangedU16 { inner: value })
    }
}

impl<const MAX: u16> From<RangedU16<MAX>> for u16 {
    fn from(value: RangedU16<MAX>) -> u16 {
        value.inner
    }
}

impl<const MAX: u16> Debug for RangedU16<MAX> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const MAX: u16> std::fmt::Display for RangedU16<MAX> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const MAX: u16> PartialEq<u16> for RangedU16<MAX> {
    fn eq(&self, other: &u16) -> bool {
        self.inner == *other
    }
}

/// A pair of u16 coordinates over an `X_COUNT` × `Y_COUNT` space.
///
/// Both parameters are **counts**, so the valid coordinates are `0..X_COUNT` and
/// `0..Y_COUNT`. The pair packs into a single u16 as `x * Y_COUNT + y`, which is a
/// bijection onto `0..X_COUNT * Y_COUNT` exactly because `Y_COUNT` is the stride.
#[derive(Clone, Default, Copy, PartialEq, Eq, Hash)]
pub struct RangedU16Pair<const X_COUNT: u16, const Y_COUNT: u16> {
    inner: (u16, u16),
}

impl<const X_COUNT: u16, const Y_COUNT: u16> RangedU16Pair<X_COUNT, Y_COUNT> {
    pub fn new(x: u16, y: u16) -> Result<Self, ParseError> {
        if x >= X_COUNT {
            return Err(ParseError::OutOfBounds {
                value: format!("{x}"),
                bound: format!("0..{X_COUNT}"),
            });
        }

        if y >= Y_COUNT {
            return Err(ParseError::OutOfBounds {
                value: format!("{y}"),
                bound: format!("0..{Y_COUNT}"),
            });
        }

        Ok(RangedU16Pair { inner: (x, y) })
    }

    pub fn from_u16(value: u16) -> Result<Self, ParseError> {
        match (value.checked_div(Y_COUNT), value.checked_rem(Y_COUNT)) {
            (Some(x), Some(y)) => (x, y).try_into(),
            // A zero `Y_COUNT` names no locations, so no value decodes into one.
            _ => Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: format!("0..{}", X_COUNT as u32 * Y_COUNT as u32),
            }),
        }
    }

    pub fn inner(&self) -> (u16, u16) {
        self.inner
    }

    pub fn as_u16(&self) -> u16 {
        (self.inner.0 * Y_COUNT) + self.inner.1
    }

    pub fn x(&self) -> u16 {
        self.inner.0
    }

    pub fn y(&self) -> u16 {
        self.inner.1
    }
}

/// The widest encoding is the last location, `X_COUNT * Y_COUNT - 1`.
impl<const X_COUNT: u16, const Y_COUNT: u16> Packed for RangedU16Pair<X_COUNT, Y_COUNT> {
    const MAX_BITS: u32 = bits_for((X_COUNT as u64 * Y_COUNT as u64).saturating_sub(1));
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        Self::from_u16(bits as u16)
    }

    fn to_bits(&self) -> u64 {
        self.as_u16() as u64
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> Debug for RangedU16Pair<X_COUNT, Y_COUNT> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.inner.0, self.inner.1)
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> Location for RangedU16Pair<X_COUNT, Y_COUNT> {
    fn inner(&self) -> (u16, u16) {
        self.inner()
    }

    fn as_u16(&self) -> u16 {
        self.as_u16()
    }

    fn x(&self) -> u16 {
        self.x()
    }

    fn y(&self) -> u16 {
        self.y()
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> TryFrom<(u16, u16)>
    for RangedU16Pair<X_COUNT, Y_COUNT>
{
    type Error = ParseError;

    fn try_from(value: (u16, u16)) -> Result<Self, Self::Error> {
        RangedU16Pair::new(value.0, value.1)
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> TryFrom<u16> for RangedU16Pair<X_COUNT, Y_COUNT> {
    type Error = ParseError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        RangedU16Pair::from_u16(value)
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> PartialEq<u16> for RangedU16Pair<X_COUNT, Y_COUNT> {
    fn eq(&self, other: &u16) -> bool {
        self.as_u16() == *other
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> PartialEq<(u16, u16)>
    for RangedU16Pair<X_COUNT, Y_COUNT>
{
    fn eq(&self, other: &(u16, u16)) -> bool {
        self.inner == *other
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> From<RangedU16Pair<X_COUNT, Y_COUNT>> for u16 {
    fn from(value: RangedU16Pair<X_COUNT, Y_COUNT>) -> u16 {
        value.as_u16()
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> From<RangedU16Pair<X_COUNT, Y_COUNT>> for u32 {
    fn from(value: RangedU16Pair<X_COUNT, Y_COUNT>) -> u32 {
        value.as_u16() as u32
    }
}

impl<const X_COUNT: u16, const Y_COUNT: u16> From<RangedU16Pair<X_COUNT, Y_COUNT>> for u64 {
    fn from(value: RangedU16Pair<X_COUNT, Y_COUNT>) -> u64 {
        value.as_u16() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::RangedU16Pair;

    #[test]
    fn ranged_tuple_can_convert_to_u16() {
        let ranged_tuple: RangedU16Pair<5, 10> = (1, 2).try_into().unwrap();
        assert_eq!(ranged_tuple.as_u16(), 12);
    }

    #[test]
    fn ranged_tuple_can_be_created_from_u16() {
        let ranged_tuple: RangedU16Pair<5, 10> = 12_u16.try_into().unwrap();
        assert_eq!(ranged_tuple, (1, 2));
    }

    /// Both parameters are counts: the last location of an 8×50 space is `(7, 49)`, and
    /// a coordinate equal to its count is one past the end.
    #[test]
    fn the_pair_parameters_are_counts_not_maxima() {
        type Location = RangedU16Pair<8, 50>;

        assert!(Location::try_from((7, 49)).is_ok());
        assert!(Location::try_from((8, 0)).is_err());
        assert!(Location::try_from((0, 50)).is_err());
    }

    /// Packing at the `Y_COUNT` stride is a bijection, so no two locations share a u16.
    #[test]
    fn packing_round_trips_without_collision() {
        type Location = RangedU16Pair<8, 50>;

        assert_eq!(Location::try_from((1, 0)).unwrap().as_u16(), 50);
        assert_eq!(Location::from_u16(50).unwrap(), (1, 0));
        assert_eq!(Location::try_from((7, 49)).unwrap().as_u16(), 399);
        assert!(Location::from_u16(400).is_err());

        let mut seen = std::collections::HashSet::new();
        for x in 0..8 {
            for y in 0..50 {
                let packed = Location::try_from((x, y)).unwrap().as_u16();
                assert!(seen.insert(packed), "({x}, {y}) collides at {packed}");
                assert_eq!(Location::from_u16(packed).unwrap(), (x, y));
            }
        }
    }
}
