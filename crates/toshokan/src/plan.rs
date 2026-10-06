//! What an intent asks for, before anything is checked or written. The intent
//! builder and undo produce plans; commit checks them against the view, logs their
//! facts and carries out their file effects.

use crate::ids::{EntityId, EntryHash, Identity, Nonce};
use crate::io::{Range, CHUNK};
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

/// Why a splice fails when its source is shorter than a range it keeps.
pub(crate) const SHRANK: &str = "the source of a splice no longer holds a range it keeps";

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
    /// The writes and copies that make the new file, in order.
    pub(crate) fn steps(self) -> impl Iterator<Item = Splicing> {
        let mut at = 0u64;
        self.pieces.into_iter().flat_map(move |piece| {
            let start = at;
            let steps: Vec<Splicing> = match piece {
                Piece::Bytes(bytes) => {
                    at = at.saturating_add(bytes.len() as u64);
                    vec![Splicing::Write { at: start, bytes }]
                }
                Piece::Kept(range) => {
                    at = at.saturating_add(range.len);
                    (0..range.len.div_ceil(CHUNK))
                        .map(|i| Splicing::Copy {
                            at: start.saturating_add(i * CHUNK),
                            range: Range {
                                offset: range.offset.saturating_add(i * CHUNK),
                                len: CHUNK.min(range.len - i * CHUNK),
                            },
                        })
                        .collect()
                }
            };
            steps
        })
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
            splice.steps().collect::<Vec<_>>(),
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
}
