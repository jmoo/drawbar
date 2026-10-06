//! Unsaved edits: opaque bytes over a base identity, kept in the local root and
//! never in the folder. Losing the local root loses them and nothing else.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use crate::error::Result;
use crate::ids::{EntityId, Identity};
use crate::path::RelPath;

/// An entity's unsaved edit being changed through `library`, a driver's library.
pub struct Draft<L> {
    library: L,
    entity: EntityId,
}

impl<L> Draft<L> {
    pub fn new(library: L, entity: EntityId) -> Self {
        Self { library, entity }
    }

    pub fn into_parts(self) -> (L, EntityId) {
        (self.library, self.entity)
    }
}

/// `drafts/<entity>.json` in a writer's local directory. It applies only while the
/// entity's file still holds `base`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DraftRecord {
    pub base: Identity,
    pub bytes: Vec<u8>,
}

impl DraftRecord {
    pub fn decode(path: &RelPath, bytes: &[u8]) -> Result<Self> {
        todo!()
    }

    pub fn encode(&self) -> Vec<u8> {
        todo!()
    }
}
