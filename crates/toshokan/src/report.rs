//! What the library tells the app: on open, on commit, on refresh and on upkeep.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::Why;
use crate::ids::{EntityId, EntryHash, Hlc, Nonce, WriterId};
use crate::io::IoError;
use crate::log::Run;
use crate::path::RelPath;
use crate::plan::FactChange;

/// What opening found. Opening writes nothing in the folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Opened {
    pub mode: Mode,
    /// Which writer this instance will write as.
    pub start: Start,
    /// This writer's effects that a crash interrupted. Opening writes nothing in
    /// the folder, so each is finished before this writer's next write, as its
    /// outcome predicts, or when the app settles them sooner
    /// (`settle_interrupted`).
    pub settled: Vec<Settled>,
    /// Unfinished effects of writers this install no longer writes as. They may
    /// still be in progress elsewhere, so they are settled only with the user's
    /// consent.
    pub orphaned: Vec<Orphan>,
    /// Files in writers' directories that end before their last byte can be read,
    /// or that are neither segment nor snapshot. Each is read again next time.
    pub unreadable: Vec<RelPath>,
    /// Pending records ignored: unreadable, naming a path outside the library, or
    /// not chained to their writer's log.
    pub ignored: Vec<RelPath>,
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
    /// A write reported as failed reached the folder all the same: the folder
    /// holds an entry this writer does not know it wrote.
    Landed,
}

/// One of this writer's interrupted effects, and how settling it will end.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settled {
    pub record: Nonce,
    pub label: String,
    pub outcome: Outcome,
}

/// An unfinished effect of another writer, read from its pending record.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Orphan {
    pub writer: WriterId,
    pub record: Nonce,
    pub label: String,
    /// The library paths the effect touches.
    pub paths: Vec<RelPath>,
    /// The writer is one of this install's pool that no instance writes as now, as
    /// another window of the app wrote until it closed: its effect is this
    /// install's to settle, with no one else to ask.
    pub ours: bool,
    /// Another instance on this machine held the writer's lock when this one
    /// opened, or at the refresh that reported it: it may be carrying the effect out
    /// still.
    pub live: bool,
    /// What the folder showed of the effect's paths, hashed: it moves while a writer
    /// carries the effect out, so two reads that show it unchanged a while apart
    /// say no writer is. 0 until a refresh looks.
    pub progress: u64,
}

/// Where one writer's history forked: the entry numbered `at`, `prev`, after
/// which its branches part. Neither branch is merged; each is shown as an
/// [`Item`] of [`ItemKind::Run`] whose run's writer is `writer`, until it is
/// dismissed. A fork stays listed after that: it is settled once no such item
/// is left, and has nothing left to review once each such item's changes are
/// empty.
///
/// When the writer is this instance's own, the instance stops writing as it
/// once a read finds the fork: [`Start::Rekeyed`] says so at opening,
/// [`Refreshed::retired`] at a refresh or rescan, and [`crate::Error::Rekey`]
/// from a compaction. Its next commit writes as a new writer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Fork {
    pub writer: WriterId,
    pub at: u64,
    pub prev: EntryHash,
}

/// Entries of one writer held back because their predecessor is neither placed nor
/// folded by a snapshot a reader sees.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Gap {
    pub writer: WriterId,
    pub missing: EntryHash,
    pub held: usize,
}

/// What a reader shows apart from the merged state.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ItemKind {
    /// A branch of a forked writer, or entries after a dismissed one.
    Run,
    /// Entries whose predecessor may still arrive. Merged once it does.
    Gap,
    /// Lines of an unsealed file shorter than a snapshot certified it: a version
    /// a sync client dropped, a copy that diverged, or a file still arriving. A
    /// line's write to a field its writer's merged history writes at the line's
    /// number or above is left out.
    Unplaced,
}

/// Entries a reader shows apart from the merged state, with the net change they
/// would make. The user keeps the current state or takes changes, with
/// [`crate::intent::Intent::take`] naming `run`, and may dismiss a run or
/// unplaced lines with [`crate::intent::Intent::dismiss`]; a gap cannot be
/// dismissed. A run is shown until it is dismissed, with no changes once it
/// would change nothing; a gap only while it would change something, and
/// unplaced lines while they would and no dismissal names exactly their `run`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item {
    pub kind: ItemKind,
    pub run: Run,
    pub by: By,
    /// Its entries' clock readings, earliest and latest. Of the entries a
    /// snapshot folds, a reader knows only the head's reading and those of the
    /// writes the snapshot still holds, so a run that a snapshot holds starts at
    /// the earliest of those.
    pub span: (Hlc, Hlc),
    /// How many entries it holds. Of a run that a snapshot holds, that counts
    /// every number from its first entry no dismissal names to the snapshot's
    /// head, whether or not a write of the entry survives.
    pub entries: u64,
    pub changes: Vec<ItemChange>,
}

/// One field an [`Item`] would change.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ItemChange {
    pub entity: EntityId,
    pub what: What,
    /// The change that takes it; `None` where the run left the entity without a
    /// file, which only trashing the file does. Taking a file binds the entity to
    /// the file the run names, which the folder must hold at its path, as the
    /// last scan found it, for no other entity: else the intent is refused with
    /// [`crate::Invalid::Unbindable`].
    pub change: Option<FactChange>,
    /// Shown whatever the merged state holds: the run replaced an earlier own
    /// write no reader holds any more, so only a take settles it.
    pub forgotten: bool,
}

/// What a scan of the library's files found against the logged facts. A scan
/// writes nothing. Files are listed in order of path, and entities in order of
/// id.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ScanReport {
    /// Files no entity is bound to.
    pub arrived: Vec<RelPath>,
    /// Entities whose file is nowhere.
    pub departed: Vec<EntityId>,
    pub moved: Vec<Moved>,
    /// Entities whose file is at its path with other contents.
    pub changed: Vec<EntityId>,
    /// Entities whose file is gone and which several files could be: bound to none.
    pub ambiguous: Vec<Ambiguous>,
    /// Entities whose file facts hold a file another entity's hold too: bound to none.
    pub joined: Vec<Joined>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Moved {
    pub entity: EntityId,
    pub from: RelPath,
    pub to: RelPath,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ambiguous {
    pub entity: EntityId,
    pub candidates: Vec<RelPath>,
}

/// An entity whose file facts hold the file bound to `into`, the lowest of the
/// entities whose facts hold it, as when two writers each adopted the file before
/// seeing the other's. [`crate::intent::Intent::join`] gives `into` what it holds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Joined {
    pub entity: EntityId,
    pub into: EntityId,
}

/// Where a file no entity is bound to was before a commit and is after it: `from`
/// alone for a file trashed, `to` alone for a file made, both for one moved or saved
/// over.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PathChange {
    pub from: Option<RelPath>,
    pub to: Option<RelPath>,
}

/// A committed intent, its file effects all made. Its entries are durable in the
/// folder and shown.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Committed {
    /// The entry that logged it.
    pub intent: EntryHash,
    /// The entities it created, in the order the intent created them.
    pub created: Vec<EntityId>,
    pub changes: Vec<Change>,
    /// Each file no entity is bound to that it made, moved or trashed, in order,
    /// but for those under a tree it moved.
    pub paths: Vec<PathChange>,
    /// Each tree it moved, from and to, in order.
    pub trees: Vec<(RelPath, RelPath)>,
    /// Each directory it was to remove that the folder still holds, as one a
    /// file it did not know of keeps, where moving a tree moves its files one
    /// by one.
    pub left: Vec<RelPath>,
    /// Whether it is a step of this instance's undo history, as every intent is
    /// but one that only dismisses runs or only joins entities.
    pub undoable: bool,
    /// Whether this instance's own state caught up with the commit.
    pub local: Local,
}

/// Whether what this instance keeps beside the folder holds everything it has
/// committed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Local {
    Current,
    /// Each part that lags, with the failure that left it behind. The next commit,
    /// refresh or close records the head again; a refresh scans again.
    Behind(BTreeMap<Lag, String>),
}

/// A part of this instance's state that can lag a commit whose entries are
/// durable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Lag {
    /// `head.json` names an earlier entry, or a failed write tore it. Reopening
    /// continues from the last entry of the writer's chain in the folder, or
    /// starts a new writer when it is torn.
    Head,
    /// The scan after the commit failed. The entities whose files its effects
    /// touched are [`crate::FileState::Unscanned`] until a scan of every file.
    Scan,
}

/// How settling an interrupted effect will end: all made, or stopped partway.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Complete,
    Partial(PartialReport),
}

/// Where an intent's file effects stopped. Every effect before `applied` was made;
/// the pending record, while there is one, names what recovery will finish.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PartialReport {
    pub applied: usize,
    pub stopped: RelPath,
    pub error: IoError,
    pub record: Option<Nonce>,
    /// The library paths the finished steps took a file from, and the directories
    /// they removed.
    pub taken: Vec<RelPath>,
}

/// What a refresh found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refreshed {
    /// What changed since the last view, by its writer or from outside.
    pub changes: Vec<Change>,
    /// What the files scanned so far hold against the logged facts; empty until
    /// a scan of every file has succeeded.
    pub scan: ScanReport,
    /// The directories the last scan of every file could not list. What is under
    /// them is unknown, as after a failed scan, until a scan lists them.
    pub unlisted: Vec<RelPath>,
    /// Every directory below the root that a scan of every file listed, in order,
    /// where this refresh finished one.
    pub dirs: Option<Vec<RelPath>>,
    /// As [`Opened::orphaned`], as the pending records stand at this refresh: a
    /// writer elsewhere may have begun or finished an effect since.
    pub orphaned: Vec<Orphan>,
    /// This instance's writer, when the read found its history forked: it writes
    /// no more, and the next commit writes as a new writer.
    pub retired: Option<WriterId>,
}

/// One change to what a reader sees.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Change {
    pub entity: EntityId,
    pub what: What,
    pub by: By,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum What {
    Created,
    Deleted,
    Field(String),
    File,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
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
    /// The writer whose trash holds the bytes. Only this instance's writer
    /// restores or empties its own; one of another writer of this install, as
    /// one it wrote as before it retired, stays as it is, since only a writer
    /// writes in its own directory.
    pub writer: WriterId,
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
    /// The writer's pending records that do not read, while which nothing is removed.
    pub held: Vec<RelPath>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Compacted {
    pub snapshot: Nonce,
    pub folded: usize,
    /// Segments that ended with a seal marker, folded and deleted.
    pub removed: Vec<RelPath>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reports an app hands from a worker to its page come back as they were.
    #[test]
    fn reports_an_app_sends_elsewhere_serialize_and_come_back_whole() {
        let writer = WriterId::from_u128(7);
        let path = RelPath::new("Set/a.wav").unwrap();
        let orphan = Orphan {
            writer,
            record: Nonce::from_u128(9),
            label: "Move".into(),
            paths: vec![path],
            ours: false,
            live: true,
            progress: 3,
        };
        let change = Change {
            entity: EntityId::from_u128(4),
            what: What::Field("tags".into()),
            by: By::Writer {
                writer,
                label: "drawbar".into(),
            },
        };
        let json = serde_json::to_string(&(&orphan, &change)).unwrap();
        let back: (Orphan, Change) = serde_json::from_str(&json).unwrap();
        assert_eq!(back, (orphan, change), "{json}");
    }
}
