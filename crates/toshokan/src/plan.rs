//! What an intent asks for, before anything is checked or written. The intent
//! builder and undo produce plans; commit checks them against the view, logs their
//! facts and carries out their file effects.

use crate::ids::{EntityId, EntryHash, Identity, Nonce};
use crate::io::{IoError, Range, CHUNK};
use crate::path::RelPath;
use crate::schema::Raw;

/// The `n`th content an intent saves. The driver holds the content and fills the
/// staged file from it, so the core never holds a saved file's bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Content(pub usize);

/// What a file effect requires at a path before it runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expect {
    /// Nothing is there.
    Absent,
    /// A file whose identity is this is there.
    Holds(Identity),
}

/// One intent: the unit of commit, undo and attribution.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Plan {
    /// Shown in history and to other writers.
    pub label: String,
    /// The entities the intent creates, their ids drawn as it was built.
    pub created: Vec<EntityId>,
    pub facts: Vec<FactChange>,
    /// Carried out in order, each only after every precondition was checked.
    pub files: Vec<FileChange>,
    /// The entry an undo or redo compensates.
    pub reverses: Option<EntryHash>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FactChange {
    /// Replaces every write of the register this writer has observed.
    Set {
        entity: EntityId,
        key: String,
        value: Raw,
    },
    /// Replaces every write of the register this writer has observed with none.
    Clear { entity: EntityId, key: String },
    Add {
        entity: EntityId,
        key: String,
        value: Raw,
    },
    /// Removes every add of `value` this writer has observed.
    Remove {
        entity: EntityId,
        key: String,
        value: Raw,
    },
    /// Ends the entity, observing the field writes this writer has seen. A
    /// concurrent write it did not observe keeps the entity, in conflict.
    Delete { entity: EntityId },
    /// Brings a deleted entity back.
    Revive { entity: EntityId },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FileChange {
    /// Writes `content` at `path` as the entity's file. Bytes it displaces go to
    /// this writer's trash.
    Save {
        entity: EntityId,
        path: RelPath,
        content: Content,
        expect: Expect,
    },
    /// Moves the entity's file to this writer's trash.
    Trash { entity: EntityId, expect: Expect },
    /// Renames the entity's file. Refused when something is at `to`.
    Rename {
        entity: EntityId,
        to: RelPath,
        expect: Expect,
    },
    /// Moves everything under `from` to the same place under `to`. Refused when
    /// something is at `to`.
    MoveTree { from: RelPath, to: RelPath },
    /// Gives the entity the file at `path` as it is, moving nothing: for a library
    /// file no entity is bound to. Undo leaves the file where it is.
    Adopt {
        entity: EntityId,
        path: RelPath,
        expect: Expect,
    },
    /// Brings displaced bytes back from this writer's trash to `to`.
    Restore {
        entity: EntityId,
        item: Nonce,
        to: RelPath,
        expect: Expect,
    },
}

/// A file's new contents made from an existing file, `from`, as an edit of a large
/// file is written: the pieces in order, each the app's bytes or a range of `from`
/// copied through. A range `from` no longer holds whole fails the save.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Splice {
    pub from: RelPath,
    pub pieces: Vec<Piece>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Piece {
    Bytes(Vec<u8>),
    Kept(Range),
}

/// One request of a [`Splice`] being written, at `at` of the new file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Splicing {
    Write {
        at: u64,
        bytes: Vec<u8>,
    },
    /// `range` of the source, at most [`CHUNK`] bytes, copied to `at`.
    Copy {
        at: u64,
        range: Range,
    },
}

impl Splice {
    /// The writes and copies that make the new file, in order, each made as it is
    /// taken. A piece that would end past the largest offset, of its source or of
    /// the new file, is [`IoError::SpliceRange`] and ends them.
    pub(crate) fn steps(self) -> SpliceSteps {
        SpliceSteps {
            pieces: self.pieces.into_iter(),
            at: Some(0),
            copying: None,
        }
    }
}

/// The steps of a [`Splice`].
pub(crate) struct SpliceSteps {
    pieces: std::vec::IntoIter<Piece>,
    /// Where the next step writes in the new file; `None` once a piece failed.
    at: Option<u64>,
    /// What is left of the kept range being copied. It ends within the largest
    /// offset, in its source and from `at` in the new file.
    copying: Option<Range>,
}

impl SpliceSteps {
    /// Copies the first chunk of `range` to `at` and keeps the rest to copy.
    fn copy(&mut self, at: u64, range: Range) -> Splicing {
        let len = CHUNK.min(range.len);
        self.at = Some(at + len);
        let rest = Range {
            offset: range.offset + len,
            len: range.len - len,
        };
        self.copying = (rest.len > 0).then_some(rest);
        Splicing::Copy {
            at,
            range: Range {
                offset: range.offset,
                len,
            },
        }
    }
}

impl Iterator for SpliceSteps {
    type Item = Result<Splicing, IoError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let at = self.at?;
            if let Some(range) = self.copying.take() {
                return Some(Ok(self.copy(at, range)));
            }
            let piece = self.pieces.next()?;
            let (len, in_source) = match &piece {
                Piece::Bytes(bytes) => (bytes.len() as u64, true),
                Piece::Kept(range) => (range.len, range.offset.checked_add(range.len).is_some()),
            };
            let Some(end) = at.checked_add(len).filter(|_| in_source) else {
                self.at = None;
                return Some(Err(IoError::SpliceRange));
            };
            match piece {
                Piece::Bytes(bytes) => {
                    self.at = Some(end);
                    return Some(Ok(Splicing::Write { at, bytes }));
                }
                Piece::Kept(range) => self.copying = (range.len > 0).then_some(range),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_splice_copies_kept_ranges_a_chunk_at_a_time_after_its_bytes() {
        let splice = Splice {
            from: RelPath::new("a").unwrap(),
            pieces: vec![
                Piece::Bytes(b"head".to_vec()),
                Piece::Kept(Range {
                    offset: 10,
                    len: CHUNK + 1,
                }),
                Piece::Bytes(b"tail".to_vec()),
            ],
        };
        let range = |offset, len| Range { offset, len };
        assert_eq!(
            splice.steps().collect::<Result<Vec<_>, _>>().unwrap(),
            [
                Splicing::Write {
                    at: 0,
                    bytes: b"head".to_vec()
                },
                Splicing::Copy {
                    at: 4,
                    range: range(10, CHUNK)
                },
                Splicing::Copy {
                    at: 4 + CHUNK,
                    range: range(10 + CHUNK, 1)
                },
                Splicing::Write {
                    at: 5 + CHUNK,
                    bytes: b"tail".to_vec()
                },
            ]
        );
    }

    #[test]
    fn a_kept_range_of_any_length_yields_its_first_chunks_at_once() {
        let splice = Splice {
            from: RelPath::new("a").unwrap(),
            pieces: vec![Piece::Kept(Range {
                offset: 0,
                len: u64::MAX / 2,
            })],
        };
        let first: Vec<_> = splice.steps().take(2).collect();
        let copy = |at| Splicing::Copy {
            at,
            range: Range {
                offset: at,
                len: CHUNK,
            },
        };
        assert_eq!(first, [Ok(copy(0)), Ok(copy(CHUNK))]);
    }

    #[test]
    fn a_piece_ending_past_the_largest_offset_ends_the_splice() {
        let head = || Piece::Bytes(b"head".to_vec());
        let kept = |offset, len| Piece::Kept(Range { offset, len });
        for (shown, piece) in [
            ("in its source", kept(u64::MAX - 1, 2)),
            ("in the new file", kept(0, u64::MAX - 3)),
        ] {
            let splice = Splice {
                from: RelPath::new("a").unwrap(),
                pieces: vec![head(), piece, head()],
            };
            let steps: Vec<_> = splice.steps().collect();
            let written = Splicing::Write {
                at: 0,
                bytes: b"head".to_vec(),
            };
            assert_eq!(steps, [Ok(written), Err(IoError::SpliceRange)], "{shown}");
        }
    }
}
