//! One timeline for Undo and Redo, as any other app has: the library's changes and the
//! open document's own edits, each stamped from one clock as it is made, undone or
//! made again. Undo takes back the latest change of either, and Redo makes again the
//! change undone last, unless a change made since ended its redo.

use std::sync::atomic::{AtomicU64, Ordering};

static CLOCK: AtomicU64 = AtomicU64::new(1);

/// The next moment on the timeline, later than every one before it.
pub fn tick() -> u64 {
    CLOCK.fetch_add(1, Ordering::Relaxed)
}

/// Where a change on the timeline was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The library: a rename, a move, a save.
    Library,
    /// The open document's own edits, such as a WAV's gain, not saved yet.
    Document,
}

/// What each side holds: when its last change not undone was made or made again, when
/// its last undo was, and when it last made a change of its own, not a redo; none where
/// it has none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub made: Option<u64>,
    pub undone: Option<u64>,
    pub changed: Option<u64>,
}

/// The side whose last change Undo takes back: the one made later.
pub fn undoes(library: Held, document: Held) -> Option<Side> {
    match (library.made, document.made) {
        (Some(l), Some(d)) => Some(if d > l { Side::Document } else { Side::Library }),
        (Some(_), None) => Some(Side::Library),
        (None, Some(_)) => Some(Side::Document),
        (None, None) => None,
    }
}

/// The side whose last undone change Redo makes again: the one undone later, where no
/// change on the other side was made after it. Each side ends its own redo when it makes
/// a change; this ends it for a change made on the other.
pub fn redoes(library: Held, document: Held) -> Option<Side> {
    let (side, undone, other) = match (library.undone, document.undone) {
        (Some(l), Some(d)) if d > l => (Side::Document, d, library.changed),
        (Some(l), _) => (Side::Library, l, document.changed),
        (None, Some(d)) => (Side::Document, d, library.changed),
        (None, None) => return None,
    };
    other.is_none_or(|changed| changed < undone).then_some(side)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(made: Option<u64>, undone: Option<u64>) -> Held {
        Held {
            made,
            undone,
            changed: made,
        }
    }

    #[test]
    fn undo_takes_back_the_latest_change_of_either_side() {
        let none = Held::default();
        assert_eq!(undoes(none, none), None);
        assert_eq!(
            undoes(held(Some(3), None), held(Some(5), None)),
            Some(Side::Document)
        );
        assert_eq!(
            undoes(held(Some(6), None), held(Some(5), None)),
            Some(Side::Library)
        );
        assert_eq!(undoes(none, held(Some(5), None)), Some(Side::Document));
        assert_eq!(undoes(held(Some(1), None), none), Some(Side::Library));
    }

    #[test]
    fn redo_makes_again_the_change_undone_last_unless_a_change_followed_it() {
        // A gain (2) after a rename (1); both undone, the rename last (4).
        assert_eq!(
            redoes(held(None, Some(4)), held(None, Some(3))),
            Some(Side::Library)
        );
        assert_eq!(
            redoes(held(None, Some(3)), held(None, Some(4))),
            Some(Side::Document)
        );
        // The rename undone (3), then a gain made (4): the rename's redo is over.
        assert_eq!(redoes(held(None, Some(3)), held(Some(4), None)), None);
        // A gain undone (4) after a rename (1): redo the gain.
        assert_eq!(
            redoes(held(Some(1), None), held(None, Some(4))),
            Some(Side::Document)
        );
        // A gain (1) undone (3) after a rename (2) undone (4); the rename redone (5): its
        // redo is no change of its own, so the gain's redo goes on.
        let redone = Held {
            made: Some(5),
            undone: None,
            changed: Some(2),
        };
        assert_eq!(redoes(redone, held(None, Some(3))), Some(Side::Document));
    }

    #[test]
    fn the_clock_only_moves_on() {
        let (a, b) = (tick(), tick());
        assert!(b > a);
    }
}
