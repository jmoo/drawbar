//! Folding a writer's own segments into a snapshot.
//!
//! A writer compacts only its own entries. It writes the snapshot under its content
//! name, and only then removes its own older segments and snapshots, so a reader that
//! meets both halves of an interrupted compaction merges them to the same state.

use crate::error::Result;
use crate::fs::{Fs, RelPath};
use crate::layout::Layout;
use crate::log::{Entry, LogWriter, WriterLog};
use crate::merge::State;

/// One writer's entries folded up to a segment.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Snapshot {
    /// The last segment folded in.
    pub through: u64,
    /// The folded facts, tombstones and set-remove observations included.
    pub state: State,
    /// The entries of the writer's undo window, kept whole so undo can reverse them.
    pub retained: Vec<Entry>,
}

impl Snapshot {
    /// The snapshot file's bytes; their BLAKE3 hash names the file.
    pub fn encode(&self) -> Vec<u8> {
        todo!()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let _ = bytes;
        todo!()
    }
}

/// What a compaction wrote and removed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Compaction {
    pub snapshot: RelPath,
    pub removed: Vec<RelPath>,
}

/// Fold `own`, this writer's log, into a new snapshot that keeps the last `window`
/// intents whole, then remove the segments and snapshots it supersedes.
pub async fn compact<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    own: &WriterLog,
    window: usize,
) -> Result<Compaction> {
    let _ = (fs, layout, log, own, window);
    todo!()
}
