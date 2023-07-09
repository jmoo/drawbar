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
pub use program::PartMix;
pub use program::SplitPoint73;
pub use program::Transpose;

// pub struct Drawbar(u8);
//
// impl Specifier for Drawbar {
//     const BITS: usize = 4;
//     const STRUCT: bool = true;
//     type Bytes = u8;
//     type InOut = Drawbar;
//
//     fn into_bytes(input: Self::InOut) -> Result<Self::Bytes, OutOfBounds> {
//         Ok(input.0)
//     }
//
//     fn from_bytes(bytes: Self::Bytes) -> Result<Self::InOut, InvalidBitPattern<Self::Bytes>> {
//         Ok(Drawbar(bytes >> 4))
//     }
// }
//
// #[bitfield]
// pub struct Drawbars {
//     pub drawbar_1: Drawbar,
//     pub drawbar_2: Drawbar,
//     pub drawbar_3: Drawbar,
//     pub drawbar_4: Drawbar,
//     pub drawbar_5: Drawbar,
//     pub drawbar_6: Drawbar,
//     pub drawbar_7: Drawbar,
//     pub drawbar_8: Drawbar,
//     pub drawbar_9: Drawbar,
// }

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
