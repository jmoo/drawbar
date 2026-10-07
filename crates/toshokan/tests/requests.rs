//! How many requests operations ask of a backend. A browser backend pays one round
//! trip for each, so a request per file of a large library is a cost the user
//! waits for.

mod common;

use std::collections::BTreeMap;

use common::{env, identify, layout, path, put, Recorded, WRITER};
use toshokan::binding::{self, Facts, Scan};
use toshokan::blocking::{self, Library};
use toshokan::log::FileFact;
use toshokan::plan::Expect;
use toshokan::reader::{CachedView, Reader};
use toshokan::report::TrashItem;
use toshokan::schema::Written;
use toshokan::trash;
use toshokan::{EntityId, EntryHash, Hlc, Io, MemDisk, Nonce, Register, Schema};

/// Each request's kind and the path it names.
fn shapes(requests: &[Io]) -> Vec<(&'static str, String)> {
    requests
        .iter()
        .map(|io| {
            let kind = match io {
                Io::List { .. } => "List",
                Io::ListStat { .. } => "ListStat",
                Io::ReadMany { reads, .. } => return ("ReadMany", reads.len().to_string()),
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
    let written = Written {
        value: fact,
        entry: EntryHash::from_u128(1),
        at: Hlc::default(),
        by: WRITER,
    };
    let facts: Facts = BTreeMap::from([(EntityId::from_u128(1), vec![written])]);
    d.take();
    let scan = blocking::run(
        &mut d,
        binding::scan(&layout(), &identify(), &facts, &Scan::default()),
    )
    .unwrap();
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
fn a_writer_directory_is_read_in_three_requests_and_reread_unchanged_in_two() {
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
            shape("ReadMany", "5"),
        ],
        "every tail, then every file whole"
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
fn a_trash_is_listed_in_one_request() {
    let mut d = Recorded::new(MemDisk::new());
    let items: Vec<TrashItem> = (1..=4)
        .map(|n| TrashItem {
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
