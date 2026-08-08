//! Nord Wave (`.nwp`, `.nwsy`) — container-verified, bodies unmapped.
//!
//! ⚠️ The program tag is three characters plus a NUL: `nwp\0`.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nwp`).
    program,
    "nwp\0",
    324
);
raw_format!(
    /// Settings (`.nwsy`).
    settings,
    "nwsy",
    35
);
