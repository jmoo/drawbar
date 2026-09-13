//! The Stage 4 piano preset body (`.ns4n`): 151 bytes.
//!
//! One piano section as a program stores it, moved down 180 bytes, keyboard zone
//! included on both layers though a preset has no use for it — layer A's is
//! confirmed against the corpus, where it varies as a zone does.

use super::fx::FxChain;
use super::piano_layers::PianoLayer;
use crate::cbin::{self, Cbin};
use crate::components::{Level, MorphTarget};
use crate::error::Error;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns4n";
/// Stored ×100. The corpus holds 2.03; ns4decode was tested on 2.01.
pub const KNOWN_VERSIONS: &[u32] = &[201, 202, 203];
pub const BODY_LEN: usize = 151;

/// The 151-byte preset body: one piano section as a program stores it.
///
/// Reads and writes byte-exactly. A read verifies the container checksum, gates
/// on [`KNOWN_VERSIONS`], and range-checks every field; unclaimed bits survive a
/// re-encode verbatim. Placements derived from ns4decode's published tables;
/// values raw. Inferred from specimens; not confirmed on hardware.
#[nord_bits_derive::bitbody(151)]
pub struct PianoPreset {
    #[bits(41..=41)]
    pub piano_b_layer_enabled: bool,
    #[bits(42..=42)]
    pub piano_a_layer_enabled: bool,
    #[bits(44..=44)]
    pub piano_b_layer_enabled_scene_2: bool,
    #[bits(45..=45)]
    pub piano_a_layer_enabled_scene_2: bool,
    #[bits(46..=52)]
    pub piano_a_volume: Level,
    #[bits(53..=60)]
    pub piano_a_volume_wheel: MorphTarget,
    #[bits(61..=68)]
    pub piano_a_volume_aftertouch: MorphTarget,
    #[bits(69..=76)]
    pub piano_a_volume_ctrl_pedal: MorphTarget,
    #[bits(77..=83)]
    pub piano_b_volume: Level,
    #[bits(84..=91)]
    pub piano_b_volume_wheel: MorphTarget,
    #[bits(92..=99)]
    pub piano_b_volume_aftertouch: MorphTarget,
    #[bits(100..=107)]
    pub piano_b_volume_ctrl_pedal: MorphTarget,

    #[at(18..27)]
    pub piano_a: PianoLayer,
    #[at(30..39)]
    pub piano_b: PianoLayer,
    #[at(42..94)]
    pub piano_a_fx: FxChain,
    #[at(97..149)]
    pub piano_b_fx: FxChain,
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<PianoPreset>, Error> {
    let file: Cbin<PianoPreset> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
