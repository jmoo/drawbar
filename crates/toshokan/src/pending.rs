//! Pending records: the journal of multi-step effects, in `pending/<nonce>.json`.
//!
//! A record is written before an intent's first file effect and removed after its
//! entry is durable, only by its owner. The folder may be adversarial: a record is
//! acted on only when it is chained to the head the owner's local root recorded
//! and every path it names is a library path or in the owner's own directory.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use thiserror::Error as ThisError;

use crate::effects::EffectStep;
use crate::error::Result;
use crate::ids::{EntryHash, Nonce, WriterId};
use crate::io::Task;
use crate::layout::Layout;
use crate::log::Logged;
use crate::path::RelPath;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PendingRecord {
    pub writer: WriterId,
    /// The writer's head when the record was written.
    pub after: EntryHash,
    pub steps: Vec<EffectStep>,
    /// The entry to append once the steps are done.
    pub logged: Logged,
}

#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[error("not a pending record: {reason}")]
pub struct NotRecord {
    pub reason: String,
}

impl PendingRecord {
    /// Refuses bytes that are not a record, or longer than a reader holds.
    pub fn decode(bytes: &[u8]) -> std::result::Result<Self, NotRecord> {
        todo!()
    }

    pub fn encode(&self) -> Vec<u8> {
        todo!()
    }

    /// Whether every path the record names is a library path or under its
    /// writer's own directory. A record that fails is ignored and reported.
    pub fn is_confined(&self, layout: &Layout) -> bool {
        todo!()
    }

    /// The library paths the effect touches.
    pub fn paths(&self, layout: &Layout) -> Vec<RelPath> {
        todo!()
    }
}

/// Every pending record of `writer` that decodes, by name. Reads only.
pub fn read_all(
    layout: &Layout,
    writer: WriterId,
) -> Task<'static, Result<Vec<(Nonce, PendingRecord)>>> {
    todo!()
}
