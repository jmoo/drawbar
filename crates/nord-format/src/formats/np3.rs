//! Nord Piano 3 (`.np3p`, `.np3l`, `.np3s`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.np3p`).
    program,
    "np3p",
    55
);
raw_format!(
    /// Live slots (`.np3l`) — same length as a program.
    live,
    "np3l",
    55
);
raw_format!(
    /// Settings (`.np3s`).
    settings,
    "np3s",
    34
);
