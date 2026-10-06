//! Compaction: a writer folds its own entries into a snapshot, then deletes what the
//! snapshot supersedes. Only the owner compacts, and never a writer it no longer
//! writes as.

use std::collections::BTreeSet;

use crate::error::{Error, Result};
use crate::flow::{self, Flow};
use crate::ids::{EntryHash, Nonce, SegmentName};
use crate::io::{Io, Kind, Root, Task};
use crate::merge::merge;
use crate::path::RelPath;
use crate::reader::{WriterFile, WriterLog, MAX_FILE};
use crate::report::{Compacted, Rekey};
use crate::snapshot::Snapshot;
use crate::writer::Writer;

/// Seals the open segment and writes `snapshot-<name>.json` folding this writer's
/// chain up to its head, durably (file and directory synced). Only once the folder
/// holds that snapshot does it delete the segments in [`Writer::sealed`] whose
/// every line it folds, and every snapshot in the writer's directory whose folded
/// list starts its own.
///
/// `own` is this writer's log, read after its last append. Fails with
/// [`Error::Rekey`] when `own` does not hold the writer's head, or the folder
/// loses the snapshot before anything is deleted. Branches of a forked history
/// other than this writer's own are not folded.
pub fn compact<'a>(
    writer: &'a mut Writer,
    own: &'a WriterLog,
    name: Nonce,
) -> Task<'a, Result<Compacted>> {
    let lost = Error::Rekey {
        writer: writer.id(),
        why: Rekey::Restored,
    };
    let Some(snapshot) = fold(writer, own) else {
        return Task::ready(Err(lost));
    };
    writer.seal();
    let layout = writer.layout.clone();
    let dir = layout.writer(writer.id());
    let path = layout.snapshot(writer.id(), name);
    let folded = snapshot.folded.len();
    let sealed: BTreeSet<RelPath> = writer
        .sealed()
        .iter()
        .map(|segment| layout.segment(writer.id(), *segment))
        .collect();
    flow::act(Io::Create {
        root: Root::Folder,
        path: path.clone(),
        bytes: snapshot.encode(),
    })
    .and_then({
        let (path, dir) = (path.clone(), dir.clone());
        move |()| sync(path).and_then(move |()| sync(dir))
    })
    .and_then({
        let path = path.clone();
        move |()| flow::stat(Root::Folder, &path)
    })
    .and_then(move |held| match held {
        None => Flow::Done(Err(lost)),
        Some(_) => superseded(dir.clone(), path, snapshot, sealed).and_then(move |removed| {
            let removed_count = removed.len();
            remove_all(removed.clone())
                .and_then(move |()| match removed_count {
                    0 => flow::ok(()),
                    _ => sync(dir),
                })
                .map_ok(move |()| removed)
        }),
    })
    .map_ok(move |removed| {
        let segments: Vec<SegmentName> = writer
            .sealed()
            .iter()
            .copied()
            .filter(|segment| removed.contains(&layout.segment(writer.id(), *segment)))
            .collect();
        writer.sealed.retain(|segment| !segments.contains(segment));
        Compacted {
            snapshot: name,
            folded,
            removed: segments,
        }
    })
    .task()
}

/// The snapshot of the chain from the genesis entry to the writer's head, with
/// the state of exactly those entries.
fn fold(writer: &Writer, own: &WriterLog) -> Option<Snapshot> {
    let folded = own.chain_to(writer.head())?;
    let on_chain: BTreeSet<EntryHash> = folded.iter().copied().collect();
    let mut chain = WriterLog::new(own.writer());
    let snapshots = own
        .snapshots()
        .iter()
        .filter(|snapshot| snapshot.head().is_some_and(|head| on_chain.contains(&head)))
        .cloned()
        .collect();
    let lines = own
        .entries()
        .iter()
        .filter(|entry| on_chain.contains(&entry.hash()))
        .map(|entry| entry.line.clone())
        .collect();
    chain.place(snapshots, lines);
    let at = chain.last_at()?;
    Some(Snapshot {
        writer: own.writer(),
        label: own.label().unwrap_or_default().to_owned(),
        at,
        folded,
        state: merge([&chain]),
        unknown: Default::default(),
    })
}

/// The files of `dir` the new snapshot at `path` supersedes: sealed segments every
/// line of which it folds, and snapshots whose folded list starts its own.
fn superseded<'a>(
    dir: RelPath,
    path: RelPath,
    snapshot: Snapshot,
    sealed: BTreeSet<RelPath>,
) -> Flow<'a, Result<Vec<RelPath>>> {
    let folded: BTreeSet<EntryHash> = snapshot.folded.iter().copied().collect();
    flow::list(Root::Folder, &dir).and_then(move |entries| {
        let candidates: Vec<RelPath> = entries
            .into_iter()
            .filter(|entry| entry.kind == Kind::File)
            .filter_map(|entry| dir.join(&entry.name).ok())
            .filter(|file| *file != path)
            .filter(|file| {
                sealed.contains(file) || file.name().is_some_and(|n| n.starts_with("snapshot-"))
            })
            .collect();
        let judge =
            std::rc::Rc::new(
                move |file: &RelPath, bytes: &[u8]| match WriterFile::parse(bytes) {
                    WriterFile::Segment { lines, stop: None } if sealed.contains(file) => {
                        lines.iter().all(|line| folded.contains(&line.hash()))
                    }
                    WriterFile::Snapshot(old) => {
                        old.writer == snapshot.writer && snapshot.extends(&old)
                    }
                    WriterFile::Segment { .. } | WriterFile::Unreadable => false,
                },
            );
        flow::fold(candidates.into_iter(), Vec::new(), move |mut removed, file| {
            let judge = std::rc::Rc::clone(&judge);
            flow::read_file(Root::Folder, &file.clone(), MAX_FILE).map_ok(move |bytes| {
                if bytes.is_some_and(|bytes| judge(&file, &bytes)) {
                    removed.push(file);
                }
                removed
            })
        })
    })
}

fn remove_all<'a>(paths: Vec<RelPath>) -> Flow<'a, Result<()>> {
    flow::fold(paths.into_iter(), (), |(), path| {
        flow::remove_if_present(Root::Folder, path)
    })
}

fn sync<'a>(path: RelPath) -> Flow<'a, Result<()>> {
    flow::act(Io::Sync {
        root: Root::Folder,
        path,
    })
}
