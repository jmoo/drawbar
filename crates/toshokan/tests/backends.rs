//! One behavior suite for every backend, run through the drivers: each behavior runs
//! on fresh, empty roots of each backend.

use std::collections::VecDeque;

use toshokan::asynch;
use toshokan::blocking::{self, Backend, Native};
use toshokan::io::{Capabilities, DirEntry, IoError, Kind, Lock, Range};
use toshokan::{Io, IoResult, MemDisk, Operation, RelPath, Reply, Root, Step};

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

/// Makes its requests in order and returns every result.
struct Script {
    requests: VecDeque<Io>,
    results: Vec<IoResult>,
}

impl Operation for Script {
    type Output = Vec<IoResult>;

    fn resume(&mut self, result: Option<IoResult>) -> Step<Vec<IoResult>> {
        self.results.extend(result);
        match self.requests.pop_front() {
            Some(io) => Step::Io(io),
            None => Step::Done(std::mem::take(&mut self.results)),
        }
    }
}

/// One process's view of a backend, driven by one of the drivers.
trait Driven {
    fn capabilities(&self, root: Root) -> Capabilities;
    fn run(&mut self, requests: Vec<Io>) -> Vec<IoResult>;
    /// A handle of another process on the same storage.
    fn other_process(&self) -> Box<dyn Driven>;

    fn one(&mut self, io: Io) -> IoResult {
        self.run(vec![io]).remove(0)
    }

    fn ok(&mut self, io: Io) -> Reply {
        let shown = format!("{io:?}");
        self.one(io).unwrap_or_else(|e| panic!("{shown}: {e}"))
    }
}

fn script(requests: Vec<Io>) -> Script {
    Script {
        requests: requests.into(),
        results: Vec::new(),
    }
}

struct BlockingMem(MemDisk);

impl Driven for BlockingMem {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.0, root)
    }

    fn run(&mut self, requests: Vec<Io>) -> Vec<IoResult> {
        blocking::run(&mut self.0, script(requests))
    }

    fn other_process(&self) -> Box<dyn Driven> {
        Box::new(Self(self.0.process()))
    }
}

struct AsyncMem(MemDisk);

impl Driven for AsyncMem {
    fn capabilities(&self, root: Root) -> Capabilities {
        asynch::Fs::capabilities(&self.0, root)
    }

    fn run(&mut self, requests: Vec<Io>) -> Vec<IoResult> {
        pollster::block_on(asynch::run(&self.0, script(requests)))
    }

    fn other_process(&self) -> Box<dyn Driven> {
        Box::new(Self(self.0.process()))
    }
}

struct NativeDirs {
    backend: Native,
    dirs: std::rc::Rc<[tempfile::TempDir; 2]>,
}

impl NativeDirs {
    fn new() -> Self {
        let dirs = std::rc::Rc::new([tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()]);
        Self {
            backend: Native::new(dirs[0].path(), dirs[1].path()),
            dirs,
        }
    }
}

impl Driven for NativeDirs {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.backend.capabilities(root)
    }

    fn run(&mut self, requests: Vec<Io>) -> Vec<IoResult> {
        blocking::run(&mut self.backend, script(requests))
    }

    fn other_process(&self) -> Box<dyn Driven> {
        Box::new(Self {
            backend: Native::new(self.dirs[0].path(), self.dirs[1].path()),
            dirs: self.dirs.clone(),
        })
    }
}

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

    pub fn a_created_file_reads_back_by_range(b: &mut dyn Driven) {
        b.ok(make_dir(Root::Folder, "a/b"));
        b.ok(create(Root::Folder, "a/b/f", b"hello"));
        let reads = b.run(vec![
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

    pub fn a_listing_is_sorted_by_name_with_kinds(b: &mut dyn Driven) {
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

    pub fn nothing_is_ever_replaced(b: &mut dyn Driven) {
        b.ok(create(Root::Folder, "f", b"old"));
        b.ok(make_dir(Root::Folder, "d/sub"));
        let results = b.run(vec![
            create(Root::Folder, "f", b"new"),
            create(Root::Folder, "gone/f", b""),
            rename("d", "f"),
            rename("d", "d/sub/in"),
            rename("gone", "x"),
        ]);
        assert_eq!(
            results,
            [
                Err(IoError::AlreadyExists),
                Err(IoError::NotFound),
                Err(IoError::AlreadyExists),
                Err(IoError::IntoItself),
                Err(IoError::NotFound),
            ]
        );
        assert_eq!(b.ok(read("f", 0, 9)), Reply::Bytes(b"old".to_vec()));
    }

    pub fn a_directory_renames_with_its_contents(b: &mut dyn Driven) {
        b.ok(make_dir(Root::Folder, "a/sub"));
        b.ok(create(Root::Folder, "a/sub/f", b"1"));
        b.ok(rename("a", "b"));
        assert_eq!(b.ok(read("b/sub/f", 0, 9)), Reply::Bytes(b"1".to_vec()));
        assert_eq!(b.ok(stat("a")), Reply::Stat(None));
    }

    pub fn removal_needs_the_right_kind_and_an_empty_directory(b: &mut dyn Driven) {
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
        let results = b.run(vec![
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

    pub fn making_a_directory_is_idempotent_and_refuses_a_file_in_the_way(b: &mut dyn Driven) {
        b.ok(create(Root::Folder, "f", b""));
        let results = b.run(vec![
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

    pub fn appends_extend_a_file(b: &mut dyn Driven) {
        if !b.capabilities(Root::Folder).append {
            return;
        }
        b.ok(create(Root::Folder, "log", b"a"));
        let append = |bytes: &[u8]| Io::Append {
            root: Root::Folder,
            path: path("log"),
            bytes: bytes.to_vec(),
        };
        b.run(vec![append(b"b"), append(b"c")]);
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

    pub fn the_roots_are_separate_trees(b: &mut dyn Driven) {
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

    pub fn a_lock_excludes_other_processes_until_released(b: &mut dyn Driven) {
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

macro_rules! for_every_backend {
    ($($behavior:ident),* $(,)?) => {
        mod blocking_mem {
            $(#[test] fn $behavior() { super::suite::$behavior(&mut super::BlockingMem(toshokan::MemDisk::new())); })*
        }
        mod async_mem {
            $(#[test] fn $behavior() { super::suite::$behavior(&mut super::AsyncMem(toshokan::MemDisk::new())); })*
        }
        mod native {
            $(#[test] fn $behavior() { super::suite::$behavior(&mut super::NativeDirs::new()); })*
        }
    };
}

for_every_backend!(
    a_created_file_reads_back_by_range,
    a_listing_is_sorted_by_name_with_kinds,
    nothing_is_ever_replaced,
    a_directory_renames_with_its_contents,
    removal_needs_the_right_kind_and_an_empty_directory,
    making_a_directory_is_idempotent_and_refuses_a_file_in_the_way,
    appends_extend_a_file,
    the_roots_are_separate_trees,
    a_lock_excludes_other_processes_until_released,
);
