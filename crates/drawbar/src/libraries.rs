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
pub use web::{picking, Heard, Libraries};

/// What the browser calls the default library.
pub const THIS_COMPUTER: &str = "This computer";

/// What a grayed-out Open library folder… says on hover, in Brave.
pub const TURNED_OFF: &str = "Brave turns off folder access. Turn on \
     brave://flags/#file-system-access-api and relaunch Brave.";

/// How far this build can open a folder the user picks as the library.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Picking {
    On,
    /// The browser can, once the user turns folder access on, as [`TURNED_OFF`] says.
    TurnedOff,
    Absent,
}

/// Whether this build can open a folder the user picks as the library.
#[cfg(not(target_arch = "wasm32"))]
pub fn picking() -> Picking {
    Picking::On
}
