//! Undo and redo of a writer's own intents.
//!
//! Undo never edits the log: it appends compensating entries under a new intent whose
//! `Intent` entry names the intent it reverses. Redo reverses an undo the same way.

use std::fmt;

use crate::effects::Effect;
use crate::error::Result;
use crate::ids::{EntityId, IntentId, Version};
use crate::log::{Kind, WriterLog};
use crate::merge::State;
use crate::value::BlobId;

/// Why an undo or redo was refused. Nothing was changed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// The writer's undo window holds nothing to undo, or nothing to redo.
    Nothing,
    /// The field was written again after the intent wrote it.
    FieldChanged {
        entity: EntityId,
        name: String,
        wrote: Version,
        current: Option<Version>,
    },
    /// The bytes the intent displaced are no longer in the blob store.
    BlobGone { blob: BlobId },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nothing => f.write_str("there is nothing to reverse"),
            Self::FieldChanged { entity, name, .. } => {
                write!(f, "field {name:?} of {entity} has changed since")
            }
            Self::BlobGone { blob } => write!(f, "blob {blob} is no longer kept"),
        }
    }
}

/// A writer's own intents inside its undo window.
pub struct History {}

impl History {
    pub fn new(own: &WriterLog) -> Self {
        let _ = own;
        todo!()
    }

    /// The intent undo would reverse next.
    pub fn undoable(&self) -> Option<IntentId> {
        todo!()
    }

    /// The undo redo would reverse next.
    pub fn redoable(&self) -> Option<IntentId> {
        todo!()
    }
}

/// What reversing an intent takes: entries to append under a new intent, and file
/// effects to apply with them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub reverses: IntentId,
    pub entries: Vec<Kind>,
    pub effects: Vec<Effect>,
}

/// The plan that undoes the latest undoable intent, or [`crate::Error::Refused`].
pub fn plan_undo(history: &History, state: &State) -> Result<Plan> {
    let _ = (history, state);
    todo!()
}

/// The plan that redoes the latest undo, or [`crate::Error::Refused`].
pub fn plan_redo(history: &History, state: &State) -> Result<Plan> {
    let _ = (history, state);
    todo!()
}
