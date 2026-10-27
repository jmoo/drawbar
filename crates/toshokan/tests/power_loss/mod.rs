//! The power-loss workloads, their model and the checks a stopped run must pass,
//! on any [`Place`]: the native file system the `power_loss` example drives, or
//! the in-memory disk the fault sweep drives.

#![allow(
    dead_code,
    reason = "the power_loss example and the fault sweep each use their own part"
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use toshokan::asynch;
use toshokan::blocking::{self, Backend, Library};
use toshokan::derive::{Derived, Placed};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::intent::{Driver, Intent};
use toshokan::io::Capabilities;
use toshokan::layout::{SEGMENT_EXTENSION, WRITERS};
use toshokan::line;
use toshokan::log::{Entry, Own, Settlement};
use toshokan::pending::{self, PendingRecord};
use toshokan::reader::Reader;
use toshokan::report::{Committed, Emptied, Opened, Orphan, Rekey, Start};
use toshokan::{
    EntryHash, Env, Expect, Identify, Identity, Io, IoError, IoResult, Layout, Register, RelPath,
    Root, Schema, Set, View, WriterId,
};

pub const NAME: Register<String> = Register::new("name");
pub const TAGS: Set<String> = Set::new("tags");
/// The entity the checker commits to make the writer settle and write again.
pub const PROBE: &str = "probe";
pub const ROOT: &str = ".t";

pub type Bytes = &'static [u8];

/// What the folder holds before the library is first opened. No step touches it.
pub const BEFORE: &[(&str, Bytes)] = &[("notes/readme.txt", b"the user's own notes")];

#[derive(Clone, Copy, Debug)]
pub enum Step {
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

pub struct Workload {
    pub name: &'static str,
    /// Steps whose crashes are not checked.
    pub setup: Vec<Step>,
    pub steps: Vec<Step>,
}

pub fn workloads() -> Vec<Workload> {
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
pub struct Model {
    /// Every library file, by path.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Every entity but the probe, by name.
    pub entities: BTreeMap<String, Entity>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Entity {
    pub tags: BTreeSet<String>,
    pub file: Option<String>,
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
pub fn models(workload: &Workload) -> Vec<Model> {
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
pub fn kept(workload: &Workload, models: &[Model], at: usize) -> BTreeSet<Vec<u8>> {
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

pub fn layout() -> Layout {
    Layout::new(ROOT).unwrap()
}

pub fn schema() -> Schema {
    Schema::of(&[NAME.key(), TAGS.key()]).unwrap()
}

pub const IDENTIFY: PrefixIdentity = PrefixIdentity { prefix: 64 * 1024 };

pub fn identity(bytes: &[u8]) -> Identity {
    IDENTIFY.of_bytes(&RelPath::ROOT, bytes)
}

pub fn env(seed: u64, clock: &TestClock) -> Env {
    Env {
        clock: Box::new(clock.clone()),
        random: Box::new(SeededRandom::new(seed)),
        identify: Rc::new(IDENTIFY),
        names: Box::new(ExactNames),
        label: "power-loss".into(),
    }
}

pub fn rel(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

/// Where a workload runs: a folder, and the local root of the install that opens it.
pub trait Place {
    type Backend: Backend;

    /// A backend on the folder and the local root.
    fn backend(&self) -> Self::Backend;

    /// Every file in the folder, by path with `/` between names.
    fn tree(&self) -> Result<BTreeMap<String, Vec<u8>>, String>;

    /// Puts `bytes` at `path` in the folder, durably, before the library opens.
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String>;

    /// Loses the local root, leaving an empty one.
    fn lose_local(&self) -> Result<(), String>;

    /// Whether moves in the folder copy.
    fn copying(&self) -> bool {
        !self.backend().capabilities(Root::Folder).rename_file
    }
}

/// How one request fails.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fault {
    /// It fails before it takes effect.
    Fails,
    /// It takes effect, and its reply is lost as an error.
    Unanswered,
    /// It writes half its bytes, and fails: a short write, which only a request
    /// that carries bytes can make.
    Short,
    /// It takes effect and its reply is lost, and every request after it fails, as
    /// when a disk is unplugged, until the fault is cleared.
    Unplugged,
}

/// What a [`Faulty`] backend is told to do.
#[derive(Default)]
pub struct Arms {
    /// Once a rename or a create makes this folder path, every request fails,
    /// as in a process that died there.
    pub stop: Option<RelPath>,
    pub stopped: bool,
    /// The request, counted from `counted`, that fails, and how.
    pub fault: Option<(u64, Fault)>,
    /// Requests performed since the count was last reset, but for those made
    /// while `paused`, which are neither counted nor faulted.
    pub counted: u64,
    pub paused: bool,
    /// Each request counted, while this is kept: what it was, and which faults
    /// can befall it.
    pub seen: Option<Vec<Seen>>,
}

/// A request a [`Faulty`] backend counted.
#[derive(Clone, Debug)]
pub struct Seen {
    pub io: String,
    pub mutates: bool,
    pub shortens: bool,
}

/// A backend that stops, or fails one request, as [`Arms`] says.
pub struct Faulty<B> {
    pub inner: B,
    pub arms: Rc<RefCell<Arms>>,
}

/// What a [`Faulty`] backend does with a request.
enum Doing {
    Perform,
    Refuse,
    Fault(Fault),
}

impl<B> Faulty<B> {
    fn doing(&self, io: &Io) -> Doing {
        let mut arms = self.arms.borrow_mut();
        let made = match io {
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
        if made.is_some() && made == arms.stop.as_ref() {
            arms.stopped = true;
        }
        if arms.stopped {
            return Doing::Refuse;
        }
        if arms.paused {
            return Doing::Perform;
        }
        let at = arms.counted;
        arms.counted += 1;
        if let Some(seen) = &mut arms.seen {
            seen.push(Seen {
                io: format!("{:?} {}", kind(io), io.path()),
                mutates: io.mutates(),
                shortens: short(io).is_some(),
            });
        }
        match arms.fault {
            Some((index, fault)) if index == at => Doing::Fault(fault),
            Some((index, Fault::Unplugged)) if index < at => Doing::Fault(Fault::Fails),
            _ => Doing::Perform,
        }
    }
}

/// The name of `io`'s kind.
fn kind(io: &Io) -> &'static str {
    match io {
        Io::List { .. } => "List",
        Io::Stat { .. } => "Stat",
        Io::ListStat { .. } => "ListStat",
        Io::Read { .. } => "Read",
        Io::ReadMany { .. } => "ReadMany",
        Io::Create { .. } => "Create",
        Io::Append { .. } => "Append",
        Io::Write { .. } => "Write",
        Io::Fill { .. } => "Fill",
        Io::Rename { .. } => "Rename",
        Io::Remove { .. } => "Remove",
        Io::RemoveDir { .. } => "RemoveDir",
        Io::MakeDir { .. } => "MakeDir",
        Io::Sync { .. } => "Sync",
        Io::Lock { .. } => "Lock",
        Io::Unlock { .. } => "Unlock",
    }
}

fn faulted() -> IoError {
    IoError::Other("faulted by the sweep".into())
}

/// `io` with half its bytes, when it carries any.
fn short(io: &Io) -> Option<Io> {
    let half = |bytes: &Vec<u8>| bytes[..bytes.len() / 2].to_vec();
    match io {
        Io::Create { root, path, bytes } => Some(Io::Create {
            root: *root,
            path: path.clone(),
            bytes: half(bytes),
        }),
        Io::Append { root, path, bytes } => Some(Io::Append {
            root: *root,
            path: path.clone(),
            bytes: half(bytes),
        }),
        Io::Write {
            root,
            path,
            offset,
            bytes,
        } => Some(Io::Write {
            root: *root,
            path: path.clone(),
            offset: *offset,
            bytes: half(bytes),
        }),
        _ => None,
    }
}

impl<B: Backend> Backend for Faulty<B> {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.inner.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        match self.doing(&io) {
            Doing::Perform => self.inner.perform(io),
            Doing::Refuse => Err(IoError::Other("the process stopped".into())),
            Doing::Fault(Fault::Fails) => Err(faulted()),
            Doing::Fault(Fault::Unanswered | Fault::Unplugged) => {
                self.inner.perform(io).and(Err(faulted()))
            }
            Doing::Fault(Fault::Short) => {
                let short = short(&io).expect("a short write of a request with bytes");
                self.inner.perform(short).and(Err(faulted()))
            }
        }
    }
}

impl<B: asynch::Fs> asynch::Fs for Faulty<B> {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.inner.capabilities(root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        match self.doing(&io) {
            Doing::Perform => self.inner.perform(io).await,
            Doing::Refuse => Err(IoError::Other("the process stopped".into())),
            Doing::Fault(Fault::Fails) => Err(faulted()),
            Doing::Fault(Fault::Unanswered | Fault::Unplugged) => {
                self.inner.perform(io).await.and(Err(faulted()))
            }
            Doing::Fault(Fault::Short) => {
                let short = short(&io).expect("a short write of a request with bytes");
                self.inner.perform(short).await.and(Err(faulted()))
            }
        }
    }

    fn own(&self, root: &RelPath) {
        self.inner.own(root);
    }
}

/// A library a workload drives, through one driver or another.
pub trait Lib<B>: Sized {
    type Driver<'a>: Driver
    where
        Self: 'a;

    fn open(backend: Faulty<B>, env: Env) -> toshokan::Result<(Self, Opened)>;
    fn view(&self) -> View;
    fn commit<'a>(
        &'a mut self,
        label: &str,
        build: impl FnOnce(Intent<Self::Driver<'a>>) -> Intent<Self::Driver<'a>>,
    ) -> toshokan::Result<Committed>;
    fn undo(&mut self) -> toshokan::Result<Committed>;
    fn redo(&mut self) -> toshokan::Result<Committed>;
    fn compact(&mut self) -> toshokan::Result<()>;
    fn empty_trash(&mut self) -> toshokan::Result<Emptied>;
    fn settle(&mut self, orphan: Orphan) -> toshokan::Result<Committed>;
    fn close(self) -> toshokan::Result<()>;
}

impl<B: Backend> Lib<B> for Library<Faulty<B>> {
    type Driver<'a>
        = &'a mut Self
    where
        Self: 'a;

    fn open(backend: Faulty<B>, env: Env) -> toshokan::Result<(Self, Opened)> {
        Library::open(backend, layout(), &schema(), env)
    }
    fn view(&self) -> View {
        Library::view(self)
    }
    fn commit<'a>(
        &'a mut self,
        label: &str,
        build: impl FnOnce(Intent<Self::Driver<'a>>) -> Intent<Self::Driver<'a>>,
    ) -> toshokan::Result<Committed> {
        build(self.intent(label)).commit()
    }
    fn undo(&mut self) -> toshokan::Result<Committed> {
        Library::undo(self)
    }
    fn redo(&mut self) -> toshokan::Result<Committed> {
        Library::redo(self)
    }
    fn compact(&mut self) -> toshokan::Result<()> {
        Library::compact(self).map(|_| ())
    }
    fn empty_trash(&mut self) -> toshokan::Result<Emptied> {
        Library::empty_trash(self, Default::default())
    }
    fn settle(&mut self, orphan: Orphan) -> toshokan::Result<Committed> {
        Library::settle(self, orphan, Settlement::Finished)
    }
    fn close(self) -> toshokan::Result<()> {
        Library::close(self).map(|_| ())
    }
}

/// The async driver's library, each call run to its end.
pub struct Async<B>(pub asynch::Library<Faulty<B>>);

impl<B: asynch::Fs> Lib<B> for Async<B> {
    type Driver<'a>
        = &'a mut asynch::Library<Faulty<B>>
    where
        Self: 'a;

    fn open(backend: Faulty<B>, env: Env) -> toshokan::Result<(Self, Opened)> {
        let schema = schema();
        let opened = pollster::block_on(asynch::Library::open(backend, layout(), &schema, env));
        opened.map(|(library, opened)| (Self(library), opened))
    }
    fn view(&self) -> View {
        self.0.view()
    }
    fn commit<'a>(
        &'a mut self,
        label: &str,
        build: impl FnOnce(Intent<Self::Driver<'a>>) -> Intent<Self::Driver<'a>>,
    ) -> toshokan::Result<Committed> {
        pollster::block_on(build(self.0.intent(label)).commit())
    }
    fn undo(&mut self) -> toshokan::Result<Committed> {
        pollster::block_on(self.0.undo())
    }
    fn redo(&mut self) -> toshokan::Result<Committed> {
        pollster::block_on(self.0.redo())
    }
    fn compact(&mut self) -> toshokan::Result<()> {
        pollster::block_on(self.0.compact()).map(|_| ())
    }
    fn empty_trash(&mut self) -> toshokan::Result<Emptied> {
        pollster::block_on(self.0.empty_trash(Default::default()))
    }
    fn settle(&mut self, orphan: Orphan) -> toshokan::Result<Committed> {
        pollster::block_on(self.0.settle(orphan, Settlement::Finished))
    }
    fn close(self) -> toshokan::Result<()> {
        pollster::block_on(self.0.close()).map(|_| ())
    }
}

/// A workload in progress: its place, its install's clock, what its backend is
/// told, and the library it opened.
pub struct Runner<'p, P: Place, L> {
    pub place: &'p P,
    pub clock: TestClock,
    pub arms: Rc<RefCell<Arms>>,
    pub library: Option<L>,
    pub opened: Option<Opened>,
    pub opens: u64,
}

impl<'p, P: Place, L: Lib<P::Backend>> Runner<'p, P, L> {
    pub fn new(place: &'p P) -> Self {
        Self {
            place,
            clock: TestClock::at(1_700_000_000_000),
            arms: Rc::default(),
            library: None,
            opened: None,
            opens: 0,
        }
    }

    fn library(&mut self) -> &mut L {
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
    pub fn perform(&mut self, step: Step, before: &Model) -> Result<Option<EntryHash>, String> {
        let shown = |e: toshokan::Error| format!("{step:?}: {e}");
        let committed = |done: toshokan::Result<Committed>| {
            done.map(|committed| Some(committed.intent)).map_err(shown)
        };
        let entry = match step {
            Step::Open => {
                let backend = Faulty {
                    inner: self.place.backend(),
                    arms: Rc::clone(&self.arms),
                };
                self.opens += 1;
                let env = env(self.opens, &self.clock);
                let (library, opened) = L::open(backend, env).map_err(shown)?;
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
                let import = self.library().commit("Import", |intent| {
                    let (intent, _) = intent.create(|e| {
                        e.save(&rel(path), bytes.to_vec(), Expect::Absent)
                            .set(NAME, name.into());
                    });
                    intent
                });
                committed(import)?
            }
            Step::Tag { name, tag } => {
                let id = self.entity(name);
                let tagged = self
                    .library()
                    .commit("Tag", |intent| intent.add(id, TAGS, tag.into()));
                committed(tagged)?
            }
            Step::Save { name, bytes } => {
                let id = self.entity(name);
                let (path, old) = before.file(name);
                let expect = Expect::Holds(identity(&old));
                let saved = self.library().commit("Save", |intent| {
                    intent.save(id, &rel(&path), bytes.to_vec(), expect)
                });
                committed(saved)?
            }
            Step::Rename { name, to } => {
                let id = self.entity(name);
                let expect = Expect::Holds(identity(&before.file(name).1));
                let renamed = self
                    .library()
                    .commit("Rename", |intent| intent.rename(id, &rel(to), expect));
                committed(renamed)?
            }
            Step::MoveTree { from, to } => {
                let moved = self
                    .library()
                    .commit("Move", |intent| intent.move_tree(&rel(from), &rel(to)));
                committed(moved)?
            }
            Step::Trash { name } => {
                let id = self.entity(name);
                let expect = Expect::Holds(identity(&before.file(name).1));
                let trashed = self
                    .library()
                    .commit("Trash", |intent| intent.trash(id, expect));
                committed(trashed)?
            }
            Step::Undo => committed(self.library().undo())?,
            Step::Redo => committed(self.library().redo())?,
            Step::Compact => self.library().compact().map(|()| None).map_err(shown)?,
            Step::EmptyTrash => {
                let emptied = self.library().empty_trash().map_err(shown)?;
                if emptied.removed.is_empty() {
                    return Err(format!("{step:?} removed nothing"));
                }
                None
            }
            Step::Interrupt { name, bytes } => {
                let id = self.entity(name);
                let (path, old) = before.file(name);
                self.arms.borrow_mut().stop = Some(rel(&path));
                let expect = Expect::Holds(identity(&old));
                let saved = self.library().commit("Save", |intent| {
                    intent.save(id, &rel(&path), bytes.to_vec(), expect)
                });
                {
                    let mut arms = self.arms.borrow_mut();
                    (arms.stop, arms.stopped) = (None, false);
                }
                if saved.is_ok() {
                    return Err(format!("{step:?} was not stopped"));
                }
                self.library = None;
                self.place
                    .lose_local()
                    .map_err(|e| format!("{step:?}: {e}"))?;
                None
            }
            Step::Settle => {
                let orphans = self.opened.as_ref().expect("open").orphaned.clone();
                if orphans.is_empty() {
                    return Err(format!("{step:?}: no orphan was reported"));
                }
                let mut settled = None;
                for orphan in orphans {
                    settled = committed(self.library().settle(orphan))?;
                }
                settled
            }
        };
        self.clock.advance(1000);
        Ok(entry)
    }
}

/// Puts the user's files that are there before the library first opens.
pub fn before(place: &impl Place) -> Result<(), String> {
    for (path, bytes) in BEFORE {
        place.put(path, bytes)?;
    }
    Ok(())
}

pub fn verify<P: Place, L: Lib<P::Backend>>(
    runner: &Runner<'_, P, L>,
    model: &Model,
    settling: bool,
) -> Result<(), String> {
    let files = library_files(&runner.place.tree()?);
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

/// Runs `workload` in `place`, checking each step against the model, and calls
/// `mark` once setup is done with `setup` and after each step with `step-<i>`,
/// and with the entry it committed, as `step-<i>:<writer>/<n>/<hash>`.
pub fn run<P: Place, L: Lib<P::Backend>>(
    workload: &Workload,
    place: &P,
    mut mark: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    before(place)?;
    let models = models(workload);
    let mut runner = Runner::<P, L>::new(place);
    let mut settling = false;
    let all = workload.setup.iter().chain(&workload.steps);
    for (i, &step) in all.enumerate() {
        if i == workload.setup.len() {
            mark("setup")?;
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
                Some(entry) => format!("step-{}:{}", n + 1, Acked::find(place, entry)?),
                None => format!("step-{}", n + 1),
            };
            mark(&label)?;
        }
    }
    Ok(())
}

pub fn read_logs(place: &impl Place) -> Result<Derived, String> {
    let mut backend = place.backend();
    let mut reader = Reader::new(layout());
    blocking::run(&mut backend, reader.read()).map_err(|e| format!("reading the logs: {e}"))?;
    Ok(reader.derived().clone())
}

/// What a stopped run left of a workload once `acked` of its steps returned.
pub struct Crash<'w, 'p, P> {
    pub workload: &'w Workload,
    pub place: &'p P,
    pub models: Vec<Model>,
    /// The models recovery may reach: after the last step that returned, and
    /// after the one in flight.
    pub candidates: [usize; 2],
}

impl<P: Place> Crash<'_, '_, P> {
    /// Each library path holds its old bytes, its new bytes, or, while a pending
    /// record names it, nothing or where moves copy a copy cut short; no bytes
    /// left the folder; logs re-encode.
    pub fn before_recovery(&self, raw: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
        self.kept_in(raw, "left the folder")?;
        let named = records(raw)?;
        let library = library_files(raw);
        let copying = self.place.copying();
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
    pub fn open(
        &self,
        raw: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(Library<P::Backend>, Opened), String> {
        let clock = TestClock::at(1_800_000_000_000);
        let backend = self.place.backend();
        let (mut library, opened) = Library::open(backend, layout(), &schema(), env(1000, &clock))
            .map_err(|e| format!("opening: {e}"))?;
        let opening = changed(raw, &self.place.tree()?);
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
    pub fn recovered(&self, library: &mut Library<P::Backend>) -> Result<usize, String> {
        let scan = library
            .rescan()
            .map_err(|e| format!("rescanning: {e}"))?
            .scan;
        let shown = facts(&library.view())?;
        let recovered = self.place.tree()?;
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
    pub fn settled_once(
        &self,
        library: Library<P::Backend>,
        writer: WriterId,
    ) -> Result<(), String> {
        let shown = facts(&library.view())?;
        library.close().map_err(|e| format!("closing: {e}"))?;
        let closed = self.place.tree()?;
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
        let backend = self.place.backend();
        let (again, reopened) = Library::open(backend, layout(), &schema(), env(1001, &clock))
            .map_err(|e| format!("reopening: {e}"))?;
        if !reopened.settled.is_empty() || !reopened.orphaned.is_empty() {
            return Err(format!("reopening settles again: {reopened:?}"));
        }
        if facts(&again.view())? != shown {
            return Err("reopening shows other facts".into());
        }
        again.close().map_err(|e| format!("closing again: {e}"))?;
        let changed = changed(&closed, &self.place.tree()?);
        if !changed.is_empty() {
            return Err(format!(
                "opening and closing again changed {changed:?} in the folder"
            ));
        }
        Ok(())
    }
}

/// Checks what a stopped run left in `place` once `acked` steps of `workload`
/// returned, committing `entries`.
pub fn check<P: Place>(
    workload: &Workload,
    place: &P,
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
        place,
        models,
        candidates: [reached, (reached + 1).min(last)],
    };
    let raw = place.tree()?;
    crash.before_recovery(&raw)?;
    let before = read_logs(place)?;
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
    let after = read_logs(place)?;
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

/// The paths whose bytes differ between `before` and `after`, or that only one
/// holds.
pub fn changed(
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

pub fn library_files(tree: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    let owned = format!("{ROOT}/");
    tree.iter()
        .filter(|(p, _)| !p.starts_with(&owned))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

/// What `view` says of each entity but the probe.
pub fn facts(view: &View) -> Result<BTreeMap<String, Entity>, String> {
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

/// What a check found beyond pass or fail, for the tally.
pub struct Checked {
    pub model: usize,
    pub start: Start,
    pub settled: usize,
    pub orphans: usize,
    pub unreadable: usize,
}

/// An entry an acknowledged step committed, by its writer, number and hash:
/// written `<writer>/<n>/<hash>`.
#[derive(Clone, Copy, Debug)]
pub struct Acked {
    pub writer: WriterId,
    pub at: Own,
}

impl Acked {
    /// The entry `hash` as a reader of the folder finds it.
    pub fn find(place: &impl Place, hash: EntryHash) -> Result<Self, String> {
        let derived = read_logs(place)?;
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
pub fn holds(derived: &Derived, writer: WriterId, at: Own) -> bool {
    let Some(placed) = derived.writers.get(&writer) else {
        return false;
    };
    let passed = placed.tips.iter().any(|tip| tip.head.n >= at.n);
    placed.holds(at.hash) || passed
}

/// Every entry a reader holds as a line or by a tip's name, by writer.
pub fn held(derived: &Derived) -> Vec<(WriterId, Own)> {
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
pub fn heads(placed: &Placed) -> Vec<EntryHash> {
    let lines: Vec<&Rc<Entry>> = placed.merged.iter().chain(&placed.runs).collect();
    let followed: BTreeSet<EntryHash> = lines.iter().map(|entry| entry.prev()).collect();
    let tips = placed.tips.iter().map(|tip| tip.head.hash);
    let all = lines.iter().map(|entry| entry.hash()).chain(tips);
    let heads: BTreeSet<EntryHash> = all.filter(|hash| !followed.contains(hash)).collect();
    heads.into_iter().collect()
}

/// Whether the line `from` descends from `to`, through lines a reader holds.
pub fn descends(placed: &Placed, from: EntryHash, to: EntryHash) -> bool {
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

/// The paths each pending record in the folder names. A record cut short before
/// its bytes landed names none.
pub fn records(tree: &BTreeMap<String, Vec<u8>>) -> Result<Vec<RelPath>, String> {
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
pub fn round_trip(tree: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
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

/// The writer that logged `probe`, the first entry after the crash: nothing the
/// folder held is lost, nothing forks, and a resumed writer continues after the
/// last entry of its chain.
pub fn continued(
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

pub fn workload(name: &str) -> Result<Workload, String> {
    workloads()
        .into_iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("no workload is named {name}"))
}
