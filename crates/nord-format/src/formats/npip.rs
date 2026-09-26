//! Nord C2 pipe-organ libraries (`.npip`): container facts only.
//!
//! No specimen has been read, so the tag is taken from the `.npip` extension.
//! Libraries are reported to run to tens of megabytes, and reading the raw body
//! allocates all of it; [`crate::cbin::inspect`] answers container questions in O(1)
//! memory.

use super::raw::raw_format;

raw_format!(
    /// The pipe library itself.
    pipe_library,
    "npip"
);
