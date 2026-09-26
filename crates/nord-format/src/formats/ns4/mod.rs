//! Nord Stage 4 (`.ns4p`, `.ns4l`, `.ns4y`, `.ns4n`, `.ns4o`, `.ns4t`).
//!
//! The Stage 4 banks its three sections separately from its programs: a synth
//! (`.ns4y`), piano (`.ns4n`) or organ (`.ns4o`) preset holds one section's
//! parameters under its own tag, and a program holds all three plus the globals
//! that route them. ⚠️ The extension letters differ from the Stage 3's: `p` is the
//! program here, and the Stage 3 uses `f`.
//!
//! # What decodes
//!
//! Every parameter placement in the program body and the three preset bodies.
//! Bits no parameter claims survive a re-encode unchanged.
//!
//! A field's type says what kind of control it is, and no more. A knob is a `Level` or
//! a `Time`, so a caller knows to draw a dial and which unit to label it with. A fixed
//! list is a [`Selector`](crate::components::Selector), so a caller knows to draw
//! positions. None of these types names a position: no specimen says which index is
//! `LP24`, so a filter type stays a number. Give a field a `sparse_enum!` once its
//! table is known, and not before.
//!
//! Three exceptions rest on specimen evidence, and each says so at its definition: the
//! octave shift reads as two's complement
//! ([`OctaveShiftNibble`](crate::components::OctaveShiftNibble)), the keyboard zone holds
//! the Stage 3's table ([`KbZone4`](crate::components::KbZone4)), and the arpeggiator's
//! three `u32` rows read as sixteen two-bit steps
//! ([`ArpPattern`](crate::components::ArpPattern)).
//!
//! The settings (`.ns4t`) are a container-verified stub.
//!
//! # Naming
//!
//! A field is `<section>_<layer>_<parameter>`: the organ and piano have layers
//! `a` and `b`, the synth `a`, `b` and `c`, and program-wide parameters have no
//! prefix. The organ's two layers share one effects chain, so those fields are
//! program-wide too: `organ_fx_*`, with no layer in the name.
//!
//! A parameter that can be driven from a performance control has three siblings
//! holding the value that control morphs to: `_wheel`, `_aftertouch` and
//! `_ctrl_pedal`. `_scene_2` is the second layer scene's copy.
//!
//! # Provenance
//!
//! Every placement is derived from the offset tables published by
//! [ns4decode](https://ns4decode.netlify.app) (MIT, © 2024 Randy), a Stage 4
//! file viewer. Reported by public documentation; not confirmed on hardware. Its
//! notation is 1-based file bytes with bits numbered 1..=8 from the MSB, which matches
//! this crate's bit numbering after the 44-byte container header.
//!
//! The mapping is checked against specimens: the tables place five parameters inside
//! the header (the tag, version, bank, slot and checksum) and one, `version_echo`, in
//! the first bytes of the body, and all six agree with the container's own parse on
//! every Stage 4 specimen. The value tables ns4decode also publishes are not used here.

use super::raw::raw_format;

pub mod fx;
pub use fx::FxChain;
pub mod organ_layers;
pub use organ_layers::OrganLayer;
pub mod piano_layers;
pub use piano_layers::PianoLayer;
pub mod synth_performance;
pub use synth_performance::SynthPerformance;
pub mod synth_voice;
pub use synth_voice::SynthVoice;
pub mod program;
pub use program::Program;

pub mod live {
    //! The live buffer (`.ns4l`): the current panel state, as a program body under its
    //! own tag.

    use super::program::{self, Program};
    use crate::cbin::{self, Cbin};
    use crate::error::Error;
    use std::io::{Read, Seek};

    pub const FORMAT: &str = "ns4l";

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
        let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
        crate::formats::known_version(FORMAT, file.header.version, program::KNOWN_VERSIONS)?;
        Ok(file)
    }
}

pub mod organ_preset;
pub mod piano_preset;
pub mod synth;

raw_format!(
    /// Settings (`.ns4t`).
    settings,
    "ns4t",
    80
);
