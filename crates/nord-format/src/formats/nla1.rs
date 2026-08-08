//! Nord Lead A1 (`.nlas`, `.nlap`, `.nlat`) — container-verified, bodies unmapped.
//!
//! Same tag inversion as the Lead 4: `.nlas` is a program, `.nlap` a performance.
//! Versions 0 through 6 occur in the factory banks; the schema differences between
//! them are unmapped, which is one more reason the body stays raw.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nlas`).
    program,
    "nlas",
    97
);
raw_format!(
    /// Performances (`.nlap`).
    performance,
    "nlap",
    401
);
raw_format!(
    /// Settings (`.nlat`).
    settings,
    "nlat",
    58
);
