//! A sync adversary for tests, mirroring the sync layer of `spec/Portable.tla`: one
//! folder that writers write and several readers see, each reader receiving every
//! file independently and in any order.

#![expect(
    dead_code,
    unused_variables,
    reason = "the skeleton's bodies are todo!()"
)]

use crate::disk::MemDisk;
use crate::path::RelPath;

/// One thing a sync client can do to a reader's copy of the folder. Paths are in the
/// folder root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SyncEvent {
    /// The origin's current bytes arrive whole.
    Deliver { path: RelPath },
    /// Only the first `len` bytes of the origin's current bytes arrive.
    Shorten { path: RelPath, len: u64 },
    /// The origin's deletion arrives, possibly before the file that replaces it.
    Delete { path: RelPath },
    /// The reader's copy disappears for a while, though the origin keeps it.
    Hide { path: RelPath },
    /// An earlier version of a file the origin deleted comes back.
    Resurrect { path: RelPath, version: usize },
    /// The origin's bytes arrive under another name beside the reader's version,
    /// as a sync client's conflicted copy.
    ConflictedCopy { path: RelPath, name: String },
}

pub struct Simulator {
    origin: MemDisk,
    readers: Vec<MemDisk>,
}

impl Simulator {
    pub fn new(readers: usize) -> Self {
        todo!()
    }

    /// The machine writers write on: its folder is the truth sync carries.
    pub fn origin(&self) -> &MemDisk {
        todo!()
    }

    /// Reader `index`'s machine: its folder holds what sync delivered, its local
    /// root is its own.
    pub fn reader(&self, index: usize) -> &MemDisk {
        todo!()
    }

    /// Every event that would change reader `index`'s folder now, in a fixed order,
    /// so a bounded search enumerates delivery orders deterministically.
    pub fn events(&self, reader: usize) -> Vec<SyncEvent> {
        todo!()
    }

    pub fn apply(&mut self, reader: usize, event: &SyncEvent) {
        todo!()
    }

    /// Delivers and deletes until reader `index`'s folder equals the origin's.
    pub fn settle(&mut self, reader: usize) {
        todo!()
    }
}
