//! The state every writer's log adds up to.
//!
//! Merging is a pure function of the logs: commutative, associative and idempotent,
//! so any order of reading, and any overlap between a snapshot and the segments it
//! folded, gives the same state.

use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{EntityId, Version, WriterId};
use crate::log::{Entry, WriterLog};
use crate::value::{BlobId, Value};

/// The merged facts: fields, sets, existence and blob adds.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct State {}

/// One writer's record of a blob it added.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlobAdd {
    pub len: u64,
    /// The writer's latest `BlobAdded` or `BlobRemoved` for this blob.
    pub version: Version,
    /// Whether that latest entry was a removal.
    pub removed: bool,
}

/// The state of every writer's log.
pub fn merge(logs: &[WriterLog]) -> State {
    let _ = logs;
    todo!()
}

impl State {
    /// Fold one entry in. Applying an entry twice changes nothing.
    pub fn apply(&mut self, entry: &Entry) {
        let _ = entry;
        todo!()
    }

    /// Fold another state in.
    pub fn join(&mut self, other: &State) {
        let _ = other;
        todo!()
    }

    pub fn exists(&self, entity: EntityId) -> bool {
        let _ = entity;
        todo!()
    }

    /// Every entity that exists, in order.
    pub fn entities(&self) -> Vec<EntityId> {
        todo!()
    }

    pub fn field(&self, entity: EntityId, name: &str) -> Option<&Value> {
        let _ = (entity, name);
        todo!()
    }

    /// The version of the write that decided the field, including a write that
    /// cleared it.
    pub fn field_version(&self, entity: EntityId, name: &str) -> Option<Version> {
        let _ = (entity, name);
        todo!()
    }

    /// The fields an entity holds, by name.
    pub fn fields(&self, entity: EntityId) -> BTreeMap<&str, &Value> {
        let _ = entity;
        todo!()
    }

    /// Existing entities whose field `name` holds `value`, in order.
    pub fn find(&self, name: &str, value: &Value) -> Vec<EntityId> {
        let _ = (name, value);
        todo!()
    }

    /// The members of a set.
    pub fn members(&self, entity: EntityId, name: &str) -> BTreeSet<&Value> {
        let _ = (entity, name);
        todo!()
    }

    /// The live add tags of `value` in a set: what a remove must observe.
    pub fn tags(&self, entity: EntityId, name: &str, value: &Value) -> BTreeSet<Version> {
        let _ = (entity, name, value);
        todo!()
    }

    /// The highest Lamport time of any entry folded in.
    pub fn max_lamport(&self) -> u64 {
        todo!()
    }

    /// Every blob some writer has added, with each writer's record of it.
    pub fn blob_adds(&self) -> &BTreeMap<BlobId, BTreeMap<WriterId, BlobAdd>> {
        todo!()
    }

    /// Every blob a live value refers to, or an entry inside a writer's retained undo
    /// window refers to.
    pub fn referenced_blobs(&self) -> BTreeSet<BlobId> {
        todo!()
    }
}
