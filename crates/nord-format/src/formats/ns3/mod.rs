//! Nord Stage 3 (`.ns3f`, `.ns3l`, `.ns3s`, `.ns3y`, `.ns3t`).
//!
//! The program body has a partial decode — the program-wide globals — and the
//! remaining formats are container-verified stubs. ⚠️ The extension letters are
//! traps here: `f` is the program, `s` is a *song* (a set list on the Electro 5),
//! `y` is a synth patch (the *settings* on the Stage 2), and `t` is the settings.
//!
//! Community documentation reports a second checksum at file offset `0x78`
//! ("covering synth and organ panel data"). The corpus refutes it: the word
//! there is not any common CRC-32 over any contiguous or field-excised range,
//! never changes between near-identical program pairs whose bodies differ (the
//! `0x18` checksum always does), takes clustered values that many unrelated
//! programs share, and sits beside bytes constant across every specimen — the
//! signature of bit-packed panel parameters, which is what body offset `0x4c`
//! holds. Programs re-saved after panel edits keep decoding, so nothing
//! verifies it; treat the claim as mistaken until a specimen shows otherwise.

use super::raw::raw_format;

pub mod program;
pub use program::Program;

pub mod live {
    //! The live buffer (`.ns3l`): the panel as it stands, not a saved program.
    //! Same body as a program, under its own tag.

    use super::program::{self, Program};
    use crate::cbin::{self, Cbin};
    use crate::error::Error;
    use std::io::{Read, Seek};

    pub const FORMAT: &str = "ns3l";

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
        let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
        crate::formats::known_version(FORMAT, file.header.version, program::KNOWN_VERSIONS)?;
        Ok(file)
    }
}

raw_format!(
    /// Songs (`.ns3s`) — the Stage 3's set-list entries.
    song,
    "ns3s",
    45
);
raw_format!(
    /// Synth patches (`.ns3y`).
    synth,
    "ns3y",
    58
);
raw_format!(
    /// Settings (`.ns3t`).
    settings,
    "ns3t",
    203
);
