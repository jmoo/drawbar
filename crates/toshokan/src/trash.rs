//! A writer's trash: bytes its intents displaced, kept for undo until it empties
//! them. Emptying a trash is the only way bytes leave the folder, and only its
//! owner empties it, never of what a pending record of its may still move.

use std::collections::{BTreeMap, BTreeSet};

use crate::effects::EffectStep;
use crate::error::Result;
use crate::flow::{self, each};
use crate::ids::{Nonce, WriterId};
use crate::io::{Kind, Root, Task};
use crate::layout::Layout;
use crate::path::RelPath;
use crate::pending::{self, PendingRecord};
use crate::report::{Emptied, TrashItem};

/// The items of `logged`, what `writer`'s entries say they displaced, still in
/// its trash, oldest first, each with its length there. Files in the trash no
/// entry displaced are not listed, and so never emptied. Reads only, in one
/// request.
pub fn list(
    layout: &Layout,
    writer: WriterId,
    logged: Vec<TrashItem>,
) -> Task<'static, Result<Vec<TrashItem>>> {
    flow::list_stat(Root::Folder, &layout.trash_dir(writer))
        .map_ok(move |entries| {
            let mut logged: BTreeMap<String, TrashItem> = logged
                .into_iter()
                .map(|item| (item.item.to_string(), item))
                .collect();
            let mut items: Vec<TrashItem> = entries
                .into_iter()
                .filter(|(_, meta)| meta.kind == Kind::File)
                .filter_map(|(name, meta)| {
                    let item = logged.remove(&name)?;
                    Some(TrashItem {
                        len: meta.len,
                        ..item
                    })
                })
                .collect();
            items.sort_by_key(|item| (item.at, item.item));
            items
        })
        .task()
}

/// Removes from `writer`'s trash every item of `items` but those of `kept` and those
/// a pending record of the writer names, which settling it may still move, then syncs
/// the trash once. While a record of the writer does not read, removes none, and
/// [`Emptied::held`] names it.
pub fn empty(
    layout: &Layout,
    writer: WriterId,
    items: Vec<TrashItem>,
    kept: BTreeSet<Nonce>,
) -> Task<'static, Result<Emptied>> {
    let layout = layout.clone();
    flow::run(pending::read_all(&layout, writer))
        .and_then(move |found| {
            if !found.unreadable.is_empty() {
                return flow::ok(Emptied {
                    held: found.unreadable,
                    ..Emptied::default()
                });
            }
            let named = named(&found.records);
            let removed = items
                .into_iter()
                .filter(|item| !kept.contains(&item.item) && !named.contains(&item.item));
            flow::run(remove(&layout, writer, removed.collect()))
        })
        .task()
}

/// The trash items `records` name: those their steps move into the trash, out of
/// it, or stage there.
fn named(records: &[(Nonce, PendingRecord)]) -> BTreeSet<Nonce> {
    let steps = records.iter().flat_map(|(_, record)| &record.steps);
    let items = steps.filter_map(|step| match step {
        EffectStep::ToTrash { item, .. } | EffectStep::FromTrash { item, .. } => Some(*item),
        EffectStep::Place { staged, .. } => Some(*staged),
        EffectStep::Rename { .. } | EffectStep::MakeDir { .. } | EffectStep::RemoveDir { .. } => {
            None
        }
    });
    items.collect()
}

/// Removes `removed` from `writer`'s trash, then syncs the trash once.
fn remove(
    layout: &Layout,
    writer: WriterId,
    removed: Vec<TrashItem>,
) -> Task<'static, Result<Emptied>> {
    let emptied = Emptied {
        removed: removed.iter().map(|item| item.item).collect(),
        bytes: removed
            .iter()
            .map(|item| item.len)
            .fold(0, u64::saturating_add),
        held: Vec::new(),
    };
    if removed.is_empty() {
        return Task::ready(Ok(emptied));
    }
    let paths: Vec<RelPath> = removed
        .iter()
        .map(|item| layout.trash(writer, item.item))
        .collect();
    let dir = layout.trash_dir(writer);
    each(paths.into_iter(), |path| {
        flow::remove_if_present(Root::Folder, path)
    })
    .and_then(move |()| flow::sync(Root::Folder, &dir))
    .map_ok(move |()| emptied)
    .task()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Moves;
    use crate::line::Line;

    fn nonce(n: u128) -> Nonce {
        Nonce::from_u128(n)
    }

    fn record(steps: Vec<EffectStep>) -> (Nonce, PendingRecord) {
        let record = PendingRecord {
            writer: WriterId::from_u128(1),
            entry: Line::seal(format!(r#"{{"prev":"{}"}}"#, crate::ids::EntryHash::ZERO)).unwrap(),
            label: "Save".into(),
            steps,
            files: Vec::new(),
            unbound: Vec::new(),
            trees: Vec::new(),
            dirs: Vec::new(),
            moves: Moves::Rename,
        };
        (nonce(9), record)
    }

    #[test]
    fn emptying_keeps_what_the_app_keeps_and_what_a_pending_record_names() {
        use crate::disk::MemDisk;
        use crate::ids::{EntryHash, Hlc};
        use crate::io::Io;

        let layout = Layout::new(".t").unwrap();
        let writer = WriterId::from_u128(1);
        let disk = MemDisk::new();
        let put = |path: RelPath, bytes: Vec<u8>| {
            let dir = path.parent().unwrap();
            disk.perform(Io::MakeDir {
                root: Root::Folder,
                path: dir,
            })
            .unwrap();
            disk.perform(Io::Create {
                root: Root::Folder,
                path,
                bytes,
            })
            .unwrap();
        };
        let items: Vec<TrashItem> = (1..=4)
            .map(|n| TrashItem {
                writer,
                item: nonce(n),
                len: 1,
                from: RelPath::new("f").unwrap(),
                at: Hlc::default(),
                by: EntryHash::from_u128(n),
            })
            .collect();
        for item in &items {
            put(layout.trash(writer, item.item), b"x".to_vec());
        }
        let (name, unsettled) = record(vec![EffectStep::FromTrash {
            item: nonce(2),
            path: RelPath::new("f").unwrap(),
        }]);
        put(layout.pending(writer, name), unsettled.encode());

        let mut disk = disk;
        let emptied = empty(&layout, writer, items, [nonce(3)].into());
        let emptied = crate::blocking::run(&mut disk, emptied).unwrap();
        assert_eq!(emptied.removed, [nonce(1), nonce(4)]);
    }

    #[test]
    fn emptying_reads_the_writers_records_first_and_removes_none_while_one_does_not_read() {
        use crate::disk::MemDisk;
        use crate::ids::{EntryHash, Hlc};
        use crate::io::{Io, Operation, Step};

        let layout = Layout::new(".t").unwrap();
        let writer = WriterId::from_u128(1);
        let disk = MemDisk::new();
        let put = |path: RelPath, bytes: &[u8]| {
            let dir = path.parent().unwrap();
            disk.perform(Io::MakeDir {
                root: Root::Folder,
                path: dir,
            })
            .unwrap();
            let _ = disk.perform(Io::Remove {
                root: Root::Folder,
                path: path.clone(),
            });
            disk.perform(Io::Create {
                root: Root::Folder,
                path,
                bytes: bytes.to_vec(),
            })
            .unwrap();
        };
        let item = TrashItem {
            writer,
            item: nonce(1),
            len: 1,
            from: RelPath::new("f").unwrap(),
            at: Hlc::default(),
            by: EntryHash::from_u128(1),
        };
        put(layout.trash(writer, item.item), b"x");
        let at = layout.pending(writer, nonce(9));
        put(at.clone(), b"not a record");
        let mut asked = Vec::new();
        let mut emptying = empty(&layout, writer, vec![item.clone()], BTreeSet::new());
        let mut result = None;
        let emptied = loop {
            match emptying.resume(result.take()) {
                Step::Io(io) => {
                    asked.push(io.clone());
                    result = Some(disk.perform(io));
                }
                Step::Done(emptied) => break emptied.unwrap(),
            }
        };
        assert_eq!(
            emptied,
            Emptied {
                held: vec![at.clone()],
                ..Emptied::default()
            },
            "a record that does not read keeps all"
        );
        let pending = Io::List {
            root: Root::Folder,
            dir: layout.pending_dir(writer),
        };
        assert_eq!(asked.first(), Some(&pending), "{asked:?}");
        assert!(!asked.iter().any(Io::mutates), "{asked:?}");

        let (_, readable) = record(vec![]);
        put(at, &readable.encode());
        put(
            layout.pending_dir(writer).join(".DS_Store").unwrap(),
            b"Bud1",
        );
        let mut disk = disk;
        let emptied = crate::blocking::run(
            &mut disk,
            empty(&layout, writer, vec![item], BTreeSet::new()),
        );
        assert_eq!(
            emptied.unwrap().removed,
            [nonce(1)],
            "what is not named as a record holds nothing"
        );
    }

    #[test]
    fn a_record_names_what_it_moves_into_or_out_of_the_trash_or_stages_there() {
        let at = RelPath::new("f").unwrap();
        let records = [record(vec![
            EffectStep::ToTrash {
                path: at.clone(),
                item: nonce(1),
            },
            EffectStep::FromTrash {
                item: nonce(2),
                path: at.clone(),
            },
            EffectStep::Place {
                staged: nonce(3),
                path: at.clone(),
            },
            EffectStep::Rename {
                from: at.clone(),
                to: RelPath::new("g").unwrap(),
            },
        ])];
        assert_eq!(named(&records), [1, 2, 3].map(nonce).into());
    }
}
