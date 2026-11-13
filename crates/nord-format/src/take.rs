//! Cut a recorded take into the strokes of a Nord piano library.
//!
//! A take is one WAV holding every note played. [`crate::wav`] reads it in any PCM or
//! float encoding and saves its regions and loops without touching the audio or any
//! chunk it does not own; [`resample`] brings it to the rate the instrument plays.
//!
//! [`plan`] runs the detectors in turn: [`detect`] finds each hit, its pitch and its
//! key-up, [`hum`] the take's mains hum so levels are measured against the floor
//! without it, [`layers`] assigns hits to notes and chooses each note's layers, and
//! [`cut`] places each hit's attack and release, and [`sustain`] its loop or fade,
//! found on the attack as [`clean`] leaves it. [`plan::Plan::regions`] and
//! [`plan::Plan::loops`] mark them as the build reads them, and the build cleans and
//! shapes each stroke from them.
//!
//! These are tuned heuristics over audio, not declarations of a file format, which is
//! why they sit outside [`crate::formats`].

pub mod clean;
pub mod cut;
pub mod detect;
mod fft;
pub mod filter;
pub mod hum;
pub mod layers;
pub mod plan;
pub mod resample;
pub mod sustain;
