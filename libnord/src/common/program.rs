use crate::types::RangedI8;

pub type OctaveShift<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;

pub type Transpose<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SplitPoint73 {
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
            _ => Err(&"Value is out of range for split point"),
        }
    }
}

impl SplitPoint73 {
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

pub enum Instrument {
    Organ,
    Piano,
    Sample,
    Synth,
}

pub trait Program {}
