//! Compaction: a writer folds its own entries into a snapshot, then deletes what the
//! snapshot supersedes. Only the owner compacts, and never a writer it no longer
//! writes as.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::error::{Error, Refusal, Result};
use crate::flow::{self, Fallible, Flow};
use crate::ids::{EntryHash, Nonce, WriterId};
use crate::io::{Io, Range, Root, Task};
use crate::line;
use crate::merge::merge;
use crate::path::RelPath;
use crate::reader::{Reader, Stamp, WriterFile, WriterLog};
use crate::report::{Compacted, Rekey};
use crate::schema::Raw;
use crate::snapshot::Snapshot;
use crate::writer::Writer;

/// Confirms the folder holds this writer's head, seals the open segment, and
/// writes `snapshot-<name>.json` folding the chain up to the head, durably (file
/// and directory synced). Only once the folder holds that snapshot does it delete
/// each segment of the writer's directory that ends with a seal marker and every
/// line of which a snapshot in the directory folds, and each snapshot whose
/// folded list starts the new one's. Each is checked again just before it is
/// deleted, and left when it changed. Returns the writer with the result.
///
/// `reader` must have read the writer's directory after its last append. Fails
/// with [`Error::Rekey`] when the reader or the folder does not hold the
/// writer's head, writing nothing, or when the folder loses the snapshot before
/// a deletion. Refused with [`Refusal::Nothing`] when a snapshot the reader
/// found in the writer's directory already folds its head; it then writes
/// nothing, deletes what that snapshot lets it that a compaction cut short
/// left, and confirms its head from that snapshot from then on.
/// Branches of a forked history other than this writer's own are not folded.
pub fn compact(
    writer: Writer,
    reader: &Reader,
    name: Nonce,
) -> Task<'static, (Writer, Result<Compacted>)> {
    let id = writer.id();
    let lost = move || Error::Rekey {
        writer: id,
        why: Rekey::Restored,
    };
    if let Some((path, stamp, folding)) = folding_head(reader, &writer) {
        let doomed = Doomed::of(reader, id, &folding, None).sparing(&path);
        let dir = writer.layout.writer(id);
        let mut writer = writer;
        writer.held_by(path.clone(), stamp.len, stamp.tail.clone());
        return deleted(doomed, path, dir, lost)
            .then(move |deleted| {
                let refused = deleted.and(Err(Error::Refused(Refusal::Nothing)));
                Flow::Done((writer, refused))
            })
            .task();
    }
    let Some(snapshot) = reader.logs().get(&id).and_then(|own| fold(&writer, own)) else {
        return Task::ready((writer, Err(lost())));
    };
    let layout = writer.layout.clone();
    let path = layout.snapshot(id, name);
    let open = writer
        .open_segment()
        .map(|open| layout.segment(id, open.name));
    let doomed = Doomed::of(reader, id, &snapshot, open.as_ref());
    let bytes = snapshot.encode();
    let (len, tail) = (
        bytes.len() as u64,
        bytes[bytes.len() - ending(&bytes)..].to_vec(),
    );
    let folded = snapshot.folded.len();
    let dir = layout.writer(id);
    writer
        .head_held()
        .then(move |held| match held {
            Ok(true) => writer.seal(),
            Ok(false) => Flow::Done((writer, Err(lost()))),
            Err(error) => Flow::Done((writer, Err(error))),
        })
        .then(move |(mut writer, sealed)| {
            let written = match sealed {
                Ok(()) => write_snapshot(path.clone(), dir.clone(), bytes),
                Err(error) => Flow::Done(Err(error)),
            };
            written.then(move |written| {
                match written {
                    Ok(true) => {}
                    Ok(false) => return Flow::Done((writer, Err(lost()))),
                    Err(error) => return Flow::Done((writer, Err(error))),
                }
                writer.held_by(path.clone(), len, tail);
                deleted(doomed, path, dir, lost).then(move |removed| {
                    let compacted = removed.map(|removed| Compacted {
                        snapshot: name,
                        folded,
                        removed,
                    });
                    Flow::Done((writer, compacted))
                })
            })
        })
        .task()
}

/// A snapshot the reader found in the writer's directory that folds its head,
/// with its path and stamp.
fn folding_head(reader: &Reader, writer: &Writer) -> Option<(RelPath, Stamp, Rc<Snapshot>)> {
    let (id, head) = (writer.id(), writer.head());
    reader
        .files(id)
        .into_iter()
        .find_map(|(path, stamp, file)| match (file, stamp) {
            (WriterFile::Snapshot(old), Some(stamp))
                if old.writer == id && old.head() == Some(head) =>
            {
                Some((path.clone(), stamp.clone(), Rc::clone(old)))
            }
            _ => None,
        })
}

/// Deletes the `doomed` files that the snapshot at `path` lets go, then syncs
/// the writer's directory `dir` if any went. Returns the deleted segments.
fn deleted<'a>(
    doomed: Doomed,
    path: RelPath,
    dir: RelPath,
    lost: impl Fn() -> Error + Copy + 'a,
) -> Fallible<'a, Vec<RelPath>> {
    doomed
        .delete(path, lost)
        .and_then(move |(removed, any)| match any {
            false => flow::ok(removed),
            true => flow::sync(Root::Folder, &dir).map_ok(move |()| removed),
        })
}

/// How many of a snapshot's last bytes stand for it.
fn ending(bytes: &[u8]) -> usize {
    bytes.len().min(line::ENDING as usize)
}

/// Writes the snapshot durably; whether the folder then holds it.
fn write_snapshot<'a>(path: RelPath, dir: RelPath, bytes: Vec<u8>) -> Fallible<'a, bool> {
    flow::act(Io::Create {
        root: Root::Folder,
        path: path.clone(),
        bytes,
    })
    .and_then({
        let path = path.clone();
        move |()| flow::sync(Root::Folder, &path).and_then(move |()| flow::sync(Root::Folder, &dir))
    })
    .and_then(move |()| flow::stat(Root::Folder, &path))
    .map_ok(|held| held.is_some())
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
    let entries = own
        .entries()
        .iter()
        .filter(|entry| on_chain.contains(&entry.hash()))
        .cloned()
        .collect();
    chain.place(snapshots, entries);
    let at = chain.last_at()?;
    let mut unknown: BTreeMap<String, Raw> = BTreeMap::new();
    for (name, raw) in chain.snapshots().iter().flat_map(|old| &old.unknown) {
        let kept = unknown.entry(name.clone()).or_insert_with(|| raw.clone());
        if raw > kept {
            *kept = raw.clone();
        }
    }
    Some(Snapshot {
        writer: own.writer(),
        label: own.label().unwrap_or_default().to_owned(),
        at,
        folded,
        state: merge([&chain]),
        unknown,
    })
}

/// The files of the writer's directory a snapshot lets it delete, as the
/// reader last found them.
struct Doomed {
    segments: Vec<Sealed>,
    /// Snapshots whose folded list starts that snapshot's, with their lengths.
    snapshots: Vec<(RelPath, u64)>,
}

/// A segment ending with a seal marker, every line of which a snapshot folds.
struct Sealed {
    path: RelPath,
    /// Its length with the marker.
    len: u64,
    last: EntryHash,
    /// The other snapshot that folds it, with its length; `None` when the one
    /// [`Doomed::of`] was given does.
    folder: Option<(RelPath, u64)>,
}

impl Doomed {
    /// `open` is the segment this process has open, which compaction seals.
    fn of(reader: &Reader, id: WriterId, new: &Snapshot, open: Option<&RelPath>) -> Self {
        let files = reader.files(id);
        let mut covering: Vec<(RelPath, u64, BTreeSet<EntryHash>)> = Vec::new();
        let mut snapshots = Vec::new();
        for (path, stamp, file) in &files {
            let (WriterFile::Snapshot(old), Some(stamp)) = (file, stamp) else {
                continue;
            };
            if old.writer != id {
                continue;
            }
            match new.extends(old) {
                true => snapshots.push(((*path).clone(), stamp.len)),
                false => covering.push((
                    (*path).clone(),
                    stamp.len,
                    old.folded.iter().copied().collect(),
                )),
            }
        }
        let folded: BTreeSet<EntryHash> = new.folded.iter().copied().collect();
        let segments = files
            .iter()
            .filter_map(|(path, _, file)| {
                let WriterFile::Segment(segment) = file else {
                    return None;
                };
                let last = segment.entries.last()?.hash();
                let sealed = segment.sealed || Some(*path) == open;
                if !sealed || segment.stop.is_some() {
                    return None;
                }
                let hashes = || segment.entries.iter().map(|entry| entry.hash());
                let folder = match hashes().all(|hash| folded.contains(&hash)) {
                    true => None,
                    false => {
                        let (path, len, _) = covering
                            .iter()
                            .find(|(_, _, folds)| hashes().all(|hash| folds.contains(&hash)))?;
                        Some((path.clone(), *len))
                    }
                };
                Some(Sealed {
                    path: (*path).clone(),
                    len: segment.end + line::SEAL_MARKER,
                    last,
                    folder,
                })
            })
            .collect();
        Self {
            segments,
            snapshots,
        }
    }

    /// Leaves the snapshot at `path`, which folds what `of` was given.
    fn sparing(mut self, path: &RelPath) -> Self {
        self.snapshots.retain(|(doomed, _)| doomed != path);
        self
    }

    /// Deletes each file once the snapshot at `path` is confirmed to be in
    /// the folder still, and the file to be as the reader found it; a segment
    /// also only while it ends with its seal marker and the snapshot that folds
    /// it is there. Returns the deleted segments, and whether it deleted any file.
    fn delete<'a>(
        self,
        path: RelPath,
        lost: impl Fn() -> Error + Copy + 'a,
    ) -> Fallible<'a, (Vec<RelPath>, bool)> {
        let confirm = move || {
            flow::stat(Root::Folder, &path).and_then(move |held| match held {
                Some(_) => flow::ok(()),
                None => Flow::Done(Err(lost())),
            })
        };
        let Doomed {
            segments,
            snapshots,
        } = self;
        let again = confirm.clone();
        flow::fold(
            segments.into_iter(),
            Vec::new(),
            move |mut removed, sealed| {
                let Sealed {
                    path,
                    len,
                    last,
                    folder,
                } = sealed;
                confirm()
                    .and_then(move |()| match folder {
                        Some((folder, len)) => same_len(folder, len),
                        None => flow::ok(true),
                    })
                    .and_then({
                        let path = path.clone();
                        move |covered| match covered {
                            true => ends_sealed(path, len, last),
                            false => flow::ok(false),
                        }
                    })
                    .and_then(move |sealed| match sealed {
                        true => {
                            flow::remove_if_present(Root::Folder, path.clone()).map_ok(move |()| {
                                removed.push(path);
                                removed
                            })
                        }
                        false => flow::ok(removed),
                    })
            },
        )
        .and_then(move |removed| {
            let any = !removed.is_empty();
            flow::fold(
                snapshots.into_iter(),
                (removed, any),
                move |(removed, any), (path, len)| {
                    again()
                        .and_then({
                            let path = path.clone();
                            move |()| same_len(path, len)
                        })
                        .and_then(move |same| match same {
                            true => flow::remove_if_present(Root::Folder, path)
                                .map_ok(|()| (removed, true)),
                            false => flow::ok((removed, any)),
                        })
                },
            )
        })
    }
}

/// Whether the file at `path` is `len` bytes long.
fn same_len<'a>(path: RelPath, len: u64) -> Fallible<'a, bool> {
    flow::stat(Root::Folder, &path).map_ok(move |meta| meta.is_some_and(|meta| meta.len == len))
}

/// Whether the segment at `path` is `len` bytes long and ends with the seal
/// marker after the line `last` names.
fn ends_sealed<'a>(path: RelPath, len: u64, last: EntryHash) -> Fallible<'a, bool> {
    same_len(path.clone(), len).and_then(move |same| {
        if !same {
            return flow::ok(false);
        }
        let range = Range {
            offset: len - line::SEAL_MARKER,
            len: line::SEAL_MARKER,
        };
        flow::read_present(Root::Folder, &path, range)
            .map_ok(move |bytes| bytes == Some(line::seal_marker(last)))
    })
}
