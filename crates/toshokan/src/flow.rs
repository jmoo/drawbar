//! Operations written as a sequence of requests, each with what to do with its
//! result, and run as a [`Task`].

use crate::error::{Error, Result};
use crate::io::{
    DirEntry, Io, IoError, IoResult, Lock, Meta, Operation, Range, Reply, Root, Step, Task,
};
use crate::path::RelPath;

type Resume<'a, T> = Box<dyn FnOnce(IoResult) -> Flow<'a, T> + 'a>;

pub(crate) enum Flow<'a, T> {
    Io(Io, Resume<'a, T>),
    Done(T),
}

impl<'a, T: 'a> Flow<'a, T> {
    pub(crate) fn then<U: 'a>(self, next: impl FnOnce(T) -> Flow<'a, U> + 'a) -> Flow<'a, U> {
        match self {
            Self::Done(value) => next(value),
            Self::Io(io, resume) => Flow::Io(io, Box::new(move |result| resume(result).then(next))),
        }
    }

    pub(crate) fn task(self) -> Task<'a, T> {
        Task::new(Run {
            flow: Some(self),
            resume: None,
        })
    }
}

impl<'a, T: 'a> Flow<'a, Result<T>> {
    pub(crate) fn ok(value: T) -> Self {
        Self::Done(Ok(value))
    }

    pub(crate) fn and_then<U: 'a>(
        self,
        next: impl FnOnce(T) -> Flow<'a, Result<U>> + 'a,
    ) -> Flow<'a, Result<U>> {
        self.then(|result| match result {
            Ok(value) => next(value),
            Err(error) => Flow::Done(Err(error)),
        })
    }

    pub(crate) fn map_ok<U: 'a>(self, map: impl FnOnce(T) -> U + 'a) -> Flow<'a, Result<U>> {
        self.and_then(|value| Flow::ok(map(value)))
    }
}

struct Run<'a, T> {
    flow: Option<Flow<'a, T>>,
    resume: Option<Resume<'a, T>>,
}

impl<T> Operation for Run<'_, T> {
    type Output = T;

    fn resume(&mut self, result: Option<IoResult>) -> Step<T> {
        let flow = match (result, self.resume.take()) {
            (Some(result), Some(resume)) => resume(result),
            _ => self.flow.take().expect("resumed after it was done"),
        };
        match flow {
            Flow::Done(value) => Step::Done(value),
            Flow::Io(io, resume) => {
                self.resume = Some(resume);
                Step::Io(io)
            }
        }
    }
}

/// Runs `step` on each item in order, threading `state`, and stops at the first
/// error.
pub(crate) fn fold<'a, I: 'a, S: 'a>(
    items: Vec<I>,
    state: S,
    step: impl FnMut(S, I) -> Flow<'a, Result<S>> + 'a,
) -> Flow<'a, Result<S>> {
    fold_rest(items.into_iter(), state, step)
}

fn fold_rest<'a, I: 'a, S: 'a, F>(
    mut items: std::vec::IntoIter<I>,
    state: S,
    mut step: F,
) -> Flow<'a, Result<S>>
where
    F: FnMut(S, I) -> Flow<'a, Result<S>> + 'a,
{
    match items.next() {
        None => Flow::ok(state),
        Some(item) => step(state, item).and_then(move |state| fold_rest(items, state, step)),
    }
}

/// `io`, with a failure as the [`Error::Io`] naming its root and path.
pub(crate) fn request<'a>(io: Io) -> Flow<'a, Result<Reply>> {
    let (root, path) = (io.root(), io.path().clone());
    Flow::Io(
        io,
        Box::new(move |result| Flow::Done(result.map_err(|error| Error::Io { root, path, error }))),
    )
}

/// `io`, with [`IoError::NotFound`] as `None`.
fn request_found<'a>(io: Io) -> Flow<'a, Result<Option<Reply>>> {
    let (root, path) = (io.root(), io.path().clone());
    Flow::Io(
        io,
        Box::new(move |result| {
            Flow::Done(match result {
                Ok(reply) => Ok(Some(reply)),
                Err(IoError::NotFound) => Ok(None),
                Err(error) => Err(Error::Io { root, path, error }),
            })
        }),
    )
}

fn unexpected(root: Root, path: RelPath, reply: &Reply) -> Error {
    Error::Io {
        root,
        path,
        error: IoError::Other(format!("unexpected reply {reply:?}")),
    }
}

/// Any request whose reply carries nothing.
pub(crate) fn done<'a>(io: Io) -> Flow<'a, Result<()>> {
    request(io).map_ok(|_| ())
}

/// A removal of something that may already be gone.
pub(crate) fn remove_if_present<'a>(root: Root, path: RelPath) -> Flow<'a, Result<()>> {
    request_found(Io::Remove { root, path }).map_ok(|_| ())
}

/// The entries of `dir`; `None` when it is not there.
pub(crate) fn list<'a>(root: Root, dir: RelPath) -> Flow<'a, Result<Option<Vec<DirEntry>>>> {
    let io = Io::List {
        root,
        dir: dir.clone(),
    };
    request_found(io).then(move |result| {
        Flow::Done(result.and_then(|reply| match reply {
            None => Ok(None),
            Some(Reply::Listed(entries)) => Ok(Some(entries)),
            Some(other) => Err(unexpected(root, dir, &other)),
        }))
    })
}

pub(crate) fn stat<'a>(root: Root, path: RelPath) -> Flow<'a, Result<Option<Meta>>> {
    let io = Io::Stat {
        root,
        path: path.clone(),
    };
    request(io).then(move |result| {
        Flow::Done(result.and_then(|reply| match reply {
            Reply::Stat(meta) => Ok(meta),
            other => Err(unexpected(root, path, &other)),
        }))
    })
}

/// The bytes of `range`; `None` when the file is not there.
pub(crate) fn read<'a>(
    root: Root,
    path: RelPath,
    range: Range,
) -> Flow<'a, Result<Option<Vec<u8>>>> {
    let io = Io::Read {
        root,
        path: path.clone(),
        range,
    };
    request_found(io).then(move |result| {
        Flow::Done(result.and_then(|reply| match reply {
            None => Ok(None),
            Some(Reply::Bytes(bytes)) => Ok(Some(bytes)),
            Some(other) => Err(unexpected(root, path, &other)),
        }))
    })
}

/// At most `max` bytes of a file from its start; `None` when it is not there.
pub(crate) fn read_file<'a>(
    root: Root,
    path: RelPath,
    max: u64,
) -> Flow<'a, Result<Option<Vec<u8>>>> {
    stat(root, path.clone()).and_then(move |meta| match meta {
        Some(meta) if meta.kind == crate::io::Kind::File => read(
            root,
            path,
            Range {
                offset: 0,
                len: meta.len.min(max),
            },
        ),
        _ => Flow::ok(None),
    })
}

pub(crate) fn lock<'a>(name: RelPath) -> Flow<'a, Result<Lock>> {
    let path = name.clone();
    request(Io::Lock { name }).then(move |result| {
        Flow::Done(result.and_then(|reply| match reply {
            Reply::Lock(lock) => Ok(lock),
            other => Err(unexpected(Root::Local, path, &other)),
        }))
    })
}

/// The sibling a replacement of `path` is staged in.
fn staged(path: &RelPath) -> RelPath {
    let parent = path.parent().expect("a replaced file has a parent");
    let name = path.name().expect("a replaced file has a name");
    parent
        .join(&format!("{name}.next"))
        .expect("a name with a suffix is one component")
}

/// Replaces the file at `path` with `bytes` without a window in which neither
/// the old nor the new bytes are durable: the new bytes are staged beside it and
/// synced first. [`read_replaced`] reads what this leaves after any crash.
pub(crate) fn replace<'a>(root: Root, path: RelPath, bytes: Vec<u8>) -> Flow<'a, Result<()>> {
    let next = staged(&path);
    let dir = path.parent().expect("a replaced file has a parent");
    remove_if_present(root, next.clone())
        .and_then({
            let next = next.clone();
            move |()| {
                done(Io::Create {
                    root,
                    path: next,
                    bytes,
                })
            }
        })
        .and_then({
            let next = next.clone();
            move |()| done(Io::Sync { root, path: next })
        })
        .and_then({
            let path = path.clone();
            move |()| remove_if_present(root, path)
        })
        .and_then(move |()| {
            done(Io::Rename {
                root,
                from: next,
                to: path,
            })
        })
        .and_then(move |()| done(Io::Sync { root, path: dir }))
}

/// The bytes [`replace`] last wrote at `path`; `None` when it never did.
pub(crate) fn read_replaced<'a>(root: Root, path: RelPath) -> Flow<'a, Result<Option<Vec<u8>>>> {
    let next = staged(&path);
    read_file(root, path, u64::MAX).and_then(move |bytes| match bytes {
        Some(bytes) => Flow::ok(Some(bytes)),
        None => read_file(root, next, u64::MAX),
    })
}
