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

/// A directory's entries with what is at each; none when it does not exist.
pub(crate) fn list_stat<'a>(root: Root, dir: &RelPath) -> Fallible<'a, Vec<(String, Meta)>> {
    let failed = Io::ListStat {
        root,
        dir: dir.clone(),
    };
    attempt(failed.clone()).then(move |result| match result {
        Ok(Reply::ListedStat(entries)) => ok(entries),
        Err(IoError::NotFound) => ok(Vec::new()),
        Err(error) => Flow::Done(Err(failed.failed(error))),
        Ok(_) => Flow::Done(Err(
            failed.failed(IoError::Other("the backend gave the wrong reply".into()))
        )),
    })
}

/// The most bytes one [`Io::ReadMany`] asks for, unless one read alone asks for
/// more.
const MANY: u64 = 64 * CHUNK;

/// The bytes of each of `reads`, in order; `None` where no file is there. The
/// reads go in as few [`Io::ReadMany`] requests as [`MANY`] allows.
pub(crate) fn read_many<'a>(
    root: Root,
    reads: Vec<(RelPath, Range)>,
) -> Fallible<'a, Vec<Option<Vec<u8>>>> {
    let mut batches: Vec<Vec<(RelPath, Range)>> = Vec::new();
    let mut asked = 0u64;
    for read in reads {
        let len = read.1.len;
        match batches.last_mut() {
            Some(batch) if asked.saturating_add(len) <= MANY => {
                asked += len;
                batch.push(read);
            }
            _ => {
                asked = len;
                batches.push(vec![read]);
            }
        }
    }
    fold(batches.into_iter(), Vec::new(), move |mut all, reads| {
        let paths: Vec<RelPath> = reads.iter().map(|(path, _)| path.clone()).collect();
        let io = Io::ReadMany { root, reads };
        attempt(io.clone()).then(move |result| {
            let read = match result {
                Ok(Reply::ReadMany(read)) if read.len() == paths.len() => read,
                Ok(_) => {
                    let wrong = IoError::Other("the backend gave the wrong reply".into());
                    return Flow::Done(Err(io.failed(wrong)));
                }
                Err(error) => return Flow::Done(Err(io.failed(error))),
            };
            for (path, read) in paths.into_iter().zip(read) {
                match read {
                    Ok(bytes) => all.push(Some(bytes)),
                    Err(IoError::NotFound) => all.push(None),
                    Err(error) => return Flow::Done(Err(Error::Io { root, path, error })),
                }
            }
            ok(all)
        })
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

/// Moves `from` to `to`, where nothing is, and makes both directories durable,
/// the destination's first, so the source's name is gone only once the
/// destination's is durable. `to` is checked first, so a backend whose renames
/// may replace replaces only what appears between the check and the rename.
pub(crate) fn rename<'a>(root: Root, from: &RelPath, to: &RelPath) -> Fallible<'a, ()> {
    rename_all(root, vec![(from.clone(), to.clone())])
}

/// Moves each of `moves` as [`rename`] does, all of them out of one directory and
/// into one directory, then syncs the destination's directory and the source's
/// once for all of them. A move that fails stops the rest and fails the run, once
/// the moves before it are synced.
pub(crate) fn rename_all<'a>(root: Root, moves: Vec<(RelPath, RelPath)>) -> Fallible<'a, ()> {
    let Some((from, to)) = moves.first() else {
        return ok(());
    };
    let from_dir = from.parent().unwrap_or_default();
    let to_dir = to.parent().unwrap_or_default();
    ensure_dir(root, &to_dir)
        .and_then(move |()| {
            fold(
                moves.into_iter(),
                (0, None),
                move |(moved, failed): (usize, Option<Error>), (from, to)| {
                    if failed.is_some() {
                        return ok((moved, failed));
                    }
                    move_to_absent(root, from, to).then(move |result| match result {
                        Ok(()) => ok((moved + 1, None)),
                        Err(error) => ok((moved, Some(error))),
                    })
                },
            )
        })
        .and_then(move |(moved, failed)| {
            let synced = match moved {
                0 => ok(()),
                _ => sync(root, &to_dir).and_then(move |()| match from_dir == to_dir {
                    true => ok(()),
                    false => sync(root, &from_dir),
                }),
            };
            synced.and_then(move |()| Flow::Done(failed.map_or(Ok(()), Err)))
        })
}

fn move_to_absent<'a>(root: Root, from: RelPath, to: RelPath) -> Fallible<'a, ()> {
    stat(root, &to).and_then(move |found| {
        let io = Io::Rename { root, from, to };
        match found {
            Some(_) => Flow::Done(Err(io.failed(IoError::AlreadyExists))),
            None => act(io),
        }
    })
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

/// Copies the file `from` to a new file `to`, a chunk at a time. Something at
/// `to` refuses it. A source that shrinks while it is copied fails the copy, and a
/// copy that fails once `to` is made removes it.
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
        let made = to.clone();
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
            .then(move |copied| match copied {
                Ok(()) => ok(()),
                Err(error) => remove_if_present(root, made).then(move |_| Flow::Done(Err(error))),
            })
        })
    })
}

/// Moves the file `from` to `to`, where nothing is, without a rename: copies it a
/// chunk at a time, syncs the copy and then its directory, and only then removes
/// `from` and syncs its directory. Until then `to` may hold the start of `from`'s
/// bytes.
pub(crate) fn copy_move<'a>(root: Root, from: &RelPath, to: &RelPath) -> Fallible<'a, ()> {
    let (from, to) = (from.clone(), to.clone());
    let dir = to.parent().unwrap_or_default();
    ensure_dir(root, &dir)
        .and_then({
            let (from, to) = (from.clone(), to.clone());
            move |()| copy(root, &from, &to)
        })
        .and_then(move |()| sync(root, &to))
        .and_then(move |()| sync(root, &dir))
        .and_then(move |()| remove(root, &from))
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

/// What is at each of `paths`, as [`observe`] says, the identities read together.
pub(crate) fn observe_all<'a>(
    root: Root,
    paths: Vec<RelPath>,
    identify: &Rc<dyn Identify>,
) -> Fallible<'a, Vec<Option<Observed>>> {
    let identify = Rc::clone(identify);
    fold(paths.into_iter(), Vec::new(), move |mut found, path| {
        stat(root, &path).and_then(move |meta| match meta {
            Some(Meta {
                kind: Kind::Directory,
                ..
            }) => Flow::Done(Err(Error::Io {
                root,
                path,
                error: IoError::IsDirectory,
            })),
            meta => {
                found.push((path, meta));
                ok(found)
            }
        })
    })
    .and_then(move |found| {
        let files = found
            .iter()
            .filter_map(|(path, meta)| Some((path.clone(), meta.as_ref()?.len)));
        identities(root, files.collect(), &identify).map_ok(move |identities| {
            let mut identities = identities.into_iter();
            found
                .into_iter()
                .map(|(_, meta)| {
                    let meta = meta?;
                    let identity = identities.next().expect("an identity for each file")?;
                    Some(Observed {
                        len: meta.len,
                        modified: meta.modified,
                        identity,
                    })
                })
                .collect()
        })
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

/// The identity of each file of `files`, given its length, read together; `None`
/// for a file no longer there.
pub(crate) fn identities<'a>(
    root: Root,
    files: Vec<(RelPath, u64)>,
    identify: &Rc<dyn Identify>,
) -> Fallible<'a, Vec<Option<Identity>>> {
    let identify = Rc::clone(identify);
    let ranges: Vec<Vec<Range>> = files.iter().map(|(_, len)| identify.ranges(*len)).collect();
    let reads = files
        .iter()
        .zip(&ranges)
        .flat_map(|((path, _), ranges)| ranges.iter().map(move |range| (path.clone(), *range)));
    read_many(root, reads.collect()).map_ok(move |parts| {
        let mut parts = parts.into_iter();
        files
            .into_iter()
            .zip(ranges)
            .map(|((_, len), ranges)| {
                let read: Vec<Option<Vec<u8>>> = parts.by_ref().take(ranges.len()).collect();
                let read: Option<Vec<Vec<u8>>> = read.into_iter().collect();
                read.map(|parts| identify.identify(len, &parts))
            })
            .collect()
    })
}

/// What one file holds of another's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Held {
    /// All of them, and nothing more.
    All,
    /// Fewer of them, from the start, as a copy cut short leaves.
    Start,
    Other,
}

/// What the file `to` holds of the file `from`'s bytes; [`Held::Other`] unless
/// both are files.
pub(crate) fn held<'a>(root: Root, from: &RelPath, to: &RelPath) -> Fallible<'a, Held> {
    let (from, to) = (from.clone(), to.clone());
    stat(root, &from).and_then(move |whole| {
        stat(root, &to).and_then(move |part| match (whole, part) {
            (Some(whole), Some(part))
                if whole.kind == Kind::File && part.kind == Kind::File && part.len <= whole.len =>
            {
                let held = match part.len == whole.len {
                    true => Held::All,
                    false => Held::Start,
                };
                compare(root, from, to, chunks(part.len).into()).map_ok(move |same| {
                    if same {
                        held
                    } else {
                        Held::Other
                    }
                })
            }
            _ => ok(Held::Other),
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
            let create = Io::Create {
                root: Root::Folder,
                path: path(name),
                bytes: bytes.clone(),
            };
            run(&disk, act(create)).unwrap();
        }
        assert_eq!(
            run(&disk, read_all(Root::Folder, &path("a"), u64::MAX)).unwrap(),
            long
        );
        assert!(
            run(&disk, read_all(Root::Folder, &path("a"), CHUNK)).is_err(),
            "too long"
        );
        let held = |a: &str, b: &str| run(&disk, held(Root::Folder, &path(a), &path(b))).unwrap();
        assert_eq!(held("a", "b"), Held::All);
        assert_eq!(held("a", "c"), Held::Other);
        assert_eq!(held("a", "none"), Held::Other);
        let start = Io::Create {
            root: Root::Folder,
            path: path("start"),
            bytes: long[..CHUNK as usize + 1].to_vec(),
        };
        run(&disk, act(start)).unwrap();
        assert_eq!(held("a", "start"), Held::Start);
        assert_eq!(held("start", "a"), Held::Other, "longer than its source");
        assert_eq!(held("c", "start"), Held::Start);
    }

    #[test]
    fn a_copy_that_fails_once_it_made_its_destination_removes_it() {
        let disk = MemDisk::new();
        let bytes = vec![7; 10];
        let create = Io::Create {
            root: Root::Folder,
            path: path("a"),
            bytes,
        };
        run(&disk, act(create)).unwrap();
        disk.set_capacity(Root::Folder, Some(15));
        let copied = run(&disk, copy(Root::Folder, &path("a"), &path("b")));
        assert!(
            matches!(
                copied,
                Err(Error::Io {
                    error: IoError::NoSpace,
                    ..
                })
            ),
            "{copied:?}"
        );
        assert_eq!(
            disk.files(Root::Folder).into_keys().collect::<Vec<_>>(),
            [path("a")]
        );
    }

    #[test]
    fn a_move_by_copy_removes_its_source_only_once_the_copy_is_durable() {
        let setup = || {
            let disk = MemDisk::new();
            let long: Vec<u8> = (0..CHUNK + 5).map(|i| i as u8).collect();
            run(&disk, ensure_dir(Root::Folder, &path("d"))).unwrap();
            let create = Io::Create {
                root: Root::Folder,
                path: path("d/a"),
                bytes: long.clone(),
            };
            run(&disk, act(create)).unwrap();
            run(&disk, sync(Root::Folder, &path("d/a"))).unwrap();
            run(&disk, sync(Root::Folder, &path("d"))).unwrap();
            (disk, long)
        };
        let (disk, long) = setup();
        let before = disk.mutations();
        run(&disk, copy_move(Root::Folder, &path("d/a"), &path("e/b"))).unwrap();
        let operations = disk.mutations() - before;
        for after in 0..=operations {
            let (disk, _) = setup();
            disk.crash_after(after);
            let _ = run(&disk, copy_move(Root::Folder, &path("d/a"), &path("e/b")));
            let files = disk.restart().files(Root::Folder);
            let whole = |at: &str| files.get(&path(at)) == Some(&long);
            assert!(
                whole("d/a") || whole("e/b"),
                "after {after}: {:?}",
                files.keys()
            );
        }
    }
}
