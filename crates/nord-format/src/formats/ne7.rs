//! Electro 7 (`.ne7p`, `.ne7l`, `.ne7t`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.ne7p`).
    program,
    "ne7p",
    336
);
raw_format!(
    /// Live slots (`.ne7l`) — same length as a program.
    live,
    "ne7l",
    336
);
raw_format!(
    /// Settings (`.ne7t`).
    settings,
    "ne7t",
    212
);
