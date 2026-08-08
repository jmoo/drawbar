//! Nord Piano 4 (`.np4p`, `.np4l`, `.np4t`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.np4p`).
    program,
    "np4p",
    90
);
raw_format!(
    /// Live slots (`.np4l`) — same length as a program.
    live,
    "np4l",
    90
);
raw_format!(
    /// Settings (`.np4t`).
    settings,
    "np4t",
    187
);
