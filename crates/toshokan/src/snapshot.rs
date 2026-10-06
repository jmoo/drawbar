//! Snapshots: a writer's own entries folded, with every folded hash in chain order.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use thiserror::Error as ThisError;

use crate::ids::{EntryHash, WriterId};
use crate::merge::Folded;

/// `snapshot-<nonce>.json` in a writer's directory.
///
/// `folded` lists every entry the snapshot folds, from the genesis entry on, each
/// the successor of the one before it, so a reader can place an entry after any
/// folded one and tell a fork from any point. Members this build does not know are
/// kept.
#[derive(Clone, PartialEq, Debug)]
pub struct Snapshot {
    pub writer: WriterId,
    pub label: String,
    pub folded: Vec<EntryHash>,
    pub state: Folded,
}

#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[error("not a snapshot: {reason}")]
pub struct NotSnapshot {
    pub reason: String,
}

impl Snapshot {
    /// Refuses bytes that are not a snapshot this build can place: not JSON, longer
    /// than a reader holds, or a `folded` list that does not start at a genesis.
    pub fn decode(bytes: &[u8]) -> Result<Self, NotSnapshot> {
        todo!()
    }

    pub fn encode(&self) -> Vec<u8> {
        todo!()
    }

    pub fn folds(&self, hash: EntryHash) -> bool {
        todo!()
    }

    /// The folded entry before `hash`: [`EntryHash::ZERO`] for the genesis entry,
    /// `None` when `hash` is not folded.
    pub fn predecessor(&self, hash: EntryHash) -> Option<EntryHash> {
        todo!()
    }

    /// The last folded entry.
    pub fn head(&self) -> Option<EntryHash> {
        todo!()
    }
}
