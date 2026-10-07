//! One behavior suite for every backend, run through the drivers: each behavior runs
//! on fresh, empty roots of each backend.

mod common;

use common::{path, Driven};
use toshokan::io::{DirEntry, IoError, Kind, Lock, Meta, Range};
use toshokan::{Io, IoResult, RelPath, Reply, Root};

fn create(root: Root, text: &str, bytes: &[u8]) -> Io {
    Io::Create {
        root,
        path: path(text),
        bytes: bytes.to_vec(),
    }
}

fn make_dir(root: Root, text: &str) -> Io {
    Io::MakeDir {
        root,
        path: path(text),
    }
}

fn read(text: &str, offset: u64, len: u64) -> Io {
    Io::Read {
        root: Root::Folder,
        path: path(text),
        range: Range { offset, len },
    }
}

fn stat(text: &str) -> Io {
    Io::Stat {
        root: Root::Folder,
        path: path(text),
    }
}

fn rename(from: &str, to: &str) -> Io {
    Io::Rename {
        root: Root::Folder,
        from: path(from),
        to: path(to),
    }
}

fn listing(entries: &[(&str, Kind)]) -> Reply {
    Reply::Listed(
        entries
            .iter()
            .map(|&(name, kind)| DirEntry {
                name: name.to_owned(),
                kind,
            })
            .collect(),
    )
}

mod suite {
    use super::*;

    pub fn a_created_file_reads_back_by_range(b: &mut impl Driven) {
        b.ok(make_dir(Root::Folder, "a/b"));
        b.ok(create(Root::Folder, "a/b/f", b"hello"));
        let reads = b.requests(vec![
            read("a/b/f", 0, 5),
            read("a/b/f", 3, 10),
            read("a/b/f", 9, 10),
        ]);
        let expected: Vec<IoResult> = [&b"hello"[..], b"lo", b""]
            .map(|bytes| Ok(Reply::Bytes(bytes.to_vec())))
            .into();
        assert_eq!(reads, expected);
        let Reply::Stat(Some(meta)) = b.ok(stat("a/b/f")) else {
            panic!("a/b/f is missing");
        };
        assert_eq!((meta.kind, meta.len), (Kind::File, 5));
        assert_eq!(b.ok(stat("a/b/f")), Reply::Stat(Some(meta)), "unchanged");
        assert_eq!(b.ok(stat("a/x")), Reply::Stat(None));
    }

    pub fn a_listing_is_sorted_by_name_with_kinds(b: &mut impl Driven) {
        for name in ["b", "a", "C"] {
            b.ok(create(Root::Folder, name, b""));
        }
        b.ok(make_dir(Root::Folder, "d"));
        let listed = b.ok(Io::List {
            root: Root::Folder,
            dir: RelPath::ROOT,
        });
        assert_eq!(
            listed,
            listing(&[
                ("C", Kind::File),
                ("a", Kind::File),
                ("b", Kind::File),
                ("d", Kind::Directory),
            ])
        );
    }

    pub fn a_listing_with_metadata_says_what_a_listing_and_each_stat_say(b: &mut impl Driven) {
        b.ok(make_dir(Root::Folder, "d/e"));
        b.ok(create(Root::Folder, "d/b", b"bb"));
        b.ok(create(Root::Folder, "d/a", b"a"));
        let dir = path("d");
        let Reply::Listed(entries) = b.ok(Io::List {
            root: Root::Folder,
            dir: dir.clone(),
        }) else {
            panic!("not a listing");
        };
        let each: Vec<(String, Meta)> = entries
            .into_iter()
            .map(|entry| {
                let at = dir.join(&entry.name).unwrap();
                let Reply::Stat(Some(meta)) = b.ok(stat(at.as_str())) else {
                    panic!("{at:?} is missing");
                };
                (entry.name, meta)
            })
            .collect();
        let names: Vec<&str> = each.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["a", "b", "e"]);
        let list_stat = |dir: &str| Io::ListStat {
            root: Root::Folder,
            dir: path(dir),
        };
        assert_eq!(b.ok(list_stat("d")), Reply::ListedStat(each));
        assert_eq!(
            b.requests(vec![list_stat("none"), list_stat("d/a")]),
            [Err(IoError::NotFound), Err(IoError::NotDirectory)]
        );
    }

    pub fn several_reads_answer_each_as_it_would_alone(b: &mut impl Driven) {
        b.ok(make_dir(Root::Folder, "d"));
        b.ok(create(Root::Folder, "f", b"hello"));
        let reads = [("f", 0, 5), ("none", 0, 1), ("d", 0, 1), ("f", 3, 10)];
        let alone: Vec<IoResult> = reads
            .iter()
            .map(|&(at, offset, len)| b.one(read(at, offset, len)))
            .collect();
        let together = b.ok(Io::ReadMany {
            root: Root::Folder,
            reads: reads
                .iter()
                .map(|&(at, offset, len)| (path(at), Range { offset, len }))
                .collect(),
        });
        let alone = alone.into_iter().map(|read| match read {
            Ok(Reply::Bytes(bytes)) => Ok(bytes),
            Ok(other) => panic!("{other:?}"),
            Err(error) => Err(error),
        });
        assert_eq!(together, Reply::ReadMany(alone.collect()));
        assert_eq!(
            b.ok(Io::ReadMany {
                root: Root::Folder,
                reads: Vec::new(),
            }),
            Reply::ReadMany(Vec::new())
        );
    }

    pub fn nothing_is_ever_replaced(b: &mut impl Driven) {
        b.ok(create(Root::Folder, "f", b"old"));
        b.ok(make_dir(Root::Folder, "d/sub"));
        let mut requests = vec![
            create(Root::Folder, "f", b"new"),
            create(Root::Folder, "gone/f", b""),
            rename("d", "d/sub/in"),
            rename("gone", "x"),
        ];
        let mut expected = vec![
            Err(IoError::AlreadyExists),
            Err(IoError::NotFound),
            Err(IoError::IntoItself),
            Err(IoError::NotFound),
        ];
        if b.capabilities(Root::Folder).no_replace {
            requests.push(rename("d", "f"));
            expected.push(Err(IoError::AlreadyExists));
        }
        assert_eq!(b.requests(requests), expected);
        assert_eq!(b.ok(read("f", 0, 9)), Reply::Bytes(b"old".to_vec()));
    }

    pub fn a_write_lands_at_its_offset_and_past_the_end_after_zeros(b: &mut impl Driven) {
        b.ok(create(Root::Folder, "f", b"abc"));
        let write = |text: &str, offset, bytes: &[u8]| Io::Write {
            root: Root::Folder,
            path: path(text),
            offset,
            bytes: bytes.to_vec(),
        };
        b.requests(vec![write("f", 1, b"XY"), write("f", 5, b"Z")]);
        assert_eq!(b.ok(read("f", 0, 9)), Reply::Bytes(b"aXY\0\0Z".to_vec()));
        assert_eq!(b.one(write("none", 0, b"x")), Err(IoError::NotFound));
    }

    pub fn a_directory_renames_with_its_contents(b: &mut impl Driven) {
        b.ok(make_dir(Root::Folder, "a/sub"));
        b.ok(create(Root::Folder, "a/sub/f", b"1"));
        b.ok(rename("a", "b"));
        assert_eq!(b.ok(read("b/sub/f", 0, 9)), Reply::Bytes(b"1".to_vec()));
        assert_eq!(b.ok(stat("a")), Reply::Stat(None));
    }

    pub fn removal_needs_the_right_kind_and_an_empty_directory(b: &mut impl Driven) {
        b.ok(make_dir(Root::Folder, "d"));
        b.ok(create(Root::Folder, "d/f", b""));
        let remove = |text: &str| Io::Remove {
            root: Root::Folder,
            path: path(text),
        };
        let remove_dir = |text: &str| Io::RemoveDir {
            root: Root::Folder,
            path: path(text),
        };
        let results = b.requests(vec![
            remove_dir("d"),
            remove("d"),
            remove_dir("d/f"),
            remove("d/f"),
            remove_dir("d"),
            remove("d"),
        ]);
        assert_eq!(
            results,
            [
                Err(IoError::NotEmpty),
                Err(IoError::IsDirectory),
                Err(IoError::NotDirectory),
                Ok(Reply::Done),
                Ok(Reply::Done),
                Err(IoError::NotFound),
            ]
        );
    }

    pub fn making_a_directory_is_idempotent_and_refuses_a_file_in_the_way(b: &mut impl Driven) {
        b.ok(create(Root::Folder, "f", b""));
        let results = b.requests(vec![
            make_dir(Root::Folder, "a/b"),
            make_dir(Root::Folder, "a/b"),
            make_dir(Root::Folder, "f/g"),
            make_dir(Root::Folder, ""),
        ]);
        assert_eq!(
            results,
            [
                Ok(Reply::Done),
                Ok(Reply::Done),
                Err(IoError::NotDirectory),
                Ok(Reply::Done),
            ]
        );
    }

    pub fn appends_extend_a_file(b: &mut impl Driven) {
        if !b.capabilities(Root::Folder).append {
            return;
        }
        b.ok(create(Root::Folder, "log", b"a"));
        let append = |bytes: &[u8]| Io::Append {
            root: Root::Folder,
            path: path("log"),
            bytes: bytes.to_vec(),
        };
        b.requests(vec![append(b"b"), append(b"c")]);
        assert_eq!(b.ok(read("log", 0, 9)), Reply::Bytes(b"abc".to_vec()));
        assert_eq!(
            b.one(Io::Append {
                root: Root::Folder,
                path: path("none"),
                bytes: b"x".to_vec(),
            }),
            Err(IoError::NotFound)
        );
    }

    pub fn the_roots_are_separate_trees(b: &mut impl Driven) {
        b.ok(create(Root::Local, "f", b"local"));
        assert_eq!(b.ok(stat("f")), Reply::Stat(None));
        b.ok(create(Root::Folder, "f", b"folder"));
        assert_eq!(
            b.ok(Io::Read {
                root: Root::Local,
                path: path("f"),
                range: Range { offset: 0, len: 9 },
            }),
            Reply::Bytes(b"local".to_vec())
        );
    }

    pub fn requests_on_the_wrong_kind_or_nothing_fail_alike(b: &mut impl Driven) {
        b.ok(make_dir(Root::Folder, "d"));
        b.ok(create(Root::Folder, "f", b"x"));
        let results = b.requests(vec![
            read("d", 0, 1),
            read("none", 0, 1),
            Io::List {
                root: Root::Folder,
                dir: path("f"),
            },
            Io::List {
                root: Root::Folder,
                dir: path("none"),
            },
            create(Root::Folder, "f/g", b""),
            rename("f", "none/f"),
            Io::Remove {
                root: Root::Folder,
                path: path("none"),
            },
            Io::Sync {
                root: Root::Folder,
                path: path("none"),
            },
        ]);
        assert_eq!(
            results,
            [
                Err(IoError::IsDirectory),
                Err(IoError::NotFound),
                Err(IoError::NotDirectory),
                Err(IoError::NotFound),
                Err(IoError::NotDirectory),
                Err(IoError::NotFound),
                Err(IoError::NotFound),
                match b.capabilities(Root::Folder).fsync {
                    true => Err(IoError::NotFound),
                    false => Ok(Reply::Done),
                },
            ]
        );
        assert_eq!(b.ok(read("f", 0, 9)), Reply::Bytes(b"x".to_vec()));
    }

    pub fn a_file_renames_across_directories_and_syncs(b: &mut impl Driven) {
        b.ok(make_dir(Root::Folder, "a"));
        b.ok(make_dir(Root::Folder, "b/c"));
        b.ok(create(Root::Folder, "a/f", b"1"));
        b.ok(rename("a/f", "b/c/g"));
        for synced in ["b/c/g", "b/c", "a", ""] {
            b.ok(Io::Sync {
                root: Root::Folder,
                path: path(synced),
            });
        }
        assert_eq!(b.ok(read("b/c/g", 0, 9)), Reply::Bytes(b"1".to_vec()));
        assert_eq!(b.ok(stat("a/f")), Reply::Stat(None));
        let Reply::Stat(Some(root)) = b.ok(stat("")) else {
            panic!("the root is missing");
        };
        assert_eq!(root.kind, Kind::Directory);
    }

    pub fn a_lock_excludes_other_processes_until_released(b: &mut impl Driven) {
        b.ok(make_dir(Root::Local, "w"));
        let lock = || Io::Lock {
            name: path("w/lock"),
        };
        let unlock = || Io::Unlock {
            name: path("w/lock"),
        };
        let mut other = b.other_process();
        assert_eq!(b.ok(lock()), Reply::Lock(Lock::Acquired));
        assert_eq!(b.ok(lock()), Reply::Lock(Lock::Acquired), "held already");
        assert_eq!(other.ok(lock()), Reply::Lock(Lock::Held));
        assert_eq!(other.ok(unlock()), Reply::Done, "not held is success");
        assert_eq!(other.ok(lock()), Reply::Lock(Lock::Held));
        b.ok(unlock());
        assert_eq!(other.ok(lock()), Reply::Lock(Lock::Acquired));
    }
}

for_every_backend!(suite:
    a_created_file_reads_back_by_range,
    a_listing_is_sorted_by_name_with_kinds,
    a_listing_with_metadata_says_what_a_listing_and_each_stat_say,
    several_reads_answer_each_as_it_would_alone,
    nothing_is_ever_replaced,
    a_directory_renames_with_its_contents,
    removal_needs_the_right_kind_and_an_empty_directory,
    making_a_directory_is_idempotent_and_refuses_a_file_in_the_way,
    appends_extend_a_file,
    a_write_lands_at_its_offset_and_past_the_end_after_zeros,
    the_roots_are_separate_trees,
    requests_on_the_wrong_kind_or_nothing_fail_alike,
    a_file_renames_across_directories_and_syncs,
    a_lock_excludes_other_processes_until_released,
);

#[test]
fn a_native_root_declares_no_replace_only_where_its_renames_refuse_a_destination() {
    let mut native = common::NativeDirs::new();
    let declared = native.capabilities(Root::Folder).no_replace;
    #[cfg(any(target_vendor = "apple", windows))]
    assert!(
        declared,
        "the temporary directory's volume refuses atomically"
    );
    native.ok(create(Root::Folder, "a", b"a"));
    native.ok(create(Root::Folder, "b", b"b"));
    let renamed = native.one(rename("a", "b"));
    if declared {
        assert_eq!(renamed, Err(IoError::AlreadyExists));
        assert_eq!(
            native.ok(read("b", 0, 1)),
            Reply::Bytes(b"b".to_vec()),
            "the destination is kept"
        );
    }
}
