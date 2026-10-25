//! Drives the native backend through short scripted workloads and checks what a
//! power loss leaves of one. The NixOS tests in `power-loss/` run it on real file
//! systems, crash them and replay their writes.
//!
//! ```text
//! power_loss list                  # the workloads, and how many steps each takes
//! power_loss run WORKLOAD DIR MARK # MARK is dm:<device> or file:<path>
//! power_loss check WORKLOAD DIR N [ENTRY…]
//! ```
//!
//! `run` performs the workload's setup in `DIR` and marks `setup`, then marks
//! `step-<i>` once step `i` returns, as a dm-log-writes mark or a synced line,
//! and prints each mark. A step that commits an entry marks
//! `step-<i>:<writer>/<n>/<hash>`.
//! `check` opens what a crash left in `DIR` as the same install, once `N` steps
//! returned, committing the `ENTRY`s their marks name, with the workload's
//! hand-written model as the oracle of what each step changed.

// Empty on wasm, which has no native backend.
#![cfg_attr(target_arch = "wasm32", no_main)]
#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::rc::Rc;

use toshokan::blocking::{self, Backend, Library, Native};
use toshokan::derive::{Derived, Placed};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::io::Capabilities;
use toshokan::layout::{SEGMENT_EXTENSION, WRITERS};
use toshokan::line;
use toshokan::log::{Entry, Own, Settlement};
use toshokan::pending::{self, PendingRecord};
use toshokan::reader::Reader;
use toshokan::report::{Opened, Rekey, Start};
use toshokan::{
    EntryHash, Env, Expect, Identify, Identity, Io, IoError, IoResult, Layout, Register, RelPath,
    Root, Schema, Set, View, WriterId,
};

const NAME: Register<String> = Register::new("name");
const TAGS: Set<String> = Set::new("tags");
/// The entity the checker commits to make the writer settle and write again.
const PROBE: &str = "probe";
const ROOT: &str = ".t";

type Bytes = &'static [u8];

/// What the folder holds before the library is first opened. No step touches it.
const BEFORE: &[(&str, Bytes)] = &[("notes/readme.txt", b"the user's own notes")];

#[derive(Clone, Copy, Debug)]
enum Step {
    Open,
    Close,
    /// Creates entity `name` with its file at `path`, where nothing is.
    Import {
        name: &'static str,
        path: &'static str,
        bytes: Bytes,
    },
    Tag {
        name: &'static str,
        tag: &'static str,
    },
    /// Saves `bytes` over the entity's file.
    Save {
        name: &'static str,
        bytes: Bytes,
    },
    Rename {
        name: &'static str,
        to: &'static str,
    },
    MoveTree {
        from: &'static str,
        to: &'static str,
    },
    Trash {
        name: &'static str,
    },
    Undo,
    Redo,
    Compact,
    /// Empties the whole trash.
    EmptyTrash,
    /// A save over the entity's file whose process stops just before the new
    /// bytes are placed; the install then loses its local root.
    Interrupt {
        name: &'static str,
        bytes: Bytes,
    },
    /// Finishes every orphan the last open reported.
    Settle,
}

struct Workload {
    name: &'static str,
    /// Steps whose crashes are not checked.
    setup: Vec<Step>,
    steps: Vec<Step>,
}

fn workloads() -> Vec<Workload> {
    use Step::*;
    let import = |name, path, bytes| Import { name, path, bytes };
    let song = import("song", "a/song.syx", b"song one");
    vec![
        Workload {
            name: "first-writer",
            setup: vec![],
            steps: vec![
                Open,
                song,
                Tag {
                    name: "song",
                    tag: "kept",
                },
                Close,
            ],
        },
        Workload {
            name: "save",
            setup: vec![Open, song],
            steps: vec![
                Save {
                    name: "song",
                    bytes: b"song two!",
                },
                import("other", "a/other.syx", b"other one"),
                Save {
                    name: "song",
                    bytes: b"song three",
                },
                Close,
            ],
        },
        Workload {
            name: "rename",
            setup: vec![Open, song, import("other", "a/b/other.syx", b"other one")],
            steps: vec![
                Rename {
                    name: "song",
                    to: "a/b/renamed.syx",
                },
                MoveTree { from: "a", to: "c" },
                Close,
            ],
        },
        Workload {
            name: "trash",
            setup: vec![Open, song, import("other", "b/other.syx", b"other one")],
            steps: vec![
                Trash { name: "song" },
                Undo,
                Trash { name: "other" },
                EmptyTrash,
                Close,
            ],
        },
        Workload {
            name: "undo",
            setup: vec![
                Open,
                song,
                Save {
                    name: "song",
                    bytes: b"song two!",
                },
            ],
            steps: vec![Undo, Redo, Undo, Close],
        },
        Workload {
            name: "upkeep",
            setup: vec![
                Open,
                song,
                Tag {
                    name: "song",
                    tag: "x",
                },
                Save {
                    name: "song",
                    bytes: b"song two!",
                },
                Tag {
                    name: "song",
                    tag: "y",
                },
            ],
            steps: vec![
                Compact,
                Tag {
                    name: "song",
                    tag: "z",
                },
                Close,
            ],
        },
        Workload {
            name: "reopen",
            setup: vec![Open, song],
            steps: vec![
                Close,
                Open,
                Tag {
                    name: "song",
                    tag: "again",
                },
                Close,
            ],
        },
        Workload {
            name: "settle",
            setup: vec![Open, song],
            steps: vec![
                Interrupt {
                    name: "song",
                    bytes: b"song two!",
                },
                Open,
                Settle,
                Close,
            ],
        },
    ]
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct Model {
    /// Every library file, by path.
    files: BTreeMap<String, Vec<u8>>,
    /// Every entity but the probe, by name.
    entities: BTreeMap<String, Entity>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct Entity {
    tags: BTreeSet<String>,
    file: Option<String>,
}

impl Model {
    fn entity(&mut self, name: &str) -> &mut Entity {
        self.entities.get_mut(name).expect("the workload names it")
    }

    fn file(&self, name: &str) -> (String, Vec<u8>) {
        let path = self.entities[name].file.clone().expect("it has a file");
        let bytes = self.files[&path].clone();
        (path, bytes)
    }

    /// The model after `step`, with `history` holding the models before each of
    /// this writer's intents, then those undone.
    fn after(&self, step: Step, history: &mut (Vec<Model>, Vec<Model>)) -> Model {
        let mut next = self.clone();
        match step {
            Step::Open | Step::Close | Step::Compact | Step::EmptyTrash | Step::Settle => {
                return next
            }
            Step::Undo => {
                let before = history.0.pop().expect("something to undo");
                history.1.push(next);
                return before;
            }
            Step::Redo => {
                let after = history.1.pop().expect("something to redo");
                history.0.push(next);
                return after;
            }
            Step::Import { name, path, bytes } => {
                next.files.insert(path.into(), bytes.to_vec());
                let file = Some(path.into());
                let entity = Entity {
                    tags: BTreeSet::new(),
                    file,
                };
                next.entities.insert(name.into(), entity);
            }
            Step::Tag { name, tag } => {
                next.entity(name).tags.insert(tag.into());
            }
            Step::Save { name, bytes } | Step::Interrupt { name, bytes } => {
                let (path, _) = self.file(name);
                next.files.insert(path, bytes.to_vec());
            }
            Step::Rename { name, to } => {
                let (path, bytes) = self.file(name);
                next.files.remove(&path);
                next.files.insert(to.into(), bytes);
                next.entity(name).file = Some(to.into());
            }
            Step::MoveTree { from, to } => {
                let moved = |path: &str| match path.strip_prefix(from) {
                    Some(rest) if rest.starts_with('/') => format!("{to}{rest}"),
                    _ => path.to_owned(),
                };
                next.files = self
                    .files
                    .iter()
                    .map(|(p, b)| (moved(p), b.clone()))
                    .collect();
                for entity in next.entities.values_mut() {
                    entity.file = entity.file.as_deref().map(moved);
                }
            }
            Step::Trash { name } => {
                let (path, _) = self.file(name);
                next.files.remove(&path);
                next.entity(name).file = None;
            }
        }
        history.0.push(self.clone());
        history.1.clear();
        next
    }

    /// Library files no entity's file is.
    fn unbound(&self) -> Vec<String> {
        let bound: BTreeSet<&String> = self.entities.values().flat_map(|e| &e.file).collect();
        self.files
            .keys()
            .filter(|p| !bound.contains(p))
            .cloned()
            .collect()
    }
}

/// The model before the first step, then after each setup step and each step.
fn models(workload: &Workload) -> Vec<Model> {
    let mut model = Model::default();
    for (path, bytes) in BEFORE {
        model.files.insert((*path).into(), bytes.to_vec());
    }
    let mut history = Default::default();
    let mut models = vec![model];
    for &step in workload.setup.iter().chain(&workload.steps) {
        let next = models.last().unwrap().after(step, &mut history);
        models.push(next);
    }
    models
}

/// The bytes the folder must hold somewhere, a trash included, once the step
/// that reached model `at` returned: emptying the trash is the only way bytes
/// leave it, and the step after may be emptying it.
fn kept(workload: &Workload, models: &[Model], at: usize) -> BTreeSet<Vec<u8>> {
    let steps: Vec<Step> = workload
        .setup
        .iter()
        .chain(&workload.steps)
        .copied()
        .collect();
    let mut kept = BTreeSet::new();
    for (i, model) in models.iter().enumerate().take(at + 2) {
        let emptied = i > 0 && matches!(steps[i - 1], Step::EmptyTrash);
        if emptied {
            kept.clear();
        }
        if i <= at || emptied {
            kept.extend(model.files.values().cloned());
        }
    }
    kept
}

struct Dirs {
    folder: PathBuf,
    local: PathBuf,
}

impl Dirs {
    fn new(dir: &Path) -> Self {
        Self {
            folder: dir.join("folder"),
            local: dir.join("local"),
        }
    }
}

fn layout() -> Layout {
    Layout::new(ROOT).unwrap()
}

fn schema() -> Schema {
    Schema::of(&[NAME.key(), TAGS.key()]).unwrap()
}

const IDENTIFY: PrefixIdentity = PrefixIdentity { prefix: 64 * 1024 };

fn identity(bytes: &[u8]) -> Identity {
    IDENTIFY.of_bytes(&RelPath::ROOT, bytes)
}

fn env(seed: u64, clock: &TestClock) -> Env {
    Env {
        clock: Box::new(clock.clone()),
        random: Box::new(SeededRandom::new(seed)),
        identify: Rc::new(IDENTIFY),
        names: Box::new(ExactNames),
        label: "power-loss".into(),
    }
}

fn rel(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

/// The native backend, which stops performing anything once a rename or a copy
/// is asked to make the folder path it is armed with, as a process that dies
/// there does.
struct Stopping {
    native: Native,
    at: Rc<RefCell<Option<RelPath>>>,
    stopped: bool,
}

impl Backend for Stopping {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.native.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let made = match &io {
            Io::Rename {
                root: Root::Folder,
                to,
                ..
            } => Some(to),
            Io::Create {
                root: Root::Folder,
                path,
                ..
            } => Some(path),
            _ => None,
        };
        if made.is_some() && made == self.at.borrow().as_ref() {
            self.stopped = true;
        }
        match self.stopped {
            true => Err(IoError::Other("the process stopped".into())),
            false => self.native.perform(io),
        }
    }
}

enum Marks {
    Device(String),
    File(File),
}

impl Marks {
    fn parse(spec: &str) -> Result<Self, String> {
        if let Some(device) = spec.strip_prefix("dm:") {
            return Ok(Self::Device(device.into()));
        }
        let Some(path) = spec.strip_prefix("file:") else {
            return Err(format!("{spec} is neither dm:<device> nor file:<path>"));
        };
        let file = OpenOptions::new().create(true).append(true).open(path);
        file.map(Self::File).map_err(|e| format!("{path}: {e}"))
    }

    fn mark(&mut self, label: &str) -> Result<(), String> {
        match self {
            Self::Device(device) => {
                let status = Command::new("dmsetup")
                    .args(["message", device, "0", "mark", label])
                    .status()
                    .map_err(|e| format!("dmsetup: {e}"))?;
                match status.success() {
                    true => Ok(()),
                    false => Err(format!("dmsetup could not mark {label}: {status}")),
                }
            }
            Self::File(file) => writeln!(file, "{label}")
                .and_then(|()| file.sync_all())
                .map_err(|e| format!("marking {label}: {e}")),
        }
    }
}

fn sync_dir(dir: &Path) -> Result<(), String> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("syncing {}: {e}", dir.display()))
}

/// Creates `dir` and every directory above it that is missing, durably.
fn make_dirs(dir: &Path) -> Result<(), String> {
    if dir.is_dir() {
        return Ok(());
    }
    let parent = dir.parent().expect("not the root");
    make_dirs(parent)?;
    fs::create_dir(dir).map_err(|e| format!("making {}: {e}", dir.display()))?;
    sync_dir(parent)
}

struct Runner {
    dirs: Dirs,
    clock: TestClock,
    stop: Rc<RefCell<Option<RelPath>>>,
    library: Option<Library<Stopping>>,
    opened: Option<Opened>,
    opens: u64,
}

impl Runner {
    fn library(&mut self) -> &mut Library<Stopping> {
        self.library.as_mut().expect("the workload opened it")
    }

    fn entity(&self, name: &str) -> toshokan::EntityId {
        let view = self.library.as_ref().expect("open").view();
        let [id] = view.find(NAME, &name.to_owned())[..] else {
            panic!("one entity is named {name}");
        };
        id
    }

    /// Performs `step` and returns the entry it committed, if it commits one.
    fn perform(&mut self, step: Step, before: &Model) -> Result<Option<EntryHash>, String> {
        let shown = |e: toshokan::Error| format!("{step:?}: {e}");
        let committed = |done: toshokan::Result<toshokan::Committed>| {
            done.map(|committed| Some(committed.intent)).map_err(shown)
        };
        let entry = match step {
            Step::Open => {
                let backend = Stopping {
                    native: Native::new(&self.dirs.folder, &self.dirs.local),
                    at: Rc::clone(&self.stop),
                    stopped: false,
                };
                self.opens += 1;
                let env = env(self.opens, &self.clock);
                let (library, opened) =
                    Library::open(backend, layout(), &schema(), env).map_err(shown)?;
                self.library = Some(library);
                self.opened = Some(opened);
                None
            }
            Step::Close => {
                let library = self.library.take().expect("open");
                library.close().map_err(shown)?;
                None
            }
            Step::Import { name, path, bytes } => {
                let (intent, _) = self.library().intent("Import").create(|e| {
                    e.save(&rel(path), bytes.to_vec(), Expect::Absent)
                        .set(NAME, name.into());
                });
                committed(intent.commit())?
            }
            Step::Tag { name, tag } => {
                let id = self.entity(name);
                let intent = self.library().intent("Tag").add(id, TAGS, tag.into());
                committed(intent.commit())?
            }
            Step::Save { name, bytes } => {
                let id = self.entity(name);
                let (path, old) = before.file(name);
                let expect = Expect::Holds(identity(&old));
                let intent = self.library().intent("Save");
                committed(
                    intent
                        .save(id, &rel(&path), bytes.to_vec(), expect)
                        .commit(),
                )?
            }
            Step::Rename { name, to } => {
                let id = self.entity(name);
                let expect = Expect::Holds(identity(&before.file(name).1));
                let intent = self.library().intent("Rename").rename(id, &rel(to), expect);
                committed(intent.commit())?
            }
            Step::MoveTree { from, to } => {
                let intent = self
                    .library()
                    .intent("Move")
                    .move_tree(&rel(from), &rel(to));
                committed(intent.commit())?
            }
            Step::Trash { name } => {
                let id = self.entity(name);
                let expect = Expect::Holds(identity(&before.file(name).1));
                let intent = self.library().intent("Trash").trash(id, expect);
                committed(intent.commit())?
            }
            Step::Undo => committed(self.library().undo())?,
            Step::Redo => committed(self.library().redo())?,
            Step::Compact => self.library().compact().map(|_| None).map_err(shown)?,
            Step::EmptyTrash => {
                let emptied = self.library().empty_trash(Default::default());
                let emptied = emptied.map_err(shown)?;
                if emptied.removed.is_empty() {
                    return Err(format!("{step:?} removed nothing"));
                }
                None
            }
            Step::Interrupt { name, bytes } => {
                let id = self.entity(name);
                let (path, old) = before.file(name);
                *self.stop.borrow_mut() = Some(rel(&path));
                let intent = self.library().intent("Save");
                let expect = Expect::Holds(identity(&old));
                let saved = intent
                    .save(id, &rel(&path), bytes.to_vec(), expect)
                    .commit();
                *self.stop.borrow_mut() = None;
                if saved.is_ok() {
                    return Err(format!("{step:?} was not stopped"));
                }
                self.library = None;
                fs::remove_dir_all(&self.dirs.local).map_err(|e| format!("{step:?}: {e}"))?;
                make_dirs(&self.dirs.local)?;
                None
            }
            Step::Settle => {
                let orphans = self.opened.as_ref().expect("open").orphaned.clone();
                if orphans.is_empty() {
                    return Err(format!("{step:?}: no orphan was reported"));
                }
                let mut settled = None;
                for orphan in orphans {
                    settled = committed(self.library().settle(orphan, Settlement::Finished))?;
                }
                settled
            }
        };
        self.clock.advance(1000);
        Ok(entry)
    }
}

/// Every file under `root`, by path relative to it with `/` between names.
fn tree(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().into_string().expect("UTF-8 names");
            let path = format!("{prefix}{name}");
            match entry.file_type()?.is_dir() {
                true => walk(&entry.path(), &format!("{path}/"), out)?,
                false => drop(out.insert(path, fs::read(entry.path())?)),
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out).map_err(|e| format!("reading {}: {e}", root.display()))?;
    Ok(out)
}

/// The paths whose bytes differ between `before` and `after`, or that only one
/// holds.
fn changed(
    before: &BTreeMap<String, Vec<u8>>,
    after: &BTreeMap<String, Vec<u8>>,
) -> BTreeSet<String> {
    let differs = |path: &&String| before.get(*path) != after.get(*path);
    before
        .keys()
        .chain(after.keys())
        .filter(differs)
        .cloned()
        .collect()
}

fn library_files(tree: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    let owned = format!("{ROOT}/");
    tree.iter()
        .filter(|(p, _)| !p.starts_with(&owned))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

/// What `view` says of each entity but the probe.
fn facts(view: &View) -> Result<BTreeMap<String, Entity>, String> {
    let mut facts = BTreeMap::new();
    for entity in view.entities() {
        let field = entity.get(NAME);
        let Some(name) = field.shown().cloned() else {
            return Err(format!("{:?} has no name: {field:?}", entity.id()));
        };
        if matches!(field, toshokan::Field::Conflict { .. }) {
            return Err(format!("{name}'s name is in conflict: {field:?}"));
        }
        if name == PROBE {
            continue;
        }
        let tags = entity.members(TAGS).values.into_iter().collect();
        let file = entity.file().map(|f| f.path.as_str().to_owned());
        facts.insert(name, Entity { tags, file });
    }
    Ok(facts)
}

fn verify(runner: &Runner, model: &Model, settling: bool) -> Result<(), String> {
    let files = library_files(&tree(&runner.dirs.folder)?);
    if !settling && files != model.files {
        return Err(format!("the folder holds {files:?}, not {:?}", model.files));
    }
    let Some(library) = &runner.library else {
        return Ok(());
    };
    let shown = facts(&library.view())?;
    if !settling && shown != model.entities {
        return Err(format!(
            "the view shows {shown:?}, not {:?}",
            model.entities
        ));
    }
    Ok(())
}

fn run(workload: &Workload, dir: &Path, marks: &mut Marks) -> Result<(), String> {
    let dirs = Dirs::new(dir);
    make_dirs(&dirs.local)?;
    for (path, bytes) in BEFORE {
        let full = dirs.folder.join(path);
        make_dirs(full.parent().unwrap())?;
        let mut file = File::create_new(&full).map_err(|e| format!("{path}: {e}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("{path}: {e}"))?;
        sync_dir(full.parent().unwrap())?;
    }
    let models = models(workload);
    let mut runner = Runner {
        dirs,
        clock: TestClock::at(1_700_000_000_000),
        stop: Rc::default(),
        library: None,
        opened: None,
        opens: 0,
    };
    let mut settling = false;
    let all = workload.setup.iter().chain(&workload.steps);
    for (i, &step) in all.enumerate() {
        if i == workload.setup.len() {
            marks.mark("setup")?;
        }
        let entry = runner.perform(step, &models[i])?;
        settling = match step {
            Step::Interrupt { .. } => true,
            Step::Settle => false,
            _ => settling,
        };
        verify(&runner, &models[i + 1], settling).map_err(|e| format!("after {step:?}: {e}"))?;
        if let Some(n) = i.checked_sub(workload.setup.len()) {
            let label = match entry {
                Some(entry) => format!("step-{}:{}", n + 1, Acked::find(&runner.dirs, entry)?),
                None => format!("step-{}", n + 1),
            };
            marks.mark(&label)?;
            println!("{label}");
        }
    }
    Ok(())
}

/// What a check found beyond pass or fail, for the tally.
struct Checked {
    model: usize,
    start: Start,
    settled: usize,
    orphans: usize,
    unreadable: usize,
}

/// An entry an acknowledged step committed, by its writer, number and hash:
/// written `<writer>/<n>/<hash>`.
#[derive(Clone, Copy, Debug)]
struct Acked {
    writer: WriterId,
    at: Own,
}

impl Acked {
    /// The entry `hash` as a reader of the folder finds it.
    fn find(dirs: &Dirs, hash: EntryHash) -> Result<Self, String> {
        let derived = read_logs(dirs)?;
        let found = derived.writers.iter().find_map(|(writer, placed)| {
            let lines = placed.merged.iter().chain(&placed.runs);
            let line = lines.into_iter().find(|entry| entry.hash() == hash)?;
            Some(Acked {
                writer: *writer,
                at: line.own(),
            })
        });
        found.ok_or_else(|| format!("no writer's line is {hash}"))
    }
}

impl std::fmt::Display for Acked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.writer, self.at.n, self.at.hash)
    }
}

impl std::str::FromStr for Acked {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || format!("{text} is not <writer>/<n>/<hash>");
        let [writer, n, hash] = text.split('/').collect::<Vec<_>>()[..] else {
            return Err(bad());
        };
        Ok(Acked {
            writer: writer.parse().map_err(|_| bad())?,
            at: Own {
                n: n.parse().map_err(|_| bad())?,
                hash: hash.parse().map_err(|_| bad())?,
            },
        })
    }
}

/// Whether a reader of the folder holds `at` of `writer`: as a line, named by a
/// tip, or numbered at or below a tip's head, which folds its chain.
fn holds(derived: &Derived, writer: WriterId, at: Own) -> bool {
    let Some(placed) = derived.writers.get(&writer) else {
        return false;
    };
    let passed = placed.tips.iter().any(|tip| tip.head.n >= at.n);
    placed.holds(at.hash) || passed
}

/// Every entry a reader holds as a line or by a tip's name, by writer.
fn held(derived: &Derived) -> Vec<(WriterId, Own)> {
    let mut held = Vec::new();
    for (writer, placed) in &derived.writers {
        let lines = placed.merged.iter().chain(&placed.runs).chain(&placed.gaps);
        held.extend(lines.map(|entry| (*writer, entry.own())));
        for tip in &placed.tips {
            let named = tip.certified.named().chain([tip.head]);
            held.extend(named.map(|at| (*writer, at)));
        }
    }
    held
}

/// The entries no held line follows: a writer's last lines and tips' heads.
fn heads(placed: &Placed) -> Vec<EntryHash> {
    let lines: Vec<&Rc<Entry>> = placed.merged.iter().chain(&placed.runs).collect();
    let followed: BTreeSet<EntryHash> = lines.iter().map(|entry| entry.prev()).collect();
    let tips = placed.tips.iter().map(|tip| tip.head.hash);
    let all = lines.iter().map(|entry| entry.hash()).chain(tips);
    let heads: BTreeSet<EntryHash> = all.filter(|hash| !followed.contains(hash)).collect();
    heads.into_iter().collect()
}

/// Whether the line `from` descends from `to`, through lines a reader holds.
fn descends(placed: &Placed, from: EntryHash, to: EntryHash) -> bool {
    let lines = placed.merged.iter().chain(&placed.runs).chain(&placed.gaps);
    let prev: BTreeMap<EntryHash, EntryHash> =
        lines.map(|entry| (entry.hash(), entry.prev())).collect();
    let mut at = Some(from);
    while let Some(hash) = at {
        if hash == to {
            return true;
        }
        at = prev.get(&hash).copied();
    }
    false
}

fn read_logs(dirs: &Dirs) -> Result<Derived, String> {
    let mut native = Native::new(&dirs.folder, &dirs.local);
    let mut reader = Reader::new(layout());
    blocking::run(&mut native, reader.read()).map_err(|e| format!("reading the logs: {e}"))?;
    Ok(reader.derived().clone())
}

/// The paths each pending record in the folder names. A record cut short before
/// its bytes landed names none.
fn records(tree: &BTreeMap<String, Vec<u8>>) -> Result<Vec<RelPath>, String> {
    let mut named = Vec::new();
    for (path, bytes) in tree {
        let parts: Vec<&str> = path.split('/').collect();
        let [ROOT, WRITERS, _, "pending", name] = parts[..] else {
            continue;
        };
        if !name.ends_with(".json") || pending::cut_short(bytes) {
            continue;
        }
        let record = PendingRecord::decode(bytes).map_err(|e| format!("{path}: {e:?}"))?;
        named.extend(record.paths());
    }
    Ok(named)
}

/// Each entry of every segment re-encodes to the bytes it was read from.
fn round_trip(tree: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    for (path, bytes) in tree {
        let parts: Vec<&str> = path.split('/').collect();
        let [ROOT, WRITERS, _, name] = parts[..] else {
            continue;
        };
        if !name.ends_with(SEGMENT_EXTENSION) {
            continue;
        }
        let read = line::read(bytes, 0, None);
        let mut again = Vec::new();
        for line in read.lines {
            let entry = Entry::decode(line).map_err(|e| format!("{path}: {e}"))?;
            again.extend(entry.to_bytes());
        }
        let end = usize::try_from(read.end).unwrap();
        if again != bytes[..end] {
            return Err(format!("{path}'s entries do not re-encode to its bytes"));
        }
    }
    Ok(())
}

/// What a crash left of a workload once `acked` of its steps returned.
struct Crash<'w> {
    workload: &'w Workload,
    dirs: Dirs,
    models: Vec<Model>,
    /// The models recovery may reach: after the last step that returned, and
    /// after the one in flight.
    candidates: [usize; 2],
}

impl Crash<'_> {
    /// Each library path holds its old bytes, its new bytes, or, while a pending
    /// record names it, nothing or where moves copy a copy cut short; no bytes
    /// left the folder; logs re-encode.
    fn before_recovery(&self, raw: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
        self.kept_in(raw, "left the folder")?;
        let named = records(raw)?;
        let library = library_files(raw);
        let native = Native::new(&self.dirs.folder, &self.dirs.local);
        let copying = !native.capabilities(Root::Folder).rename_file;
        let [a, b] = self.candidates.map(|i| &self.models[i].files);
        let paths: BTreeSet<&String> = a.keys().chain(b.keys()).chain(library.keys()).collect();
        for path in paths {
            let holds = library.get(path);
            let allowed = [a.get(path), b.get(path)];
            let pending = named.iter().any(|n| rel(path).starts_with(n));
            let cut = |new: &Vec<u8>| holds.is_some_and(|part| part.len() <= new.len());
            let moving = holds.is_none() || copying && b.get(path).is_some_and(cut);
            if !allowed.contains(&holds) && !(pending && moving) {
                let shown = holds.map(|b| String::from_utf8_lossy(b).into_owned());
                return Err(format!(
                    "{path} holds {shown:?}, neither its old nor its new bytes"
                ));
            }
        }
        round_trip(raw)
    }

    fn kept_in(&self, tree: &BTreeMap<String, Vec<u8>>, what: &str) -> Result<(), String> {
        let present: BTreeSet<&Vec<u8>> = tree.values().collect();
        let kept = kept(self.workload, &self.models, self.candidates[0]);
        match kept.iter().find(|bytes| !present.contains(bytes)) {
            Some(bytes) => Err(format!("{:?} {what}", String::from_utf8_lossy(bytes))),
            None => Ok(()),
        }
    }

    /// Opens as the same install, which writes nothing in the folder and finds
    /// nothing amiss, and finishes the orphan an interrupted step left.
    fn open(&self, raw: &BTreeMap<String, Vec<u8>>) -> Result<(Library<Native>, Opened), String> {
        make_dirs(&self.dirs.local)?;
        let clock = TestClock::at(1_800_000_000_000);
        let native = Native::new(&self.dirs.folder, &self.dirs.local);
        let (mut library, opened) = Library::open(native, layout(), &schema(), env(1000, &clock))
            .map_err(|e| format!("opening: {e}"))?;
        let opening = changed(raw, &tree(&self.dirs.folder)?);
        if !opening.is_empty() {
            return Err(format!("opening changed {opening:?} in the folder"));
        }
        let cut_short = |path: &RelPath| {
            raw.get(path.as_str())
                .is_some_and(|b| pending::cut_short(b))
        };
        let ignored = opened.ignored.iter().all(cut_short);
        let view = library.view();
        if !view.forks().is_empty() || !view.gaps().is_empty() || !ignored {
            return Err(format!("opening found {opened:?}"));
        }
        if let Start::Rekeyed {
            why: why @ (Rekey::Forked | Rekey::Restored),
            ..
        } = opened.start
        {
            return Err(format!("the writer was rekeyed: {why:?}"));
        }
        let first = self.workload.setup.len();
        let interrupted = self.models[first..self.candidates[1]]
            .iter()
            .zip(&self.workload.steps)
            .find_map(|(model, step)| match step {
                Step::Interrupt { name, .. } => Some(vec![rel(&model.file(name).0)]),
                _ => None,
            });
        for orphan in &opened.orphaned {
            if interrupted.as_ref() != Some(&orphan.paths) {
                return Err(format!("an orphan was reported: {orphan:?}"));
            }
            library
                .settle(orphan.clone(), Settlement::Finished)
                .map_err(|e| format!("settling {orphan:?}: {e}"))?;
        }
        Ok((library, opened))
    }

    /// The folder and the view hold one of the candidate models, and a scan
    /// finds the files where the facts put them.
    fn recovered(&self, library: &mut Library<Native>) -> Result<usize, String> {
        let scan = library
            .rescan()
            .map_err(|e| format!("rescanning: {e}"))?
            .scan;
        let shown = facts(&library.view())?;
        let recovered = tree(&self.dirs.folder)?;
        let files = library_files(&recovered);
        let found = |&i: &usize| self.models[i].files == files && self.models[i].entities == shown;
        let Some(model) = self.candidates.into_iter().find(found) else {
            let [a, b] = self.candidates.map(|i| &self.models[i]);
            return Err(format!(
                "recovery left files {files:?} and facts {shown:?}, neither {a:?} nor {b:?}"
            ));
        };
        let arrived: Vec<String> = scan.arrived.iter().map(|p| p.as_str().to_owned()).collect();
        let unchanged = scan.departed.is_empty()
            && scan.moved.is_empty()
            && scan.changed.is_empty()
            && scan.ambiguous.is_empty();
        if arrived != self.models[model].unbound() || !unchanged {
            return Err(format!("a scan after recovery found {scan:?}"));
        }
        self.kept_in(&recovered, "left the folder in recovery")?;
        Ok(model)
    }

    /// Closing leaves none of `writer`'s records or staging, and opening and
    /// closing again settles and changes nothing.
    fn settled_once(&self, library: Library<Native>, writer: WriterId) -> Result<(), String> {
        let shown = facts(&library.view())?;
        library.close().map_err(|e| format!("closing: {e}"))?;
        let closed = tree(&self.dirs.folder)?;
        round_trip(&closed)?;
        let own = format!("{ROOT}/writers.2/{writer}/");
        let left: Vec<&String> = closed
            .keys()
            .filter(|p| {
                p.starts_with(&format!("{own}pending/")) || p.starts_with(&format!("{own}tmp/"))
            })
            .collect();
        if !left.is_empty() {
            return Err(format!("closing left {left:?}"));
        }
        let clock = TestClock::at(1_900_000_000_000);
        let native = Native::new(&self.dirs.folder, &self.dirs.local);
        let (again, reopened) = Library::open(native, layout(), &schema(), env(1001, &clock))
            .map_err(|e| format!("reopening: {e}"))?;
        if !reopened.settled.is_empty() || !reopened.orphaned.is_empty() {
            return Err(format!("reopening settles again: {reopened:?}"));
        }
        if facts(&again.view())? != shown {
            return Err("reopening shows other facts".into());
        }
        again.close().map_err(|e| format!("closing again: {e}"))?;
        let changed = changed(&closed, &tree(&self.dirs.folder)?);
        if !changed.is_empty() {
            return Err(format!(
                "opening and closing again changed {changed:?} in the folder"
            ));
        }
        Ok(())
    }
}

/// The writer that logged `probe`, the first entry after the crash: nothing the
/// folder held is lost, nothing forks, and a resumed writer continues after the
/// last entry of its chain.
fn continued(
    before: &Derived,
    after: &Derived,
    start: &Start,
    probe: EntryHash,
) -> Result<WriterId, String> {
    if let Some((writer, at)) = held(before)
        .into_iter()
        .find(|(writer, at)| !holds(after, *writer, *at))
    {
        return Err(format!("the folder lost {at:?} of {writer}"));
    }
    if let Some((id, placed)) = after
        .writers
        .iter()
        .find(|(_, placed)| !placed.forks.is_empty())
    {
        return Err(format!("{id} forked: {:?}", placed.forks));
    }
    let (&writer, placed) = after
        .writers
        .iter()
        .find(|(_, placed)| placed.holds(probe))
        .ok_or("no writer holds the probe")?;
    if heads(placed) != [probe] {
        return Err(format!("{writer} has heads {:?}", heads(placed)));
    }
    let Start::Resumed(id) = *start else {
        return Ok(writer);
    };
    let tips = before.writers.get(&id).map(heads).unwrap_or_default();
    let [tip] = tips[..] else {
        return Err(format!("{id} resumed with heads {tips:?}"));
    };
    if writer != id || !descends(placed, probe, tip) {
        return Err(format!("{id} did not resume after its last entry {tip}"));
    }
    Ok(writer)
}

/// Checks what a crash left in `dir` once `acked` steps of `workload` returned,
/// committing `entries`.
fn check(
    workload: &Workload,
    dir: &Path,
    acked: usize,
    entries: &[Acked],
) -> Result<Checked, String> {
    let models = models(workload);
    let last = models.len() - 1;
    let reached = workload.setup.len() + acked;
    if reached > last {
        return Err(format!(
            "{} has {} steps",
            workload.name,
            workload.steps.len()
        ));
    }
    let crash = Crash {
        workload,
        dirs: Dirs::new(dir),
        models,
        candidates: [reached, (reached + 1).min(last)],
    };
    let raw = tree(&crash.dirs.folder)?;
    crash.before_recovery(&raw)?;
    let before = read_logs(&crash.dirs)?;
    if let Some(lost) = entries
        .iter()
        .find(|acked| !holds(&before, acked.writer, acked.at))
    {
        return Err(format!(
            "the folder lacks {lost}, an acknowledged step's entry"
        ));
    }
    let (mut library, opened) = crash.open(&raw)?;
    let (intent, _) = library.intent("Probe").create(|e| {
        e.set(NAME, PROBE.into());
    });
    let probe = intent
        .commit()
        .map_err(|e| format!("the first write after the crash: {e}"))?;
    let after = read_logs(&crash.dirs)?;
    let writer = continued(&before, &after, &opened.start, probe.intent)?;
    let model = crash.recovered(&mut library)?;
    crash.settled_once(library, writer)?;
    Ok(Checked {
        model: model - workload.setup.len(),
        start: opened.start,
        settled: opened.settled.len(),
        orphans: opened.orphaned.len(),
        unreadable: opened.unreadable.len(),
    })
}

fn workload(name: &str) -> Result<Workload, String> {
    workloads()
        .into_iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("no workload is named {name}"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let done = match args[..] {
        ["list"] => {
            for workload in workloads() {
                println!("{} {}", workload.name, workload.steps.len());
            }
            Ok(())
        }
        ["run", name, dir, marks] => workload(name).and_then(|workload| {
            let mut marks = Marks::parse(marks)?;
            run(&workload, Path::new(dir), &mut marks)
        }),
        ["check", name, dir, acked, ref entries @ ..] => workload(name).and_then(|workload| {
            let acked: usize = acked.parse().map_err(|e| format!("{acked}: {e}"))?;
            let entries = entries
                .iter()
                .map(|entry| entry.parse())
                .collect::<Result<Vec<Acked>, String>>()?;
            let checked = check(&workload, Path::new(dir), acked, &entries)?;
            let start = match checked.start {
                Start::Resumed(_) => "resumed",
                Start::New => "new",
                Start::Rekeyed { .. } => "rekeyed",
            };
            println!(
                "ok {name} acked={acked} model={} start={start} settled={} orphans={} unreadable={}",
                checked.model, checked.settled, checked.orphans, checked.unreadable
            );
            Ok(())
        }),
        _ => Err(
            "usage: power_loss list | run WORKLOAD DIR MARK | check WORKLOAD DIR N [ENTRY…]".into(),
        ),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("power_loss: {error}");
            ExitCode::FAILURE
        }
    }
}
