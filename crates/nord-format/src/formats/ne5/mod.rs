//! Nord Electro 5: program (`ne5p`), live slot (`ne5l`), song / set
//! (`ne5t`), settings (`ne5s`), and the ZIP backup bundle.

pub mod live;
pub mod settings;
pub use settings::Settings;
pub mod song;
pub use song::Song;
pub mod program;
pub use program::{
    B3PercSpeed, B3Vib, Drawbars, EqualizerPart, FarfisaVib, Fx1Type, Fx2Type, Fx3Type, Fx5Type,
    OrganModel, OrganType, PianoCategory, Program, Routing, VoxVib,
};
#[cfg(feature = "bundle")]
pub mod bundle;
use crate::components;
#[cfg(feature = "bundle")]
pub use bundle::Bundle;

pub type OctaveShift = components::OctaveShift<7, -6, 6>;
pub type Transpose = components::Transpose<6, -6, 6>;
pub type SplitPoint = components::SplitPoint73;
pub type PartMix = components::PartMix;
pub use components::{Level, PercSpeed, VibChorus};

/// The three instrument sections a part can select.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub enum Instrument {
    #[default]
    Organ,
    Piano,
    Sample,
}

impl Instrument {
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    pub fn as_u16(&self) -> u16 {
        *self as u16
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Instrument::Organ => "organ",
            Instrument::Piano => "piano",
            Instrument::Sample => "sample",
        }
    }
}

impl crate::bits::Packed for Instrument {
    // Three variants, so two bits — even though the panel's slot is three bits wide.
    const MAX_BITS: u32 = 2;
    type Error = crate::error::ParseError;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Instrument::try_from(bits as u8).map_err(|_| crate::error::ParseError::OutOfBounds {
            value: format!("{bits}"),
            bound: "0..=2 (Instrument)".to_string(),
        })
    }

    fn to_bits(&self) -> u64 {
        self.as_u8() as u64
    }
}

impl TryFrom<u8> for Instrument {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Instrument, Self::Error> {
        match value {
            0 => Ok(Instrument::Organ),
            1 => Ok(Instrument::Piano),
            2 => Ok(Instrument::Sample),
            _ => Err("Value is out of range for instrument"),
        }
    }
}
