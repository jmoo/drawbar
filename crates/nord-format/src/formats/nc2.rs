//! Nord C2 (`.ncpg`, `.ncsy`) — container-verified, bodies unmapped.
//!
//! The C2's pipe-organ library (`.npip`) lives in [`super::npip`]: it is a
//! multi-megabyte library format, not a slot-addressed one.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.ncpg`).
    program,
    "ncpg",
    271
);
raw_format!(
    /// Settings (`.ncsy`).
    settings,
    "ncsy",
    33
);
