//! Nord Stage 3 (`.ns3f`, `.ns3l`, `.ns3s`, `.ns3y`, `.ns3t`).
//!
//! The program body decodes in full, including both panels. A program is 22 bytes of
//! globals followed by two 263-byte panel blocks with the same layout: Panel A and
//! Panel B, the instrument's two independent setups, each with its own organ, piano,
//! synth, extern and effects. `panel_enable` selects A, B, or both layered, and their
//! fields are reached as `panel_a.*` and `panel_b.*`. The synth preset (`ns3y`) is
//! Panel A's synth block under its own tag. The song and settings are stubs.
//!
//! ⚠️ The extension letters differ from other models: `f` is the program, `s` is a
//! song (what the Electro 5 calls a set list), `y` is a synth patch (the settings on
//! the Stage 2), and `t` is the settings.
//!
//! The placements come from the community byte maps in
//! [Chris55/nord-documentation](https://github.com/Chris55/nord-documentation), the
//! public documentation this module's provenance comments refer to.
//!
//! Community documentation reports a second checksum at file offset `0x78`
//! ("covering synth and organ panel data"). Specimens contradict it. The word there
//! is not a common CRC-32 over any contiguous or field-excised range. It does not
//! change between near-identical program pairs whose bodies differ, while the `0x18`
//! checksum always does. It takes clustered values that many unrelated programs
//! share, and it sits beside bytes that are constant across every specimen: the
//! signature of bit-packed panel parameters, which body offset `0x4c` holds. Programs
//! re-saved after panel edits still decode, so nothing verifies the word. Treat the
//! claim as mistaken until a specimen shows otherwise.

use super::raw::raw_format;

pub mod panel;
pub use panel::Panel;
pub mod program;
pub use program::Program;

pub mod synth;
pub use synth::SynthPreset;

pub mod live {
    //! The live buffer (`.ns3l`): the current panel state, as a program body under its
    //! own tag.

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
    /// Songs (`.ns3s`): the Stage 3's set-list entries.
    song,
    "ns3s",
    45
);
raw_format!(
    /// Settings (`.ns3t`).
    settings,
    "ns3t",
    203
);
