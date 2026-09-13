//! The Stage 4 organ preset body (`.ns4o`): 139 bytes.
//!
//! One organ section as a program stores it, moved down 45 bytes, keyboard zone
//! included on both layers though a preset has no use for it — layer A's is
//! confirmed against the corpus, where it varies as a zone does. The two layers
//! still share one effects chain, so its fields carry no layer.

use super::fx::FxChain;
use super::organ_layers::OrganLayer;
use crate::cbin::{self, Cbin};
use crate::components::{Level, MorphTarget};
use crate::error::Error;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns4o";
/// Stored ×100. The corpus holds 2.05; ns4decode was tested on 2.01.
pub const KNOWN_VERSIONS: &[u32] = &[201, 202, 203, 204, 205];
pub const BODY_LEN: usize = 139;

/// The 139-byte preset body: one organ section as a program stores it.
///
/// Reads and writes byte-exactly. A read verifies the container checksum, gates
/// on [`KNOWN_VERSIONS`], and range-checks every field; unclaimed bits survive a
/// re-encode verbatim. Placements derived from ns4decode's published tables;
/// values raw. Inferred from specimens; not confirmed on hardware.
#[nord_bits_derive::bitbody(139)]
pub struct OrganPreset {
    #[bits(41..=41)]
    pub organ_b_layer_enabled: bool,
    #[bits(42..=42)]
    pub organ_a_layer_enabled: bool,
    #[bits(44..=44)]
    pub organ_b_layer_enabled_scene_2: bool,
    #[bits(45..=45)]
    pub organ_a_layer_enabled_scene_2: bool,
    #[bits(46..=52)]
    pub organ_a_volume: Level,
    #[bits(53..=60)]
    pub organ_a_volume_wheel: MorphTarget,
    #[bits(61..=68)]
    pub organ_a_volume_aftertouch: MorphTarget,
    #[bits(69..=76)]
    pub organ_a_volume_ctrl_pedal: MorphTarget,
    #[bits(77..=83)]
    pub organ_b_volume: Level,
    #[bits(84..=91)]
    pub organ_b_volume_wheel: MorphTarget,
    #[bits(92..=99)]
    pub organ_b_volume_aftertouch: MorphTarget,
    #[bits(100..=107)]
    pub organ_b_volume_ctrl_pedal: MorphTarget,

    #[at(23..52)]
    pub organ_a: OrganLayer,
    #[at(54..83)]
    pub organ_b: OrganLayer,
    #[at(85..137)]
    pub organ_fx: FxChain,
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<OrganPreset>, Error> {
    let file: Cbin<OrganPreset> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
