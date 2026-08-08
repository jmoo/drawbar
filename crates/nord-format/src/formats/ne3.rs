//! Electro 3 and Electro 3 HP (`.nepg`, `.neop`) — container-verified, bodies unmapped.
//!
//! The two products export byte-identical factory content; nothing in a file says
//! which of them wrote it.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nepg`).
    program,
    "nepg",
    110
);
raw_format!(
    /// Organ presets (`.neop`) — the B3/Farfisa/Vox preset banks.
    organ_preset,
    "neop",
    11
);
