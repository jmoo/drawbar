//! Nord C2 pipe-organ libraries (`.npip`): container facts only.
//!
//! Libraries run to tens of megabytes, and reading the raw body allocates all of
//! it; [`crate::cbin::inspect`] answers container questions in O(1) memory.

use super::raw::raw_format;

raw_format!(
    /// The pipe library itself.
    pipe_library,
    "npip"
);
