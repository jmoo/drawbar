//! Operations written as continuations: each request carries the code that
//! consumes its result, so a sequence of requests reads as a sequence.

use std::collections::VecDeque;
use std::rc::Rc;

use crate::env::Identify;
use crate::error::{Error, Result};
use crate::ids::Identity;
use crate::io::{
    DirEntry, Io, IoError, IoResult, Kind, Lock, Meta, Operation, Range, Reply, Root, Step, Task,
    CHUNK,
};
use crate::path::RelPath;

type Then<'a, T> = Box<dyn FnOnce(IoResult) -> Flow<'a, T> + 'a>;

/// A request and what follows it, or the result.
pub(crate) enum Flow<'a, T> {
    Io(Io, Then<'a, T>),
    Done(T),
}

pub(crate) type Fallible<'a, T> = Flow<'a, Result<T>>;

impl<'a, T: 'a> Flow<'a, T> {
    pub(crate) fn io(io: Io, then: impl FnOnce(IoResult) -> Self + 'a) -> Self {
        Self::Io(io, Box::new(then))
    }

    pub(crate) fn then<U: 'a>(self, next: impl FnOnce(T) -> Flow<'a, U> + 'a) -> Flow<'a, U> {
        match self {
            Self::Done(value) => next(value),
            Self::Io(io, then) => Flow::Io(io, Box::new(move |result| then(result).then(next))),
        }
    }

    pub(crate) fn task(self) -> Task<'a, T> {
        Task::new(Running::Ready(self))
    }
}

impl<'a, T: 'a> Flow<'a, Result<T>> {
    /// Continues with the value, or ends with the error.
    pub(crate) fn and_then<U: 'a>(
        self,
        next: impl FnOnce(T) -> Fallible<'a, U> + 'a,
    ) -> Fallible<'a, U> {
        self.then(|result| match result {
            Ok(value) => next(value),
            Err(error) => Flow::Done(Err(error)),
        })
    }

    pub(crate) fn map_ok<U: 'a>(self, f: impl FnOnce(T) -> U + 'a) -> Fallible<'a, U> {
        self.and_then(|value| ok(f(value)))
    }
}

pub(crate) fn ok<'a, T>(value: T) -> Fallible<'a, T> {
    Flow::Done(Ok(value))
}

/// Runs a task as part of a flow.
pub(crate) fn run<'a, T: 'a>(task: Task<'a, T>) -> Flow<'a, T> {
    drive(task, None)
}

fn drive<'a, T: 'a>(mut task: Task<'a, T>, result: Option<IoResult>) -> Flow<'a, T> {
    match task.resume(result) {
        Step::Done(value) => Flow::Done(value),
        Step::Io(io) => Flow::io(io, move |result| drive(task, Some(result))),
    }
}

enum Running<'a, T> {
    Ready(Flow<'a, T>),
    Waiting(Then<'a, T>),
    Finished,
}

impl<T> Operation for Running<'_, T> {
    type Output = T;

    fn resume(&mut self, result: Option<IoResult>) -> Step<T> {
        let flow = match (std::mem::replace(self, Self::Finished), result) {
            (Self::Ready(flow), None) => flow,
            (Self::Waiting(then), Some(result)) => then(result),
            _ => panic!("an operation was resumed out of turn"),
        };
        match flow {
            Flow::Io(io, then) => {
                *self = Self::Waiting(then);
                Step::Io(io)
            }
            Flow::Done(value) => Step::Done(value),
        }
    }
}

/// Folds `items` in order. Items that make no request do not deepen the stack.
pub(crate) fn fold<'a, I, A, F>(mut items: I, mut acc: A, mut f: F) -> Fallible<'a, A>
where
    I: Iterator + 'a,
    A: 'a,
    F: FnMut(A, I::Item) -> Fallible<'a, A> + 'a,
{
    loop {
        let Some(item) = items.next() else {
            return ok(acc);
        };
        match f(acc, item) {
            Flow::Done(Ok(next)) => acc = next,
            Flow::Done(Err(error)) => return Flow::Done(Err(error)),
            flow => return flow.and_then(move |acc| fold(items, acc, f)),
        }
    }
}

pub(crate) fn each<'a, I, F>(items: I, mut f: F) -> Fallible<'a, ()>
where
    I: Iterator + 'a,
    F: FnMut(I::Item) -> Fallible<'a, ()> + 'a,
{
    fold(items, (), move |(), item| f(item))
}

/// `io` with its result as it came.
pub(crate) fn attempt<'a>(io: Io) -> Flow<'a, IoResult> {
    Flow::io(io, Flow::Done)
}

fn ask<'a, T: 'a>(io: Io, pick: fn(Reply) -> Option<T>) -> Fallible<'a, T> {
    let (root, path) = (io.root(), io.path().clone());
    Flow::io(io, move |result| {
        let picked = result.and_then(|reply| {
            pick(reply).ok_or_else(|| IoError::Other("the backend gave the wrong reply".into()))
        });
        Flow::Done(picked.map_err(|error| Error::Io { root, path, error }))
    })
}

/// A mutating request, failing the flow with its error.
pub(crate) fn act<'a>(io: Io) -> Fallible<'a, ()> {
    ask(io, |reply| matches!(reply, Reply::Done).then_some(()))
}

pub(crate) fn stat<'a>(root: Root, path: &RelPath) -> Fallible<'a, Option<Meta>> {
    let io = Io::Stat {
        root,
        path: path.clone(),
    };
    ask(io, |reply| match reply {
        Reply::Stat(meta) => Some(meta),
        _ => None,
    })
}

pub(crate) fn read<'a>(root: Root, path: &RelPath, range: Range) -> Fallible<'a, Vec<u8>> {
    let io = Io::Read {
        root,
        path: path.clone(),
        range,
    };
    ask(io, |reply| match reply {
        Reply::Bytes(bytes) => Some(bytes),
        _ => None,
    })
}

/// A directory's entries; none when it does not exist.
pub(crate) fn list<'a>(root: Root, dir: &RelPath) -> Fallible<'a, Vec<DirEntry>> {
    let failed = Io::List {
        root,
        dir: dir.clone(),
    };
    attempt(failed.clone()).then(move |result| match result {
        Ok(Reply::Listed(entries)) => ok(entries),
        Err(IoError::NotFound) => ok(Vec::new()),
        Err(error) => Flow::Done(Err(failed.failed(error))),
        Ok(_) => Flow::Done(Err(
            failed.failed(IoError::Other("the backend gave the wrong reply".into()))
        )),
    })
}

pub(crate) fn sync<'a>(root: Root, path: &RelPath) -> Fallible<'a, ()> {
    act(Io::Sync {
        root,
        path: path.clone(),
    })
}

/// Syncs the directory holding `path`.
pub(crate) fn sync_parent<'a>(root: Root, path: &RelPath) -> Fallible<'a, ()> {
    sync(root, &path.parent().unwrap_or_default())
}

/// Creates `path` durably: the file's contents, then its name.
pub(crate) fn create<'a>(root: Root, path: &RelPath, bytes: Vec<u8>) -> Fallible<'a, ()> {
    let (synced, named) = (path.clone(), path.clone());
    act(Io::Create {
        root,
        path: path.clone(),
        bytes,
    })
    .and_then(move |()| sync(root, &synced))
    .and_then(move |()| sync_parent(root, &named))
}

/// Moves `from` to `to`, where nothing is, and makes both directories durable,
/// the destination's first, so the source's name is gone only once the
/// destination's is durable. `to` is checked first, so a backend whose renames
/// may replace replaces only what appears between the check and the rename.
pub(crate) fn rename<'a>(root: Root, from: &RelPath, to: &RelPath) -> Fallible<'a, ()> {
    let (from, to) = (from.clone(), to.clone());
    ensure_dir(root, &to.parent().unwrap_or_default())
        .and_then({
            let to = to.clone();
            move |()| stat(root, &to)
        })
        .and_then({
            let (from, to) = (from.clone(), to.clone());
            move |found| {
                let io = Io::Rename { root, from, to };
                match found {
                    Some(_) => Flow::Done(Err(io.failed(IoError::AlreadyExists))),
                    None => act(io),
                }
            }
        })
        .and_then({
            let to = to.clone();
            move |()| sync_parent(root, &to)
        })
        .and_then(move |()| sync_parent(root, &from))
}

/// Removes the file at `path` and makes that durable; nothing there is success.
pub(crate) fn remove<'a>(root: Root, path: &RelPath) -> Fallible<'a, ()> {
    let io = Io::Remove {
        root,
        path: path.clone(),
    };
    let parent = path.clone();
    attempt(io.clone()).then(move |result| match result {
        Ok(_) => sync_parent(root, &parent),
        Err(IoError::NotFound) => ok(()),
        Err(error) => Flow::Done(Err(io.failed(error))),
    })
}

/// Makes `dir` exist durably: creates what is missing of it and syncs the
/// directory holding each name it created.
pub(crate) fn ensure_dir<'a>(root: Root, dir: &RelPath) -> Fallible<'a, ()> {
    let mut chain: Vec<RelPath> =
        std::iter::successors(Some(dir.clone()), RelPath::parent).collect();
    chain.reverse();
    let probe = chain.clone();
    fold(
        probe.into_iter().enumerate(),
        None,
        move |missing, (i, path)| {
            if missing.is_some() {
                return ok(missing);
            }
            stat(root, &path).and_then(move |meta| match meta.map(|meta| meta.kind) {
                Some(Kind::Directory) => ok(None),
                Some(Kind::File) => Flow::Done(Err(Error::Io {
                    root,
                    path,
                    error: IoError::NotDirectory,
                })),
                None => ok(Some(i)),
            })
        },
    )
    .and_then(move |missing| {
        let Some(first) = missing else {
            return ok(());
        };
        let dir = chain.last().cloned().unwrap_or_default();
        let named = chain[first.saturating_sub(1)..chain.len() - 1].to_vec();
        act(Io::MakeDir { root, path: dir })
            .and_then(move |()| each(named.into_iter(), move |parent| sync(root, &parent)))
    })
}

/// A removal of something that may already be gone, without syncing.
pub(crate) fn remove_if_present<'a>(root: Root, path: RelPath) -> Fallible<'a, ()> {
    let io = Io::Remove { root, path };
    attempt(io.clone()).then(move |result| match result {
        Ok(_) | Err(IoError::NotFound) => ok(()),
        Err(error) => Flow::Done(Err(io.failed(error))),
    })
}

/// The bytes of `range`; `None` when the file is not there.
pub(crate) fn read_present<'a>(
    root: Root,
    path: &RelPath,
    range: Range,
) -> Fallible<'a, Option<Vec<u8>>> {
    let io = Io::Read {
        root,
        path: path.clone(),
        range,
    };
    attempt(io.clone()).then(move |result| match result {
        Ok(Reply::Bytes(bytes)) => ok(Some(bytes)),
        Err(IoError::NotFound) => ok(None),
        Err(error) => Flow::Done(Err(io.failed(error))),
        Ok(_) => Flow::Done(Err(
            io.failed(IoError::Other("the backend gave the wrong reply".into()))
        )),
    })
}

/// At most `max` bytes of a file from its start; `None` when no file is there.
pub(crate) fn read_file<'a>(root: Root, path: &RelPath, max: u64) -> Fallible<'a, Option<Vec<u8>>> {
    let path = path.clone();
    stat(root, &path).and_then(move |meta| match meta {
        Some(meta) if meta.kind == Kind::File => read_present(
            root,
            &path,
            Range {
                offset: 0,
                len: meta.len.min(max),
            },
        ),
        _ => ok(None),
    })
}

pub(crate) fn lock<'a>(name: RelPath) -> Fallible<'a, Lock> {
    ask(Io::Lock { name }, |reply| match reply {
        Reply::Lock(lock) => Some(lock),
        _ => None,
    })
}

/// The sibling a replacement of `path` is staged in.
pub(crate) fn staged(path: &RelPath) -> RelPath {
    let parent = path.parent().expect("a replaced file has a parent");
    let name = path.name().expect("a replaced file has a name");
    parent
        .join(&format!("{name}.next"))
        .expect("a name with a suffix is one component")
}

/// Replaces the file at `path` with `bytes` without a window in which neither
/// the old nor the new bytes are durable: the new bytes are staged beside it and
/// synced first. [`read_replaced`] reads what this leaves after any crash.
pub(crate) fn replace<'a>(root: Root, path: RelPath, bytes: Vec<u8>) -> Fallible<'a, ()> {
    let next = staged(&path);
    let dir = path.parent().expect("a replaced file has a parent");
    remove_if_present(root, next.clone())
        .and_then({
            let next = next.clone();
            move |()| {
                act(Io::Create {
                    root,
                    path: next,
                    bytes,
                })
            }
        })
        .and_then({
            let next = next.clone();
            move |()| sync(root, &next)
        })
        .and_then({
            let path = path.clone();
            move |()| remove_if_present(root, path)
        })
        .and_then(move |()| {
            act(Io::Rename {
                root,
                from: next,
                to: path,
            })
        })
        .and_then(move |()| sync(root, &dir))
}

/// The bytes [`replace`] last wrote at `path`; `None` when it never did.
pub(crate) fn read_replaced<'a>(root: Root, path: RelPath) -> Fallible<'a, Option<Vec<u8>>> {
    let next = staged(&path);
    read_file(root, &path, u64::MAX).and_then(move |bytes| match bytes {
        Some(bytes) => ok(Some(bytes)),
        None => read_file(root, &next, u64::MAX),
    })
}

/// The whole file at `path`, refusing one longer than `max`.
pub(crate) fn read_all<'a>(root: Root, path: &RelPath, max: u64) -> Fallible<'a, Vec<u8>> {
    let path = path.clone();
    stat(root, &path).and_then(move |meta| {
        let error = |error| {
            Flow::Done(Err(Error::Io {
                root,
                path: path.clone(),
                error,
            }))
        };
        match meta {
            None => error(IoError::NotFound),
            Some(Meta {
                kind: Kind::Directory,
                ..
            }) => error(IoError::IsDirectory),
            Some(Meta { len, .. }) if len > max => error(IoError::Other(format!(
                "the file is longer than {max} bytes"
            ))),
            Some(Meta { len, .. }) => {
                read_ranges(root, path.clone(), chunks(len)).map_ok(|parts| parts.concat())
            }
        }
    })
}

/// Copies the file `from` to a new file `to`, a chunk at a time. A source that
/// shrinks while it is copied fails the copy.
pub(crate) fn copy<'a>(root: Root, from: &RelPath, to: &RelPath) -> Fallible<'a, ()> {
    let (from, to) = (from.clone(), to.clone());
    stat(root, &from).and_then(move |meta| {
        let len = match meta {
            Some(Meta {
                kind: Kind::File,
                len,
                ..
            }) => len,
            found => {
                let error = match found {
                    None => IoError::NotFound,
                    Some(_) => IoError::IsDirectory,
                };
                return Flow::Done(Err(Error::Io {
                    root,
                    path: from,
                    error,
                }));
            }
        };
        let created = act(Io::Create {
            root,
            path: to.clone(),
            bytes: Vec::new(),
        });
        created.and_then(move |()| {
            each(chunks(len).into_iter(), move |range| {
                let to = to.clone();
                let source = from.clone();
                read(root, &from, range).and_then(move |bytes| {
                    if bytes.len() as u64 != range.len {
                        return Flow::Done(Err(Error::Io {
                            root,
                            path: source,
                            error: IoError::Other("the file shrank while it was copied".into()),
                        }));
                    }
                    act(Io::Write {
                        root,
                        path: to,
                        offset: range.offset,
                        bytes,
                    })
                })
            })
        })
    })
}

fn chunks(len: u64) -> Vec<Range> {
    (0..len.div_ceil(CHUNK))
        .map(|i| Range {
            offset: i * CHUNK,
            len: CHUNK.min(len - i * CHUNK),
        })
        .collect()
}

fn read_ranges<'a>(root: Root, path: RelPath, ranges: Vec<Range>) -> Fallible<'a, Vec<Vec<u8>>> {
    fold(ranges.into_iter(), Vec::new(), move |mut parts, range| {
        read(root, &path, range).map_ok(move |bytes| {
            parts.push(bytes);
            parts
        })
    })
}

/// A file as the app's identity function sees it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Observed {
    pub len: u64,
    pub modified: Option<u64>,
    pub identity: Identity,
}

/// What is at `path`: nothing, or a file and its identity. A directory is an error.
pub(crate) fn observe<'a>(
    root: Root,
    path: &RelPath,
    identify: &Rc<dyn Identify>,
) -> Fallible<'a, Option<Observed>> {
    let identify = Rc::clone(identify);
    let path = path.clone();
    stat(root, &path).and_then(move |meta| match meta {
        None => ok(None),
        Some(Meta {
            kind: Kind::Directory,
            ..
        }) => Flow::Done(Err(Error::Io {
            root,
            path,
            error: IoError::IsDirectory,
        })),
        Some(Meta { len, modified, .. }) => {
            identity(root, path, len, &identify).map_ok(move |identity| {
                Some(Observed {
                    len,
                    modified,
                    identity,
                })
            })
        }
    })
}

/// The identity of the file of `len` bytes at `path`.
pub(crate) fn identity<'a>(
    root: Root,
    path: RelPath,
    len: u64,
    identify: &Rc<dyn Identify>,
) -> Fallible<'a, Identity> {
    let identify = Rc::clone(identify);
    read_ranges(root, path, identify.ranges(len))
        .map_ok(move |parts| identify.identify(len, &parts))
}

/// Whether `a` and `b` are files with the same bytes.
pub(crate) fn same_bytes<'a>(root: Root, a: &RelPath, b: &RelPath) -> Fallible<'a, bool> {
    let (a, b) = (a.clone(), b.clone());
    stat(root, &a).and_then(move |first| {
        stat(root, &b).and_then(move |second| match (first, second) {
            (Some(first), Some(second))
                if first.kind == Kind::File
                    && second.kind == Kind::File
                    && first.len == second.len =>
            {
                compare(root, a, b, chunks(first.len).into())
            }
            _ => ok(false),
        })
    })
}

fn compare<'a>(
    root: Root,
    a: RelPath,
    b: RelPath,
    mut ranges: VecDeque<Range>,
) -> Fallible<'a, bool> {
    let Some(range) = ranges.pop_front() else {
        return ok(true);
    };
    read(root, &a, range).and_then(move |first| {
        read(root, &b, range).and_then(move |second| match first == second {
            true => compare(root, a, b, ranges),
            false => ok(false),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocking;
    use crate::disk::MemDisk;

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn run<'a, T: 'a>(disk: &MemDisk, flow: Flow<'a, T>) -> T {
        blocking::run(&mut disk.clone(), flow.task())
    }

    #[test]
    fn a_directory_made_where_some_of_it_exists_survives_a_crash() {
        let disk = MemDisk::new();
        run(&disk, ensure_dir(Root::Folder, &path("a"))).unwrap();
        let before = disk.mutations();
        run(&disk, ensure_dir(Root::Folder, &path("a/b/c"))).unwrap();
        assert_eq!(
            disk.mutations() - before,
            3,
            "one request to make, two syncs"
        );
        run(&disk, ensure_dir(Root::Folder, &path("a/b/c"))).unwrap();
        assert_eq!(
            disk.mutations() - before,
            3,
            "nothing to do the second time"
        );
        let dirs = disk.restart().directories(Root::Folder);
        assert_eq!(dirs, [path("a"), path("a/b"), path("a/b/c")].into());
    }

    #[test]
    fn files_longer_than_one_read_are_read_and_compared_whole() {
        let disk = MemDisk::new();
        let long: Vec<u8> = (0..CHUNK * 2 + 5).map(|i| i as u8).collect();
        let mut late = long.clone();
        *late.last_mut().unwrap() ^= 1;
        for (name, bytes) in [("a", &long), ("b", &long), ("c", &late)] {
            run(&disk, create(Root::Folder, &path(name), bytes.clone())).unwrap();
        }
        assert_eq!(
            run(&disk, read_all(Root::Folder, &path("a"), u64::MAX)).unwrap(),
            long
        );
        assert!(
            run(&disk, read_all(Root::Folder, &path("a"), CHUNK)).is_err(),
            "too long"
        );
        assert!(run(&disk, same_bytes(Root::Folder, &path("a"), &path("b"))).unwrap());
        assert!(!run(&disk, same_bytes(Root::Folder, &path("a"), &path("c"))).unwrap());
        assert!(!run(&disk, same_bytes(Root::Folder, &path("a"), &path("none"))).unwrap());
    }
}
