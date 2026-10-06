//! A writer's segments, its pool, its genesis and its rekeying, and compaction.

mod common;

use std::collections::BTreeMap;

use common::{files_under, fresh, held, layout, machine, Instance};
use toshokan::blocking::run;
use toshokan::disk::Tail;
use toshokan::reader::{CachedView, Reader};
use toshokan::report::{Rekey, Start};
use toshokan::simulator::Machine;
use toshokan::writer::{Claimed, Writer};
use toshokan::{Error, Hlc, MemDisk, Root, SegmentName, WriterId};

fn segments(machine: &Machine, writer: WriterId) -> usize {
    files_under(&machine.folder, &layout().writer(writer))
        .iter()
        .filter(|path| path.as_str().ends_with(".jsonl"))
        .count()
}

#[test]
fn a_writer_appends_to_one_new_segment_per_process() {
    let mut first = Instance::open(machine(), 1);
    assert_eq!(first.start, Start::New);
    let mut written = first.write("a").unwrap();
    written.extend(first.write("b").unwrap());
    let id = first.id();
    let machine = first.close();
    assert_eq!(segments(&machine, id), 1);

    let mut second = Instance::open(machine, 2);
    assert_eq!(second.start, Start::Resumed(id));
    written.extend(second.write("c").unwrap());
    let machine = second.close();
    assert_eq!(segments(&machine, id), 2);

    let view = fresh(&machine.folder);
    let log = &view.writers()[&id];
    assert_eq!(log.heads(), [*written.last().unwrap()]);
    assert_eq!(log.chain_to(*written.last().unwrap()), Some(written));
    assert_eq!(log.forks(), []);
}

#[test]
fn opening_writes_nothing_in_the_folder() {
    let mut first = Instance::open(machine(), 1);
    first.write("a").unwrap();
    let machine = first.close();
    let before = machine.folder.mutations();
    let second = Instance::open(machine, 2);
    assert_eq!(second.machine.folder.mutations(), before);
}

#[test]
fn an_instance_takes_a_writer_no_other_process_holds() {
    let mut first = Instance::open(machine(), 1);
    first.write("a").unwrap();
    let mut other = first.machine.clone();
    other.local = other.local.process();
    let mut second = Instance::open(other, 2);
    assert_eq!(second.start, Start::New);
    second.write("b").unwrap();
    assert_ne!(first.id(), second.id());
    let ids = [first.id(), second.id()];
    let (first, second) = (first.close(), second.close());

    let mut third = Instance::open(first, 3);
    let mut again = second;
    again.local = again.local.process();
    let mut fourth = Instance::open(again, 4);
    let mut resumed = [third.start.clone(), fourth.start.clone()];
    resumed.sort_by_key(|start| format!("{start:?}"));
    let mut expected = ids.map(Start::Resumed);
    expected.sort_by_key(|start| format!("{start:?}"));
    assert_eq!(resumed, expected);
    third.write("c").unwrap();
    fourth.write("d").unwrap();
}

fn create(disk: &mut MemDisk) -> toshokan::Result<Writer> {
    let create = Writer::create(
        layout(),
        WriterId::from_u128(7),
        SegmentName::from_u128(8),
        "w".into(),
        Hlc::ZERO,
    );
    run(disk, create)
}

#[test]
fn no_writer_joins_the_pool_before_its_genesis_entry_is_durable() {
    let mut disk = MemDisk::new();
    create(&mut disk).unwrap();
    let operations = disk.mutations();
    for after in 0..=operations {
        for tail in [Tail::Lost, Tail::Torn(5), Tail::Zeroed(5)] {
            for eager in [false, true] {
                let case = format!("crash after {after}, {tail:?}, eager names {eager}");
                let mut disk = MemDisk::new();
                disk.set_tail(tail);
                disk.set_eager_names(eager);
                disk.crash_after(after);
                assert_eq!(create(&mut disk).is_ok(), after == operations, "{case}");
                let mut disk = disk.restart();
                let mut reader = Reader::new(layout(), CachedView::default());
                run(&mut disk, reader.read()).unwrap();
                let Claimed { start, .. } =
                    run(&mut disk, Writer::claim(&layout(), reader.logs())).unwrap();
                match start {
                    Start::New => assert!(after < operations, "{case}"),
                    Start::Resumed(id) => assert!(reader.logs()[&id].genesis().is_some(), "{case}"),
                    Start::Rekeyed { .. } => panic!("{case}: {start:?}"),
                }
            }
        }
    }
}

/// A machine whose folder and local root are on one disk, so a power loss takes
/// both at once.
fn one_disk(disk: MemDisk) -> Machine {
    Machine {
        folder: disk.clone(),
        local: disk,
    }
}

/// A writer resumed after a power loss; with `open`, it has appended once since,
/// so its next append goes to an open segment.
fn resumed(tail: Tail, open: bool) -> Instance {
    let disk = MemDisk::new();
    disk.set_tail(tail);
    let mut instance = Instance::open(one_disk(disk), 1);
    instance.write("a").unwrap();
    let id = instance.id();
    let mut instance = Instance::open(one_disk(instance.machine.folder.restart()), 2);
    assert_eq!(instance.start, Start::Resumed(id));
    if open {
        instance.write("a2").unwrap();
    }
    instance
}

#[test]
fn an_interrupted_append_is_followed_by_a_new_segment() {
    for open in [false, true] {
        let mut probe = resumed(Tail::Lost, open);
        let before = probe.machine.folder.mutations();
        probe.write("b").unwrap();
        let operations = probe.machine.folder.mutations() - before;
        for after in 0..operations {
            for tail in [Tail::Lost, Tail::Torn(7), Tail::Zeroed(7)] {
                let case = format!("open {open}, crash after {after}, {tail:?}");
                let mut instance = resumed(tail, open);
                let id = instance.id();
                instance.machine.folder.crash_after(after);
                assert!(instance.write("b").is_err(), "{case}");
                let restarted = one_disk(instance.machine.folder.restart());
                let mut again = Instance::open(restarted, 3);
                assert_eq!(again.start, Start::Resumed(id), "{case}");
                let last = again.write("c").unwrap();
                let view = fresh(&again.machine.folder);
                let log = &view.writers()[&id];
                assert_eq!(log.heads(), last, "{case}");
                assert_eq!(log.forks(), [], "{case}");
                assert!(log.chain_to(last[0]).is_some(), "{case}");
            }
        }
    }
}

/// A copy of the folder of `disk` on a disk of its own.
fn folder_copy(disk: &MemDisk) -> MemDisk {
    let copy = MemDisk::new();
    for path in disk.directories(Root::Folder) {
        copy.perform(toshokan::Io::MakeDir {
            root: Root::Folder,
            path,
        })
        .unwrap();
    }
    for (path, bytes) in disk.files(Root::Folder) {
        copy.perform(toshokan::Io::Create {
            root: Root::Folder,
            path,
            bytes,
        })
        .unwrap();
    }
    sync_everything(&copy);
    copy
}

fn sync_everything(disk: &MemDisk) {
    let paths = disk
        .directories(Root::Folder)
        .into_iter()
        .chain(disk.files(Root::Folder).into_keys())
        .chain([toshokan::RelPath::ROOT]);
    for path in paths {
        disk.perform(toshokan::Io::Sync {
            root: Root::Folder,
            path,
        })
        .unwrap();
    }
}

#[test]
fn a_forked_history_is_never_written_again() {
    let mut original = Instance::open(machine(), 1);
    original.write("a1").unwrap();
    let id = original.id();
    let mut clone = Instance::open(original.machine.cloned(), 2);
    assert_eq!(clone.start, Start::Resumed(id));
    clone.write("b2").unwrap();
    original.write("a2").unwrap();
    let view = fresh(&original.machine.folder);
    assert_eq!(view.writers()[&id].forks().len(), 1);

    let dir = layout().writer(id);
    let before: BTreeMap<_, _> = original.machine.folder.files(Root::Folder);
    for (seed, instance) in [(3, original), (4, clone)] {
        let mut reopened = Instance::open(instance.close(), seed);
        assert_eq!(
            reopened.start,
            Start::Rekeyed {
                old: id,
                why: Rekey::Forked
            }
        );
        reopened.write("after").unwrap();
        assert_ne!(reopened.id(), id);
        let again = Instance::open(reopened.close(), seed + 10);
        assert_ne!(again.start, Start::Resumed(id));
    }
    let folder = machine_folder_after(&before, &dir);
    assert_eq!(
        folder,
        before
            .into_iter()
            .filter(|(p, _)| p.starts_with(&dir))
            .collect()
    );
}

fn machine_folder_after(
    files: &BTreeMap<toshokan::RelPath, Vec<u8>>,
    dir: &toshokan::RelPath,
) -> BTreeMap<toshokan::RelPath, Vec<u8>> {
    files
        .iter()
        .filter(|(path, _)| path.starts_with(dir))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}

#[test]
fn a_folder_restored_from_an_older_copy_rekeys_its_writers() {
    let mut instance = Instance::open(machine(), 1);
    instance.write("a1").unwrap();
    let id = instance.id();
    let mut machine = instance.close();
    let backup = folder_copy(&machine.folder);
    let mut later = Instance::open(machine.clone(), 2);
    later.write("a2").unwrap();
    machine = later.close();
    machine.folder = backup;
    let restored = Instance::open(machine, 3);
    assert_eq!(
        restored.start,
        Start::Rekeyed {
            old: id,
            why: Rekey::Restored
        }
    );
}

#[test]
fn a_running_writer_whose_segment_was_restored_refuses_to_append() {
    let mut instance = Instance::open(machine(), 1);
    instance.write("a1").unwrap();
    let id = instance.id();
    let backup = instance.machine.folder.files(Root::Folder);
    instance.write("a2").unwrap();
    for (path, bytes) in backup {
        let disk = &instance.machine.folder;
        disk.perform(toshokan::Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        })
        .unwrap();
        disk.perform(toshokan::Io::Create {
            root: Root::Folder,
            path,
            bytes,
        })
        .unwrap();
    }
    let refused = instance.write("a3");
    assert!(
        matches!(
            refused,
            Err(Error::Rekey {
                writer,
                why: Rekey::Restored
            }) if writer == id
        ),
        "{refused:?}"
    );
}

#[test]
fn losing_the_local_root_starts_a_new_writer_and_leaves_the_old_one_alone() {
    let mut instance = Instance::open(machine(), 1);
    instance.write("a").unwrap();
    let old = instance.id();
    let mut machine = instance.close();
    let dir = layout().writer(old);
    let before = machine_folder_after(&machine.folder.files(Root::Folder), &dir);
    machine.lose_local();
    let mut again = Instance::open(machine, 2);
    assert_eq!(again.start, Start::New);
    again.write("b").unwrap();
    assert_ne!(again.id(), old);
    let after = machine_folder_after(&again.machine.folder.files(Root::Folder), &dir);
    assert_eq!(after, before);
}

#[test]
fn a_failed_append_seals_its_segment() {
    let mut instance = Instance::open(machine(), 1);
    instance.write("a").unwrap();
    let id = instance.id();
    instance.machine.folder.set_capacity(Root::Folder, Some(0));
    assert!(instance.write("b").is_err());
    instance.machine.folder.set_capacity(Root::Folder, None);
    let last = instance.write("c").unwrap();
    assert_eq!(segments(&instance.machine, id), 2);
    let view = fresh(&instance.machine.folder);
    assert_eq!(view.writers()[&id].heads(), last);
}

#[test]
fn compaction_deletes_only_segments_this_process_sealed() {
    let mut first = Instance::open(machine(), 1);
    let mut written = first.write("a").unwrap();
    let id = first.id();
    let mut second = Instance::open(first.crash(), 2);
    written.extend(second.write("b").unwrap());
    assert_eq!(segments(&second.machine, id), 2);

    let compacted = second.compact().unwrap();
    assert_eq!(compacted.folded, 3);
    assert_eq!(compacted.removed.len(), 1);
    assert_eq!(
        segments(&second.machine, id),
        1,
        "the crashed process's segment stays"
    );
    assert!(second.writer.as_ref().unwrap().sealed().is_empty());

    written.extend(second.write("c").unwrap());
    let again = second.compact().unwrap();
    assert_eq!(again.folded, 4);
    let files = files_under(&second.machine.folder, &layout().writer(id));
    assert_eq!(
        files.len(),
        2,
        "one snapshot and the crashed segment: {files:?}"
    );
    let view = fresh(&second.machine.folder);
    assert_eq!(held(&view), written.iter().copied().collect());
    assert_eq!(
        view.writers()[&id].chain_to(*written.last().unwrap()),
        Some(written)
    );
}

#[test]
fn compacting_one_branch_of_a_fork_keeps_the_other() {
    let mut original = Instance::open(machine(), 1);
    let mut written = original.write("a1").unwrap();
    let mut clone = Instance::open(original.machine.cloned(), 2);
    written.extend(clone.write("b2").unwrap());
    written.extend(original.write("a2").unwrap());
    original.compact().unwrap();
    let view = fresh(&original.machine.folder);
    assert_eq!(held(&view), written.iter().copied().collect());
    assert_eq!(view.writers()[&original.id()].forks().len(), 1);
}

#[test]
fn a_crash_anywhere_in_compaction_keeps_every_entry() {
    let setup = |seed| {
        let mut instance = Instance::open(machine(), seed);
        let mut written = instance.write("a").unwrap();
        written.extend(instance.write("b").unwrap());
        (instance, written)
    };
    let (mut probe, _) = setup(1);
    let before = probe.machine.folder.mutations();
    probe.compact().unwrap();
    let operations = probe.machine.folder.mutations() - before;
    for after in 0..operations {
        let (mut instance, written) = setup(1);
        instance.machine.folder.crash_after(after);
        assert!(instance.compact().is_err());
        let restarted = instance.machine.folder.restart();
        let view = fresh(&restarted);
        assert_eq!(
            held(&view),
            written.iter().copied().collect(),
            "crash after {after}"
        );
    }
}

/// A disk whose next sync fails once `fail` is set.
struct FailingSync {
    disk: MemDisk,
    fail: bool,
}

impl toshokan::blocking::Backend for FailingSync {
    fn capabilities(&self, root: Root) -> toshokan::io::Capabilities {
        self.disk.capabilities(root)
    }

    fn perform(&mut self, io: toshokan::Io) -> toshokan::IoResult {
        if self.fail && matches!(io, toshokan::Io::Sync { .. }) {
            self.fail = false;
            return Err(toshokan::IoError::Other("sync failed".into()));
        }
        self.disk.perform(io)
    }
}

#[test]
fn entries_that_reached_the_folder_before_a_failure_are_followed_not_forked() {
    let intent = || {
        toshokan::log::EntryKind::Intent(toshokan::log::Logged {
            label: "x".into(),
            ops: Vec::new(),
            displaced: Vec::new(),
            reverses: None,
        })
    };
    for open in [false, true] {
        let mut backend = FailingSync {
            disk: MemDisk::new(),
            fail: false,
        };
        let mut writer = create(&mut backend.disk).unwrap();
        if !open {
            writer.seal();
        }
        backend.fail = true;
        let failed = run(
            &mut backend,
            writer.append(vec![(Hlc::ZERO, intent())], SegmentName::from_u128(9)),
        );
        assert!(failed.is_err());
        let last = run(
            &mut backend,
            writer.append(vec![(Hlc::ZERO, intent())], SegmentName::from_u128(10)),
        )
        .unwrap();
        let view = fresh(&backend.disk);
        let log = &view.writers()[&writer.id()];
        assert_eq!(log.heads(), [last[0].hash()], "open {open}");
        assert_eq!(log.forks(), [], "open {open}");
        assert_eq!(
            log.chain_to(last[0].hash()).map(|chain| chain.len()),
            Some(3)
        );
    }
}
