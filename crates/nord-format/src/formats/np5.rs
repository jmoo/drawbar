//! Nord Piano 5 (`.np5p`, `.np5l`, `.np5t`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.np5p`).
    program,
    "np5p",
    237
);
raw_format!(
    /// Live slots (`.np5l`) — same length as a program.
    live,
    "np5l",
    237
);
raw_format!(
    /// Settings (`.np5t`).
    settings,
    "np5t",
    191
);
