//! The library as one instance holds it: the pure core both drivers run.
//!
//! Only [`Library::commit`] (and undo, redo and settling, which commit),
//! [`Library::empty_trash`], [`Library::compact`] and
//! [`Library::close`] write the folder. Opening, viewing, refreshing and scanning
//! never do; they write only in the local root.
//!
//! Every operation is a chain of requests. Each step that needs I/O runs as a task
//! that owns what it reads, and its result is folded into the library afterwards,
//! so no task borrows the library while it runs.

use std::borrow::BorrowMut;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use crate::binding::{self, Bindings, Fact, Facts, Scan, Unscanned, Walk};
use crate::contribution;
use crate::derive::{self, Placed};
use crate::effects::{self, Applied, EffectPlan, EffectStep, FileEnd};
use crate::env::Env;
use crate::error::{Error, Invalid, Refusal, Result, Why};
use crate::flow::{self, fold, ok, Fallible, Flow};
use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::intent;
use crate::io::{Capabilities, Io, Kind, Lock, Root, Task};
use crate::layout::Layout;
use crate::log::{
    Bound, Displaced, Entry, EntryKind, FileFact, Forked, Genesis, Logged, Op, Settle, Settlement,
};
use crate::merge::{side_by_side, Folded, Part};
use crate::path::RelPath;
use crate::pending::{self, PendingRecord};
use crate::plan::{FactChange, FileChange, Plan};
use crate::reader::{ReadReport, Reader, Stamp};
use crate::recovery::{self, Settling};
use crate::report::{
    By, Change, Committed, Compacted, Emptied, Fork, Gap, HistoryItem, Item, ItemChange, Lag,
    Local, Mode, Opened, Orphan, Outcome, Partial, PartialReport, PathChange, Presence, Refreshed,
    Rekey, ScanReport, Settled, Start, TrashItem, What, WriterInfo,
};
use crate::schema::Schema;
use crate::trash;
use crate::undo::{self, Before, History};
use crate::view::{FileRef, FileState, Listed, Parts, View};
use crate::writer::{self, Claimed, Writer};

pub struct Library {
    layout: Layout,
    schema: Schema,
    env: Env,
    /// The folder's.
    capabilities: Capabilities,
    mode: Mode,
    reader: Reader,
    /// `None` until the first write, and again after this writer had to stop.
    writer: Option<Writer>,
    /// The latest clock reading seen or written.
    clock: Hlc,
    /// This writer's interrupted effects, settled before its next write is planned.
    unsettled: Vec<Settling>,
    /// This writer's records whose entries are logged and that are still in the
    /// folder: those opening found, and one whose removal at commit failed. Each
    /// is removed before the next write.
    spent: Vec<(Nonce, PendingRecord)>,
    orphaned: Vec<Orphan>,
    /// The other writers of this install's pool that no instance writes as now, by
    /// id, with their geneses: their locks are held, and their effects are this
    /// instance's to settle.
    pooled: BTreeMap<WriterId, EntryHash>,
    /// Pending records recovery would not act on.
    ignored: Vec<RelPath>,
    /// What is shown: the merged state the reader derives.
    folded: Folded,
    /// Readings too far past this machine's clock for it to follow yet.
    ahead: BTreeSet<Hlc>,
    /// The file facts `folded` shows.
    facts: Facts,
    /// The last scan; before the first, the identities `remembered` keeps.
    scan: Scan,
    /// Each file the last scan listed, as the views show it; `None` until a scan of
    /// every file has run.
    listing: Option<Arc<BTreeMap<RelPath, Listed>>>,
    /// The directories the last scan of every file could not list.
    unlisted: Vec<RelPath>,
    /// The directories below the root a scan of every file just listed, until a
    /// refresh reports them.
    walked: Option<Vec<RelPath>>,
    /// The identities scans read that no fact gives, as the local root should keep
    /// them.
    remembered: Scan,
    /// Whether the local root lacks `remembered`, since writing it failed.
    unkept: bool,
    /// What a failed scan left unknown, until a scan of every file succeeds.
    unscanned: Unscanned,
    /// A scan of every file that stopped before it finished, to go on with.
    walking: Option<Walk>,
    /// Bound from `facts`, `scan` and `unscanned` unless a binding is due.
    bindings: Arc<Bindings>,
    /// Whether `facts`, `scan` or `unscanned` changed since `bindings` were bound.
    bind_due: bool,
    /// The moves the bindings found that the facts do not say yet.
    pins: Vec<Op>,
    presence: BTreeMap<WriterId, Presence>,
    history: History,
    /// The entry the intent being committed undoes or redoes.
    reversing: Option<EntryHash>,
    view: View,
    /// Whether a read folded in something the view does not show yet.
    unshown: bool,
    /// The parts of this instance's state that lag what it committed, with why.
    behind: BTreeMap<Lag, String>,
}

/// An intent resolved against the view, before anything is written.
struct Resolved {
    label: String,
    created: Vec<EntityId>,
    facts: Vec<Op>,
    effects: Rc<EffectPlan>,
    reverses: Option<EntryHash>,
}

impl Library {
    /// Claims a writer from the pool, reads the folder and assesses recovery.
    /// No library file is scanned: each entity's file is
    /// [`FileState::Unscanned`] at the path its facts give until
    /// [`Library::rescan`] finishes. `capabilities` are the folder's.
    pub fn open(
        layout: Layout,
        schema: Schema,
        env: Env,
        capabilities: Capabilities,
    ) -> Task<'static, Result<(Library, Opened)>> {
        flow::run(Writer::pick())
            .and_then(|picked| {
                flow::read_replaced(Root::Local, Layout::identities()).map_ok(move |bytes| {
                    let remembered = bytes.and_then(|bytes| Scan::of_identities(&bytes));
                    (picked, remembered.unwrap_or_default())
                })
            })
            .and_then(move |(picked, remembered)| {
                let reader = Reader::new(layout.clone());
                flow::run(reader.list()).and_then(move |listing| {
                    let mut reader = reader;
                    let report = reader.absorb(listing);
                    let claimed = match picked {
                        Some(picked) => picked.resume(layout.clone(), &reader),
                        None => Task::ready(Ok(Claimed {
                            writer: None,
                            start: Start::New,
                        })),
                    };
                    flow::run(claimed).map_ok(move |claimed| {
                        let library = Library::new(
                            layout,
                            schema,
                            env,
                            capabilities,
                            reader,
                            claimed.writer,
                            remembered,
                        );
                        (library, report, claimed.start)
                    })
                })
            })
            .map_ok(|(library, report, start)| (fold_opened(library), report, start))
            .and_then(|(library, report, start)| {
                idle_pool(library)
                    .and_then(recover)
                    .map_ok(move |library| (library, report, start))
            })
            .map_ok(|(mut library, report, start)| {
                library.presume_bound();
                (library, report, start)
            })
            .and_then(|(library, report, start)| {
                flow::run(library.presence_task()).and_then(move |presence| {
                    let mut library = library;
                    for orphan in &mut library.orphaned {
                        orphan.live = presence.get(&orphan.writer) == Some(&Presence::SameMachine);
                    }
                    library.presence = presence;
                    library.show();
                    ok((library, report, start))
                })
            })
            .map_ok(|(library, report, start)| {
                let opened = Opened {
                    mode: library.mode.clone(),
                    start,
                    settled: library
                        .unsettled
                        .iter()
                        .map(|settling| Settled {
                            record: settling.record,
                            label: settling.pending.label.clone(),
                            outcome: settling.outcome.clone(),
                        })
                        .collect(),
                    orphaned: library.orphaned.clone(),
                    unreadable: report
                        .unreadable
                        .into_iter()
                        .map(|(path, _)| path)
                        .collect(),
                    ignored: library.ignored.clone(),
                };
                (library, opened)
            })
            .task()
    }

    fn new(
        layout: Layout,
        schema: Schema,
        env: Env,
        capabilities: Capabilities,
        reader: Reader,
        writer: Option<Writer>,
        remembered: Scan,
    ) -> Self {
        Self {
            layout,
            schema,
            env,
            capabilities,
            mode: Mode::Writable,
            reader,
            writer,
            clock: Hlc::ZERO,
            unsettled: Vec::new(),
            spent: Vec::new(),
            orphaned: Vec::new(),
            pooled: BTreeMap::new(),
            ignored: Vec::new(),
            folded: Folded::default(),
            ahead: BTreeSet::new(),
            facts: Facts::new(),
            scan: remembered.clone(),
            listing: None,
            unlisted: Vec::new(),
            walked: None,
            remembered,
            unkept: false,
            unscanned: Unscanned::default(),
            walking: None,
            bindings: Arc::default(),
            bind_due: false,
            pins: Vec::new(),
            presence: BTreeMap::new(),
            history: History::default(),
            reversing: None,
            unshown: false,
            view: View::new(Parts {
                folded: Folded::default(),
                bindings: Arc::default(),
                listing: None,
                writers: Vec::new(),
                forks: Vec::new(),
                gaps: Vec::new(),
                items: Vec::new(),
            }),
            behind: BTreeMap::new(),
        }
    }

    /// Whether a library just opened may write.
    fn mode_at_open(&self) -> Mode {
        match (&self.writer, self.capabilities.append) {
            (_, false) => Mode::ReadOnly(Why::FolderNotWritable),
            (Some(writer), true) => match newer_entry(&self.reader, writer.id()) {
                Some(entry) => Mode::ReadOnly(Why::NewerOwnHistory { entry }),
                None => Mode::Writable,
            },
            (None, true) => Mode::Writable,
        }
    }

    pub fn view(&self) -> View {
        self.view.clone()
    }

    /// What the last binding found, with every file no entity is bound to as
    /// arrived.
    fn scan_report(&self) -> ScanReport {
        let unbound = self.bindings.unbound.iter();
        ScanReport {
            arrived: unbound.cloned().collect(),
            ..self.bindings.report.clone()
        }
    }

    /// What the view would show if the reader derived it again from scratch, as
    /// tests compare with what it shows.
    #[doc(hidden)]
    pub fn refolded(&self) -> Folded {
        self.reader.derive_afresh().folded
    }

    /// What this instance binds, and what binding every fact of the view afresh
    /// to the last scan binds, as tests compare them.
    #[doc(hidden)]
    pub fn rebound(&mut self) -> [(Bindings, Vec<Op>); 2] {
        self.bind_facts();
        let names = &*self.env.names;
        let facts = self.folded.files();
        let afresh = binding::bind_known(&facts, &self.scan, names, &self.unscanned);
        [((*self.bindings).clone(), self.pins.clone()), afresh]
    }

    /// An id for an entity an intent being built creates.
    pub fn entity_id(&mut self) -> EntityId {
        self.env.entity_id()
    }

    pub fn history(&self) -> &[HistoryItem] {
        self.history.items()
    }

    /// Settles this writer's interrupted effects, then checks every precondition
    /// against the view they leave and the files, logs the intent and carries out
    /// its file effects. Before its first write a new writer is created. A refusal
    /// is [`crate::Error::Refused`], and the refused intent changes nothing. Effects
    /// that stop partway are [`crate::Error::Partial`]; the intent is logged as far
    /// as they got. Settling that stops partway is [`crate::Error::Unfinished`],
    /// and the intent is not tried. Otherwise a commit whose entries are durable
    /// succeeds, and [`Committed::local`] says what of this instance lags it.
    pub fn commit(
        &mut self,
        plan: std::result::Result<Plan, Invalid>,
    ) -> Task<'_, Result<Committed>> {
        match plan {
            Ok(plan) => self.commit_with(move |library| library.resolve(plan)),
            Err(invalid) => Task::ready(
                self.writable()
                    .and(Err(Error::Refused(Refusal::Invalid(invalid)))),
            ),
        }
    }

    /// Undoes this session's latest intent not undone.
    pub fn undo(&mut self) -> Task<'_, Result<Committed>> {
        self.commit_with(|library| {
            let writer = library.own_id()?;
            let plan = library.history.plan_undo(writer, &library.view);
            library.resolve(plan.map_err(Error::Refused)?)
        })
    }

    /// Redoes this session's latest undo, while nothing was committed since.
    pub fn redo(&mut self) -> Task<'_, Result<Committed>> {
        self.commit_with(|library| {
            let writer = library.own_id()?;
            let plan = library.history.plan_redo(writer, &library.view);
            library.resolve(plan.map_err(Error::Refused)?)
        })
    }

    /// Settles now this writer's effects a crash interrupted, as
    /// [`Opened::settled`] reports them, rather than before its next write, so the
    /// files they moved are where their outcome puts them.
    pub fn settle_interrupted(&mut self) -> Task<'_, Result<()>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        settle_first(self).map_ok(drop).task()
    }

    /// Commits what `resolve` makes of the library once this writer's
    /// interrupted effects are settled.
    fn commit_with<'a>(
        &'a mut self,
        resolve: impl FnOnce(&mut Library) -> Result<Resolved> + 'a,
    ) -> Task<'a, Result<Committed>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        settle_first(self)
            .and_then(move |library| match resolve(library) {
                Ok(resolved) => commit(library, resolved),
                Err(error) => Flow::Done(Err(error)),
            })
            .map_ok(|(_, committed)| committed)
            .task()
    }

    /// Notes how saving `part` again went.
    fn lag(&mut self, part: Lag, saved: &Result<()>) {
        match saved {
            Ok(()) => self.behind.remove(&part),
            Err(error) => self.behind.insert(part, error.to_string()),
        };
    }

    fn local(&self) -> Local {
        match self.behind.is_empty() {
            true => Local::Current,
            false => Local::Behind(self.behind.clone()),
        }
    }

    fn own_id(&self) -> Result<WriterId> {
        self.writer
            .as_ref()
            .map(Writer::id)
            .ok_or(Error::Refused(Refusal::Nothing))
    }

    /// Settles another writer's unfinished effect, with the user's consent, in
    /// this writer's own log: finishes it, rolls it back or dismisses it. Nothing
    /// is written in the other writer's directory. Effects that stop partway are
    /// [`crate::Error::Partial`] and leave the effect open to settle again. Finishing
    /// or rolling back a directory rename this folder cannot make is refused with
    /// [`crate::Refusal::Unsupported`]; dismissing it is not.
    pub fn settle(&mut self, orphan: Orphan, how: Settlement) -> Task<'_, Result<Committed>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        settle_first(self)
            .and_then(move |library| {
                let read = pending::read_one(&library.layout, orphan.writer, orphan.record);
                flow::run(read).map_ok(move |theirs| (library, theirs))
            })
            .and_then(move |(library, theirs)| {
                let open = theirs.filter(|theirs| library.is_orphan(orphan.record, theirs));
                let Some(theirs) = open else {
                    return Flow::Done(Err(Error::Refused(Refusal::Nothing)));
                };
                let reached = recovery::progress(
                    &library.layout,
                    orphan.record,
                    &theirs,
                    library.capabilities,
                );
                flow::run(reached).and_then(move |reached| {
                    let plan = recovery::orphan_plan(
                        &library.layout,
                        &theirs,
                        &reached,
                        how,
                        library.capabilities,
                        &mut library.env,
                    );
                    match plan {
                        Ok(plan) => settle_orphan(library, orphan, theirs, how, plan),
                        Err(refusal) => Flow::Done(Err(Error::Refused(refusal))),
                    }
                })
            })
            .task()
    }

    /// Reads other writers' new entries: what changed since the last view,
    /// attributed to its writer, or to nobody for outside changes, and what the
    /// folder no longer holds. A sealed segment or snapshot read before is taken
    /// by its name. Of the library's files it scans again only those at the paths
    /// whose file facts the entries changed, unless no scan of every file has
    /// finished since the library opened or since a scan failed: then it scans
    /// every file. Writes nothing in the folder.
    pub fn refresh(&mut self) -> Task<'_, Result<Refreshed>> {
        self.look(Look::Logs)
    }

    /// Refreshes, asking the length and time of every file in the writers'
    /// directories, and scans every library file, to find what changed outside.
    /// An app rescans once a library opens. A rescan that stops partway, as when
    /// the app drops it to commit, keeps what it listed, and the next one goes on
    /// from there; nothing is bound from it until every directory is listed, and
    /// the directories a commit or refresh scanned meanwhile are listed again.
    pub fn rescan(&mut self) -> Task<'_, Result<Refreshed>> {
        self.look(Look::Everything)
    }

    /// Refreshes, and scans the library files at `paths` and under them, such as
    /// those a watcher saw change.
    pub fn rescan_paths(&mut self, paths: Vec<RelPath>) -> Task<'_, Result<Refreshed>> {
        self.look(Look::Paths(paths))
    }

    /// Reads the writers' directories and scans what `scope` asks, then reports
    /// what changed since the view, so a look the app dropped partway is reported
    /// by the next.
    fn look(&mut self, scope: Look) -> Task<'_, Result<Refreshed>> {
        let listed = match scope {
            Look::Everything => self.reader.list(),
            Look::Logs | Look::Paths(_) => self.reader.probe(),
        };
        flow::run(listed)
            .and_then(move |listing| {
                let report = self.reader.absorb(listing);
                let moved = self.absorb_read(&report);
                self.unshown |= report.anything_new();
                let retired = self.forked();
                keep_local(self, false)
                    .then(move |(library, kept)| match (kept, retired) {
                        (Err(error), _) => Flow::Done(Err(error)),
                        (Ok(()), Some(_)) => library.stop_writing().map_ok(move |()| library),
                        (Ok(()), None) => ok(library),
                    })
                    .and_then(move |library| scan_for(library, scope, moved))
                    .map_ok(move |library| (library, retired))
            })
            .and_then(|(library, retired)| {
                repooled(library)
                    .and_then(reorphaned)
                    .map_ok(move |library| (library, retired))
            })
            .and_then(|(library, retired)| {
                let changes = library.since_view();
                library.reshow();
                ok(Refreshed {
                    changes,
                    scan: library.scan_report(),
                    unlisted: library.unlisted.clone(),
                    dirs: library.walked.take(),
                    orphaned: library.orphaned.clone(),
                    retired,
                })
            })
            .task()
    }

    /// What changed since the view.
    fn since_view(&self) -> Vec<Change> {
        let shown = self.view.clone();
        let mut changes = match self.unshown {
            true => self.attribute(shown.folded()),
            false => Vec::new(),
        };
        let explained: BTreeSet<EntityId> = changes
            .iter()
            .filter(|change| change.what == What::File)
            .map(|change| change.entity)
            .collect();
        let before = &shown.parts().bindings;
        if Arc::ptr_eq(before, &self.bindings) {
            return changes;
        }
        for pair in side_by_side(&self.bindings.bound, &before.bound) {
            let (Some((entity, file)), was) = pair else {
                continue;
            };
            let was = was.map(|(_, was)| presumed(was));
            if was.as_ref() != Some(file) && !explained.contains(entity) {
                changes.push(Change {
                    entity: *entity,
                    what: What::File,
                    by: By::Outside,
                });
            }
        }
        changes
    }

    /// Every writer with its last entry time, and whether another instance on this
    /// machine holds it.
    pub fn others(&mut self) -> Task<'_, Result<Vec<WriterInfo>>> {
        flow::run(self.presence_task())
            .map_ok(move |presence| {
                self.presence = presence;
                self.show();
                self.writer_infos()
            })
            .task()
    }

    /// The bytes still in the trash of each writer this install wrote as, oldest
    /// first: this writer's, which undo restores and emptying removes, and those
    /// of the writers it retired, kept as they are.
    pub fn trash(&mut self) -> Task<'_, Result<Vec<TrashItem>>> {
        flow::list(Root::Local, &RelPath::ROOT)
            .and_then(move |entries| {
                let geneses: BTreeSet<EntryHash> = entries
                    .iter()
                    .filter(|entry| entry.kind == Kind::Directory)
                    .filter_map(|entry| entry.name.parse().ok())
                    .collect();
                let writers = self.reader.derived().writers.iter();
                let installed = writers.filter(|(_, placed)| {
                    genesis_of(placed).is_some_and(|genesis| geneses.contains(&genesis))
                });
                let lists: Vec<_> = installed
                    .map(|(writer, _)| {
                        trash::list(&self.layout, *writer, self.folded.trash(*writer))
                    })
                    .collect();
                fold(lists.into_iter(), Vec::new(), |mut all, list| {
                    flow::run(list).map_ok(move |items| {
                        all.extend(items);
                        all
                    })
                })
            })
            .map_ok(|mut items: Vec<TrashItem>| {
                items.sort_by_key(|item| (item.at, item.item));
                items
            })
            .task()
    }

    /// Removes from this writer's trash every item but those of `kept` and those a
    /// pending record of its names, which settling may still move: the only way bytes
    /// leave the folder. An undo that needs a removed item is refused with
    /// [`Refusal::Emptied`].
    pub fn empty_trash(&mut self, kept: BTreeSet<Nonce>) -> Task<'_, Result<Emptied>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        let Some(writer) = self.writer.as_ref().map(Writer::id) else {
            return Task::ready(Ok(Emptied::default()));
        };
        remove_spent(self)
            .then(move |(library, _)| {
                let listed = trash::list(&library.layout, writer, library.folded.trash(writer));
                flow::run(listed).and_then(move |items| {
                    flow::run(trash::empty(&library.layout, writer, items, kept))
                })
            })
            .task()
    }

    /// Folds this writer's own entries into a snapshot and deletes the sealed
    /// segments of its directory the snapshot certifies. A writer also compacts
    /// at close and every [`COMPACT_AFTER`] entries. Refused with
    /// [`Refusal::Nothing`] when this instance writes as no writer yet, and when
    /// a snapshot in the writer's directory already has its last entry. Fails
    /// with [`Error::Rekey`] when the writer's history forked or the folder lost
    /// its last entry, and the instance stops writing as it. This writer's
    /// interrupted effects are settled first, and a failure to settle them fails
    /// the compaction.
    #[doc(hidden)]
    pub fn compact(&mut self) -> Task<'_, Result<Compacted>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        if self.writer.is_none() {
            return Task::ready(Err(Error::Refused(Refusal::Nothing)));
        }
        settle_first(self)
            .and_then(|library| compacted(library).then(|(_, compacted)| Flow::Done(compacted)))
            .task()
    }

    /// Compacts the writer of a writable library, records its head if that lags,
    /// removes the records its entries closed that are still in the folder, seals
    /// the open segment and releases the writer's lock, then lets go of what the
    /// library holds: nothing is left to view, and the library only drops. A
    /// record it cannot remove stays for the next instance to find, and that
    /// failure is returned once the library is closed. A library dropped without
    /// closing leaves its segment open, as a crash does.
    pub fn close(&mut self) -> Task<'_, Result<()>> {
        let Some(writer) = self.writer.take() else {
            self.dismantle();
            return Task::ready(Ok(()));
        };
        self.writer = Some(writer);
        compact_due(self, true)
            .then(|library| keep_local(library, false))
            .then(|(library, kept)| match (kept, library.writable()) {
                (Err(error), _) => Flow::Done(Err(error)),
                (Ok(()), Ok(())) => remove_spent(library).then(flow::ok),
                (Ok(()), Err(_)) => ok((library, Ok(()))),
            })
            .and_then(|(library, removed)| match library.writer.take() {
                Some(writer) => flow::run(writer.close()).map_ok(move |()| (library, removed)),
                None => ok((library, removed)),
            })
            .then(|closed| {
                let closed = closed.and_then(|(library, removed)| {
                    library.dismantle();
                    removed
                });
                Flow::Done(closed)
            })
            .task()
    }

    /// Lets go of what the library holds: nothing is left to view.
    fn dismantle(&mut self) {
        self.view = View::new(Parts::default());
        self.folded = Folded::default();
        self.reader.shed();
        self.facts = Facts::default();
        self.scan = Scan::default();
        self.bind_due = false;
        self.bindings = Arc::default();
        self.history = History::default();
    }

    /// Whether `theirs`, another writer's record `name`, is still open: confined
    /// to the library, chained to its writer's log, not closed and not settled.
    fn is_orphan(&self, name: Nonce, theirs: &PendingRecord) -> bool {
        let own = self.writer.as_ref().map(Writer::id);
        let Some(placed) = self.reader.derived().writers.get(&theirs.writer) else {
            return false;
        };
        own != Some(theirs.writer)
            && theirs.is_confined(&self.layout)
            && recovery::Chain::holds(placed, theirs.after())
            && !self.closed_records().contains(&(theirs.writer, name))
    }

    fn writable(&self) -> Result<()> {
        match &self.mode {
            Mode::Writable => Ok(()),
            Mode::ReadOnly(why) => Err(Error::ReadOnly(why.clone())),
        }
    }

    /// Checks `plan` against the view and turns its facts into ops and its file
    /// changes into effects. A file change taken from an item is refused unless
    /// the last scan found that file.
    fn resolve(&mut self, plan: Plan) -> Result<Resolved> {
        let refused = |invalid| Error::Refused(Refusal::Invalid(invalid));
        for (change, take) in &plan.facts {
            let (FactChange::File { entity, file }, Some(_)) = (change, take) else {
                continue;
            };
            let bound = self.view.at(&file.path).map(|bound| bound.id());
            binding::bindable(*entity, file, &self.scan, &self.unscanned, bound).map_err(
                |why| {
                    refused(Invalid::Unbindable {
                        entity: *entity,
                        path: file.path.clone(),
                        why,
                    })
                },
            )?;
        }
        let facts = intent::ops(&plan, &self.folded, &self.schema).map_err(refused)?;
        let revived: BTreeSet<EntityId> = plan
            .facts
            .iter()
            .filter_map(|(change, _)| match change {
                FactChange::Revive { entity } => Some(*entity),
                _ => None,
            })
            .collect();
        for change in &plan.files {
            let (FileChange::Save { entity, .. } | FileChange::Adopt { entity, .. }) = change
            else {
                continue;
            };
            let known = plan.created.contains(entity)
                || self.folded.present(*entity)
                || revived.contains(entity);
            if !known {
                return Err(refused(Invalid::NoEntity(*entity)));
            }
        }
        let effects = effects::resolve(
            &plan.files,
            &self.bindings,
            &self.layout,
            self.capabilities,
            &mut self.env,
        )
        .map_err(Error::Refused)?;
        Ok(Resolved {
            label: plan.label,
            created: plan.created,
            facts,
            effects: Rc::new(effects),
            reverses: plan.reverses,
        })
    }

    fn tick(&mut self) -> Result<Hlc> {
        let now = self.env.now_ms();
        let at = self
            .clock
            .tick(now)
            .ok_or(Error::ReadOnly(Why::ClockExhausted))?;
        self.clock = at;
        Ok(at)
    }

    /// The file-register writes that pin the moves this writer found, but for
    /// entities whose files the intent changes itself.
    fn pins(&mut self, effects: &EffectPlan) -> Vec<Op> {
        self.bind_facts();
        let changed: BTreeSet<EntityId> = effects.files.iter().map(|end| end.entity).collect();
        let pins = self
            .pins
            .iter()
            .filter(|op| file_of(op).is_none_or(|e| !changed.contains(&e)));
        pins.cloned().collect()
    }

    /// `logged` with every file op and displaced item carrying out `effects` may
    /// add, each as long as it can be, and the fork points of every writer they
    /// name: an intent whose widest entry does not fit a line is refused before
    /// anything is written.
    fn widest(&self, logged: &Logged, effects: &EffectPlan) -> Logged {
        let fact = |path: &RelPath| FileFact {
            path: path.clone(),
            identity: Identity::from_u128(u128::MAX),
            len: u64::MAX,
            modified: Some(u64::MAX),
        };
        let mut widest = logged.clone();
        for end in &effects.files {
            let writes = self.folded.file_writes(end.entity);
            let replaces = writes.iter().map(crate::merge::Write::named).collect();
            let (entity, file) = (end.entity, end.path.as_ref().map(fact));
            widest.ops.push(match (file, end.pin) {
                (Some(file), true) => Op::Pin {
                    entity,
                    file,
                    replaces,
                    take: None,
                },
                (file, _) => Op::File {
                    entity,
                    file,
                    replaces,
                    take: None,
                },
            });
        }
        widest
            .ops
            .extend(effects.unbound.iter().map(|end| Op::Path {
                from: end.from.clone(),
                file: end.path.as_ref().map(fact),
            }));
        widest.ops.extend(effects.trees.iter().map(|end| Op::Tree {
            from: end.from.clone(),
            to: end.to.clone(),
        }));
        widest.ops.extend(effects.dirs.iter().map(|end| Op::Dir {
            path: end.path.clone(),
            made: end.made,
        }));
        widest.displaced = effects
            .steps
            .iter()
            .filter_map(|step| match step {
                EffectStep::ToTrash { path, item } => Some(Displaced {
                    item: *item,
                    from: path.clone(),
                    identity: Identity::from_u128(u128::MAX),
                    len: u64::MAX,
                }),
                _ => None,
            })
            .collect();
        widest.forks = self.known_forks(&widest.ops);
        widest
    }

    /// The file ops logging what the effects, whose ends are `ends`, did to each
    /// entity's file. A file given as it is is pinned, so undo leaves it alone.
    fn file_ops(&self, applied: &Applied, ends: &[FileEnd]) -> Vec<Op> {
        let pinned: BTreeSet<EntityId> = ends
            .iter()
            .filter(|end| end.pin)
            .map(|end| end.entity)
            .collect();
        let ops = applied
            .files
            .iter()
            .map(|(entity, file)| {
                let replaces = self
                    .folded
                    .file_writes(*entity)
                    .iter()
                    .map(crate::merge::Write::named)
                    .collect();
                match (file, pinned.contains(entity)) {
                    (Some(file), true) => Op::Pin {
                        entity: *entity,
                        file: file.clone(),
                        replaces,
                        take: None,
                    },
                    (file, _) => Op::File {
                        entity: *entity,
                        file: file.clone(),
                        replaces,
                        take: None,
                    },
                }
            })
            .chain(applied.unbound.iter().map(|(from, file)| Op::Path {
                from: from.clone(),
                file: file.clone(),
            }));
        let trees = applied.trees.iter().map(|(from, to)| Op::Tree {
            from: from.clone(),
            to: to.clone(),
        });
        let dirs = applied.dirs.iter().map(|(path, made)| Op::Dir {
            path: path.clone(),
            made: *made,
        });
        trees.chain(dirs).chain(ops).collect()
    }

    /// Keeps an open pending record a failure left behind: this writer's is
    /// settled before its next write is planned; one of a writer it stopped
    /// writing as is an orphan.
    fn interrupted(&mut self, name: Nonce, record: &PendingRecord) {
        let own = self.writer.as_ref().map(Writer::id) == Some(record.writer);
        match own {
            true => self.unsettled.push(Settling {
                record: name,
                pending: record.clone(),
                outcome: Outcome::Complete,
            }),
            false => {
                let ours = self.pooled.contains_key(&record.writer);
                self.orphan(name, record, ours);
            }
        }
    }

    /// Keeps another writer's open record `name` to settle with consent, as
    /// this install's own when `ours`.
    fn orphan(&mut self, name: Nonce, record: &PendingRecord, ours: bool) {
        self.orphaned.push(Orphan {
            ours,
            live: self.presence.get(&record.writer) == Some(&Presence::SameMachine),
            writer: record.writer,
            record: name,
            label: record.label.clone(),
            paths: record.paths(),
            progress: 0,
        });
    }

    /// Places entries this instance just appended to `written`, the open segment
    /// with its stamp when known, and folds them in, keeping its intents in the
    /// history with what they replaced.
    fn absorb_own(&mut self, entries: &[Entry], written: Option<(RelPath, Option<Stamp>)>) {
        let Some(id) = self.writer.as_ref().map(Writer::id) else {
            return;
        };
        let mut reverses = self.reversing.take();
        let mut intents: Vec<(&Entry, Logged)> = Vec::new();
        for entry in entries {
            if let EntryKind::Intent(logged) = entry.kind() {
                intents.push((entry, logged));
            }
            self.clock = self.clock.observe(entry.at());
        }
        for (entry, logged) in intents {
            let before = Before::of(&logged.ops, &self.folded);
            self.history.push(entry, logged, reverses.take(), before);
        }
        if let Some((path, stamp)) = written {
            self.reader.add(id, entries, &path, stamp);
        }
        self.take_derived();
    }

    /// Takes what the reader derives as what is shown. Returns the library
    /// paths the changed file facts name, and those where the entities they
    /// changed were bound.
    fn take_derived(&mut self) -> Vec<RelPath> {
        let folded = self.reader.derived().folded.clone();
        let changed = folded.changed(&self.folded);
        self.folded = folded;
        self.refile(changed.into_iter())
    }

    /// Takes in what a read derived. Returns the library paths the changed file
    /// facts name, and those where the entities they changed were bound.
    fn absorb_read(&mut self, report: &ReadReport) -> Vec<RelPath> {
        if !report.anything_new() {
            self.follow(Vec::new());
            return Vec::new();
        }
        let moved = self.take_derived();
        self.follow(self.reader.readings().collect());
        moved
    }

    /// The fork points this writer knows of the writers whose writes `ops` name.
    fn known_forks(&self, ops: &[Op]) -> Vec<Forked> {
        let named: BTreeSet<WriterId> = ops
            .iter()
            .flat_map(|op| contribution::said(op).refs)
            .map(|(_, target)| target.writer)
            .collect();
        let writers = &self.reader.derived().writers;
        let fork = |writer: WriterId| {
            Some(Forked {
                writer,
                at: writers.get(&writer)?.fork?,
            })
        };
        named.into_iter().filter_map(fork).collect()
    }

    /// The paths `was`, an entity's file facts before, named, those its facts name
    /// now, and where it was bound.
    fn fact_paths(&self, entity: EntityId, was: Option<&[Fact]>) -> Vec<RelPath> {
        let now = self.facts.get(&entity).map(|now| &now[..]);
        let logged = was.into_iter().chain(now).flatten();
        let bound = self.bindings.bound.get(&entity).map(|file| &file.path);
        logged
            .map(|fact| &fact.value.path)
            .chain(bound)
            .cloned()
            .collect()
    }

    /// Moves the clock to the latest of `readings`, and of those it could not
    /// follow before, no more than [`MAX_DRIFT_MS`] past this machine's clock.
    /// Later ones wait until they are not, so one wrong or forged clock cannot
    /// pin every writer's.
    fn follow(&mut self, readings: Vec<Hlc>) {
        let until = self.env.now_ms().saturating_add(MAX_DRIFT_MS);
        let waiting = std::mem::take(&mut self.ahead);
        for at in readings.into_iter().chain(waiting) {
            match at.wall_ms <= until {
                true => self.clock = self.clock.observe(at),
                false => {
                    self.ahead.insert(at);
                }
            }
        }
    }

    /// Takes the file facts of `entities` from what is shown, and binds again
    /// before the next view if any changed. Returns the paths those facts named
    /// before and name now, and where their entities were bound.
    fn refile(&mut self, entities: impl Iterator<Item = EntityId>) -> Vec<RelPath> {
        let mut moved = Vec::new();
        for entity in entities {
            let now = self.folded.file(entity);
            if self
                .facts
                .get(&entity)
                .map_or(now.is_empty(), |was| *was == now)
            {
                continue;
            }
            let was = match now.is_empty() {
                true => self.facts.remove(&entity),
                false => self.facts.insert(entity, now),
            };
            self.bind_due = true;
            moved.extend(self.fact_paths(entity, was.as_deref()));
        }
        moved
    }

    /// The pending records a placed entry closes, in the merged state or on a
    /// branch shown apart as its own item or dismissed.
    fn closed_records(&self) -> BTreeSet<(WriterId, Nonce)> {
        let mut closed = self.folded.settlements().clone();
        for (writer, placed) in &self.reader.derived().writers {
            for tip in &placed.tips {
                closed.extend(tip.state.settled.iter().copied());
            }
            let apart = placed.runs.iter().chain(&placed.gaps);
            for entry in apart.chain(&placed.dismissed) {
                match entry.kind() {
                    EntryKind::Intent(Logged {
                        record: Some(record),
                        ..
                    }) => {
                        closed.insert((*writer, record));
                    }
                    EntryKind::Settle(settle) => {
                        closed.insert((settle.writer, settle.record));
                    }
                    _ => {}
                }
            }
        }
        closed
    }

    /// This instance's writer, when the reader finds its history forked.
    fn forked(&self) -> Option<WriterId> {
        let own = self.writer.as_ref().map(Writer::id)?;
        let placed = self.reader.derived().writers.get(&own)?;
        placed.fork.map(|_| own)
    }

    /// Stops writing as this writer: it leaves the pool, and the next commit
    /// creates a new one. Its open interrupted effects become orphans, settled
    /// only with consent, and its spent records stay in the folder.
    fn stop_writing<'a>(&mut self) -> Fallible<'a, ()> {
        let Some(writer) = self.writer.take() else {
            return ok(());
        };
        self.history = History::default();
        self.behind.remove(&Lag::Head);
        self.spent.clear();
        for settling in std::mem::take(&mut self.unsettled) {
            self.orphan(settling.record, &settling.pending, true);
        }
        flow::run(writer::retire_writer(writer.genesis()))
    }

    /// Binds again where the facts or the scan changed since the last binding.
    fn bind_facts(&mut self) {
        if self.bind_due {
            self.bind_everything();
        }
    }

    /// Binds every fact to the scan.
    fn bind_everything(&mut self) {
        let names = &*self.env.names;
        let (bindings, pins) = binding::bind_known(&self.facts, &self.scan, names, &self.unscanned);
        self.pins = pins;
        self.bindings = Arc::new(bindings);
        self.bind_due = false;
    }

    /// Takes `scan`, listed as the views show it once a scan of every file ran,
    /// and binds the facts to it, unless neither changed.
    fn rebind(&mut self, scan: Scan) {
        if !self.bind_due && scan == self.scan {
            return;
        }
        if self.listing.is_some() {
            self.listing = Some(listing(&scan));
        }
        self.scan = scan;
        self.bind_everything();
    }

    /// Binds every entity with a file where its facts say, knowing none of the
    /// library's files until a scan of every one succeeds.
    fn presume_bound(&mut self) {
        self.unscanned.add(vec![RelPath::ROOT], &self.bindings);
        self.bind_everything();
    }

    /// Shows what changed since the view, if anything did.
    fn reshow(&mut self) {
        self.bind_facts();
        let rebound = !Arc::ptr_eq(&self.view.parts().bindings, &self.bindings);
        if self.unshown || rebound {
            self.show();
        }
    }

    /// The identities the local root should keep of those scans read that no fact
    /// gives: those just read, and those kept before of files the last scan found
    /// unchanged. `None` when the local root keeps them already.
    fn remembering(&self, read: Vec<RelPath>) -> Option<Scan> {
        let scan = &self.scan.files;
        let unchanged = self.remembered.files.iter();
        let unchanged = unchanged.filter(|(path, file)| scan.get(*path) == Some(*file));
        let read = read
            .into_iter()
            .filter_map(|path| Some((path.clone(), *scan.get(&path)?)));
        let remembered = Scan {
            files: unchanged
                .map(|(path, file)| (path.clone(), *file))
                .chain(read)
                .collect(),
        };
        (remembered != self.remembered || self.unkept).then_some(remembered)
    }

    fn scan_task(&self) -> Task<'static, Result<Scan>> {
        binding::list_all(&self.layout).task()
    }

    /// Takes `facts`, the file facts of what is now shown.
    fn refiled(&mut self, facts: Facts) {
        self.facts = facts;
        self.bind_due = true;
    }

    /// Who `writer` is to the user: this writer, or another by its label.
    fn by(&self, writer: WriterId) -> By {
        match self.writer.as_ref().map(Writer::id) == Some(writer) {
            true => By::This,
            false => By::Writer {
                writer,
                label: self
                    .reader
                    .derived()
                    .writers
                    .get(&writer)
                    .map(label)
                    .unwrap_or_default(),
            },
        }
    }

    /// Rebuilds the view from the library's state.
    fn show(&mut self) {
        self.bind_facts();
        self.unshown = false;
        let derived = self.reader.derived();
        let mut forks = Vec::new();
        let mut gaps = Vec::new();
        for (writer, placed) in &derived.writers {
            forks.extend(placed.forks.iter().map(|fork| Fork {
                writer: *writer,
                at: fork.n,
                prev: fork.hash,
            }));
            if let Some(first) = placed.gaps.first() {
                gaps.push(Gap {
                    writer: *writer,
                    missing: first.prev(),
                    held: placed.gaps.len(),
                });
            }
        }
        let items = derived
            .items
            .iter()
            .flat_map(|(writer, items)| items.iter().map(move |item| (*writer, item)))
            .map(|(writer, item)| self.item(writer, item))
            .collect();
        let parts = Parts {
            folded: self.folded.clone(),
            bindings: Arc::clone(&self.bindings),
            listing: self.listing.clone(),
            writers: self.writer_infos(),
            forks,
            gaps,
            items,
        };
        self.view = View::new(parts);
    }

    /// `item` of `writer`, as the view shows it.
    fn item(&self, writer: WriterId, item: &derive::Item) -> Item {
        let changes = item.change.iter().map(|(field, (_, _, value))| ItemChange {
            entity: field.entity(),
            what: what_of(field),
            change: fact_change(field, value),
            forgotten: item.forgotten.contains(field),
        });
        Item {
            kind: item.kind,
            run: item.run,
            by: self.by(writer),
            span: item.span,
            entries: item.entries,
            changes: changes.collect(),
        }
    }

    fn writer_infos(&self) -> Vec<WriterInfo> {
        let own = self.writer.as_ref().map(Writer::id);
        self.reader
            .derived()
            .writers
            .iter()
            .map(|(writer, placed)| WriterInfo {
                writer: *writer,
                label: label(placed),
                last_entry_at: last_at(placed),
                here: match Some(*writer) == own {
                    true => Presence::This,
                    false => self
                        .presence
                        .get(writer)
                        .copied()
                        .unwrap_or(Presence::Elsewhere),
                },
            })
            .collect()
    }

    /// Which writers another instance on this machine holds: their locks, tried
    /// and released at once. The locks this instance holds, its own and those of
    /// the idle writers it took up, are not tried: a backend answers that a lock
    /// its process holds is taken, and releasing it would let it go.
    fn presence_task(&self) -> Task<'static, Result<BTreeMap<WriterId, Presence>>> {
        let own = self.writer.as_ref().map(Writer::genesis);
        let held: BTreeSet<EntryHash> = self.pooled.values().copied().chain(own).collect();
        flow::list(Root::Local, &RelPath::ROOT)
            .and_then(move |entries| {
                let pool = entries
                    .into_iter()
                    .filter(|entry| entry.kind == Kind::Directory)
                    .filter_map(|entry| entry.name.parse::<EntryHash>().ok())
                    .filter(move |genesis| !held.contains(genesis));
                fold(pool, BTreeMap::new(), |mut presence, genesis| {
                    flow::read_file(Root::Local, &Layout::head(genesis), u64::MAX).and_then(
                        move |head| {
                            let writer = head
                                .and_then(|bytes| writer::head_record(&bytes))
                                .map(|(writer, _)| writer);
                            let Some(writer) = writer else {
                                return ok(presence);
                            };
                            flow::lock(Layout::head(genesis)).and_then(move |lock| match lock {
                                Lock::Held => {
                                    presence.insert(writer, Presence::SameMachine);
                                    ok(presence)
                                }
                                Lock::Acquired => flow::act(Io::Unlock {
                                    name: Layout::head(genesis),
                                })
                                .map_ok(move |()| presence),
                            })
                        },
                    )
                })
            })
            .task()
    }

    /// The changes the entries and snapshots read since `before` made, by writer.
    fn attribute(&self, before: &Folded) -> Vec<Change> {
        let since = self.folded.since(before).into_iter();
        distinct(since.map(|(entity, part, writer)| Change {
            entity,
            what: what(&part),
            by: self.by(writer),
        }))
    }
}

/// Where `file` was presumed to be: an unscanned file is where its fact says.
fn presumed(file: &FileRef) -> FileRef {
    match file.state {
        FileState::Unscanned => FileRef {
            path: file.path.clone(),
            state: FileState::InSync,
        },
        _ => file.clone(),
    }
}

fn what(part: &Part) -> What {
    match part {
        Part::Created => What::Created,
        Part::Deleted => What::Deleted,
        Part::Field(key) => What::Field(key.clone()),
        Part::File => What::File,
    }
}

/// The part of an entity a write of `field` changes.
fn what_of(field: &contribution::FieldKey) -> What {
    use contribution::FieldKey;
    match field {
        FieldKey::Exist(_) => What::Created,
        FieldKey::Register(_, key) | FieldKey::Member(_, key, _) => What::Field(key.to_string()),
        FieldKey::File(_) => What::File,
    }
}

/// The fact change that writes `value` to `field`; `None` for a file write of no
/// file, which only trashing the file makes.
fn fact_change(field: &contribution::FieldKey, value: &contribution::Value) -> Option<FactChange> {
    use contribution::{FieldKey, Value};
    Some(match (field, value) {
        (FieldKey::Exist(entity), Value::Exist(true)) => FactChange::Revive { entity: *entity },
        (FieldKey::Exist(entity), Value::Exist(false)) => FactChange::Delete { entity: *entity },
        (FieldKey::Register(entity, key), Value::Register(Some(value))) => FactChange::Set {
            entity: *entity,
            key: key.to_string(),
            value: value.clone(),
        },
        (FieldKey::Register(entity, key), Value::Register(None)) => FactChange::Clear {
            entity: *entity,
            key: key.to_string(),
        },
        (FieldKey::Member(entity, key, member), Value::Member(true)) => FactChange::Add {
            entity: *entity,
            key: key.to_string(),
            value: member.clone(),
        },
        (FieldKey::Member(entity, key, member), Value::Member(false)) => FactChange::Remove {
            entity: *entity,
            key: key.to_string(),
            value: member.clone(),
        },
        (
            FieldKey::File(entity),
            Value::File {
                fact: Some(file), ..
            },
        ) => FactChange::File {
            entity: *entity,
            file: file.clone(),
        },
        _ => return None,
    })
}

/// The label a writer's genesis entry gave it, as its tips or its entries hold it.
fn label(placed: &Placed) -> String {
    if let Some(tip) = placed.tips.first() {
        return tip.label.clone();
    }
    let first = placed.merged.first().or(placed.runs.first());
    match first.map(|entry| entry.kind()) {
        Some(EntryKind::Genesis(genesis)) => genesis.label,
        _ => String::new(),
    }
}

/// The latest clock reading of what the reader holds of a writer.
fn last_at(placed: &Placed) -> Option<Hlc> {
    let tips = placed.tips.iter().map(|tip| tip.at);
    let entries = placed.merged.iter().chain(&placed.runs).chain(&placed.gaps);
    tips.chain(entries.map(|entry| entry.at())).max()
}

/// An entry of `writer`'s own history this build does not understand.
fn newer_entry(reader: &Reader, writer: WriterId) -> Option<EntryHash> {
    let placed = reader.derived().writers.get(&writer)?;
    let folded = placed.tips.iter().flat_map(|tip| &tip.state.unknown);
    if let Some((entry, _)) = folded.into_iter().next() {
        return Some(*entry);
    }
    let unknown = |entry: &&std::rc::Rc<Entry>| {
        let kind = entry.kind();
        let op = contribution::ops(&kind)
            .iter()
            .any(|op| matches!(op, Op::Unknown(_)));
        op || matches!(kind, EntryKind::Unknown(_))
    };
    placed.merged.iter().find(unknown).map(|entry| entry.hash())
}

/// How far past this machine's wall clock a reading in the folder may be and still
/// move this writer's clock.
const MAX_DRIFT_MS: u64 = 24 * 60 * 60 * 1000;

/// `changes` without repeats, in order of their first appearance.
fn distinct(changes: impl Iterator<Item = Change>) -> Vec<Change> {
    let mut seen = BTreeSet::new();
    changes
        .filter(|change| seen.insert(change.clone()))
        .collect()
}

/// What `op` changes, as this writer's change.
fn change_of(op: &Op) -> Option<Change> {
    let (entity, what) = match op {
        Op::Create { entity, .. } => (*entity, What::Created),
        Op::Delete { entity, .. } => (*entity, What::Deleted),
        Op::Write { entity, key, .. }
        | Op::Add { entity, key, .. }
        | Op::Remove { entity, key, .. } => (*entity, What::Field(key.clone())),
        Op::File { entity, .. } | Op::Pin { entity, .. } => (*entity, What::File),
        Op::Path { .. }
        | Op::Tree { .. }
        | Op::Dir { .. }
        | Op::Dismiss { .. }
        | Op::Unknown(_) => return None,
    };
    Some(Change {
        entity,
        what,
        by: By::This,
    })
}

/// Takes what a library just opened shows and its file facts, then follows its
/// clock readings and decides whether it may write.
fn fold_opened(mut library: Library) -> Library {
    library.folded = library.reader.derived().folded.clone();
    let facts = library.folded.files();
    library.refiled(facts);
    library.follow(library.reader.readings().collect());
    library.mode = library.mode_at_open();
    library
}

/// How many of its own entries since its last snapshot a writer appends before
/// it compacts.
pub const COMPACT_AFTER: usize = 512;

/// Reads this writer's directory, folds its own entries into a snapshot and
/// deletes what the snapshot certifies, then reads the directory again. A writer
/// whose head the folder no longer holds stops writing.
fn compacted(library: &mut Library) -> Flow<'_, (&mut Library, Result<Compacted>)> {
    let id = library
        .writer
        .as_ref()
        .map(Writer::id)
        .expect("a writer compacts");
    flow::run(library.reader.list_writer(id)).then(move |listing| {
        let listing = match listing {
            Ok(listing) => listing,
            Err(error) => return Flow::Done((library, Err(error))),
        };
        let recorded = recorded_segments(&library.reader, id);
        library.reader.absorb(listing);
        let writer = library.writer.take().expect("checked above");
        let name = library.env.nonce();
        let capabilities = library.capabilities;
        let compaction =
            crate::compaction::compact(writer, &library.reader, &recorded, name, capabilities);
        flow::run(compaction).then(move |(writer, compacted)| {
            library.writer = Some(writer);
            match compacted {
                Err(error @ Error::Rekey { .. }) => {
                    library.take_derived();
                    library.show();
                    library
                        .stop_writing()
                        .then(move |stopped| Flow::Done((library, stopped.and(Err(error)))))
                }
                Err(error) => Flow::Done((library, Err(error))),
                Ok(compacted) => flow::run(library.reader.list_writer(id)).then(move |listing| {
                    let listing = match listing {
                        Ok(listing) => listing,
                        Err(error) => return Flow::Done((library, Err(error))),
                    };
                    library.reader.absorb(listing);
                    library.take_derived();
                    library.show();
                    let Some(writer) = library.forked() else {
                        return Flow::Done((library, Ok(compacted)));
                    };
                    let forked = Error::Rekey {
                        writer,
                        why: Rekey::Forked,
                    };
                    library
                        .stop_writing()
                        .then(move |stopped| Flow::Done((library, stopped.and(Err(forked)))))
                }),
            }
        })
    })
}

/// The lines of each segment of `writer`'s directory the reader's last read
/// found.
fn recorded_segments(reader: &Reader, writer: WriterId) -> Vec<Vec<Rc<Entry>>> {
    let files = reader.files(writer).into_iter();
    let segments = files.filter_map(|(_, _, file)| match file {
        crate::reader::WriterFile::Segment(segment) => Some(segment.entries.clone()),
        _ => None,
    });
    segments.collect()
}

/// Compacts this writer once its own entries since its last snapshot reach
/// [`COMPACT_AFTER`], or, when `closing`, once there are any. A compaction that
/// fails leaves the files for the next.
fn compact_due(library: &mut Library, closing: bool) -> Flow<'_, &mut Library> {
    let own = library.writer.as_ref().map(Writer::id);
    let since = own
        .and_then(|own| library.reader.derived().writers.get(&own))
        .map_or(0, |placed| placed.merged.len());
    let due = since >= COMPACT_AFTER || (closing && since > 0);
    if !due || !library.unsettled.is_empty() || library.writable().is_err() {
        return Flow::Done(library);
    }
    compacted(library).then(|(library, _)| Flow::Done(library))
}

/// Reads every writer's pending records and sorts out this writer's and the
/// orphans. Writes nothing.
fn recover(library: Library) -> Fallible<'static, Library> {
    let writers = library.reader.writers_known().into_iter().collect();
    flow::run(recovery::read(&library.layout, writers)).and_then(move |found| {
        let mut library = library;
        let own = library.writer.as_ref().map(Writer::id);
        let settled = library.closed_records();
        let chains = &library.reader.derived().writers;
        let (assessed, open) = recovery::classify(&library.layout, own, chains, &settled, found);
        flow::run(recovery::predict(
            &library.layout,
            assessed,
            open,
            library.capabilities,
        ))
        .map_ok(move |assessed| {
            library.unsettled = assessed.own;
            library.spent = assessed.spent;
            library.orphaned = assessed.orphaned;
            for orphan in &mut library.orphaned {
                orphan.ours = library.pooled.contains_key(&orphan.writer);
            }
            library.ignored = assessed.ignored;
            library
        })
    })
}

/// Reads every writer's pending records again, for the orphans that stand now, each
/// the install's own and live as the pool and presence now say. Writes nothing.
fn reorphaned(library: &mut Library) -> Fallible<'_, &mut Library> {
    let writers = library.reader.writers_known().into_iter().collect();
    flow::run(recovery::read(&library.layout, writers)).and_then(move |found| {
        let records: BTreeMap<(WriterId, Nonce), PendingRecord> = found
            .iter()
            .flat_map(|(writer, found)| {
                let records = found.records.iter();
                records.map(move |(name, record)| ((*writer, *name), record.clone()))
            })
            .collect();
        let own = library.writer.as_ref().map(Writer::id);
        let settled = library.closed_records();
        let (assessed, _) = recovery::classify(
            &library.layout,
            own,
            &library.reader.derived().writers,
            &settled,
            found,
        );
        let layout = library.layout.clone();
        fold(
            assessed.orphaned.into_iter(),
            Vec::new(),
            move |mut orphaned, mut orphan| {
                let record = &records[&(orphan.writer, orphan.record)];
                recovery::glance(&layout, orphan.record, record).map_ok(move |progress| {
                    orphan.progress = progress;
                    orphaned.push(orphan);
                    orphaned
                })
            },
        )
        .map_ok(move |orphaned| {
            library.orphaned = orphaned;
            for orphan in &mut library.orphaned {
                orphan.ours = library.pooled.contains_key(&orphan.writer);
                let presence = library.presence.get(&orphan.writer);
                orphan.live = presence == Some(&Presence::SameMachine);
            }
            library
        })
    })
}

/// Takes the lock of every other writer of this install's pool that no instance
/// writes as now and that left pending records open, and keeps it, so this
/// instance settles their effects, and no other does. Every other idle writer's lock is let go at once, so another
/// instance may write as it.
fn idle_pool<'a, L: BorrowMut<Library> + 'a>(library: L) -> Fallible<'a, L> {
    let own = library.borrow().writer.as_ref().map(Writer::genesis);
    flow::run(Writer::pool()).and_then(move |pool| {
        let others = pool
            .into_iter()
            .filter(move |genesis| Some(*genesis) != own);
        fold(others, library, |library, genesis| {
            flow::lock(Layout::head(genesis)).and_then(move |lock| match lock {
                Lock::Held => ok(library),
                Lock::Acquired => flow::run(Writer::id_of(genesis)).and_then(move |writer| {
                    let mut library = library;
                    let Some(writer) = writer else {
                        return unlock_idle(genesis).map_ok(move |()| library);
                    };
                    library.borrow_mut().pooled.insert(writer, genesis);
                    release_if_idle(library, writer)
                }),
            })
        })
    })
}

/// Takes up again what idle writers of this install's pool left, as opening does,
/// and asks which writers other instances on this machine hold now: a window may
/// have closed or stopped since. Writes nothing in the folder.
fn repooled(library: &mut Library) -> Fallible<'_, &mut Library> {
    idle_pool(library).and_then(|library| {
        flow::run(library.presence_task()).map_ok(move |presence| {
            library.presence = presence;
            library
        })
    })
}

/// Lets go of the pooled writer `writer` once it holds no open pending record. A
/// record its own log closed, which only the writer removes, holds nothing.
fn release_if_idle<'a, L: BorrowMut<Library> + 'a>(
    library: L,
    writer: WriterId,
) -> Fallible<'a, L> {
    let Some(&genesis) = library.borrow().pooled.get(&writer) else {
        return ok(library);
    };
    let layout = library.borrow().layout.clone();
    flow::run(pending::read_all(&layout, writer)).and_then(move |found| {
        let open = {
            let held = library.borrow();
            let mut names = found.records.iter().map(|(name, _)| name);
            let closed = held.closed_records();
            names.any(|name| !closed.contains(&(writer, *name)))
        };
        if open || !found.unreadable.is_empty() {
            return ok(library);
        }
        let mut library = library;
        library.borrow_mut().pooled.remove(&writer);
        unlock_idle(genesis).map_ok(move |()| library)
    })
}

fn unlock_idle<'a>(genesis: EntryHash) -> Fallible<'a, ()> {
    flow::act(Io::Unlock {
        name: Layout::head(genesis),
    })
}

/// What a refresh scans of the library's files.
enum Look {
    /// The paths whose file facts the entries it read changed.
    Logs,
    /// Those, and these paths.
    Paths(Vec<RelPath>),
    Everything,
}

/// Scans what `scope` asks of the library's files, given `moved`, the paths whose
/// file facts the read changed, and binds. Everything is scanned while a failed
/// scan left something unknown.
fn scan_for<'a>(
    library: &'a mut Library,
    scope: Look,
    mut moved: Vec<RelPath>,
) -> Fallible<'a, &'a mut Library> {
    if let Some(walking) = library.walking.as_mut() {
        walking.forget(&moved);
    }
    let mut paths = match scope {
        _ if !library.unscanned.is_empty() => return rescan(library),
        Look::Everything => return rescan(library),
        Look::Logs => moved,
        Look::Paths(paths) => {
            moved.extend(paths);
            moved
        }
    };
    paths.sort();
    paths.dedup();
    match paths.is_empty() && !library.bind_due {
        true => ok(library),
        false => rescan_paths(library, paths),
    }
}

/// Scans every library file, going on from where a scan that stopped left off,
/// and binds them, what a failed scan left unknown included.
fn rescan<'a, L: BorrowMut<Library> + 'a>(mut library: L) -> Fallible<'a, L> {
    let known = library.borrow_mut();
    known.walking.get_or_insert_with(Walk::default);
    walk(library)
}

/// Lists the directories the walk has left, one at a time, keeping each listing
/// in the library, then binds every file listed.
fn walk<'a, L: BorrowMut<Library> + 'a>(library: L) -> Fallible<'a, L> {
    let next = library.borrow().walking.as_ref().and_then(Walk::next);
    let Some(dir) = next else {
        return walked(library);
    };
    flow::list_stat(Root::Folder, &dir).then(move |listed| {
        let mut library = library;
        let known = library.borrow_mut();
        if let Some(walking) = known.walking.as_mut() {
            match listed {
                Ok(entries) => walking.listed(&known.layout, dir, entries),
                Err(_) => walking.unlistable(dir),
            }
        }
        walk(library)
    })
}

/// Binds every file the finished walk listed.
fn walked<'a, L: BorrowMut<Library> + 'a>(library: L) -> Fallible<'a, L> {
    let known = library.borrow();
    let mut found = known.walking.as_ref().map(Walk::found).unwrap_or_default();
    let names = &*known.env.names;
    let needed = binding::resolve(&mut found, &known.facts, &known.scan, names);
    let identified = binding::identified(found, needed, &known.env.identify);
    identified
        .map_ok(move |read| (library, read))
        .and_then(move |(library, (scan, read))| {
            let mut library = library;
            let known = library.borrow_mut();
            let walk = known.walking.take();
            known.walked = walk.as_ref().map(Walk::dirs);
            let unlisted = walk.map(Walk::unlisted).unwrap_or_default();
            known.listing = Some(listing(&scan));
            if !known.unscanned.is_empty() || !unlisted.is_empty() {
                known.unscanned = Unscanned::default();
                known.unscanned.add(unlisted.clone(), &known.bindings);
                known.bind_due = true;
            }
            known.unlisted = unlisted;
            known.behind.remove(&Lag::Scan);
            known.rebind(scan);
            remember(library, read).then(|(library, kept)| Flow::Done(kept.map(|()| library)))
        })
}

/// The files of `scan`, as the views list them.
fn listing(scan: &Scan) -> Arc<BTreeMap<RelPath, Listed>> {
    let files = scan.files.iter();
    Arc::new(
        files
            .map(|(path, file)| (path.clone(), Listed::of(file)))
            .collect(),
    )
}

/// Keeps in the local root the identities [`Library::remembering`] gives, so a
/// later open need not read them again. Returns the library with how it went.
fn remember<'a, L: BorrowMut<Library> + 'a>(
    library: L,
    read: Vec<RelPath>,
) -> Flow<'a, (L, Result<()>)> {
    let Some(remembered) = library.borrow().remembering(read) else {
        return Flow::Done((library, Ok(())));
    };
    let bytes = remembered.identities();
    let mut library = library;
    library.borrow_mut().remembered = remembered;
    flow::replace(Root::Local, Layout::identities(), bytes).then(move |kept| {
        library.borrow_mut().unkept = kept.is_err();
        Flow::Done((library, kept))
    })
}

/// Once entries are durable, scans again only `paths`, and binds.
fn rescan_paths(library: &mut Library, paths: Vec<RelPath>) -> Fallible<'_, &mut Library> {
    let listed = binding::relist(&library.layout, &library.scan, paths.clone()).task();
    rebind_logged(library, listed, paths)
}

/// After a scan of `paths` failed once entries were durable: they, and the
/// entities bound there, are unknown until a scan of every file succeeds.
fn unscanned(
    library: &mut Library,
    paths: Vec<RelPath>,
    error: Error,
) -> Fallible<'_, &mut Library> {
    library.unscanned.add(paths, &library.bindings);
    library.lag(Lag::Scan, &Err(error));
    library.bind_due = true;
    ok(library)
}

/// Once entries are durable, binds to what `scan` finds. A scan that fails does
/// not fail what was logged: `unsure`, where files may have moved, and the
/// entities bound there are unknown until a scan of every file succeeds.
/// Identities it fails to keep in the local root are kept after the next scan.
fn rebind_logged<'a>(
    library: &'a mut Library,
    listed: Task<'static, Result<Scan>>,
    unsure: Vec<RelPath>,
) -> Fallible<'a, &'a mut Library> {
    if let Some(walking) = library.walking.as_mut() {
        walking.forget(&unsure);
    }
    flow::run(listed).then(move |listed| {
        let identified = match listed {
            Ok(mut scan) => {
                let names = &*library.env.names;
                let unknown = binding::resolve(&mut scan, &library.facts, &library.scan, names);
                binding::identified(scan, unknown, &library.env.identify)
            }
            Err(error) => Flow::Done(Err(error)),
        };
        identified.then(move |scanned| match scanned {
            Ok((scan, read)) => {
                library.rebind(scan);
                remember(library, read).then(|(library, _)| ok(library))
            }
            Err(error) => unscanned(library, unsure, error),
        })
    })
}

/// Once the facts changed, reads the identities they now need of the files the
/// last scan found, and binds.
fn reidentify(library: &mut Library) -> Fallible<'_, &mut Library> {
    match library.bind_due {
        true => rescan_paths(library, Vec::new()),
        false => ok(library),
    }
}

/// The library paths `steps` move files from and to.
fn moved_paths(steps: &[EffectStep]) -> Vec<RelPath> {
    let moves = steps.iter().filter(|step| {
        !matches!(
            step,
            EffectStep::MakeDir { .. } | EffectStep::RemoveDir { .. }
        )
    });
    moves.flat_map(EffectStep::library_paths).cloned().collect()
}

/// Removes this writer's spent records, then settles its interrupted effects, so
/// that what is planned next is planned against the facts and history they leave.
/// A spent record that cannot be removed fails the write.
fn settle_first(library: &mut Library) -> Fallible<'_, &mut Library> {
    remove_spent(library).then(
        |(library, removed)| match (removed, library.unsettled.is_empty()) {
            (Err(error), _) => Flow::Done(Err(error)),
            (Ok(()), true) => ok(library),
            (Ok(()), false) => settle_own(library),
        },
    )
}

/// Commits `resolved` once this writer's interrupted effects are settled: creates
/// the writer if there is none, removes stale staging, then logs the intent and
/// carries out its file effects.
fn commit<'a>(
    library: &'a mut Library,
    resolved: Resolved,
) -> Fallible<'a, (&'a mut Library, Committed)> {
    let Resolved {
        label,
        created,
        facts,
        effects,
        reverses,
    } = resolved;
    let shown = label.clone();
    ensure_writer(library, &effects)
        .and_then(tidy_staging)
        .and_then(move |library| {
            let filed: BTreeSet<EntityId> = facts.iter().filter_map(file_of).collect();
            let pins = library.pins(&effects).into_iter();
            let pins = pins.filter(|op| file_of(op).is_none_or(|e| !filed.contains(&e)));
            let logged = Logged {
                label,
                forks: library.known_forks(&facts),
                ops: facts,
                displaced: Vec::new(),
                record: None,
            };
            library.reversing = reverses;
            transact(library, logged, effects, None, pins.collect())
        })
        .and_then(move |(library, entries, ended)| {
            library.reversing = None;
            let Ended { outcome, left } = ended;
            match committed(shown, &entries, created, outcome, library.local()) {
                Ok(committed) => ok((library, Committed { left, ..committed })),
                Err(partial) => Flow::Done(Err(Error::Partial(partial))),
            }
        })
}

/// What appending `entries`, an intent labeled `label` and what followed it, did:
/// the intent, or how its effects stopped partway.
fn committed(
    label: String,
    entries: &[Entry],
    created: Vec<EntityId>,
    outcome: Outcome,
    local: Local,
) -> std::result::Result<Committed, Box<Partial>> {
    let kinds: Vec<EntryKind> = entries.iter().map(Entry::kind).collect();
    let ops = kinds.iter().flat_map(|kind| match kind {
        EntryKind::Intent(Logged { ops, .. }) | EntryKind::Bind(Bound { ops }) => ops.as_slice(),
        EntryKind::Genesis(_) | EntryKind::Settle(_) | EntryKind::Unknown(_) => &[],
    });
    let ops: Vec<&Op> = ops.collect();
    let paths = ops.iter().filter_map(|op| match op {
        Op::Path { from, file } => Some(PathChange {
            from: from.clone(),
            to: file.as_ref().map(|file| file.path.clone()),
        }),
        _ => None,
    });
    let trees = ops.iter().filter_map(|op| match op {
        Op::Tree { from, to } => Some((from.clone(), to.clone())),
        _ => None,
    });
    let undoable = match kinds.first() {
        Some(EntryKind::Intent(logged)) => undo::joins(logged.ops.iter()),
        _ => false,
    };
    let committed = Committed {
        intent: entries[0].hash(),
        undoable,
        created,
        changes: distinct(ops.iter().copied().filter_map(change_of)),
        paths: paths.collect(),
        trees: trees.collect(),
        local,
        left: Vec::new(),
    };
    match outcome {
        Outcome::Complete => Ok(committed),
        Outcome::Partial(report) => {
            let landed = ops.iter().filter_map(|op| match op {
                Op::File {
                    file: Some(file), ..
                }
                | Op::Pin { file, .. }
                | Op::Path {
                    file: Some(file), ..
                } => Some(file.path.clone()),
                Op::Tree { to, .. } => Some(to.clone()),
                Op::Dir { path, made: true } => Some(path.clone()),
                _ => None,
            });
            Err(Box::new(Partial {
                label,
                committed,
                report: PartialReport {
                    record: None,
                    landed: landed.collect(),
                    ..report
                },
            }))
        }
    }
}

/// The entity whose file `op` writes, if it writes one.
fn file_of(op: &Op) -> Option<EntityId> {
    match op {
        Op::File { entity, .. } | Op::Pin { entity, .. } => Some(*entity),
        _ => None,
    }
}

/// The genesis entry of the writer whose history `placed` is.
fn genesis_of(placed: &derive::Placed) -> Option<EntryHash> {
    let tip = placed.tips.first().map(|tip| tip.genesis);
    tip.or_else(|| {
        let first = placed.merged.first().filter(|entry| entry.n() == 0);
        first.map(|entry| entry.hash())
    })
}

/// Creates this instance's writer when it has none. Its genesis entry is the
/// first thing it writes in the folder, so the effects' preconditions are checked
/// first: a refused intent leaves nothing behind.
fn ensure_writer<'a>(
    library: &'a mut Library,
    effects: &EffectPlan,
) -> Fallible<'a, &'a mut Library> {
    if library.writer.is_some() {
        return ok(library);
    }
    let id = library.env.writer_id();
    let check = match effects.is_empty() {
        true => Task::ready(Ok(None)),
        false => effects::check(&library.layout, id, effects, &library.env.identify),
    };
    flow::run(check).and_then(move |refused| {
        if let Some(refusal) = refused {
            return Flow::Done(Err(Error::Refused(refusal)));
        }
        let at = match library.tick() {
            Ok(at) => at,
            Err(error) => return Flow::Done(Err(error)),
        };
        let label = library.env.label.clone();
        let kind = EntryKind::Genesis(Genesis {
            writer: id,
            label: label.clone(),
        });
        let Ok(genesis) = Entry::encode(EntryHash::ZERO, 0, at, kind) else {
            return Flow::Done(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong))));
        };
        let segment = library.env.segment_name();
        let create = Writer::create(library.layout.clone(), id, segment, label, at);
        flow::run(create).and_then(move |writer| {
            writer.stamp().then(move |written| {
                library.writer = Some(writer);
                library.absorb_own(&[genesis], written);
                ok(library)
            })
        })
    })
}

/// Settles this writer's interrupted effects, oldest first, and shows what they
/// leave. A record stays to settle while settling it fails, and is removed once
/// its entry is durable. One whose effects stop partway is logged as far as they got,
/// and fails the settling with [`Error::Unfinished`].
fn settle_own(library: &mut Library) -> Fallible<'_, &mut Library> {
    let open = library.unsettled.clone();
    flow::fold(open.into_iter(), library, settle_one).map_ok(|library| {
        library.show();
        library
    })
}

fn settle_one(library: &mut Library, settling: Settling) -> Fallible<'_, &mut Library> {
    let name = settling.record;
    let record = Rc::new(settling.pending);
    let identify = Rc::clone(&library.env.identify);
    let settled = recovery::settle(
        &library.layout,
        name,
        Rc::clone(&record),
        identify,
        library.capabilities,
    );
    let own = library.writer.as_ref().map(Writer::id);
    let closed = own.is_some_and(|own| library.closed_records().contains(&(own, name)));
    flow::run(settled)
        .and_then(move |applied| {
            let mut planned = planned(&record);
            planned.record = Some(name);
            let file_ops = library.file_ops(&applied, &record.files);
            let label = planned.label.clone();
            let resumed = closed;
            let kinds = match closed {
                true => Vec::new(),
                false => {
                    planned.ops.extend(file_ops);
                    planned.displaced = applied.displaced;
                    planned.forks = library.known_forks(&planned.ops);
                    vec![EntryKind::Intent(planned)]
                }
            };
            let appended = match kinds.is_empty() {
                true => ok((library, Vec::new())),
                false => append(library, kinds),
            };
            appended.and_then(move |(library, entries)| {
                remove_record(library, name, &record).then(move |library| {
                    let scan = library.scan_task();
                    rebind_logged(library, scan, record.paths())
                        .map_ok(move |library| (library, label, entries, applied.outcome, resumed))
                })
            })
        })
        .and_then(|(library, label, entries, outcome, resumed)| {
            if resumed {
                library.show();
                return ok(library);
            }
            match committed(label, &entries, Vec::new(), outcome, library.local()) {
                Ok(_) => ok(library),
                Err(partial) => {
                    library.show();
                    Flow::Done(Err(Error::Unfinished(partial)))
                }
            }
        })
}

/// Removes this writer's record `name` once its entries are durable, with the
/// others its entries closed. One that stays is removed before the next write.
/// A writer that retired meanwhile leaves it where it is, since nothing writes a
/// retired writer's directory again.
fn remove_record<'a>(
    library: &'a mut Library,
    name: Nonce,
    record: &PendingRecord,
) -> Flow<'a, &'a mut Library> {
    library.unsettled.retain(|settling| settling.record != name);
    if library.writer.as_ref().map(Writer::id) != Some(record.writer) {
        return Flow::Done(library);
    }
    library.spent.push((name, record.clone()));
    remove_spent(library).then(|(library, _)| Flow::Done(library))
}

/// Removes this writer's records its entries closed that are still in the
/// folder, each after its markers. One whose removal fails stays to try again,
/// and the first failure is returned once the others are tried.
fn remove_spent(library: &mut Library) -> Flow<'_, (&mut Library, Result<()>)> {
    let spent = std::mem::take(&mut library.spent);
    flow::fold(
        spent.into_iter(),
        (library, Ok(())),
        |(library, failed), (name, record)| {
            let removing = effects::finish(&library.layout, name, &record);
            flow::run(removing).then(move |removed| {
                if removed.is_err() {
                    library.spent.push((name, record));
                }
                flow::ok((library, failed.and(removed)))
            })
        },
    )
    .then(|folded| Flow::Done(folded.expect("removing a record fails nothing else")))
}

/// Removes what a run cut short before its steps left: staged files no record
/// places, and this writer's empty records. Runs once settling has left no
/// record open.
fn tidy_staging(library: &mut Library) -> Fallible<'_, &mut Library> {
    let Some(writer) = library.writer.as_ref().map(Writer::id) else {
        return ok(library);
    };
    let layout = library.layout.clone();
    let ignored = library.ignored.clone();
    let capabilities = library.capabilities;
    flow::run(recovery::tidy(&layout, writer))
        .and_then(move |()| {
            flow::run(recovery::remove_empty(
                &layout,
                writer,
                &ignored,
                capabilities,
            ))
        })
        .map_ok(move |removed| {
            library.ignored.retain(|path| !removed.contains(path));
            library
        })
}

/// The intent a pending record planned to log, without the file ops its steps
/// decide. A record whose entry is not an intent logs its label alone.
fn planned(record: &PendingRecord) -> Logged {
    let decoded = Entry::decode(record.entry.clone())
        .ok()
        .map(|entry| entry.kind());
    let mut logged = match decoded {
        Some(EntryKind::Intent(logged)) => logged,
        _ => Logged {
            label: record.label.clone(),
            ops: Vec::new(),
            displaced: Vec::new(),
            forks: Vec::new(),
            record: None,
        },
    };
    logged.ops.retain(|op| {
        !matches!(
            op,
            Op::File { .. } | Op::Path { .. } | Op::Tree { .. } | Op::Dir { .. }
        )
    });
    logged.displaced.clear();
    logged
}

/// Appends `kinds` as this writer's next entries and folds them in.
fn append<'a>(
    library: &'a mut Library,
    kinds: Vec<EntryKind>,
) -> Fallible<'a, (&'a mut Library, Vec<Entry>)> {
    appending(library, kinds).then(|(library, appended)| match appended {
        Ok(entries) => ok((library, entries)),
        Err(error) => Flow::Done(Err(error)),
    })
}

/// As [`append`], with the library back whatever happened. Once the entries are
/// durable in the folder, they are shown and kept in the history, and the head
/// recorded. Recording it failing does not fail the append; see [`keep_local`].
/// A writer that cannot continue its history stops writing: the next commit
/// creates another.
fn appending<'a>(
    library: &'a mut Library,
    kinds: Vec<EntryKind>,
) -> Flow<'a, (&'a mut Library, Result<Vec<Entry>>)> {
    let mut stamped = Vec::with_capacity(kinds.len());
    for kind in kinds {
        match library.tick() {
            Ok(at) => stamped.push((at, kind)),
            Err(error) => return Flow::Done((library, Err(error))),
        }
    }
    let writer = library.writer.take().expect("a writer appends");
    let fresh = library.env.segment_name();
    flow::run(writer.append(stamped, fresh)).then(move |(writer, appended)| {
        library.writer = Some(writer);
        match appended {
            Ok(entries) => {
                let writer = library.writer.as_ref().expect("put back above");
                writer.stamp().then(move |written| {
                    library.absorb_own(&entries, written);
                    keep_local(library, true)
                        .then(|(library, _)| compact_due(library, false))
                        .then(move |library| Flow::Done((library, Ok(entries))))
                })
            }
            Err(error @ Error::Rekey { .. }) => {
                let stopped = library.stop_writing();
                stopped.then(move |stopped| Flow::Done((library, stopped.and(Err(error)))))
            }
            Err(error) => Flow::Done((library, Err(error))),
        }
    })
}

/// Records the head when `head` asks or when it lags. A failure leaves it behind,
/// to record again the next time. A crash meanwhile loses nothing the folder
/// holds: reopening reads the folder and continues from the last entry of the
/// writer's chain there.
fn keep_local(library: &mut Library, head: bool) -> Flow<'_, (&mut Library, Result<()>)> {
    let due = head || library.behind.contains_key(&Lag::Head);
    let recorded = match &library.writer {
        Some(writer) if due => writer.record(),
        _ => Task::ready(Ok(())),
    };
    flow::run(recorded).then(move |recorded| {
        library.lag(Lag::Head, &recorded);
        Flow::Done((library, recorded))
    })
}

/// What settling another writer's record adds once the settlement's effects
/// complete: the facts the record planned, ahead of the intent's own ops, then
/// the entry that closes the record.
struct Closing {
    facts: Vec<Op>,
    settle: Settle,
}

/// The most bytes the ops of one `bind` entry take, well within a line, so that a
/// commit pins any number of moves.
const BIND_BYTES: usize = 1 << 20;

/// The `bind` entries that pin `pins`.
fn bound(pins: Vec<Op>) -> Vec<EntryKind> {
    let mut kinds = Vec::new();
    let (mut ops, mut size) = (Vec::new(), 0);
    for op in pins {
        let len = serde_json::to_string(&op).map_or(0, |json| json.len()) + 1;
        if size + len > BIND_BYTES && !ops.is_empty() {
            kinds.push(EntryKind::Bind(Bound {
                ops: std::mem::take(&mut ops),
            }));
            size = 0;
        }
        size += len;
        ops.push(op);
    }
    if !ops.is_empty() {
        kinds.push(EntryKind::Bind(Bound { ops }));
    }
    kinds
}

/// The entries that log `logged`, closed by `closing` when there is one.
fn closed(logged: Logged, closing: Option<Closing>) -> Vec<EntryKind> {
    let (logged, settle) = close(logged, closing);
    let settle = settle.map(EntryKind::Settle);
    std::iter::once(EntryKind::Intent(logged))
        .chain(settle)
        .collect()
}

/// `logged` with the facts `closing` plans first, and the settlement that
/// closes it.
fn close(mut logged: Logged, closing: Option<Closing>) -> (Logged, Option<Settle>) {
    let Some(Closing { mut facts, settle }) = closing else {
        return (logged, None);
    };
    facts.append(&mut logged.ops);
    logged.ops = facts;
    (logged, Some(settle))
}

/// How a commit's effects ended: whether they all ran, and the directories the
/// folder still holds that they were to remove.
struct Ended {
    outcome: Outcome,
    left: Vec<RelPath>,
}

/// Logs `logged`, carrying out `effects` under a pending record when there are
/// any, closed by `closing` only when they complete, and followed by `bind`
/// entries pinning `pins`. Returns the entries, the intent's first, and how the
/// effects ended. An interrupted run leaves the record for the next write to
/// settle.
fn transact<'a>(
    library: &'a mut Library,
    logged: Logged,
    effects: Rc<EffectPlan>,
    closing: Option<Closing>,
    pins: Vec<Op>,
) -> Fallible<'a, (&'a mut Library, Vec<Entry>, Ended)> {
    if effects.is_empty() {
        let mut kinds = closed(logged, closing);
        kinds.extend(bound(pins));
        return append(library, kinds)
            .and_then(|(library, entries)| {
                reidentify(library).map_ok(move |library| (library, entries))
            })
            .map_ok(|(library, entries)| {
                library.show();
                let ended = Ended {
                    outcome: Outcome::Complete,
                    left: Vec::new(),
                };
                (library, entries, ended)
            });
    }
    let mut logged = logged;
    logged.record = effects.moves_files().then_some(effects.record);
    carry_out(library, &logged, Rc::clone(&effects)).and_then(move |(library, record, applied)| {
        log_effects(library, logged, &effects, record, applied, closing, pins)
    })
}

/// Stages, checks and journals `effects` for the intent `logged`, then carries
/// them out. A refusal changes nothing; a failure once the record is written
/// leaves it to settle, and one before leaves nothing to settle.
fn carry_out<'a>(
    library: &'a mut Library,
    logged: &Logged,
    effects: Rc<EffectPlan>,
) -> Fallible<'a, (&'a mut Library, Rc<PendingRecord>, Applied)> {
    let writer = library.writer.as_ref().expect("a writer commits");
    let (id, head) = (writer.id(), writer.head());
    let at = match library.tick() {
        Ok(at) => at,
        Err(error) => return Flow::Done(Err(error)),
    };
    let encoded = head
        .n
        .checked_add(1)
        .and_then(|n| Entry::encode(head.hash, n, at, EntryKind::Intent(logged.clone())).ok());
    let latest = Hlc {
        wall_ms: u64::MAX,
        counter: u32::MAX,
    };
    let widest = EntryKind::Intent(library.widest(logged, &effects));
    let widest = Entry::encode(head.hash, u64::MAX, latest, widest);
    let (Some(entry), Ok(_)) = (encoded, widest) else {
        return Flow::Done(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong))));
    };
    let record = Rc::new(PendingRecord::new(
        id,
        &logged.label,
        entry.line(),
        &effects,
    ));
    let (name, journaled) = (effects.record, effects.moves_files());
    if journaled && record.encode().len() as u64 > pending::MAX_RECORD {
        return Flow::Done(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong))));
    }
    let layout = library.layout.clone();
    let identify = Rc::clone(&library.env.identify);
    let prepared = effects::prepare(&layout, effects, Rc::clone(&record), Rc::clone(&identify));
    flow::run(prepared).then(move |prepared| match prepared {
        Ok(Ok(())) => {
            let applied = effects::apply(&layout, name, Rc::clone(&record), 0, identify);
            flow::run(applied).then(move |applied| match applied {
                Ok(applied) => ok((library, record, applied)),
                Err(error) => {
                    if journaled {
                        library.interrupted(name, &record);
                    }
                    Flow::Done(Err(error))
                }
            })
        }
        Ok(Err(refusal)) => Flow::Done(Err(Error::Refused(refusal))),
        Err(error) if journaled => {
            let path = layout.pending(id, name);
            flow::stat(Root::Folder, &path).then(move |written| {
                if let Ok(Some(_)) = written {
                    library.interrupted(name, &record);
                }
                Flow::Done(Err(error))
            })
        }
        Err(error) => Flow::Done(Err(error)),
    })
}

/// Appends the intent `logged` with what `applied` says the effects did, closed
/// by `closing` if they complete and followed by `bind` entries pinning `pins`,
/// removes the record once the entries are durable, and scans again the paths
/// the effects moved files from and to. Once the entries are
/// durable nothing fails.
fn log_effects<'a>(
    library: &'a mut Library,
    logged: Logged,
    effects: &EffectPlan,
    record: Rc<PendingRecord>,
    applied: Applied,
    closing: Option<Closing>,
    pins: Vec<Op>,
) -> Fallible<'a, (&'a mut Library, Vec<Entry>, Ended)> {
    let (name, journaled) = (effects.record, effects.moves_files());
    let file_ops = library.file_ops(&applied, &record.files);
    let outcome = applied.outcome;
    let closing = closing.filter(|_| outcome == Outcome::Complete);
    let (mut logged, settle) = close(logged, closing);
    logged.ops.extend(file_ops);
    logged.displaced = applied.displaced;
    logged.forks = library.known_forks(&logged.ops);
    let mut kinds = vec![EntryKind::Intent(logged)];
    kinds.extend(settle.map(EntryKind::Settle));
    kinds.extend(bound(pins));
    let touched = moved_paths(&effects.steps);
    appending(library, kinds).then(move |(library, appended)| {
        let retired = library.writer.as_ref().map(Writer::id) != Some(record.writer);
        match (journaled, appended.is_ok(), retired) {
            (true, false, false) => library.interrupted(name, &record),
            (true, false, true) => library.orphan(name, &record, true),
            (false, _, _) | (true, true, _) => {}
        }
        let entries = match appended {
            Ok(entries) => entries,
            Err(error) => return Flow::Done(Err(error)),
        };
        let removed = match journaled {
            true => remove_record(library, name, &record),
            false => Flow::Done(library),
        };
        removed.then(move |library| {
            rescan_paths(library, touched).map_ok(move |library| {
                library.show();
                let ended = Ended {
                    outcome,
                    left: applied.left,
                };
                (library, entries, ended)
            })
        })
    })
}

/// Settles another writer's record `theirs` as `how` with `plan`, once this
/// writer's own interrupted effects are settled: removes stale staging, then logs
/// the settlement. Finishing logs the facts the other writer planned with it.
/// Effects that stop partway are logged as far as they got, without those facts
/// or the `settle` entry, and leave the record open to settle again.
fn settle_orphan(
    library: &mut Library,
    orphan: Orphan,
    theirs: PendingRecord,
    how: Settlement,
    plan: EffectPlan,
) -> Fallible<'_, Committed> {
    let effects = Rc::new(plan);
    let facts = match how {
        Settlement::Finished => planned(&theirs).ops,
        Settlement::RolledBack | Settlement::Dismissed => Vec::new(),
    };
    let settle = Settle {
        writer: orphan.writer,
        record: orphan.record,
        outcome: how,
    };
    let label = theirs.label.clone();
    ensure_writer(library, &effects)
        .and_then(tidy_staging)
        .and_then(
            move |library| match effects.is_empty() && facts.is_empty() {
                true => append(library, vec![EntryKind::Settle(settle)])
                    .map_ok(|(library, entries)| (library, entries, Outcome::Complete)),
                false => {
                    let logged = Logged {
                        label: theirs.label,
                        ops: Vec::new(),
                        displaced: Vec::new(),
                        forks: Vec::new(),
                        record: None,
                    };
                    let closing = Closing { facts, settle };
                    transact(library, logged, effects, Some(closing), Vec::new())
                        .map_ok(|(library, entries, ended)| (library, entries, ended.outcome))
                }
            },
        )
        .and_then(move |(library, entries, outcome)| {
            library.show();
            match committed(label, &entries, Vec::new(), outcome, library.local()) {
                Err(partial) => Flow::Done(Err(Error::Partial(partial))),
                Ok(committed) => {
                    library
                        .orphaned
                        .retain(|o| (o.writer, o.record) != (orphan.writer, orphan.record));
                    release_if_idle(library, orphan.writer).map_ok(move |_| committed)
                }
            }
        })
}
