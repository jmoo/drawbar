use std::fmt::{Debug, Formatter};

use crate::common::bank::Location;
use crate::error::ParseError;

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

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> TryFrom<u16> for RangedI8<OFFSET, MIN, MAX> {
    type Error = ParseError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        (value as u8).try_into()
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> TryFrom<i8> for RangedI8<OFFSET, MIN, MAX> {
    type Error = ParseError;

    fn try_from(value: i8) -> Result<Self, Self::Error> {
        if value < MIN || value > MAX {
            return Err(ParseError::OutOfBounds(
                format!("{:?}", value),
                format!(" <{:?} >{:?}", MIN, MAX),
            ));
        }

        Ok(RangedI8 { inner: value })
    }
}

impl<const OFFSET: u8, const MIN: i8, const MAX: i8> TryFrom<i32> for RangedI8<OFFSET, MIN, MAX> {
    type Error = ParseError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        if value < (MIN as i32) || value > (MAX as i32) {
            return Err(ParseError::OutOfBounds(
                format!("{:?}", value),
                format!(" <{:?} >{:?}", MIN, MAX),
            ));
        }

        Ok(RangedI8 {
            inner: (value as i8) - (OFFSET as i8),
        })
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

/// A pair of u16 values that are guaranteed to be within a given range.
#[derive(Clone, Default, Copy, PartialEq, Eq, Hash)]
pub struct RangedU16Pair<const X_MAX: u16, const Y_MAX: u16> {
    inner: (u16, u16),
}

impl<const X_MAX: u16, const Y_MAX: u16> RangedU16Pair<X_MAX, Y_MAX> {
    pub fn new(x: u16, y: u16) -> Result<Self, ParseError> {
        if x > X_MAX {
            return Err(ParseError::OutOfBounds(
                format!("{:?}", x),
                format!("{:?}", X_MAX),
            ));
        }

        if y > Y_MAX {
            return Err(ParseError::OutOfBounds(
                format!("{:?}", y),
                format!("{:?}", Y_MAX),
            ));
        }

        Ok(RangedU16Pair { inner: (x, y) })
    }

    pub fn from_u16(value: u16) -> Result<Self, ParseError> {
        if Y_MAX == 0 {
            if value > 0 {
                Err(ParseError::OutOfBounds(
                    format!("{:?}", value),
                    format!("{:?}", X_MAX),
                ))
            } else {
                (0, 0).try_into()
            }
        } else {
            (value / Y_MAX, value % Y_MAX).try_into()
        }
    }

    pub fn inner(&self) -> (u16, u16) {
        self.inner
    }

    pub fn as_u16(&self) -> u16 {
        (self.inner.0 * Y_MAX) + self.inner.1
    }

    pub fn x(&self) -> u16 {
        self.inner.0
    }

    pub fn y(&self) -> u16 {
        self.inner.1
    }
}

impl<const X: u16, const Y: u16> Debug for RangedU16Pair<X, Y> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.inner.0, self.inner.1)
    }
}

impl<const X: u16, const Y: u16> Location for RangedU16Pair<X, Y> {
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

impl<const X: u16, const Y: u16> TryFrom<(u16, u16)> for RangedU16Pair<X, Y> {
    type Error = ParseError;

    fn try_from(value: (u16, u16)) -> Result<Self, Self::Error> {
        RangedU16Pair::new(value.0, value.1)
    }
}

impl<const X: u16, const Y: u16> TryFrom<[u8; 4]> for RangedU16Pair<X, Y> {
    type Error = ParseError;

    fn try_from(value: [u8; 4]) -> Result<Self, Self::Error> {
        RangedU16Pair::new(
            u16::from_be_bytes([value[0], value[1]]),
            u16::from_be_bytes([value[1], value[2]]),
        )
    }
}

impl<const X: u16, const Y: u16> TryFrom<u16> for RangedU16Pair<X, Y> {
    type Error = ParseError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        RangedU16Pair::from_u16(value)
    }
}

impl<const B: u16, const S: u16> PartialEq<u16> for RangedU16Pair<B, S> {
    fn eq(&self, other: &u16) -> bool {
        self.as_u16() == *other
    }
}

impl<const B: u16, const S: u16> PartialEq<(u16, u16)> for RangedU16Pair<B, S> {
    fn eq(&self, other: &(u16, u16)) -> bool {
        self.inner == *other
    }
}

impl<const X: u16, const Y: u16> From<RangedU16Pair<X, Y>> for u16 {
    fn from(value: RangedU16Pair<X, Y>) -> u16 {
        value.as_u16()
    }
}

impl<const X: u16, const Y: u16> From<RangedU16Pair<X, Y>> for u32 {
    fn from(value: RangedU16Pair<X, Y>) -> u32 {
        value.as_u16() as u32
    }
}

impl<const X: u16, const Y: u16> From<RangedU16Pair<X, Y>> for u64 {
    fn from(value: RangedU16Pair<X, Y>) -> u64 {
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

    #[test]
    fn can_create_identity_point_in_empty_tuple_range() {
        let ranged_tuple: RangedU16Pair<0, 0> = 0_u16.try_into().unwrap();
        assert_eq!(ranged_tuple, (0, 0));
    }
}
