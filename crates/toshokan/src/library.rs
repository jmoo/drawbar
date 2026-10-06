//! The library as one instance holds it: the pure core both drivers run.
//!
//! Only [`Library::commit`] (and undo, redo and settling, which commit),
//! [`Library::empty_trash`] and [`Library::compact`] write the folder. Opening,
//! viewing, refreshing and scanning never do; they write only in the local root.
//!
//! Every operation is a chain of requests. Each step that needs I/O runs as a task
//! that owns what it reads, and its result is folded into the library afterwards,
//! so no task borrows the library while it runs.

use std::borrow::BorrowMut;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::binding::{self, Bindings, Scan};
use crate::drafts::{self, DraftRecord};
use crate::effects::{self, Applied, EffectPlan, FileEnd};
use crate::env::Env;
use crate::error::{Error, Invalid, Refusal, Result, Why};
use crate::flow::{self, fold, ok, Fallible, Flow};
use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::intent;
use crate::io::{Capabilities, Io, Kind, Lock, Root, Task};
use crate::layout::Layout;
use crate::log::{Entry, EntryKind, Genesis, Logged, Op, Settle, Settlement};
use crate::merge::{merge, Folded, Part};
use crate::path::RelPath;
use crate::pending::{self, PendingRecord};
use crate::plan::{FactChange, FileChange, Plan, Target};
use crate::reader::{CachedView, ReadReport, Reader, WriterLog};
use crate::recovery::{self, Settling};
use crate::report::{
    By, Change, Committed, Compacted, DraftState, DraftStatus, Emptied, HistoryItem, Mode, Opened,
    Orphan, Outcome, Presence, Settled, Start, TrashItem, What, WriterInfo,
};
use crate::schema::Schema;
use crate::trash::{self, Policy};
use crate::undo::History;
use crate::view::{FileState, Parts, View};
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
    /// This writer's interrupted effects, settled before its next write.
    unsettled: Vec<Settling>,
    orphaned: Vec<Orphan>,
    folded: Folded,
    scan: Scan,
    bindings: Bindings,
    presence: BTreeMap<WriterId, Presence>,
    history: History,
    view: View,
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
    /// Claims a writer from the pool, reads its cached view and the folder,
    /// assesses recovery, scans the library's files and checks drafts.
    /// `capabilities` are the folder's.
    pub fn open(
        layout: Layout,
        schema: Schema,
        env: Env,
        capabilities: Capabilities,
    ) -> Task<'static, Result<(Library, Opened)>> {
        flow::run(Writer::pick())
            .and_then(|picked| {
                let cached = match &picked {
                    Some(picked) => CachedView::load(picked.genesis()),
                    None => flow::run(Writer::retired())
                        .and_then(|retired| flow::run(CachedView::load_all(retired)))
                        .task(),
                };
                flow::run(cached).then(move |cached| match cached {
                    Ok(cached) => ok((picked, cached)),
                    Err(Error::Corrupt { .. }) => ok((picked, CachedView::default())),
                    Err(error) => Flow::Done(Err(error)),
                })
            })
            .and_then(move |(picked, cached)| {
                let mut reader = Reader::new(layout.clone(), cached);
                flow::run(reader.list()).and_then(move |listing| {
                    let report = reader.absorb(listing);
                    let claimed = match picked {
                        Some(picked) => picked.resume(layout.clone(), reader.logs()),
                        None => Task::ready(Ok(Claimed {
                            writer: None,
                            start: Start::New,
                        })),
                    };
                    flow::run(claimed).map_ok(move |claimed| {
                        let library =
                            Library::new(layout, schema, env, capabilities, reader, claimed.writer);
                        (library, report, claimed.start)
                    })
                })
            })
            .and_then(|(library, report, start)| {
                let saved = library.save_view(&report);
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
    ) -> Self {
        let folded = merge(reader.logs().values());
        let mode = match (&writer, capabilities.append && capabilities.rename_file) {
            (_, false) => Mode::ReadOnly(Why::FolderNotWritable),
            (Some(writer), true) => match newer_entry(&folded, &reader, writer.id()) {
                Some(entry) => Mode::ReadOnly(Why::NewerOwnHistory { entry }),
                None => Mode::Writable,
            },
            (None, true) => Mode::Writable,
        };
        let clock = latest(&reader);
        let mut library = Self {
            layout,
            schema,
            env,
            capabilities,
            mode,
            reader,
            writer,
            clock,
            unsettled: Vec::new(),
            orphaned: Vec::new(),
            folded,
            scan: Scan::default(),
            bindings: Bindings::default(),
            presence: BTreeMap::new(),
            history: History::default(),
            view: View::new(Parts {
                folded: Folded::default(),
                bindings: Bindings::default(),
                writers: Vec::new(),
                forks: Vec::new(),
                gaps: Vec::new(),
            }),
        };
        library.show();
        library
    }

    pub fn view(&self) -> View {
        self.view.clone()
    }

    pub fn history(&self) -> &[HistoryItem] {
        self.history.items()
    }

    /// Checks every precondition against the current view and the files, then
    /// logs the intent and carries out its file effects. Before its first write a
    /// new writer is created; before any write this writer's interrupted effects
    /// are settled. A refusal is [`crate::Error::Refused`] and changes nothing.
    pub fn commit(
        &mut self,
        plan: std::result::Result<Plan, Invalid>,
    ) -> Task<'_, Result<Committed>> {
        let resolved = self
            .writable()
            .and_then(|()| plan.map_err(|invalid| Error::Refused(Refusal::Invalid(invalid))))
            .and_then(|plan| self.resolve(plan));
        match resolved {
            Ok(resolved) => commit(self, resolved).task(),
            Err(error) => Task::ready(Err(error)),
        }
    }

    pub fn undo(&mut self) -> Task<'_, Result<Committed>> {
        let plan = self.own_entries().and_then(|(writer, own)| {
            self.history
                .plan_undo(&own, writer, &self.view)
                .map_err(Error::Refused)
        });
        self.commit_planned(plan)
    }

    pub fn redo(&mut self) -> Task<'_, Result<Committed>> {
        let plan = self.own_entries().and_then(|(writer, own)| {
            self.history
                .plan_redo(&own, writer, &self.view)
                .map_err(Error::Refused)
        });
        self.commit_planned(plan)
    }

    fn commit_planned(&mut self, plan: Result<Plan>) -> Task<'_, Result<Committed>> {
        match plan {
            Ok(plan) => self.commit(Ok(plan)),
            Err(error) => Task::ready(Err(error)),
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
            .map(|log| log.entries().to_vec())
            .unwrap_or_default();
        Ok((writer.id(), own))
    }

    /// Settles another writer's unfinished effect, with the user's consent, in
    /// this writer's own log: finishes it, rolls it back or dismisses it. Nothing
    /// is written in the other writer's directory.
    pub fn settle(&mut self, orphan: Orphan, how: Settlement) -> Task<'_, Result<()>> {
        if let Err(error) = self.writable() {
            return Task::ready(Err(error));
        }
        flow::run(pending::read_one(
            &self.layout,
            orphan.writer,
            orphan.record,
        ))
        .and_then(move |theirs| {
            let open = theirs.filter(|theirs| self.is_orphan(orphan.record, theirs));
            let Some(theirs) = open else {
                return Flow::Done(Err(Error::Refused(Refusal::Nothing)));
            };
            flow::run(recovery::progress(&self.layout, &theirs)).and_then(move |progress| {
                let plan =
                    recovery::orphan_plan(&self.layout, &theirs, &progress, how, &mut self.env);
                settle_orphan(self, orphan, theirs, how, plan)
            })
        })
        .task()
    }

    /// Reads other writers' new entries and rescans: what changed since the last
    /// view, attributed to its writer, or to nobody for outside changes. Writes
    /// nothing in the folder.
    pub fn refresh(&mut self) -> Task<'_, Result<Vec<Change>>> {
        let before = self.bindings.bound.clone();
        flow::run(self.reader.list())
            .and_then(move |listing| {
                let report = self.reader.absorb(listing);
                let before =
                    std::mem::replace(&mut self.folded, merge(self.reader.logs().values()));
                let changes = self.attribute(&before);
                self.clock = self.clock.observe(latest(&self.reader));
                let forked = self
                    .writer
                    .as_ref()
                    .is_some_and(|own| report.forks.iter().any(|fork| fork.writer == own.id()));
                let saved = flow::run(self.save_view(&report));
                let stopped = match forked {
                    true => self.stop_writing(),
                    false => ok(()),
                };
                saved
                    .and_then(move |()| stopped)
                    .and_then(move |()| rescan(self).map_ok(move |library| (library, changes)))
            })
            .map_ok(move |(library, mut changes)| {
                let explained: BTreeSet<EntityId> = changes
                    .iter()
                    .filter(|change| change.what == What::File)
                    .map(|change| change.entity)
                    .collect();
                for (entity, file) in &library.bindings.bound {
                    if before.get(entity) != Some(file) && !explained.contains(entity) {
                        changes.push(Change {
                            entity: *entity,
                            what: What::File,
                            by: By::Outside,
                        });
                    }
                }
                library.show();
                changes
            })
            .task()
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

    /// Folds this writer's own entries into a snapshot and deletes the segments
    /// this process sealed that it folds. Undo reaches back only to the snapshot.
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
                let own = &self.reader.logs()[&id];
                let name = self.env.nonce();
                flow::run(crate::compaction::compact(writer, own, name)).then(
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
                                self.folded = merge(self.reader.logs().values());
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

    /// Seals the open segment, writes the cached view and releases the writer's
    /// lock. A library dropped without closing leaves its segment open, as a crash
    /// does.
    pub fn close(&mut self) -> Task<'_, Result<()>> {
        let Some(mut writer) = self.writer.take() else {
            return Task::ready(Ok(()));
        };
        let saved = self.reader.cached().save(writer.genesis());
        flow::run(saved)
            .and_then(move |()| flow::run(writer.close()))
            .task()
    }

    /// Keeps the cached view in this writer's local directory after a read that
    /// changed it, so a crash does not take back what was shown.
    fn save_view(&self, report: &ReadReport) -> Task<'static, Result<()>> {
        match (&self.writer, report.changed) {
            (Some(writer), true) => self.reader.cached().save(writer.genesis()),
            _ => Task::ready(Ok(())),
        }
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
    /// changes into effects. Draws the ids of created entities.
    fn resolve(&mut self, plan: Plan) -> Result<Resolved> {
        let refused = |invalid| Error::Refused(Refusal::Invalid(invalid));
        let created: Vec<EntityId> = (0..plan.creates).map(|_| self.env.entity_id()).collect();
        let facts = intent::ops(&plan, &created, &self.folded, &self.schema).map_err(refused)?;
        let revived: BTreeSet<EntityId> = plan
            .facts
            .iter()
            .filter_map(|change| match change {
                FactChange::Revive { entity } => Some(*entity),
                _ => None,
            })
            .collect();
        for change in &plan.files {
            let entity = match change {
                FileChange::Save {
                    entity: Target::Existing(entity),
                    ..
                } => *entity,
                _ => continue,
            };
            if !self.folded.present(entity) && !revived.contains(&entity) {
                return Err(refused(Invalid::NoEntity(entity)));
            }
        }
        let effects = effects::resolve(
            &plan.files,
            &created,
            &self.bindings,
            &self.layout,
            self.capabilities,
            &mut self.env,
        )
        .map_err(Error::Refused)?;
        Ok(Resolved {
            label: plan.label,
            created,
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

    /// The file-register writes that pin this writer's derived bindings, but for
    /// entities whose files the intent changes itself.
    fn pins(&self, effects: &EffectPlan) -> Vec<Op> {
        let changed: BTreeSet<EntityId> = effects.files.iter().map(|end| end.entity).collect();
        binding::pins(&self.folded.files(), &self.bindings, &self.scan)
            .into_iter()
            .filter(|op| !matches!(op, Op::Pin { entity, .. } if changed.contains(entity)))
            .collect()
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
    /// before its next write; one of a writer it stopped writing as is an orphan.
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

    /// Places entries this instance just appended and folds them in.
    fn absorb_own(&mut self, entries: &[Entry]) {
        let Some(id) = self.writer.as_ref().map(Writer::id) else {
            return;
        };
        self.reader.add(id, entries);
        for entry in entries {
            self.folded.apply(id, entry);
            self.clock = self.clock.observe(entry.at);
        }
    }

    /// Stops writing as this writer: it leaves the pool, and the next commit
    /// creates a new one. Its open interrupted effects become orphans, settled
    /// only with consent.
    fn stop_writing<'a>(&mut self) -> Fallible<'a, ()> {
        let Some(writer) = self.writer.take() else {
            return ok(());
        };
        let open = std::mem::take(&mut self.unsettled).into_iter();
        for settling in open.filter(|settling| !settling.logged) {
            self.interrupted(settling.record, &settling.pending, false);
        }
        flow::run(writer::retire_writer(writer.genesis()))
    }

    fn rebind(&mut self, scan: Scan) {
        self.bindings = binding::bind(&self.folded.files(), &scan, &*self.env.names);
        self.scan = scan;
    }

    fn scan_task(&self) -> Task<'static, Result<Scan>> {
        binding::scan(
            &self.layout,
            &self.env.identify,
            &self.folded.files(),
            &self.scan,
        )
    }

    /// Rebuilds the history and the view from the library's state.
    fn show(&mut self) {
        self.history = match &self.writer {
            Some(writer) => self
                .reader
                .logs()
                .get(&writer.id())
                .map(|log| History::of(log.entries()))
                .unwrap_or_default(),
            None => History::default(),
        };
        let logs = self.reader.logs().values();
        let forks = logs.clone().flat_map(|log| log.forks()).copied().collect();
        let gaps = logs.flat_map(|log| log.gaps()).copied().collect();
        self.view = View::new(Parts {
            folded: self.folded.clone(),
            bindings: self.bindings.clone(),
            writers: self.writer_infos(),
            forks,
            gaps,
        });
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
        let own = self.writer.as_ref().map(Writer::id);
        let mut changes: Vec<Change> = Vec::new();
        for (entity, part, writer) in self.folded.since(before) {
            let by = match Some(writer) == own {
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
            };
            let what = match part {
                Part::Created => What::Created,
                Part::Deleted => What::Deleted,
                Part::Field(key) => What::Field(key),
                Part::File => What::File,
            };
            let change = Change { entity, what, by };
            if !changes.contains(&change) {
                changes.push(change);
            }
        }
        changes
    }
}

/// An entry of `writer`'s own history this build does not understand.
fn newer_entry(folded: &Folded, reader: &Reader, writer: WriterId) -> Option<EntryHash> {
    let log = reader.logs().get(&writer)?;
    folded
        .unknown()
        .map(|(entry, _)| *entry)
        .find(|entry| log.holds(*entry))
}

fn latest(reader: &Reader) -> Hlc {
    reader
        .logs()
        .values()
        .filter_map(|log| log.last_at())
        .max()
        .unwrap_or(Hlc::ZERO)
}

fn changes_of(ops: &[Op], by: &By, changes: &mut Vec<Change>) {
    for op in ops {
        let (entity, what) = match op {
            Op::Create { entity, .. } => (*entity, What::Created),
            Op::Delete { entity, .. } => (*entity, What::Deleted),
            Op::Write { entity, key, .. }
            | Op::Add { entity, key, .. }
            | Op::Remove { entity, key, .. } => (*entity, What::Field(key.clone())),
            Op::File { entity, .. } | Op::Pin { entity, .. } => (*entity, What::File),
            Op::Unknown(_) => continue,
        };
        let change = Change {
            entity,
            what,
            by: by.clone(),
        };
        if !changes.contains(&change) {
            changes.push(change);
        }
    }
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
            library
        })
    })
}

/// Scans the library's files and binds them.
fn rescan<'a, L: BorrowMut<Library> + 'a>(library: L) -> Fallible<'a, L> {
    let scan = library.borrow().scan_task();
    flow::run(scan).map_ok(move |scan| {
        let mut library = library;
        library.borrow_mut().rebind(scan);
        library
    })
}

/// Commits `resolved`: creates the writer if there is none, settles this writer's
/// interrupted effects, then logs the intent and carries out its file effects.
fn commit<'a>(library: &'a mut Library, resolved: Resolved) -> Fallible<'a, Committed> {
    let Resolved {
        label,
        created,
        facts,
        effects,
        reverses,
    } = resolved;
    ensure_writer(library, &effects)
        .and_then(settle_own)
        .and_then(move |library| {
            let mut ops = facts;
            ops.extend(library.pins(&effects));
            let logged = Logged {
                label,
                ops,
                displaced: Vec::new(),
                reverses,
            };
            transact(library, logged, effects, Vec::new())
        })
        .map_ok(move |(_, entry, outcome)| {
            let mut changes = Vec::new();
            if let EntryKind::Intent(logged) = &entry.kind {
                changes_of(&logged.ops, &By::This, &mut changes);
            }
            Committed {
                intent: entry.hash(),
                created,
                changes,
                outcome,
            }
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
        let Ok(genesis) = Entry::encode(EntryHash::ZERO, at, kind) else {
            return Flow::Done(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong))));
        };
        let segment = library.env.segment_name();
        let create = Writer::create(library.layout.clone(), id, segment, label, at);
        flow::run(create).and_then(move |writer| {
            let local = writer.genesis();
            library.writer = Some(writer);
            library.absorb_own(&[genesis]);
            flow::run(library.reader.cached().save(local)).map_ok(move |()| library)
        })
    })
}

/// Settles this writer's interrupted effects, oldest first, then removes staging
/// no record places. A record stays to settle while settling it fails.
fn settle_own(library: &mut Library) -> Fallible<'_, &mut Library> {
    let Some(settling) = library.unsettled.first().cloned() else {
        let Some(writer) = library.writer.as_ref().map(Writer::id) else {
            return ok(library);
        };
        return flow::run(recovery::tidy(&library.layout, writer, &[])).map_ok(move |()| library);
    };
    let layout = library.layout.clone();
    let PendingRecord { writer, .. } = settling.pending;
    let name = settling.record;
    let settled = match settling.logged {
        true => ok(library),
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
                append(library, vec![EntryKind::Intent(logged)])
                    .and_then(|(library, _)| rescan(library))
            })
        }
    };
    settled.and_then(move |library| {
        flow::run(effects::finish(&layout, writer, name)).and_then(move |()| {
            library.unsettled.retain(|settling| settling.record != name);
            settle_own(library)
        })
    })
}

/// The intent a pending record planned to log, without the file ops its steps
/// decide. A record whose entry is not an intent logs its label alone.
fn planned(record: &PendingRecord) -> Logged {
    let decoded = Entry::decode(record.entry.clone())
        .ok()
        .map(|entry| entry.kind);
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

/// As [`append`], with the library back whatever happened. A writer that cannot
/// continue its history stops writing: the next commit creates another.
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
                library.absorb_own(&entries);
                Flow::Done((library, Ok(entries)))
            }
            Err(error @ Error::Rekey { .. }) => {
                let stopped = library.stop_writing();
                stopped.then(move |stopped| Flow::Done((library, stopped.and(Err(error)))))
            }
            Err(error) => Flow::Done((library, Err(error))),
        }
    })
}

/// Logs `logged`, carrying out `effects` under a pending record when there are
/// any, then appends `after`. Returns the intent's entry and how the effects
/// ended. An interrupted run leaves the record for the next write to settle.
fn transact<'a>(
    library: &'a mut Library,
    logged: Logged,
    effects: Rc<EffectPlan>,
    after: Vec<EntryKind>,
) -> Fallible<'a, (&'a mut Library, Entry, Outcome)> {
    if effects.is_empty() {
        let mut kinds = vec![EntryKind::Intent(logged)];
        kinds.extend(after);
        return append(library, kinds).map_ok(|(library, entries)| {
            library.rebind(library.scan.clone());
            library.show();
            let entry = entries.into_iter().next().expect("the intent was appended");
            (library, entry, Outcome::Complete)
        });
    }
    carry_out(library, &logged, Rc::clone(&effects)).and_then(move |(library, record, applied)| {
        log_effects(library, logged, &effects, record, applied, after)
    })
}

/// Stages, checks and journals `effects` for the intent `logged`, then carries
/// them out. A refusal changes nothing; a failure past the journal leaves its
/// record to settle.
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
    let record = Rc::new(PendingRecord::new(id, &logged.label, entry.line, &effects));
    let layout = library.layout.clone();
    let identify = Rc::clone(&library.env.identify);
    let (name, journaled) = (effects.record, effects.moves_files());
    let prepared = effects::prepare(&layout, effects, Rc::clone(&record), Rc::clone(&identify));
    flow::run(prepared)
        .and_then(|prepared| Flow::Done(prepared.map_err(Error::Refused)))
        .and_then({
            let record = Rc::clone(&record);
            move |()| flow::run(effects::apply(&layout, name, record, 0, identify))
        })
        .then(move |applied| match applied {
            Ok(applied) => ok((library, record, applied)),
            Err(error) => {
                if journaled {
                    library.interrupted(name, &record, false);
                }
                Flow::Done(Err(error))
            }
        })
}

/// Appends the intent `logged` with what `applied` says the effects did, then
/// `after`, and removes the record once the entries are durable.
fn log_effects<'a>(
    library: &'a mut Library,
    mut logged: Logged,
    effects: &EffectPlan,
    record: Rc<PendingRecord>,
    applied: Applied,
    after: Vec<EntryKind>,
) -> Fallible<'a, (&'a mut Library, Entry, Outcome)> {
    let (name, journaled) = (effects.record, effects.moves_files());
    logged.ops.extend(library.file_ops(&applied, &record.files));
    logged.displaced = applied.displaced;
    let outcome = match applied.outcome {
        Outcome::Partial(report) => Outcome::Partial(crate::report::PartialReport {
            record: None,
            ..report
        }),
        Outcome::Complete => Outcome::Complete,
    };
    let mut kinds = vec![EntryKind::Intent(logged)];
    kinds.extend(after);
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
        let entry = entries.into_iter().next().expect("the intent was appended");
        let finish = match journaled {
            true => effects::finish(&layout, record.writer, name),
            false => Task::ready(Ok(())),
        };
        flow::run(finish)
            .then(move |finished| match finished {
                Ok(()) => rescan(library),
                Err(error) => {
                    library.interrupted(name, &record, true);
                    Flow::Done(Err(error))
                }
            })
            .map_ok(move |library| {
                library.show();
                (library, entry, outcome)
            })
    })
}

/// Settles another writer's record `theirs` as `how` with `plan`, then logs the
/// settlement. Finishing logs the facts the other writer planned with it.
fn settle_orphan(
    library: &mut Library,
    orphan: Orphan,
    theirs: PendingRecord,
    how: Settlement,
    plan: EffectPlan,
) -> Fallible<'_, ()> {
    let effects = Rc::new(plan);
    let facts = match how {
        Settlement::Finished => planned(&theirs).ops,
        Settlement::RolledBack | Settlement::Dismissed => Vec::new(),
    };
    let settle = EntryKind::Settle(Settle {
        writer: orphan.writer,
        record: orphan.record,
        outcome: how,
    });
    ensure_writer(library, &effects)
        .and_then(settle_own)
        .and_then(move |library| {
            let logged: Fallible<'_, &mut Library> = match effects.is_empty() && facts.is_empty() {
                true => append(library, vec![settle]).map_ok(|(library, _)| library),
                false => {
                    let logged = Logged {
                        label: theirs.label,
                        ops: facts,
                        displaced: Vec::new(),
                        reverses: None,
                    };
                    transact(library, logged, effects, vec![settle]).map_ok(|(library, ..)| library)
                }
            };
            logged.map_ok(move |library| {
                library.rebind(library.scan.clone());
                library.show();
                library
                    .orphaned
                    .retain(|o| (o.writer, o.record) != (orphan.writer, orphan.record));
            })
        })
}
