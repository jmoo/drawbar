//! Nord Wave 2 (`.nw2p`, `.nw2l`, `.nw2s`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nw2p`).
    program,
    "nw2p",
    1044
);
raw_format!(
    /// Live slots (`.nw2l`) — same length as a program.
    live,
    "nw2l",
    1044
);
raw_format!(
    /// Settings (`.nw2s`).
    settings,
    "nw2s",
    109
);
