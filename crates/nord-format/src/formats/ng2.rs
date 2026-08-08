//! Nord Grand (`.ng2p`, `.ng2l`, `.ng2t`) — container-verified, bodies unmapped.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.ng2p`).
    program,
    "ng2p",
    185
);
raw_format!(
    /// Live slots (`.ng2l`) — same length as a program.
    live,
    "ng2l",
    185
);
raw_format!(
    /// Settings (`.ng2t`).
    settings,
    "ng2t",
    80
);
