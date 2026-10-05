//! Folding a writer's own segments into a snapshot.
//!
//! A writer compacts only its own entries. It writes the snapshot under its content
//! name, and only then removes its own older segments and snapshots, so a reader that
//! meets both halves of an interrupted compaction merges them to the same state.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fs::{ensure_dir, hash_file, Fs, RelPath};
use crate::ids::{IntentId, Version, WriterId};
use crate::layout::{Layout, LogFile};
use crate::log::{json_texts, Entry, LogWriter, WriterLog};
use crate::merge::{State, StateFile};
use crate::value::BlobId;

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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFile {
    through: u64,
    state: StateFile,
    #[serde(with = "json_texts")]
    retained: Vec<Entry>,
}

impl Snapshot {
    /// The snapshot file's bytes; their BLAKE3 hash names the file.
    pub fn encode(&self) -> Vec<u8> {
        let file = SnapshotFile {
            through: self.through,
            state: StateFile::from(&self.state),
            retained: self.retained.clone(),
        };
        let mut bytes = serde_json::to_vec(&file).expect("a snapshot serializes");
        bytes.push(b'\n');
        bytes
    }

    /// The snapshot in `bytes`, read from `path`.
    pub fn decode(path: &RelPath, bytes: &[u8]) -> Result<Self> {
        let file: SnapshotFile = serde_json::from_slice(bytes).map_err(|error| Error::Corrupt {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        Ok(Self {
            through: file.through,
            state: file.state.into(),
            retained: file.retained,
        })
    }

    /// Both snapshots of one writer at once, as a reader meets them after a
    /// compaction that stopped before removing the older.
    pub(crate) fn combine(mut self, other: Snapshot) -> Snapshot {
        self.through = self.through.max(other.through);
        self.state.join(&other.state);
        let mut retained: BTreeMap<Version, Entry> = BTreeMap::new();
        for entry in self.retained.into_iter().chain(other.retained) {
            retained.entry(entry.version).or_insert(entry);
        }
        self.retained = retained.into_values().collect();
        self
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
///
/// `own` must hold every entry `log` has appended. A writer that is read-only cannot
/// compact, so entries it does not understand stay in their segments.
pub async fn compact<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    own: &WriterLog,
    window: usize,
) -> Result<Compaction> {
    let writer = log.writer();
    let read_only = |reason: &str| Error::ReadOnly {
        writer,
        reason: reason.to_owned(),
    };
    if let Some(reason) = log.read_only() {
        return Err(read_only(reason));
    }
    if own.writer != writer {
        return Err(Error::InvalidId {
            what: "log of this writer",
            text: own.writer.to_string(),
        });
    }
    if own.has_unknown() {
        return Err(read_only(
            "its log holds entries this build does not understand",
        ));
    }
    if !log.is_covered_by(own) {
        return Err(Error::StaleLog { writer });
    }
    let snapshot = fold(own, window);
    log.close_through(snapshot.through);

    let bytes = snapshot.encode();
    let hash = BlobId::of(&bytes);
    let path = layout.snapshot(writer, hash);
    write_snapshot(fs, layout, writer, &path, &bytes).await?;

    let dir = layout.writer(writer);
    let mut removed = Vec::new();
    for file in fs.list(&dir).await? {
        let superseded = match LogFile::parse(&file.name) {
            Some(LogFile::Segment(number)) => number <= snapshot.through,
            Some(LogFile::Snapshot(other)) => other != hash,
            None => false,
        };
        if superseded {
            let file = dir.join(&file.name)?;
            fs.remove_file(&file).await?;
            removed.push(file);
        }
    }
    if !removed.is_empty() {
        fs.sync(&dir).await?;
    }
    Ok(Compaction {
        snapshot: path,
        removed,
    })
}

fn fold(own: &WriterLog, window: usize) -> Snapshot {
    let mut state = own
        .snapshot
        .as_ref()
        .map(|s| s.state.clone())
        .unwrap_or_default();
    own.entries.iter().for_each(|entry| state.apply(entry));
    state.fold_window();
    let folded = own.snapshot.as_ref().map_or(0, |s| s.through);
    Snapshot {
        through: own.segments.iter().copied().fold(folded, u64::max),
        state,
        retained: undo_window(own, window),
    }
}

/// The entries of the last `window` intents, by the version of each intent's first
/// entry, in version order.
fn undo_window(own: &WriterLog, window: usize) -> Vec<Entry> {
    let entries: BTreeMap<Version, &Entry> = own
        .all_entries()
        .map(|entry| (entry.version, entry))
        .collect();
    let mut seen = BTreeSet::new();
    let mut intents: Vec<IntentId> = entries
        .values()
        .filter(|entry| seen.insert(entry.intent))
        .map(|entry| entry.intent)
        .collect();
    let kept: BTreeSet<IntentId> = intents
        .split_off(intents.len().saturating_sub(window))
        .into_iter()
        .collect();
    entries
        .into_values()
        .filter(|entry| kept.contains(&entry.intent))
        .cloned()
        .collect()
}

/// Write the snapshot under its content name, durably. Through the writer's `tmp/`
/// directory and a rename where the backend renames files, so the name never holds
/// partial bytes.
async fn write_snapshot<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    path: &RelPath,
    bytes: &[u8],
) -> Result<()> {
    let dir = layout.writer(writer);
    if fs.metadata(path).await?.is_some() {
        return match hash_file(fs, path).await? {
            (hash, _) if hash == BlobId::of(bytes) => Ok(()),
            _ => Err(Error::Corrupt {
                path: path.clone(),
                reason: "its contents do not match the hash in its name".to_owned(),
            }),
        };
    }
    ensure_dir(fs, &dir).await?;
    if !fs.capabilities().rename_file {
        fs.create(path, bytes).await?;
        fs.sync(path).await?;
        return fs.sync(&dir).await;
    }
    let tmp_dir = layout.tmp(writer);
    let name = path.name().expect("a snapshot path names a file");
    let tmp = tmp_dir.join(name)?;
    ensure_dir(fs, &tmp_dir).await?;
    if fs.metadata(&tmp).await?.is_some() {
        fs.remove_file(&tmp).await?;
    }
    fs.create(&tmp, bytes).await?;
    fs.sync(&tmp).await?;
    fs.rename(&tmp, path).await?;
    fs.sync(&dir).await?;
    fs.sync(&tmp_dir).await
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

    use super::*;
    use crate::fs::Capabilities;
    use crate::ids::EntityId;
    use crate::log::testing::{facts, field, text, writer, Session};
    use crate::log::{read_log, read_logs, Kind};
    use crate::merge::merge;
    use crate::undo::{plan_undo, History};
    use crate::value::Value;
    use crate::MemFs;

    const A: u128 = 1;
    const B: u128 = 2;

    /// Two writers' histories: creates, a delete, field writes, set adds and
    /// removes on both sides, and a blob.
    fn library(capabilities: Capabilities) -> MemFs {
        let fs = MemFs::with_capabilities(capabilities);
        let mut a = Session::open(&fs, writer(A));
        let e = a.log.new_entity();
        let tags = |value: &str| (e, "tags".to_owned(), text(value));
        a.act(vec![
            Kind::Create { entity: e },
            field(e, "name", Some(text("one"))),
            Kind::SetAdd {
                entity: e,
                name: tags("x").1,
                value: tags("x").2,
            },
        ]);
        let observed = a.state().tags(e, "tags", &text("x"));
        let blob = BlobId::of(b"contents");
        a.act(vec![
            field(e, "name", Some(text("two"))),
            Kind::SetRemove {
                entity: e,
                name: tags("x").1,
                value: tags("x").2,
                observed,
            },
            Kind::BlobAdded { blob, len: 8 },
            field(e, "content", Some(Value::Blob(blob))),
        ]);
        a.act(vec![Kind::Delete { entity: e }]);

        let mut b = Session::open(&fs, writer(B));
        let f = b.log.new_entity();
        b.act(vec![
            field(e, "name", Some(text("three"))),
            Kind::SetAdd {
                entity: e,
                name: tags("y").1,
                value: tags("y").2,
            },
            Kind::Create { entity: f },
        ]);

        let mut a = Session::open(&fs, writer(A));
        let observed = a.state().tags(e, "tags", &text("y"));
        a.act(vec![
            field(f, "note", Some(text("from a"))),
            Kind::SetRemove {
                entity: e,
                name: tags("y").1,
                value: tags("y").2,
                observed,
            },
        ]);
        a.act(vec![]);
        fs
    }

    fn compact_a(fs: &MemFs, window: usize) -> Result<Compaction> {
        let mut a = Session::open(fs, writer(A));
        let own = a.own();
        block_on(compact(fs, &a.layout, &mut a.log, &own, window))
    }

    fn merged(fs: &MemFs) -> State {
        let logs = block_on(read_logs(fs, &Layout::default())).unwrap();
        facts(merge(&logs))
    }

    fn files_of(fs: &MemFs, id: u128) -> BTreeMap<RelPath, Vec<u8>> {
        let dir = Layout::default().writer(writer(id));
        fs.files()
            .into_iter()
            .filter(|(path, _)| path.starts_with(&dir))
            .collect()
    }

    #[test]
    fn compaction_keeps_tombstones_and_remove_observations() {
        let fs = library(Capabilities::ALL);
        let e = EntityId::new(writer(A), 0);
        let before = merged(&fs);
        compact_a(&fs, 0).unwrap();
        let after = merged(&fs);
        assert_eq!(after, before);
        assert!(!after.exists(e));
        assert!(after.members(e, "tags").is_empty());
        assert_eq!(after.field(e, "name"), Some(&text("three")));
        let own = Session::open(&fs, writer(A)).own();
        assert_eq!((own.entries.len(), own.segments.len()), (0, 0));
    }

    #[test]
    fn a_crash_at_any_point_of_compaction_leaves_the_merged_state_unchanged() {
        let without_rename = Capabilities {
            rename_file: false,
            ..Capabilities::ALL
        };
        let without_fsync = Capabilities {
            fsync: false,
            ..Capabilities::ALL
        };
        for capabilities in [
            Capabilities::ALL,
            without_rename,
            without_fsync,
            Capabilities::NONE,
        ] {
            let fs = library(capabilities);
            let expected = merged(&fs);
            let others = files_of(&fs, B);
            let start = fs.mutations();
            compact_a(&fs, 1).unwrap();
            let steps = fs.mutations() - start;
            for step in 0..steps {
                let fs = library(capabilities);
                fs.crash_after(step);
                let crashed = compact_a(&fs, 1);
                assert!(
                    matches!(crashed, Err(Error::Crashed)),
                    "{capabilities:?} step {step}: {crashed:?}"
                );
                let fs = fs.restart();
                assert_eq!(merged(&fs), expected, "{capabilities:?} step {step}");
                compact_a(&fs, 1).unwrap();
                assert_eq!(
                    merged(&fs),
                    expected,
                    "{capabilities:?} step {step}, compacted again"
                );
                assert_eq!(files_of(&fs, B), others, "{capabilities:?} step {step}");
                let own = Session::open(&fs, writer(A)).own();
                assert!(own.segments.is_empty(), "{capabilities:?} step {step}");
                assert_eq!(files_of(&fs, A).len(), 1, "{capabilities:?} step {step}");
            }
        }
    }

    #[test]
    fn a_reader_meeting_both_halves_of_a_compaction_merges_them_to_the_same_state() {
        let fs = library(Capabilities::ALL);
        let expected = merged(&fs);
        let segments = files_of(&fs, A);
        compact_a(&fs, 1).unwrap();
        for (path, bytes) in segments {
            block_on(fs.create(&path, &bytes)).unwrap();
        }
        assert_eq!(merged(&fs), expected);
        let own = Session::open(&fs, writer(A)).own();
        assert_eq!(
            own.entries,
            [],
            "segments the snapshot folded are not read again"
        );
    }

    #[test]
    fn compaction_keeps_the_undo_window_whole() {
        let fs = MemFs::new();
        let mut a = Session::open(&fs, writer(A));
        let e = a.log.new_entity();
        let intents: Vec<_> = ["one", "two", "three"]
            .map(|name| a.act(vec![field(e, "name", Some(text(name)))]))
            .into();
        let own = a.own();
        block_on(compact(&fs, &a.layout, &mut a.log, &own, 2)).unwrap();
        let own = a.own();
        let retained: BTreeSet<_> = own
            .snapshot
            .as_ref()
            .unwrap()
            .retained
            .iter()
            .map(|e| e.intent)
            .collect();
        assert_eq!(retained, intents[1..].iter().copied().collect());
        let plan = plan_undo(&History::new(&own), &a.state()).unwrap();
        assert_eq!(plan.reverses, intents[2]);
    }

    #[test]
    fn ids_and_the_clock_continue_after_compaction() {
        let fs = MemFs::new();
        let mut a = Session::open(&fs, writer(A));
        let e = a.log.new_entity();
        let unused = a.log.new_entity();
        a.act(vec![
            Kind::Create { entity: e },
            field(e, "link", Some(Value::Ref(unused))),
        ]);
        let own = a.own();
        block_on(compact(&fs, &a.layout, &mut a.log, &own, 0)).unwrap();
        let mut reopened = Session::open(&fs, writer(A));
        assert_eq!(reopened.log.new_entity(), EntityId::new(writer(A), 2));
        let intent = reopened.act(vec![]);
        assert_eq!(intent.counter, 1);
        let lamport = reopened.own().entries[0].version.lamport;
        assert_eq!(lamport, 4);
    }

    #[test]
    fn compaction_refuses_a_log_read_before_the_latest_append() {
        let fs = MemFs::new();
        let mut a = Session::open(&fs, writer(A));
        a.act(vec![]);
        let stale = a.own();
        a.act(vec![]);
        let before = fs.files();
        let refused = block_on(compact(&fs, &a.layout, &mut a.log, &stale, 1));
        assert!(
            matches!(refused, Err(Error::StaleLog { .. })),
            "{refused:?}"
        );
        assert_eq!(fs.files(), before);
    }

    #[test]
    fn a_writer_with_entries_it_does_not_understand_keeps_them_uncompacted() {
        let fs = library(Capabilities::ALL);
        let mut a = Session::open(&fs, writer(A));
        let json = format!(
            r#"{{"version":"99@{w}","intent":"{w}:9","kind":"comment","text":"kept"}}"#,
            w = writer(A)
        );
        let line = crate::log::encode_line(&crate::log::parse_entry(&json).unwrap());
        let last = *a.own().segments.last().unwrap();
        block_on(fs.append(&a.layout.segment(writer(A), last), line.as_bytes())).unwrap();
        let before = fs.files();

        a = Session::open(&fs, writer(A));
        let own = a.own();
        let refused = block_on(compact(&fs, &a.layout, &mut a.log, &own, 1));
        assert!(
            matches!(refused, Err(Error::ReadOnly { .. })),
            "{refused:?}"
        );
        assert_eq!(fs.files(), before);
    }

    #[test]
    fn a_snapshot_reads_back_from_its_bytes_with_unknown_entries_verbatim() {
        let fs = library(Capabilities::ALL);
        let mut a = Session::open(&fs, writer(A));
        let own = a.own();
        let mut snapshot = fold(&own, 2);
        let json = format!(
            r#"{{"version":"99@{w}","intent":"{w}:9","kind":"comment"}}"#,
            w = writer(A)
        );
        snapshot
            .retained
            .push(crate::log::parse_entry(&json).unwrap());
        let path = a.layout.snapshot(writer(A), BlobId::of(&snapshot.encode()));
        assert_eq!(
            Snapshot::decode(&path, &snapshot.encode()).unwrap(),
            snapshot
        );
        assert_eq!(
            Snapshot::decode(&path, &snapshot.encode())
                .unwrap()
                .encode(),
            snapshot.encode()
        );

        let written = block_on(compact(&fs, &a.layout, &mut a.log, &own, 2)).unwrap();
        let bytes = fs.files()[&written.snapshot].clone();
        assert_eq!(
            written.snapshot,
            a.layout.snapshot(writer(A), BlobId::of(&bytes))
        );
    }

    #[test]
    fn a_snapshot_whose_bytes_do_not_match_its_name_is_corrupt() {
        let fs = library(Capabilities::ALL);
        let layout = Layout::default();
        let path = layout.snapshot(writer(A), BlobId::of(b"other"));
        block_on(fs.create(&path, b"{}")).unwrap();
        let read = block_on(read_log(&fs, &layout, writer(A)));
        assert!(matches!(read, Err(Error::Corrupt { .. })), "{read:?}");
    }

    #[test]
    fn a_later_compaction_removes_the_earlier_snapshot() {
        let fs = library(Capabilities::ALL);
        let first = compact_a(&fs, 1).unwrap();
        Session::open(&fs, writer(A)).act(vec![]);
        let second = compact_a(&fs, 1).unwrap();
        assert!(second.removed.contains(&first.snapshot), "{second:?}");
        assert_eq!(
            files_of(&fs, A).into_keys().collect::<Vec<_>>(),
            [second.snapshot]
        );
    }
}
