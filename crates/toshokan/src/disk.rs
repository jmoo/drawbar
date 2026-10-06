//! An in-memory disk holding both roots: deterministic, with a model of durability,
//! crashes and the faults real storage shows.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::io::{
    Capabilities, Capability, DirEntry, Io, IoError, IoResult, Kind, Lock, Meta, Range, Reply, Root,
};
use crate::path::RelPath;

/// What a crash leaves of bytes written to a file but not yet durable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tail {
    /// Nothing of them.
    #[default]
    Lost,
    /// Their first `n` bytes: a torn append.
    Torn(u64),
    /// `n` zero bytes in their place: the file grew, its data did not.
    Zeroed(u64),
}

impl Tail {
    fn keep(self, bytes: &[u8]) -> Vec<u8> {
        let kept = |n: u64| bytes.len().min(usize::try_from(n).unwrap_or(usize::MAX));
        match self {
            Self::Lost => Vec::new(),
            Self::Torn(n) => bytes[..kept(n)].to_vec(),
            Self::Zeroed(n) => vec![0; kept(n)],
        }
    }
}

/// How a file rename is carried out.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Renames {
    /// One step.
    #[default]
    Atomic,
    /// A copy, then removal of the source: two operations, so a crash between them
    /// leaves both. The copy is as durable as its source. Directories still rename
    /// in one step.
    CopyThenRemove,
}

/// An in-memory disk with a folder root and a local root.
///
/// Clones are handles of one process; [`MemDisk::process`] gives a handle of
/// another process on the same disk, as a second instance of an app would have.
/// Modification times come from a logical clock that ticks once per operation.
///
/// **Durability.** In a root whose [`Capabilities::fsync`] is set, a file's contents
/// and a directory's names each become durable only when [`Io::Sync`] names that
/// file or directory. A directory's rename becomes durable whole when either
/// directory it moved between is synced: no file system leaves a directory with
/// two names. A crash keeps exactly the durable state, and what [`Tail`]
/// says of bytes appended since the file's last sync. Without `fsync`, every
/// completed operation is durable at once. With [`MemDisk::set_eager_names`], a new
/// name is durable as soon as it is created.
///
/// **Renames.** In a root without [`Capabilities::no_replace`], a file renamed onto
/// a file replaces it, as a POSIX rename does.
///
/// **Crashes.** Every mutating request ([`Io::mutates`]) counts as one operation,
/// whether or not it succeeds, and a [`Renames::CopyThenRemove`] file rename counts
/// as two. After [`MemDisk::crash_after`]`(n)` the next `n` operations run and the
/// one after fails with [`IoError::Crashed`] without effect, except that a
/// crashing append lands as [`Tail`] says. From then on every request on every
/// handle fails the same way. [`MemDisk::restart`] gives a handle on a new disk
/// holding what survived, with every lock released.
#[derive(Clone)]
pub struct MemDisk {
    disk: Rc<RefCell<Disk>>,
    process: u64,
}

type Ino = usize;

const TOP: Ino = 0;

#[derive(Clone)]
enum Content {
    File { data: Vec<u8>, modified: u64 },
    Directory(BTreeMap<String, Ino>),
}

impl Content {
    fn kind(&self) -> Kind {
        match self {
            Self::File { .. } => Kind::File,
            Self::Directory(_) => Kind::Directory,
        }
    }
}

/// Nodes are never freed: a durable name may still point at one whose live names
/// are gone.
#[derive(Clone)]
struct Node {
    live: Content,
    durable: Content,
}

#[derive(Clone)]
struct Tree {
    nodes: Vec<Node>,
    capabilities: Capabilities,
    capacity: Option<u64>,
    /// Directory renames not yet durable.
    moves: Vec<DirMove>,
}

/// A directory renamed from one directory's names to another's.
#[derive(Clone)]
struct DirMove {
    ino: Ino,
    from: (Ino, String),
    to: (Ino, String),
}

struct Disk {
    folder: Tree,
    local: Tree,
    clock: u64,
    mutations: u64,
    crash_at: Option<u64>,
    crashed: bool,
    tail: Tail,
    renames: Renames,
    eager_names: bool,
    locks: BTreeMap<RelPath, u64>,
    processes: u64,
}

const ROOT_ITSELF: &str = "the root itself cannot be created, moved or removed";

impl Default for MemDisk {
    fn default() -> Self {
        Self::new()
    }
}

impl MemDisk {
    /// An empty disk with every capability in both roots.
    pub fn new() -> Self {
        Self::with_capabilities(Capabilities::ALL, Capabilities::ALL)
    }

    pub fn with_capabilities(folder: Capabilities, local: Capabilities) -> Self {
        Self::from_disk(Disk {
            folder: Tree::new(folder),
            local: Tree::new(local),
            clock: 0,
            mutations: 0,
            crash_at: None,
            crashed: false,
            tail: Tail::default(),
            renames: Renames::default(),
            eager_names: false,
            locks: BTreeMap::new(),
            processes: 0,
        })
    }

    fn from_disk(disk: Disk) -> Self {
        Self {
            disk: Rc::new(RefCell::new(disk)),
            process: 0,
        }
    }

    /// A handle of a new process on this disk. Locks are held per process.
    pub fn process(&self) -> Self {
        let mut disk = self.disk.borrow_mut();
        disk.processes += 1;
        Self {
            disk: Rc::clone(&self.disk),
            process: disk.processes,
        }
    }

    pub fn capabilities(&self, root: Root) -> Capabilities {
        self.disk.borrow().tree(root).capabilities
    }

    pub fn perform(&self, io: Io) -> IoResult {
        let mut disk = self.disk.borrow_mut();
        if disk.crashed {
            return Err(IoError::Crashed);
        }
        match io {
            Io::List { root, dir } => disk.tree(root).list(&dir).map(Reply::Listed),
            Io::Stat { root, path } => disk.tree(root).stat(&path).map(Reply::Stat),
            Io::Read { root, path, range } => disk.tree(root).read(&path, range).map(Reply::Bytes),
            Io::Lock { name } => Ok(Reply::Lock(disk.lock(self.process, name))),
            Io::Unlock { name } => {
                if disk.locks.get(&name) == Some(&self.process) {
                    disk.locks.remove(&name);
                }
                Ok(Reply::Done)
            }
            mutating => disk.mutate(mutating).map(|()| Reply::Done),
        }
    }

    /// Let `operations` more operations run, then crash on the next.
    pub fn crash_after(&self, operations: u64) {
        let mut disk = self.disk.borrow_mut();
        disk.crash_at = Some(disk.mutations + operations);
    }

    /// Operations counted since this disk started.
    pub fn mutations(&self) -> u64 {
        self.disk.borrow().mutations
    }

    pub fn crashed(&self) -> bool {
        self.disk.borrow().crashed
    }

    /// Crash this disk if it has not crashed, and return a handle on a new disk
    /// holding what survived, with the same capabilities, faults and clock, no
    /// locks and no crash scheduled.
    pub fn restart(&self) -> MemDisk {
        let mut disk = self.disk.borrow_mut();
        disk.crashed = true;
        Self::from_disk(Disk {
            folder: disk.folder.restarted(disk.tail),
            local: disk.local.restarted(disk.tail),
            clock: disk.clock,
            mutations: 0,
            crash_at: None,
            crashed: false,
            tail: disk.tail,
            renames: disk.renames,
            eager_names: disk.eager_names,
            locks: BTreeMap::new(),
            processes: 0,
        })
    }

    /// Empty the local root and release every lock, as when an install's storage
    /// is cleared. Not counted as an operation.
    pub fn lose_local(&self) {
        let mut disk = self.disk.borrow_mut();
        disk.local = Tree::new(disk.local.capabilities);
        disk.locks.clear();
    }

    pub fn set_tail(&self, tail: Tail) {
        self.disk.borrow_mut().tail = tail;
    }

    pub fn set_renames(&self, renames: Renames) {
        self.disk.borrow_mut().renames = renames;
    }

    /// Make each new file's or directory's name durable as soon as it is created, as
    /// a file system may on its own, while contents still wait for a sync.
    pub fn set_eager_names(&self, eager: bool) {
        self.disk.borrow_mut().eager_names = eager;
    }

    /// Refuse writes that would make the files of `root` hold more than `bytes` in
    /// total, with [`IoError::NoSpace`].
    pub fn set_capacity(&self, root: Root, bytes: Option<u64>) {
        self.disk.borrow_mut().tree_mut(root).capacity = bytes;
    }

    /// Set a file's modification time without touching its contents, as a program
    /// that preserves times would. Not counted as an operation.
    pub fn set_modified(&self, root: Root, path: &RelPath, modified: u64) -> Result<(), IoError> {
        let mut disk = self.disk.borrow_mut();
        let tree = disk.tree_mut(root);
        let ino = tree.file(path)?;
        if let Content::File { modified: time, .. } = &mut tree.nodes[ino].live {
            *time = modified;
        }
        Ok(())
    }

    /// Every file of `root` reachable by name, durable or not. Works on a crashed
    /// disk, for inspection.
    pub fn files(&self, root: Root) -> BTreeMap<RelPath, Vec<u8>> {
        let disk = self.disk.borrow();
        let mut files = BTreeMap::new();
        disk.tree(root)
            .walk(TOP, &RelPath::ROOT, &mut |path, content| {
                if let Content::File { data, .. } = content {
                    files.insert(path.clone(), data.clone());
                }
            });
        files
    }

    /// Every directory of `root` reachable by name, the root itself excluded.
    pub fn directories(&self, root: Root) -> BTreeSet<RelPath> {
        let disk = self.disk.borrow();
        let mut directories = BTreeSet::new();
        disk.tree(root)
            .walk(TOP, &RelPath::ROOT, &mut |path, content| {
                if let Content::Directory(_) = content {
                    directories.insert(path.clone());
                }
            });
        directories
    }
}

impl Disk {
    fn tree(&self, root: Root) -> &Tree {
        match root {
            Root::Folder => &self.folder,
            Root::Local => &self.local,
        }
    }

    fn tree_mut(&mut self, root: Root) -> &mut Tree {
        match root {
            Root::Folder => &mut self.folder,
            Root::Local => &mut self.local,
        }
    }

    fn lock(&mut self, process: u64, name: RelPath) -> Lock {
        match self.locks.get(&name) {
            Some(&holder) if holder != process => Lock::Held,
            _ => {
                self.locks.insert(name, process);
                Lock::Acquired
            }
        }
    }

    /// Count one operation, or crash in its place.
    fn step(&mut self, io: Option<&Io>) -> Result<(), IoError> {
        if self.crash_at != Some(self.mutations) {
            self.mutations += 1;
            return Ok(());
        }
        if let Some(Io::Append { root, path, bytes }) = io {
            let kept = self.tail.keep(bytes);
            let tree = self.tree_mut(*root);
            if let Ok(ino) = tree.file(path) {
                if let Content::File { data, .. } = &mut tree.nodes[ino].live {
                    data.extend_from_slice(&kept);
                }
            }
        }
        self.crashed = true;
        Err(IoError::Crashed)
    }

    fn mutate(&mut self, io: Io) -> Result<(), IoError> {
        self.step(Some(&io))?;
        self.clock += 1;
        let (now, eager) = (self.clock, self.eager_names);
        match io {
            Io::Create { root, path, bytes } => {
                self.tree_mut(root).create(&path, bytes, now, eager)
            }
            Io::Append { root, path, bytes } => self.tree_mut(root).append(&path, &bytes, now),
            Io::Rename { root, from, to } => self.rename(root, &from, &to),
            Io::Remove { root, path } => self.tree_mut(root).remove(&path, Kind::File),
            Io::RemoveDir { root, path } => self.tree_mut(root).remove(&path, Kind::Directory),
            Io::MakeDir { root, path } => self.tree_mut(root).make_dir(&path, eager),
            Io::Sync { root, path } => self.tree_mut(root).sync(&path),
            Io::List { .. }
            | Io::Stat { .. }
            | Io::Read { .. }
            | Io::Lock { .. }
            | Io::Unlock { .. } => {
                unreachable!("only mutating requests are counted")
            }
        }
    }

    fn rename(&mut self, root: Root, from: &RelPath, to: &RelPath) -> Result<(), IoError> {
        let renames = self.renames;
        let tree = self.tree_mut(root);
        let (from_dir, from_name) = tree.place(from)?;
        let ino = tree.child(from_dir, from_name).ok_or(IoError::NotFound)?;
        let kind = tree.nodes[ino].live.kind();
        tree.require(match kind {
            Kind::File => Capability::RenameFile,
            Kind::Directory => Capability::RenameDir,
        })?;
        let (to_dir, to_name) = tree.place(to)?;
        match tree
            .child(to_dir, to_name)
            .map(|ino| tree.nodes[ino].live.kind())
        {
            None => {}
            Some(Kind::File) if kind == Kind::File && !tree.capabilities.no_replace => {}
            Some(_) => return Err(IoError::AlreadyExists),
        }
        if to.starts_with(from) {
            return Err(IoError::IntoItself);
        }
        let to_name = to_name.to_owned();
        if renames == Renames::Atomic || kind == Kind::Directory {
            tree.entries_mut(from_dir).remove(from_name);
            tree.entries_mut(to_dir).insert(to_name.clone(), ino);
            if kind == Kind::Directory {
                tree.moves.push(DirMove {
                    ino,
                    from: (from_dir, from_name.to_owned()),
                    to: (to_dir, to_name),
                });
            }
            return Ok(());
        }
        let copy = tree.nodes[ino].clone();
        tree.nodes.push(copy);
        let copied = tree.nodes.len() - 1;
        tree.entries_mut(to_dir).insert(to_name, copied);
        let from_name = from_name.to_owned();
        self.step(None)?;
        self.tree_mut(root).entries_mut(from_dir).remove(&from_name);
        Ok(())
    }
}

impl Tree {
    fn new(capabilities: Capabilities) -> Self {
        let top = Content::Directory(BTreeMap::new());
        Self {
            nodes: vec![Node {
                live: top.clone(),
                durable: top,
            }],
            capabilities,
            capacity: None,
            moves: Vec::new(),
        }
    }

    fn restarted(&self, tail: Tail) -> Self {
        let nodes = self
            .nodes
            .iter()
            .map(|node| {
                let kept = match self.capabilities.fsync {
                    false => node.live.clone(),
                    true => survivor(node, tail),
                };
                Node {
                    live: kept.clone(),
                    durable: kept,
                }
            })
            .collect();
        Self {
            nodes,
            capabilities: self.capabilities,
            capacity: self.capacity,
            moves: Vec::new(),
        }
    }

    fn require(&self, capability: Capability) -> Result<(), IoError> {
        match self.capabilities.has(capability) {
            true => Ok(()),
            false => Err(IoError::Unsupported(capability)),
        }
    }

    fn lookup(&self, path: &RelPath) -> Result<Option<Ino>, IoError> {
        let mut ino = TOP;
        for name in path.components() {
            let Content::Directory(entries) = &self.nodes[ino].live else {
                return Err(IoError::NotDirectory);
            };
            let Some(&child) = entries.get(name) else {
                return Ok(None);
            };
            ino = child;
        }
        Ok(Some(ino))
    }

    fn existing(&self, path: &RelPath) -> Result<Ino, IoError> {
        self.lookup(path)?.ok_or(IoError::NotFound)
    }

    fn file(&self, path: &RelPath) -> Result<Ino, IoError> {
        let ino = self.existing(path)?;
        match self.nodes[ino].live.kind() {
            Kind::File => Ok(ino),
            Kind::Directory => Err(IoError::IsDirectory),
        }
    }

    /// The directory `path` would live in, and its name there.
    fn place<'p>(&self, path: &'p RelPath) -> Result<(Ino, &'p str), IoError> {
        let (Some(parent), Some(name)) = (path.parent(), path.name()) else {
            return Err(IoError::Other(ROOT_ITSELF.into()));
        };
        let ino = self.existing(&parent)?;
        match self.nodes[ino].live.kind() {
            Kind::Directory => Ok((ino, name)),
            Kind::File => Err(IoError::NotDirectory),
        }
    }

    fn child(&self, dir: Ino, name: &str) -> Option<Ino> {
        match &self.nodes[dir].live {
            Content::Directory(entries) => entries.get(name).copied(),
            Content::File { .. } => None,
        }
    }

    fn entries_mut(&mut self, dir: Ino) -> &mut BTreeMap<String, Ino> {
        match &mut self.nodes[dir].live {
            Content::Directory(entries) => entries,
            Content::File { .. } => unreachable!("only directories are placed into"),
        }
    }

    /// Give a new node its name in `dir`, durably at once with eager names.
    fn link(&mut self, dir: Ino, name: &str, content: Content, eager: bool) -> Ino {
        let durable = match &content {
            Content::File { modified, .. } => Content::File {
                data: Vec::new(),
                modified: *modified,
            },
            Content::Directory(_) => Content::Directory(BTreeMap::new()),
        };
        self.nodes.push(Node {
            live: content,
            durable,
        });
        let ino = self.nodes.len() - 1;
        self.entries_mut(dir).insert(name.to_owned(), ino);
        if let (true, Content::Directory(entries)) = (eager, &mut self.nodes[dir].durable) {
            entries.insert(name.to_owned(), ino);
        }
        ino
    }

    fn reserve(&self, more: usize) -> Result<(), IoError> {
        let Some(capacity) = self.capacity else {
            return Ok(());
        };
        let mut counted = BTreeSet::new();
        let mut used = 0u64;
        self.walk_inodes(TOP, &mut |ino, content| {
            if let (Content::File { data, .. }, true) = (content, counted.insert(ino)) {
                used += data.len() as u64;
            }
        });
        match used.checked_add(more as u64) {
            Some(total) if total <= capacity => Ok(()),
            _ => Err(IoError::NoSpace),
        }
    }

    fn walk_inodes(&self, ino: Ino, visit: &mut impl FnMut(Ino, &Content)) {
        let content = &self.nodes[ino].live;
        visit(ino, content);
        if let Content::Directory(entries) = content {
            for &child in entries.values() {
                self.walk_inodes(child, visit);
            }
        }
    }

    fn walk(&self, ino: Ino, path: &RelPath, visit: &mut impl FnMut(&RelPath, &Content)) {
        let content = &self.nodes[ino].live;
        if !path.is_root() {
            visit(path, content);
        }
        if let Content::Directory(entries) = content {
            for (name, &child) in entries {
                let child_path = path.join(name).expect("a stored name is one component");
                self.walk(child, &child_path, visit);
            }
        }
    }

    fn list(&self, dir: &RelPath) -> Result<Vec<DirEntry>, IoError> {
        match &self.nodes[self.existing(dir)?].live {
            Content::Directory(entries) => Ok(entries
                .iter()
                .map(|(name, &ino)| DirEntry {
                    name: name.clone(),
                    kind: self.nodes[ino].live.kind(),
                })
                .collect()),
            Content::File { .. } => Err(IoError::NotDirectory),
        }
    }

    fn stat(&self, path: &RelPath) -> Result<Option<Meta>, IoError> {
        let Some(ino) = self.lookup(path)? else {
            return Ok(None);
        };
        Ok(Some(match &self.nodes[ino].live {
            Content::File { data, modified } => Meta {
                kind: Kind::File,
                len: data.len() as u64,
                modified: Some(*modified),
            },
            Content::Directory(_) => Meta {
                kind: Kind::Directory,
                len: 0,
                modified: None,
            },
        }))
    }

    fn read(&self, path: &RelPath, range: Range) -> Result<Vec<u8>, IoError> {
        let Content::File { data, .. } = &self.nodes[self.file(path)?].live else {
            unreachable!("`file` finds files")
        };
        let clamp = |n: u64| usize::try_from(n).unwrap_or(usize::MAX).min(data.len());
        let start = clamp(range.offset);
        let end = start.saturating_add(clamp(range.len)).min(data.len());
        Ok(data[start..end].to_vec())
    }

    fn create(
        &mut self,
        path: &RelPath,
        bytes: Vec<u8>,
        now: u64,
        eager: bool,
    ) -> Result<(), IoError> {
        let (dir, name) = self.place(path)?;
        if self.child(dir, name).is_some() {
            return Err(IoError::AlreadyExists);
        }
        self.reserve(bytes.len())?;
        let file = Content::File {
            data: bytes,
            modified: now,
        };
        self.link(dir, name, file, eager);
        Ok(())
    }

    fn append(&mut self, path: &RelPath, bytes: &[u8], now: u64) -> Result<(), IoError> {
        self.require(Capability::Append)?;
        let ino = self.file(path)?;
        self.reserve(bytes.len())?;
        if let Content::File { data, modified } = &mut self.nodes[ino].live {
            data.extend_from_slice(bytes);
            *modified = now;
        }
        Ok(())
    }

    fn remove(&mut self, path: &RelPath, kind: Kind) -> Result<(), IoError> {
        let (dir, name) = self.place(path)?;
        let ino = self.child(dir, name).ok_or(IoError::NotFound)?;
        match (&self.nodes[ino].live, kind) {
            (Content::File { .. }, Kind::File) => {}
            (Content::Directory(entries), Kind::Directory) if entries.is_empty() => {}
            (Content::Directory(_), Kind::Directory) => return Err(IoError::NotEmpty),
            (Content::Directory(_), Kind::File) => return Err(IoError::IsDirectory),
            (Content::File { .. }, Kind::Directory) => return Err(IoError::NotDirectory),
        }
        self.entries_mut(dir).remove(name);
        Ok(())
    }

    fn make_dir(&mut self, path: &RelPath, eager: bool) -> Result<(), IoError> {
        let mut ino = TOP;
        for name in path.components() {
            ino = match self.child(ino, name) {
                Some(child) if self.nodes[child].live.kind() == Kind::Directory => child,
                Some(_) => return Err(IoError::NotDirectory),
                None => self.link(ino, name, Content::Directory(BTreeMap::new()), eager),
            };
        }
        Ok(())
    }

    fn sync(&mut self, path: &RelPath) -> Result<(), IoError> {
        let ino = self.existing(path)?;
        let node = &mut self.nodes[ino];
        node.durable = node.live.clone();
        let (now, later) = std::mem::take(&mut self.moves)
            .into_iter()
            .partition(|moved| moved.from.0 == ino || moved.to.0 == ino);
        self.moves = later;
        for DirMove {
            ino: moved,
            from,
            to,
        } in now
        {
            if let Content::Directory(names) = &mut self.nodes[from.0].durable {
                if names.get(&from.1) == Some(&moved) {
                    names.remove(&from.1);
                }
            }
            if let Content::Directory(names) = &mut self.nodes[to.0].durable {
                names.insert(to.1, moved);
            }
        }
        Ok(())
    }
}

/// What of a node survives a crash in a root with fsync.
fn survivor(node: &Node, tail: Tail) -> Content {
    match (&node.durable, &node.live) {
        (
            Content::File {
                data: durable,
                modified,
            },
            Content::File { data: live, .. },
        ) if live.len() > durable.len() && live.starts_with(durable) => {
            let mut data = durable.clone();
            data.extend(tail.keep(&live[durable.len()..]));
            Content::File {
                data,
                modified: *modified,
            }
        }
        _ => node.durable.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn files(entries: &[(&str, &[u8])]) -> BTreeMap<RelPath, Vec<u8>> {
        entries
            .iter()
            .map(|(name, bytes)| (path(name), bytes.to_vec()))
            .collect()
    }

    /// Runs requests on the folder root, panicking on failure.
    struct Folder<'d>(&'d MemDisk);

    impl Folder<'_> {
        fn run(&self, io: Io) -> Reply {
            let shown = format!("{io:?}");
            self.0
                .perform(io)
                .unwrap_or_else(|e| panic!("{shown}: {e}"))
        }

        fn try_run(&self, io: Io) -> IoResult {
            self.0.perform(io)
        }

        fn dir(&self, text: &str) {
            self.run(Io::MakeDir {
                root: Root::Folder,
                path: path(text),
            });
        }

        fn create(&self, text: &str, bytes: &[u8]) {
            self.run(create(text, bytes));
        }

        fn append(&self, text: &str, bytes: &[u8]) {
            self.run(append(text, bytes));
        }

        fn sync(&self, text: &str) {
            self.run(Io::Sync {
                root: Root::Folder,
                path: path(text),
            });
        }

        fn rename(&self, from: &str, to: &str) {
            self.run(rename(from, to));
        }
    }

    fn create(text: &str, bytes: &[u8]) -> Io {
        Io::Create {
            root: Root::Folder,
            path: path(text),
            bytes: bytes.to_vec(),
        }
    }

    fn append(text: &str, bytes: &[u8]) -> Io {
        Io::Append {
            root: Root::Folder,
            path: path(text),
            bytes: bytes.to_vec(),
        }
    }

    fn rename(from: &str, to: &str) -> Io {
        Io::Rename {
            root: Root::Folder,
            from: path(from),
            to: path(to),
        }
    }

    fn no_fsync() -> MemDisk {
        let without = Capabilities {
            fsync: false,
            ..Capabilities::ALL
        };
        MemDisk::with_capabilities(without, without)
    }

    #[test]
    fn only_synced_names_and_contents_survive_a_crash() {
        let disk = MemDisk::new();
        let f = Folder(&disk);
        f.create("named", b"lost");
        f.create("whole", b"kept");
        f.sync("whole");
        f.sync("");
        f.append("whole", b" but not this");
        f.create("unnamed", b"lost");
        f.sync("unnamed");
        let after = disk.restart();
        assert_eq!(
            after.files(Root::Folder),
            files(&[("named", b""), ("whole", b"kept")])
        );
        assert_eq!(
            disk.perform(Io::List {
                root: Root::Folder,
                dir: RelPath::ROOT
            }),
            Err(IoError::Crashed)
        );
    }

    #[test]
    fn with_eager_names_a_created_file_survives_empty_until_synced() {
        let disk = MemDisk::new();
        disk.set_eager_names(true);
        let f = Folder(&disk);
        f.dir("d");
        f.create("d/torn", b"lost");
        f.create("d/whole", b"kept");
        f.sync("d/whole");
        assert_eq!(
            disk.restart().files(Root::Folder),
            files(&[("d/torn", b""), ("d/whole", b"kept")])
        );
    }

    #[test]
    fn a_rename_survives_once_both_directories_are_synced() {
        let disk = MemDisk::new();
        let f = Folder(&disk);
        f.dir("a");
        f.dir("b");
        f.create("a/f", b"x");
        for synced in ["a/f", "a", "b", ""] {
            f.sync(synced);
        }
        f.rename("a/f", "b/f");
        assert_eq!(disk.restart().files(Root::Folder), files(&[("a/f", b"x")]));

        let disk = disk.restart();
        let f = Folder(&disk);
        f.rename("a/f", "b/f");
        f.sync("b");
        assert_eq!(
            disk.restart().files(Root::Folder),
            files(&[("a/f", b"x"), ("b/f", b"x")]),
            "a rename whose source directory was not synced leaves both names"
        );
    }

    #[test]
    fn a_directory_rename_survives_whole_once_either_directory_is_synced() {
        for synced in ["a", "b"] {
            let disk = MemDisk::new();
            let f = Folder(&disk);
            f.dir("a/d");
            f.dir("b");
            f.create("a/d/f", b"x");
            for path in ["a/d/f", "a/d", "a", "b", ""] {
                f.sync(path);
            }
            f.rename("a/d", "b/d");
            f.sync(synced);
            assert_eq!(
                disk.restart().files(Root::Folder),
                files(&[("b/d/f", b"x")]),
                "synced {synced}"
            );
        }
    }

    #[test]
    fn without_fsync_every_completed_operation_survives() {
        let disk = no_fsync();
        let f = Folder(&disk);
        f.dir("d");
        f.create("d/f", b"a");
        f.append("d/f", b"b");
        f.rename("d", "e");
        assert_eq!(disk.restart().files(Root::Folder), files(&[("e/f", b"ab")]));
    }

    #[test]
    fn a_crash_stops_the_disk_after_the_scheduled_operations() {
        let disk = no_fsync();
        let f = Folder(&disk);
        disk.crash_after(2);
        f.create("a", b"");
        assert_eq!(f.try_run(create("a", b"")), Err(IoError::AlreadyExists));
        assert_eq!(f.try_run(create("b", b"")), Err(IoError::Crashed));
        assert!(disk.crashed() && disk.process().crashed());
        assert_eq!(disk.mutations(), 2);
        let after = disk.restart();
        assert_eq!(after.files(Root::Folder), files(&[("a", b"")]));
        Folder(&after).create("b", b"");
        assert_eq!(after.mutations(), 1);
    }

    #[test]
    fn reads_and_locks_are_not_counted_as_operations() {
        let disk = MemDisk::new();
        disk.crash_after(0);
        let f = Folder(&disk);
        f.run(Io::Stat {
            root: Root::Folder,
            path: RelPath::ROOT,
        });
        f.run(Io::Lock { name: path("l") });
        assert_eq!(
            f.try_run(Io::Sync {
                root: Root::Folder,
                path: RelPath::ROOT
            }),
            Err(IoError::Crashed)
        );
    }

    #[test]
    fn a_crash_tears_or_zeroes_unsynced_appends_as_configured() {
        let cases = [
            (Tail::Lost, b"head".to_vec()),
            (Tail::Torn(3), b"head ta".to_vec()),
            (Tail::Torn(99), b"head tail".to_vec()),
            (Tail::Zeroed(2), b"head\0\0".to_vec()),
        ];
        for (tail, expected) in cases {
            let disk = MemDisk::new();
            disk.set_tail(tail);
            let f = Folder(&disk);
            f.create("log", b"head");
            f.sync("log");
            f.sync("");
            f.append("log", b" ta");
            f.append("log", b"il");
            assert_eq!(
                disk.restart().files(Root::Folder),
                files(&[("log", &expected)]),
                "{tail:?}"
            );
        }
    }

    #[test]
    fn a_crashing_append_lands_as_the_tail_says() {
        let disk = no_fsync();
        disk.set_tail(Tail::Torn(2));
        let f = Folder(&disk);
        f.create("log", b"a");
        disk.crash_after(0);
        assert_eq!(f.try_run(append("log", b"bcd")), Err(IoError::Crashed));
        assert_eq!(
            disk.restart().files(Root::Folder),
            files(&[("log", b"abc")])
        );
    }

    #[test]
    fn a_rename_by_copy_leaves_both_files_when_cut_short() {
        let disk = no_fsync();
        disk.set_renames(Renames::CopyThenRemove);
        let f = Folder(&disk);
        f.create("a", b"x");
        disk.crash_after(1);
        assert_eq!(f.try_run(rename("a", "b")), Err(IoError::Crashed));
        assert_eq!(
            disk.restart().files(Root::Folder),
            files(&[("a", b"x"), ("b", b"x")])
        );

        let disk = no_fsync();
        disk.set_renames(Renames::CopyThenRemove);
        let f = Folder(&disk);
        f.create("a", b"x");
        f.rename("a", "b");
        assert_eq!(disk.mutations(), 3);
        assert_eq!(disk.files(Root::Folder), files(&[("b", b"x")]));
    }

    #[test]
    fn a_lock_is_held_per_process_until_released_or_restart() {
        let disk = MemDisk::new();
        let other = disk.process();
        let lock = |d: &MemDisk| {
            d.perform(Io::Lock {
                name: path("w/lock"),
            })
        };
        assert_eq!(lock(&disk), Ok(Reply::Lock(Lock::Acquired)));
        assert_eq!(lock(&disk.clone()), Ok(Reply::Lock(Lock::Acquired)));
        assert_eq!(lock(&other), Ok(Reply::Lock(Lock::Held)));
        other
            .perform(Io::Unlock {
                name: path("w/lock"),
            })
            .unwrap();
        assert_eq!(lock(&other), Ok(Reply::Lock(Lock::Held)));
        disk.perform(Io::Unlock {
            name: path("w/lock"),
        })
        .unwrap();
        assert_eq!(lock(&other), Ok(Reply::Lock(Lock::Acquired)));
        assert_eq!(lock(&other.restart()), Ok(Reply::Lock(Lock::Acquired)));
    }

    #[test]
    fn losing_the_local_root_keeps_the_folder() {
        let disk = MemDisk::new();
        for root in [Root::Folder, Root::Local] {
            disk.perform(Io::Create {
                root,
                path: path("f"),
                bytes: b"x".to_vec(),
            })
            .unwrap();
        }
        disk.perform(Io::Lock { name: path("l") }).unwrap();
        disk.lose_local();
        assert_eq!(disk.files(Root::Folder), files(&[("f", b"x")]));
        assert_eq!(disk.files(Root::Local), files(&[]));
        assert_eq!(
            disk.process().perform(Io::Lock { name: path("l") }),
            Ok(Reply::Lock(Lock::Acquired))
        );
    }

    #[test]
    fn a_full_root_refuses_writes_and_keeps_what_it_has() {
        let disk = MemDisk::new();
        disk.set_capacity(Root::Folder, Some(4));
        let f = Folder(&disk);
        f.create("a", b"abc");
        assert_eq!(f.try_run(append("a", b"de")), Err(IoError::NoSpace));
        assert_eq!(f.try_run(create("c", b"de")), Err(IoError::NoSpace));
        f.append("a", b"d");
        disk.perform(Io::Create {
            root: Root::Local,
            path: path("big"),
            bytes: vec![0; 64],
        })
        .unwrap();
        assert_eq!(disk.files(Root::Folder), files(&[("a", b"abcd")]));
    }

    #[test]
    fn undeclared_capabilities_are_refused_and_change_nothing() {
        let disk = MemDisk::with_capabilities(
            Capabilities {
                rename_file: true,
                ..Capabilities::NONE
            },
            Capabilities::NONE,
        );
        let f = Folder(&disk);
        f.dir("d");
        f.create("f", b"x");
        assert_eq!(
            f.try_run(append("f", b"y")),
            Err(IoError::Unsupported(Capability::Append))
        );
        assert_eq!(
            f.try_run(rename("d", "e")),
            Err(IoError::Unsupported(Capability::RenameDir))
        );
        f.rename("f", "d/f");
        assert_eq!(disk.files(Root::Folder), files(&[("d/f", b"x")]));
    }

    #[test]
    fn modification_times_tick_with_each_write_and_repeat_across_runs() {
        let run = || {
            let disk = MemDisk::new();
            let f = Folder(&disk);
            f.create("a", b"");
            f.create("b", b"");
            f.append("a", b"x");
            ["a", "b"].map(|name| {
                match f.run(Io::Stat {
                    root: Root::Folder,
                    path: path(name),
                }) {
                    Reply::Stat(Some(meta)) => meta.modified,
                    other => panic!("{other:?}"),
                }
            })
        };
        let [a, b] = run();
        assert!(a > b, "an append moves the time past a later create");
        assert_eq!(run(), [a, b]);
    }
}
