//! What the browser library makes of the browser's file system answers, kept apart from
//! the browser so it can be tested anywhere.

use std::cell::Cell;
use std::io;

use super::Stat;

/// An error from the browser, by its `DOMException` name, as the kind of I/O error it is.
pub(super) fn failure(name: &str, message: &str) -> io::Error {
    let kind = match name {
        "NotFoundError" => io::ErrorKind::NotFound,
        "QuotaExceededError" => io::ErrorKind::StorageFull,
        "NotSupportedError" => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    match name {
        "" | "Error" => io::Error::new(kind, message.to_string()),
        _ => io::Error::new(kind, format!("{name}: {message}")),
    }
}

/// Whether a folder lets the page move a file, learned from the first move it refuses.
///
/// Brave refuses `move()` anywhere but the origin private file system, so in a picked
/// folder a file is copied to its new name and the old one removed.
pub(super) struct Moves {
    /// A refused move may be made by copying instead: true for a picked folder only.
    copies: bool,
    /// The folder has refused a move, so later ones copy without asking.
    refused: Cell<bool>,
}

impl Moves {
    pub(super) fn new(copies: bool) -> Moves {
        Moves {
            copies,
            refused: Cell::new(false),
        }
    }

    /// Whether a move is worth asking for: until the folder refuses one.
    pub(super) fn tries(&self) -> bool {
        !self.refused.get()
    }

    /// Whether a move that failed with `e` is to be made by copying instead. A refusal
    /// as [`io::ErrorKind::Unsupported`] is remembered, and any other failure stands.
    pub(super) fn copy_after(&self, e: &io::Error) -> bool {
        let copy = self.copies && e.kind() == io::ErrorKind::Unsupported;
        if copy {
            self.refused.set(true);
        }
        copy
    }
}

/// Whether a file copied to its new name may be removed from its old one, `name`, which
/// was copied as it stood at `copied` and is at `now`: only while it is unchanged, and
/// not where it is gone already. A file changed since is refused, and both copies stay.
pub(super) fn copied_away(name: &str, copied: Stat, now: Option<Stat>) -> io::Result<bool> {
    match now {
        None => Ok(false),
        Some(now) if now == copied => Ok(true),
        Some(_) => Err(io::Error::other(format!(
            "“{name}” changed while it was being moved, so both copies are kept"
        ))),
    }
}

/// The file `getFileHandle(name, {create: true})` gave, at `stat`, where nothing was at
/// `name` a moment before: one this made only where it is empty. Otherwise another
/// program made it first, and the name is taken.
pub(super) fn made(name: &str, stat: Stat) -> io::Result<Stat> {
    match stat.len {
        0 => Ok(stat),
        _ => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("something else made “{name}” first"),
        )),
    }
}

/// Whether a file this made, at `made`, may be removed after a write to it failed: only
/// while it is still empty and untouched, at `now`.
pub(super) fn unmade(made: Stat, now: Option<Stat>) -> bool {
    now == Some(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browsers_errors_keep_their_kind_and_name() {
        let kind = |name| failure(name, "why").kind();
        assert_eq!(kind("NotFoundError"), io::ErrorKind::NotFound);
        assert_eq!(kind("QuotaExceededError"), io::ErrorKind::StorageFull);
        assert_eq!(kind("NotSupportedError"), io::ErrorKind::Unsupported);
        assert_eq!(kind("InvalidStateError"), io::ErrorKind::Other);
        let refused = failure("NotSupportedError", "The implementation did not support");
        assert_eq!(
            refused.to_string(),
            "NotSupportedError: The implementation did not support"
        );
        assert_eq!(failure("Error", "plain").to_string(), "plain");
    }

    /// A picked folder that refuses a move copies that file and every later one,
    /// without asking again.
    #[test]
    fn a_picked_folder_that_refuses_a_move_copies_from_then_on() {
        let moves = Moves::new(true);
        assert!(moves.tries());
        assert!(moves.copy_after(&failure("NotSupportedError", "no")));
        assert!(!moves.tries());
    }

    /// Only a refusal is copied around: any other failure is the move's answer, and the
    /// next move is asked for again.
    #[test]
    fn any_other_failure_stands() {
        let moves = Moves::new(true);
        for name in [
            "NotFoundError",
            "QuotaExceededError",
            "InvalidStateError",
            "",
        ] {
            assert!(!moves.copy_after(&failure(name, "no")), "{name}");
        }
        assert!(!moves.copy_after(&io::Error::other("no")));
        assert!(moves.tries());
    }

    /// The private file system always moves: a refusal there stands.
    #[test]
    fn the_private_file_system_never_copies() {
        let moves = Moves::new(false);
        assert!(!moves.copy_after(&failure("NotSupportedError", "no")));
        assert!(moves.tries());
    }

    fn at(len: u64, modified: u64) -> Stat {
        Stat {
            len,
            modified: Some(modified),
        }
    }

    /// A source is removed after its copy only while it is as it was copied; one changed
    /// meanwhile is refused and kept, and one gone already is left alone.
    #[test]
    fn a_source_changed_during_its_copy_is_kept() {
        let copied = at(10, 1);
        assert!(copied_away("c3.wav", copied, Some(copied)).unwrap());
        assert!(!copied_away("c3.wav", copied, None).unwrap());
        for now in [at(11, 1), at(10, 2)] {
            let refused = copied_away("c3.wav", copied, Some(now)).unwrap_err();
            assert!(
                refused.to_string().contains("both copies are kept"),
                "{refused}"
            );
        }
    }

    /// A file found holding bytes where drawbar meant to make one is another program's:
    /// the name is taken, and nothing is written or removed.
    #[test]
    fn a_file_someone_else_made_first_is_theirs() {
        assert_eq!(made("c3.wav", at(0, 1)).unwrap(), at(0, 1));
        let taken = made("c3.wav", at(3, 1)).unwrap_err();
        assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
    }

    /// A file this made is removed after a failed write only while it is empty and
    /// untouched.
    #[test]
    fn only_an_untouched_empty_file_is_unmade() {
        let made = at(0, 1);
        assert!(unmade(made, Some(made)));
        assert!(!unmade(made, Some(at(0, 2))));
        assert!(!unmade(made, Some(at(4, 1))));
        assert!(!unmade(made, None));
    }
}
