//! Copies of one folder, each on its own machine, kept in step as Syncthing keeps them:
//! each path has one global version, made by the copy that changed it last, and reaches
//! every other copy whole, path by path, in any order. The chaos events, and the names of
//! every event, are [`toshokan::simulator`]'s.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use toshokan::simulator::{self, Files, SyncEvent, Versions};
use toshokan::RelPath;

use super::{name, Times};
use crate::testing::Temp;

/// Where each writer of the library keeps its log, in the folder.
const WRITERS: &str = ".drawbar/library/writers.2/";

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Entry {
    Folder,
    File(Vec<u8>),
}

/// An entry, with a file's modification time, which Syncthing carries with its bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Item {
    pub entry: Entry,
    pub modified: Option<SystemTime>,
}

/// Everything in a folder, by `/`-separated path.
pub(super) type Tree = BTreeMap<String, Item>;

pub(super) fn tree(root: &Path) -> Tree {
    fn walk(dir: &Path, at: &str, into: &mut Tree) {
        for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{at}: {e}")) {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let path = match at {
                "" => name,
                _ => format!("{at}/{name}"),
            };
            let item = match entry.file_type().unwrap().is_dir() {
                true => {
                    walk(&entry.path(), &path, into);
                    Item {
                        entry: Entry::Folder,
                        modified: None,
                    }
                }
                false => Item {
                    entry: Entry::File(fs::read(entry.path()).unwrap()),
                    modified: entry.metadata().unwrap().modified().ok(),
                },
            };
            into.insert(path, item);
        }
    }
    let mut found = Tree::new();
    walk(root, "", &mut found);
    found
}

/// The leaf name of a `/`-separated path.
pub(super) fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// One global version of a path.
struct Version {
    number: usize,
    /// `None` once the path is deleted.
    item: Option<Item>,
    /// The copy whose change made it: `None` for what every copy held at the start, and
    /// for a conflict copy.
    from: Option<usize>,
}

struct Copy {
    root: Temp,
    /// What the copy held when the model last looked at it.
    known: Tree,
    /// The global version each path of the copy descends from.
    base: BTreeMap<String, usize>,
}

pub(super) struct Sync {
    copies: Vec<Copy>,
    global: BTreeMap<String, Version>,
    /// Every version of each file, for [`SyncEvent::Resurrect`].
    versions: Versions,
    made: usize,
    conflicts: usize,
    /// Which copy changes each writer's log: the first copy to hold it.
    writers: BTreeMap<String, usize>,
    /// The paths a chaos event or a conflict touched.
    strange: BTreeSet<String>,
    /// What each file a chaos event cut short or brought back held after, which a drawbar
    /// may carry to another path, or another file holds too.
    chaos: BTreeSet<Vec<u8>>,
    /// The order in which the model first saw each path, which the walk's choices follow
    /// so that the seed decides them though drawbar names its own files at random.
    born: BTreeMap<String, usize>,
    /// What dates a file a chaos event writes.
    times: Times,
}

impl Sync {
    /// `copies` copies of what `first` holds, each delivered whole, `first` among them.
    pub fn new(first: Temp, copies: usize, times: Times) -> Sync {
        let known = tree(&first.0);
        let global: BTreeMap<String, Version> = (known.iter().enumerate())
            .map(|(at, (path, item))| {
                let version = Version {
                    number: at + 1,
                    item: Some(item.clone()),
                    from: None,
                };
                (path.clone(), version)
            })
            .collect();
        let first = Copy {
            root: first,
            known,
            base: BTreeMap::new(),
        };
        let others = (1..copies).map(|_| Copy {
            root: Temp::new(),
            known: Tree::new(),
            base: BTreeMap::new(),
        });
        let mut sync = Sync {
            copies: [first].into_iter().chain(others).collect(),
            made: global.len(),
            global,
            versions: Versions::default(),
            conflicts: 0,
            writers: BTreeMap::new(),
            strange: BTreeSet::new(),
            chaos: BTreeSet::new(),
            born: BTreeMap::new(),
            times,
        };
        sync.bear();
        sync.versions.observe(&sync.files());
        for copy in 0..copies {
            sync.rebase(copy);
            sync.settle(copy);
        }
        sync
    }

    pub fn root(&self, copy: usize) -> &Temp {
        &self.copies[copy].root
    }

    /// Whether a chaos event or a conflict touched `path`.
    pub fn strange(&self, path: &str) -> bool {
        self.strange.contains(path)
    }

    /// Whether a chaos event cut short or brought back a file holding `bytes`.
    pub fn chaos_wrote(&self, bytes: &[u8]) -> bool {
        self.chaos.contains(bytes)
    }

    /// Whether a chaos event or a conflict touched a path named `name`.
    pub fn names_strange(&self, name: &str) -> bool {
        self.strange.iter().any(|path| leaf(path) == name)
    }

    /// Forget each path a chaos event or a conflict touched whose version, which every
    /// copy holds once sync settles, holds nothing a chaos event wrote.
    pub fn settled(&mut self) {
        let Sync {
            global,
            strange,
            chaos,
            ..
        } = self;
        strange.retain(|path| {
            let item = global.get(path).and_then(|version| version.item.as_ref());
            matches!(item, Some(Item { entry: Entry::File(bytes), .. }) if chaos.contains(bytes))
        });
    }

    /// Take in what changed in `copy` since the model last looked: each changed path is a
    /// new global version from it. Returns what sync did with a change another copy made
    /// first, or the change that breaks the model: one to a log by a copy other than the
    /// one that writes it.
    pub fn record(&mut self, copy: usize) -> Result<Vec<String>, String> {
        let now = tree(&self.copies[copy].root.0);
        let known = std::mem::take(&mut self.copies[copy].known);
        let paths: BTreeSet<&String> = now.keys().chain(known.keys()).collect();
        let mut notes = Vec::new();
        for path in paths {
            let item = now.get(path);
            if item != known.get(path) {
                let chaos = |item: &Item| match &item.entry {
                    Entry::File(bytes) => self.chaos.contains(bytes),
                    Entry::Folder => false,
                };
                if item.is_some_and(chaos) {
                    self.strange.insert(path.clone());
                }
                self.change(copy, path, item.cloned(), &mut notes)?;
            }
        }
        self.copies[copy].known = now;
        self.keep_folders(copy, &mut notes);
        self.bear();
        self.versions.observe(&self.files());
        self.rebase(copy);
        Ok(notes)
    }

    /// Give each path the model has not seen before its place in [`Sync::born`], by its
    /// name with drawbar's random ones masked and then by its length.
    fn bear(&mut self) {
        let unborn = (self.global.iter()).filter(|(path, _)| !self.born.contains_key(*path));
        let length = |version: &Version| match version.item.as_ref().map(|item| &item.entry) {
            Some(Entry::File(bytes)) => bytes.len(),
            _ => 0,
        };
        let mut unborn: Vec<(String, usize, &String)> = unborn
            .map(|(path, version)| (masked(path), length(version), path))
            .collect();
        unborn.sort();
        let next = self.born.len();
        let born: Vec<String> = unborn.into_iter().map(|(.., path)| path.clone()).collect();
        for (at, path) in born.into_iter().enumerate() {
            self.born.insert(path, next + at);
        }
    }

    /// `events` in the order their paths were born.
    fn in_order(&self, mut events: Vec<SyncEvent>) -> Vec<SyncEvent> {
        events.sort_by_key(|event| self.born.get(event_path(event).as_str()).copied());
        events
    }

    fn change(
        &mut self,
        copy: usize,
        path: &str,
        item: Option<Item>,
        notes: &mut Vec<String>,
    ) -> Result<(), String> {
        if let Some(writer) = path.strip_prefix(WRITERS).and_then(|r| r.split('/').next()) {
            let writes = *self.writers.entry(writer.to_string()).or_insert(copy);
            if writes != copy {
                return Err(format!(
                    "{}'s copy changed {path}, in the log {}'s copy writes",
                    name(copy),
                    name(writes)
                ));
            }
        }
        let (number, held) = match self.global.get(path) {
            Some(version) => (version.number, version.item.clone()),
            None => (0, None),
        };
        let base = self.copies[copy].base.get(path).copied().unwrap_or(0);
        if base == number {
            self.version(path, item, Some(copy));
            return Ok(());
        }
        let entry = |item: &Option<Item>| item.as_ref().map(|item| item.entry.clone());
        if entry(&item) == entry(&held) {
            if item != held {
                self.version(path, item, Some(copy));
            }
            return Ok(());
        }
        match (item, held) {
            // Syncthing keeps a change over a deletion it was concurrent with.
            (None, _) => {
                self.strange.insert(path.to_string());
                let kept = format!(
                    "sync keeps {path} over its deletion in {}'s copy",
                    name(copy)
                );
                notes.push(kept);
            }
            (Some(item), None) => {
                self.strange.insert(path.to_string());
                self.version(path, Some(item), Some(copy));
                notes.push(format!(
                    "sync keeps {}'s {path} over its deletion",
                    name(copy)
                ));
            }
            (Some(item), Some(older)) => {
                self.version(path, Some(item), Some(copy));
                if let Entry::File(_) = older.entry {
                    let conflict = self.conflict_name(path);
                    self.strange.insert(conflict.clone());
                    self.version(&conflict, Some(older), None);
                    notes.push(format!(
                        "sync keeps {}'s {path} and moves the other to {conflict}",
                        name(copy)
                    ));
                }
            }
        }
        Ok(())
    }

    fn version(&mut self, path: &str, item: Option<Item>, from: Option<usize>) {
        self.made += 1;
        let version = Version {
            number: self.made,
            item,
            from,
        };
        self.global.insert(path.to_string(), version);
        if let Some(copy) = from {
            self.copies[copy].base.insert(path.to_string(), self.made);
        }
    }

    /// `Strings.sync-conflict-<n>.ne5p` beside `path`, as Syncthing names a conflict copy.
    fn conflict_name(&mut self, path: &str) -> String {
        self.conflicts += 1;
        let n = self.conflicts;
        let (dir, name) = match path.rsplit_once('/') {
            Some((dir, name)) => (format!("{dir}/"), name),
            None => (String::new(), path),
        };
        match name.rsplit_once('.') {
            Some((stem, extension)) => format!("{dir}{stem}.sync-conflict-{n}.{extension}"),
            None => format!("{dir}{name}.sync-conflict-{n}"),
        }
    }

    /// A folder one copy deleted while another put something in it stays, as Syncthing
    /// cannot delete a folder that is not empty.
    fn keep_folders(&mut self, copy: usize, notes: &mut Vec<String>) {
        let live = (self.global.iter()).filter(|(_, version)| version.item.is_some());
        let mut kept = BTreeSet::new();
        for (path, _) in live {
            let mut at = path.as_str();
            while let Some((parent, _)) = at.rsplit_once('/') {
                let folder = self.global.get(parent).and_then(|v| v.item.as_ref());
                if folder.map(|item| &item.entry) != Some(&Entry::Folder) {
                    kept.insert(parent.to_string());
                }
                at = parent;
            }
        }
        for folder in kept {
            self.strange.insert(folder.clone());
            let item = Item {
                entry: Entry::Folder,
                modified: None,
            };
            self.version(&folder, Some(item), Some(copy));
            notes.push(format!("sync keeps the folder {folder}"));
        }
    }

    /// A copy that holds a path's global version descends from it.
    fn rebase(&mut self, copy: usize) {
        let Copy { known, base, .. } = &mut self.copies[copy];
        for (path, version) in &self.global {
            if known.get(path) == version.item.as_ref() {
                base.insert(path.clone(), version.number);
            }
        }
    }

    /// Every file a global version holds now, as the simulator's origin.
    fn files(&self) -> Files {
        let held = self.global.iter().filter_map(|(path, version)| {
            let Entry::File(bytes) = &version.item.as_ref()?.entry else {
                return None;
            };
            Some((relative(path), bytes.clone()))
        });
        held.collect()
    }

    fn held(&self, copy: usize) -> Files {
        let held = self.copies[copy].known.iter().filter_map(|(path, item)| {
            let Entry::File(bytes) = &item.entry else {
                return None;
            };
            Some((relative(path), bytes.clone()))
        });
        held.collect()
    }

    /// Each path whose copy in `copy` is behind its global version, delivered whole: its
    /// bytes, or its deletion. A folder is deleted once nothing is in it.
    pub fn deliveries(&self, copy: usize) -> Vec<SyncEvent> {
        let known = &self.copies[copy].known;
        let inside = |folder: &str| {
            let prefix = format!("{folder}/");
            known.keys().any(|path| path.starts_with(&prefix))
        };
        let stray = known.keys().filter(|path| !self.global.contains_key(*path));
        let behind = (self.global.iter())
            .filter(|(path, version)| known.get(*path) != version.item.as_ref())
            .map(|(path, version)| (path, version.item.is_some()))
            .chain(stray.map(|path| (path, false)));
        let deliverable = behind.filter(|(path, delivers)| *delivers || !inside(path));
        let events = deliverable.map(|(path, delivers)| {
            let path = relative(path);
            match delivers {
                true => SyncEvent::Deliver { path },
                false => SyncEvent::Delete { path },
            }
        });
        self.in_order(events.collect())
    }

    /// The chaos the simulator offers on `copy`'s files that another copy changed last:
    /// a file shortened to a prefix, hidden for a while, or an old version of a deleted
    /// file brought back.
    pub fn chaos(&self, copy: usize) -> Vec<SyncEvent> {
        let elsewhere = |path: &RelPath| {
            (self.global.get(path.as_str())).is_some_and(|version| version.from != Some(copy))
        };
        let events = simulator::events(&self.files(), &self.versions, &self.held(copy));
        let chaos = events.into_iter().filter(|event| match event {
            SyncEvent::Shorten { path, .. }
            | SyncEvent::Hide { path }
            | SyncEvent::Resurrect { path, .. } => elsewhere(path),
            SyncEvent::Deliver { .. }
            | SyncEvent::Delete { .. }
            | SyncEvent::ConflictedCopy { .. } => false,
        });
        self.in_order(chaos.collect())
    }

    /// Apply `event`, one of [`Sync::deliveries`] or [`Sync::chaos`], to `copy`.
    pub fn apply(&mut self, copy: usize, event: &SyncEvent) {
        let root = self.copies[copy].root.0.clone();
        let applied = match event {
            SyncEvent::Deliver { path } => {
                let version = &self.global[path.as_str()];
                let item = version.item.as_ref().expect("a delivery holds something");
                match &item.entry {
                    Entry::Folder => remove(&root, path.as_str()).and_then(|()| {
                        fs::create_dir_all(root.join(path.as_str())).map_err(|e| e.to_string())
                    }),
                    Entry::File(bytes) => {
                        let modified = item.modified.expect("a file has a time");
                        put(&root, path.as_str(), bytes, modified)
                    }
                }
            }
            SyncEvent::Delete { path } => remove(&root, path.as_str()),
            SyncEvent::Shorten { path, .. }
            | SyncEvent::Hide { path }
            | SyncEvent::Resurrect { path, .. } => {
                let known = &self.copies[copy].known;
                let mut at = path.as_str();
                while let Some((parent, _)) = at.rsplit_once('/') {
                    if !known.contains_key(parent) {
                        self.strange.insert(parent.to_string());
                    }
                    at = parent;
                }
                self.strange.insert(path.as_str().to_string());
                let mut held = self.held(copy);
                simulator::apply(&self.files(), &self.versions, &mut held, event);
                let wrote = matches!(
                    event,
                    SyncEvent::Shorten { .. } | SyncEvent::Resurrect { .. }
                );
                if let (true, Some(bytes)) = (wrote, held.get(path)) {
                    self.chaos.insert(bytes.clone());
                }
                match held.get(path) {
                    Some(bytes) => {
                        let time = self.times.stamp(&root.join(path.as_str()), None);
                        put(&root, path.as_str(), bytes, time)
                    }
                    None => remove(&root, path.as_str()),
                }
            }
            SyncEvent::ConflictedCopy { .. } => Err("sync names its conflict copies".into()),
        };
        applied.unwrap_or_else(|e| panic!("{event:?} in {}'s copy: {e}", name(copy)));
        self.copies[copy].known = tree(&root);
        self.rebase(copy);
    }

    /// Deliver everything `copy` is behind on, with no chaos, and say what arrived.
    pub fn settle(&mut self, copy: usize) -> Vec<SyncEvent> {
        let mut delivered = Vec::new();
        while let Some(event) = self.deliveries(copy).into_iter().next() {
            assert!(
                delivered.len() < 100_000,
                "sync settles {}'s copy",
                name(copy)
            );
            self.apply(copy, &event);
            delivered.push(event);
        }
        delivered
    }
}

fn event_path(event: &SyncEvent) -> &RelPath {
    match event {
        SyncEvent::Deliver { path }
        | SyncEvent::Shorten { path, .. }
        | SyncEvent::Delete { path }
        | SyncEvent::Hide { path }
        | SyncEvent::Resurrect { path, .. }
        | SyncEvent::ConflictedCopy { path, .. } => path,
    }
}

/// `path` with each run of 32 hexadecimal digits, a name drawbar draws at random, masked.
fn masked(path: &str) -> String {
    let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
    let mut masked = String::new();
    let mut run = String::new();
    for c in path.chars().chain(['/']) {
        if hex(c) {
            run.push(c);
            continue;
        }
        match run.len() {
            32 => masked.push('#'),
            _ => masked.push_str(&run),
        }
        run.clear();
        masked.push(c);
    }
    masked.pop();
    masked
}

fn relative(path: &str) -> RelPath {
    RelPath::new(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Write `bytes` at `path` as Syncthing does: whole, under a temporary name, with the
/// origin's time, then renamed into place.
fn put(root: &Path, path: &str, bytes: &[u8], modified: SystemTime) -> Result<(), String> {
    let target = root.join(path);
    let parent = target.parent().expect("a path in a folder has a parent");
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    remove(root, path)?;
    let staged: PathBuf = parent.join(format!(".syncthing.{}.tmp", leaf(path)));
    fs::write(&staged, bytes).map_err(|e| e.to_string())?;
    let file = fs::File::options().write(true).open(&staged);
    file.and_then(|file| file.set_modified(modified))
        .map_err(|e| e.to_string())?;
    fs::rename(&staged, &target).map_err(|e| e.to_string())
}

/// Remove the file or the empty folder at `path`, if anything is there.
fn remove(root: &Path, path: &str) -> Result<(), String> {
    let target = root.join(path);
    let removed = match fs::symlink_metadata(&target) {
        Err(_) => Ok(()),
        Ok(meta) if meta.is_dir() => fs::remove_dir(&target),
        Ok(_) => fs::remove_file(&target),
    };
    removed.map_err(|e| e.to_string())
}
