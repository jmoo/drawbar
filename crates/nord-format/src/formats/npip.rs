//! Nord C2 pipe-organ libraries (`.npip`) — container facts only.
//!
//! No specimen has been read: the tag is taken from the `.npip` extension, and
//! libraries are reported to run to tens of megabytes, so the raw body allocation
//! is real — [`crate::cbin::inspect`] answers container questions in O(1) instead.

use super::raw::raw_format;

raw_format!(
    /// The pipe library itself.
    pipe_library,
    "npip"
);
