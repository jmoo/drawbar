//! Where the occupant of a slot waits while a write replaces it, when it is too large to
//! hold: a file under the library's `.drawbar/tmp/` on the desktop, or a `rescued` folder
//! of drawbar's own data where the library cannot take one, and `.drawbar/tmp/` of the
//! browser's private storage in the browser.
//!
//! A file is named as the rescue it becomes if the write and its restore both fail
//! ([`nord_usb::envelope::rescue_name_for`]). The library's sweep of `tmp/` leaves those
//! names alone, since one an interrupted write left behind is the slot's only copy, and
//! the next open offers each to the user ([`crate::store::Rescue`]).

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{shelf, Kept, Scratch};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{Kept, Scratch};

/// How many names a new file tries before giving up, its own name first.
const NAMES: u32 = 100;

/// `name` for the first try, then `name` with `-2`, `-3`, … before its extension, so an
/// earlier rescue of the same slot is never written over.
fn numbered(name: &str, attempt: u32) -> String {
    if attempt == 0 {
        return name.to_string();
    }
    let n = attempt + 1;
    match name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-{n}.{extension}"),
        None => format!("{name}-{n}"),
    }
}

/// Why no new file could be named.
fn all_taken(name: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("{NAMES} files named like {name} are already there"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_name_is_numbered_before_its_extension() {
        assert_eq!(
            numbered("nord-rescued-1-1.npno", 0),
            "nord-rescued-1-1.npno"
        );
        assert_eq!(
            numbered("nord-rescued-1-1.npno", 1),
            "nord-rescued-1-1-2.npno"
        );
        assert_eq!(numbered("plain", 2), "plain-3");
    }
}
