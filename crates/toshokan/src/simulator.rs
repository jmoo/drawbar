//! A sync adversary for tests, mirroring the sync layer of `spec/Portable.tla`: one
//! folder that writers write and several readers see, each reader receiving every
//! file independently and in any order.
//!
//! The spec lets sync show a reader any prefix of each file at any time. A
//! [`SyncEvent`] is one such step: [`SyncEvent::Deliver`] shows all of a file,
//! [`SyncEvent::Shorten`] a prefix, [`SyncEvent::Delete`] and [`SyncEvent::Hide`]
//! none of it, and [`SyncEvent::Resurrect`] a file the origin deleted. The spec
//! names no file, so [`SyncEvent::ConflictedCopy`], which shows a file under
//! another name, is a step it covers without modeling. Steps the spec charges to
//! its chaos budget are those [`SyncEvent::is_chaos`] says.

use std::collections::BTreeMap;

use crate::blocking::Backend;
use crate::disk::MemDisk;
use crate::io::{Capabilities, Io, IoResult, Root};
use crate::path::RelPath;

/// The files of a folder root, by path.
pub type Files = BTreeMap<RelPath, Vec<u8>>;

/// One thing a sync client can do to a reader's copy of the folder. Paths are in the
/// folder root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SyncEvent {
    /// The origin's current bytes arrive whole.
    Deliver { path: RelPath },
    /// Only the first `len` bytes of the origin's current bytes arrive.
    Shorten { path: RelPath, len: usize },
    /// The origin's deletion arrives, possibly before the file that replaces it.
    Delete { path: RelPath },
    /// The reader's copy disappears for a while, though the origin keeps it.
    Hide { path: RelPath },
    /// An earlier version of a file the origin deleted comes back.
    Resurrect { path: RelPath, version: usize },
    /// The origin's bytes arrive under another name beside the reader's version,
    /// as a sync client's conflicted copy.
    ConflictedCopy { path: RelPath, name: String },
}

impl SyncEvent {
    /// Whether the spec spends its chaos budget on this step: showing a reader less
    /// of a file than it has, hiding a file the origin keeps, or a conflicted copy.
    /// Growing a file, delivering it whole, and delivering or undoing a deletion
    /// are free.
    pub fn is_chaos(&self, versions: &Versions, reader: &Files) -> bool {
        let shrinks =
            |path: &RelPath, len: usize| reader.get(path).is_some_and(|bytes| len < bytes.len());
        match self {
            Self::Deliver { .. } | Self::Delete { .. } => false,
            Self::Shorten { path, len } => shrinks(path, *len),
            Self::Resurrect { path, version } => shrinks(path, versions.0[path][*version].len()),
            Self::Hide { .. } | Self::ConflictedCopy { .. } => true,
        }
    }
}

/// Every version of each file the origin has held, as observed.
#[derive(Clone, Default, Debug)]
pub struct Versions(BTreeMap<RelPath, Vec<Vec<u8>>>);

impl Versions {
    /// Every recorded version of every file.
    pub fn iter(&self) -> impl Iterator<Item = (&RelPath, &[u8])> {
        self.0
            .iter()
            .flat_map(|(path, versions)| versions.iter().map(move |bytes| (path, bytes.as_slice())))
    }

    /// Records the origin's current bytes of each file not already recorded.
    pub fn observe(&mut self, origin: &Files) {
        for (path, bytes) in origin {
            let versions = self.0.entry(path.clone()).or_default();
            if !versions.contains(bytes) {
                versions.push(bytes.clone());
            }
        }
    }
}

/// Every event that would change `reader` now, in a fixed order, so a bounded
/// search enumerates delivery orders deterministically. Prefixes end at line
/// boundaries.
pub fn events(origin: &Files, versions: &Versions, reader: &Files) -> Vec<SyncEvent> {
    let mut events = Vec::new();
    for (path, bytes) in origin {
        let mine = reader.get(path);
        if mine != Some(bytes) {
            events.push(SyncEvent::Deliver { path: path.clone() });
            let prefixes = bytes
                .iter()
                .enumerate()
                .filter(|(at, byte)| **byte == b'\n' && at + 1 < bytes.len())
                .map(|(at, _)| at + 1)
                .filter(|len| mine.map(Vec::as_slice) != Some(&bytes[..*len]));
            events.extend(prefixes.map(|len| SyncEvent::Shorten {
                path: path.clone(),
                len,
            }));
            if mine.is_some() {
                let name = conflict_name(path, reader);
                events.push(SyncEvent::ConflictedCopy {
                    path: path.clone(),
                    name,
                });
            }
        }
        if mine.is_some() {
            events.push(SyncEvent::Hide { path: path.clone() });
        }
    }
    let gone = reader.keys().filter(|path| !origin.contains_key(*path));
    events.extend(gone.map(|path| SyncEvent::Delete { path: path.clone() }));
    for (path, kept) in &versions.0 {
        if origin.contains_key(path) {
            continue;
        }
        let differs = |bytes: &Vec<u8>| reader.get(path) != Some(bytes);
        events.extend(
            (0..kept.len())
                .filter(|version| differs(&kept[*version]))
                .map(|version| SyncEvent::Resurrect {
                    path: path.clone(),
                    version,
                }),
        );
    }
    events
}

/// Applies `event` to `reader`.
///
/// ⚠️ Panics on an event [`events`] would not offer: a path the origin or
/// `versions` does not hold, or a prefix longer than the file.
pub fn apply(origin: &Files, versions: &Versions, reader: &mut Files, event: &SyncEvent) {
    match event {
        SyncEvent::Deliver { path } => {
            reader.insert(path.clone(), origin[path].clone());
        }
        SyncEvent::Shorten { path, len } => {
            reader.insert(path.clone(), origin[path][..*len].to_vec());
        }
        SyncEvent::Delete { path } | SyncEvent::Hide { path } => {
            reader.remove(path);
        }
        SyncEvent::Resurrect { path, version } => {
            reader.insert(path.clone(), versions.0[path][*version].clone());
        }
        SyncEvent::ConflictedCopy { path, name } => {
            let parent = path.parent().expect("a file has a parent");
            let copy = parent.join(name).expect("a conflict name is one component");
            reader.insert(copy, origin[path].clone());
        }
    }
}

fn conflict_name(path: &RelPath, reader: &Files) -> String {
    let parent = path.parent().expect("a file has a parent");
    let name = path.name().expect("a file has a name");
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    (1..)
        .map(|n| format!("{stem} (conflicted copy {n}).{extension}"))
        .find(|candidate| {
            parent
                .join(candidate)
                .is_ok_and(|copy| !reader.contains_key(&copy))
        })
        .expect("some number is free")
}

pub struct Simulator {
    origin: MemDisk,
    versions: Versions,
    readers: Vec<MemDisk>,
}

impl Simulator {
    pub fn new(readers: usize) -> Self {
        Self {
            origin: MemDisk::new(),
            versions: Versions::default(),
            readers: (0..readers).map(|_| MemDisk::new()).collect(),
        }
    }

    /// The machine writers write on: its folder is the truth sync carries.
    pub fn origin(&self) -> &MemDisk {
        &self.origin
    }

    /// Reader `index`'s machine: its folder holds what sync delivered, its local
    /// root is its own.
    pub fn reader(&self, index: usize) -> &MemDisk {
        &self.readers[index]
    }

    /// A machine that writes in the origin's folder, with a local root of its own.
    pub fn machine(&self) -> Machine {
        Machine {
            folder: self.origin.clone(),
            local: MemDisk::new(),
        }
    }

    pub fn versions(&self) -> &Versions {
        &self.versions
    }

    /// Every event that would change reader `index`'s folder now, after recording
    /// what writers wrote since the last call.
    pub fn events(&mut self, reader: usize) -> Vec<SyncEvent> {
        let origin = self.observe();
        events(
            &origin,
            &self.versions,
            &self.readers[reader].files(Root::Folder),
        )
    }

    pub fn apply(&mut self, reader: usize, event: &SyncEvent) {
        let origin = self.observe();
        let disk = &self.readers[reader];
        let before = disk.files(Root::Folder);
        let mut after = before.clone();
        apply(&origin, &self.versions, &mut after, event);
        rewrite(disk, &before, &after);
    }

    /// Delivers and deletes until reader `index`'s folder equals the origin's.
    pub fn settle(&mut self, reader: usize) {
        let origin = self.observe();
        let disk = &self.readers[reader];
        rewrite(disk, &disk.files(Root::Folder), &origin);
    }

    /// Whether reader `index`'s folder equals the origin's.
    pub fn delivered(&mut self, reader: usize) -> bool {
        self.observe() == self.readers[reader].files(Root::Folder)
    }

    /// The origin's files now, recorded as versions.
    pub fn observe(&mut self) -> Files {
        let origin = self.origin.files(Root::Folder);
        self.versions.observe(&origin);
        origin
    }
}

/// Changes the folder of `disk` from `before` to `after`, a new file for each
/// changed one, so modification times move.
fn rewrite(disk: &MemDisk, before: &Files, after: &Files) {
    let perform = |io: Io| {
        let shown = format!("{io:?}");
        disk.perform(io)
            .unwrap_or_else(|error| panic!("{shown}: {error}"));
    };
    for path in before
        .keys()
        .filter(|path| after.get(*path) != before.get(*path))
    {
        perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
    }
    for (path, bytes) in after
        .iter()
        .filter(|(path, bytes)| before.get(*path) != Some(*bytes))
    {
        perform(Io::MakeDir {
            root: Root::Folder,
            path: path.parent().expect("a file has a parent"),
        });
        perform(Io::Create {
            root: Root::Folder,
            path: path.clone(),
            bytes: bytes.clone(),
        });
    }
}

/// One instance's machine: a folder, which may be shared with other machines, and
/// a local root of its own.
#[derive(Clone)]
pub struct Machine {
    pub folder: MemDisk,
    pub local: MemDisk,
}

impl Machine {
    /// Another machine on the same folder whose local root is a copy of this one's,
    /// as a restored backup, a cloned disk or copied app data gives.
    pub fn cloned(&self) -> Self {
        let local = MemDisk::new();
        let copy = |io: Io| {
            local.perform(io).expect("copying into an empty disk");
        };
        for path in self.local.directories(Root::Local) {
            copy(Io::MakeDir {
                root: Root::Local,
                path,
            });
        }
        let files = self.local.files(Root::Local);
        for (path, bytes) in &files {
            copy(Io::Create {
                root: Root::Local,
                path: path.clone(),
                bytes: bytes.clone(),
            });
        }
        let names = local
            .directories(Root::Local)
            .into_iter()
            .chain([RelPath::ROOT]);
        for path in files.into_keys().chain(names) {
            copy(Io::Sync {
                root: Root::Local,
                path,
            });
        }
        Self {
            folder: self.folder.clone(),
            local,
        }
    }

    /// The instance stops without closing: its local root keeps what was durable
    /// and its locks are released. The folder is unaffected.
    pub fn crash(&mut self) {
        self.local = self.local.restart();
    }

    /// The local root is lost, as when an install's storage is cleared.
    pub fn lose_local(&mut self) {
        self.local = MemDisk::new();
    }
}

impl Backend for Machine {
    fn capabilities(&self, root: Root) -> Capabilities {
        match root {
            Root::Folder => self.folder.capabilities(root),
            Root::Local => self.local.capabilities(root),
        }
    }

    fn perform(&mut self, io: Io) -> IoResult {
        match io.root() {
            Root::Folder => self.folder.perform(io),
            Root::Local => self.local.perform(io),
        }
    }
}
