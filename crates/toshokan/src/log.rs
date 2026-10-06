//! Entries: what one line of a writer's log says.
//!
//! An entry is the JSON of a [`Line`]: an object with `prev`, `at` (an [`Hlc`]) and
//! `kind`, and the members its kind defines. Unknown kinds, ops and members are
//! kept verbatim: the line itself is kept, and what does not decode is carried as
//! [`Raw`].

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use thiserror::Error as ThisError;

use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::line::{Line, LineError};
use crate::path::RelPath;
use crate::schema::Raw;

/// One verified, decoded line, kept with the line it was decoded from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub line: Line,
    pub at: Hlc,
    pub kind: EntryKind,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EntryKind {
    Genesis(Genesis),
    Intent(Logged),
    Settle(Settle),
    /// A kind this build does not know, or a known kind whose members do not
    /// decode: kept, merged by nothing, and reported.
    Unknown(Raw),
}

/// The first entry of a writer, with `prev` = [`EntryHash::ZERO`]. Its hash names
/// the writer's directory in the local root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Genesis {
    pub writer: WriterId,
    /// Shown to other writers beside this writer's entries.
    pub label: String,
}

/// A committed intent: its facts, the file effects it made, and the bindings the
/// writer pinned with it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Logged {
    pub label: String,
    pub ops: Vec<Op>,
    /// Bytes the intent moved into this writer's trash.
    pub displaced: Vec<Displaced>,
    /// The entry this one compensates, for an undo or redo.
    pub reverses: Option<EntryHash>,
}

/// One fact change. References name entry hashes: a write is identified by the
/// entry that logged it with its entity and key, so an intent writes each key of
/// an entity at most once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Op {
    /// A write of the existence register: the entity exists.
    Create {
        entity: EntityId,
        replaces: Vec<EntryHash>,
    },
    /// A write of the existence register: the entity is deleted. `observed` names
    /// the field writes the deleting writer had seen.
    Delete {
        entity: EntityId,
        replaces: Vec<EntryHash>,
        observed: Vec<EntryHash>,
    },
    /// A register write replacing the writes it names; `None` clears.
    Write {
        entity: EntityId,
        key: String,
        value: Option<Raw>,
        replaces: Vec<EntryHash>,
    },
    /// A set add, tagged by its entry.
    Add {
        entity: EntityId,
        key: String,
        value: Raw,
    },
    /// A set remove of the adds of `value` that the entries `tags` made.
    Remove {
        entity: EntityId,
        key: String,
        value: Raw,
        tags: Vec<EntryHash>,
    },
    /// A write of the entity's file register by a file effect; `None` says it has
    /// no file.
    File {
        entity: EntityId,
        file: Option<FileFact>,
        replaces: Vec<EntryHash>,
    },
    /// A write of the entity's file register recording a binding a scan derived.
    /// Merged as [`Op::File`]; undo leaves it alone.
    Pin {
        entity: EntityId,
        file: FileFact,
        replaces: Vec<EntryHash>,
    },
    /// An op this build does not know.
    Unknown(Raw),
}

/// Where an entity's file is and what it held when this writer last wrote or bound
/// it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileFact {
    pub path: RelPath,
    pub identity: Identity,
    pub len: u64,
    /// As the backend reported it; only equality means anything.
    pub modified: Option<u64>,
}

/// Bytes an intent moved from a library path into this writer's trash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Displaced {
    pub item: Nonce,
    pub from: RelPath,
    pub identity: Identity,
    pub len: u64,
}

/// This writer settled another writer's unfinished effect, with the user's consent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settle {
    pub writer: WriterId,
    pub record: Nonce,
    pub outcome: Settlement,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Settlement {
    Finished,
    RolledBack,
    /// Left as it was, and no longer reported.
    Dismissed,
}

/// A verified line whose JSON is not an entry: not an object with a readable `at`.
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[error("entry {hash} is malformed: {reason}")]
pub struct Malformed {
    pub hash: EntryHash,
    pub reason: String,
}

impl Entry {
    /// Decodes a verified line. A known kind whose members do not decode becomes
    /// [`EntryKind::Unknown`]; an unknown op becomes [`Op::Unknown`]; unknown members
    /// are ignored here and survive in the line.
    pub fn decode(line: Line) -> Result<Self, Malformed> {
        todo!()
    }

    /// The entry logging `kind` at `at` after `prev`.
    pub fn encode(prev: EntryHash, at: Hlc, kind: EntryKind) -> Result<Self, LineError> {
        todo!()
    }

    pub fn hash(&self) -> EntryHash {
        self.line.hash()
    }

    pub fn prev(&self) -> EntryHash {
        self.line.prev()
    }
}
