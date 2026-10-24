//! How many requests operations ask of a backend. A browser backend pays one round
//! trip for each, so a request per file of a large library is a cost the user
//! waits for.

mod common;

use common::{
    bound, commit, copying, env, identify, layout, path, put, Driven, Fill, Recorded, WRITER,
};
use toshokan::binding::{self, Fact, Facts, Scan};
use toshokan::blocking::{self, Library};
use toshokan::log::FileFact;
use toshokan::plan::{Content, Expect, FileChange};
use toshokan::reader::{CachedView, Reader};
use toshokan::report::TrashItem;
use toshokan::trash;
use toshokan::{EntityId, EntryHash, Hlc, Io, MemDisk, Nonce, Register, RelPath, Root, Schema};

/// Each request's kind and the path it names.
fn shapes(requests: &[Io]) -> Vec<(&'static str, String)> {
    requests
        .iter()
        .map(|io| {
            let kind = match io {
                Io::List { .. } => "List",
                Io::ListStat { .. } => "ListStat",
                Io::Stat { .. } => "Stat",
                Io::ReadMany { reads, .. } => return ("ReadMany", reads.len().to_string()),
                Io::Create { .. } => "Create",
                Io::Write { .. } => "Write",
                Io::Rename { .. } => "Rename",
                Io::Remove { .. } => "Remove",
                Io::Sync { .. } => "Sync",
                other => panic!("unexpected {other:?}"),
            };
            (kind, io.path().as_str().to_owned())
        })
        .collect()
}

fn shape(kind: &'static str, at: &str) -> (&'static str, String) {
    (kind, at.to_owned())
}

#[test]
fn a_scan_asks_once_per_directory_and_reads_identities_together() {
    let mut d = Recorded::new(MemDisk::new());
    put(
        &mut d,
        &[
            ("d/a", b"0123"),
            ("d/b", b"4567"),
            ("e/c", b"89ab"),
            (".t/x", b"cdef"),
        ],
    );
    let fact = FileFact {
        path: path("elsewhere"),
        identity: toshokan::Identity::from_u128(1),
        len: 4,
        modified: None,
    };
    let written = Fact {
        value: fact,
        entry: toshokan::log::Ref {
            writer: toshokan::WriterId::from_u128(1),
            n: 1,
            hash: EntryHash::from_u128(1),
        },
    };
    let facts = Facts::from([(EntityId::from_u128(1), vec![written].into())]);
    d.take();
    let scan = blocking::run(
        &mut d,
        binding::scan(&layout(), &identify(), &facts, &Scan::default()),
    )
    .unwrap()
    .0;
    assert_eq!(scan.files.len(), 3);
    assert!(scan.files.values().all(|file| file.identity.is_some()));
    assert_eq!(
        shapes(&d.take()),
        [
            shape("ListStat", ""),
            shape("ListStat", "d"),
            shape("ListStat", "e"),
            shape("ReadMany", "3"),
        ]
    );
}

#[test]
fn a_writer_directory_is_read_and_reread_unchanged_in_two_requests() {
    let mut d = Recorded::new(MemDisk::new());
    let dir = layout().writer(WRITER);
    let segments: Vec<String> = (0..5).map(|i| format!("{dir}/s{i}.jsonl")).collect();
    let files: Vec<(&str, &[u8])> = segments
        .iter()
        .map(|at| (at.as_str(), &b"x\n"[..]))
        .collect();
    put(&mut d, &files);
    let mut reader = Reader::new(layout(), CachedView::default());
    d.take();
    blocking::run(&mut d, reader.read()).unwrap();
    let writers = layout().writers();
    assert_eq!(
        shapes(&d.take()),
        [
            shape("List", writers.as_str()),
            shape("ListStat", dir.as_str()),
            shape("ReadMany", "5"),
        ],
        "every file whole"
    );
    blocking::run(&mut d, reader.read()).unwrap();
    assert_eq!(
        shapes(&d.take()),
        [
            shape("List", writers.as_str()),
            shape("ListStat", dir.as_str()),
            shape("ReadMany", "5"),
        ],
        "only the tails"
    );
}

#[test]
fn a_trash_is_listed_in_one_request_and_emptied_with_one_sync() {
    let mut d = Recorded::new(MemDisk::new());
    let items: Vec<TrashItem> = (1..=4)
        .map(|n| TrashItem {
            writer: WRITER,
            item: Nonce::from_u128(n),
            len: 0,
            from: path("f"),
            at: Hlc::default(),
            by: EntryHash::from_u128(n),
        })
        .collect();
    let paths: Vec<String> = items
        .iter()
        .map(|item| layout().trash(WRITER, item.item).as_str().to_owned())
        .collect();
    let files: Vec<(&str, &[u8])> = paths
        .iter()
        .map(|at| (at.as_str(), &b"bytes"[..]))
        .collect();
    put(&mut d, &files);
    d.take();
    let listed = blocking::run(&mut d, trash::list(&layout(), WRITER, items)).unwrap();
    assert!(listed.iter().all(|item| item.len == 5));
    let dir = layout().trash_dir(WRITER);
    assert_eq!(shapes(&d.take()), [shape("ListStat", dir.as_str())]);
    let emptied = blocking::run(
        &mut d,
        trash::empty(
            &layout(),
            WRITER,
            listed,
            Default::default(),
            Default::default(),
        ),
    )
    .unwrap();
    assert_eq!(emptied.removed.len(), 4);
    let pending = layout().pending_dir(WRITER);
    let mut expected = vec![shape("List", pending.as_str())];
    expected.extend(paths.iter().map(|at| shape("Remove", at)));
    expected.push(shape("Sync", dir.as_str()));
    assert_eq!(shapes(&d.take()), expected);
}

/// The directories and the files a commit of new files into one directory syncs.
fn synced_saving(files: usize) -> (Vec<RelPath>, usize) {
    let mut d = Recorded::new(MemDisk::new());
    let saves: Vec<FileChange> = (0..files)
        .map(|i| FileChange::Save {
            entity: EntityId::from_u128(i as u128 + 1),
            path: path(&format!("d/f{i}")),
            content: Content(i),
            expect: Expect::Absent,
        })
        .collect();
    let contents = (0..files).map(|i| Fill::Bytes(vec![i as u8; 3])).collect();
    commit(&mut d, &saves, contents, &bound(&[]), &mut env(1)).unwrap();
    let synced = d.take().into_iter().filter_map(|io| match io {
        Io::Sync { path, .. } => Some(path),
        _ => None,
    });
    let (dirs, files): (Vec<RelPath>, Vec<RelPath>) = synced.partition(|synced| {
        d.disk.directories(toshokan::Root::Folder).contains(synced) || synced.is_root()
    });
    (dirs, files.len())
}

#[test]
fn saving_more_files_into_one_directory_adds_no_directory_sync() {
    let (one_dirs, one_files) = synced_saving(1);
    let (many_dirs, many_files) = synced_saving(20);
    assert_eq!(many_dirs, one_dirs);
    assert_eq!(
        many_files - one_files,
        19,
        "each staged file's contents are synced"
    );
}

const NAME: Register<String> = Register::new("name");

/// A library whose one writer saved `files` files into two directories.
fn saved_library(files: usize) -> MemDisk {
    let disk = MemDisk::new();
    let schema = Schema::of(&[NAME.key()]).unwrap();
    let (mut library, _) = Library::open(disk.clone(), layout(), &schema, env(1)).unwrap();
    let mut intent = library.intent("Import");
    for i in 0..files {
        let at = path(&format!("d{}/f{i}", i % 2));
        let bytes = vec![i as u8; 10 + i];
        (intent, _) = intent.create(|e| {
            e.save(&at, bytes, Expect::Absent).set(NAME, format!("{i}"));
        });
    }
    intent.commit().unwrap();
    library.close().unwrap();
    disk
}

/// The requests to open the library on `disk` and to refresh it with nothing new.
fn opening(disk: &MemDisk) -> (Vec<Io>, Vec<Io>) {
    let recorded = Recorded::new(disk.process());
    let schema = Schema::of(&[NAME.key()]).unwrap();
    let (mut library, _) = Library::open(recorded.clone(), layout(), &schema, env(2)).unwrap();
    let opened = recorded.take();
    library.refresh().unwrap();
    (opened, recorded.take())
}

#[test]
fn opening_and_refreshing_ask_per_directory_not_per_file() {
    let (few_open, few_refresh) = opening(&saved_library(10));
    let (many_open, many_refresh) = opening(&saved_library(60));
    assert_eq!(many_open.len(), few_open.len());
    assert_eq!(many_refresh.len(), few_refresh.len());
    assert!(
        many_refresh.iter().all(|io| !io.mutates()),
        "{many_refresh:?}"
    );
}

#[test]
fn a_backend_without_fsync_is_never_asked_to_sync() {
    let without = toshokan::io::Capabilities {
        fsync: false,
        ..toshokan::io::Capabilities::ALL
    };
    let sync = || Io::Sync {
        root: toshokan::Root::Folder,
        path: path("nothing/there"),
    };
    let disk = MemDisk::with_capabilities(without, without);
    assert_eq!(
        common::BlockingMem(disk.clone()).one(sync()),
        Ok(toshokan::Reply::Done)
    );
    assert_eq!(
        common::AsyncMem(disk.clone()).one(sync()),
        Ok(toshokan::Reply::Done)
    );
    assert_eq!(
        disk.mutations(),
        0,
        "the disk counts every sync it is asked"
    );
}

#[test]
fn a_refresh_reads_nothing_of_sealed_segments_it_read_before() {
    let disk = saved_library(4);
    let schema = Schema::of(&[NAME.key()]).unwrap();
    let mut other = Library::open(disk.process(), layout(), &schema, env(3))
        .unwrap()
        .0;
    let (note, _) = other.intent("Note").create(|e| {
        e.set(NAME, "note".into());
    });
    note.commit().unwrap();
    let recorded = Recorded::new(disk.process());
    let (mut library, _) = Library::open(recorded.clone(), layout(), &schema, env(2)).unwrap();
    recorded.take();
    library.refresh().unwrap();
    let tails: Vec<usize> = recorded
        .take()
        .iter()
        .filter_map(|io| match io {
            Io::ReadMany { reads, .. } => Some(reads.len()),
            _ => None,
        })
        .collect();
    assert_eq!(tails, [1], "only the open segment's tail");
}

#[test]
fn a_refresh_asks_the_length_and_time_only_of_files_that_may_grow() {
    let disk = saved_library(4);
    let schema = Schema::of(&[NAME.key()]).unwrap();
    let mut other = Library::open(disk.process(), layout(), &schema, env(3))
        .unwrap()
        .0;
    let (note, _) = other.intent("Note").create(|e| {
        e.set(NAME, "note".into());
    });
    note.commit().unwrap();
    let recorded = Recorded::new(disk.process());
    let (mut library, _) = Library::open(recorded.clone(), layout(), &schema, env(2)).unwrap();
    library.rescan().unwrap();
    recorded.take();
    library.refresh().unwrap();
    let asked: Vec<&'static str> = recorded
        .take()
        .iter()
        .filter(|io| io.root() == Root::Folder)
        .filter_map(|io| match io {
            Io::ListStat { .. } => Some("ListStat"),
            Io::Stat { .. } => Some("Stat"),
            _ => None,
        })
        .collect();
    assert_eq!(asked, ["Stat"], "only the open segment's");
    library.rescan().unwrap();
    let listed = recorded.take();
    let stats = listed.iter().filter(|io| matches!(io, Io::ListStat { .. }));
    assert!(
        stats.count() >= 2,
        "a rescan asks of every writer's files: {listed:?}"
    );
}

#[test]
fn a_commit_writes_the_head_over_the_last_one_in_three_requests() {
    let disk = saved_library(1);
    let schema = Schema::of(&[NAME.key()]).unwrap();
    let recorded = Recorded::new(disk.process());
    let (mut library, _) = Library::open(recorded.clone(), layout(), &schema, env(2)).unwrap();
    let (note, _) = library.intent("Note").create(|e| {
        e.set(NAME, "note".into());
    });
    recorded.take();
    note.commit().unwrap();
    let head: Vec<Io> = recorded
        .take()
        .into_iter()
        .filter(|io| {
            let name = io.path().name().unwrap_or_default();
            io.root() == Root::Local && name.starts_with("head.json")
        })
        .collect();
    let named: Vec<(&str, String)> = shapes(&head)
        .into_iter()
        .map(|(kind, at)| (kind, at.rsplit('/').next().unwrap_or_default().to_owned()))
        .collect();
    assert_eq!(
        named,
        [
            shape("Stat", "head.json"),
            shape("Write", "head.json"),
            shape("Sync", "head.json")
        ]
    );
}

/// A library on `disk` whose writer saved `files` files, an intent each.
fn saved_one_by_one(disk: MemDisk, files: usize) -> (Library<Recorded>, Recorded) {
    let recorded = Recorded::new(disk);
    let schema = Schema::of(&[NAME.key()]).unwrap();
    let (mut library, _) = Library::open(recorded.clone(), layout(), &schema, env(1)).unwrap();
    for i in 0..files {
        let (save, _) = library.intent("Save").create(|e| {
            e.save(&path(&format!("f{i}")), vec![i as u8], Expect::Absent);
        });
        save.commit().unwrap();
    }
    (library, recorded)
}

/// Every file in a writer's `pending/`.
fn pending(recorded: &Recorded) -> Vec<RelPath> {
    let files = recorded.disk.files(Root::Folder).into_keys();
    files
        .filter(|path| path.components().any(|c| c == "pending"))
        .collect()
}

/// The files in a writer's `pending/` that `requests` remove, in order.
fn removed_from_pending(requests: &[Io]) -> Vec<RelPath> {
    let removed = requests.iter().filter_map(|io| match io {
        Io::Remove { path, .. } if path.components().any(|c| c == "pending") => Some(path.clone()),
        _ => None,
    });
    removed.collect()
}

#[test]
fn no_commit_removes_a_pending_record_and_tidying_removes_each() {
    let (mut library, recorded) = saved_one_by_one(MemDisk::new(), 2);
    let (note, _) = library.intent("Note").create(|e| {
        e.set(NAME, "note".into());
    });
    note.commit().unwrap();
    assert_eq!(removed_from_pending(&recorded.take()), []);
    let records = pending(&recorded);
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(library.tidy().unwrap().removed.len(), 2);
    assert_eq!(removed_from_pending(&recorded.take()), records);
}

#[test]
fn tidying_removes_a_records_markers_before_the_record() {
    let (mut library, recorded) = saved_one_by_one(copying(), 1);
    let (records, markers): (Vec<RelPath>, Vec<RelPath>) = pending(&recorded)
        .into_iter()
        .partition(|path| path.as_str().ends_with(".json"));
    assert_eq!((records.len(), markers.len()), (1, 1), "{markers:?}");
    recorded.take();
    library.tidy().unwrap();
    assert_eq!(
        removed_from_pending(&recorded.take()),
        [markers, records].concat()
    );
}
