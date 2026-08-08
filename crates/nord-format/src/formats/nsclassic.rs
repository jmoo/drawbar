//! Nord Stage Classic and Stage EX (`.nspg`, `.nss`, `.nsp`) — container-verified,
//! bodies unmapped.
//!
//! The two products share every tag, and the EX ships the Classic's 297 synth
//! patches byte-for-byte; only programs differ between them. Named for the model
//! because its tags share no usable prefix (`nspg`, `nss\0`, `nsp\0`).
//!
//! ⚠️ Two of the three tags are three characters plus a NUL.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nspg`).
    program,
    "nspg",
    400
);
raw_format!(
    /// Synth patches (`.nss`).
    synth,
    "nss\0",
    27
);
raw_format!(
    /// Piano libraries (`.nsp`) — megabytes per file, so the raw body allocation
    /// is real; [`crate::cbin::inspect`] answers container questions in O(1).
    piano_library,
    "nsp\0"
);
