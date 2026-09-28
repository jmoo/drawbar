//! The folders a window opens as its library, one at a time, and the list of those it
//! opened lately.
//!
//! The list is a preference of the app, never kept in a library.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{name, Picker, Recent};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{can_pick, Heard, Libraries};

/// What the browser calls the default library.
pub const THIS_COMPUTER: &str = "This computer";

/// Whether this build can open a folder the user picks as the library.
#[cfg(not(target_arch = "wasm32"))]
pub fn can_pick() -> bool {
    true
}
