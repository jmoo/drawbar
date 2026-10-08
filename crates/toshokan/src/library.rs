//! The library as one instance holds it: the pure core both drivers run.
//!
//! Only [`Library::commit`] (and undo, redo, settling and adopting, which commit),
//! [`Library::empty_trash`] and [`Library::compact`] write the folder. Opening,
//! viewing, refreshing and scanning never do; they write only in the local root.
//!
//! Every operation is a chain of requests. Each step that needs I/O runs as a task
//! that owns what it reads, and its result is folded into the library afterwards,
//! so no task borrows the library while it runs.

use std::borrow::BorrowMut;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use crate::binding::{self, Binding, Bindings, Facts, Scan, Unscanned, Walk};
use crate::cache;
use crate::drafts::{self, DraftRecord};
use crate::effects::{self, Applied, EffectPlan, EffectStep, FileEnd};
use crate::env::Env;
use crate::error::{Error, Invalid, Refusal, Result, Why};
use crate::flow::{self, fold, ok, Fallible, Flow};
use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::intent;
use crate::io::{Capabilities, Io, Kind, Lock, Root, Task};
use crate::layout::Layout;
use crate::line::MAX_LINE;
use crate::log::{Bound, Entry, EntryKind, FileFact, Genesis, Logged, Op, Settle, Settlement};
use crate::merge::{merge, Beyond, Folded, Merging, Part};
use crate::path::RelPath;
use crate::pending::{self, PendingRecord};
use crate::plan::{FactChange, FileChange, Plan};
use crate::reader::{Listing, ReadReport, Reader, Stamp, WriterLog};
use crate::recovery::{self, Settling};
use crate::report::{
    By, Change, Committed, Compacted, DraftState, DraftStatus, Emptied, HistoryItem, Lag, Local,
    Mode, Opened, Orphan, Outcome, Partial, PartialReport, Presence, Refreshed, Settled, Start,
    TrashItem, What, WriterInfo,
};
use crate::schema::{Schema, Written};
use crate::trash::{self, Policy};
use crate::undo::History;
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
    orphaned: Vec<Orphan>,
    /// Pending records recovery would not act on.
    ignored: Vec<RelPath>,
    /// What is shown: each writer's log, or what the folder holds of it once the
    /// install let go every entry of it the folder lost.
    folded: Folded,
    /// The snapshots `folded` joined, by writer and last folded entry.
    joined: Joined,
    /// Each writer's placed and folded entries no file in the folder held at the
    /// last read.
    lost: Let,
    /// The entries this install let go, by writer, while the folder lacks them.
    let_go: Let,
    /// What is shown that the folder no longer holds, and not let go.
    removed: Vec<Beyond>,
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
    /// Bound from `facts`, `scan` and `unscanned` unless `bind_due`.
    bindings: Arc<Bindings>,
    bind_due: bool,
    /// The moves the bindings found that the facts do not say yet.
    pins: Vec<Op>,
    presence: BTreeMap<WriterId, Presence>,
    history: History,
    view: View,
    /// Whether a read folded in something the view does not show yet.
    unshown: bool,
    /// The parts of this instance's state that lag what it committed, with why.
    behind: BTreeMap<Lag, String>,
}

type Let = BTreeMap<WriterId, BTreeSet<EntryHash>>;

/// Snapshots a merged state joined, by writer and last folded entry.
type Joined = BTreeSet<(WriterId, Option<EntryHash>)>;

/// The snapshots of `logs`, as [`Joined`].
fn joined<'a>(logs: impl Iterator<Item = &'a WriterLog>) -> Joined {
    logs.flat_map(|log| log.snapshots().iter().map(|s| (log.writer(), s.head())))
        .collect()
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
    /// of the pool joined, reads the folder, assesses recovery, scans the
    /// library's files and checks drafts. `capabilities` are the folder's.
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
                flow::read_replaced(Root::Local, Layout::let_go()).map_ok(move |bytes| {
                    let let_go = bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok());
                    (picked, cached, let_go.unwrap_or_default())
                })
            })
            .and_then(|(picked, cached, let_go)| {
                flow::read_replaced(Root::Local, Layout::identities()).map_ok(move |bytes| {
                    let remembered = bytes.and_then(|bytes| Scan::of_identities(&bytes));
                    (picked, cached, (let_go, remembered.unwrap_or_default()))
                })
            })
            .and_then(move |(picked, mut cached, kept)| {
                let resumed = cached.state.take().map(|state| {
                    let logs = cached.view.writers().values();
                    (state, joined(logs))
                });
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
                                kept,
                            );
                            (library, report, claimed.start, resumed)
                        })
                    })
                })
            })
            .and_then(|(mut library, report, start, resumed)| {
                let resumed =
                    resumed.is_some_and(|(state, joined)| library.resume(state, joined, &report));
                fold_opened(library, resumed).then(move |library| ok((library, report, start)))
            })
            .and_then(|(mut library, report, start)| {
                let saved = library.save_view();
                flow::run(saved).and_then(move |()| {
                    recover(library).map_ok(move |library| (library, report, start))
                })
            })
            .and_then(|(library, report, start)| {
                rescan(library).map_ok(move |library| (library, report, start))
            })
            .and_then(|(library, report, start)| {
                flow::run(library.presence_task()).map_ok(move |presence| {
                    let mut library = library;
                    library.presence = presence;
                    library.show();
                    (library, report, start)
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
                        removed: library.removed_facts(),
                        forks: report.forks,
                        gaps: report.gaps,
                        unreadable: report
                            .unreadable
                            .into_iter()
                            .map(|(path, _)| path)
                            .collect(),
                        ignored: library.ignored.clone(),
                        scan: library.bindings.report.clone(),
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
        (let_go, remembered): (Let, Scan),
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
            orphaned: Vec::new(),
            ignored: Vec::new(),
            folded: Folded::default(),
            joined: BTreeSet::new(),
            lost: Let::new(),
            let_go,
            removed: Vec::new(),
            ahead: BTreeSet::new(),
            facts: Facts::new(),
            scan: remembered.clone(),
            remembered,
            unkept: false,
            unscanned: Unscanned::default(),
            walking: None,
            bindings: Arc::default(),
            bind_due: false,
            pins: Vec::new(),
            presence: BTreeMap::new(),
            history: History::default(),
            unshown: false,
            view: View::new(Parts {
                folded: Folded::default(),
                bindings: Arc::default(),
                writers: Vec::new(),
                forks: Vec::new(),
                gaps: Vec::new(),
            }),
            behind: BTreeMap::new(),
        }
    }

    /// Whether a library just opened may write.
    fn mode_at_open(&self) -> Mode {
        match (&self.writer, self.capabilities.append) {
            (_, false) => Mode::ReadOnly(Why::FolderNotWritable),
            (Some(writer), true) => match newer_entry(&self.folded, &self.reader, writer.id()) {
                Some(entry) => Mode::ReadOnly(Why::NewerOwnHistory { entry }),
                None => Mode::Writable,
            },
            (None, true) => Mode::Writable,
        }
    }

    pub fn view(&self) -> View {
        self.view.clone()
    }

    /// What the view would show if every log were folded again from scratch, as
    /// tests compare with what it shows.
    #[doc(hidden)]
    pub fn refolded(&self) -> Folded {
        let folder = self.shown_as_folder();
        merge(shown_logs(&self.reader, |writer| folder.contains(&writer)))
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

    pub fn undo(&mut self) -> Task<'_, Result<Committed>> {
        self.commit_with(|library| {
            let (writer, own) = library.own_entries()?;
            let plan = library.history.plan_undo(&own, writer, &library.view);
            library.resolve(plan.map_err(Error::Refused)?)
        })
    }

    pub fn redo(&mut self) -> Task<'_, Result<Committed>> {
        self.commit_with(|library| {
            let (writer, own) = library.own_entries()?;
            let plan = library.history.plan_redo(&own, writer, &library.view);
            library.resolve(plan.map_err(Error::Refused)?)
        })
    }

    /// Republishes what [`Opened::removed`] reports as an intent of this writer
    /// labeled `label`, then lets the entries the folder lost go: the folder then
    /// holds what this install showed. Refused with [`Refusal::Nothing`] when
    /// nothing is reported. A writer whose own entries the folder lost stops
    /// first, so the intent is a new writer's.
    pub fn adopt(&mut self, label: &str) -> Task<'_, Result<Committed>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        let label = label.to_owned();
        let removed = self.reader.removed();
        let lost = self.writer.as_ref().map(Writer::id);
        let stopped = match lost.is_some_and(|own| removed.contains_key(&own)) {
            true => self.stop_writing(),
            false => ok(()),
        };
        stopped
            .and_then(move |()| settle_first(self))
            .and_then(move |library| {
                if library.removed.is_empty() {
                    return Flow::Done(Err(Error::Refused(Refusal::Nothing)));
                }
                let resolved = Resolved {
                    label,
                    created: Vec::new(),
                    facts: library.removed.iter().map(|b| b.op.clone()).collect(),
                    effects: Rc::new(EffectPlan::new(library.env.nonce())),
                    reverses: None,
                };
                commit(library, resolved)
            })
            .and_then(|(library, mut committed)| {
                flow::run(library.forget()).then(move |kept| {
                    library.lag(Lag::LetGo, &kept);
                    committed.local = library.local();
                    ok(committed)
                })
            })
            .task()
    }

    /// Stops showing what [`Opened::removed`] reports: each writer whose entries
    /// the folder lost is shown as the folder holds it, on every open of this
    /// install, until the folder holds them again. Writes only in the local root.
    /// When that write fails, they are not shown for now, and the next commit,
    /// refresh or close writes it again.
    pub fn let_go(&mut self) -> Task<'_, Result<()>> {
        flow::run(self.forget())
            .then(move |kept| {
                self.lag(Lag::LetGo, &kept);
                Flow::Done(kept)
            })
            .task()
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

    /// Lets go every entry the folder lost: shows their writers as the folder
    /// holds them, and keeps the entries in `let-go.json` of the local root.
    fn forget(&mut self) -> Task<'static, Result<()>> {
        self.let_go = self.reader.removed();
        self.refold();
        self.show();
        self.keep_let_go()
    }

    fn keep_let_go(&self) -> Task<'static, Result<()>> {
        let bytes = serde_json::to_vec(&self.let_go).expect("hashes by writer are JSON");
        flow::replace(Root::Local, Layout::let_go(), bytes).task()
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

    fn own_entries(&self) -> Result<(WriterId, Vec<Entry>)> {
        let writer = self
            .writer
            .as_ref()
            .ok_or(Error::Refused(Refusal::Nothing))?;
        let own = self
            .reader
            .logs()
            .get(&writer.id())
            .map(|log| {
                log.entries()
                    .iter()
                    .map(|entry| Entry::clone(entry))
                    .collect()
            })
            .unwrap_or_default();
        Ok((writer.id(), own))
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
                let reached = recovery::progress(&library.layout, orphan.record, &theirs);
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
    /// folder no longer holds. Of the library's files it scans again only those at
    /// the paths whose file facts the entries changed, unless a scan failed since
    /// the last scan of every file, which it then repeats. Writes nothing in the
    /// folder.
    pub fn refresh(&mut self) -> Task<'_, Result<Refreshed>> {
        self.look(Look::Logs)
    }

    /// Refreshes, and scans every library file, to find what changed outside. A
    /// rescan that stops partway, as when the app drops it to commit, keeps what
    /// it listed, and the next one goes on from there; nothing is bound from it
    /// until every directory is listed, and the directories a commit or refresh
    /// scanned meanwhile are listed again.
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
        flow::run(self.reader.list())
            .and_then(move |listing| {
                let report = self.reader.absorb(listing);
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
            .map_ok(Library::refreshed)
            .task()
    }

    /// What changed since the view, which is then shown.
    fn refreshed(&mut self) -> Refreshed {
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
        let rebound = !Arc::ptr_eq(before, &self.bindings);
        for (entity, file) in self.bindings.bound.iter().filter(|_| rebound) {
            let was = before.bound.get(entity).map(presumed);
            if was.as_ref() != Some(file) && !explained.contains(entity) {
                changes.push(Change {
                    entity: *entity,
                    what: What::File,
                    by: By::Outside,
                });
            }
        }
        if self.unshown || rebound {
            self.show();
        }
        Refreshed {
            changes,
            removed: self.removed_facts(),
        }
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
    /// segments of its directory a snapshot folds. Undo reaches back only to the
    /// snapshot. Refused with [`Refusal::Nothing`] when this instance writes as no
    /// writer yet, and when a snapshot in the writer's directory already folds its
    /// last entry.
    pub fn compact(&mut self) -> Task<'_, Result<Compacted>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        let Some(id) = self.writer.as_ref().map(Writer::id) else {
            return Task::ready(Err(Error::Refused(Refusal::Nothing)));
        };
        flow::run(self.reader.list_writer(id))
            .and_then(move |listing| {
                self.reader.absorb(listing);
                let writer = self.writer.take().expect("checked above");
                let name = self.env.nonce();
                flow::run(crate::compaction::compact(writer, &self.reader, name)).then(
                    move |(writer, compacted)| match compacted {
                        Err(error @ Error::Rekey { .. }) => {
                            self.writer = Some(writer);
                            self.stop_writing()
                                .and_then(move |()| Flow::Done(Err(error)))
                        }
                        Err(error) => {
                            self.writer = Some(writer);
                            Flow::Done(Err(error))
                        }
                        Ok(compacted) => {
                            self.writer = Some(writer);
                            flow::run(self.reader.list_writer(id)).map_ok(move |listing| {
                                self.reader.absorb(listing);
                                self.refold();
                                self.show();
                                compacted
                            })
                        }
                    },
                )
            })
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

    /// Writes the cached view and what else lags, seals the open segment and
    /// releases the writer's lock. A library dropped without closing leaves its
    /// segment open, as a crash does.
    pub fn close(&mut self) -> Task<'_, Result<()>> {
        if self.writer.is_none() {
            return Task::ready(Ok(()));
        }
        keep_local(self, false)
            .then(|(library, kept)| {
                let writer = library.writer.take().expect("checked above");
                match kept {
                    Ok(()) => flow::run(writer.close()),
                    Err(error) => Flow::Done(Err(error)),
                }
            })
            .task()
    }

    /// Keeps what a read added to the cached view in this writer's local
    /// directory, so a crash does not take back what was shown.
    fn save_view(&mut self) -> Task<'static, Result<()>> {
        let Some(writer) = &self.writer else {
            return Task::ready(Ok(()));
        };
        let state = self.shown_as_folder().is_empty().then_some(&self.folded);
        self.reader.save(writer.genesis(), state)
    }

    /// Whether `theirs`, another writer's record `name`, is still open: confined
    /// to the library, chained to its writer's log, not closed and not settled.
    fn is_orphan(&self, name: Nonce, theirs: &PendingRecord) -> bool {
        let own = self.writer.as_ref().map(Writer::id);
        let Some(log) = self.reader.logs().get(&theirs.writer) else {
            return false;
        };
        own != Some(theirs.writer)
            && theirs.is_confined(&self.layout)
            && recovery::Chain::holds(log, theirs.after())
            && !recovery::Chain::continues(log, theirs.after())
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
            .filter_map(|change| match change {
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
                    .into_iter()
                    .map(|write| write.entry)
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
                    },
                }
            })
            .collect()
    }

    /// Keeps a pending record a failure left behind: this writer's is settled
    /// before its next write is planned; one of a writer it stopped writing as
    /// is an orphan.
    fn interrupted(&mut self, name: Nonce, record: &PendingRecord, logged: bool) {
        let own = self.writer.as_ref().map(Writer::id) == Some(record.writer);
        match own {
            true => self.unsettled.push(Settling {
                record: name,
                pending: record.clone(),
                logged,
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

    /// Notes how removing this writer's record `name`, whose entry is logged,
    /// went: one that stays is removed by the next write.
    fn closed(&mut self, name: Nonce, removed: &Result<()>) {
        let record = |settling: &Settling| settling.record == name;
        match removed {
            Ok(()) => self.unsettled.retain(|settling| !record(settling)),
            Err(_) => {
                for settling in self.unsettled.iter_mut().filter(|s| record(s)) {
                    settling.logged = true;
                }
            }
        }
        let left = self.unsettled.iter().any(|settling| settling.logged);
        if removed.is_err() || !left {
            self.lag(Lag::Record, removed);
        }
    }

    /// Places entries this instance just appended to `written` and folds them in.
    fn absorb_own(&mut self, entries: &[Entry], written: Option<(RelPath, Stamp)>) {
        let Some(id) = self.writer.as_ref().map(Writer::id) else {
            return;
        };
        self.reader.add(id, entries, written);
        for entry in entries {
            let kind = entry.kind();
            self.folded.fold(id, entry, &kind);
            self.history.push(entry);
            self.clock = self.clock.observe(entry.at());
            self.refile(entities(&kind));
        }
    }

    /// Shows `state`, the merged state the cached view kept with the snapshots
    /// it `joined`, and what `read` placed since. False, taking nothing, while a
    /// writer is shown as the folder holds it: everything is folded again then.
    fn resume(&mut self, state: Folded, joined: Joined, read: &ReadReport) -> bool {
        self.lost = self.reader.removed();
        self.retain_let_go();
        if !self.shown_as_folder().is_empty() {
            return false;
        }
        self.folded = state;
        self.joined = joined;
        self.fold_read(read);
        true
    }

    /// Folds what a read placed, and the snapshots it kept, into what is shown.
    /// Refolds every log while a writer is shown as the folder holds it, since
    /// what the folder holds of it may have shrunk. Returns the library paths the
    /// changed file facts name, and those where the entities they changed were
    /// bound.
    fn absorb_read(&mut self, report: &ReadReport) -> Vec<RelPath> {
        if !report.anything_new() {
            self.follow(Vec::new());
            return Vec::new();
        }
        let shown_as_folder = !self.shown_as_folder().is_empty();
        self.lost = self.reader.removed();
        self.retain_let_go();
        if shown_as_folder || !self.shown_as_folder().is_empty() {
            let was = std::mem::take(&mut self.facts);
            self.refold();
            self.follow(readings(&self.reader).collect::<Vec<_>>());
            return self.moved_since(&was);
        }
        let own = self.writer.as_ref().map(Writer::id);
        let (followed, joined, touched) = self.fold_read(report);
        let moved = match joined {
            true => {
                let was = std::mem::replace(&mut self.facts, self.folded.files());
                self.bind_due = true;
                self.moved_since(&was)
            }
            false => self.refile(touched.into_iter()),
        };
        if own.is_some_and(|own| joined || report.placed.contains_key(&own)) {
            self.rehistory();
        }
        self.follow(followed);
        moved
    }

    /// Folds into what is shown the snapshots of the logs it has not joined and
    /// the entries `report` placed. Returns their readings, whether it joined a
    /// snapshot, and the entities the entries changed.
    fn fold_read(&mut self, report: &ReadReport) -> (Vec<Hlc>, bool, BTreeSet<EntityId>) {
        let mut followed = Vec::new();
        let mut joined = false;
        let mut touched = BTreeSet::new();
        for log in self.reader.logs().values() {
            for snapshot in log.snapshots() {
                if self.joined.insert((log.writer(), snapshot.head())) {
                    self.folded.join(&snapshot.state);
                    followed.push(snapshot.at);
                    joined = true;
                }
            }
        }
        for (writer, placed) in &report.placed {
            for entry in placed_entries(&self.reader.logs()[writer], placed) {
                let kind = entry.kind();
                self.folded.fold(*writer, entry, &kind);
                followed.push(entry.at());
                touched.extend(entities(&kind));
            }
        }
        (followed, joined, touched)
    }

    /// The paths of the file facts that differ between `was` and the facts now,
    /// and those where the entities whose facts differ are bound.
    fn moved_since(&self, was: &Facts) -> Vec<RelPath> {
        let entities: BTreeSet<EntityId> = was.keys().chain(self.facts.keys()).copied().collect();
        let changed = entities
            .into_iter()
            .filter(|entity| was.get(entity) != self.facts.get(entity));
        changed
            .flat_map(|entity| self.fact_paths(entity, was.get(&entity)))
            .collect()
    }

    /// The paths `was`, an entity's file facts before, named, those its facts name
    /// now, and where it was bound.
    fn fact_paths(&self, entity: EntityId, was: Option<&Vec<Written<FileFact>>>) -> Vec<RelPath> {
        let logged = was.into_iter().chain(self.facts.get(&entity)).flatten();
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
            moved.extend(self.fact_paths(entity, was.as_ref()));
            self.bind_due = true;
        }
        moved
    }

    /// Stops writing as this writer: it leaves the pool, and the next commit
    /// creates a new one. Its open interrupted effects become orphans, settled
    /// only with consent.
    fn stop_writing<'a>(&mut self) -> Fallible<'a, ()> {
        let Some(writer) = self.writer.take() else {
            return ok(());
        };
        self.history = History::default();
        self.behind.remove(&Lag::Head);
        self.behind.remove(&Lag::Record);
        let open = std::mem::take(&mut self.unsettled).into_iter();
        for settling in open.filter(|settling| !settling.logged) {
            self.interrupted(settling.record, &settling.pending, false);
        }
        flow::run(writer::retire_writer(writer.genesis()))
    }

    /// Binds again where the facts changed since the last binding.
    fn bind_facts(&mut self) {
        if self.bind_due {
            self.bind();
        }
    }

    fn bind(&mut self) {
        let mut binding = Binding::new(&self.facts, &self.scan, &self.unscanned);
        binding.step(&self.facts, &self.scan, &*self.env.names, usize::MAX);
        self.bound(binding);
    }

    /// Takes what a finished binding bound.
    fn bound(&mut self, binding: Binding) {
        let (bindings, pins) = binding.finish();
        self.pins = pins;
        self.bindings = Arc::new(bindings);
        self.bind_due = false;
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

    fn scan_task(&self) -> Task<'static, Result<(Scan, Vec<RelPath>)>> {
        binding::scan(&self.layout, &self.env.identify, &self.facts, &self.scan)
    }

    /// Folds what is shown from scratch: each writer's log, or what the folder
    /// holds of it once every entry of it the folder lost was let go. Forgets
    /// let-go entries the folder holds again.
    fn refold(&mut self) {
        let mut merging = self.unfold();
        merging.step(usize::MAX);
        self.folded = merging.folded();
        self.refiled(self.folded.files());
    }

    /// The merge [`Library::refold`] folds.
    fn unfold(&mut self) -> Merging {
        self.lost = self.reader.removed();
        self.retain_let_go();
        let folder = self.shown_as_folder();
        let logs = shown_logs(&self.reader, |writer| folder.contains(&writer));
        self.joined = joined(logs.iter().copied());
        Merging::new(logs)
    }

    /// Takes `facts`, the file facts of what is now shown.
    fn refiled(&mut self, facts: Facts) {
        self.facts = facts;
        self.bind_due = true;
        self.rehistory();
    }

    /// Forgets let-go entries the folder holds again.
    fn retain_let_go(&mut self) {
        for (writer, let_go) in &mut self.let_go {
            let gone = self.lost.get(writer);
            let_go.retain(|hash| gone.is_some_and(|gone| gone.contains(hash)));
        }
        self.let_go.retain(|_, let_go| !let_go.is_empty());
    }

    /// The writers shown as the folder holds them: every entry of theirs the
    /// folder lost was let go.
    fn shown_as_folder(&self) -> BTreeSet<WriterId> {
        let let_go = |writer| self.let_go.get(writer);
        self.lost
            .iter()
            .filter(|(writer, gone)| let_go(*writer).is_some_and(|l| gone.is_subset(l)))
            .map(|(writer, _)| *writer)
            .collect()
    }

    fn rehistory(&mut self) {
        self.history = match &self.writer {
            Some(writer) => self
                .reader
                .logs()
                .get(&writer.id())
                .map(|log| History::of(log.entries().iter().map(|entry| &**entry)))
                .unwrap_or_default(),
            None => History::default(),
        };
    }

    /// What is shown that the folder no longer holds and was not let go, with the
    /// ops that would republish it.
    fn unkept(&self) -> Vec<Beyond> {
        if self.lost.len() == self.shown_as_folder().len() {
            return Vec::new();
        }
        let kept = merge(shown_logs(&self.reader, |writer| {
            self.lost.contains_key(&writer)
        }));
        self.folded.beyond(&kept)
    }

    /// What is shown that the folder no longer holds, as changes by writer.
    fn removed_facts(&self) -> Vec<Change> {
        distinct(self.removed.iter().map(|beyond| Change {
            entity: beyond.entity,
            what: what(&beyond.part),
            by: self.by(beyond.by),
        }))
    }

    /// Who `writer` is to the user: this writer, or another by its label.
    fn by(&self, writer: WriterId) -> By {
        match self.writer.as_ref().map(Writer::id) == Some(writer) {
            true => By::This,
            false => By::Writer {
                writer,
                label: self
                    .reader
                    .logs()
                    .get(&writer)
                    .and_then(WriterLog::label)
                    .unwrap_or_default()
                    .to_owned(),
            },
        }
    }

    /// Rebuilds the view from the library's state.
    fn show(&mut self) {
        self.unshown = false;
        self.bind_facts();
        self.removed = self.unkept();
        let logs = self.reader.logs().values();
        let forks = logs.clone().flat_map(|log| log.forks()).copied().collect();
        let gaps = logs.flat_map(|log| log.gaps()).copied().collect();
        let mut index = Arc::clone(self.view.index());
        let shown = self.view.parts();
        Arc::make_mut(&mut index).update(
            (&shown.folded, &shown.bindings),
            (&self.folded, &self.bindings),
        );
        let parts = Parts {
            folded: self.folded.clone(),
            bindings: Arc::clone(&self.bindings),
            writers: self.writer_infos(),
            forks,
            gaps,
        };
        self.view = View::indexed(parts, index);
    }

    fn writer_infos(&self) -> Vec<WriterInfo> {
        let own = self.writer.as_ref().map(Writer::id);
        self.reader
            .logs()
            .values()
            .map(|log| WriterInfo {
                writer: log.writer(),
                label: log.label().unwrap_or_default().to_owned(),
                last_entry_at: log.last_at(),
                here: match Some(log.writer()) == own {
                    true => Presence::This,
                    false => self
                        .presence
                        .get(&log.writer())
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

/// Every writer's log, or what the folder holds of it for each writer `folder`
/// names.
fn shown_logs(reader: &Reader, folder: impl Fn(WriterId) -> bool) -> Vec<&WriterLog> {
    let logs = reader.logs().iter();
    let logs = logs.filter_map(|(writer, log)| match folder(*writer) {
        true => reader.folder_log(*writer),
        false => Some(log),
    });
    logs.collect()
}

/// The entries of `log` among `hashes`, which it placed last.
fn placed_entries<'a>(log: &'a WriterLog, hashes: &[EntryHash]) -> Vec<&'a Entry> {
    let placed = hashes.iter().filter_map(|hash| log.entry(*hash));
    placed.map(Rc::as_ref).collect()
}

/// The entities whose state folding an entry of `kind` changes.
fn entities(kind: &EntryKind) -> impl Iterator<Item = EntityId> + '_ {
    let ops = match kind {
        EntryKind::Intent(logged) => logged.ops.as_slice(),
        EntryKind::Bind(bound) => bound.ops.as_slice(),
        EntryKind::Genesis(_) | EntryKind::Settle(_) | EntryKind::Unknown(_) => &[],
    };
    ops.iter().filter_map(Op::entity)
}

/// An entry of `writer`'s own history this build does not understand.
fn newer_entry(folded: &Folded, reader: &Reader, writer: WriterId) -> Option<EntryHash> {
    let log = reader.logs().get(&writer)?;
    folded
        .unknown()
        .map(|(entry, _)| *entry)
        .find(|entry| log.holds(*entry))
}

/// How far past this machine's wall clock a reading in the folder may be and still
/// move this writer's clock.
const MAX_DRIFT_MS: u64 = 24 * 60 * 60 * 1000;

/// Every reading of the placed entries and kept snapshots.
fn readings(reader: &Reader) -> impl Iterator<Item = Hlc> + '_ {
    reader.logs().values().flat_map(WriterLog::readings)
}

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
        Op::Unknown(_) => return None,
    };
    Some(Change {
        entity,
        what,
        by: By::This,
    })
}

/// About how many entries one slice of placing offers.
const PLACE_SLICE: usize = 1 << 11;

/// About how many ops one slice of folding applies.
const FOLD_SLICE: usize = 1 << 12;

/// How many entities' file facts one slice takes.
const FILE_SLICE: usize = 1 << 12;

/// About how many entities or files one slice of binding takes.
const BIND_SLICE: usize = 1 << 12;

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

/// Folds what a library just opened shows, a slice at a time, unless it
/// `resumed` from the state its cached view kept, then follows its clock readings
/// and decides whether it may write.
fn fold_opened(mut library: Library, resumed: bool) -> Flow<'static, Library> {
    let folded = match resumed {
        true => Flow::Done(library),
        false => {
            let merging = library.unfold();
            flow::sliced(merging, |merging| merging.step(FOLD_SLICE)).then(move |merging| {
                library.folded = merging.folded();
                Flow::Done(library)
            })
        }
    };
    folded
        .then(|library| {
            let filing = (library, Facts::new(), None);
            flow::sliced(filing, |(library, facts, after)| {
                library.folded.files_slice(facts, after, FILE_SLICE)
            })
        })
        .then(|(mut library, facts, _)| {
            library.refiled(facts);
            library.follow(readings(&library.reader).collect::<Vec<_>>());
            library.mode = library.mode_at_open();
            Flow::Done(library)
        })
}

/// Reads every writer's pending records and sorts out this writer's and the
/// orphans. Writes nothing.
fn recover(library: Library) -> Fallible<'static, Library> {
    let writers = library.reader.logs().keys().copied().collect();
    flow::run(recovery::read(&library.layout, writers)).and_then(move |found| {
        let mut library = library;
        let own = library
            .writer
            .as_ref()
            .map(|writer| (writer.id(), writer.head()));
        let settled = library.folded.settlements().clone();
        let (assessed, open) =
            recovery::classify(&library.layout, own, library.reader.logs(), &settled, found);
        flow::run(recovery::predict(&library.layout, assessed, open)).map_ok(move |assessed| {
            library.unsettled = assessed.own;
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
    let known = library.borrow();
    let found = known.walking.as_ref().map(Walk::found).unwrap_or_default();
    let identify = binding::identify(found, &known.env.identify, &known.facts, &known.scan);
    flow::run(identify).and_then(move |(scan, read)| {
        let mut library = library;
        let known = library.borrow_mut();
        known.walking = None;
        if !known.unscanned.is_empty() {
            known.unscanned = Unscanned::default();
            known.bind_due = true;
        }
        known.behind.remove(&Lag::Scan);
        rebound(library, scan).then(move |library| {
            remember(library, read).then(|(library, kept)| Flow::Done(kept.map(|()| library)))
        })
    })
}

/// Binds the facts to `scan` a slice at a time, unless neither changed.
fn rebound<'a, L: BorrowMut<Library> + 'a>(mut library: L, scan: Scan) -> Flow<'a, L> {
    let known = library.borrow_mut();
    if !known.bind_due && scan == known.scan {
        return Flow::Done(library);
    }
    known.scan = scan;
    let binding = Binding::new(&known.facts, &known.scan, &known.unscanned);
    flow::sliced((library, binding), |(library, binding)| {
        let known = library.borrow();
        binding.step(&known.facts, &known.scan, &*known.env.names, BIND_SLICE)
    })
    .then(|(mut library, binding)| {
        library.borrow_mut().bound(binding);
        Flow::Done(library)
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

/// Once entries are durable, scans again only `paths`, and binds.
fn rescan_paths(library: &mut Library, paths: Vec<RelPath>) -> Fallible<'_, &mut Library> {
    let Library {
        layout,
        env,
        facts,
        scan,
        ..
    } = &*library;
    let scan = binding::rescan(layout, &env.identify, facts, scan, paths.clone());
    rebind_logged(library, scan, paths)
}

/// Once entries are durable, binds to what `scan` finds. A scan that fails does
/// not fail what was logged: `unsure`, where files may have moved, and the
/// entities bound there are unknown until a scan of every file succeeds.
/// Identities it fails to keep in the local root are kept after the next scan.
fn rebind_logged<'a>(
    library: &'a mut Library,
    scan: Task<'static, Result<(Scan, Vec<RelPath>)>>,
    unsure: Vec<RelPath>,
) -> Fallible<'a, &'a mut Library> {
    if let Some(walking) = library.walking.as_mut() {
        walking.forget(&unsure);
    }
    flow::run(scan).then(move |scanned| match scanned {
        Ok((scan, read)) => rebound(library, scan)
            .then(move |library| remember(library, read).then(|(library, _)| ok(library))),
        Err(error) => {
            library.unscanned.add(unsure, &library.bindings);
            library.lag(Lag::Scan, &Err(error));
            library.bind_due = true;
            ok(library)
        }
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
                ops: facts,
                displaced: Vec::new(),
                reverses,
            };
            transact(library, logged, effects, None, pins.collect())
        })
        .and_then(move |(library, entries, outcome)| {
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
        let Ok(genesis) = Entry::encode(EntryHash::ZERO, at, kind) else {
            return Flow::Done(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong))));
        };
        let segment = library.env.segment_name();
        let whole = library.shown_as_folder().is_empty();
        let (reader, folded) = (&mut library.reader, &library.folded);
        let create = Writer::create(library.layout.clone(), id, segment, label, at, |genesis| {
            reader.checkpoint(genesis, whole.then_some(folded))
        });
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
/// leave. A record stays to settle while settling it fails, and to remove while
/// removing it fails. One whose effects stop partway is logged as far as they got,
/// removed, and fails the settling with [`Error::Unfinished`].
fn settle_own(library: &mut Library) -> Fallible<'_, &mut Library> {
    let open = library.unsettled.clone();
    flow::fold(open.into_iter(), library, settle_one).map_ok(|library| {
        library.show();
        library
    })
}

fn settle_one(library: &mut Library, settling: Settling) -> Fallible<'_, &mut Library> {
    let layout = library.layout.clone();
    let pending = settling.pending.clone();
    let name = settling.record;
    type Appended = Option<(String, Vec<Entry>, Outcome)>;
    let settled: Fallible<'_, (&mut Library, Appended)> = match settling.logged {
        true => ok((library, None)),
        false => {
            let record = Rc::new(settling.pending);
            let identify = Rc::clone(&library.env.identify);
            flow::run(recovery::settle(
                &layout,
                name,
                Rc::clone(&record),
                identify,
            ))
            .and_then(move |applied| {
                let mut logged = planned(&record);
                logged.ops.extend(library.file_ops(&applied, &record.files));
                logged.displaced = applied.displaced;
                let label = logged.label.clone();
                append(library, vec![EntryKind::Intent(logged)]).and_then(
                    move |(library, entries)| {
                        let scan = library.scan_task();
                        rebind_logged(library, scan, record.paths()).map_ok(move |library| {
                            (library, Some((label, entries, applied.outcome)))
                        })
                    },
                )
            })
        }
    };
    settled.and_then(move |(library, appended)| {
        flow::run(effects::finish(&layout, name, &pending)).then(move |removed| {
            library.closed(name, &removed);
            let partial = appended.and_then(|(label, entries, outcome)| {
                committed(label, &entries, Vec::new(), outcome, library.local()).err()
            });
            match partial {
                None => ok(library),
                Some(partial) => {
                    library.show();
                    Flow::Done(Err(Error::Unfinished(partial)))
                }
            }
        })
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
    flow::run(recovery::tidy(&layout, writer))
        .and_then(move |()| flow::run(recovery::remove_empty(&layout, writer, &ignored)))
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
            reverses: None,
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
                        .then(move |(library, _)| Flow::Done((library, Ok(entries))))
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
/// lags, then keeps what was let go when that lags, each whatever became of the
/// one before. Each failure leaves its part behind, to save again the next time;
/// the first is returned. A crash meanwhile loses nothing the folder holds:
/// reopening reads the folder into the view, which only grows, and continues from
/// the last entry of the writer's chain there.
fn keep_local(library: &mut Library, head: bool) -> Flow<'_, (&mut Library, Result<()>)> {
    let saved = library.save_view();
    flow::run(saved).then(move |saved| {
        library.lag(Lag::View, &saved);
        let due = head || library.behind.contains_key(&Lag::Head);
        let recorded = match &library.writer {
            Some(writer) if due => writer.record(),
            _ => Task::ready(Ok(())),
        };
        flow::run(recorded).then(move |recorded| {
            library.lag(Lag::Head, &recorded);
            let let_go = match library.behind.contains_key(&Lag::LetGo) {
                true => library.keep_let_go(),
                false => Task::ready(Ok(())),
            };
            flow::run(let_go).then(move |kept| {
                library.lag(Lag::LetGo, &kept);
                Flow::Done((library, saved.and(recorded).and(kept)))
            })
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
    let Ok(entry) = Entry::encode(head, at, EntryKind::Intent(logged.clone())) else {
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
                        library.interrupted(name, &record, false);
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
                    library.interrupted(name, &record, false);
                }
                Flow::Done(Err(error))
            })
        }
        Err(error) => Flow::Done(Err(error)),
    })
}

/// Appends the intent `logged` with what `applied` says the effects did, closed
/// by `closing` if they complete and followed by `bind` entries pinning `pins`,
/// removes the record once the entries are durable, and scans again the paths the
/// effects moved files from and to. Once the entries are durable nothing fails.
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
    logged.displaced = applied.displaced;
    let outcome = applied.outcome;
    let closing = closing.filter(|_| outcome == Outcome::Complete);
    let mut kinds = closed(logged, closing);
    kinds.extend(bound(pins));
    let touched = moved_paths(&effects.steps);
    let layout = library.layout.clone();
    appending(library, kinds).then(move |(library, appended)| {
        let entries = match appended {
            Ok(entries) => entries,
            Err(error) => {
                let logged = library
                    .writer
                    .as_ref()
                    .is_some_and(|writer| writer.head() != record.after());
                if journaled {
                    library.interrupted(name, &record, logged);
                }
                return Flow::Done(Err(error));
            }
        };
        let finish = match journaled {
            true => effects::finish(&layout, name, &record),
            false => Task::ready(Ok(())),
        };
        flow::run(finish)
            .then(move |removed| {
                if removed.is_err() {
                    library.interrupted(name, &record, true);
                    library.closed(name, &removed);
                }
                rescan_paths(library, touched)
            })
            .map_ok(move |library| {
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
                        reverses: None,
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
