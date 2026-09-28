//! Nord Electro 5: program (`ne5p`), live slot (`ne5l`), song / set
//! (`ne5t`), settings (`ne5s`), and the ZIP backup bundle.

mod center_panel;
mod effects_panel;
pub mod live;
mod organ_panel;
mod piano_panel;
mod program_panel;
mod sample_panel;
pub mod settings;
pub use settings::Settings;
pub mod song;
pub use song::Song;
pub mod program;
pub use program::{
    B3PercSpeed, B3Vib, Drawbars, EqualizerPart, FarfisaVib, Fx1Type, Fx2Type, Fx3Type, Fx5Type,
    OrganModel, OrganType, PianoCategory, Preset, Program, Routing, VoxVib,
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

impl crate::bits::Packed for Instrument {
    const MAX_BITS: u32 = 2;
    const DECODE_BITS: u32 = u8::BITS;
    const CONTROL: crate::fields::ControlKind = crate::fields::ControlKind::Selector;
    type Error = crate::error::ParseError;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        match bits {
            0 => Ok(Instrument::Organ),
            1 => Ok(Instrument::Piano),
            2 => Ok(Instrument::Sample),
            _ => Err(crate::error::ParseError::OutOfBounds {
                value: format!("{bits}"),
                bound: "0..=2 (Instrument)".to_string(),
            }),
        }
    }

    fn to_bits(&self) -> u64 {
        *self as u64
    }
}
