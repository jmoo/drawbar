//! Each writer's append-only log: its entries, their line encoding, and its segments.
//!
//! A writer appends only to its own directory, `writers/<writer>/` (see
//! [`crate::layout`]), and never edits or removes another writer's bytes. Each line
//! is an entry's JSON, a tab, and the CRC-32 of the JSON in eight lowercase
//! hexadecimal digits, ending in a newline. A line that fails its checksum, or a
//! final line without its newline, ends the readable segment: everything before it
//! counts, and nothing after it does.

use std::collections::BTreeSet;

use crate::compact::Snapshot;
use crate::error::Result;
use crate::fs::Fs;
use crate::ids::{EntityId, IntentId, Version, WriterId};
use crate::layout::Layout;
use crate::value::{BlobId, Value};

/// One line of a writer's log.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// Unique across all writers; orders this entry against every other.
    pub version: Version,
    /// The intent this entry belongs to, which undo reverses as a whole.
    pub intent: IntentId,
    pub kind: Kind,
}

/// What an entry records.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The first entry of every intent.
    Intent {
        label: Option<String>,
        /// The intent this one undoes or redoes.
        reverses: Option<IntentId>,
    },
    /// Sets the entity's last-writer-wins `exists` register to true.
    Create { entity: EntityId },
    /// Sets the entity's `exists` register to false. Its fields are kept.
    Delete { entity: EntityId },
    /// Writes a last-writer-wins field; `None` clears it.
    Field {
        entity: EntityId,
        name: String,
        value: Option<Value>,
        /// What this writer's merged state held when it wrote, which undo restores.
        prior: Option<Value>,
    },
    /// Adds `value` to an add-wins set, tagged with this entry's version.
    SetAdd {
        entity: EntityId,
        name: String,
        value: Value,
    },
    /// Removes the adds of `value` whose tags this writer had observed. An add it
    /// had not observed survives.
    SetRemove {
        entity: EntityId,
        name: String,
        value: Value,
        observed: BTreeSet<Version>,
    },
    /// This writer put `blob`, of `len` bytes, in the blob store.
    BlobAdded { blob: BlobId, len: u64 },
    /// This writer's garbage collection removed `blob` from the blob store.
    BlobRemoved { blob: BlobId },
    /// An entry this build does not understand, kept verbatim and ignored by merge.
    Unknown {
        kind: String,
        /// The entry's JSON exactly as it was read.
        json: String,
    },
}

/// The line for `entry`, newline included.
pub fn encode_line(entry: &Entry) -> String {
    let _ = entry;
    todo!()
}

/// The entries of one segment's bytes, up to any torn tail.
pub fn read_segment(bytes: &[u8]) -> Segment {
    let _ = bytes;
    todo!()
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Segment {
    pub entries: Vec<Entry>,
    /// Where the readable entries end early; `None` when every byte was read.
    pub torn: Option<Torn>,
}

/// A segment's readable end, before the bytes that could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Torn {
    /// The byte offset of the first unreadable line.
    pub offset: u64,
}

/// One writer's log as read from its directory.
#[derive(Clone, PartialEq, Debug)]
pub struct WriterLog {
    pub writer: WriterId,
    pub snapshot: Option<Snapshot>,
    /// The entries of every segment, oldest first. They may overlap the snapshot.
    pub entries: Vec<Entry>,
    /// The segments that ended early, by segment number.
    pub torn: Vec<(u64, Torn)>,
}

impl WriterLog {
    /// Whether the log holds an entry this build does not understand.
    pub fn has_unknown(&self) -> bool {
        todo!()
    }
}

/// `writer`'s log; empty when the writer has no directory.
pub async fn read_log<F: Fs>(fs: &F, layout: &Layout, writer: WriterId) -> Result<WriterLog> {
    let _ = (fs, layout, writer);
    todo!()
}

/// Every writer's log, sorted by writer.
pub async fn read_logs<F: Fs>(fs: &F, layout: &Layout) -> Result<Vec<WriterLog>> {
    let _ = (fs, layout);
    todo!()
}

/// One writer's handle for appending: its clock and counters.
pub struct LogWriter {
    writer: WriterId,
}

impl LogWriter {
    /// A handle for `writer`, given every writer's log as read at open. The clock
    /// starts past every version in `logs`, the counters past every id `writer`
    /// has allocated. The handle is read-only when `writer`'s own log has an entry
    /// this build does not understand.
    pub fn open(writer: WriterId, logs: &[WriterLog]) -> Self {
        let _ = logs;
        Self { writer }
    }

    pub fn writer(&self) -> WriterId {
        self.writer
    }

    /// Why this writer cannot append, if it cannot.
    pub fn read_only(&self) -> Option<&str> {
        todo!()
    }

    /// Raise the clock past a version seen since open.
    pub fn observe(&mut self, version: Version) {
        let _ = version;
        todo!()
    }

    pub fn new_entity(&mut self) -> EntityId {
        todo!()
    }

    pub fn new_intent(&mut self) -> IntentId {
        todo!()
    }

    /// An entry of `intent` with the next version.
    pub fn stamp(&mut self, intent: IntentId, kind: Kind) -> Entry {
        let _ = (intent, kind);
        todo!()
    }

    /// Append `entries` to this writer's log; they are durable when this returns.
    /// Refuses with [`crate::Error::ReadOnly`] when [`Self::read_only`] says so.
    pub async fn append<F: Fs>(
        &mut self,
        fs: &F,
        layout: &Layout,
        entries: &[Entry],
    ) -> Result<()> {
        let _ = (fs, layout, entries);
        todo!()
    }
}
