//! Recovery of interrupted effects, after a crash and after loss of the local root.
//!
//! Opening assesses and writes nothing in the folder. This writer's own records
//! are settled before its next write. A record of a writer this install no longer
//! writes as is reported as possibly still in progress elsewhere, and settled
//! only with the user's consent, in this writer's own log; only its owner ever
//! removes it. Settling twice equals settling once.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use std::collections::BTreeMap;

use crate::error::Result;
use crate::ids::{EntryHash, Nonce, WriterId};
use crate::io::Task;
use crate::layout::Layout;
use crate::log::Settlement;
use crate::path::RelPath;
use crate::pending::PendingRecord;
use crate::reader::WriterLog;
use crate::report::{Orphan, Outcome, Settled};
use crate::writer::Writer;

/// What opening found to recover.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Recovery {
    /// This writer's records, with how each will be settled.
    pub own: Vec<Settling>,
    pub orphaned: Vec<Orphan>,
    /// Records ignored as unchained or unconfined.
    pub ignored: Vec<RelPath>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settling {
    pub record: Nonce,
    pub pending: PendingRecord,
    pub outcome: Outcome,
}

/// Reads every writer's pending records. `own` is this writer and the head its
/// local root recorded. Every other writer's records are orphans: after the local
/// root is lost, an install cannot tell its former writer's records from a live
/// writer's.
pub fn assess<'a>(
    layout: &'a Layout,
    own: Option<(WriterId, EntryHash)>,
    logs: &'a BTreeMap<WriterId, WriterLog>,
) -> Task<'a, Result<Recovery>> {
    todo!()
}

/// Finishes or rolls back each of this writer's records, appends the entry a
/// finished record carries unless its log holds it, then removes the record.
pub fn settle_own<'a>(
    writer: &'a mut Writer,
    settling: &'a [Settling],
) -> Task<'a, Result<Vec<Settled>>> {
    todo!()
}

/// Settles another writer's record with the user's consent, recording a
/// [`crate::log::Settle`] entry in this writer's log. Never writes in the other
/// writer's directory.
pub fn settle_orphan<'a>(
    writer: &'a mut Writer,
    orphan: &'a Orphan,
    how: Settlement,
) -> Task<'a, Result<()>> {
    todo!()
}
