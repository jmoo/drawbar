//! Nord C2D (`.nc2p`, `.nc2s`) — container-verified, bodies unmapped.
//!
//! ⚠️ The C2D's tags carry the `nc2` prefix while the original C2 uses `ncpg` /
//! `ncsy` — the prefix names the *other* model. Dispatch by tag, never by guess.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nc2p`).
    program,
    "nc2p",
    347
);
raw_format!(
    /// Settings (`.nc2s`).
    settings,
    "nc2s",
    33
);
