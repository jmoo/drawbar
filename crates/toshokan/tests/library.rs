//! The library end to end, through the blocking and the async driver.
//!
//! Every instance runs on a probe that panics on a write in another writer's
//! directory (I3) and counts the writes it passes to the folder (I4).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use toshokan::asynch;
use toshokan::blocking::{self, Backend};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::intent::Intent;
use toshokan::io::{Capabilities, Range};
use toshokan::log::Settlement;
use toshokan::report::{
    By, Change, Committed, Compacted, DraftState, Emptied, HistoryItem, Opened, Presence, Rekey,
    Start, TrashItem, What, WriterInfo,
};
use toshokan::simulator::Machine;
use toshokan::view::Conflicted;
use toshokan::{
    EntityId, Env, Error, Expect, Field, FileState, Identify, Identity, Io, IoResult, Layout,
    MemDisk, Policy, Refusal, RelPath, Register, Reply, Root, Schema, Set, View, WriterId,
};

const ORIGIN: Register<String> = Register::new("origin");
const TAGS: Set<String> = Set::new("tags");

fn schema() -> Schema {
    Schema::of(&[ORIGIN.key(), TAGS.key()]).unwrap()
}

fn layout() -> Layout {
    Layout::new(".t").unwrap()
}

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

const IDENTIFY: PrefixIdentity = PrefixIdentity { prefix: 1 << 16 };

fn identity(bytes: &[u8]) -> Identity {
    IDENTIFY.identify(bytes.len() as u64, &[bytes.to_vec()])
}

fn env(label: &str, seed: u64, clock: &TestClock) -> Env {
    Env {
        clock: Box::new(clock.clone()),
        random: Box::new(SeededRandom::new(seed)),
        identify: Rc::new(IDENTIFY),
        names: Box::new(ExactNames),
        label: label.into(),
    }
}

/// What the probe saw pass to the folder.
#[derive(Default)]
struct Seen {
    /// The writer whose directory this instance writes; a new one only once its
    /// directory is created.
    own: Option<String>,
    folder_writes: u64,
    /// The longest read of a pending record.
    longest_pending_read: u64,
}

/// One instance's storage: a machine, checked and counted.
#[derive(Clone)]
struct Probe {
    machine: Machine,
    seen: Rc<RefCell<Seen>>,
}

impl Probe {
    fn new(machine: &Machine) -> Self {
        Self {
            machine: Machine {
                folder: machine.folder.process(),
                local: machine.local.process(),
            },
            seen: Rc::default(),
        }
    }

    fn folder_writes(&self) -> u64 {
        self.seen.borrow().folder_writes
    }

    /// Panics on a write under toshokan's root outside the directory of the writer
    /// this instance writes as. The directories holding every writer's may be
    /// made and synced.
    fn check(&self, io: &Io) {
        if let Io::Read { path, range, .. } = io {
            if path.components().any(|name| name == "pending") {
                let mut seen = self.seen.borrow_mut();
                let len = range.len.min(self.len(path));
                seen.longest_pending_read = seen.longest_pending_read.max(len);
            }
        }
        if !io.mutates() || io.root() != Root::Folder {
            return;
        }
        self.seen.borrow_mut().folder_writes += 1;
        let writers = layout().writers();
        let mut paths = vec![io.path()];
        if let Io::Rename { from, .. } = io {
            paths.push(from);
        }
        for path in paths {
            let shared = writers.starts_with(path);
            if !layout().owns(path) || shared && matches!(io, Io::MakeDir { .. } | Io::Sync { .. }) {
                continue;
            }
            let Some(writer) = path
                .strip_prefix(&writers)
                .and_then(|rest| rest.split('/').next().map(str::to_owned))
            else {
                panic!("{io:?} writes outside every writer's directory");
            };
            let dir = writers.join(&writer).unwrap();
            let new = self.machine.folder.files(Root::Folder).keys().all(|p| !p.starts_with(&dir))
                && !self.machine.folder.directories(Root::Folder).contains(&dir);
            let mut seen = self.seen.borrow_mut();
            match &seen.own {
                Some(own) if *own == writer => {}
                _ if new => seen.own = Some(writer),
                own => panic!("{io:?} writes in {writer}'s directory as {own:?}"),
            }
        }
    }

    fn len(&self, path: &RelPath) -> u64 {
        self.machine
            .folder
            .files(Root::Folder)
            .get(path)
            .map_or(0, |bytes| bytes.len() as u64)
    }
}

trait StripPrefix {
    fn strip_prefix(&self, prefix: &RelPath) -> Option<&str>;
}

impl StripPrefix for RelPath {
    fn strip_prefix(&self, prefix: &RelPath) -> Option<&str> {
        self.as_str()
            .strip_prefix(prefix.as_str())?
            .strip_prefix('/')
    }
}

impl Backend for Probe {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.machine, root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        self.check(&io);
        self.machine.perform(io)
    }
}

impl asynch::Fs for Probe {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.machine, root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        self.check(&io);
        asynch::Fs::perform(&self.machine, io).await
    }
}

/// Tells the probe which writer an instance resumed.
fn resumed(seen: &RefCell<Seen>, opened: &Opened) {
    if let Start::Resumed(writer) = opened.start {
        seen.borrow_mut().own = Some(writer.to_string());
    }
}

/// Both drivers' libraries behind one interface.
trait Facade: Sized {
    type Inner;

    fn open(probe: Probe, env: Env) -> Result<(Self, Opened), Error>;
    fn view(&self) -> View;
    fn history(&self) -> Vec<HistoryItem>;
    fn commit(
        &mut self,
        label: &str,
        build: impl FnOnce(Intent<&mut Self::Inner>) -> Intent<&mut Self::Inner>,
    ) -> Result<Committed, Error>;
    fn undo(&mut self) -> Result<Committed, Error>;
    fn redo(&mut self) -> Result<Committed, Error>;
    fn refresh(&mut self) -> Result<Vec<Change>, Error>;
    fn others(&mut self) -> Result<Vec<WriterInfo>, Error>;
    fn trash(&mut self) -> Result<Vec<TrashItem>, Error>;
    fn empty_trash(&mut self, policy: Policy) -> Result<Emptied, Error>;
    fn compact(&mut self) -> Result<Compacted, Error>;
    fn put_draft(&mut self, entity: EntityId, base: Identity, bytes: &[u8]) -> Result<(), Error>;
    fn discard_draft(&mut self, entity: EntityId) -> Result<(), Error>;
    fn settle(&mut self, orphan: toshokan::report::Orphan, how: Settlement)
        -> Result<(), Error>;
    fn close(self) -> Result<(), Error>;
}

struct Blocking(blocking::Library<Probe>);
struct Async(asynch::Library<Probe>);

impl Facade for Blocking {
    type Inner = blocking::Library<Probe>;

    fn open(probe: Probe, env: Env) -> Result<(Self, Opened), Error> {
        let seen = Rc::clone(&probe.seen);
        let (library, opened) = blocking::Library::open(probe, layout(), &schema(), env)?;
        resumed(&seen, &opened);
        Ok((Self(library), opened))
    }
    fn view(&self) -> View {
        self.0.view()
    }
    fn history(&self) -> Vec<HistoryItem> {
        self.0.history().to_vec()
    }
    fn commit(
        &mut self,
        label: &str,
        build: impl FnOnce(Intent<&mut Self::Inner>) -> Intent<&mut Self::Inner>,
    ) -> Result<Committed, Error> {
        build(self.0.intent(label)).commit()
    }
    fn undo(&mut self) -> Result<Committed, Error> {
        self.0.undo()
    }
    fn redo(&mut self) -> Result<Committed, Error> {
        self.0.redo()
    }
    fn refresh(&mut self) -> Result<Vec<Change>, Error> {
        self.0.refresh()
    }
    fn others(&mut self) -> Result<Vec<WriterInfo>, Error> {
        self.0.others()
    }
    fn trash(&mut self) -> Result<Vec<TrashItem>, Error> {
        self.0.trash()
    }
    fn empty_trash(&mut self, policy: Policy) -> Result<Emptied, Error> {
        self.0.empty_trash(policy)
    }
    fn compact(&mut self) -> Result<Compacted, Error> {
        self.0.compact()
    }
    fn put_draft(&mut self, entity: EntityId, base: Identity, bytes: &[u8]) -> Result<(), Error> {
        self.0.draft(entity).put(base, bytes.to_vec())
    }
    fn discard_draft(&mut self, entity: EntityId) -> Result<(), Error> {
        self.0.draft(entity).discard()
    }
    fn settle(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<(), Error> {
        self.0.settle(orphan, how)
    }
    fn close(self) -> Result<(), Error> {
        self.0.close().map(drop)
    }
}

impl Facade for Async {
    type Inner = asynch::Library<Probe>;

    fn open(probe: Probe, env: Env) -> Result<(Self, Opened), Error> {
        let seen = Rc::clone(&probe.seen);
        let schema = schema();
        let opened = asynch::Library::open(probe, layout(), &schema, env);
        let (library, opened) = pollster::block_on(opened)?;
        resumed(&seen, &opened);
        Ok((Self(library), opened))
    }
    fn view(&self) -> View {
        self.0.view()
    }
    fn history(&self) -> Vec<HistoryItem> {
        self.0.history().to_vec()
    }
    fn commit(
        &mut self,
        label: &str,
        build: impl FnOnce(Intent<&mut Self::Inner>) -> Intent<&mut Self::Inner>,
    ) -> Result<Committed, Error> {
        pollster::block_on(build(self.0.intent(label)).commit())
    }
    fn undo(&mut self) -> Result<Committed, Error> {
        pollster::block_on(self.0.undo())
    }
    fn redo(&mut self) -> Result<Committed, Error> {
        pollster::block_on(self.0.redo())
    }
    fn refresh(&mut self) -> Result<Vec<Change>, Error> {
        pollster::block_on(self.0.refresh())
    }
    fn others(&mut self) -> Result<Vec<WriterInfo>, Error> {
        pollster::block_on(self.0.others())
    }
    fn trash(&mut self) -> Result<Vec<TrashItem>, Error> {
        pollster::block_on(self.0.trash())
    }
    fn empty_trash(&mut self, policy: Policy) -> Result<Emptied, Error> {
        pollster::block_on(self.0.empty_trash(policy))
    }
    fn compact(&mut self) -> Result<Compacted, Error> {
        pollster::block_on(self.0.compact())
    }
    fn put_draft(&mut self, entity: EntityId, base: Identity, bytes: &[u8]) -> Result<(), Error> {
        pollster::block_on(self.0.draft(entity).put(base, bytes.to_vec()))
    }
    fn discard_draft(&mut self, entity: EntityId) -> Result<(), Error> {
        pollster::block_on(self.0.draft(entity).discard())
    }
    fn settle(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<(), Error> {
        pollster::block_on(self.0.settle(orphan, how))
    }
    fn close(self) -> Result<(), Error> {
        pollster::block_on(self.0.close()).map(drop)
    }
}

/// Runs each scenario through both drivers.
macro_rules! through_both {
    ($($scenario:ident),* $(,)?) => {
        mod blocking_driver {
            $(#[test] fn $scenario() { super::$scenario::<super::Blocking>(); })*
        }
        mod async_driver {
            $(#[test] fn $scenario() { super::$scenario::<super::Async>(); })*
        }
    };
}

through_both!(
    two_writers_tag_one_library_and_converge,
    a_conflict_is_shown_and_resolved,
    a_clone_forks_and_both_branches_survive,
    a_restored_folder_makes_the_writer_rekey,
    undoing_a_save_restores_the_displaced_bytes,
    a_copy_has_no_entity_until_one_is_said,
    a_move_keeps_its_tags_and_is_pinned_by_the_next_commit,
    opening_viewing_and_refreshing_write_nothing_in_the_folder,
    losing_the_local_root_at_any_step_loses_only_drafts,
    a_crash_at_any_step_is_settled_before_the_next_write,
    an_untrusted_folder_is_read_within_bounds_and_never_acted_on,
    drafts_come_back_only_while_the_file_holds_their_base,
    the_trash_keeps_displaced_bytes_until_emptied,
    compaction_keeps_the_view_and_ends_undo_at_the_snapshot,
    others_on_this_machine_are_told_apart_from_others_elsewhere,
    a_refused_intent_changes_nothing,
);

/// One machine of a shared folder.
fn machine(folder: &MemDisk) -> Machine {
    Machine {
        folder: folder.clone(),
        local: MemDisk::new(),
    }
}

/// Puts `bytes` at `path` in the folder, durably, as another program would.
fn put(folder: &MemDisk, text: &str, bytes: &[u8]) {
    let file = path(text);
    let parent = file.parent().unwrap();
    let ok = |io| {
        folder.perform(io).unwrap();
    };
    ok(Io::MakeDir {
        root: Root::Folder,
        path: parent.clone(),
    });
    let _ = folder.perform(Io::Remove {
        root: Root::Folder,
        path: file.clone(),
    });
    ok(Io::Create {
        root: Root::Folder,
        path: file.clone(),
        bytes: bytes.to_vec(),
    });
    ok(Io::Sync {
        root: Root::Folder,
        path: file,
    });
    let mut dir = Some(parent);
    while let Some(synced) = dir {
        ok(Io::Sync {
            root: Root::Folder,
            path: synced.clone(),
        });
        dir = synced.parent();
    }
}

fn read(folder: &MemDisk, text: &str) -> Option<Vec<u8>> {
    match folder.perform(Io::Read {
        root: Root::Folder,
        path: path(text),
        range: Range {
            offset: 0,
            len: u64::MAX,
        },
    }) {
        Ok(Reply::Bytes(bytes)) => Some(bytes),
        _ => None,
    }
}

/// Every library file outside toshokan's root.
fn library_files(folder: &MemDisk) -> BTreeMap<RelPath, Vec<u8>> {
    folder
        .files(Root::Folder)
        .into_iter()
        .filter(|(path, _)| !layout().owns(path))
        .collect()
}

/// What a view says of each entity: its origin, tags and file.
type Facts = BTreeMap<EntityId, (Option<String>, Vec<String>, Option<(RelPath, FileState)>)>;

fn facts(view: &View) -> Facts {
    view.entities()
        .into_iter()
        .map(|entity| {
            let origin = entity.get(ORIGIN).shown().cloned();
            let tags = entity.members(TAGS).values;
            let file = entity.file().map(|file| (file.path, file.state));
            (entity.id(), (origin, tags, file))
        })
        .collect()
}

fn tags(view: &View, entity: EntityId) -> Vec<String> {
    view.entity(entity).unwrap().members(TAGS).values
}

fn tag(text: &str) -> String {
    text.to_owned()
}

/// Creates one entity with a file at `at`.
fn create<F: Facade>(library: &mut F, at: &str, bytes: &[u8]) -> EntityId {
    let committed = library
        .commit("Import", |intent| {
            intent.create(|e| {
                e.save(&path(at), bytes.to_vec(), Expect::Absent)
                    .add(TAGS, tag("new"));
            })
        })
        .unwrap();
    assert_eq!(committed.outcome, toshokan::Outcome::Complete);
    committed.created[0]
}

fn two_writers_tag_one_library_and_converge<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, opened) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    assert_eq!(opened.start, Start::New);
    assert_eq!(tags(&b.view(), song), ["new"]);

    clock.advance(10);
    b.commit("Tag", |i| i.add(song, TAGS, tag("live"))).unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("piano"))).unwrap();
    let from_b = a.refresh().unwrap();
    b.refresh().unwrap();

    assert_eq!(tags(&a.view(), song), ["live", "new", "piano"]);
    assert_eq!(facts(&a.view()), facts(&b.view()));
    let b_id = b.view().writers().iter().find(|w| w.label == "b").unwrap().writer;
    assert!(
        from_b.contains(&Change {
            entity: song,
            what: What::Field("tags".into()),
            by: By::Writer {
                writer: b_id,
                label: "b".into(),
            },
        }),
        "{from_b:?}"
    );
    let fresh = F::open(Probe::new(&machine(&folder)), env("c", 3, &clock))
        .unwrap()
        .0;
    assert_eq!(facts(&fresh.view()), facts(&a.view()), "a new reader agrees");
}

fn a_conflict_is_shown_and_resolved<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    a.commit("Origin", |i| i.set(song, ORIGIN, "from a".into()))
        .unwrap();
    clock.advance(5);
    b.commit("Origin", |i| i.set(song, ORIGIN, "from b".into()))
        .unwrap();
    a.refresh().unwrap();
    b.refresh().unwrap();
    for view in [a.view(), b.view()] {
        let Field::Conflict { shown, all } = view.entity(song).unwrap().get(ORIGIN) else {
            panic!("a conflict");
        };
        assert_eq!(shown, "from b", "the later write is shown");
        assert_eq!(all.len(), 2);
        assert_eq!(
            view.conflicts(),
            [Conflicted::Field {
                entity: song,
                key: "origin".into()
            }]
        );
    }
    a.commit("Origin", |i| i.set(song, ORIGIN, "agreed".into()))
        .unwrap();
    b.refresh().unwrap();
    for view in [a.view(), b.view()] {
        assert_eq!(
            view.entity(song).unwrap().get(ORIGIN),
            Field::Value("agreed".into())
        );
        assert_eq!(view.conflicts(), []);
    }
}

fn a_clone_forks_and_both_branches_survive<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let there = here.cloned();

    let (mut original, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let Start::Resumed(writer) = opened.start else {
        panic!("{:?}", opened.start);
    };
    let (mut clone, opened) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    assert_eq!(opened.start, Start::Resumed(writer), "a copied local root");
    original
        .commit("Tag", |i| i.add(song, TAGS, tag("here")))
        .unwrap();
    clone
        .commit("Tag", |i| i.add(song, TAGS, tag("there")))
        .unwrap();
    original.refresh().unwrap();
    clone.refresh().unwrap();
    for view in [original.view(), clone.view()] {
        assert_eq!(tags(&view, song), ["here", "new", "there"]);
        assert_eq!(view.forks().len(), 1);
        assert_eq!(view.forks()[0].writer, writer);
    }
    clone
        .commit("Tag", |i| i.add(song, TAGS, tag("after")))
        .unwrap();
    original.refresh().unwrap();
    assert_eq!(original.view().forks().len(), 1, "one fork, reported once");
    let writers: BTreeSet<WriterId> = original.view().writers().iter().map(|w| w.writer).collect();
    assert_eq!(writers.len(), 2, "the clone continued as a new writer");
    original.close().unwrap();
    let (_, opened) = F::open(Probe::new(&here), env("a", 4, &clock)).unwrap();
    assert!(
        matches!(opened.start, Start::Rekeyed { old, why: Rekey::Forked } if old == writer)
            || opened.start == Start::New,
        "{:?}",
        opened.start
    );
}

fn copy_folder(folder: &MemDisk) -> MemDisk {
    let copy = MemDisk::new();
    for dir in folder.directories(Root::Folder) {
        copy.perform(Io::MakeDir {
            root: Root::Folder,
            path: dir,
        })
        .unwrap();
    }
    for (path, bytes) in folder.files(Root::Folder) {
        copy.perform(Io::Create {
            root: Root::Folder,
            path,
            bytes,
        })
        .unwrap();
    }
    copy
}

fn a_restored_folder_makes_the_writer_rekey<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let backup = copy_folder(&folder);
    let (mut a, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let Start::Resumed(writer) = opened.start else {
        panic!("{:?}", opened.start);
    };
    a.commit("Tag", |i| i.add(song, TAGS, tag("lost")))
        .unwrap();
    a.close().unwrap();

    let restored = Machine {
        folder: backup,
        local: here.local.clone(),
    };
    let (mut a, opened) = F::open(Probe::new(&restored), env("a", 3, &clock)).unwrap();
    assert_eq!(
        opened.start,
        Start::Rekeyed {
            old: writer,
            why: Rekey::Restored
        }
    );
    assert_eq!(
        tags(&a.view(), song),
        ["lost", "new"],
        "this install's cached view keeps what it saw"
    );
    a.commit("Tag", |i| i.add(song, TAGS, tag("kept")))
        .unwrap();
    let writers: Vec<WriterInfo> = a.others().unwrap();
    let this: Vec<_> = writers.iter().filter(|w| w.here == Presence::This).collect();
    assert_eq!(this.len(), 1);
    assert_ne!(this[0].writer, writer, "a fresh writer");
    let elsewhere = machine(&restored.folder);
    let (fresh, _) = F::open(Probe::new(&elsewhere), env("b", 4, &clock)).unwrap();
    assert_eq!(tags(&fresh.view(), song), ["kept", "new"]);
}

fn undoing_a_save_restores_the_displaced_bytes<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    a.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"two".to_vec(), Expect::Holds(identity(b"one")))
    })
    .unwrap();
    assert_eq!(read(&folder, "song.npno").unwrap(), b"two");
    assert_eq!(
        a.history().iter().map(|h| h.label.as_str()).collect::<Vec<_>>(),
        ["Import", "Save"]
    );

    let undone = a.undo().unwrap();
    assert_eq!(undone.outcome, toshokan::Outcome::Complete);
    assert_eq!(read(&folder, "song.npno").unwrap(), b"one");
    assert!(a.history()[1].undone);
    a.redo().unwrap();
    assert_eq!(read(&folder, "song.npno").unwrap(), b"two");
    assert!(!a.history()[1].undone);

    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"three".to_vec(), Expect::Holds(identity(b"two")))
    })
    .unwrap();
    a.refresh().unwrap();
    assert!(
        matches!(a.undo(), Err(Error::Refused(Refusal::ChangedSince { .. }))),
        "another writer saved since"
    );
    assert_eq!(read(&folder, "song.npno").unwrap(), b"three");
}

fn a_copy_has_no_entity_until_one_is_said<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    put(&folder, "copy.npno", b"song");
    a.refresh().unwrap();
    let view = a.view();
    assert_eq!(view.unbound(), [path("copy.npno")]);
    assert_eq!(view.entities().len(), 1);
    assert_eq!(
        view.entity(song).unwrap().file().unwrap().path,
        path("song.npno")
    );
    a.close().unwrap();

    let (mut a, opened) = F::open(Probe::new(&machine(&folder)), env("a", 2, &clock)).unwrap();
    assert_eq!(
        opened.scan.copied,
        [toshokan::report::Copied {
            entity: song,
            copy: path("copy.npno")
        }]
    );
    let committed = a
        .commit("Tag", |i| {
            i.create(|e| {
                e.adopt(&path("copy.npno"), Expect::Holds(identity(b"song")))
                    .add(TAGS, tag("copy"));
            })
        })
        .unwrap();
    let copy = committed.created[0];
    let view = a.view();
    assert_eq!(view.unbound(), [] as [RelPath; 0]);
    assert_eq!(tags(&view, copy), ["copy"]);
    assert_eq!(
        view.entity(copy).unwrap().file().unwrap().path,
        path("copy.npno")
    );
    a.undo().unwrap();
    assert_eq!(
        read(&folder, "copy.npno").unwrap(),
        b"song",
        "undoing an adoption leaves the file"
    );
    assert_eq!(a.view().unbound(), [path("copy.npno")]);
}

fn a_move_keeps_its_tags_and_is_pinned_by_the_next_commit<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("moved"),
        })
        .unwrap();
    folder
        .perform(Io::Rename {
            root: Root::Folder,
            from: path("song.npno"),
            to: path("moved/song.npno"),
        })
        .unwrap();
    let changes = a.refresh().unwrap();
    assert_eq!(
        changes,
        [Change {
            entity: song,
            what: What::File,
            by: By::Outside
        }]
    );
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("moved/song.npno"), FileState::InSync)
    );
    assert_eq!(tags(&a.view(), song), ["new"]);
    let committed = a
        .commit("Tag", |i| i.add(song, TAGS, tag("moved")))
        .unwrap();
    assert!(committed.changes.contains(&Change {
        entity: song,
        what: What::File,
        by: By::This
    }));
    put(&folder, "song.npno", b"song");
    let (b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let file = b.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        file.path,
        path("moved/song.npno"),
        "the pinned path wins over a copy at the old one"
    );
    assert_eq!(b.view().unbound(), [path("song.npno")]);
}

fn opening_viewing_and_refreshing_write_nothing_in_the_folder<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"two".to_vec(), Expect::Holds(identity(b"song")))
    })
    .unwrap();
    put(&folder, "outside.npno", b"outside");
    let probe = Probe::new(&machine(&folder));
    let (mut b, opened) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    assert_eq!(opened.scan.arrived, [path("outside.npno")]);
    let _ = b.view();
    a.commit("Tag", |i| i.add(song, TAGS, tag("more"))).unwrap();
    b.refresh().unwrap();
    b.others().unwrap();
    b.trash().unwrap();
    assert_eq!(probe.folder_writes(), 0);
    b.commit("Tag", |i| i.add(song, TAGS, tag("b"))).unwrap();
    assert!(probe.folder_writes() > 0);
}

/// The scenario losing the local root interrupts: every step a user might take.
fn steps<F: Facade>(library: &mut F, clock: &TestClock, done: &mut usize) -> Result<(), Error> {
    let song = library
        .commit("Import", |i| {
            i.create(|e| {
                e.save(&path("a/song.npno"), b"one".to_vec(), Expect::Absent)
                    .add(TAGS, tag("new"));
            })
        })?
        .created[0];
    *done += 1;
    library.put_draft(song, identity(b"one"), b"draft")?;
    library.commit("Save", |i| {
        i.save(song, &path("a/song.npno"), b"two".to_vec(), Expect::Holds(identity(b"one")))
    })?;
    library.commit("Rename", |i| {
        i.rename(song, &path("b/song.npno"), Expect::Holds(identity(b"two")))
    })?;
    library.undo()?;
    library.compact()?;
    library.commit("Move", |i| i.move_tree(&path("a"), &path("c")))?;
    clock.advance(1);
    library.commit("Trash", |i| i.trash(song, Expect::Holds(identity(b"two"))))?;
    Ok(())
}

fn losing_the_local_root_at_any_step_loses_only_drafts<F: Facade>() {
    let total = {
        let disk = MemDisk::new();
        let clock = TestClock::at(1_000);
        let mut library = F::open(Probe::new(&machine(&disk)), env("a", 1, &clock))
            .unwrap()
            .0;
        steps(&mut library, &clock, &mut 0).unwrap();
        disk.mutations()
    };
    let contents: [&[u8]; 2] = [b"one", b"two"];
    let mut orphaned = 0;
    for crash in 0..total {
        let folder = MemDisk::new();
        let clock = TestClock::at(1_000);
        let here = machine(&folder);
        let (mut library, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
        folder.crash_after(crash);
        let mut done = 0;
        let _ = steps(&mut library, &clock, &mut done);
        drop(library);
        let folder = folder.restart();
        let here = machine(&folder);
        let shown = format!("crash after {crash}");

        let all = folder.files(Root::Folder);
        let present: BTreeSet<&[u8]> = all.values().map(Vec::as_slice).collect();
        let written: Vec<&[u8]> = contents
            .iter()
            .copied()
            .filter(|bytes| present.contains(bytes))
            .collect();
        let (mut heir, opened) = F::open(Probe::new(&here), env("heir", 2, &clock))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert_eq!(opened.start, Start::New, "{shown}");
        assert!(opened.drafts.is_empty(), "{shown}: drafts are lost");
        assert!(opened.settled.is_empty(), "{shown}: nothing is this writer's");
        if done > 0 {
            let view = heir.view();
            let imported = view.entities().into_iter().any(|e| {
                e.members(TAGS).values.contains(&tag("new"))
            });
            assert!(imported, "{shown}: a committed intent survives");
        }
        orphaned += opened.orphaned.len();
        for orphan in opened.orphaned.clone() {
            heir.settle(orphan, Settlement::Finished)
                .unwrap_or_else(|e| panic!("{shown}: {e}"));
        }
        let (again, opened) = F::open(Probe::new(&here), env("third", 3, &clock)).unwrap();
        assert!(opened.orphaned.is_empty(), "{shown}: settled once");
        let after: BTreeSet<Vec<u8>> = folder.files(Root::Folder).into_values().collect();
        for bytes in &written {
            assert!(after.contains(*bytes), "{shown}: {bytes:?} left the folder");
        }
        assert_eq!(again.view().gaps(), [], "{shown}");
        let view = again.view();
        for entity in view.entities() {
            if let Some(file) = entity.file() {
                assert!(
                    file.state != FileState::Missing || entity.members(TAGS).values.is_empty(),
                    "{shown}: {:?} lost its file",
                    entity.id()
                );
            }
        }
    }
    assert!(orphaned > 0, "some crash interrupted an effect");
}

fn a_crash_at_any_step_is_settled_before_the_next_write<F: Facade>() {
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let total = {
        let disk = MemDisk::new();
        let clock = TestClock::at(1_000);
        let mut library = F::open(Probe::new(&one_disk(&disk)), env("a", 1, &clock))
            .unwrap()
            .0;
        steps(&mut library, &clock, &mut 0).unwrap();
        disk.mutations()
    };
    let mut settled = 0;
    for crash in 0..total {
        let disk = MemDisk::new();
        let clock = TestClock::at(1_000);
        let (mut library, _) = F::open(Probe::new(&one_disk(&disk)), env("a", 1, &clock)).unwrap();
        disk.crash_after(crash);
        let _ = steps(&mut library, &clock, &mut 0);
        drop(library);
        let disk = disk.restart();
        let shown = format!("crash after {crash}");
        let present: BTreeSet<Vec<u8>> = library_files(&disk).into_values().collect();

        let (mut again, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 2, &clock))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert!(opened.orphaned.is_empty(), "{shown}: {:?}", opened.orphaned);
        settled += opened.settled.len();
        let next = again.commit("Next", |i| {
            i.create(|e| {
                e.add(TAGS, tag("next"));
            })
        });
        next.unwrap_or_else(|e| panic!("{shown}: {e}"));
        again.close().unwrap();
        let (last, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 3, &clock)).unwrap();
        assert!(opened.settled.is_empty(), "{shown}: settled once");
        assert_eq!(last.view().gaps(), [], "{shown}");
        let everywhere: BTreeSet<Vec<u8>> = disk.files(Root::Folder).into_values().collect();
        for bytes in &present {
            assert!(everywhere.contains(bytes), "{shown}: {bytes:?} left the folder");
        }
    }
    assert!(settled > 0, "some crash interrupted an effect");
}

fn an_untrusted_folder_is_read_within_bounds_and_never_acted_on<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let writers = layout().writers();
    let real = folder
        .files(Root::Folder)
        .into_keys()
        .find(|p| p.starts_with(&writers))
        .unwrap();
    let own = real.as_str().split('/').nth(2).unwrap().to_owned();
    let stranger = WriterId::from_u128(0x5);
    let forged = |after: &str, path: &str| {
        format!(
            r#"{{"writer":"{stranger}","entry":{{"prev":"{after}","at":[1,0],"kind":"intent","label":"x","ops":[]}},"label":"forged","steps":[{{"step":"to_trash","path":"{path}","item":"00000000000000000000000000000001"}}],"files":[]}}"#
        )
    };
    let zero = "0".repeat(32);
    let mut random = SeededRandom::new(7);
    let mut noise = |len: usize| -> Vec<u8> {
        use toshokan::Random;
        (0..len).map(|_| random.next_u128() as u8).collect()
    };
    let w = |name: &str| format!(".t/writers/{name}");
    let garbage: Vec<(String, Vec<u8>)> = vec![
        (format!("{}/noise.jsonl", w(&own)), noise(4096)),
        (format!("{}/snapshot-x.json", w(&own)), br#"{"writer":1}"#.to_vec()),
        (format!("{}/pending/{}1.json", w(&own), &zero[1..]), vec![b' '; (16 << 20) + 1]),
        (format!("{}/pending/{zero}.json", w(&stranger.to_string())), forged(&zero, ".t/x").into_bytes()),
        (format!("{}/pending/{zero}.json", w(&own)), forged(&zero, "song.npno").into_bytes()),
        (format!("{}/x.jsonl", w(&stranger.to_string())), b"{\"prev\":\"zz\"}\tbad\n".to_vec()),
        (format!("{}/zeros.jsonl", w(&stranger.to_string())), vec![0; 1000]),
        (w("not-a-writer/seg.jsonl"), noise(100)),
        (".t/writers/file".into(), noise(10)),
    ];
    for (at, bytes) in &garbage {
        put(&folder, at, bytes);
    }
    let before = library_files(&folder);
    let probe = Probe::new(&here);
    let (mut a, opened) = F::open(probe.clone(), env("a", 2, &clock)).unwrap();
    assert_eq!(probe.folder_writes(), 0);
    assert!(opened.orphaned.is_empty(), "{:?}", opened.orphaned);
    assert!(opened.settled.is_empty(), "{:?}", opened.settled);
    assert!(
        probe.seen.borrow().longest_pending_read <= toshokan::pending::MAX_RECORD,
        "an oversized record is not read"
    );
    assert_eq!(tags(&a.view(), song), ["new"]);
    a.commit("Tag", |i| i.add(song, TAGS, tag("still"))).unwrap();
    assert_eq!(library_files(&folder), before, "no forged effect ran");

    for seed in 0..20 {
        let fuzzed = MemDisk::new();
        let clock = TestClock::at(1_000);
        let mut random = SeededRandom::new(seed);
        use toshokan::Random;
        for n in 0..8u128 {
            let dir = WriterId::from_u128(random.next_u128() % 3);
            let name = match n % 4 {
                0 => format!("{n}.jsonl"),
                1 => format!("snapshot-{n}.json"),
                2 => format!("pending/{:032x}.json", random.next_u128() % 2),
                _ => format!("trash/{n}"),
            };
            let len = (random.next_u128() % 300) as usize;
            let alphabet = b"{}[]\":,\t\n0123456789abcdefprevatkindopsintent";
            let bytes: Vec<u8> = (0..len)
                .map(|_| alphabet[(random.next_u128() % alphabet.len() as u128) as usize])
                .collect();
            put(&fuzzed, &format!(".t/writers/{dir}/{name}"), &bytes);
        }
        let probe = Probe::new(&machine(&fuzzed));
        let (_, opened) = F::open(probe.clone(), env("f", seed, &clock))
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert_eq!(probe.folder_writes(), 0, "seed {seed}");
        assert!(opened.orphaned.is_empty(), "seed {seed}");
    }
}

fn drafts_come_back_only_while_the_file_holds_their_base<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    assert!(matches!(
        a.put_draft(EntityId::from_u128(1), identity(b"x"), b"x"),
        Err(Error::NoWriter)
    ));
    let song = create(&mut a, "song.npno", b"song");
    let other = create(&mut a, "other.npno", b"other");
    a.put_draft(song, identity(b"song"), b"edited").unwrap();
    a.put_draft(other, identity(b"other"), b"edited too").unwrap();
    a.close().unwrap();
    put(&folder, "other.npno", b"changed outside");

    let (mut a, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let states: BTreeMap<EntityId, DraftState> = opened
        .drafts
        .into_iter()
        .map(|d| (d.entity, d.state))
        .collect();
    assert_eq!(
        states[&song],
        DraftState::Applies {
            bytes: b"edited".to_vec()
        }
    );
    assert_eq!(
        states[&other],
        DraftState::BaseChanged {
            found: Some(identity(b"changed outside"))
        }
    );
    a.discard_draft(song).unwrap();
    a.discard_draft(other).unwrap();
    a.close().unwrap();
    let (_, opened) = F::open(Probe::new(&here), env("a", 3, &clock)).unwrap();
    assert_eq!(opened.drafts, []);
}

fn the_trash_keeps_displaced_bytes_until_emptied<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    a.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"two".to_vec(), Expect::Holds(identity(b"one")))
    })
    .unwrap();
    let trash = a.trash().unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!((trash[0].from.clone(), trash[0].len), (path("song.npno"), 3));
    let kept = a.empty_trash(Policy::default()).unwrap();
    assert_eq!(kept, Emptied::default(), "nothing is old or over the cap");
    clock.advance(31 * 24 * 60 * 60 * 1000);
    let emptied = a.empty_trash(Policy::default()).unwrap();
    assert_eq!((emptied.removed.len(), emptied.bytes), (1, 3));
    assert_eq!(a.trash().unwrap(), []);
    let present: BTreeSet<Vec<u8>> = folder.files(Root::Folder).into_values().collect();
    assert!(!present.contains(b"one".as_slice()));
    assert!(
        matches!(a.undo(), Err(Error::Refused(Refusal::Emptied))),
        "undo needs what was emptied"
    );
}

fn compaction_keeps_the_view_and_ends_undo_at_the_snapshot<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    for n in 0..5 {
        a.commit("Tag", |i| i.add(song, TAGS, format!("t{n}"))).unwrap();
    }
    a.close().unwrap();
    let (mut a, _) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("last"))).unwrap();
    let before = facts(&a.view());
    let compacted = a.compact().unwrap();
    assert_eq!(compacted.folded, 8, "genesis, import, five tags and the last");
    assert_eq!(compacted.removed.len(), 1, "only the segment this process sealed");
    assert_eq!(facts(&a.view()), before);
    assert!(matches!(a.undo(), Err(Error::Refused(Refusal::Nothing))));
    let fresh = F::open(Probe::new(&machine(&folder)), env("b", 3, &clock))
        .unwrap()
        .0;
    assert_eq!(facts(&fresh.view()), before);
}

fn others_on_this_machine_are_told_apart_from_others_elsewhere<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&here), env("b", 2, &clock)).unwrap();
    b.commit("Tag", |i| i.add(song, TAGS, tag("b"))).unwrap();
    let (mut c, _) = F::open(Probe::new(&machine(&folder)), env("c", 3, &clock)).unwrap();
    c.commit("Tag", |i| i.add(song, TAGS, tag("c"))).unwrap();
    a.refresh().unwrap();
    let presence: BTreeMap<String, Presence> = a
        .others()
        .unwrap()
        .into_iter()
        .map(|w| (w.label, w.here))
        .collect();
    assert_eq!(
        presence,
        BTreeMap::from([
            ("a".into(), Presence::This),
            ("b".into(), Presence::SameMachine),
            ("c".into(), Presence::Elsewhere),
        ])
    );
    b.close().unwrap();
    let presence = a.others().unwrap();
    assert!(presence
        .iter()
        .all(|w| w.label != "b" || w.here == Presence::Elsewhere));
}

fn a_refused_intent_changes_nothing<F: Facade>() {
    let folder = MemDisk::new();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    put(&folder, "taken.npno", b"taken");
    let refused = a.commit("Import", |i| {
        i.create(|e| {
            e.save(&path("taken.npno"), b"mine".to_vec(), Expect::Absent);
        })
    });
    assert!(
        matches!(refused, Err(Error::Refused(Refusal::Changed(_)))),
        "{refused:?}"
    );
    assert!(
        !folder
            .directories(Root::Folder)
            .iter()
            .any(|d| d.starts_with(&layout().writers())),
        "a refused first intent creates no writer"
    );
    let song = create(&mut a, "song.npno", b"song");
    let written = folder.files(Root::Folder);
    let refused = a.commit("Rename", |i| {
        i.add(song, TAGS, tag("x"))
            .rename(song, &path("taken.npno"), Expect::Holds(identity(b"song")))
    });
    assert!(matches!(refused, Err(Error::Refused(Refusal::Changed(_)))));
    let after = folder.files(Root::Folder);
    let changed: Vec<&RelPath> = after
        .iter()
        .filter(|(path, bytes)| written.get(*path) != Some(*bytes))
        .map(|(path, _)| path)
        .collect();
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(tags(&a.view(), song), ["new"]);
    assert!(matches!(
        a.commit("Nothing", |i| i),
        Err(Error::Refused(Refusal::Invalid(toshokan::Invalid::Empty)))
    ));
}
