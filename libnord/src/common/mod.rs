pub mod bank;
pub mod piano;
pub mod sample;
pub mod settings;
pub mod song;

pub mod header;
pub use header::Header;
pub use header::Preamble;

use crate::error;
pub use error::Error;

pub mod program;
pub use program::OctaveShift;
pub use program::Transpose;
pub use program::SplitPoint73;
pub use program::PartMix;
use crate::error::ParseError;

// pub struct RotaryEncoder {
//     pub inner: u8,
// }
//
// impl RotaryEncoder {
//     pub fn inner(&self) -> u8 {
//         self.inner
//     }
//
//     pub fn from_u8(value: u8) -> Result<Self, Self::Error>{
//         if value > 127 {
//             Err(ParseError::OutOfBounds(format!("{}", value), ">=0 <=127".to_string()))
//         } else {
//             Ok(Self { inner: value })
//         }
//     }
// }
//
// impl TryFrom<u8> for RotaryEncoder {
//     type Error = ParseError;
//
//     fn try_from(value: u8) -> Result<Self, Self::Error> {
//        Self::from_u8(value)
//     }
// }