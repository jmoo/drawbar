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

impl TryFrom<u8> for PartMix {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        (value as u16).try_into()
    }
}

impl TryFrom<u16> for PartMix {
    type Error = ParseError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value > 127 {
            return Err(ParseError::OutOfBounds(
                format!("{:?}", value),
                format!(" <{:?} >{:?}", 0, 127),
            ));
        }

        Ok(PartMix { inner: value as u8 })
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

impl SplitPoint73 {
    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            SplitPoint73::C3 => "c3",
            SplitPoint73::F3 => "f3",
            SplitPoint73::C4 => "c4",
            SplitPoint73::F4 => "f4",
            SplitPoint73::C5 => "c5",
            SplitPoint73::F5 => "f5",
            SplitPoint73::Upper => "upper",
            SplitPoint73::Lower => "lower",
        }
    }
}

#[derive(Default)]
pub enum Instrument {
    #[default]
    Organ,
    Piano,
    Sample,
    Synth,
}

pub trait Program {}
