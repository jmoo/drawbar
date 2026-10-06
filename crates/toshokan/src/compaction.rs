//! Compaction: a writer folds its own entries into a snapshot, then deletes what the
//! snapshot supersedes. Only the owner compacts, and never a writer it no longer
//! writes as.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use crate::error::Result;
use crate::ids::Nonce;
use crate::io::Task;
use crate::reader::WriterLog;
use crate::report::Compacted;
use crate::writer::Writer;

/// Writes `snapshot-<name>.json` folding every entry of `own`, durably (file and
/// directory synced), and only then deletes the segments in [`Writer::sealed`]
/// whose every entry it folds, and the snapshots this process wrote that it
/// supersedes. Refuses a forked history.
pub fn compact<'a>(
    writer: &'a mut Writer,
    own: &'a WriterLog,
    name: Nonce,
) -> Task<'a, Result<Compacted>> {
    todo!()
}
