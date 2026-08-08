//! Nord Piano 2 (`.np2p`, `.np2l`, `.np2s`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.np2p`).
    program,
    "np2p",
    44
);
raw_format!(
    /// Live slots (`.np2l`) — same length as a program.
    live,
    "np2l",
    44
);
raw_format!(
    /// Settings (`.np2s`).
    settings,
    "np2s",
    16
);
