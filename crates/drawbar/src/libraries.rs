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

/// What a browser says the first time it opens a picked folder: its lock keeps other
/// tabs of drawbar.app to reading, and the desktop app never sees it.
pub const SHARED: &str = "Don't open this folder in the drawbar desktop app at the same time.";

/// [`SHARED`] where `warned` says it has not been said about this folder, which it then
/// says it has.
pub fn warn_once(warned: &mut bool) -> Option<&'static str> {
    (!std::mem::replace(warned, true)).then_some(SHARED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_warned_about_the_desktop_app_once() {
        let mut warned = false;
        assert_eq!(warn_once(&mut warned), Some(SHARED));
        assert_eq!(warn_once(&mut warned), None);
        assert!(warned, "kept with the folder");
    }
}
