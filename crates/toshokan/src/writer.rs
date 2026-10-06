//! This instance's writer: the only code that writes under `writers/<w>/` for its
//! own `w`, and it writes nowhere else in toshokan's root.
//!
//! A writer opens a new segment under a random name the first time it appends in a
//! process, appends to it, and seals it when the process closes it cleanly. It
//! deletes only segments this process opened and sealed. Before each append it
//! confirms the folder still holds its head; after each, it records the head in
//! the local root.

#![expect(
    dead_code,
    unused_variables,
    reason = "the skeleton's bodies are todo!()"
)]

use std::collections::BTreeMap;

use crate::error::Result;
use crate::ids::{EntryHash, Hlc, SegmentName, WriterId};
use crate::io::Task;
use crate::layout::Layout;
use crate::log::{Entry, EntryKind};
use crate::reader::WriterLog;
use crate::report::Start;

pub struct Writer {
    layout: Layout,
    id: WriterId,
    genesis: EntryHash,
    head: EntryHash,
}

/// The segment this process appends to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenSegment {
    pub name: SegmentName,
    pub len: u64,
}

/// What claiming a writer from the pool found.
pub struct Claimed {
    /// `None` when no writer in the pool can continue: the first commit creates one.
    pub writer: Option<Writer>,
    pub start: Start,
}

impl Writer {
    /// Takes the first writer in this install's pool, the directories of the local
    /// root, whose lock it gets and whose history the folder continues.
    ///
    /// A writer whose history forked (another instance appended after its head) or
    /// whose head the folder no longer holds (a restored folder) is never written
    /// again; [`Claimed::start`] reports it as [`Start::Rekeyed`]. Writes nothing in
    /// the folder.
    pub fn claim(
        layout: &Layout,
        logs: &BTreeMap<WriterId, WriterLog>,
    ) -> Task<'static, Result<Claimed>> {
        todo!()
    }

    /// A new writer: its first segment, `segment`, holding its genesis entry, is
    /// made durable in the folder before its directory in the local root, and so
    /// its id, exists anywhere else.
    pub fn create(
        layout: Layout,
        id: WriterId,
        segment: SegmentName,
        label: String,
        at: Hlc,
    ) -> Task<'static, Result<Writer>> {
        todo!()
    }

    pub fn id(&self) -> WriterId {
        todo!()
    }

    pub fn genesis(&self) -> EntryHash {
        todo!()
    }

    pub fn head(&self) -> EntryHash {
        todo!()
    }

    pub fn open_segment(&self) -> Option<OpenSegment> {
        todo!()
    }

    /// Appends `kinds` as consecutive entries, durably, opening a segment named
    /// `fresh` when none is open. Refuses, appending nothing, when the folder no
    /// longer holds this writer's head.
    pub fn append(
        &mut self,
        kinds: Vec<(Hlc, EntryKind)>,
        fresh: SegmentName,
    ) -> Task<'_, Result<Vec<Entry>>> {
        todo!()
    }

    /// Ends the open segment: this process will not append to it again, and may
    /// delete it once a snapshot folds it. No request.
    pub fn seal(&mut self) {
        todo!()
    }

    /// Segments this process opened and sealed: the only ones it may delete.
    pub fn sealed(&self) -> &[SegmentName] {
        todo!()
    }

    /// Seals the open segment and releases the writer's lock.
    pub fn close(&mut self) -> Task<'_, Result<()>> {
        todo!()
    }
}
