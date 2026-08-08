//! The `no3` organ (`.no3p`, `.no3t`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.no3p`).
    program,
    "no3p",
    204
);
raw_format!(
    /// Settings (`.no3t`).
    settings,
    "no3t",
    452
);
