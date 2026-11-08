//! Cut a recorded take into the strokes of a Nord piano library.
//!
//! A take is one WAV holding every note played. [`wav`] reads it in any PCM or float
//! encoding and saves its regions and loops without touching the audio or any chunk it
//! does not own; [`resample`] brings it to the rate the instrument plays.
//!
//! [`plan`] runs the detectors in turn: [`detect`] finds each hit, its pitch and its
//! key-up, [`hum`] the take's mains hum so levels are measured against the floor
//! without it, [`layers`] assigns hits to notes and chooses each note's layers, and
//! [`cut`] places each hit's attack and release, and [`sustain`] its loop or fade.
//! [`plan::Plan::regions`] and [`plan::Plan::loops`] mark them as the build reads them.

pub mod cut;
pub mod detect;
mod fft;
pub mod filter;
pub mod hum;
pub mod layers;
pub mod plan;
pub mod resample;
pub mod sustain;
pub mod wav;

use thiserror::Error as ThisError;

#[derive(ThisError, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Bytes that do not form what they claim to.
    #[error("{0}")]
    Malformed(String),

    /// A well-formed file in an encoding this crate does not read.
    #[error("{0}")]
    Unsupported(String),

    #[error("value {value} is outside {bound}")]
    OutOfBounds { value: String, bound: String },
}

fn malformed<T>(why: impl Into<String>) -> Result<T, Error> {
    Err(Error::Malformed(why.into()))
}

fn out_of_bounds<T>(value: impl Into<String>, bound: impl Into<String>) -> Result<T, Error> {
    Err(Error::OutOfBounds {
        value: value.into(),
        bound: bound.into(),
    })
}
