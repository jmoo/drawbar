//! Nord Lead 4 (`.nl4s`, `.nl4p`, `.nl4t`): container-verified, bodies unmapped.
//!
//! ⚠️ The Leads invert the `p` convention: `.nl4s` is a program (a sound) and `.nl4p`
//! is a performance. The earlier Leads (1, 2, 2X, 3) ship SysEx instead of CBIN; see
//! [`super::sysex`].

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nl4s`; the vendor calls them sounds).
    program,
    "nl4s",
    317
);
raw_format!(
    /// Performances (`.nl4p`): four slots plus the morphs that bind them.
    performance,
    "nl4p",
    1269
);
raw_format!(
    /// Settings (`.nl4t`).
    settings,
    "nl4t",
    38
);
