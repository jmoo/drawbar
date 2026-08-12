//! Electro 3 and Electro 3 HP (`.nepg`, `.neop`) — container-verified, bodies unmapped.
//!
//! The two products export byte-identical factory content; nothing in a file says
//! which of them wrote it.
//!
//! **`nepg` and `ne4p` are the same layout.** Same 110-byte body, and indexing one
//! bank's byte windows against the other's puts essentially all of them at offset
//! delta zero — 10,709 matches there against 4 at the next-best delta — with 122
//! program names shared between the two factory banks. The bytes are not identical,
//! so the Electro 4 refines values rather than reissuing the bank. Whichever gets
//! decoded first decodes both, over a combined 512 specimens.

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
