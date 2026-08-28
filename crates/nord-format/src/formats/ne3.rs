//! Electro 3 and Electro 3 HP (`.nepg`, `.neop`) — container-verified, bodies unmapped.
//!
//! The two products export byte-identical factory content; nothing in a file says
//! which of them wrote it.
//!
//! **`nepg` and `ne4p` share a layout.** Their 110-byte factory-program windows align
//! at offset zero across shared names, although their values differ. Inferred from
//! specimens; not confirmed on hardware.

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
