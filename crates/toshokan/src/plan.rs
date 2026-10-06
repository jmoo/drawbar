//! What an intent asks for, before anything is checked or written. The intent
//! builder and undo produce plans; commit checks them against the view, logs their
//! facts and carries out their file effects.

use crate::ids::{EntityId, EntryHash, Identity, Nonce};
use crate::path::RelPath;
use crate::schema::Raw;

/// An entity a plan names: one that exists, or the `n`th the plan creates.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Target {
    Existing(EntityId),
    New(usize),
}

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
    /// How many entities the intent creates; [`Target::New`] indexes them.
    pub creates: usize,
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
        entity: Target,
        key: String,
        value: Raw,
    },
    /// Replaces every write of the register this writer has observed with none.
    Clear { entity: Target, key: String },
    Add {
        entity: Target,
        key: String,
        value: Raw,
    },
    /// Removes every add of `value` this writer has observed.
    Remove {
        entity: Target,
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
    /// Writes `bytes` at `path` as the entity's file. Bytes it displaces go to this
    /// writer's trash.
    Save {
        entity: Target,
        path: RelPath,
        bytes: Vec<u8>,
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
    /// Brings displaced bytes back from this writer's trash to `to`.
    Restore {
        entity: EntityId,
        item: Nonce,
        to: RelPath,
        expect: Expect,
    },
}
