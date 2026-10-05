use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::{Capabilities, Capability, DirEntry, FileKind, Fs, Metadata, RelPath};
use crate::error::{Error, Result};

/// An in-memory file system: deterministic, with a model of durability and crashes.
///
/// Clones are handles to one disk, as two processes share one folder. Modification
/// times come from a logical clock that ticks once per completed mutation.
///
/// **Durability.** With [`Capabilities::fsync`], a file's contents and a directory's
/// names each become durable only when [`Fs::sync`] is called on that file or
/// directory. A crash keeps exactly the durable state: an unsynced new file vanishes,
/// a file whose name was synced but whose contents were not comes back empty, and a
/// rename across directories that syncs only one side loses or duplicates the file.
/// Without `fsync`, every completed operation is durable at once.
///
/// **Crashes.** Every call of a mutating method (`create_dir_all`, `create`, `append`,
/// `rename`, `remove_file`, `remove_dir`, `sync`) counts as one
/// operation, whether or not it succeeds. After [`MemFs::crash_after`]`(n)`, the next
/// `n` operations run and the one after fails with [`Error::Crashed`] without taking
/// effect; from then on every call on every handle to this disk fails the same way.
/// [`MemFs::restart`] returns a handle to a new disk holding the durable state.
#[derive(Clone)]
pub struct MemFs {
    disk: Rc<RefCell<Disk>>,
}

type Ino = usize;

const ROOT: Ino = 0;

#[derive(Clone)]
enum Content {
    File { data: Vec<u8>, modified: u64 },
    Directory(BTreeMap<String, Ino>),
}

impl Content {
    fn kind(&self) -> FileKind {
        match self {
            Self::File { .. } => FileKind::File,
            Self::Directory(_) => FileKind::Directory,
        }
    }
}

/// An inode. Nodes are never freed, because a durable name may still point at one
/// whose live names are gone. `durable` is unused without fsync, where every
/// completed operation is durable.
struct Node {
    live: Content,
    durable: Content,
}

struct Disk {
    capabilities: Capabilities,
    nodes: Vec<Node>,
    clock: u64,
    mutations: u64,
    crash_at: Option<u64>,
    crashed: bool,
    capacity: Option<u64>,
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemFs {
    /// An empty disk with every capability.
    pub fn new() -> Self {
        Self::with_capabilities(Capabilities::ALL)
    }

    /// An empty disk that offers only `capabilities`, refusing the rest with
    /// [`Error::Unsupported`].
    pub fn with_capabilities(capabilities: Capabilities) -> Self {
        let root = Content::Directory(BTreeMap::new());
        Self::from_disk(Disk {
            capabilities,
            nodes: vec![Node {
                live: root.clone(),
                durable: root,
            }],
            clock: 0,
            mutations: 0,
            crash_at: None,
            crashed: false,
            capacity: None,
        })
    }

    fn from_disk(disk: Disk) -> Self {
        Self {
            disk: Rc::new(RefCell::new(disk)),
        }
    }

    /// Let `operations` more mutating operations run, then crash on the next.
    pub fn crash_after(&self, operations: u64) {
        let mut disk = self.disk.borrow_mut();
        disk.crash_at = Some(disk.mutations + operations);
    }

    /// Mutating operations called on this disk since it started.
    pub fn mutations(&self) -> u64 {
        self.disk.borrow().mutations
    }

    pub fn crashed(&self) -> bool {
        self.disk.borrow().crashed
    }

    /// Crash this disk if it has not crashed, and return a handle to a new disk
    /// holding exactly its durable state, with the same capabilities, capacity and
    /// clock and no crash scheduled.
    pub fn restart(&self) -> MemFs {
        let mut disk = self.disk.borrow_mut();
        disk.crashed = true;
        let fsync = disk.capabilities.fsync;
        let nodes = disk
            .nodes
            .iter()
            .map(|node| {
                let kept = if fsync { &node.durable } else { &node.live };
                Node {
                    live: kept.clone(),
                    durable: kept.clone(),
                }
            })
            .collect();
        Self::from_disk(Disk {
            capabilities: disk.capabilities,
            nodes,
            clock: disk.clock,
            mutations: 0,
            crash_at: None,
            crashed: false,
            capacity: disk.capacity,
        })
    }

    /// Refuse writes that would make the files hold more than `bytes` in total, with
    /// [`Error::NoSpace`].
    pub fn set_capacity(&self, bytes: Option<u64>) {
        self.disk.borrow_mut().capacity = bytes;
    }

    /// Set a file's modification time without touching its contents, as another
    /// program that preserves times would. Not counted as an operation.
    pub fn set_modified(&self, path: &RelPath, modified: u64) -> Result<()> {
        let mut disk = self.disk.borrow_mut();
        disk.check_alive()?;
        let ino = disk.existing(path)?;
        match &mut disk.nodes[ino].live {
            Content::File { modified: time, .. } => *time = modified,
            Content::Directory(_) => return Err(Error::IsDirectory { path: path.clone() }),
        }
        Ok(())
    }

    /// Every file reachable by name, with its contents, whether or not it is durable.
    /// Works on a crashed disk, for inspection.
    pub fn files(&self) -> BTreeMap<RelPath, Vec<u8>> {
        let disk = self.disk.borrow();
        let mut files = BTreeMap::new();
        disk.walk(ROOT, &RelPath::ROOT, &mut |path, content| {
            if let Content::File { data, .. } = content {
                files.insert(path.clone(), data.clone());
            }
        });
        files
    }

    /// Every directory reachable by name, the root excluded.
    pub fn directories(&self) -> BTreeSet<RelPath> {
        let disk = self.disk.borrow();
        let mut directories = BTreeSet::new();
        disk.walk(ROOT, &RelPath::ROOT, &mut |path, content| {
            if let Content::Directory(_) = content {
                directories.insert(path.clone());
            }
        });
        directories
    }

    fn inspect<T>(&self, op: impl FnOnce(&Disk) -> Result<T>) -> Result<T> {
        let disk = self.disk.borrow();
        disk.check_alive()?;
        op(&disk)
    }

    fn mutate<T>(&self, op: impl FnOnce(&mut Disk) -> Result<T>) -> Result<T> {
        let mut disk = self.disk.borrow_mut();
        disk.check_alive()?;
        if disk.crash_at == Some(disk.mutations) {
            disk.crashed = true;
            return Err(Error::Crashed);
        }
        disk.mutations += 1;
        op(&mut disk)
    }
}

impl Disk {
    fn check_alive(&self) -> Result<()> {
        match self.crashed {
            true => Err(Error::Crashed),
            false => Ok(()),
        }
    }

    fn require(&self, capability: Capability) -> Result<()> {
        match self.capabilities.has(capability) {
            true => Ok(()),
            false => Err(Error::Unsupported(capability)),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn add(&mut self, live: Content) -> Ino {
        let durable = match &live {
            Content::File { modified, .. } => Content::File {
                data: Vec::new(),
                modified: *modified,
            },
            Content::Directory(_) => Content::Directory(BTreeMap::new()),
        };
        self.nodes.push(Node { live, durable });
        self.nodes.len() - 1
    }

    fn add_dir(&mut self, parent: Ino, name: &str) -> Ino {
        let ino = self.add(Content::Directory(BTreeMap::new()));
        self.entries_mut(parent).insert(name.to_owned(), ino);
        ino
    }

    fn lookup(&self, path: &RelPath) -> Result<Option<Ino>> {
        let mut ino = ROOT;
        let mut walked = RelPath::ROOT;
        for name in path.components() {
            let Content::Directory(entries) = &self.nodes[ino].live else {
                return Err(Error::NotDirectory { path: walked });
            };
            let Some(&child) = entries.get(name) else {
                return Ok(None);
            };
            ino = child;
            walked = walked.join(name)?;
        }
        Ok(Some(ino))
    }

    fn existing(&self, path: &RelPath) -> Result<Ino> {
        self.lookup(path)?
            .ok_or_else(|| Error::NotFound { path: path.clone() })
    }

    fn file(&self, path: &RelPath) -> Result<&[u8]> {
        match &self.nodes[self.existing(path)?].live {
            Content::File { data, .. } => Ok(data),
            Content::Directory(_) => Err(Error::IsDirectory { path: path.clone() }),
        }
    }

    fn entries(&self, path: &RelPath) -> Result<&BTreeMap<String, Ino>> {
        match &self.nodes[self.existing(path)?].live {
            Content::Directory(entries) => Ok(entries),
            Content::File { .. } => Err(Error::NotDirectory { path: path.clone() }),
        }
    }

    /// The directory `path` would live in, and its name there.
    fn place<'p>(&self, path: &'p RelPath) -> Result<(Ino, &'p str)> {
        let (Some(parent), Some(name)) = (path.parent(), path.name()) else {
            return Err(Error::InvalidPath {
                path: path.as_str().to_owned(),
                reason: "the library folder itself cannot be created, moved or removed",
            });
        };
        let ino = self.existing(&parent)?;
        match self.nodes[ino].live {
            Content::Directory(_) => Ok((ino, name)),
            Content::File { .. } => Err(Error::NotDirectory { path: parent }),
        }
    }

    fn entries_mut(&mut self, dir: Ino) -> &mut BTreeMap<String, Ino> {
        match &mut self.nodes[dir].live {
            Content::Directory(entries) => entries,
            Content::File { .. } => {
                unreachable!("only `place` and `create_dir_all` name directories")
            }
        }
    }

    fn child(&self, dir: Ino, name: &str) -> Option<Ino> {
        match &self.nodes[dir].live {
            Content::Directory(entries) => entries.get(name).copied(),
            Content::File { .. } => None,
        }
    }

    fn reserve(&self, path: &RelPath, more: usize) -> Result<()> {
        let Some(capacity) = self.capacity else {
            return Ok(());
        };
        let mut used = 0u64;
        self.walk(ROOT, &RelPath::ROOT, &mut |_, content| {
            if let Content::File { data, .. } = content {
                used += data.len() as u64;
            }
        });
        match used.checked_add(more as u64) {
            Some(total) if total <= capacity => Ok(()),
            _ => Err(Error::NoSpace { path: path.clone() }),
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
}

impl Fs for MemFs {
    fn capabilities(&self) -> Capabilities {
        self.disk.borrow().capabilities
    }

    async fn metadata(&self, path: &RelPath) -> Result<Option<Metadata>> {
        self.inspect(|disk| {
            let Some(ino) = disk.lookup(path)? else {
                return Ok(None);
            };
            Ok(Some(match &disk.nodes[ino].live {
                Content::File { data, modified } => Metadata {
                    kind: FileKind::File,
                    len: data.len() as u64,
                    modified: Some(*modified),
                },
                Content::Directory(_) => Metadata {
                    kind: FileKind::Directory,
                    len: 0,
                    modified: None,
                },
            }))
        })
    }

    async fn list(&self, dir: &RelPath) -> Result<Vec<DirEntry>> {
        self.inspect(|disk| {
            Ok(disk
                .entries(dir)?
                .iter()
                .map(|(name, &ino)| DirEntry {
                    name: name.clone(),
                    kind: disk.nodes[ino].live.kind(),
                })
                .collect())
        })
    }

    async fn read(&self, path: &RelPath) -> Result<Vec<u8>> {
        self.inspect(|disk| disk.file(path).map(<[u8]>::to_vec))
    }

    async fn read_at(&self, path: &RelPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        self.inspect(|disk| {
            let data = disk.file(path)?;
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(data.len());
            let end = start.saturating_add(len).min(data.len());
            Ok(data[start..end].to_vec())
        })
    }

    async fn create_dir_all(&self, dir: &RelPath) -> Result<()> {
        self.mutate(|disk| {
            let mut ino = ROOT;
            let mut names = dir.components();
            let mut walked = RelPath::ROOT;
            for name in names.by_ref() {
                walked = walked.join(name)?;
                let Some(child) = disk.child(ino, name) else {
                    ino = disk.add_dir(ino, name);
                    break;
                };
                if disk.nodes[child].live.kind() == FileKind::File {
                    return Err(Error::NotDirectory { path: walked });
                }
                ino = child;
            }
            for name in names {
                ino = disk.add_dir(ino, name);
            }
            Ok(())
        })
    }

    async fn create(&self, path: &RelPath, bytes: &[u8]) -> Result<()> {
        self.mutate(|disk| {
            let (dir, name) = disk.place(path)?;
            if disk.child(dir, name).is_some() {
                return Err(Error::AlreadyExists { path: path.clone() });
            }
            disk.reserve(path, bytes.len())?;
            let modified = disk.tick();
            let ino = disk.add(Content::File {
                data: bytes.to_vec(),
                modified,
            });
            disk.entries_mut(dir).insert(name.to_owned(), ino);
            Ok(())
        })
    }

    async fn append(&self, path: &RelPath, bytes: &[u8]) -> Result<()> {
        self.mutate(|disk| {
            disk.require(Capability::Append)?;
            disk.file(path)?;
            disk.reserve(path, bytes.len())?;
            let now = disk.tick();
            let ino = disk.existing(path)?;
            if let Content::File { data, modified } = &mut disk.nodes[ino].live {
                data.extend_from_slice(bytes);
                *modified = now;
            }
            Ok(())
        })
    }

    async fn rename(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        self.mutate(|disk| {
            let (from_dir, from_name) = disk.place(from)?;
            let ino = disk
                .child(from_dir, from_name)
                .ok_or_else(|| Error::NotFound { path: from.clone() })?;
            disk.require(match disk.nodes[ino].live.kind() {
                FileKind::File => Capability::RenameFile,
                FileKind::Directory => Capability::RenameDir,
            })?;
            let (to_dir, to_name) = disk.place(to)?;
            if disk.child(to_dir, to_name).is_some() {
                return Err(Error::AlreadyExists { path: to.clone() });
            }
            if to.starts_with(from) {
                return Err(Error::InvalidPath {
                    path: to.as_str().to_owned(),
                    reason: "a directory cannot move inside itself",
                });
            }
            disk.entries_mut(from_dir).remove(from_name);
            disk.entries_mut(to_dir).insert(to_name.to_owned(), ino);
            Ok(())
        })
    }

    async fn remove_file(&self, path: &RelPath) -> Result<()> {
        self.mutate(|disk| {
            let (dir, name) = disk.place(path)?;
            disk.file(path)?;
            disk.entries_mut(dir).remove(name);
            Ok(())
        })
    }

    async fn remove_dir(&self, path: &RelPath) -> Result<()> {
        self.mutate(|disk| {
            let (dir, name) = disk.place(path)?;
            if !disk.entries(path)?.is_empty() {
                return Err(Error::DirectoryNotEmpty { path: path.clone() });
            }
            disk.entries_mut(dir).remove(name);
            Ok(())
        })
    }

    async fn sync(&self, path: &RelPath) -> Result<()> {
        self.mutate(|disk| {
            let ino = disk.existing(path)?;
            let node = &mut disk.nodes[ino];
            node.durable = node.live.clone();
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

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

    macro_rules! assert_fails {
        ($result:expr, $pattern:pat) => {
            let result = $result;
            assert!(matches!(result, Err($pattern)), "{result:?}");
        };
    }

    #[test]
    fn a_created_file_reads_back_with_its_length() {
        let fs = MemFs::new();
        block_on(fs.create_dir_all(&path("a/b"))).unwrap();
        block_on(fs.create(&path("a/b/f"), b"hello")).unwrap();
        assert_eq!(block_on(fs.read(&path("a/b/f"))).unwrap(), b"hello");
        assert_eq!(block_on(fs.read_at(&path("a/b/f"), 3, 10)).unwrap(), b"lo");
        assert_eq!(block_on(fs.read_at(&path("a/b/f"), 9, 10)).unwrap(), b"");
        let metadata = block_on(fs.metadata(&path("a/b/f"))).unwrap().unwrap();
        assert_eq!((metadata.kind, metadata.len), (FileKind::File, 5));
        assert_eq!(block_on(fs.metadata(&path("a/x"))).unwrap(), None);
    }

    #[test]
    fn a_listing_is_sorted_by_name_and_reports_kinds() {
        let fs = MemFs::new();
        for name in ["b", "a", "C"] {
            block_on(fs.create(&path(name), b"")).unwrap();
        }
        block_on(fs.create_dir_all(&path("d"))).unwrap();
        let listing = block_on(fs.list(&RelPath::ROOT)).unwrap();
        let names: Vec<_> = listing.iter().map(|e| (e.name.as_str(), e.kind)).collect();
        assert_eq!(
            names,
            [
                ("C", FileKind::File),
                ("a", FileKind::File),
                ("b", FileKind::File),
                ("d", FileKind::Directory),
            ]
        );
        assert_fails!(block_on(fs.list(&path("a"))), Error::NotDirectory { .. });
    }

    #[test]
    fn create_never_replaces_and_needs_an_existing_directory() {
        let fs = MemFs::new();
        block_on(fs.create(&path("f"), b"old")).unwrap();
        assert_fails!(
            block_on(fs.create(&path("f"), b"new")),
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            block_on(fs.create(&path("d/f"), b"")),
            Error::NotFound { .. }
        );
        assert_fails!(
            block_on(fs.create(&path("f/g"), b"")),
            Error::NotDirectory { .. }
        );
        assert_fails!(
            block_on(fs.create(&RelPath::ROOT, b"")),
            Error::InvalidPath { .. }
        );
        assert_eq!(fs.files(), files(&[("f", b"old")]));
    }

    #[test]
    fn rename_never_replaces_and_moves_a_directory_whole() {
        let fs = MemFs::new();
        block_on(fs.create_dir_all(&path("a/sub"))).unwrap();
        block_on(fs.create(&path("a/sub/f"), b"1")).unwrap();
        block_on(fs.create(&path("taken"), b"2")).unwrap();
        assert_fails!(
            block_on(fs.rename(&path("a"), &path("taken"))),
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            block_on(fs.rename(&path("a"), &path("a/sub/in"))),
            Error::InvalidPath { .. }
        );
        assert_fails!(
            block_on(fs.rename(&path("gone"), &path("x"))),
            Error::NotFound { .. }
        );
        block_on(fs.rename(&path("a"), &path("b"))).unwrap();
        assert_eq!(fs.files(), files(&[("b/sub/f", b"1"), ("taken", b"2")]));
        assert_eq!(fs.directories(), [path("b"), path("b/sub")].into());
    }

    #[test]
    fn removal_refuses_the_wrong_kind_and_full_directories() {
        let fs = MemFs::new();
        block_on(fs.create_dir_all(&path("d"))).unwrap();
        block_on(fs.create(&path("d/f"), b"")).unwrap();
        assert_fails!(
            block_on(fs.remove_dir(&path("d"))),
            Error::DirectoryNotEmpty { .. }
        );
        assert_fails!(
            block_on(fs.remove_file(&path("d"))),
            Error::IsDirectory { .. }
        );
        assert_fails!(
            block_on(fs.remove_dir(&path("d/f"))),
            Error::NotDirectory { .. }
        );
        block_on(fs.remove_file(&path("d/f"))).unwrap();
        block_on(fs.remove_dir(&path("d"))).unwrap();
        assert!(fs.files().is_empty() && fs.directories().is_empty());
    }

    #[test]
    fn undeclared_capabilities_are_refused_and_change_nothing() {
        let fs = MemFs::with_capabilities(Capabilities {
            rename_file: true,
            ..Capabilities::NONE
        });
        block_on(fs.create_dir_all(&path("d"))).unwrap();
        block_on(fs.create(&path("f"), b"x")).unwrap();
        let refused = [
            (block_on(fs.append(&path("f"), b"y")), Capability::Append),
            (
                block_on(fs.rename(&path("d"), &path("e"))),
                Capability::RenameDir,
            ),
        ];
        for (result, capability) in refused {
            assert!(
                matches!(result, Err(Error::Unsupported(c)) if c == capability),
                "{capability:?}"
            );
        }
        block_on(fs.rename(&path("f"), &path("d/f"))).unwrap();
        assert_eq!(fs.files(), files(&[("d/f", b"x")]));
        assert_eq!(fs.directories(), [path("d")].into());
    }

    #[test]
    fn with_fsync_only_synced_names_and_contents_survive_a_crash() {
        let fs = MemFs::new();
        block_on(fs.create(&path("named"), b"lost")).unwrap();
        block_on(fs.create(&path("whole"), b"kept")).unwrap();
        block_on(fs.sync(&path("whole"))).unwrap();
        block_on(fs.sync(&RelPath::ROOT)).unwrap();
        block_on(fs.append(&path("whole"), b" but not this")).unwrap();
        block_on(fs.create(&path("unnamed"), b"lost")).unwrap();
        block_on(fs.sync(&path("unnamed"))).unwrap();
        let after = fs.restart();
        assert_eq!(after.files(), files(&[("named", b""), ("whole", b"kept")]));
        assert_fails!(block_on(fs.read(&path("whole"))), Error::Crashed);
    }

    #[test]
    fn a_rename_survives_once_both_directories_are_synced() {
        let fs = MemFs::new();
        block_on(fs.create_dir_all(&path("a"))).unwrap();
        block_on(fs.create_dir_all(&path("b"))).unwrap();
        block_on(fs.create(&path("a/f"), b"x")).unwrap();
        for synced in ["a/f", "a", "b", ""] {
            block_on(fs.sync(&path(synced))).unwrap();
        }
        block_on(fs.rename(&path("a/f"), &path("b/f"))).unwrap();
        assert_eq!(fs.restart().files(), files(&[("a/f", b"x")]));

        let fs = fs.restart();
        block_on(fs.rename(&path("a/f"), &path("b/f"))).unwrap();
        block_on(fs.sync(&path("b"))).unwrap();
        block_on(fs.sync(&path("a"))).unwrap();
        assert_eq!(fs.restart().files(), files(&[("b/f", b"x")]));
    }

    #[test]
    fn without_fsync_every_completed_operation_survives_a_crash() {
        let fs = MemFs::with_capabilities(Capabilities {
            fsync: false,
            ..Capabilities::ALL
        });
        block_on(fs.create_dir_all(&path("d"))).unwrap();
        block_on(fs.create(&path("d/f"), b"a")).unwrap();
        block_on(fs.append(&path("d/f"), b"b")).unwrap();
        block_on(fs.rename(&path("d"), &path("e"))).unwrap();
        assert_eq!(fs.restart().files(), files(&[("e/f", b"ab")]));
    }

    #[test]
    fn a_crash_stops_the_disk_after_the_scheduled_operations() {
        let fs = MemFs::with_capabilities(Capabilities {
            fsync: false,
            ..Capabilities::ALL
        });
        fs.crash_after(2);
        block_on(fs.create(&path("a"), b"")).unwrap();
        assert_fails!(
            block_on(fs.create(&path("a"), b"")),
            Error::AlreadyExists { .. }
        );
        assert_fails!(block_on(fs.create(&path("b"), b"")), Error::Crashed);
        assert!(fs.crashed() && fs.clone().crashed());
        assert_fails!(block_on(fs.metadata(&path("a"))), Error::Crashed);
        assert_eq!(fs.mutations(), 2);
        let after = fs.restart();
        assert_eq!(after.files(), files(&[("a", b"")]));
        block_on(after.create(&path("b"), b"")).unwrap();
        assert_eq!(after.mutations(), 1);
    }

    #[test]
    fn crash_after_zero_crashes_on_the_next_operation() {
        let fs = MemFs::new();
        fs.crash_after(0);
        assert_eq!(
            block_on(fs.metadata(&RelPath::ROOT))
                .unwrap()
                .map(|m| m.kind),
            Some(FileKind::Directory)
        );
        assert_fails!(block_on(fs.sync(&RelPath::ROOT)), Error::Crashed);
    }

    #[test]
    fn a_full_disk_refuses_writes_and_keeps_what_it_has() {
        let fs = MemFs::new();
        fs.set_capacity(Some(4));
        block_on(fs.create(&path("a"), b"abc")).unwrap();
        assert_fails!(
            block_on(fs.append(&path("a"), b"de")),
            Error::NoSpace { .. }
        );
        assert_fails!(
            block_on(fs.create(&path("c"), b"de")),
            Error::NoSpace { .. }
        );
        block_on(fs.append(&path("a"), b"d")).unwrap();
        assert_eq!(fs.files(), files(&[("a", b"abcd")]));
    }

    #[test]
    fn modification_times_tick_with_each_write_and_repeat_across_runs() {
        let run = || {
            let fs = MemFs::new();
            block_on(fs.create(&path("a"), b"")).unwrap();
            block_on(fs.create(&path("b"), b"")).unwrap();
            block_on(fs.append(&path("a"), b"x")).unwrap();
            ["a", "b"].map(|name| {
                block_on(fs.metadata(&path(name)))
                    .unwrap()
                    .unwrap()
                    .modified
            })
        };
        let [a, b] = run();
        assert!(
            a > b,
            "an append moves the time past a later create: {a:?} {b:?}"
        );
        assert_eq!(run(), [a, b]);
    }

    #[test]
    fn set_modified_changes_the_time_and_not_the_bytes() {
        let fs = MemFs::new();
        block_on(fs.create(&path("a"), b"x")).unwrap();
        fs.set_modified(&path("a"), 99).unwrap();
        let metadata = block_on(fs.metadata(&path("a"))).unwrap().unwrap();
        assert_eq!((metadata.modified, metadata.len), (Some(99), 1));
        assert_eq!(fs.mutations(), 1);
    }
}
