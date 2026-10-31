//! Where the occupant of a slot waits while a write replaces it: a file in the `rescued`
//! folder of this install's own state for the open library, on the desktop and in the
//! browser's storage alike, or of drawbar's own data where no library can take one.
//!
//! A file is named as the rescue it becomes if the write and its restore both fail
//! ([`nord_usb::envelope::rescue_name_for`]). It is written under a
//! [partial](nord_usb::envelope::PARTIAL) name, which the next open deletes, and takes its
//! own only once [`Kept::finish`] is called on it whole. One an interrupted write left
//! behind may be the slot's only copy, and goes into the library
//! ([`crate::store::Store::look_for_rescues`]).

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

/// What the file to be called `leaf` is called until it is finished.
fn partial(leaf: &str) -> String {
    format!("{leaf}{}", nord_usb::envelope::PARTIAL)
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
