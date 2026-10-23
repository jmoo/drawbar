//! Compaction: a writer folds its own entries into a snapshot, then deletes what the
//! snapshot certifies. Only the owner compacts, and never a writer it no longer
//! writes as.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::error::{Error, Refusal, Result};
use crate::flow::{self, Fallible, Flow};
use crate::ids::{EntryHash, Nonce, WriterId};
use crate::io::{Capabilities, Io, Range, Root, Task};
use crate::line;
use crate::log::{Entry, EntryKind, Own};
use crate::path::RelPath;
use crate::reader::{Reader, Stamp, WriterFile};
use crate::report::{Compacted, Rekey};
use crate::snapshot::{SegmentCert, Snapshot};
use crate::writer::Writer;

/// Confirms the folder holds this writer's head, seals the open segment, and
/// writes `snapshot-<name>.json` folding the chain up to the head into its
/// previous snapshot, durably: staged in the writer's `tmp/` and renamed into
/// place where `capabilities`, the folder's, rename files, else in place, where
/// a crash can tear it. Only once the folder holds that snapshot does it delete
/// each segment of the writer's directory that ends with a seal marker and
/// every line of which the snapshot certifies, and each snapshot whose head it
/// certifies. Each is checked again just before it is deleted, and left when it
/// changed. Returns the writer with the result.
///
/// `reader` must have read the writer's directory after its last append.
/// `recorded` holds the lines of segments of the directory the writer read
/// before, which may be gone now: each is certified as a segment too. Fails
/// with [`Error::Rekey`] when the reader or the folder does not hold the
/// writer's head, or the reader finds the writer's history forked, writing
/// nothing, or when the folder loses the snapshot before a deletion. Refused with [`Refusal::Nothing`] when a snapshot the reader
/// found in the writer's directory already has its head; it then writes
/// nothing, deletes what that snapshot lets it that a compaction cut short
/// left, and confirms its head from that snapshot from then on.
pub fn compact(
    writer: Writer,
    reader: &Reader,
    recorded: &[Vec<Rc<Entry>>],
    name: Nonce,
    capabilities: Capabilities,
) -> Task<'static, (Writer, Result<Compacted>)> {
    let id = writer.id();
    let lost = move || Error::Rekey {
        writer: id,
        why: Rekey::Restored,
    };
    let forked = reader
        .derived()
        .writers
        .get(&id)
        .and_then(|placed| placed.fork);
    if forked.is_some() {
        let forked = Error::Rekey {
            writer: id,
            why: Rekey::Forked,
        };
        return Task::ready((writer, Err(forked)));
    }
    if let Some((path, stamp, tip)) = at_head(reader, &writer) {
        let doomed = Doomed::of(reader, id, &tip, None).sparing(&path);
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
    let Some((mut snapshot, folded)) = fold(&writer, reader, recorded) else {
        return Task::ready((writer, Err(lost())));
    };
    let layout = writer.layout.clone();
    let path = layout.snapshot(id, name);
    let staged = capabilities
        .rename_file
        .then(|| (layout.tmp_dir(id), layout.staged(id, name, &path)));
    let open = writer
        .open_segment()
        .map(|open| layout.segment(id, open.name));
    let doomed = Doomed::of(reader, id, &snapshot, open.as_ref());
    let dir = layout.writer(id);
    flow::list(Root::Folder, &layout.trash_dir(id))
        .then(move |listed| {
            let listed = match listed {
                Ok(listed) => listed,
                Err(error) => return Flow::Done((writer, Err(error))),
            };
            let held: BTreeSet<String> = listed.into_iter().map(|entry| entry.name).collect();
            snapshot
                .state
                .trash
                .retain(|item, _| held.contains(&item.to_string()));
            writer.head_held().then(move |held| match held {
                Ok(true) => writer
                    .seal()
                    .then(move |(writer, sealed)| Flow::Done((writer, sealed.map(|()| snapshot)))),
                Ok(false) => Flow::Done((writer, Err(lost()))),
                Err(error) => Flow::Done((writer, Err(error))),
            })
        })
        .then(move |(mut writer, sealed): (Writer, Result<Snapshot>)| {
            let bytes = sealed.as_ref().map(Snapshot::encode).unwrap_or_default();
            let (len, tail) = (
                bytes.len() as u64,
                bytes[bytes.len() - ending(&bytes)..].to_vec(),
            );
            let written = match sealed {
                Ok(_) => write_snapshot(path.clone(), dir.clone(), staged, bytes),
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

/// A snapshot the reader found in the writer's directory whose head is the
/// writer's, with its path and stamp.
fn at_head(reader: &Reader, writer: &Writer) -> Option<(RelPath, Stamp, Rc<Snapshot>)> {
    let (id, head) = (writer.id(), writer.head());
    reader
        .files(id)
        .into_iter()
        .find_map(|(path, stamp, file)| match (file, stamp) {
            (WriterFile::Snapshot(old), Some(stamp)) if old.writer == id && old.head == head => {
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

/// Writes the snapshot durably at `path` in the writer's directory `dir`, and
/// says whether the folder then holds it. With `staged`, the writer's `tmp/` and
/// a path in it, the bytes are written there and renamed into place; without,
/// they are written in place, where a crash can tear them.
fn write_snapshot<'a>(
    path: RelPath,
    dir: RelPath,
    staged: Option<(RelPath, RelPath)>,
    bytes: Vec<u8>,
) -> Fallible<'a, bool> {
    let written = match staged {
        Some((tmp, staged)) => flow::ensure_dir(Root::Folder, &tmp)
            .and_then({
                let staged = staged.clone();
                move |()| synced(staged, bytes)
            })
            .and_then({
                let path = path.clone();
                move |()| flow::rename(Root::Folder, &staged, &path)
            }),
        None => synced(path.clone(), bytes).and_then(move |()| flow::sync(Root::Folder, &dir)),
    };
    written
        .and_then(move |()| flow::stat(Root::Folder, &path))
        .map_ok(|held| held.is_some())
}

/// Creates `path` holding `bytes`, and syncs it.
fn synced<'a>(path: RelPath, bytes: Vec<u8>) -> Fallible<'a, ()> {
    flow::act(Io::Create {
        root: Root::Folder,
        path: path.clone(),
        bytes,
    })
    .and_then(move |()| flow::sync(Root::Folder, &path))
}

/// The snapshot of the chain to the writer's head: its previous snapshot, the
/// tip the head descends from, with the entries since folded in; and how many
/// entries that is. `None` when the reader does not hold the chain from its
/// genesis entry or a tip.
fn fold(
    writer: &Writer,
    reader: &Reader,
    recorded: &[Vec<Rc<Entry>>],
) -> Option<(Snapshot, usize)> {
    let id = writer.id();
    let placed = reader.derived().writers.get(&id)?;
    let known: BTreeMap<EntryHash, &Rc<Entry>> = placed
        .merged
        .iter()
        .chain(&placed.runs)
        .chain(&placed.gaps)
        .map(|entry| (entry.hash(), entry))
        .collect();
    let mut since: Vec<Rc<Entry>> = Vec::new();
    let mut at = writer.head();
    let previous = loop {
        if let Some(tip) = placed.tips.iter().find(|tip| tip.head == at) {
            break Some(tip);
        }
        let entry = known.get(&at.hash).filter(|entry| entry.own() == at)?;
        since.push(Rc::clone(entry));
        if at.n == 0 {
            if at.hash != writer.genesis() || entry.prev() != EntryHash::ZERO {
                return None;
            }
            break None;
        }
        at = Own {
            n: at.n - 1,
            hash: entry.prev(),
        };
    };
    since.reverse();
    let label = match since.first().map(|entry| entry.kind()) {
        Some(EntryKind::Genesis(genesis)) => genesis.label,
        _ => String::new(),
    };
    let found = reader
        .files(id)
        .into_iter()
        .filter_map(|(_, _, file)| match file {
            WriterFile::Segment(segment) => Some(Found::Lines(&segment.entries)),
            WriterFile::Snapshot(snapshot) => Some(Found::Snapshot(snapshot)),
            WriterFile::Unreadable => None,
        });
    let found = found.chain(recorded.iter().map(|lines| Found::Lines(lines)));
    let previous = previous.map(Rc::as_ref);
    let snapshot = snapshot(id, writer.genesis(), label, previous, &since, found)?;
    Some((snapshot, since.len()))
}

/// A file of a writer's directory as a compaction finds it.
pub enum Found<'a> {
    Lines(&'a [Rc<Entry>]),
    Snapshot(&'a Snapshot),
}

/// The snapshot of `writer` that folds `since`, its entries after `previous`'s
/// head in chain order, into `previous`, labeled `label` when there is no
/// previous one. It certifies each segment `found` shows by its first line and
/// the last line on the chain, the heads of the snapshots on the chain with
/// their certificates, and by hash every other entry of `since` and the last
/// entry of each dismissal of its own runs on the chain. `None` when it folds
/// nothing.
pub fn snapshot<'a>(
    writer: WriterId,
    genesis: EntryHash,
    label: String,
    previous: Option<&Snapshot>,
    since: &[Rc<Entry>],
    found: impl IntoIterator<Item = Found<'a>>,
) -> Option<Snapshot> {
    let head = since
        .last()
        .map(|entry| entry.own())
        .or(previous.map(|tip| tip.head))?;
    let mut state = previous.map(|tip| tip.state.clone()).unwrap_or_default();
    for entry in since {
        state.fold(writer, entry);
    }
    let mut certified = previous
        .map(|tip| tip.certified.clone())
        .unwrap_or_default();
    certified.heads.extend(previous.map(|tip| tip.head));
    let added: BTreeSet<EntryHash> = since.iter().map(|entry| entry.hash()).collect();
    let on_chain =
        |own: Own| added.contains(&own.hash) || previous.is_some_and(|tip| tip.names(own));
    let mut covered: BTreeSet<EntryHash> = BTreeSet::new();
    for file in found {
        match file {
            Found::Lines(lines) => {
                let on = certified_prefix(lines, previous, &added);
                if let (Some(first), Some(last)) =
                    (lines.first(), on.checked_sub(1).map(|at| &lines[at]))
                {
                    certified.segments.insert(SegmentCert {
                        first: first.own(),
                        last: last.own(),
                    });
                    covered.extend(lines[..on].iter().map(|entry| entry.hash()));
                }
            }
            Found::Snapshot(old) if old.writer == writer && on_chain(old.head) => {
                certified.heads.insert(old.head);
                let theirs = &old.certified;
                certified.segments.extend(theirs.segments.iter().copied());
                certified.heads.extend(theirs.heads.iter().copied());
                certified.hashes.extend(theirs.hashes.iter().copied());
            }
            Found::Snapshot(_) => {}
        }
    }
    let uncovered = since
        .iter()
        .map(|entry| entry.own())
        .filter(|own| !covered.contains(&own.hash));
    certified.hashes.extend(uncovered);
    // A reader matches a dismissal of this writer's own run to its chain by the
    // hash of the run's last entry.
    for (_, run) in &state.dismissals {
        if run.writer == writer && (covered.contains(&run.last.hash) || on_chain(run.last)) {
            certified.hashes.insert(run.last);
        }
    }
    certified.heads.remove(&head);
    let at = since
        .last()
        .map(|entry| entry.at())
        .or(previous.map(|tip| tip.at))?;
    Some(Snapshot {
        writer,
        label: previous.map_or(label, |tip| tip.label.clone()),
        at,
        genesis,
        head,
        certified,
        state,
        unknown: previous.map(|tip| tip.unknown.clone()).unwrap_or_default(),
    })
}

/// How many of a segment's first lines `snapshot`, or `added`, certifies: those
/// a segment certificate of it covers, and those after them it names.
fn certified_prefix(
    lines: &[Rc<Entry>],
    snapshot: Option<&Snapshot>,
    added: &BTreeSet<EntryHash>,
) -> usize {
    let Some(first) = lines.first() else {
        return 0;
    };
    let matched = snapshot
        .into_iter()
        .flat_map(|snapshot| &snapshot.certified.segments);
    let mut on = matched
        .filter(|cert| cert.first.hash == first.hash())
        .filter_map(|cert| {
            let at = usize::try_from(cert.last.n.checked_sub(cert.first.n)?).ok()?;
            (lines.get(at)?.hash() == cert.last.hash).then_some(at + 1)
        })
        .max()
        .unwrap_or(0);
    while let Some(line) = lines.get(on) {
        let named = snapshot.is_some_and(|snapshot| snapshot.names(line.own()));
        if !named && !added.contains(&line.hash()) {
            break;
        }
        on += 1;
    }
    on
}

/// The files of the writer's directory a snapshot lets it delete, as the
/// reader last found them.
struct Doomed {
    segments: Vec<Sealed>,
    /// Snapshots whose heads that snapshot certifies, with their lengths.
    snapshots: Vec<(RelPath, u64)>,
}

/// A segment ending with a seal marker, every line of which the snapshot
/// certifies.
struct Sealed {
    path: RelPath,
    /// Its length with the marker.
    len: u64,
    last: EntryHash,
}

impl Doomed {
    /// `open` is the segment this process has open, which compaction seals.
    fn of(reader: &Reader, id: WriterId, new: &Snapshot, open: Option<&RelPath>) -> Self {
        let mut segments = Vec::new();
        let mut snapshots = Vec::new();
        for (path, stamp, file) in reader.files(id) {
            match file {
                WriterFile::Snapshot(old) => {
                    if let Some(stamp) = stamp
                        .filter(|_| old.writer == id && old.head != new.head && new.names(old.head))
                    {
                        snapshots.push((path.clone(), stamp.len));
                    }
                }
                WriterFile::Segment(segment) => {
                    let Some(last) = segment.entries.last() else {
                        continue;
                    };
                    let sealed = segment.sealed || Some(path) == open;
                    let whole = certified_prefix(&segment.entries, Some(new), &BTreeSet::new())
                        == segment.entries.len();
                    if sealed && whole && segment.stop.is_none() {
                        segments.push(Sealed {
                            path: path.clone(),
                            len: segment.end + line::SEAL_MARKER,
                            last: last.hash(),
                        });
                    }
                }
                WriterFile::Unreadable => {}
            }
        }
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
    /// also only while it ends with its seal marker. Returns the deleted segments, and whether it deleted any file.
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
                let Sealed { path, len, last } = sealed;
                confirm()
                    .and_then({
                        let path = path.clone();
                        move |()| ends_sealed(path, len, last)
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
