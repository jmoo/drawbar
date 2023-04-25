pub mod live;
pub mod settings;
pub use settings::Settings;
pub mod song;
pub use song::Song;
pub mod program;
pub use program::Program;
pub mod bundle;
pub use bundle::Bundle;
use crate::common;

pub type OctaveShift = common::OctaveShift<7, -6, 6>;
pub type Transpose = common::Transpose<6, -6, 6>;
pub type SplitPoint = common::SplitPoint73;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Instrument {
    Organ,
    Piano,
    Sample,
}

impl Instrument {
    fn as_u8(&self) -> u8 {
        *self as u8
    }

    fn as_u16(&self) -> u16 {
        *self as u16
    }

    fn as_str(&self) -> &'static str {
        match self {
            Instrument::Organ => "organ",
            Instrument::Piano => "piano",
            Instrument::Sample => "sample",
        }
    }
}

impl TryFrom<u8> for Instrument {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Instrument, Self::Error> {
        match value {
            0 => Ok(Instrument::Organ),
            1 => Ok(Instrument::Piano),
            2 => Ok(Instrument::Sample),
            _ => Err(&"Value is out of range for instrument"),
        }
    }
}