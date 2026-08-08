//! Nord C2 pipe-organ libraries (`.npip`) — container facts only.
//!
//! The one known specimen is a 56.7 MB CBIN blob, so the raw body allocation is
//! real — [`crate::cbin::inspect`] answers container questions in O(1) instead.
//!
//! The tag is inferred from the extension; no local specimen confirms it.

use super::raw::raw_format;

raw_format!(
    /// The pipe library itself.
    pipe_library,
    "npip"
);
