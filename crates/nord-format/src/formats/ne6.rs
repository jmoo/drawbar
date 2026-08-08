//! Electro 6 (`.ne6p`, `.ne6l`, `.ne6t`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.ne6p`).
    program,
    "ne6p",
    211
);
raw_format!(
    /// Live slots (`.ne6l`) — same length as a program.
    live,
    "ne6l",
    211
);
raw_format!(
    /// Settings (`.ne6t`).
    settings,
    "ne6t",
    260
);
