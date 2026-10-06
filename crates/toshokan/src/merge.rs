//! The merge: a join over entries.
//!
//! Folding an entry and joining two states commute, associate and are idempotent,
//! so readers holding the same entries compute the same state whatever was
//! compacted and in whatever order files arrived. Registers keep every write with a
//! grow-only set of replaced writes, so a snapshot never resurrects a write another
//! log still holds; sets are observed-remove; existence is a register whose delete
//! observes field writes. Unknown entries and ops are kept and joined verbatim.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::{EntityId, WriterId};
use crate::log::{Entry, FileFact};
use crate::reader::WriterLog;
use crate::schema::{Raw, Written};

/// The folded state of a set of entries.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Folded {
    unknown: Vec<Raw>,
}

impl Folded {
    /// Folds one entry `writer` logged.
    pub fn apply(&mut self, writer: WriterId, entry: &Entry) {}

    pub fn join(&mut self, other: &Folded) {}

    /// Every entity that exists, or whose deletion is in conflict.
    pub fn entities(&self) -> Vec<EntityId> {
        todo!()
    }

    /// The surviving writes of a register as JSON; empty when unset.
    pub fn register(&self, entity: EntityId, key: &str) -> Vec<Written<Raw>> {
        todo!()
    }

    /// The members of a set as JSON, sorted and without duplicates.
    pub fn members(&self, entity: EntityId, key: &str) -> Vec<Raw> {
        todo!()
    }

    /// The surviving writes of every existing entity's file register. More than
    /// one is a conflict.
    pub fn files(&self) -> BTreeMap<EntityId, Vec<Written<FileFact>>> {
        todo!()
    }
}

/// Snapshots and the cached view store this; members this build does not know
/// are kept.
impl Serialize for Folded {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Stored {
            unknown: self.unknown.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Folded {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Stored::deserialize(deserializer).map(|stored| Self {
            unknown: stored.unknown,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    unknown: Vec<Raw>,
}

/// The join of every writer's snapshots and placed entries.
pub fn merge<'a>(logs: impl IntoIterator<Item = &'a WriterLog>) -> Folded {
    Folded::default()
}
