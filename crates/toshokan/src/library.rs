//! The library as one instance holds it: the pure core both drivers run.
//!
//! Only [`Library::commit`] (and undo, redo and settling, which commit),
//! [`Library::empty_trash`], [`Library::tidy`], [`Library::compact`] and
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

use crate::binding::{
    self, Binding, Bindings, Fact, Facts, Resolving, Scan, Scanned, Unscanned, Walk,
};
use crate::cache::{self, Checkpoint, Encoding};
use crate::contribution;
use crate::derive::{self, Placed};
use crate::drafts::{self, DraftRecord};
use crate::effects::{self, Applied, EffectPlan, EffectStep, FileEnd};
use crate::env::Env;
use crate::error::{Error, Invalid, Refusal, Result, Why};
use crate::flow::{self, fold, ok, Fallible, Flow};
use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::index::{Follow, Index, Indexing};
use crate::intent;
use crate::io::{Capabilities, Io, Kind, Lock, Root, Task};
use crate::layout::Layout;
use crate::line::MAX_LINE;
use crate::log::{Bound, Entry, EntryKind, Forked, Genesis, Logged, Op, Settle, Settlement};
use crate::merge::{Folded, Part};
use crate::path::RelPath;
use crate::pending::{self, PendingRecord};
use crate::plan::{FactChange, FileChange, Plan};
use crate::reach::{self, Linking, Links, Reach};
use crate::reader::{Listing, ReadReport, Reader, Stamp};
use crate::recovery::{self, Settling};
use crate::report::{
    By, Change, Committed, Compacted, DraftState, DraftStatus, Emptied, Fork, Gap, HistoryItem,
    Item, ItemChange, Lag, Local, Mode, Opened, Orphan, Outcome, Partial, PartialReport, Presence,
    Refreshed, Settled, Start, Tidied, TrashItem, What, WriterInfo,
};
use crate::schema::Schema;
use crate::trash::{self, Policy};
use crate::undo::{Before, History};
use crate::view::{FileRef, FileState, Parts, View};
use crate::writer::{self, Claimed, Picked, Writer};

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
    /// This writer's records whose entries are logged, left in the folder until
    /// [`Library::tidy`] or [`Library::close`] removes them.
    spent: BTreeMap<Nonce, PendingRecord>,
    orphaned: Vec<Orphan>,
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
    /// The identities scans read that no fact gives, as the local root should keep
    /// them.
    remembered: Scan,
    /// Whether the local root lacks `remembered`, since writing it failed.
    unkept: bool,
    /// What a failed scan left unknown, until a scan of every file succeeds.
    unscanned: Unscanned,
    /// A scan of every file that stopped before it finished, to go on with.
    walking: Option<Walk>,
    /// Bound from `facts`, `scan` and `unscanned` unless `reach` says a binding
    /// is due.
    bindings: Arc<Bindings>,
    /// What changed in `facts` and `scan` since they were bound and resolved.
    reach: Reach,
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
    /// Claims a writer from the pool, starts from the cached views of every writer
    /// of the pool joined, reads the folder, assesses recovery and checks drafts.
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
                let own = picked.as_ref().map(Picked::genesis);
                flow::run(Writer::pool())
                    .and_then(move |pool| flow::run(cache::load(pool, own)))
                    .map_ok(move |cached| (picked, cached))
            })
            .and_then(|(picked, cached)| {
                flow::read_replaced(Root::Local, Layout::identities()).map_ok(move |bytes| {
                    let remembered = bytes.and_then(|bytes| Scan::of_identities(&bytes));
                    (picked, cached, remembered.unwrap_or_default())
                })
            })
            .and_then(move |(picked, cached, remembered)| {
                let reader = Reader::open(layout.clone(), cached);
                flow::run(reader.list()).and_then(move |listing| {
                    absorbed(reader, listing).then(move |(reader, report)| {
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
            })
            .and_then(|(mut library, report, start)| {
                library.grace(true);
                fold_opened(library).then(move |library| ok((library, report, start)))
            })
            .and_then(|(library, report, start)| {
                saved_view(library).then(move |(library, saved)| match saved {
                    Ok(()) => recover(library).map_ok(move |library| (library, report, start)),
                    Err(error) => Flow::Done(Err(error)),
                })
            })
            .and_then(|(library, report, start)| {
                presumed_bound(library).then(move |library| ok((library, report, start)))
            })
            .and_then(|(library, report, start)| {
                flow::run(library.presence_task()).and_then(move |presence| {
                    let mut library = library;
                    library.presence = presence;
                    indexed(library).then(move |library| ok((library, report, start)))
                })
            })
            .and_then(|(library, report, start)| {
                flow::run(library.drafts_task()).map_ok(move |drafts| {
                    let opened = Opened {
                        mode: library.mode.clone(),
                        no_replace: library.capabilities.no_replace,
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
                        drafts,
                        forks: report.forks,
                        gaps: report.gaps,
                        unreadable: report
                            .unreadable
                            .into_iter()
                            .map(|(path, _)| path)
                            .collect(),
                        ignored: library.ignored.clone(),
                    };
                    (library, opened)
                })
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
            spent: BTreeMap::new(),
            orphaned: Vec::new(),
            ignored: Vec::new(),
            folded: Folded::default(),
            ahead: BTreeSet::new(),
            facts: Facts::new(),
            scan: remembered.clone(),
            remembered,
            unkept: false,
            unscanned: Unscanned::default(),
            walking: None,
            bindings: Arc::default(),
            reach: Reach::default(),
            pins: Vec::new(),
            presence: BTreeMap::new(),
            history: History::default(),
            reversing: None,
            unshown: false,
            view: View::new(Parts {
                folded: Folded::default(),
                bindings: Arc::default(),
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
        let full = matches!(scope, Look::Everything);
        flow::run(listed)
            .and_then(move |listing| {
                let mut report = self.reader.absorb(listing);
                report.changed |= self.grace(full);
                let moved = self.absorb_read(&report);
                self.unshown |= report.anything_new();
                let forked = self
                    .writer
                    .as_ref()
                    .is_some_and(|own| report.forks.iter().any(|fork| fork.writer == own.id()));
                keep_local(self, false)
                    .then(move |(library, kept)| match (kept, forked) {
                        (Err(error), _) => Flow::Done(Err(error)),
                        (Ok(()), true) => library.stop_writing().map_ok(move |()| library),
                        (Ok(()), false) => ok(library),
                    })
                    .and_then(move |library| scan_for(library, scope, moved))
            })
            .and_then(|library| {
                let changes = library.since_view();
                reshown(library).map_ok(move |library| Refreshed {
                    changes,
                    scan: library.bindings.report.clone(),
                })
            })
            .task()
    }

    /// Takes the grace step once it is due, after a read that scanned every
    /// file when `full`; whether it took it.
    fn grace(&mut self, full: bool) -> bool {
        let due = self.reader.note_grace(self.env.now_ms(), full);
        if due {
            self.reader.take_grace();
        }
        due
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
        for pair in self.bindings.bound.diff(&before.bound) {
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

    /// The bytes this writer displaced that are still in its trash, oldest first.
    pub fn trash(&mut self) -> Task<'_, Result<Vec<TrashItem>>> {
        match &self.writer {
            Some(writer) => trash::list(&self.layout, writer.id(), self.folded.trash(writer.id())),
            None => Task::ready(Ok(Vec::new())),
        }
    }

    /// Removes from this writer's trash what `policy` does not keep: the only way
    /// bytes leave the folder.
    pub fn empty_trash(&mut self, policy: Policy) -> Task<'_, Result<Emptied>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        let Some(writer) = self.writer.as_ref().map(Writer::id) else {
            return Task::ready(Ok(Emptied::default()));
        };
        let listed = trash::list(&self.layout, writer, self.folded.trash(writer));
        flow::run(listed)
            .and_then(move |items| {
                let now = self.env.now_ms();
                flow::run(trash::empty(&self.layout, writer, items, policy, now))
            })
            .task()
    }

    /// Folds this writer's own entries into a snapshot and deletes the sealed
    /// segments of its directory the snapshot certifies. A writer also compacts
    /// at close and every [`COMPACT_AFTER`] entries. Refused with
    /// [`Refusal::Nothing`] when this instance writes as no writer yet, and when
    /// a snapshot in the writer's directory already has its last entry.
    pub fn compact(&mut self) -> Task<'_, Result<Compacted>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        if self.writer.is_none() {
            return Task::ready(Err(Error::Refused(Refusal::Nothing)));
        }
        compacted(self)
            .then(|(_, compacted)| Flow::Done(compacted))
            .task()
    }

    /// Keeps an unsaved edit of `entity` over a file holding `base`, in the local
    /// root only. Fails with [`Error::NoWriter`] before this instance's first
    /// commit, when it has no local directory to keep it in.
    pub fn put_draft(
        &mut self,
        entity: EntityId,
        base: Identity,
        bytes: Vec<u8>,
    ) -> Task<'_, Result<()>> {
        match &self.writer {
            Some(writer) => drafts::put(writer.genesis(), entity, &DraftRecord { base, bytes }),
            None => Task::ready(Err(Error::NoWriter)),
        }
    }

    pub fn discard_draft(&mut self, entity: EntityId) -> Task<'_, Result<()>> {
        match &self.writer {
            Some(writer) => drafts::discard(writer.genesis(), entity),
            None => Task::ready(Ok(())),
        }
    }

    /// Removes this writer's spent pending records: those of its intents whose
    /// entries are logged, which commits leave in the folder. An app tidies when
    /// it is idle, and [`Library::close`] removes what is left. Every record is
    /// tried; one whose removal fails stays to try again, and the first failure is
    /// returned once the others are tried. An app may drop the task to commit, and
    /// what it removed stays removed.
    pub fn tidy(&mut self) -> Task<'_, Result<Tidied>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        tidied(self).then(|(_, tidied)| Flow::Done(tidied)).task()
    }

    /// Compacts the writer of a writable library, writes the cached view and
    /// what else lags, removes the spent pending records of a writable library,
    /// seals the open segment and releases the writer's lock, then lets go of
    /// what the library holds a part at a time: nothing is left to view, and the
    /// library only drops. A record it cannot
    /// remove stays for the next instance to find, and that failure is returned
    /// once the library is closed. A library dropped without closing leaves its
    /// segment open, as a crash does.
    pub fn close(&mut self) -> Task<'_, Result<()>> {
        let Some(writer) = self.writer.take() else {
            return dismantled(self).then(|()| ok(())).task();
        };
        self.writer = Some(writer);
        compact_due(self, true)
            .then(|library| keep_local(library, false))
            .then(|(library, kept)| match (kept, library.writable()) {
                (Err(error), _) => Flow::Done(Err(error)),
                (Ok(()), Ok(())) => tidied(library).then(|(library, tidied)| ok((library, tidied))),
                (Ok(()), Err(_)) => ok((library, Ok(Tidied::default()))),
            })
            .and_then(|(library, tidied)| match library.writer.take() {
                Some(writer) => flow::run(writer.close()).map_ok(move |()| (library, tidied)),
                None => ok((library, tidied)),
            })
            .and_then(|(library, tidied)| {
                dismantled(library).then(|()| Flow::Done(tidied.map(drop)))
            })
            .task()
    }

    /// Lets go of part `part` of what the library holds, the largest first;
    /// whether none is left.
    fn shed(&mut self, part: usize) -> bool {
        match part {
            0 => drop(std::mem::replace(
                &mut self.view,
                View::new(Parts::default()),
            )),
            1 => drop(std::mem::take(&mut self.folded)),
            2 => self.reader.shed_logs(),
            3 => self.reader.shed_files(),
            _ => {
                drop(std::mem::take(&mut self.facts));
                drop(std::mem::take(&mut self.scan));
                drop(std::mem::take(&mut self.reach));
                drop(std::mem::take(&mut self.bindings));
                drop(std::mem::take(&mut self.history));
                return true;
            }
        }
        false
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
            && !recovery::Chain::continues(placed, theirs.after())
            && !self.folded.settled(theirs.writer, name)
    }

    fn writable(&self) -> Result<()> {
        match &self.mode {
            Mode::Writable => Ok(()),
            Mode::ReadOnly(why) => Err(Error::ReadOnly(why.clone())),
        }
    }

    /// Checks `plan` against the view and turns its facts into ops and its file
    /// changes into effects.
    fn resolve(&mut self, plan: Plan) -> Result<Resolved> {
        let refused = |invalid| Error::Refused(Refusal::Invalid(invalid));
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

    /// The file ops logging what the effects, whose ends are `ends`, did to each
    /// entity's file. A file given as it is is pinned, so undo leaves it alone.
    fn file_ops(&self, applied: &Applied, ends: &[FileEnd]) -> Vec<Op> {
        let pinned: BTreeSet<EntityId> = ends
            .iter()
            .filter(|end| end.pin)
            .map(|end| end.entity)
            .collect();
        applied
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
                    },
                    (file, _) => Op::File {
                        entity: *entity,
                        file: file.clone(),
                        replaces,
                        take: None,
                    },
                }
            })
            .collect()
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
            false => self.orphaned.push(Orphan {
                writer: record.writer,
                record: name,
                label: record.label.clone(),
                paths: record.paths(),
            }),
        }
    }

    /// Keeps this writer's record `name`, whose entry is now logged, to remove
    /// when the library tidies.
    fn spend(&mut self, name: Nonce, record: &PendingRecord) {
        self.unsettled.retain(|settling| settling.record != name);
        self.spent.insert(name, record.clone());
    }

    /// Places entries this instance just appended to `written` and folds them in,
    /// keeping its intents in the history with what they replaced.
    fn absorb_own(&mut self, entries: &[Entry], written: Option<(RelPath, Stamp)>) {
        let Some(id) = self.writer.as_ref().map(Writer::id) else {
            return;
        };
        let mut reverses = self.reversing.take();
        for entry in entries {
            if let EntryKind::Intent(logged) = entry.kind() {
                let before = Before::of(&logged.ops, &self.folded);
                self.history.push(entry, reverses.take(), before);
            }
            self.clock = self.clock.observe(entry.at());
        }
        self.reader.add(id, entries, written);
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
            let names = &*self.env.names;
            let was = self
                .reach
                .refile(&mut self.facts, &self.scan, names, entity, now);
            moved.extend(self.fact_paths(entity, was.as_deref()));
        }
        moved
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
            self.interrupted(settling.record, &settling.pending);
        }
        flow::run(writer::retire_writer(writer.genesis()))
    }

    /// Binds again where the facts or the scan changed since the last binding.
    fn bind_facts(&mut self) {
        if self.reach.due() {
            self.bind();
        }
    }

    fn bind(&mut self) {
        if self.rebind_reached() {
            return;
        }
        let mut binding = Binding::new(&self.facts, &self.scan, &self.unscanned);
        let names = &*self.env.names;
        binding.step(&self.facts, &self.scan, names, usize::MAX);
        let links = self
            .unscanned
            .is_empty()
            .then(|| Links::of(&self.facts, &self.scan, names));
        self.bound(binding, links);
    }

    /// Binds again only what the changes since the last binding reach, when they
    /// reach little; whether nothing is left to bind.
    fn rebind_reached(&mut self) -> bool {
        if !self.reach.due() {
            return true;
        }
        if !self.unscanned.is_empty() || !self.reach.ready() {
            return false;
        }
        let Some(seeds) = self.reach.take() else {
            return false;
        };
        let mut bindings = Arc::clone(&self.bindings);
        let names = &*self.env.names;
        let (facts, scan) = (&self.facts, &self.scan);
        let rebound = reach::rebind(
            &self.reach,
            seeds,
            facts,
            scan,
            names,
            Arc::make_mut(&mut bindings),
            &mut self.pins,
        );
        match rebound {
            Some(()) => self.bindings = bindings,
            None => self.reach.everything(),
        }
        rebound.is_some()
    }

    /// Takes what a finished binding of everything bound, and the links of what
    /// it bound unless a failed scan left something unknown.
    fn bound(&mut self, binding: Binding, links: Option<Links>) {
        let (bindings, pins) = binding.finish();
        self.pins = pins;
        self.bindings = Arc::new(bindings);
        self.reach.bound(links);
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

    /// Takes what relisting changed in the scan, with the identities `read`
    /// gave the files whose identities were read: a file gone before its read is
    /// gone. Returns the paths read.
    fn rescanned(
        &mut self,
        mut changes: reach::Changes,
        read: impl Iterator<Item = (RelPath, Option<Identity>)>,
    ) -> Vec<RelPath> {
        let mut paths = Vec::new();
        for (path, identity) in read {
            let Some(identity) = identity else {
                changes.insert(path, None);
                continue;
            };
            let entry = match changes.remove(&path) {
                Some(changed) => changed,
                None => self.scan.files.get(&path).copied(),
            };
            let file = entry.map(|file| Scanned {
                identity: Some(identity),
                ..file
            });
            changes.insert(path.clone(), file);
            paths.push(path);
        }
        self.reach.resolved();
        let names = &*self.env.names;
        for (path, file) in changes {
            self.reach
                .rescan(&mut self.scan, &self.facts, names, path, file);
        }
        paths
    }

    fn scan_task(&self) -> Task<'static, Result<Scan>> {
        binding::list_all(&self.layout).task()
    }

    /// Takes `facts`, the file facts of what is now shown.
    fn refiled(&mut self, facts: Facts) {
        self.reach.refile_all(&mut self.facts, facts);
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
        let mut index = Arc::clone(self.view.index());
        let shown = self.view.parts();
        Arc::make_mut(&mut index).update(
            (&shown.folded, &shown.bindings),
            (&self.folded, &self.bindings),
        );
        self.show_indexed(index);
    }

    /// Rebuilds the view from the library's state, bound already, whose index
    /// `index` is.
    fn show_indexed(&mut self, index: Arc<Index>) {
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
            writers: self.writer_infos(),
            forks,
            gaps,
            items,
        };
        self.view = View::indexed(parts, index);
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
    /// and released at once.
    fn presence_task(&self) -> Task<'static, Result<BTreeMap<WriterId, Presence>>> {
        let own = self.writer.as_ref().map(Writer::genesis);
        flow::list(Root::Local, &RelPath::ROOT)
            .and_then(move |entries| {
                let pool = entries
                    .into_iter()
                    .filter(|entry| entry.kind == Kind::Directory)
                    .filter_map(|entry| entry.name.parse::<EntryHash>().ok())
                    .filter(move |genesis| Some(*genesis) != own);
                fold(pool, BTreeMap::new(), |mut presence, genesis| {
                    flow::read_replaced(Root::Local, Layout::head(genesis)).and_then(move |head| {
                        let writer = head
                            .and_then(|bytes| writer::head_record(&bytes))
                            .map(|(writer, _)| writer);
                        let Some(writer) = writer else {
                            return ok(presence);
                        };
                        flow::lock(Layout::lock(genesis)).and_then(move |lock| match lock {
                            Lock::Held => {
                                presence.insert(writer, Presence::SameMachine);
                                ok(presence)
                            }
                            Lock::Acquired => flow::act(Io::Unlock {
                                name: Layout::lock(genesis),
                            })
                            .map_ok(move |()| presence),
                        })
                    })
                })
            })
            .task()
    }

    /// The drafts kept for this writer, each with whether it still applies.
    fn drafts_task(&self) -> Task<'static, Result<Vec<DraftStatus>>> {
        let Some(genesis) = self.writer.as_ref().map(Writer::genesis) else {
            return Task::ready(Ok(Vec::new()));
        };
        let files: BTreeMap<EntityId, RelPath> = self
            .bindings
            .bound
            .iter()
            .filter(|(_, file)| file.state != FileState::Missing)
            .map(|(entity, file)| (*entity, file.path.clone()))
            .collect();
        let identify = Rc::clone(&self.env.identify);
        flow::run(drafts::read_all(genesis))
            .and_then(move |found| {
                fold(found.into_iter(), Vec::new(), move |mut statuses, kept| {
                    let drafts::Kept { entity, record } = kept;
                    let status = move |state| DraftStatus { entity, state };
                    let (Some(record), Some(path)) = (record.clone(), files.get(&entity)) else {
                        statuses.push(status(match record {
                            Some(_) => DraftState::BaseChanged { found: None },
                            None => DraftState::Unreadable,
                        }));
                        return ok(statuses);
                    };
                    flow::observe(Root::Folder, path, &identify).map_ok(move |seen| {
                        let found = seen.map(|seen| seen.identity);
                        statuses.push(status(match found == Some(record.base) {
                            true => DraftState::Applies {
                                bytes: record.bytes,
                            },
                            false => DraftState::BaseChanged { found },
                        }));
                        statuses
                    })
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

/// The fact change that writes `value` to `field`; `None` for a file write, which
/// only a file effect makes.
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
        Op::Dismiss { .. } | Op::Unknown(_) => return None,
    };
    Some(Change {
        entity,
        what,
        by: By::This,
    })
}

/// About how many entries one slice of placing offers.
const PLACE_SLICE: usize = 1 << 11;

/// How many entities' file facts one slice takes.
const FILE_SLICE: usize = 1 << 12;

/// About how many entities or files one slice of binding takes.
const BIND_SLICE: usize = 1 << 12;

/// How many entities one slice of indexing takes.
const INDEX_SLICE: usize = 1 << 12;

/// About how many files listed one slice of a scan of every file takes.
const FOUND_SLICE: usize = 1 << 13;

/// Places what a read of every writer's directory found, a slice at a time.
fn absorbed<'a>(mut reader: Reader, listing: Listing) -> Flow<'a, (Reader, ReadReport)> {
    let absorbing = reader.absorbing(listing);
    let placing = flow::sliced((reader, absorbing), |(reader, absorbing)| {
        reader.absorb_slice(absorbing, PLACE_SLICE)
    });
    placing.then(|(mut reader, absorbing)| {
        let report = reader.absorbed(absorbing);
        Flow::Done((reader, report))
    })
}

/// Takes what a library just opened shows, its file facts a slice at a time,
/// then follows its clock readings and decides whether it may write.
fn fold_opened(mut library: Library) -> Flow<'static, Library> {
    library.folded = library.reader.derived().folded.clone();
    Flow::Done(library)
        .then(|library| {
            let filing = (library, Facts::new(), None);
            flow::sliced(filing, |(library, facts, after)| {
                library.folded.files_slice(facts, after, FILE_SLICE)
            })
        })
        .then(|(mut library, facts, _)| {
            library.refiled(facts);
            library.follow(library.reader.readings().collect());
            library.mode = library.mode_at_open();
            Flow::Done(library)
        })
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
                Err(error @ Error::Rekey { .. }) => library
                    .stop_writing()
                    .then(move |stopped| Flow::Done((library, stopped.and(Err(error))))),
                Err(error) => Flow::Done((library, Err(error))),
                Ok(compacted) => flow::run(library.reader.list_writer(id)).then(move |listing| {
                    let read = listing.map(|listing| {
                        library.reader.absorb(listing);
                        library.take_derived();
                        library.show();
                    });
                    Flow::Done((library, read.map(|()| compacted)))
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
    if !due || library.writable().is_err() {
        return Flow::Done(library);
    }
    compacted(library).then(|(library, _)| Flow::Done(library))
}

/// Lets go of what `library` holds a part at a time, pausing between parts.
fn dismantled(library: &mut Library) -> Flow<'_, ()> {
    flow::sliced((library, 0), |(library, part)| {
        let done = library.shed(*part);
        *part += 1;
        done
    })
    .then(|_| Flow::Done(()))
}

/// Shows what changed since the view, if anything did: moves the entries of the
/// entities that changed in the view's index, or builds its paths or all of it
/// again a slice at a time when most did.
fn reshown<'a, L: BorrowMut<Library> + 'a>(mut library: L) -> Fallible<'a, L> {
    let known = library.borrow_mut();
    known.bind_facts();
    let shown = known.view.parts();
    let rebound = !Arc::ptr_eq(&shown.bindings, &known.bindings);
    if !known.unshown && !rebound {
        return ok(library);
    }
    let (before, after) = (
        (&shown.folded, &*shown.bindings),
        (&known.folded, &*known.bindings),
    );
    let mut index = Arc::clone(known.view.index());
    match Index::changed(before, after) {
        Follow::Build => return indexed(library).then(flow::ok),
        Follow::Paths(entities) => {
            let indexing = Indexing::paths(&index);
            return reindexed(library, indexing, entities).then(flow::ok);
        }
        Follow::Move(entities) => Arc::make_mut(&mut index).move_entries(before, after, entities),
    }
    known.show_indexed(index);
    ok(library)
}

/// Binds what is due, then builds the index of what is shown a slice of
/// entities at a time, and shows it.
fn indexed<'a, L: BorrowMut<Library> + 'a>(mut library: L) -> Flow<'a, L> {
    library.borrow_mut().bind_facts();
    reindexed(library, Indexing::default(), Vec::new())
}

/// Builds what `indexing` builds a slice of entities at a time, moves the
/// entries of `entities` from the view's state to the library's, and shows it.
fn reindexed<'a, L: BorrowMut<Library> + 'a>(
    library: L,
    indexing: Indexing,
    entities: Vec<EntityId>,
) -> Flow<'a, L> {
    flow::sliced((library, indexing), |(library, indexing)| {
        let known = library.borrow();
        indexing.step(&known.folded, &known.bindings, INDEX_SLICE)
    })
    .then(|(mut library, indexing)| {
        let known = library.borrow_mut();
        let mut index = indexing.finish();
        let shown = known.view.parts();
        let before = (&shown.folded, &*shown.bindings);
        index.move_entries(before, (&known.folded, &known.bindings), entities);
        known.show_indexed(Arc::new(index));
        Flow::Done(library)
    })
}

/// Keeps what a read added to the cached view in this writer's local
/// directory, so a crash does not take back what was shown: appended to the
/// journal, or as a checkpoint encoded a slice at a time when one is due.
/// Without a writer, keeps only a change to the grace step, its state and what
/// it let go of, appended to the journal of the view it loaded and of each
/// retired writer's view it read, which the next open joins; a change it could
/// not keep is kept by the next save.
fn saved_view<'a, L: BorrowMut<Library> + 'a>(mut library: L) -> Flow<'a, (L, Result<()>)> {
    let known = library.borrow_mut();
    let Some(genesis) = known.writer.as_ref().map(Writer::genesis) else {
        if !known.reader.unsaved.grace {
            return Flow::Done((library, Ok(())));
        }
        let grace = known.reader.cached().grace();
        let let_go = known.reader.unsaved.let_go.clone();
        let mut views = known.reader.retired.clone();
        let store = known.reader.store.borrow();
        views.extend(store.genesis.map(|loaded| (loaded, store.generation)));
        drop(store);
        let saved = fold(views.into_iter(), (), move |(), (genesis, generation)| {
            flow::run(cache::keep_grace(genesis, generation, grace, &let_go))
        });
        return saved.then(|saved| {
            if saved.is_ok() {
                let unsaved = &mut library.borrow_mut().reader.unsaved;
                unsaved.grace = false;
                unsaved.let_go.clear();
            }
            Flow::Done((library, saved))
        });
    };
    if !cache::due(&known.reader, genesis) {
        let appended = cache::journal(&mut known.reader, genesis);
        return flow::run(appended).then(|saved| Flow::Done((library, saved)));
    }
    encoded(library).then(move |(mut library, checkpoint)| {
        let reader = &mut library.borrow_mut().reader;
        let written = cache::keep_checkpoint(reader, genesis, checkpoint);
        flow::run(written).then(|saved| Flow::Done((library, saved)))
    })
}

/// The checkpoint of the cached view as it is.
fn encoded<'a, L: BorrowMut<Library> + 'a>(library: L) -> Flow<'a, (L, Checkpoint)> {
    let encoding = Encoding::new(&library.borrow().reader);
    flow::sliced((library, encoding), |(_, encoding)| encoding.step())
        .then(|(library, encoding)| Flow::Done((library, encoding.finish())))
}

/// Reads every writer's pending records and sorts out this writer's and the
/// orphans. Writes nothing.
fn recover(library: Library) -> Fallible<'static, Library> {
    let writers = library.reader.writers_known().into_iter().collect();
    flow::run(recovery::read(&library.layout, writers)).and_then(move |found| {
        let mut library = library;
        let own = library
            .writer
            .as_ref()
            .map(|writer| (writer.id(), writer.head()));
        let settled = library.folded.settlements().clone();
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
            library.spent = assessed.spent.into_iter().collect();
            library.orphaned = assessed.orphaned;
            library.ignored = assessed.ignored;
            library
        })
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
    match paths.is_empty() && !library.reach.due() {
        true => ok(library),
        false => rescan_paths(library, paths),
    }
}

/// Binds every entity with a file where its facts say, knowing none of the
/// library's files until a scan of every one succeeds.
fn presumed_bound(mut library: Library) -> Flow<'static, Library> {
    library
        .unscanned
        .add(vec![RelPath::ROOT], &library.bindings);
    library.reach.everything();
    let scan = library.scan.clone();
    rebound(library, scan)
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
    flow::list_stat(Root::Folder, &dir).and_then(move |entries| {
        let mut library = library;
        let known = library.borrow_mut();
        if let Some(walking) = known.walking.as_mut() {
            walking.listed(&known.layout, dir, entries);
        }
        walk(library)
    })
}

/// Binds every file the finished walk listed.
fn walked<'a, L: BorrowMut<Library> + 'a>(library: L) -> Fallible<'a, L> {
    flow::sliced(
        (library, Scan::default(), None),
        |(library, found, after)| {
            let Some(walk) = library.borrow().walking.as_ref() else {
                return true;
            };
            let (last, finished) = walk.found_after(found, after.as_ref(), FOUND_SLICE);
            *after = last;
            finished
        },
    )
    .then(|(library, found, _)| {
        let resolving = (library, found, Resolving::default());
        flow::sliced(resolving, |(library, found, resolving)| {
            let known = library.borrow();
            resolving.step(found, &known.facts, &known.scan, FOUND_SLICE)
        })
    })
    .then(|(library, found, resolving)| {
        let identify = &library.borrow().env.identify;
        let identified = binding::identified(found, resolving.needed, identify);
        identified.map_ok(move |read| (library, read))
    })
    .and_then(move |(library, (scan, read))| {
        let mut library = library;
        let known = library.borrow_mut();
        known.walking = None;
        if !known.unscanned.is_empty() {
            known.unscanned = Unscanned::default();
            known.reach.everything();
        }
        known.reach.resolved();
        known.behind.remove(&Lag::Scan);
        rebound(library, scan).then(move |library| {
            remember(library, read).then(|(library, kept)| Flow::Done(kept.map(|()| library)))
        })
    })
}

/// Binds the facts to `scan` a slice at a time, unless neither changed.
fn rebound<'a, L: BorrowMut<Library> + 'a>(mut library: L, scan: Scan) -> Flow<'a, L> {
    let known = library.borrow_mut();
    if !known.reach.due() && scan == known.scan {
        return Flow::Done(library);
    }
    known.scan = scan;
    known.reach.everything();
    bind_everything(library)
}

/// Binds every fact to the scan a slice at a time, then links what it bound
/// unless a failed scan left something unknown.
fn bind_everything<'a, L: BorrowMut<Library> + 'a>(library: L) -> Flow<'a, L> {
    let known = library.borrow();
    let binding = Binding::new(&known.facts, &known.scan, &known.unscanned);
    flow::sliced((library, binding), |(library, binding)| {
        let known = library.borrow();
        binding.step(&known.facts, &known.scan, &*known.env.names, BIND_SLICE)
    })
    .then(|(library, binding)| {
        let linking = library.borrow().unscanned.is_empty().then(Linking::default);
        flow::sliced((library, linking), |(library, linking)| {
            let Some(linking) = linking else {
                return true;
            };
            let known = library.borrow();
            linking.step(&known.facts, &known.scan, &*known.env.names, BIND_SLICE)
        })
        .then(move |(mut library, linking)| {
            library
                .borrow_mut()
                .bound(binding, linking.map(Linking::finish));
            Flow::Done(library)
        })
    })
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

/// Once entries are durable, scans again only `paths`, and binds: only what the
/// changes reach, unless the facts changed wholesale or a failed scan left
/// something unknown.
fn rescan_paths(library: &mut Library, paths: Vec<RelPath>) -> Fallible<'_, &mut Library> {
    if !library.reach.ready() || !library.unscanned.is_empty() {
        let listed = binding::relist(&library.layout, &library.scan, paths.clone()).task();
        return rebind_logged(library, listed, paths);
    }
    if let Some(walking) = library.walking.as_mut() {
        walking.forget(&paths);
    }
    let listed = binding::relist(&library.layout, &Scan::default(), paths.clone());
    listed.then(move |listed| {
        let found = match listed {
            Ok(found) => found,
            Err(error) => return unscanned(library, paths, error),
        };
        let gone = binding::under_any(&library.scan, &paths);
        let links = library.reach.links().expect("ready above");
        let (changes, unknown) = reach::resolve(
            &library.scan,
            &library.facts,
            links,
            &*library.env.names,
            gone,
            found,
            library.reach.gained(),
        );
        let reading: Vec<RelPath> = unknown.iter().map(|(path, _)| path.clone()).collect();
        let identities = flow::identities(Root::Folder, unknown, &library.env.identify);
        identities.then(move |identities| {
            let identities = match identities {
                Ok(identities) => identities,
                Err(error) => return unscanned(library, paths, error),
            };
            let read = library.rescanned(changes, reading.into_iter().zip(identities));
            bind_reached(library)
                .then(move |library| remember(library, read).then(|(library, _)| ok(library)))
        })
    })
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
    library.reach.everything();
    ok(library)
}

/// Binds again what changed reaches, or everything a slice at a time when it
/// reaches too much.
fn bind_reached<'a, L: BorrowMut<Library> + 'a>(mut library: L) -> Flow<'a, L> {
    match library.borrow_mut().rebind_reached() {
        true => Flow::Done(library),
        false => bind_everything(library),
    }
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
                let unknown = binding::resolve(&mut scan, &library.facts, &library.scan);
                library.reach.resolved();
                binding::identified(scan, unknown, &library.env.identify)
            }
            Err(error) => Flow::Done(Err(error)),
        };
        identified.then(move |scanned| match scanned {
            Ok((scan, read)) => rebound(library, scan)
                .then(move |library| remember(library, read).then(|(library, _)| ok(library))),
            Err(error) => unscanned(library, unsure, error),
        })
    })
}

/// Once the facts changed, reads the identities they now need of the files the
/// last scan found, and binds.
fn reidentify(library: &mut Library) -> Fallible<'_, &mut Library> {
    match library.reach.due() {
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

/// Settles this writer's interrupted effects, so that what is planned next is
/// planned against the facts and history they leave.
fn settle_first(library: &mut Library) -> Fallible<'_, &mut Library> {
    if library.unsettled.is_empty() {
        return ok(library);
    }
    settle_own(library)
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
            };
            library.reversing = reverses;
            transact(library, logged, effects, None, pins.collect())
        })
        .and_then(move |(library, entries, outcome)| {
            library.reversing = None;
            match committed(shown, &entries, created, outcome, library.local()) {
                Ok(committed) => ok((library, committed)),
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
    let committed = Committed {
        intent: entries[0].hash(),
        created,
        changes: distinct(ops.filter_map(change_of)),
        local,
    };
    match outcome {
        Outcome::Complete => Ok(committed),
        Outcome::Partial(report) => Err(Box::new(Partial {
            label,
            committed,
            report: PartialReport {
                record: None,
                ..report
            },
        })),
    }
}

/// The entity whose file `op` writes, if it writes one.
fn file_of(op: &Op) -> Option<EntityId> {
    match op {
        Op::File { entity, .. } | Op::Pin { entity, .. } => Some(*entity),
        _ => None,
    }
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
        encoded(library).then(move |(library, checkpoint)| {
            let segment = library.env.segment_name();
            let (layout, reader) = (library.layout.clone(), &mut library.reader);
            let create = Writer::create(layout, id, segment, label, at, |genesis| {
                cache::keep_checkpoint(reader, genesis, checkpoint)
            });
            flow::run(create).and_then(move |writer| {
                writer.stamp().then(move |written| {
                    library.writer = Some(writer);
                    library.absorb_own(&[genesis], written);
                    ok(library)
                })
            })
        })
    })
}

/// Settles this writer's interrupted effects, oldest first, and shows what they
/// leave. A record stays to settle while settling it fails, and is spent once its
/// entry is logged. One whose effects stop partway is logged as far as they got,
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
    flow::run(settled)
        .and_then(move |applied| {
            let mut logged = planned(&record);
            logged.ops.extend(library.file_ops(&applied, &record.files));
            logged.forks = library.known_forks(&logged.ops);
            logged.displaced = applied.displaced;
            let label = logged.label.clone();
            append(library, vec![EntryKind::Intent(logged)]).and_then(move |(library, entries)| {
                library.spend(name, &record);
                let scan = library.scan_task();
                rebind_logged(library, scan, record.paths())
                    .map_ok(move |library| (library, label, entries, applied.outcome))
            })
        })
        .and_then(|(library, label, entries, outcome)| {
            match committed(label, &entries, Vec::new(), outcome, library.local()) {
                Ok(_) => ok(library),
                Err(partial) => {
                    library.show();
                    Flow::Done(Err(Error::Unfinished(partial)))
                }
            }
        })
}

/// Removes this writer's spent records, each after its markers, and returns those
/// removed, or the first failure once every record is tried. Each is forgotten as
/// it is removed, so a task dropped partway keeps what it removed.
fn tidied(library: &mut Library) -> Flow<'_, (&mut Library, Result<Tidied>)> {
    let names: Vec<Nonce> = library.spent.keys().copied().collect();
    remove_spent(library, names.into_iter(), Tidied::default(), Ok(()))
}

fn remove_spent(
    library: &mut Library,
    mut names: std::vec::IntoIter<Nonce>,
    mut tidied: Tidied,
    failed: Result<()>,
) -> Flow<'_, (&mut Library, Result<Tidied>)> {
    let Some(name) = names.next() else {
        return Flow::Done((library, failed.map(|()| tidied)));
    };
    let removing = effects::finish(&library.layout, name, &library.spent[&name]);
    flow::run(removing).then(move |removed| {
        let failed = match removed {
            Ok(()) => {
                library.spent.remove(&name);
                tidied.removed.push(name);
                failed
            }
            Err(error) => failed.and(Err(error)),
        };
        remove_spent(library, names, tidied, failed)
    })
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
        },
    };
    logged.ops.retain(|op| !matches!(op, Op::File { .. }));
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
/// durable in the folder, they are shown and kept in the history, and the cached
/// view holding them is saved and then the head recorded, so that what a commit
/// returns survives a crash and a restore. Neither failing fails the append; see
/// [`keep_local`]. A writer that cannot continue its history stops writing: the
/// next commit creates another.
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

/// Saves the cached view, then records the head when `head` asks or when it
/// lags, each whatever became of the one before. Each failure leaves its part behind, to save again the next time;
/// the first is returned. A crash meanwhile loses nothing the folder holds:
/// reopening reads the folder into the view, which only grows, and continues from
/// the last entry of the writer's chain there.
fn keep_local(library: &mut Library, head: bool) -> Flow<'_, (&mut Library, Result<()>)> {
    saved_view(library).then(move |(library, saved)| {
        library.lag(Lag::View, &saved);
        let due = head || library.behind.contains_key(&Lag::Head);
        let recorded = match &library.writer {
            Some(writer) if due => writer.record(),
            _ => Task::ready(Ok(())),
        };
        flow::run(recorded).then(move |recorded| {
            library.lag(Lag::Head, &recorded);
            Flow::Done((library, saved.and(recorded)))
        })
    })
}

/// What settling another writer's record adds once the settlement's effects
/// complete: the facts the record planned, ahead of the intent's own ops, then
/// the entry that closes the record.
struct Closing {
    facts: Vec<Op>,
    settle: Settle,
}

/// The most of a line the ops of one `bind` entry take, so that a commit pins any
/// number of moves.
const BIND_BYTES: usize = MAX_LINE / 2;

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
fn closed(mut logged: Logged, closing: Option<Closing>) -> Vec<EntryKind> {
    let Some(Closing { mut facts, settle }) = closing else {
        return vec![EntryKind::Intent(logged)];
    };
    facts.append(&mut logged.ops);
    logged.ops = facts;
    vec![EntryKind::Intent(logged), EntryKind::Settle(settle)]
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
) -> Fallible<'a, (&'a mut Library, Vec<Entry>, Outcome)> {
    if effects.is_empty() {
        let mut kinds = closed(logged, closing);
        kinds.extend(bound(pins));
        return append(library, kinds)
            .and_then(|(library, entries)| {
                reidentify(library).map_ok(move |library| (library, entries))
            })
            .map_ok(|(library, entries)| {
                library.show();
                (library, entries, Outcome::Complete)
            });
    }
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
    let Some(entry) = encoded else {
        return Flow::Done(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong))));
    };
    let record = Rc::new(PendingRecord::new(
        id,
        &logged.label,
        entry.line(),
        &effects,
    ));
    let layout = library.layout.clone();
    let identify = Rc::clone(&library.env.identify);
    let (name, journaled) = (effects.record, effects.moves_files());
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
/// keeps the record as spent once the entries are durable, and scans again the
/// paths the effects moved files from and to. Once the entries are durable nothing
/// fails.
fn log_effects<'a>(
    library: &'a mut Library,
    mut logged: Logged,
    effects: &EffectPlan,
    record: Rc<PendingRecord>,
    applied: Applied,
    closing: Option<Closing>,
    pins: Vec<Op>,
) -> Fallible<'a, (&'a mut Library, Vec<Entry>, Outcome)> {
    let (name, journaled) = (effects.record, effects.moves_files());
    logged.ops.extend(library.file_ops(&applied, &record.files));
    logged.forks = library.known_forks(&logged.ops);
    logged.displaced = applied.displaced;
    let outcome = applied.outcome;
    let closing = closing.filter(|_| outcome == Outcome::Complete);
    let mut kinds = closed(logged, closing);
    kinds.extend(bound(pins));
    let touched = moved_paths(&effects.steps);
    appending(library, kinds).then(move |(library, appended)| {
        let logged = library
            .writer
            .as_ref()
            .is_some_and(|writer| writer.head() != record.after());
        match (journaled, logged) {
            (false, _) => {}
            (true, true) => library.spend(name, &record),
            (true, false) => library.interrupted(name, &record),
        }
        let entries = match appended {
            Ok(entries) => entries,
            Err(error) => return Flow::Done(Err(error)),
        };
        rescan_paths(library, touched).map_ok(move |library| {
            library.show();
            (library, entries, outcome)
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
                    };
                    let closing = Closing { facts, settle };
                    transact(library, logged, effects, Some(closing), Vec::new())
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
                    ok(committed)
                }
            }
        })
}
