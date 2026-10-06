//! Reading writers' logs from the folder and placing their entries by chain.
//!
//! A reader takes every file directly in `writers/<w>/`, whatever its name, and
//! reads it by its contents as segment lines or as a snapshot. It places an entry
//! once its predecessor is placed or folded by a snapshot it has seen; an entry
//! after a gap is held back. Two placed entries with one predecessor are a fork:
//! both branches are merged and the fork is reported once. What is placed is kept
//! in the cached view, which only grows. Reading writes nothing in the folder.

#![expect(
    dead_code,
    unused_variables,
    reason = "the skeleton's bodies are todo!()"
)]

use std::collections::BTreeMap;

use crate::error::Result;
use crate::ids::{EntryHash, Hlc, WriterId};
use crate::io::Task;
use crate::layout::Layout;
use crate::line::{Line, Stop};
use crate::log::Entry;
use crate::path::RelPath;
use crate::report::{Fork, Gap};
use crate::snapshot::Snapshot;

/// What one file in a writer's directory holds, judged by its contents.
#[derive(Clone, PartialEq, Debug)]
pub enum WriterFile {
    /// Lines up to the first unreadable one.
    Segment {
        lines: Vec<Line>,
        stop: Option<Stop>,
    },
    Snapshot(Box<Snapshot>),
    /// Neither: reported, and read again next time.
    Unreadable,
}

impl WriterFile {
    pub fn parse(bytes: &[u8]) -> Self {
        todo!()
    }
}

/// One writer's history as this install has placed it.
#[derive(Clone, PartialEq, Debug)]
pub struct WriterLog {
    writer: WriterId,
}

impl WriterLog {
    pub fn writer(&self) -> WriterId {
        todo!()
    }

    /// From the genesis entry, once placed or folded.
    pub fn label(&self) -> Option<&str> {
        todo!()
    }

    pub fn genesis(&self) -> Option<EntryHash> {
        todo!()
    }

    /// Whether `hash` is placed, or folded by a snapshot this install has seen.
    pub fn holds(&self, hash: EntryHash) -> bool {
        todo!()
    }

    /// The snapshots whose folded lists the placed entries continue. More than one
    /// only when the writer's history forked and each branch compacted.
    pub fn snapshots(&self) -> &[Snapshot] {
        todo!()
    }

    /// The placed entries no snapshot folds, each after its predecessor; fork
    /// branches in the order of their first entries' hashes.
    pub fn entries(&self) -> &[Entry] {
        todo!()
    }

    /// Placed or folded entries with no placed successor: one, unless forked.
    pub fn heads(&self) -> Vec<EntryHash> {
        todo!()
    }

    pub fn last_at(&self) -> Option<Hlc> {
        todo!()
    }

    /// Every fork found so far. Never shrinks.
    pub fn forks(&self) -> &[Fork] {
        todo!()
    }

    /// Entries seen in the last read but held back.
    pub fn gaps(&self) -> &[Gap] {
        todo!()
    }
}

/// What this install has placed of each writer, kept in the local root.
///
/// It only grows: a writer's facts never disappear from it because files arrive
/// in another order, are deleted early or vanish. It never holds an entry whose
/// predecessor it neither holds nor sees folded.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct CachedView {
    writers: BTreeMap<WriterId, WriterLog>,
}

impl CachedView {
    /// Refuses what this build cannot read with [`crate::Error::Corrupt`] naming
    /// `path`; the caller then reads from scratch.
    pub fn decode(path: &RelPath, bytes: &[u8]) -> Result<Self> {
        todo!()
    }

    pub fn encode(&self) -> Vec<u8> {
        todo!()
    }

    pub fn writers(&self) -> &BTreeMap<WriterId, WriterLog> {
        todo!()
    }
}

/// What one read placed and found.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReadReport {
    /// Newly placed entries per writer, in placement order.
    pub placed: BTreeMap<WriterId, Vec<EntryHash>>,
    /// Forks found by this read and not reported before.
    pub forks: Vec<Fork>,
    /// Every gap as of this read.
    pub gaps: Vec<Gap>,
    /// Files whose readable part ended early, or that are neither segment nor
    /// snapshot.
    pub unreadable: Vec<(RelPath, Option<Stop>)>,
}

pub struct Reader {
    layout: Layout,
    cached: CachedView,
}

impl Reader {
    pub fn new(layout: Layout, cached: CachedView) -> Self {
        todo!()
    }

    pub fn cached(&self) -> &CachedView {
        todo!()
    }

    pub fn logs(&self) -> &BTreeMap<WriterId, WriterLog> {
        todo!()
    }

    /// Reads every writer's directory and places what it can. Requests only
    /// [`crate::Io::List`], [`crate::Io::Stat`] and [`crate::Io::Read`] in the
    /// folder, each read bounded.
    pub fn read(&mut self) -> Task<'_, Result<ReadReport>> {
        todo!()
    }

    /// As [`Reader::read`], for one writer's directory.
    pub fn read_writer(&mut self, writer: WriterId) -> Task<'_, Result<ReadReport>> {
        todo!()
    }
}
