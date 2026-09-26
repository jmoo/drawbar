//! Nord Stage 2 and 2 EX (`.ns2p`, `.ns2l`, `.ns2s`, `.ns2y`).
//!
//! The program body decodes in full, including both slots. A program is 23 bytes of
//! globals followed by two 249-byte slot blocks with the same layout: Slot A and Slot
//! B, the two complete setups the panel's Slot buttons switch between, each with its
//! own organ, piano, synth, extern and effects. Their fields are reached as `slot_a.*`
//! and `slot_b.*`. The other formats are container-verified stubs; the synth file
//! (`ns2s`) is Slot A's synth block, located but not declared.
//!
//! ⚠️ `s` is a synth patch here (a song on the Stage 3), and `y` is the settings (a
//! synth patch on the Stage 3 and 4).

use super::raw::raw_format;

pub mod slot;
pub use slot::Slot;
pub mod program;
pub use program::Program;

pub mod live {
    //! The live buffer (`.ns2l`): a program body under its own tag.

    use super::program::{self, Program};
    use crate::cbin::{self, Cbin};
    use crate::error::Error;
    use std::io::{Read, Seek};

    pub const FORMAT: &str = "ns2l";

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
        let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
        crate::formats::known_version(FORMAT, file.header.version, program::KNOWN_VERSIONS)?;
        Ok(file)
    }
}

raw_format!(
    /// Synth patches (`.ns2s`).
    synth,
    "ns2s",
    34
);
raw_format!(
    /// Settings (`.ns2y`).
    settings,
    "ns2y",
    32
);
