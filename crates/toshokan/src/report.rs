//! What the library tells the app: on open, on commit, on refresh and on upkeep.

use crate::error::Why;
use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, SegmentName, WriterId};
use crate::io::IoError;
use crate::path::RelPath;

/// What opening found. Opening writes nothing in the folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Opened {
    pub mode: Mode,
    /// Whether the folder's renames refuse an existing destination themselves.
    /// Without it, a file another program makes at the destination of a file
    /// effect between toshokan's check and the rename is replaced, and its bytes
    /// leave the folder.
    pub no_replace: bool,
    /// Which writer this instance will write as.
    pub start: Start,
    /// This writer's effects that a crash interrupted. Opening writes nothing in
    /// the folder, so each is finished before this writer's next write, as its
    /// outcome predicts.
    pub settled: Vec<Settled>,
    /// Unfinished effects of writers this install no longer writes as. They may
    /// still be in progress elsewhere, so they are settled only with the user's
    /// consent.
    pub orphaned: Vec<Orphan>,
    pub drafts: Vec<DraftStatus>,
    /// Facts this install shows whose entries no file in the folder holds now: a
    /// restore of the folder removed them, or sync has not yet brought the files
    /// that hold them. They stay shown until the user lets them go or adopts them.
    pub removed: Vec<Change>,
    pub forks: Vec<Fork>,
    pub gaps: Vec<Gap>,
    /// Files in writers' directories that end before their last byte can be read,
    /// or that are neither segment nor snapshot. Each is read again next time.
    pub unreadable: Vec<RelPath>,
    /// Pending records ignored: unreadable, naming a path outside the library, or
    /// not chained to their writer's log.
    pub ignored: Vec<RelPath>,
    pub scan: ScanReport,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    Writable,
    ReadOnly(Why),
}

/// The writer an instance writes as. A new writer's id reaches the folder with its
/// genesis entry, at the first commit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Start {
    /// A writer from this install's pool, continuing its history.
    Resumed(WriterId),
    /// A new writer: the pool was empty, or every writer in it is in use.
    New,
    /// A new writer replacing one whose history can no longer be continued.
    Rekeyed { old: WriterId, why: Rekey },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rekey {
    /// Another instance wrote the same writer's history: a copied local root.
    Forked,
    /// The folder no longer holds this writer's last entry: it was restored from an
    /// older copy.
    Restored,
}

/// One of this writer's interrupted effects, and how settling it will end.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settled {
    pub record: Nonce,
    pub label: String,
    pub outcome: Outcome,
}

/// An unfinished effect of another writer, read from its pending record.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Orphan {
    pub writer: WriterId,
    pub record: Nonce,
    pub label: String,
    /// The library paths the effect touches.
    pub paths: Vec<RelPath>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DraftStatus {
    pub entity: EntityId,
    pub state: DraftState,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DraftState {
    /// The entity's file still holds the base the draft was made over: the draft's
    /// bytes, to restore.
    Applies { bytes: Vec<u8> },
    /// The file changed since; `None` when it is gone. The draft is kept until it
    /// is discarded.
    BaseChanged { found: Option<Identity> },
    /// The draft cannot be read. It is kept until it is discarded.
    Unreadable,
}

/// Two entries of one writer with one predecessor. Both branches are merged.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Fork {
    pub writer: WriterId,
    pub prev: EntryHash,
    /// Sorted.
    pub branches: [EntryHash; 2],
}

/// Entries of one writer held back because their predecessor is neither placed nor
/// folded by a snapshot a reader sees.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Gap {
    pub writer: WriterId,
    pub missing: EntryHash,
    pub held: usize,
}

/// What a scan of the library's files found against the logged facts. A scan
/// writes nothing.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ScanReport {
    /// Files no entity is bound to.
    pub arrived: Vec<RelPath>,
    /// Entities whose file is nowhere.
    pub departed: Vec<EntityId>,
    pub moved: Vec<Moved>,
    /// Files that hold an entity's contents while the entity's own path still does.
    pub copied: Vec<Copied>,
    /// Entities whose file is at its path with other contents.
    pub changed: Vec<EntityId>,
    /// Entities whose file is gone and which several files could be: bound to none.
    pub ambiguous: Vec<Ambiguous>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Moved {
    pub entity: EntityId,
    pub from: RelPath,
    pub to: RelPath,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Copied {
    pub entity: EntityId,
    pub copy: RelPath,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ambiguous {
    pub entity: EntityId,
    pub candidates: Vec<RelPath>,
}

/// A committed intent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Committed {
    /// The entry that logged it.
    pub intent: EntryHash,
    /// The entities it created, in the plan's order.
    pub created: Vec<EntityId>,
    pub changes: Vec<Change>,
    pub outcome: Outcome,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Complete,
    Partial(PartialReport),
}

/// An intent whose file effects stopped partway. Every effect before `applied`
/// was made; the pending record, while there is one, names what recovery will
/// finish.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PartialReport {
    pub applied: usize,
    pub stopped: RelPath,
    pub error: IoError,
    pub record: Option<Nonce>,
}

/// What a refresh found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refreshed {
    /// What changed since the last view, by its writer or from outside.
    pub changes: Vec<Change>,
    /// As [`Opened::removed`], as of this refresh.
    pub removed: Vec<Change>,
}

/// One change to what a reader sees.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Change {
    pub entity: EntityId,
    pub what: What,
    pub by: By,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum What {
    Created,
    Deleted,
    Field(String),
    File,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum By {
    This,
    Writer {
        writer: WriterId,
        label: String,
    },
    /// Found by a scan with no entry behind it.
    Outside,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WriterInfo {
    pub writer: WriterId,
    pub label: String,
    pub last_entry_at: Option<Hlc>,
    pub here: Presence,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
    This,
    /// Another instance on this machine holds its lock.
    SameMachine,
    Elsewhere,
}

/// One of this writer's intents, for undo and redo.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistoryItem {
    pub intent: EntryHash,
    pub label: String,
    pub at: Hlc,
    pub undone: bool,
}

/// Displaced bytes in this writer's trash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrashItem {
    pub item: Nonce,
    pub len: u64,
    /// Where the bytes were.
    pub from: RelPath,
    pub at: Hlc,
    /// The entry that displaced them.
    pub by: EntryHash,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Emptied {
    pub removed: Vec<Nonce>,
    pub bytes: u64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Compacted {
    pub snapshot: Nonce,
    pub folded: usize,
    /// Segments this process sealed, folded and deleted.
    pub removed: Vec<SegmentName>,
}
