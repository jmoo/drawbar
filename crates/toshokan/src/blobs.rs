//! The content-addressed blob store: bytes named by their BLAKE3 hash.
//!
//! A blob is written under the writer's `tmp/` directory and renamed into
//! `blobs/<hash>`, so a reader never sees a partial blob, and it is never rewritten.

use crate::error::Result;
use crate::fs::{Fs, RelPath};
use crate::ids::WriterId;
use crate::layout::Layout;
use crate::log::LogWriter;
use crate::merge::State;
use crate::value::BlobId;

/// Store `bytes` and return their id. Storing bytes already present changes nothing.
/// The caller logs `BlobAdded`.
pub async fn put<F: Fs>(fs: &F, layout: &Layout, writer: WriterId, bytes: &[u8]) -> Result<BlobId> {
    let _ = (fs, layout, writer, bytes);
    todo!()
}

/// Move the file at `path` into the store by rename, hashing it first, and return
/// its id and length. The caller logs `BlobAdded`.
pub async fn adopt<F: Fs>(fs: &F, layout: &Layout, path: &RelPath) -> Result<(BlobId, u64)> {
    let _ = (fs, layout, path);
    todo!()
}

pub async fn get<F: Fs>(fs: &F, layout: &Layout, blob: BlobId) -> Result<Vec<u8>> {
    let _ = (fs, layout, blob);
    todo!()
}

/// What a garbage collection removed and what it left.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Collection {
    pub removed: Vec<BlobId>,
    pub freed: u64,
    /// Bytes this writer's blobs still occupy.
    pub kept: u64,
}

/// Remove this writer's eligible blobs, oldest first, until its blobs occupy at most
/// `budget` bytes, logging `BlobRemoved` for each.
///
/// A blob is eligible when this writer added it, no live value refers to it, no entry
/// inside any writer's retained undo window refers to it, and no other writer's
/// retained log has added it.
pub async fn collect<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    state: &State,
    budget: u64,
) -> Result<Collection> {
    let _ = (fs, layout, log, state, budget);
    todo!()
}
