//! The boundary between the pure core and a driver.
//!
//! An [`Operation`] never touches a file system. It returns [`Step::Io`] with a
//! request, the driver performs it and resumes the operation with the
//! [`IoResult`], until the operation returns [`Step::Done`]. The same operation runs
//! under the blocking driver, the async driver, the in-memory disk and the
//! simulator, and replays exactly.

use std::fmt;

use thiserror::Error as ThisError;

use crate::error::Error;
use crate::path::RelPath;
use crate::plan::Content;

/// The most bytes the core and the drivers read or copy in one request.
pub const CHUNK: u64 = 1 << 20;

/// The two trees an operation addresses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Root {
    /// The user's library folder, synced and shared between writers.
    Folder,
    /// This install's own storage for the library: never synced, never shared.
    Local,
}

/// Bytes from `offset`, at most `len` of them; fewer only at the end of the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Range {
    pub offset: u64,
    pub len: u64,
}

/// One request to a driver. Every mutating request either completes or has no
/// effect the requester can observe; whether a completed one survives a crash is
/// the business of [`Io::Sync`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Io {
    /// The names in a directory, sorted. Symbolic links and special files are left
    /// out.
    List { root: Root, dir: RelPath },
    /// What is at a path; `None` when nothing is.
    Stat { root: Root, path: RelPath },
    /// What [`Io::List`] and an [`Io::Stat`] of each name would say, in one
    /// request. A name gone before it is stated is left out.
    ListStat { root: Root, dir: RelPath },
    Read {
        root: Root,
        path: RelPath,
        range: Range,
    },
    /// Several reads in one request, each answered as [`Io::Read`] would answer it
    /// alone, in order.
    ReadMany {
        root: Root,
        reads: Vec<(RelPath, Range)>,
    },
    /// A new file holding `bytes`, in an existing directory. Never replaces.
    Create {
        root: Root,
        path: RelPath,
        bytes: Vec<u8>,
    },
    /// `bytes` added to the end of an existing file.
    Append {
        root: Root,
        path: RelPath,
        bytes: Vec<u8>,
    },
    /// `bytes` written at `offset` of an existing file, over what is there and past
    /// its end, zeros filling any gap. A write that fails may have written part of
    /// `bytes`.
    Write {
        root: Root,
        path: RelPath,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// The empty file at `path` filled with the app's `content`, which the driver
    /// holds: the driver writes it with [`Io::Write`] requests to its backend, so its
    /// bytes never pass through the core. A backend refuses it.
    Fill {
        root: Root,
        path: RelPath,
        content: Content,
    },
    /// A file or directory moved to a path where nothing is, never inside itself.
    /// Something at `to` refuses it with [`IoError::AlreadyExists`], atomically
    /// where the backend declares [`Capabilities::no_replace`]; elsewhere the
    /// backend may replace it, and the core checks `to` first.
    Rename {
        root: Root,
        from: RelPath,
        to: RelPath,
    },
    /// Removes a file.
    Remove { root: Root, path: RelPath },
    /// Removes an empty directory.
    RemoveDir { root: Root, path: RelPath },
    /// Creates a directory and any missing ancestors. An existing directory is
    /// success.
    MakeDir { root: Root, path: RelPath },
    /// Makes durable a file's contents, or a directory's names. A new or renamed
    /// name is durable only once its directory is synced; a rename across
    /// directories needs both. The drivers answer it themselves, without asking
    /// a backend that does not declare [`Capabilities::fsync`].
    Sync { root: Root, path: RelPath },
    /// Takes the lock `name` in the local root for this process, without waiting.
    /// A lock held by another process answers [`Reply::Lock`]`(`[`Lock::Held`]`)`.
    /// Locks are released by [`Io::Unlock`] or when the process ends. The backend
    /// may create an empty file at `name` to hold the lock.
    Lock { name: RelPath },
    /// Releases a lock this process holds; releasing one it does not hold is
    /// success.
    Unlock { name: RelPath },
}

impl Io {
    /// The tree the request addresses.
    pub fn root(&self) -> Root {
        match self {
            Self::List { root, .. }
            | Self::Stat { root, .. }
            | Self::ListStat { root, .. }
            | Self::Read { root, .. }
            | Self::ReadMany { root, .. }
            | Self::Create { root, .. }
            | Self::Append { root, .. }
            | Self::Write { root, .. }
            | Self::Fill { root, .. }
            | Self::Rename { root, .. }
            | Self::Remove { root, .. }
            | Self::RemoveDir { root, .. }
            | Self::MakeDir { root, .. }
            | Self::Sync { root, .. } => *root,
            Self::Lock { .. } | Self::Unlock { .. } => Root::Local,
        }
    }

    /// The path the request is about; for a rename, its destination; for several
    /// reads, the first one's.
    pub fn path(&self) -> &RelPath {
        static ROOT: RelPath = RelPath::ROOT;
        match self {
            Self::ReadMany { reads, .. } => reads.first().map_or(&ROOT, |(path, _)| path),
            Self::List { dir: path, .. }
            | Self::ListStat { dir: path, .. }
            | Self::Stat { path, .. }
            | Self::Read { path, .. }
            | Self::Create { path, .. }
            | Self::Append { path, .. }
            | Self::Write { path, .. }
            | Self::Fill { path, .. }
            | Self::Rename { to: path, .. }
            | Self::Remove { path, .. }
            | Self::RemoveDir { path, .. }
            | Self::MakeDir { path, .. }
            | Self::Sync { path, .. }
            | Self::Lock { name: path }
            | Self::Unlock { name: path } => path,
        }
    }

    /// Whether the request can change what is stored. Locks do not count.
    pub fn mutates(&self) -> bool {
        match self {
            Self::List { .. }
            | Self::Stat { .. }
            | Self::ListStat { .. }
            | Self::Read { .. }
            | Self::ReadMany { .. }
            | Self::Lock { .. }
            | Self::Unlock { .. } => false,
            Self::Create { .. }
            | Self::Append { .. }
            | Self::Write { .. }
            | Self::Fill { .. }
            | Self::Rename { .. }
            | Self::Remove { .. }
            | Self::RemoveDir { .. }
            | Self::MakeDir { .. }
            | Self::Sync { .. } => true,
        }
    }

    /// The error for this request failing with `error`.
    pub fn failed(&self, error: IoError) -> Error {
        Error::Io {
            root: self.root(),
            path: self.path().clone(),
            error,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    File,
    Directory,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Meta {
    pub kind: Kind,
    /// Bytes in a file; 0 for a directory.
    pub len: u64,
    /// When a file last changed, in nanoseconds since the Unix epoch where the
    /// backend reports a time. Only equality means anything: a backend may use a
    /// logical clock.
    pub modified: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DirEntry {
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lock {
    Acquired,
    /// Another live process holds it.
    Held,
}

/// What a request returned.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reply {
    /// For [`Io::List`].
    Listed(Vec<DirEntry>),
    /// For [`Io::Stat`].
    Stat(Option<Meta>),
    /// For [`Io::ListStat`]: each name with what is there, sorted by name.
    ListedStat(Vec<(String, Meta)>),
    /// For [`Io::Read`].
    Bytes(Vec<u8>),
    /// For [`Io::ReadMany`]: each read's bytes or why it failed, in order.
    ReadMany(Vec<Result<Vec<u8>, IoError>>),
    /// For [`Io::Lock`].
    Lock(Lock),
    /// For every other request.
    Done,
}

pub type IoResult = std::result::Result<Reply, IoError>;

/// Why a request failed. A failed request had no effect, except where a backend
/// declares otherwise in its [`Capabilities`].
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
pub enum IoError {
    #[error("nothing is there")]
    NotFound,
    #[error("something is already there")]
    AlreadyExists,
    #[error("a directory is there")]
    IsDirectory,
    #[error("a file is where a directory should be")]
    NotDirectory,
    #[error("the directory is not empty")]
    NotEmpty,
    #[error("no space is left")]
    NoSpace,
    #[error("a directory cannot move inside itself")]
    IntoItself,
    /// A splice keeps a range its source does not hold whole, or would make a
    /// file longer than the largest offset.
    #[error("a splice keeps a range its source does not hold")]
    SpliceRange,
    /// The backend cannot do what the request needs. Operations plan from
    /// [`Capabilities`], so reaching this is a planning bug, or a browser refusing
    /// what its backend declared from its brand.
    #[error("the backend cannot {0}")]
    Unsupported(Capability),
    /// An in-memory disk was told to crash. Every later request on it fails the same
    /// way.
    #[error("the disk crashed")]
    Crashed,
    /// Anything else the backend reported, as it described it.
    #[error("{0}")]
    Other(String),
}

/// One thing a backend may or may not be able to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capability {
    Append,
    RenameFile,
    RenameDir,
    Fsync,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Append => "append to a file",
            Self::RenameFile => "rename a file",
            Self::RenameDir => "rename a directory",
            Self::Fsync => "sync to storage",
        })
    }
}

/// What a backend can do in one root. Operations plan from this, never from errors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Capabilities {
    /// [`Io::Append`] works.
    pub append: bool,
    /// [`Io::Rename`] works on files, atomically.
    pub rename_file: bool,
    /// [`Io::Rename`] refuses an existing destination in the same step as it
    /// renames. Without it, something another program makes at the destination
    /// between the core's check and the rename is replaced.
    pub no_replace: bool,
    /// [`Io::Rename`] works on directories, atomically, with everything inside.
    pub rename_dir: bool,
    /// [`Io::Sync`] makes completed requests durable. Without it `Sync` does
    /// nothing and every completed request is as durable as the backend makes it.
    pub fsync: bool,
}

impl Capabilities {
    pub const ALL: Self = Self {
        append: true,
        rename_file: true,
        no_replace: true,
        rename_dir: true,
        fsync: true,
    };

    pub const NONE: Self = Self {
        append: false,
        rename_file: false,
        no_replace: false,
        rename_dir: false,
        fsync: false,
    };

    pub fn has(&self, capability: Capability) -> bool {
        match capability {
            Capability::Append => self.append,
            Capability::RenameFile => self.rename_file,
            Capability::RenameDir => self.rename_dir,
            Capability::Fsync => self.fsync,
        }
    }
}

/// What an operation wants next.
#[derive(Debug)]
pub enum Step<T> {
    Io(Io),
    Done(T),
}

/// A resumable operation of the core.
///
/// The driver calls [`Operation::resume`] first with `None`, then with the result
/// of each [`Io`] it returned, until it returns [`Step::Done`]. An operation keeps
/// every decision it makes in its own state, so feeding it the same results
/// produces the same requests.
pub trait Operation {
    type Output;

    /// ⚠️ Resuming after [`Step::Done`], or with a result for a request that was
    /// never made, may panic.
    fn resume(&mut self, result: Option<IoResult>) -> Step<Self::Output>;
}

/// An operation of any type, boxed, so the library's operations compose without
/// naming each other's state machines.
pub struct Task<'a, T>(Box<dyn Operation<Output = T> + 'a>);

impl<'a, T: 'a> Task<'a, T> {
    pub fn new(operation: impl Operation<Output = T> + 'a) -> Self {
        Self(Box::new(operation))
    }

    /// An operation that makes no request and returns `value`.
    pub fn ready(value: T) -> Self {
        Self::new(Ready(Some(value)))
    }
}

impl<T> Operation for Task<'_, T> {
    type Output = T;

    fn resume(&mut self, result: Option<IoResult>) -> Step<T> {
        self.0.resume(result)
    }
}

struct Ready<T>(Option<T>);

impl<T> Operation for Ready<T> {
    type Output = T;

    fn resume(&mut self, _: Option<IoResult>) -> Step<T> {
        Step::Done(self.0.take().expect("resumed after it was done"))
    }
}
