//! Nord Piano (`.nppg`, `.npli`, `.npsy`) — container-verified, bodies unmapped.
//!
//! The first Piano's tags predate the model-prefix convention: program is `nppg`,
//! live is `npli`, and the settings tag is `npsy` (the vendor calls it System).

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nppg`).
    program,
    "nppg",
    31
);
raw_format!(
    /// Live slots (`.npli`) — same length as a program.
    live,
    "npli",
    31
);
raw_format!(
    /// Settings (`.npsy`).
    settings,
    "npsy",
    10
);
