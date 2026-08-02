use crate::bits::Packed;
use crate::error::ParseError;
use crate::types::RangedI8;
use std::fmt::{Debug, Formatter};

pub type OctaveShift<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;
pub type Transpose<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;

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
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
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
            return Err(ParseError::OutOfBounds(
                format!("{value}"),
                format!("{}", 127),
            ));
        }

        Ok(PartMix { inner: value })
    }
}

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
        SplitPoint73::try_from(bits as u8)
            .map_err(|e| ParseError::OutOfBounds(format!("{bits}"), e.to_string()))
    }

    fn to_bits(&self) -> u64 {
        *self as u64
    }
}
