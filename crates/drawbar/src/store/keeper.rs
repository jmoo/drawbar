//! A library kept by toshokan's async driver, over whatever storage the backend has: the
//! desktop's folder on its backend thread, or the browser's in a worker. Every change the
//! app asks for is an intent here, and every file it writes is filled from a [`Fill`] as
//! toshokan stages it.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::rc::Rc;
use std::sync::Arc;

use toshokan::asynch::{Filling as Filled, Fs, Library, Source as Filler, Staging};
use toshokan::error::{Refusal, Why};
use toshokan::io::{Io, IoError, Kind, Lock, Range, Reply, Root};
use toshokan::log::Settlement;
use toshokan::plan::FactChange;
use toshokan::report::{
    Item, ItemChange, ItemKind, Mode, Refreshed, Rekey, Start, TrashItem, What,
};
use toshokan::{EntryHash, Env, Error, Expect, Nonce, RelPath, Set, View, WriterId};

use super::keys::{self, Came, ORIGIN, TAGS, TAG_NAMES, TAKEN_IN};
use super::shown::Change;
use super::storage::{self, CHUNK};
use super::unsaved::{self, Base, Of};
use super::{
    Cmd, Event, Failure, Fingerprint, Found, Intent, Kept, KeptKey, Key, Leftover, LibPath, Opened,
    Part, Source, Stat, Unwritable, Viewed, Waiting,
};
use crate::rewrite::{self, Pieces};

/// The most entries a listing of the library's folders looks at: a guard against a
/// folder no library is, such as a whole disk.
const MOST_ENTRIES: usize = 1_000_000;

/// What other writers see this one called. The folder may be shared, so it says no
/// more than which app wrote.
pub(super) const LABEL: &str = "drawbar";

/// The most bytes of this session's undo the trash holds.
const TRASH: u64 = 1 << 30;

/// What toshokan is told of the volume and the time, for a library drawbar keeps.
pub fn env(
    clock: Box<dyn toshokan::Clock>,
    random: Box<dyn toshokan::Random>,
    names: Box<dyn toshokan::Names>,
) -> Env {
    Env {
        clock,
        random,
        identify: Rc::new(keys::Contents),
        names,
        label: LABEL.to_string(),
    }
}

/// One clock for toshokan and the unsaved edits.
struct Shared(Rc<RefCell<Box<dyn toshokan::Clock>>>);

impl toshokan::Clock for Shared {
    fn now_ms(&mut self) -> u64 {
        self.0.borrow_mut().now_ms()
    }
}

pub struct Keeper<F> {
    /// The storage and what toshokan is told, until the library opens over them.
    unopened: Option<(F, Env)>,
    library: Option<Library<F>>,
    unsaved: unsaved::Store,
    /// The entries this session logged, whose undo ends with it.
    session: BTreeSet<EntryHash>,
    /// The most bytes this writer's trash holds before its oldest go.
    trash: u64,
    /// The view the app was last shown.
    shown: Option<View>,
    /// Whether the app was told the library may not be written.
    told_read_only: bool,
    /// The writer the library wrote as, as the app was last told.
    writer: Option<toshokan::WriterId>,
    /// Whether the app was told the trash could not be emptied.
    told_unemptied: bool,
    /// The lock this window holds while it has the library open.
    window: Option<RelPath>,
}

impl<F: Fs> Keeper<F> {
    /// The library on `fs`, opened by [`Cmd::Open`].
    pub fn new(fs: F, mut env: Env) -> Keeper<F> {
        let clock = Rc::new(RefCell::new(env.clock));
        env.clock = Box::new(Shared(clock.clone()));
        let instance = env.random.next_u128();
        Keeper {
            unopened: Some((fs, env)),
            library: None,
            unsaved: unsaved::Store::new(instance, Box::new(Shared(clock))),
            session: BTreeSet::new(),
            trash: TRASH,
            shown: None,
            told_read_only: false,
            writer: None,
            told_unemptied: false,
            window: None,
        }
    }

    /// Show the whole view in the next answer, as the app lost what it was last shown.
    #[cfg(any(target_arch = "wasm32", test))]
    pub fn forget_shown(&mut self) {
        self.shown = None;
    }

    /// Run a command on the library, and answer it.
    pub async fn run(&mut self, cmd: Cmd, answer: &mut impl FnMut(Event)) {
        if let Ok(fs) = storage_of(&self.library) {
            let _ = self.unsaved.let_go_of_ended(fs).await;
        }
        let changes = matches!(
            cmd,
            Cmd::Intent(_) | Cmd::Undo | Cmd::Redo | Cmd::Settle { .. }
        );
        let finished = match changes {
            true => self.finish_earlier().await,
            false => (None, Ok(())),
        };
        let finished = match finished {
            (finished, Ok(())) => finished,
            (finished, Err(why)) => return self.not_tried(&cmd, finished, &why, answer),
        };
        match cmd {
            Cmd::Open => {
                let opened = self.open().await;
                answer(Event::Opened(opened));
            }
            Cmd::Rescan | Cmd::Refresh => {
                let scanned = self.scanned(matches!(cmd, Cmd::Rescan)).await;
                // A history that forked is said with the scan's answer.
                let forked = scanned.as_ref().is_ok_and(|viewed| viewed.retired);
                if self.restarted() && !forked {
                    answer(Event::Restarted);
                }
                answer(Event::Scanned(scanned));
            }
            Cmd::Intent(intent) => {
                let label = intent.label().to_string();
                let committed = self.commit(intent, answer).await;
                self.made(label, finished, committed, false, answer).await;
            }
            Cmd::Undo => {
                let (undone, forgot) = self.undone(false).await;
                self.made("Undo".to_string(), finished, undone, forgot, answer)
                    .await;
            }
            Cmd::Redo => {
                let (redone, forgot) = self.undone(true).await;
                self.made("Redo".to_string(), finished, redone, forgot, answer)
                    .await;
            }
            Cmd::Settle { orphan, how } => {
                let settled = self.settled(orphan, how).await;
                self.made("Settle".to_string(), finished, settled, false, answer)
                    .await;
            }
            Cmd::Draft {
                of,
                base,
                bytes,
                replaces,
            } => {
                let kept = self.draft(&of, base, bytes, replaces).await;
                answer(Event::Drafted {
                    of,
                    kept: kept.map_err(|e| e.to_string()),
                });
            }
            Cmd::Discard(of) => {
                if let Err(e) = self.discard(&Of::Asset(of)).await {
                    answer(Event::Unkept(format!("letting go of an unsaved edit: {e}")));
                }
            }
            Cmd::KeepSlot { slot, base, bytes } => {
                let kept = match storage_of(&self.library) {
                    Ok(fs) => {
                        let of = Of::Slot(slot.clone());
                        self.unsaved.keep(fs, of, Base::Slot(base), bytes).await
                    }
                    Err(e) => Err(e),
                };
                answer(Event::SlotDrafted {
                    slot,
                    kept: kept.map_err(|e| e.to_string()),
                });
            }
            Cmd::DiscardSlot(slot) => {
                if let Err(e) = self.discard(&Of::Slot(slot)).await {
                    answer(Event::Unkept(format!("letting go of a slot's edit: {e}")));
                }
            }
            Cmd::ReadSlot(slot) => {
                let kept = self.read_slot(&slot).await.map_err(|e| e.to_string());
                answer(Event::SlotKept { slot, kept });
            }
            Cmd::Read { .. } => unreachable!("the backend reads files itself"),
            Cmd::Restore(from) => {
                let restored = match &self.unopened {
                    Some((fs, _)) => super::copy::restore(fs, &from).await,
                    None => Err("the library was already open".to_string()),
                };
                answer(Event::Restored(restored));
            }
        }
    }

    /// Finish first this session's own change an earlier write left unfinished, as the
    /// next write would, so the answer can say which entry logged it: none where it
    /// never landed. The result is `Err` with why it still waits, though it may have
    /// logged it.
    async fn finish_earlier(&mut self) -> (Option<EntryHash>, Result<(), String>) {
        let Some(library) = self.library.as_mut() else {
            return (None, Ok(()));
        };
        if library.finishing().is_empty() {
            return (None, Ok(()));
        }
        let before = library.last_logged();
        let settled = library.settle_interrupted().await.map_err(|e| told(&e));
        let finished = library
            .last_logged()
            .filter(|_| library.last_logged() != before);
        self.session.extend(finished);
        (finished, settled)
    }

    /// Answer the change `cmd` as one not tried, since this session's own change an
    /// earlier write left unfinished could not be finished first, for `why`.
    fn not_tried(
        &mut self,
        cmd: &Cmd,
        finished: Option<EntryHash>,
        why: &str,
        answer: &mut impl FnMut(Event),
    ) {
        let waiting = match self.waits() {
            true => Waiting::Earlier,
            false => Waiting::Nothing,
        };
        self.tell_restarted(answer);
        self.tell_read_only(answer);
        for mut event in super::refusing::refused(cmd).answers(why, true) {
            if let Event::Committed {
                finished: told,
                waiting: waits,
                ..
            } = &mut event
            {
                (*told, *waits) = (finished, waiting);
            }
            answer(event);
        }
    }

    /// Whether a change of this session's own waits for the next write to finish it.
    fn waits(&self) -> bool {
        let finishing = self.library.as_ref().map(Library::finishing);
        finishing.is_some_and(|paths| !paths.is_empty())
    }

    /// Answer a change, an undo or a redo, after saying once that the library may no
    /// longer be written, and empty the trash as [`Keeper::empty_trash`] says once it
    /// was made.
    async fn made(
        &mut self,
        label: String,
        finished: Option<EntryHash>,
        committed: Result<(Option<Made>, Viewed), String>,
        forgot: bool,
        answer: &mut impl FnMut(Event),
    ) {
        let made = committed.as_ref().ok().and_then(|(made, _)| *made);
        self.session.extend(made.map(|(entry, _)| entry));
        let waiting = match self.waits() {
            true => Waiting::This,
            false => Waiting::Nothing,
        };
        // Ahead of the change's own answer, which is all a sender waits for.
        self.tell_restarted(answer);
        self.tell_read_only(answer);
        answer(Event::Committed {
            label,
            made: made.map(|(entry, _)| entry),
            undoable: made.is_some_and(|(_, undoable)| undoable),
            forgot,
            finished,
            waiting,
            result: committed.map(|(_, viewed)| viewed),
        });
        if made.is_some() {
            if let Some(trimmed) = self.empty_trash().await {
                answer(trimmed);
            }
        }
    }

    /// Say once a session why the trash could not be emptied.
    fn unemptied(&mut self, why: String) -> Option<Event> {
        let told = std::mem::replace(&mut self.told_unemptied, true);
        (!told).then_some(Event::Unemptied(why))
    }

    /// Say that this session's undo history began again, where the library stopped
    /// writing as the writer it wrote as.
    fn tell_restarted(&mut self, answer: &mut impl FnMut(Event)) {
        if self.restarted() {
            answer(Event::Restarted);
        }
    }

    /// Whether the library stopped writing as the writer it wrote as when last asked.
    fn restarted(&mut self) -> bool {
        let writer = self.library.as_ref().and_then(Library::writer_id);
        let was = std::mem::replace(&mut self.writer, writer);
        was.is_some() && writer != was
    }

    /// Say once that the library may no longer be written, where it may not.
    fn tell_read_only(&mut self, answer: &mut impl FnMut(Event)) {
        let mode = self.library.as_ref().map(Library::mode);
        if let (Some(Mode::ReadOnly(why)), false) = (mode, self.told_read_only) {
            self.told_read_only = true;
            answer(Event::ReadOnly(read_only(why)));
        }
    }

    /// Keep in the trash only what this session's undo reaches, newest first within
    /// [`TRASH`], and say which of this session's changes lost what their undo needed.
    /// toshokan also keeps what an unsettled change may still move.
    async fn empty_trash(&mut self) -> Option<Event> {
        let library = self.library.as_mut()?;
        let writer = library.writer_id()?;
        let held = library.trash().await.ok()?;
        let kept = keeping(&held, writer, |by| self.session.contains(by), self.trash)?;
        let emptied = match library.empty_trash(kept).await {
            Ok(emptied) => emptied,
            Err(e) => return self.unemptied(told(&e)),
        };
        if let Some(record) = emptied.held.first() {
            return self.unemptied(format!(
                "“{record}” in the library folder is no record drawbar reads, and while it \
                 is there, drawbar keeps everything in its trash: move it out"
            ));
        }
        let gone = held
            .iter()
            .filter(|item| emptied.removed.contains(&item.item) && self.session.contains(&item.by));
        let gone: Vec<EntryHash> = gone.map(|item| item.by).collect();
        (!gone.is_empty()).then_some(Event::Trimmed(gone))
    }

    /// Keep `bytes` as the unsaved edit of the file `of` names, made over `base`, then
    /// let go of the edits it `replaces`.
    async fn draft(
        &mut self,
        of: &Key,
        base: toshokan::Identity,
        bytes: Vec<u8>,
        replaces: Vec<Key>,
    ) -> io::Result<()> {
        let fs = storage_of(&self.library)?;
        let unsaved = &mut self.unsaved;
        unsaved
            .keep(fs, Of::Asset(of.clone()), Base::File(base), bytes)
            .await?;
        for replaced in replaces {
            unsaved.discard(fs, &Of::Asset(replaced)).await?;
        }
        Ok(())
    }

    /// Let go of the unsaved edit of `of`.
    async fn discard(&mut self, of: &Of) -> io::Result<()> {
        let fs = storage_of(&self.library)?;
        self.unsaved.discard(fs, of).await
    }

    /// The newest edit kept of `slot`, with the hash of the slot's bytes it was made over.
    async fn read_slot(
        &mut self,
        slot: &super::Slot,
    ) -> io::Result<Option<(blake3::Hash, Vec<u8>)>> {
        let fs = storage_of(&self.library)?;
        let found = self.unsaved.read(fs, Some(&Of::Slot(slot.clone()))).await?;
        let unreadable = found.iter().any(Result::is_err);
        match unsaved::newest(found) {
            Some(unsaved::Unsaved {
                base: Base::Slot(base),
                bytes,
                ..
            }) => Ok(Some((base, bytes))),
            None if !unreadable => Ok(None),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "it is not an edit drawbar kept",
            )),
        }
    }

    /// The unsaved edits of assets an earlier session kept, each with whether its file
    /// still holds what it was made over, as `view` places it and a read of it now finds,
    /// or `Err` where the edit or its file does not read.
    async fn drafts(&mut self, view: &View) -> io::Result<Vec<Result<Kept, String>>> {
        let fs = storage_of(&self.library)?;
        let mut drafts = Vec::new();
        for found in self.unsaved.read(fs, None).await? {
            let kept = match found {
                Ok(unsaved::Unsaved {
                    of: Of::Asset(of),
                    base: Base::File(base),
                    at,
                    bytes,
                }) => match file_holds(fs, view, &of, base).await {
                    Ok(holds) => Ok(Kept {
                        of,
                        at,
                        holds,
                        bytes,
                    }),
                    Err(e) => Err(format!("its file could not be read: {e}")),
                },
                Ok(unsaved::Unsaved {
                    of: Of::Slot(_), ..
                }) => continue,
                Ok(_) => Err("it is not an edit drawbar kept".to_string()),
                Err(name) => Err(name),
            };
            drafts.push(kept);
        }
        Ok(drafts)
    }

    /// Let the library go cleanly, once every command has run: this session's undo
    /// ends now, and the trash lets go of what only it needed.
    pub async fn close(&mut self) {
        self.session.clear();
        let _ = self.empty_trash().await;
        if let Ok(fs) = storage_of(&self.library) {
            let _ = self.unsaved.let_go_of_ended(fs).await;
            if let Some(name) = self.window.take() {
                let _ = fs.perform(toshokan::Io::Unlock { name }).await;
            }
        }
        let Some(library) = self.library.take() else {
            return;
        };
        let _ = library.close().await;
    }

    async fn open(&mut self) -> Result<Opened, String> {
        let (fs, env) = self.unopened.take().ok_or("the library opened before")?;
        self.window = Some(super::copy::hold_window(&fs).await?);
        let (mut library, opened) = Library::open(fs, keys::layout(), &keys::schema(), env)
            .await
            .map_err(|e| told(&e))?;
        let mut trouble = Vec::new();
        let opening_copy = super::copy::opening(library.fs()).await;
        let opening_copy = opening_copy.unwrap_or_else(|e| {
            trouble.push(format!("looking for a copy being opened: {e}"));
            false
        });
        if opened.mode == Mode::Writable && !opening_copy {
            let finishing = |e: Error| format!("finishing a change: {}", told(&e));
            let settled = library.settle_interrupted().await;
            let failed = settled.err().filter(|e| !matches!(e, Error::ReadOnly(_)));
            trouble.extend(failed.map(finishing));
            // Another window of this install left these: they are ours to finish.
            for orphan in opened.orphaned.into_iter().filter(|orphan| orphan.ours) {
                let settled = library.settle(orphan, Settlement::Finished).await;
                trouble.extend(settled.err().map(finishing));
            }
        }
        let viewed = Viewed {
            retired: matches!(
                opened.start,
                Start::Rekeyed {
                    why: Rekey::Forked,
                    ..
                }
            ),
            ..committed_view(&library, &mut self.shown, Vec::new())
        };
        let writable = match library.mode() {
            Mode::Writable if opening_copy => Err(Unwritable::ReadOnly(format!(
                "a copy was being opened into it when drawbar stopped; open the copy again \
                 with File ▸ {}",
                crate::menu::OPEN_COPY
            ))),
            Mode::Writable => Ok(()),
            Mode::ReadOnly(why @ Why::Unfinished { .. }) => {
                Err(Unwritable::Unfinished(read_only(why)))
            }
            Mode::ReadOnly(
                why @ (Why::NewerOwnHistory { .. }
                | Why::NewerLayout { .. }
                | Why::FolderNotWritable
                | Why::ClockExhausted),
            ) => Err(Unwritable::ReadOnly(read_only(why))),
        };
        self.told_read_only = writable.is_err();
        let view = library.view();
        self.library = Some(library);
        // After settling, which may need what the trash holds.
        if writable.is_ok() {
            if let Some(Event::Unemptied(why)) = self.empty_trash().await {
                trouble.push(format!("emptying the trash: {why}"));
            }
        }
        let drafts = self.drafts(&view).await.unwrap_or_else(|e| {
            trouble.push(format!("reading the unsaved edits kept: {e}"));
            Vec::new()
        });
        Ok(Opened {
            writable,
            opening_copy,
            viewed,
            drafts,
            ignored: opened.ignored.len(),
            trouble,
        })
    }

    /// Read what other writers logged since, and scan every file where `whole`, or else
    /// only the files their entries moved.
    async fn scanned(&mut self, whole: bool) -> Result<Viewed, String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let refreshed = match whole {
            true => library.rescan().await,
            false => library.refresh().await,
        };
        let mut refreshed = refreshed.map_err(|e| told(&e))?;
        // Another window of this install left these and closed since: they are ours to
        // finish, as at open. One that will not finish is the user's to settle.
        let (ours, theirs) = std::mem::take(&mut refreshed.orphaned)
            .into_iter()
            .partition::<Vec<_>, _>(|orphan| orphan.ours);
        refreshed.orphaned = theirs;
        // Not while this session's own change waits: settling would finish it unheard.
        let ours = match library.finishing().is_empty() {
            true => ours,
            false => Vec::new(),
        };
        let mut unfinishable = Vec::new();
        for mut orphan in ours {
            match library.settle(orphan.clone(), Settlement::Finished).await {
                Ok(_) => {}
                Err(e @ Error::Refused(Refusal::Gone(_))) => unfinishable.push(Leftover {
                    orphan,
                    unfinishable: Some(told(&e)),
                }),
                Err(_) => {
                    orphan.ours = false;
                    refreshed.orphaned.push(orphan);
                }
            }
        }
        Ok(viewed(library, &mut self.shown, refreshed, unfinishable))
    }

    /// Undo, or redo where `again`, and the view after it, with the files no entity is
    /// bound to that it moved for the app to follow. One refused is let go of, which
    /// the second value says.
    async fn undone(&mut self, again: bool) -> (Result<(Option<Made>, Viewed), String>, bool) {
        let Some(library) = self.library.as_mut() else {
            return (Err("the library is not open".to_string()), false);
        };
        let committed = match again {
            true => library.redo().await,
            false => library.undo().await,
        };
        let committed = match committed {
            Ok(committed) => committed,
            // A step another drawbar changed since, or whose trash item went, can
            // never be taken back; any other refusal may pass once the files are
            // looked at again.
            Err(e @ Error::Refused(Refusal::ChangedSince { .. } | Refusal::Emptied)) => {
                match again {
                    true => library.forget_redo(),
                    false => library.forget_undo(),
                }
                return (Err(told(&e)), true);
            }
            Err(e) => return (Err(told(&e)), false),
        };
        let viewed = Viewed {
            followed: Some(followed(&committed)),
            dirs: Some(folders(library.fs()).await),
            left: shown_paths(&committed.left),
            ..committed_view(library, &mut self.shown, committed.changes)
        };
        (
            Ok((Some((committed.intent, committed.undoable)), viewed)),
            false,
        )
    }

    /// Settle another writer's unfinished change, and the view after it.
    async fn settled(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<(Option<Made>, Viewed), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let committed = library.settle(orphan, how).await.map_err(|e| told(&e))?;
        let viewed = Viewed {
            followed: Some(followed(&committed)),
            dirs: Some(folders(library.fs()).await),
            ..committed_view(library, &mut self.shown, committed.changes)
        };
        Ok((Some((committed.intent, committed.undoable)), viewed))
    }

    /// Commit an intent, and say which entry logged it, where it was made. A change to a
    /// file answers how it went first, and is answered here with the view as it stands,
    /// whether it was made or not.
    async fn commit(
        &mut self,
        intent: Intent,
        answer: &mut impl FnMut(Event),
    ) -> Result<(Option<Made>, Viewed), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let label = intent.label();
        // ⚠️ Where folders do not rename, one moves file by file as the last scan found
        // it: a look at it first takes along what came into it since.
        let mut looked = Vec::new();
        let folders = moved_folders(&intent);
        if !folders.is_empty() && !library.fs().capabilities(Root::Folder).rename_dir {
            let paths = folders.iter().filter_map(relative).collect();
            looked = library
                .rescan_paths(paths)
                .await
                .map_err(|e| told(&e))?
                .changes;
        }
        let changes = match intent {
            Intent::Tag { files, name, make } => {
                // ⚠️ A file another drawbar made an entity of since the last refresh would
                // be adopted twice, as two entities in conflict. Reading what others wrote
                // first leaves that only to one that adopts it in the moment between.
                let mut changes = Vec::new();
                if files.iter().any(|key| matches!(key, Key::File(_))) {
                    changes = library.refresh().await.map_err(|e| told(&e))?.changes;
                }
                let mut committed = facts(library, label, |built, view| {
                    let built = tag(built, view, &files, &name)?;
                    Ok(match make {
                        true => list_tag(built, view, name),
                        false => built,
                    })
                })
                .await?;
                changes.append(&mut committed.changes);
                Some(toshokan::Committed {
                    changes,
                    ..committed
                })
            }
            Intent::Untag { entities, name } => Some(
                facts(library, label, |mut built, _| {
                    for entity in entities {
                        built = built.remove(entity, TAGS, &name);
                    }
                    Ok(built)
                })
                .await?,
            ),
            Intent::MakeTag { name } => {
                let made = facts(library, label, |built, view| {
                    Ok(list_tag(built, view, name))
                });
                Some(made.await?)
            }
            Intent::RenameTag { entities, from, to } => Some(
                facts(library, label, |mut built, view| {
                    for entity in entities {
                        built = built
                            .remove(entity, TAGS, &from)
                            .add(entity, TAGS, to.clone());
                    }
                    for list in view.find(TAG_NAMES, &from) {
                        built =
                            built
                                .remove(list, TAG_NAMES, &from)
                                .add(list, TAG_NAMES, to.clone());
                    }
                    Ok(built)
                })
                .await?,
            ),
            Intent::RemoveTag { entities, name } => Some(
                facts(library, label, |mut built, view| {
                    for entity in entities {
                        built = built.remove(entity, TAGS, &name);
                    }
                    for list in view.find(TAG_NAMES, &name) {
                        built = built.remove(list, TAG_NAMES, &name);
                    }
                    Ok(built)
                })
                .await?,
            ),
            Intent::Forget { entity } => {
                let forgot = facts(library, label, |built, _| Ok(built.delete(entity)));
                Some(forgot.await?)
            }
            Intent::Take { run, changes } => {
                let view = library.view();
                let item = view.items().iter().find(|item| item.run == run);
                let item = item.ok_or("those changes are no longer kept apart")?;
                let takes = taking(&listed(item, &view), &changes).into_iter();
                let built = takes.fold(library.intent(label), |built, (change, onto)| match onto {
                    true => built.change(change),
                    false => built.take(run, change),
                });
                Some(built.commit().await.map_err(|e| told(&e))?)
            }
            Intent::Dismiss { run } => {
                let dismissed = facts(library, label, |built, _| Ok(built.dismiss(run)));
                Some(dismissed.await?)
            }
            Intent::Batch(parts) => gathered(library, label, parts, &mut looked, answer).await?,
            part => gathered(library, label, vec![part], &mut looked, answer).await?,
        };
        let intent = changes
            .as_ref()
            .map(|committed| (committed.intent, committed.undoable));
        let left = changes
            .as_ref()
            .map(|committed| shown_paths(&committed.left));
        looked.extend(changes.into_iter().flat_map(|committed| committed.changes));
        let viewed = Viewed {
            left: left.unwrap_or_default(),
            ..committed_view(library, &mut self.shown, looked)
        };
        Ok((intent, viewed))
    }
}

/// What `committed` moved, for the app to follow without a word: each tree, then each
/// file no entity is bound to that it made, moved or trashed, from and to.
fn followed(committed: &toshokan::report::Committed) -> Vec<(Option<LibPath>, Option<LibPath>)> {
    let path = |path: Option<&RelPath>| path.and_then(|path| LibPath::parse(path.as_str()));
    let trees = committed.trees.iter();
    let trees = trees.map(|(from, to)| (path(Some(from)), path(Some(to))));
    let paths = committed.paths.iter();
    let paths = paths.map(|moved| (path(moved.from.as_ref()), path(moved.to.as_ref())));
    trees.chain(paths).collect()
}

/// The entry that logged a change, and whether the change joined this session's undo.
type Made = (EntryHash, bool);

/// `paths` as the app names library paths.
fn shown_paths(paths: &[RelPath]) -> Vec<LibPath> {
    let shown = paths
        .iter()
        .filter_map(|path| LibPath::parse(path.as_str()));
    shown.collect()
}

/// The folders `intent` moves, its parts' included.
fn moved_folders(intent: &Intent) -> Vec<LibPath> {
    match intent {
        Intent::MoveFolder { from, .. } => vec![from.clone()],
        Intent::Batch(parts) => parts.iter().flat_map(moved_folders).collect(),
        _ => Vec::new(),
    }
}

/// The storage `library` is kept on, once it is open.
fn storage_of<F: Fs>(library: &Option<Library<F>>) -> io::Result<&F> {
    let library = library.as_ref();
    library
        .map(Library::fs)
        .ok_or_else(|| io::Error::other("the library is not open"))
}

/// Whether the file `of` names in `view` holds the file of identity `base`, by its
/// identity read now.
async fn file_holds<F: Fs>(
    fs: &F,
    view: &View,
    of: &Key,
    base: toshokan::Identity,
) -> io::Result<bool> {
    let at = match of {
        Key::Entity(entity) => view
            .entity(*entity)
            .and_then(|entity| entity.file())
            .filter(|file| file.state != toshokan::FileState::Missing)
            .map(|file| file.path),
        Key::File(path) => relative(path),
    };
    let Some(at) = at else {
        return Ok(false);
    };
    let Some(stat) = storage::stat(fs, &at).await? else {
        return Ok(false);
    };
    let read = async |range| storage::read(fs, Root::Folder, &at, range).await;
    Ok(storage::identity(&at, stat.len, read).await? == base)
}

type Built<'l, F> = toshokan::intent::Intent<&'l mut Library<F>>;

/// Commit an intent labeled `label` that `build` makes against the view as it stands,
/// and return what it changed.
async fn facts<'a, F: Fs + 'a>(
    library: &'a mut Library<F>,
    label: &str,
    build: impl FnOnce(Built<'a, F>, &View) -> Result<Built<'a, F>, String>,
) -> Result<toshokan::Committed, String> {
    let view = library.view();
    let built = build(library.intent(label), &view)?;
    built.commit().await.map_err(|e| told(&e))
}

/// Add the tag `name` to the library's list of tags.
fn list_tag<'l, F: Fs + 'l>(built: Built<'l, F>, view: &View, name: String) -> Built<'l, F> {
    on_list(built, view, vec![(TAG_NAMES, name)])
}

/// Add each value to its set on the library's list of tags: the list the view holds, or
/// a new one.
fn on_list<'l, F: Fs + 'l>(
    built: Built<'l, F>,
    view: &View,
    values: Vec<(Set<String>, String)>,
) -> Built<'l, F> {
    match view.with(TAG_NAMES).into_iter().min() {
        Some(list) => values
            .into_iter()
            .fold(built, |built, (key, value)| built.add(list, key, value)),
        None => {
            built
                .create(|list| {
                    for (key, value) in values {
                        list.add(key, value);
                    }
                })
                .0
        }
    }
}

/// Put a tag on each file of `files`: on its entity, the one bound to its path in `view`
/// where another writer made one since the app looked, or a new one that adopts it.
fn tag<'l, F: Fs + 'l>(
    mut built: Built<'l, F>,
    view: &View,
    files: &[Key],
    name: &str,
) -> Result<Built<'l, F>, String> {
    for key in files {
        let path = match key {
            Key::Entity(entity) => {
                built = built.add(*entity, TAGS, name.to_string());
                continue;
            }
            Key::File(path) => path,
        };
        let at = relative(path).ok_or_else(|| no_path(path))?;
        if let Some(entity) = view.at(&at).map(|entity| entity.id()) {
            built = built.add(entity, TAGS, name.to_string());
            continue;
        }
        let listed = view
            .listed(&at)
            .ok_or_else(|| format!("{path} is not listed"))?;
        let expect = Expect::Listed {
            len: listed.len,
            modified: listed.modified,
        };
        let (next, _) = built.create(|e| {
            e.adopt(&at, expect).add(TAGS, name.to_string());
        });
        built = next;
    }
    Ok(built)
}

/// Where a write goes, what it expects to find there, and where a new file came from.
#[derive(Clone, Copy)]
struct To<'p> {
    path: &'p LibPath,
    expect: Expect,
    came: Option<Came>,
}

/// What a fill that failed found, kept to say why its write did not land.
type FillFailure = Rc<RefCell<Option<io::Error>>>;

/// `built` with the write of the file `to` names from `fill`: as the file of the
/// entity bound there, as a new entity's file where `to` says where it came from, or
/// as one no entity is bound to, unless `tags` are to go on it, which makes it an
/// entity's.
fn save_part<'l, F: Fs + 'l>(
    built: Built<'l, F>,
    view: &View,
    at: &RelPath,
    to: To<'_>,
    fill: Fill,
    tags: &[String],
) -> (Built<'l, F>, FillFailure) {
    let To { expect, came, .. } = to;
    let failed = Rc::new(RefCell::new(None));
    let source = || -> Box<dyn Filler<F> + 'l> {
        Box::new(Filling {
            fill,
            failed: failed.clone(),
        })
    };
    let entity = view
        .at(at)
        .map(|entity| entity.id())
        .or_else(|| missing_at(view, at));
    let built = match (entity, came) {
        (Some(entity), _) => {
            let built = built.save_from(entity, at, source(), expect);
            tags.iter()
                .fold(built, |built, name| built.add(entity, TAGS, name.clone()))
        }
        (None, Some(came)) => {
            let source = source();
            let (built, _) = built.create(|e| {
                e.set(ORIGIN, came).save_from(at, source, expect);
                for name in tags {
                    e.add(TAGS, name.clone());
                }
            });
            built
        }
        (None, None) if tags.is_empty() => built.save_file_from(at, source(), expect),
        (None, None) => {
            let source = source();
            let (built, _) = built.create(|e| {
                e.save_from(at, source, expect);
                for name in tags {
                    e.add(TAGS, name.clone());
                }
            });
            built
        }
    };
    (built, failed)
}

/// How one part of a batch is answered once the batch is committed: the event it
/// answers with.
struct Owed {
    /// The part of the batch it is of, in order.
    part: usize,
    failed: Option<FillFailure>,
    answer: Answering,
}

enum Answering {
    Saved {
        id: u64,
        path: LibPath,
        len: u64,
        crc: u32,
        identity: Option<toshokan::Identity>,
    },
    Rewritten {
        id: u64,
        path: LibPath,
    },
    Imported {
        id: u64,
        path: LibPath,
    },
    Moved {
        from: LibPath,
        to: LibPath,
    },
    /// Answered only where it did not land: what it was doing.
    Quiet(String),
}

impl Answering {
    /// The event that says the part did not land, because of `failure`.
    fn refused(self, failure: Failure) -> Option<Event> {
        Some(match self {
            Answering::Saved { id, path, .. } => Event::Saved {
                id,
                path,
                result: Err(failure),
            },
            Answering::Rewritten { id, path } => Event::Rewritten {
                id,
                path,
                result: Err(failure),
            },
            Answering::Imported { id, path } => Event::Imported {
                id,
                path,
                result: Err(failure),
            },
            Answering::Moved { from, to } => Event::Moved {
                from,
                to,
                result: Err(failure),
            },
            Answering::Quiet(doing) => Event::Failed(format!("{doing}: {failure}")),
        })
    }

    /// The event that says the part landed; none for one answered only on failure.
    async fn landed<F: Fs>(self, fs: &F) -> Option<Event> {
        Some(match self {
            Answering::Saved {
                id,
                path,
                len,
                crc,
                identity,
            } => {
                // The commit landed: a stat that fails now leaves what it wrote, with no
                // time, which the next scan reads the identity again for.
                let stat = stat_at(fs, &path).await;
                let stat = stat.unwrap_or(Stat {
                    len,
                    modified: None,
                });
                let result = Ok(Fingerprint {
                    crc: Some(crc),
                    identity,
                    ..Fingerprint::unread(stat)
                });
                Event::Saved { id, path, result }
            }
            Answering::Rewritten { id, path } => {
                let result = landed(fs, &path).await;
                Event::Rewritten { id, path, result }
            }
            Answering::Imported { id, path } => {
                let result = landed(fs, &path).await;
                Event::Imported { id, path, result }
            }
            Answering::Moved { from, to } => Event::Moved {
                from,
                to,
                result: Ok(()),
            },
            Answering::Quiet(_) => return None,
        })
    }
}

/// Commit the parts of one gesture as one intent, and answer each as it would be
/// answered alone: each by whether its file or folder landed. A part whose file could
/// not be read before anything was written is answered so, and the rest committed
/// without it. A tag on a file saved in the same batch goes on it as it is made.
async fn batch<F: Fs>(
    library: &mut Library<F>,
    label: &str,
    mut parts: Vec<Intent>,
    answer: &mut impl FnMut(Event),
) -> Option<toshokan::Committed> {
    loop {
        let again = parts.iter().any(Intent::reads).then(|| parts.clone());
        let tried = attempt(library, label, parts).await;
        let unread: BTreeSet<usize> = tried
            .owed
            .iter()
            .filter(|owed| {
                owed.failed
                    .as_ref()
                    .is_some_and(|failed| failed.borrow().is_some())
            })
            .map(|owed| owed.part)
            .collect();
        let retry = again.filter(|_| tried.changes.is_none() && !unread.is_empty());
        let Some(again) = retry else {
            return tried.answer(library.fs(), answer).await;
        };
        for owed in tried
            .owed
            .into_iter()
            .filter(|owed| unread.contains(&owed.part))
        {
            let filled = owed
                .failed
                .and_then(|failed| failed.take())
                .map(fill_failure);
            answer_some(
                answer,
                owed.answer
                    .refused(filled.unwrap_or(Failure::Io("not read".into()))),
            );
        }
        parts = again
            .into_iter()
            .enumerate()
            .filter(|(at, _)| !unread.contains(at))
            .map(|(_, part)| part)
            .collect();
    }
}

/// Commit the parts of one change, as [`take_in`] does where one takes in the list an
/// older drawbar kept, adding to `looked` what reading the library first found.
async fn gathered<F: Fs>(
    library: &mut Library<F>,
    label: &str,
    parts: Vec<Intent>,
    looked: &mut Vec<toshokan::report::Change>,
    answer: &mut impl FnMut(Event),
) -> Result<Option<toshokan::Committed>, String> {
    let hash = parts.iter().find_map(|part| match part {
        Intent::TakeIn { hash, .. } => Some(hash.clone()),
        _ => None,
    });
    match hash {
        Some(hash) => take_in(library, label, parts, hash, looked, answer).await,
        None => Ok(batch(library, label, parts, answer).await),
    }
}

/// The lock, in an install's own state, that a window holds while it takes in the list
/// an older drawbar kept, which is the install's, so that one window takes it in.
fn taking_in() -> RelPath {
    RelPath::new("taking-in.lock").expect("one component")
}

/// Commit the `parts` of the take-in of the list of hash `hash` holding [`taking_in`],
/// after reading what other writers logged since, which goes in `looked`. Where another
/// window holds the lock, or the library took the list in already, nothing is tried, and
/// [`Event::TakenInElsewhere`] says so.
async fn take_in<F: Fs>(
    library: &mut Library<F>,
    label: &str,
    parts: Vec<Intent>,
    hash: String,
    looked: &mut Vec<toshokan::report::Change>,
    answer: &mut impl FnMut(Event),
) -> Result<Option<toshokan::Committed>, String> {
    let lock = Io::Lock { name: taking_in() };
    match library.fs().perform(lock).await {
        Ok(Reply::Lock(Lock::Acquired)) => {}
        Ok(_) => {
            answer(Event::TakenInElsewhere);
            return Ok(None);
        }
        Err(e) => {
            let why = storage::failed(e);
            return Err(format!(
                "taking this window's hold on bringing in kept sounds: {why}"
            ));
        }
    }
    let committed = match library.refresh().await {
        Err(e) => Err(told(&e)),
        Ok(refreshed) => {
            looked.extend(refreshed.changes);
            match library.view().find(TAKEN_IN, &hash).is_empty() {
                true => Ok(batch(library, label, parts, answer).await),
                false => {
                    answer(Event::TakenInElsewhere);
                    Ok(None)
                }
            }
        }
    };
    let _ = library.fs().perform(Io::Unlock { name: taking_in() }).await;
    committed
}

/// One try at a batch: what each part is owed, and how the commit went.
struct Tried {
    owed: Vec<Owed>,
    refused: Vec<Event>,
    changes: Option<toshokan::Committed>,
    why: Option<Failure>,
}

impl Tried {
    /// Answer each part, and return what the commit changed.
    async fn answer<F: Fs>(
        self,
        fs: &F,
        answer: &mut impl FnMut(Event),
    ) -> Option<toshokan::Committed> {
        let Tried {
            owed,
            refused,
            changes,
            why,
        } = self;
        for event in refused {
            answer(event);
        }
        for Owed {
            failed,
            answer: answering,
            ..
        } in owed
        {
            let event = match changes.is_some() {
                true => answering.landed(fs).await,
                false => {
                    let filled = failed.and_then(|failed| failed.take()).map(fill_failure);
                    let failure = filled.or_else(|| why.clone());
                    answering.refused(failure.unwrap_or(Failure::Io("not made".to_string())))
                }
            };
            answer_some(answer, event);
        }
        changes
    }
}

async fn attempt<F: Fs>(library: &mut Library<F>, label: &str, parts: Vec<Intent>) -> Tried {
    let view = library.view();
    let saved: BTreeSet<LibPath> = parts
        .iter()
        .filter_map(|part| match part {
            Intent::Save { path, .. } | Intent::Import { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    let mut tags: BTreeMap<LibPath, Vec<String>> = BTreeMap::new();
    for part in &parts {
        let Intent::Tag { files, name, .. } = part else {
            continue;
        };
        for key in files {
            if let Key::File(path) = key {
                if saved.contains(path) {
                    tags.entry(path.clone()).or_default().push(name.clone());
                }
            }
        }
    }
    let mut refused = Vec::new();
    let mut unchecked = BTreeSet::new();
    for (at, part) in parts.iter().enumerate() {
        if let Err(event) = checked(part, library.fs()).await {
            refused.push(*event);
            unchecked.insert(at);
        }
    }
    let mut parting = Parting {
        built: Some(library.intent(label)),
        view: &view,
        tags: &tags,
        part: 0,
        owed: Vec::new(),
    };
    for (at, part) in parts.into_iter().enumerate() {
        if unchecked.contains(&at) {
            continue;
        }
        parting.part = at;
        if let Err(event) = parting.add(part) {
            refused.push(*event);
        }
    }
    let Parting { built, owed, .. } = parting;
    let built = built.expect("each part puts the intent back");
    let (changes, why) = match built.commit().await {
        Ok(committed) => (Some(committed), None),
        Err(e) => (None, Some(failure(e, None))),
    };
    Tried {
        owed,
        refused,
        changes,
        why,
    }
}

/// Look, before anything is built, at what `part` copies from the library: a copy of a
/// file no longer what the app knew of it is refused.
async fn checked<F: Fs>(part: &Intent, fs: &F) -> Result<(), Box<Event>> {
    let Intent::Import { id, path, from, .. } = part else {
        return Ok(());
    };
    let (Source::Library(source, known) | Source::Edited(source, known, _)) = from else {
        return Ok(());
    };
    if still(fs, source, known).await.unwrap_or(false) {
        return Ok(());
    }
    let answering = Answering::Imported {
        id: *id,
        path: path.clone(),
    };
    let changed = Failure::Io("the file it copies changed on disk".into());
    Err(Box::new(
        answering.refused(changed).expect("a copy answers"),
    ))
}

fn answer_some(answer: &mut impl FnMut(Event), event: Option<Event>) {
    if let Some(event) = event {
        answer(event);
    }
}

/// One intent being built from the parts of a batch.
struct Parting<'l, 'v, F: Fs + 'l> {
    /// Taken while a part is built into it, and put back.
    built: Option<Built<'l, F>>,
    view: &'v View,
    tags: &'v BTreeMap<LibPath, Vec<String>>,
    /// The part being built.
    part: usize,
    owed: Vec<Owed>,
}

impl<'l, F: Fs + 'l> Parting<'l, '_, F> {
    /// Build `part` into the intent, or say at once why it cannot be.
    fn add(&mut self, part: Intent) -> Result<(), Box<Event>> {
        let at = |path: &LibPath| relative(path).ok_or_else(|| no_path(path));
        match part {
            Intent::Save {
                id,
                path,
                bytes,
                expect,
                came,
            } => {
                let answering = Answering::Saved {
                    id,
                    len: bytes.len() as u64,
                    crc: nord_format::crc::crc32(&bytes),
                    identity: keys::identity_of_bytes(&path, &bytes),
                    path: path.clone(),
                };
                let to = To {
                    path: &path,
                    expect,
                    came,
                };
                self.save(to, Fill::Bytes(bytes), answering)
            }
            Intent::Rewrite {
                id,
                path,
                pieces,
                expect,
            } => {
                let answering = Answering::Rewritten {
                    id,
                    path: path.clone(),
                };
                let to = To {
                    path: &path,
                    expect,
                    came: None,
                };
                self.save(to, Fill::Edited(path.clone(), pieces), answering)
            }
            Intent::Import {
                id,
                path,
                from,
                expect,
                came,
            } => {
                let answering = Answering::Imported {
                    id,
                    path: path.clone(),
                };
                let fill = match from {
                    Source::Outside(from) => Fill::Outside(from),
                    Source::Part(part) => Fill::Part(part),
                    Source::Edited(source, known, pieces) => {
                        Fill::Library(source, known, Some(pieces))
                    }
                    Source::Library(source, known) => Fill::Library(source, known, None),
                };
                let to = To {
                    path: &path,
                    expect,
                    came,
                };
                self.save(to, fill, answering)
            }
            Intent::Move { moves } => {
                for moving in moves {
                    let answering = Answering::Moved {
                        from: moving.from.clone(),
                        to: moving.to.clone(),
                    };
                    let (from, to) = match (at(&moving.from), at(&moving.to)) {
                        (Ok(from), Ok(to)) => (from, to),
                        (Err(why), _) | (_, Err(why)) => {
                            let refused = answering.refused(Failure::Io(why));
                            return Err(Box::new(refused.expect("a move answers")));
                        }
                    };
                    let built = self.take();
                    self.built = Some(match self.view.at(&from).map(|entity| entity.id()) {
                        Some(entity) => built.rename(entity, &to, moving.expect),
                        None => built.rename_file(&from, &to, moving.expect),
                    });
                    self.owe(None, answering);
                }
                Ok(())
            }
            Intent::MoveFolder { from, to } => {
                let answering = Answering::Moved {
                    from: from.clone(),
                    to: to.clone(),
                };
                let (from, to) = match (at(&from), at(&to)) {
                    (Ok(from), Ok(to)) => (from, to),
                    (Err(why), _) | (_, Err(why)) => {
                        return Err(Box::new(
                            answering.refused(Failure::Io(why)).expect("a move answers"),
                        ));
                    }
                };
                self.built = Some(self.take().move_tree(&from, &to));
                self.owe(None, answering);
                Ok(())
            }
            Intent::Delete { files } => {
                for (path, expect) in files {
                    let doing = Answering::Quiet(format!("deleting {path}"));
                    let at = at(&path).map_err(|why| {
                        Box::new(Event::Failed(format!("deleting {path}: {why}")))
                    })?;
                    let built = self.take();
                    self.built = Some(match self.view.at(&at).map(|entity| entity.id()) {
                        Some(entity) => built.trash(entity, expect).delete(entity),
                        None => built.trash_file(&at, expect),
                    });
                    self.owe(None, doing);
                }
                Ok(())
            }
            Intent::MakeDir { path } => {
                let at = at(&path)
                    .map_err(|why| Box::new(Event::Failed(format!("making {path}: {why}"))))?;
                self.built = Some(self.take().make_dir(&at));
                let doing = Answering::Quiet(format!("making {path}"));
                self.owe(None, doing);
                Ok(())
            }
            Intent::RemoveDir { path } => {
                let at = at(&path)
                    .map_err(|why| Box::new(Event::Failed(format!("removing {path}: {why}"))))?;
                self.built = Some(self.take().remove_dir(&at));
                let doing = Answering::Quiet(format!("removing {path}"));
                self.owe(None, doing);
                Ok(())
            }
            Intent::Tag { files, name, make } => {
                // A file this batch saves wears the tag as it is made.
                let rest: Vec<Key> = files
                    .into_iter()
                    .filter(|key| !matches!(key, Key::File(path) if self.tags.contains_key(path)))
                    .collect();
                let taggable = |key: &Key| match key {
                    Key::Entity(_) => true,
                    Key::File(path) => relative(path).is_some_and(|at| {
                        self.view.at(&at).is_some() || self.view.listed(&at).is_some()
                    }),
                };
                if let Some(Key::File(path)) = rest.iter().find(|key| !taggable(key)) {
                    return Err(Box::new(Event::Failed(format!(
                        "tagging {path}: it is not listed"
                    ))));
                }
                let built = tag(self.take(), self.view, &rest, &name)
                    .expect("every file it tags is listed");
                self.built = Some(match make {
                    true => list_tag(built, self.view, name.clone()),
                    false => built,
                });
                let doing = Answering::Quiet(format!("tagging with “{name}”"));
                self.owe(None, doing);
                Ok(())
            }
            Intent::TakeIn { tags, hash } => {
                let names = tags.into_iter().map(|name| (TAG_NAMES, name));
                let values = names.chain([(TAKEN_IN, hash)]).collect();
                self.built = Some(on_list(self.take(), self.view, values));
                self.owe(None, Answering::Quiet(super::TAKE_IN.to_lowercase()));
                Ok(())
            }
            other => Err(Box::new(Event::Failed(format!(
                "{} cannot be made with other changes",
                other.label()
            )))),
        }
    }

    fn save(&mut self, to: To<'_>, fill: Fill, answering: Answering) -> Result<(), Box<Event>> {
        let Some(at) = relative(to.path) else {
            let failure = Failure::Io(no_path(to.path));
            return Err(Box::new(
                answering.refused(failure).expect("a write answers"),
            ));
        };
        let tags = self.tags.get(to.path).map_or(&[][..], Vec::as_slice);
        let built = self.take();
        let (built, failed) = save_part(built, self.view, &at, to, fill, tags);
        self.built = Some(built);
        self.owe(Some(failed), answering);
        Ok(())
    }

    fn take(&mut self) -> Built<'l, F> {
        self.built.take().expect("each part puts the intent back")
    }

    fn owe(&mut self, failed: Option<FillFailure>, answer: Answering) {
        let part = self.part;
        self.owed.push(Owed {
            part,
            failed,
            answer,
        });
    }
}

/// Whether the library file at `path` still holds what `known` says: its stat has not
/// moved, or its identity is the one known.
async fn still<F: Fs>(fs: &F, path: &LibPath, known: &Fingerprint) -> io::Result<bool> {
    let at = relative(path).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    let stat = storage::stat(fs, &at).await?;
    holds(&at, known, stat, async |range| {
        storage::read(fs, Root::Folder, &at, range).await
    })
    .await
}

/// Whether the library file at `at`, found as `stat`, holds what `known` says: `stat`
/// is the one known, or the identity `read` finds is.
async fn holds(
    at: &RelPath,
    known: &Fingerprint,
    stat: Option<Stat>,
    read: impl AsyncFnMut(Range) -> io::Result<Vec<u8>>,
) -> io::Result<bool> {
    let Some(stat) = stat else {
        return Ok(false);
    };
    if stat == known.stat() {
        return Ok(true);
    }
    let Some(identity) = known.identity else {
        return Ok(false);
    };
    Ok(storage::identity(at, stat.len, read).await? == identity)
}

/// Why a change is refused that is too large for the library to log as one.
const TOO_LARGE: &str = "it is too large a change to make at once; change fewer files at a time";

/// What failed to fill a file, or why toshokan refused the change, as the app tells it.
fn failure(e: Error, filled: Option<io::Error>) -> Failure {
    if let Some(e) = filled {
        return fill_failure(e);
    }
    match e {
        Error::Refused(Refusal::Changed(_) | Refusal::Directory(_)) => Failure::Moved,
        e => Failure::Io(told(&e)),
    }
}

/// Why a fill that failed did not land.
fn fill_failure(e: io::Error) -> Failure {
    match rewrite::is_changed(&e) {
        true => Failure::Moved,
        false => Failure::Io(e.to_string()),
    }
}

/// The entity whose file is missing from `path`: one whose file a save writes back.
fn missing_at(view: &View, path: &RelPath) -> Option<toshokan::EntityId> {
    let mut entities = view.entities().into_iter();
    let missing = entities.find(|entity| {
        entity
            .file()
            .is_some_and(|file| file.path == *path && file.state == toshokan::FileState::Missing)
    });
    missing.map(|entity| entity.id())
}

/// The file just written at `path`, as a listing finds it, with nothing read: the
/// backend, which reads files, looks at what it holds.
async fn landed<F: Fs>(fs: &F, path: &LibPath) -> Result<Found, Failure> {
    Ok(Found::unread(path.clone(), stat_at(fs, path).await?))
}

async fn stat_at<F: Fs>(fs: &F, path: &LibPath) -> Result<Stat, Failure> {
    let gone = || Failure::Io("the file was gone as soon as it was written".into());
    let at = relative(path).ok_or_else(|| Failure::Io(no_path(path)))?;
    let stat = storage::stat(fs, &at).await.map_err(|_| gone())?;
    stat.ok_or_else(gone)
}

/// What a file toshokan stages is filled from.
enum Fill {
    Bytes(Vec<u8>),
    /// A file outside the library, never held whole.
    Outside(super::Outside),
    /// Some bytes of a file outside the library, which must have their CRC.
    Part(Part),
    /// The library's own file at this path, as the app knew it, read by range and never
    /// held whole: as it is, or as an edit's pieces make it. It fails where the file
    /// changed while it was read.
    Library(LibPath, Fingerprint, Option<Arc<Pieces>>),
    /// The file an edit makes of a piano or sample instrument resting in the library at
    /// this path, which it replaces, read by range and never held whole.
    Edited(LibPath, Arc<Pieces>),
}

/// A [`Fill`] as toshokan takes it. What failed is kept in `failed`, since toshokan
/// carries only its text.
struct Filling {
    fill: Fill,
    failed: FillFailure,
}

impl<F: Fs> Filler<F> for Filling {
    fn fill<'s>(self: Box<Self>, staging: Staging<'s, F>) -> Filled<'s>
    where
        Self: 's,
    {
        Box::pin(async move {
            let Filling { fill, failed } = *self;
            fill.into(&staging).await.map_err(|e| {
                let why = IoError::Other(e.to_string());
                *failed.borrow_mut() = Some(e);
                why
            })
        })
    }
}

impl Fill {
    async fn into<F: Fs>(self, staging: &Staging<'_, F>) -> io::Result<()> {
        let put = async |at: u64, bytes: Vec<u8>| {
            staging
                .write(at, bytes)
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        };
        match self {
            Fill::Bytes(bytes) => put(0, bytes).await,
            Fill::Outside(from) => {
                let mut source = super::outside_source(&from)?;
                let len = nord_usb::FileSource::len(&source);
                copy(&mut source, 0..len, put).await.map(drop)
            }
            Fill::Part(part) => {
                let mut source = super::outside_source(&part.from)?;
                let crc = copy(&mut source, part.bytes.clone(), put).await?;
                match crc == part.crc32 {
                    true => Ok(()),
                    false => Err(part.mismatch()),
                }
            }
            Fill::Library(from, known, pieces) => {
                let from = relative(&from).ok_or_else(|| io::Error::other(no_path(&from)))?;
                let read =
                    async |range: Range| staging.read(&from, range).await.map_err(storage::failed);
                match pieces {
                    Some(pieces) => rewrite::stream(&pieces, by_range(read), put).await?,
                    None => whole(known.len, read, put).await?,
                }
                let stat = staging.stat(&from).await.map_err(storage::failed)?;
                match holds(&from, &known, storage::file(stat), read).await? {
                    true => Ok(()),
                    false => Err(rewrite::changed(
                        "the file it copies changed on disk".into(),
                    )),
                }
            }
            Fill::Edited(from, pieces) => {
                let from = relative(&from).ok_or_else(|| io::Error::other(no_path(&from)))?;
                let read =
                    async |range: Range| staging.read(&from, range).await.map_err(storage::failed);
                rewrite::stream(&pieces, by_range(read), put).await
            }
        }
    }
}

/// `read` taking a range of offsets.
fn by_range(
    mut read: impl AsyncFnMut(Range) -> io::Result<Vec<u8>>,
) -> impl AsyncFnMut(std::ops::Range<u64>) -> io::Result<Vec<u8>> {
    async move |range: std::ops::Range<u64>| {
        let range = Range {
            offset: range.start,
            len: range.end - range.start,
        };
        read(range).await
    }
}

/// Copy the first `len` bytes `read` finds through `put` a chunk at a time. A source
/// shorter than `len` fails the copy.
async fn whole(
    len: u64,
    mut read: impl AsyncFnMut(Range) -> io::Result<Vec<u8>>,
    mut put: impl AsyncFnMut(u64, Vec<u8>) -> io::Result<()>,
) -> io::Result<()> {
    let mut at = 0;
    while at < len {
        let chunk = CHUNK.min(len - at);
        let bytes = read(Range {
            offset: at,
            len: chunk,
        })
        .await?;
        if bytes.len() as u64 != chunk {
            return Err(rewrite::changed(
                "the file it copies changed on disk".into(),
            ));
        }
        put(at, bytes).await?;
        at += chunk;
    }
    Ok(())
}

/// Copy `range` of `from` through `put` a chunk at a time, from offset 0, and return
/// its CRC-32. A source shorter than the range fails the copy.
async fn copy(
    from: &mut impl nord_usb::FileSource,
    range: std::ops::Range<u64>,
    mut put: impl AsyncFnMut(u64, Vec<u8>) -> io::Result<()>,
) -> io::Result<u32> {
    let mut crc = nord_format::crc::Crc32Stream::new();
    let mut at = range.start;
    while at < range.end {
        let len = CHUNK.min(range.end - at);
        let mut chunk = vec![0; len as usize];
        from.read_at(at, &mut chunk).await?;
        crc.update(&chunk);
        put(at - range.start, chunk).await?;
        at += len;
    }
    Ok(crc.value())
}

fn committed_view<F: Fs>(
    library: &Library<F>,
    shown: &mut Option<View>,
    changes: Vec<toshokan::report::Change>,
) -> Viewed {
    Viewed {
        shown: show(shown, library.view()),
        dirs: None,
        changes,
        moves: Vec::new(),
        followed: None,
        unlisted: None,
        unfinished: None,
        kept: kept(library),
        retired: false,
        left: Vec::new(),
        finishing: shown_paths(&library.finishing()),
    }
}

/// One change Kept apart lists, and whether taking it makes it on the library's own
/// record of a file rather than taking it from the run.
struct Listed {
    change: ItemChange,
    onto: bool,
}

/// The changes of `item` as Kept apart lists them, each one taking it can make. A
/// change to an entity the library does not show is listed where the item makes the
/// entity, or as one to the entity the library binds the entity's file to, which a kept
/// item names. Moved there, it is listed only while it would change that entity, and
/// making or placing the item's entity is not listed. A change to an entity nothing can
/// make is not listed. A gap, which waits for the history it follows, lists its changes
/// as they are.
fn listed(item: &Item, view: &View) -> Vec<Listed> {
    let kept = view.items().iter().flat_map(|item| &item.changes);
    let files = kept.filter_map(|change| match &change.change {
        Some(FactChange::File { entity, file }) => Some((*entity, &file.path)),
        _ => None,
    });
    let held = files.filter(|(made, _)| view.entity(*made).is_none());
    let held = held.filter_map(|(made, path)| Some((made, view.at(path)?.id())));
    let onto: BTreeMap<toshokan::EntityId, toshokan::EntityId> = held.collect();
    let made: BTreeSet<toshokan::EntityId> = item
        .changes
        .iter()
        .filter(|change| change.what == What::Created)
        .map(|change| change.entity)
        .collect();
    let listed = item.changes.iter().filter_map(|change| {
        let own = || Listed {
            change: change.clone(),
            onto: false,
        };
        if item.kind == ItemKind::Gap || view.entity(change.entity).is_some() {
            return Some(own());
        }
        let Some(to) = onto.get(&change.entity) else {
            return made.contains(&change.entity).then(own);
        };
        let moved = field_onto(change.change.as_ref()?, *to)?;
        if changes_nothing(&moved, view) {
            return None;
        }
        let change = ItemChange {
            entity: *to,
            what: change.what.clone(),
            change: Some(moved),
            forgotten: false,
        };
        Some(Listed { change, onto: true })
    });
    listed.collect()
}

/// `change` made to the same field of `to`; `None` where it makes, ends or places an
/// entity rather than changing a field.
fn field_onto(change: &FactChange, to: toshokan::EntityId) -> Option<FactChange> {
    Some(match change.clone() {
        FactChange::Set { key, value, .. } => FactChange::Set {
            entity: to,
            key,
            value,
        },
        FactChange::Clear { key, .. } => FactChange::Clear { entity: to, key },
        FactChange::Add { key, value, .. } => FactChange::Add {
            entity: to,
            key,
            value,
        },
        FactChange::Remove { key, value, .. } => FactChange::Remove {
            entity: to,
            key,
            value,
        },
        FactChange::Delete { .. } | FactChange::Revive { .. } | FactChange::File { .. } => {
            return None
        }
    })
}

/// What of `writer`'s trash to keep, where emptying it would remove anything: the items
/// `reached` says this session's undo needs, newest first within `room` bytes. The items
/// of another writer of this install stay as they are, since only a writer empties its
/// own trash.
fn keeping(
    held: &[TrashItem],
    writer: WriterId,
    reached: impl Fn(&EntryHash) -> bool,
    room: u64,
) -> Option<BTreeSet<Nonce>> {
    let held: Vec<&TrashItem> = held.iter().filter(|item| item.writer == writer).collect();
    let mut room = room;
    let kept: BTreeSet<Nonce> = held
        .iter()
        .rev()
        .filter(|item| reached(&item.by))
        .take_while(|item| {
            let fits = item.len <= room;
            room = room.saturating_sub(item.len);
            fits
        })
        .map(|item| item.item)
        .collect();
    (!held.iter().all(|item| kept.contains(&item.item))).then_some(kept)
}

/// Whether making `change` would leave the field it changes as `view` shows it.
fn changes_nothing(change: &FactChange, view: &View) -> bool {
    let folded = view.folded();
    match change {
        FactChange::Add { entity, key, value } => folded.members(*entity, key).contains(value),
        FactChange::Remove { entity, key, value } => !folded.members(*entity, key).contains(value),
        FactChange::Set { entity, key, value } => {
            let writes = folded.register(*entity, key);
            !writes.is_empty() && writes.iter().all(|write| write.value == *value)
        }
        FactChange::Clear { entity, key } => folded.register(*entity, key).is_empty(),
        FactChange::Delete { .. } | FactChange::Revive { .. } | FactChange::File { .. } => false,
    }
}

/// The changes of `listed` that `named` names, or all it can take where it names none,
/// each once, and whether each is made on the library's own record. A tag taken brings
/// its entry in the library's list of tags where the run added it there too, and a
/// change taken to an entity the run made brings its making and its file.
fn taking(listed: &[Listed], named: &[KeptKey]) -> Vec<(FactChange, bool)> {
    let picked: Vec<&Listed> = listed
        .iter()
        .filter(|listed| {
            let change = &listed.change;
            let does = change.change.as_ref().map(super::KeptDo::of);
            named.is_empty() || named.contains(&(change.entity, change.what.clone(), does))
        })
        .collect();
    let gained = |tag: &toshokan::Raw| {
        picked.iter().any(|taken| {
            matches!(&taken.change.change, Some(FactChange::Add { key, value, .. })
                if key == TAGS.name() && value == tag)
        })
    };
    let tag_list = listed.iter().filter(|listed| {
        matches!(&listed.change.change, Some(FactChange::Add { key, value, .. })
            if key == TAG_NAMES.name() && gained(value))
    });
    let taken: Vec<&Listed> = picked.iter().copied().chain(tag_list).collect();
    let made = |entity| {
        let mut changes = listed.iter().map(|listed| &listed.change);
        changes.any(|change| change.entity == entity && change.what == What::Created)
    };
    let (made, taken) = (&made, &taken);
    let along = |what: What| {
        listed.iter().filter(move |listed| {
            let change = &listed.change;
            change.what == what
                && made(change.entity)
                && taken
                    .iter()
                    .any(|taken| taken.change.entity == change.entity)
        })
    };
    let all = along(What::Created)
        .chain(along(What::File))
        .chain(taken.iter().copied());
    let mut takes: Vec<(FactChange, bool)> = Vec::new();
    for listed in all {
        let Some(change) = &listed.change.change else {
            continue;
        };
        if !takes.iter().any(|(taken, _)| taken == change) {
            takes.push((change.clone(), listed.onto));
        }
    }
    takes
}

/// Why the library did not do what was asked, in the user's words: a file by its path
/// in the library, never an id, a record of drawbar's own, or a name of toshokan's.
fn told(e: &Error) -> String {
    match e {
        Error::Io { root, path, error } => format!("{}: {error}", place(*root, path)),
        Error::InvalidPath { path, reason } => {
            format!("“{path}” cannot be a name in the library: {reason}")
        }
        // Why is said once, when the library opens or turns read-only.
        Error::ReadOnly(_) => "the library is read-only".to_string(),
        Error::Refused(refusal) => refused(refusal),
        Error::Rekey { .. } => "this drawbar's history of the library went on elsewhere too, \
                                so it starts one of its own; try again"
            .to_string(),
        Error::InvalidId { .. } => "drawbar found a record of its own it cannot read".to_string(),
        Error::DuplicateKey { .. } | Error::InvalidKey { .. } => FAULT.to_string(),
    }
}

/// What a refusal only a fault in drawbar itself can cause says.
const FAULT: &str = "drawbar has a fault that stops it making the change; please report it";

/// What `path` in `root` is, in the user's words.
fn place(root: Root, path: &RelPath) -> String {
    match root {
        Root::Folder if !keys::layout().owns(path) => format!("“{path}”"),
        Root::Folder => "drawbar's records in the library folder".to_string(),
        Root::Local => "drawbar's own data on this computer".to_string(),
    }
}

/// Why nothing may be written, in the user's words, and how to put it right where
/// they can.
fn read_only(why: &Why) -> String {
    match why {
        Why::NewerOwnHistory { .. } => {
            "a newer drawbar changed it from this computer; update drawbar to change it".to_string()
        }
        Why::NewerLayout { .. } => {
            "a newer drawbar keeps it in a newer way; update drawbar to change it".to_string()
        }
        Why::FolderNotWritable => "the library folder cannot be written".to_string(),
        Why::ClockExhausted => {
            "this computer's clock reads a time drawbar cannot record".to_string()
        }
        Why::Unfinished { label, path, error } => unfinished(label, path, error),
    }
}

/// Why drawbar could not finish the change `label` at `path`, and what to do.
fn unfinished(label: &str, path: &RelPath, error: &IoError) -> String {
    let unfinished = format!("drawbar could not finish “{label}”");
    let again = format!("then open it again from File ▸ {}", crate::menu::RECENT);
    match error {
        IoError::AlreadyExists
        | IoError::IsDirectory
        | IoError::NotDirectory
        | IoError::NotEmpty => {
            format!("something is in the way at {path}, so {unfinished}: move it, {again}")
        }
        IoError::NoSpace => format!("the disk is full, so {unfinished}: make room, {again}"),
        IoError::Denied => format!("{path} may not be written, so {unfinished}: allow it, {again}"),
        error => format!("{unfinished} at {path} ({error}): {again}"),
    }
}

/// Why a change was refused before anything was written, in the user's words.
fn refused(refusal: &Refusal) -> String {
    use toshokan::{Invalid, Unbindable};
    match refusal {
        Refusal::Changed(mismatch) => format!("“{}” is not as drawbar last saw it", mismatch.path),
        Refusal::Directory(path) => format!("“{path}” is a folder"),
        Refusal::ChangedSince { .. } => "another drawbar has changed it since".to_string(),
        Refusal::Nothing => "there is nothing to do".to_string(),
        Refusal::Emptied => "drawbar no longer keeps what undoing it needs".to_string(),
        Refusal::Unsupported(capability) => {
            use toshokan::io::Capability;
            match capability {
                Capability::RenameDir => "the library folder cannot rename a folder",
                Capability::RenameFile => "the library folder cannot rename a file",
                Capability::Append => "the library folder cannot add to a file",
                Capability::Fsync => "the library folder cannot make sure a change is kept",
            }
            .to_string()
        }
        Refusal::Gone(path) => format!("what was to go at “{path}” is gone"),
        Refusal::SnapshotTooLong => {
            "the library's history is too long to fold into one record".to_string()
        }
        Refusal::Invalid(Invalid::Unbindable { path, why, .. }) => {
            let why = match why {
                Unbindable::Absent => "no file is there",
                Unbindable::Changed => "the file there holds something else now",
                Unbindable::Unscanned => "drawbar has not looked at it yet",
                Unbindable::Bound(_) => "it is another asset's file",
            };
            format!("the file at {path} cannot be taken: {why}")
        }
        Refusal::Invalid(Invalid::NoEntity(_)) => "it is no longer in the library".to_string(),
        Refusal::Invalid(Invalid::NoFile(_)) => "it has no file".to_string(),
        Refusal::Invalid(Invalid::Bound(path)) => format!("“{path}” is another asset's file"),
        Refusal::Invalid(Invalid::ReservedPath(path)) => format!("“{path}” is drawbar's own"),
        Refusal::Invalid(Invalid::NotEmpty(path)) => format!("“{path}” holds files"),
        Refusal::Invalid(Invalid::TooLong) => TOO_LARGE.to_string(),
        Refusal::Invalid(Invalid::Empty) => "it changes nothing".to_string(),
        Refusal::Invalid(Invalid::Overlapping(path)) => {
            format!("two parts of the change are at “{path}”")
        }
        Refusal::Invalid(Invalid::UndeclaredKey(_) | Invalid::Unencodable { .. }) => {
            FAULT.to_string()
        }
    }
}

/// What the library keeps apart, as its view shows it.
fn kept<F: Fs>(library: &Library<F>) -> Vec<super::KeptApart> {
    let view = library.view();
    let items = view.items().iter().map(|item| Item {
        changes: listed(item, &view)
            .into_iter()
            .map(|listed| listed.change)
            .collect(),
        ..item.clone()
    });
    let items = items.filter(|item| item.kind == ItemKind::Run || !item.changes.is_empty());
    items.map(|item| super::KeptApart::of(&item)).collect()
}

/// What the answer to a scan shows, with every folder where it scanned every file.
fn viewed<F: Fs>(
    library: &Library<F>,
    shown: &mut Option<View>,
    refreshed: Refreshed,
    unfinishable: Vec<Leftover>,
) -> Viewed {
    let moves = refreshed.scan.moved.iter().filter_map(|moved| {
        let from = LibPath::parse(moved.from.as_str())?;
        Some((from, LibPath::parse(moved.to.as_str())?))
    });
    let unlisted = refreshed.unlisted.iter();
    let unlisted = unlisted.filter_map(|dir| LibPath::parse(dir.as_str()));
    let theirs = refreshed.orphaned.into_iter().filter(|orphan| !orphan.ours);
    let theirs = theirs.map(|orphan| Leftover {
        orphan,
        unfinishable: None,
    });
    let dirs = refreshed.dirs.map(|dirs| {
        let shown = dirs.iter().filter_map(|dir| LibPath::parse(dir.as_str()));
        shown.collect::<Vec<_>>()
    });
    Viewed {
        shown: show(shown, library.view()),
        unlisted: dirs.is_some().then(|| unlisted.collect()),
        dirs,
        moves: moves.collect(),
        changes: refreshed.changes,
        followed: None,
        unfinished: Some(theirs.chain(unfinishable).collect()),
        kept: kept(library),
        retired: refreshed.retired.is_some(),
        left: Vec::new(),
        finishing: shown_paths(&library.finishing()),
    }
}

/// What `view` changed of what the app was last `shown`: everything where it was shown
/// nothing yet.
fn show(shown: &mut Option<View>, view: View) -> Change {
    let change = Change::since(&view, shown.as_ref());
    *shown = Some(view);
    change
}

fn relative(path: &LibPath) -> Option<RelPath> {
    RelPath::new(path.as_str()).ok()
}

fn no_path(path: &LibPath) -> String {
    format!("{path} is no library path")
}

/// Every folder below the root drawbar lists, in path order: none whose name starts with
/// a dot, none reached through a link, and none past [`MOST_ENTRIES`] entries.
async fn folders<F: Fs>(fs: &F) -> Vec<LibPath> {
    let mut found = Vec::new();
    let mut waiting = VecDeque::from([RelPath::ROOT]);
    let mut looked = 0;
    while let Some(dir) = waiting.pop_front() {
        let Ok(entries) = storage::list(fs, Root::Folder, &dir).await else {
            continue;
        };
        for (name, meta) in entries {
            looked += 1;
            if looked > MOST_ENTRIES {
                break;
            }
            if meta.kind != Kind::Directory || name.starts_with('.') {
                continue;
            }
            let Ok(at) = dir.join(&name) else {
                continue;
            };
            if let Some(path) = LibPath::parse(at.as_str()) {
                found.push(path);
                waiting.push_back(at);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::sync::atomic::Ordering;

    use toshokan::env::SystemClock;

    use super::super::counted::{Counted, Signs};
    use super::super::native::{self, Blocking};
    use super::super::Moving;
    use super::*;
    use crate::ondisk::OnDisk;
    use crate::rewrite::Rewrite;
    use crate::testing::Temp;

    type Keeper = super::Keeper<Counted<Blocking>>;

    /// A keeper of the library in `root`, open, telling the time by `clock`.
    fn keeper_at(root: &Temp, clock: Box<dyn toshokan::Clock>) -> Keeper {
        let mut keeper = native::keeper(&root.0, &root.local(), Arc::default(), clock);
        let opened = keeper.run_one(Cmd::Open);
        assert!(matches!(opened[..], [Event::Opened(Ok(_))]), "{opened:?}");
        let listed = keeper.run_one(Cmd::Rescan);
        assert!(matches!(listed[..], [Event::Scanned(Ok(_))]), "{listed:?}");
        keeper
    }

    fn keeper(root: &Temp) -> Keeper {
        keeper_at(root, Box::new(SystemClock))
    }

    impl Keeper {
        /// Run one command, and every answer it gives.
        fn run_one(&mut self, cmd: Cmd) -> Vec<Event> {
            let mut answers = Vec::new();
            nord_usb::block_on(self.run(cmd, &mut |event| answers.push(event)));
            answers
        }

        fn trash_now(&mut self) -> Vec<toshokan::report::TrashItem> {
            let trash = self.library.as_mut().unwrap().trash();
            nord_usb::block_on(trash).unwrap()
        }

        /// The stat the library listed for the file at `path`.
        fn stat(&self, path: &str) -> Stat {
            let Expect::Listed { len, modified } = self.listed(path) else {
                unreachable!("a listing says how long a file is")
            };
            Stat { len, modified }
        }

        /// What the library knows the file at `path` by, as the app last read it.
        fn listed(&self, path: &str) -> Expect {
            let view = self.library.as_ref().unwrap().view();
            let listed = view.listed(&RelPath::new(path).unwrap()).expect(path);
            Expect::Listed {
                len: listed.len,
                modified: listed.modified,
            }
        }
    }

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    /// A batch that stops partway answers a deletion it never made as failed, so the app
    /// lists again the file it did not delete.
    #[cfg(unix)]
    #[test]
    fn a_deletion_a_batch_stopped_before_is_answered_as_failed() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = Temp::new();
        fs::write(root.at("a.ne5p"), b"a").unwrap();
        fs::create_dir(root.at("Locked")).unwrap();
        fs::write(root.at("Locked/b.ne5p"), b"b").unwrap();
        let mut keeper = keeper(&root);
        keeper.run_one(Cmd::Rescan);
        let (a, b) = (keeper.listed("a.ne5p"), keeper.listed("Locked/b.ne5p"));
        let mode = |mode| fs::Permissions::from_mode(mode);
        fs::set_permissions(root.at("Locked"), mode(0o555)).unwrap();
        let answers = keeper.run_one(Cmd::Intent(Intent::Batch(vec![
            Intent::Save {
                id: 1,
                path: path("a.ne5p"),
                bytes: b"saved".to_vec(),
                expect: a,
                came: None,
            },
            Intent::Delete {
                files: vec![(path("Locked/b.ne5p"), b)],
            },
        ])));
        fs::set_permissions(root.at("Locked"), mode(0o755)).unwrap();
        let failed = |event: &Event| matches!(event, Event::Failed(why) if why.starts_with("deleting Locked/b.ne5p"));
        assert!(answers.iter().any(failed), "{answers:?}");
        assert!(root.at("Locked/b.ne5p").exists());
    }

    /// A change too large for the library to log as one entry is refused whole, each
    /// file saying why, and the folder keeps every file.
    #[test]
    fn a_change_too_large_to_log_as_one_is_refused_whole_and_says_why() {
        const FILES: usize = 13_000;
        let root = Temp::new();
        let dir = format!("{}/{}", "a".repeat(200), "b".repeat(200));
        fs::create_dir_all(root.at(&dir)).unwrap();
        let name = |n: usize| format!("{dir}/{n:05}{}.ne5p", "c".repeat(190));
        for n in 0..FILES {
            fs::write(root.at(&name(n)), b"x").unwrap();
        }
        let mut keeper = keeper(&root);
        let files = (0..FILES).map(|n| (path(&name(n)), keeper.listed(&name(n))));
        let answers = keeper.run_one(Cmd::Intent(Intent::Delete {
            files: files.collect(),
        }));
        let refused =
            |event: &Event| matches!(event, Event::Failed(why) if why.ends_with(TOO_LARGE));
        assert_eq!(answers.iter().filter(|event| refused(event)).count(), FILES);
        assert!((0..FILES).all(|n| root.at(&name(n)).exists()));
        assert_eq!(keeper.trash_now(), []);
    }

    /// A save to the path of a tagged file deleted outside drawbar writes that entity's
    /// file, in sync, rather than a file of its own the entity then finds changed.
    #[test]
    fn a_save_where_a_tagged_file_went_missing_is_its_entitys_file() {
        let root = Temp::new();
        fs::write(root.at("Grand.ne5p"), b"grand").unwrap();
        let mut keeper = keeper(&root);
        keeper.run_one(Cmd::Intent(Intent::Tag {
            files: vec![Key::File(path("Grand.ne5p"))],
            name: "Sunday".into(),
            make: false,
        }));
        fs::remove_file(root.at("Grand.ne5p")).unwrap();
        keeper.run_one(Cmd::Rescan);
        let view = keeper.library.as_ref().unwrap().view();
        let entity = view.entities()[0].id();
        assert_eq!(
            view.entity(entity).unwrap().file().unwrap().state,
            toshokan::FileState::Missing
        );
        keeper.run_one(Cmd::Intent(Intent::Save {
            id: 1,
            path: path("Grand.ne5p"),
            bytes: b"grand, saved again".to_vec(),
            expect: Expect::Absent,
            came: None,
        }));
        let view = keeper.library.as_ref().unwrap().view();
        let file = view.entity(entity).unwrap().file().unwrap();
        assert_eq!(file.state, toshokan::FileState::InSync);
    }

    /// The keeper of the library in `root`, not opened yet.
    fn unopened(root: &Temp) -> Keeper {
        native::keeper(
            &root.0,
            &root.local(),
            Arc::default(),
            Box::new(SystemClock),
        )
    }

    /// A copy of the library in `from`, as File ▸ Save a copy of the library saves it,
    /// in a folder of its own.
    fn saved(from: &Temp) -> (Temp, std::path::PathBuf) {
        let saved = Temp::new();
        fs::write(saved.at("copy.zip"), super::super::copy::saved(&from.0)).unwrap();
        let at = saved.at("copy.zip");
        (saved, at)
    }

    #[test]
    fn a_copy_opened_into_an_empty_library_brings_its_files_and_their_tags() {
        let from = Temp::new();
        fs::create_dir(from.at("Set")).unwrap();
        fs::write(from.at("Set/Grand.ne5p"), b"grand").unwrap();
        let mut keeper = keeper(&from);
        keeper.run_one(Cmd::Intent(Intent::Tag {
            files: vec![Key::File(path("Set/Grand.ne5p"))],
            name: "Live".into(),
            make: false,
        }));
        nord_usb::block_on(keeper.close());
        let (_saved, copy) = saved(&from);

        let to = Temp::new();
        let mut keeper = unopened(&to);
        let restored = keeper.run_one(Cmd::Restore(copy));
        assert!(
            matches!(restored[..], [Event::Restored(Ok(1))]),
            "{restored:?}"
        );
        let opened = keeper.run_one(Cmd::Open);
        assert!(matches!(opened[..], [Event::Opened(Ok(_))]), "{opened:?}");
        keeper.run_one(Cmd::Rescan);
        assert_eq!(to.read("Set/Grand.ne5p"), b"grand");
        let view = keeper.library.as_ref().unwrap().view();
        let entity = view.entities()[0].id();
        let worn = view.entity(entity).unwrap().members(TAGS).values;
        assert_eq!(worn.into_iter().collect::<Vec<_>>(), ["Live"]);
    }

    #[test]
    fn a_copy_is_not_opened_into_a_library_that_holds_files() {
        let from = Temp::new();
        fs::write(from.at("a.ne5p"), b"a").unwrap();
        let (_saved, copy) = saved(&from);
        let to = Temp::new();
        fs::write(to.at("b.ne5p"), b"b").unwrap();
        let answers = unopened(&to).run_one(Cmd::Restore(copy));
        let [Event::Restored(Err(why))] = &answers[..] else {
            panic!("{answers:?}")
        };
        assert!(why.contains("already holds files"), "{why}");
        assert_eq!(to.names(""), ["b.ne5p"]);
    }

    #[test]
    fn a_damaged_copy_takes_back_what_it_wrote() {
        let from = Temp::new();
        fs::write(from.at("a.ne5p"), b"first").unwrap();
        fs::write(from.at("b.ne5p"), b"second").unwrap();
        let (saved, copy) = saved(&from);
        let mut bytes = fs::read(&copy).unwrap();
        let at = bytes.windows(6).position(|run| run == b"second").unwrap();
        bytes[at] ^= 1;
        fs::write(&copy, bytes).unwrap();
        let to = Temp::new();
        let answers = unopened(&to).run_one(Cmd::Restore(copy));
        let [Event::Restored(Err(why))] = &answers[..] else {
            panic!("{answers:?}")
        };
        assert!(why.contains("damaged"), "{why}");
        assert!(to.names("").is_empty(), "{:?}", to.names(""));
        drop(saved);
    }

    /// A copy is not opened into a library another window holds open, even one that has
    /// written nothing, and what that window saves meanwhile stays.
    #[test]
    fn a_copy_is_not_opened_into_a_library_another_window_has_open() {
        let from = Temp::new();
        fs::write(from.at("a.ne5p"), b"a").unwrap();
        let (_saved, copy) = saved(&from);
        let to = Temp::new();
        let mut other = keeper(&to);
        let answers = unopened(&to).run_one(Cmd::Restore(copy.clone()));
        let [Event::Restored(Err(why))] = &answers[..] else {
            panic!("{answers:?}")
        };
        assert!(why.contains("another tab or window"), "{why}");
        other.run_one(Cmd::Intent(Intent::Save {
            id: 1,
            path: path("theirs.ne5p"),
            bytes: b"theirs".to_vec(),
            expect: Expect::Absent,
            came: None,
        }));
        assert_eq!(to.read("theirs.ne5p"), b"theirs");
        nord_usb::block_on(other.close());
        drop(other);
        let answers = unopened(&to).run_one(Cmd::Restore(copy));
        let [Event::Restored(Err(why))] = &answers[..] else {
            panic!("{answers:?}")
        };
        assert!(why.contains("already holds files"), "once it closes: {why}");
        assert_eq!(to.read("theirs.ne5p"), b"theirs");
    }

    /// A save of `name`, as an asset new to the library.
    fn new_file(name: &str) -> Cmd {
        Cmd::Intent(Intent::Save {
            id: 1,
            path: path(name),
            bytes: name.as_bytes().to_vec(),
            expect: Expect::Absent,
            came: None,
        })
    }

    /// Every window holds a lock of its own: one past them all is refused before it
    /// writes anything, and opens once another closes. The windows holding one write
    /// side by side.
    #[test]
    fn a_window_past_every_lock_held_does_not_open_the_library() {
        let root = Temp::new();
        let mut windows: Vec<Keeper> = (0..16).map(|_| keeper(&root)).collect();
        for (at, window) in windows.iter_mut().take(2).enumerate() {
            let answers = window.run_one(new_file(&format!("{at}.ne5p")));
            assert!(
                answers
                    .iter()
                    .any(|event| matches!(event, Event::Saved { result: Ok(_), .. })),
                "{answers:?}"
            );
        }
        let names = root.names("");
        let mut last = native::keeper(
            &root.0,
            &root.local(),
            Arc::default(),
            Box::new(SystemClock),
        );
        let answers = last.run_one(Cmd::Open);
        let [Event::Opened(Err(why))] = &answers[..] else {
            panic!("{answers:?}")
        };
        assert!(why.contains("16 tabs or windows"), "{why}");
        assert_eq!(root.names(""), names, "nothing written");

        let mut closed = windows.pop().unwrap();
        nord_usb::block_on(closed.close());
        let mut next = keeper(&root);
        let answers = next.run_one(new_file("next.ne5p"));
        assert!(
            answers
                .iter()
                .any(|event| matches!(event, Event::Saved { result: Ok(_), .. })),
            "{answers:?}"
        );
    }

    /// One window of an install takes in the list an older drawbar kept: another tries
    /// nothing while the first holds the take-in lock, nor once the library holds the list,
    /// though it has not read the library since.
    #[test]
    fn a_take_in_another_window_makes_or_made_is_not_tried() {
        let root = Temp::new();
        let take_in = || {
            Cmd::Intent(Intent::Batch(vec![
                Intent::TakeIn {
                    tags: Vec::new(),
                    hash: "list".to_string(),
                },
                Intent::MakeDir { path: path("Gig") },
            ]))
        };
        let elsewhere = |answers: &[Event]| {
            let committed = answers.iter().any(|event| {
                matches!(
                    event,
                    Event::Committed {
                        made: None,
                        result: Ok(_),
                        ..
                    }
                )
            });
            matches!(answers.first(), Some(Event::TakenInElsewhere)) && committed
        };
        let mut first = keeper(&root);
        let mut second = keeper(&root);
        fn fs(keeper: &Keeper) -> &Counted<Blocking> {
            keeper.library.as_ref().unwrap().fs()
        }
        let held = nord_usb::block_on(fs(&first).perform(Io::Lock { name: taking_in() }));
        assert_eq!(held, Ok(Reply::Lock(Lock::Acquired)));
        let answers = second.run_one(take_in());
        assert!(elsewhere(&answers), "while the lock is held: {answers:?}");
        assert!(!root.names("").contains(&"Gig".to_string()), "nothing made");

        nord_usb::block_on(fs(&first).perform(Io::Unlock { name: taking_in() })).unwrap();
        first.run_one(take_in());
        assert!(root.names("").contains(&"Gig".to_string()), "taken in");
        let answers = second.run_one(take_in());
        assert!(elsewhere(&answers), "once taken in: {answers:?}");
    }

    /// Storage whose locks fail.
    struct Unlockable(Counted<Blocking>);

    impl Fs for Unlockable {
        fn capabilities(&self, root: Root) -> toshokan::io::Capabilities {
            self.0.capabilities(root)
        }

        async fn perform(&self, io: toshokan::Io) -> toshokan::io::IoResult {
            match io {
                toshokan::Io::Lock { .. } => Err(IoError::Other("no locks here".into())),
                io => self.0.perform(io).await,
            }
        }
    }

    #[test]
    fn a_window_whose_lock_fails_does_not_open_the_library() {
        let root = Temp::new();
        let mut native = native::keeper(
            &root.0,
            &root.local(),
            Arc::default(),
            Box::new(SystemClock),
        );
        let (fs, env) = native.unopened.take().unwrap();
        let mut keeper = super::Keeper::new(Unlockable(fs), env);
        let mut answers = Vec::new();
        nord_usb::block_on(keeper.run(Cmd::Open, &mut |event| answers.push(event)));
        let [Event::Opened(Err(why))] = &answers[..] else {
            panic!("{answers:?}")
        };
        assert!(why.contains("no locks here"), "{why}");
        assert!(root.names("").is_empty(), "nothing written");
    }

    #[test]
    fn a_copy_is_not_opened_into_a_library_already_open() {
        let from = Temp::new();
        let (_saved, copy) = saved(&from);
        let to = Temp::new();
        let answers = keeper(&to).run_one(Cmd::Restore(copy));
        assert!(
            matches!(&answers[..], [Event::Restored(Err(_))]),
            "{answers:?}"
        );
    }

    /// A library whose files are all gone holds only its history, which a copy replaces.
    #[test]
    fn a_copy_opens_over_the_history_of_a_library_whose_files_are_gone() {
        let from = Temp::new();
        fs::write(from.at("a.ne5p"), b"first").unwrap();
        let (_saved, copy) = saved(&from);
        let to = Temp::new();
        let old = to.at(".drawbar/library/writers.2/old");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("segment.txt"), b"history").unwrap();
        let answers = unopened(&to).run_one(Cmd::Restore(copy));
        assert!(
            matches!(answers[..], [Event::Restored(Ok(1))]),
            "{answers:?}"
        );
        assert_eq!(to.read("a.ne5p"), b"first");
        assert!(!old.exists(), "the old history is replaced");
    }

    /// A copy whose opening stopped partway leaves the library read-only, saying to open
    /// it again, and opening it again takes away what the first left.
    #[test]
    fn a_copy_stopped_partway_leaves_the_library_read_only_until_it_is_opened_again() {
        let from = Temp::new();
        fs::write(from.at("a.ne5p"), b"first").unwrap();
        let (_saved, copy) = saved(&from);
        let to = Temp::new();
        fs::create_dir_all(to.at(".drawbar")).unwrap();
        fs::write(to.at(super::super::copy::OPENING), b"").unwrap();
        fs::write(to.at("half.ne5p"), b"ha").unwrap();
        let opened = unopened(&to).run_one(Cmd::Open);
        let [Event::Opened(Ok(opened))] = &opened[..] else {
            panic!("{opened:?}")
        };
        assert!(opened.opening_copy);
        assert!(
            matches!(&opened.writable, Err(Unwritable::ReadOnly(why)) if why.contains(crate::menu::OPEN_COPY)),
            "{:?}",
            opened.writable
        );
        let answers = unopened(&to).run_one(Cmd::Restore(copy));
        assert!(
            matches!(answers[..], [Event::Restored(Ok(1))]),
            "{answers:?}"
        );
        assert_eq!(to.names(""), ["a.ne5p"], "the first opening's file is gone");
    }

    /// A scan leaves out every name that starts with a dot, as drawbar never lists one:
    /// nothing under such a folder is read for its identity.
    #[test]
    fn a_scan_leaves_out_names_that_start_with_a_dot() {
        let root = Temp::new();
        fs::create_dir(root.at(".git")).unwrap();
        fs::write(root.at(".git/hidden.ne5p"), b"hidden").unwrap();
        fs::write(root.at(".DS_Store"), b"hidden").unwrap();
        fs::write(root.at("shown.ne5p"), b"shown").unwrap();
        let keeper = keeper(&root);
        let view = keeper.library.as_ref().unwrap().view();
        let listed = |path: &str| view.listed(&RelPath::new(path).unwrap()).is_some();
        assert!(listed("shown.ne5p"));
        assert!(!listed(".git/hidden.ne5p"));
        assert!(!listed(".DS_Store"));
    }

    /// The trash keeps what a session's undo needs up to its size, and empties the
    /// oldest past it, naming the change whose undo needed them. The next open, once it
    /// settled, empties what a session stopped short left, and closing empties what the
    /// session's undo needed.
    #[test]
    fn the_trash_keeps_what_this_sessions_undo_needs_within_its_size_and_no_longer() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"first").unwrap();
        let save = |keeper: &mut Keeper, bytes: &[u8]| {
            let expect = keeper.listed("c3.ne5p");
            let answers = keeper.run_one(Cmd::Intent(Intent::Save {
                id: 1,
                path: path("c3.ne5p"),
                bytes: bytes.to_vec(),
                expect,
                came: None,
            }));
            let made = answers.iter().find_map(|event| match event {
                Event::Committed { made, .. } => *made,
                _ => None,
            });
            (made.expect("the save was made"), answers)
        };
        let trash = Keeper::trash_now;
        let mut first = keeper(&root);
        first.trash = 8;
        let (saved, answers) = save(&mut first, b"second");
        assert!(!answers
            .iter()
            .any(|event| matches!(event, Event::Trimmed(_))));
        let (_, answers) = save(&mut first, b"third");
        assert!(
            answers
                .iter()
                .any(|event| matches!(event, Event::Trimmed(entries) if *entries == [saved])),
            "{answers:?}"
        );
        assert_eq!(trash(&mut first).len(), 1, "what the last save replaced");
        drop(first);

        let mut second = keeper(&root);
        assert_eq!(trash(&mut second), [], "a session stopped short left it");
        save(&mut second, b"fourth");
        assert_eq!(trash(&mut second).len(), 1, "the session's undo needs it");
        nord_usb::block_on(second.close());
        let mut third = keeper(&root);
        assert_eq!(trash(&mut third), [], "the closed session's undo needed it");
    }

    /// Only this writer's trash is emptied: what an earlier writer of this install left
    /// stays, and alone it leaves nothing to empty.
    #[test]
    fn the_trash_of_an_earlier_writer_is_neither_emptied_nor_counted() {
        let item = |writer, n, len| TrashItem {
            writer: WriterId::from_u128(writer),
            item: Nonce::from_u128(n),
            len,
            from: RelPath::new("c3.ne5p").unwrap(),
            at: toshokan::Hlc {
                wall_ms: n as u64,
                counter: 0,
            },
            by: EntryHash::from_u128(n),
        };
        let reached = |_: &EntryHash| true;
        let (earlier, own) = (0xe, 0xa);
        let held = [item(own, 1, 4), item(earlier, 2, 4)];
        assert_eq!(keeping(&held, WriterId::from_u128(own), reached, 4), None);

        let held = [item(own, 1, 4), item(own, 2, 4), item(earlier, 3, 4)];
        let kept = keeping(&held, WriterId::from_u128(own), reached, 4);
        assert_eq!(
            kept,
            Some(BTreeSet::from([Nonce::from_u128(2)])),
            "the newest"
        );
    }

    /// A file another program leaves among drawbar's records keeps nothing in the trash,
    /// and a record that does not read keeps everything, saying what to move.
    #[test]
    fn only_a_record_that_does_not_read_stops_the_trash_emptying() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"first").unwrap();
        let mut keeper = keeper(&root);
        keeper.trash = 0;
        let save = |keeper: &mut Keeper, bytes: &[u8]| {
            let expect = keeper.listed("c3.ne5p");
            keeper.run_one(Cmd::Intent(Intent::Save {
                id: 1,
                path: path("c3.ne5p"),
                bytes: bytes.to_vec(),
                expect,
                came: None,
            }))
        };
        save(&mut keeper, b"second");
        let writers = root.at(".drawbar/library/writers.2");
        let writer = fs::read_dir(&writers).unwrap().next().unwrap().unwrap();
        let pending = writer.path().join("pending");
        fs::create_dir_all(&pending).unwrap();
        fs::write(pending.join(".DS_Store"), b"Bud1").unwrap();
        save(&mut keeper, b"third");
        assert_eq!(keeper.trash_now(), [], "the stray file holds nothing");

        fs::write(pending.join(format!("{:032x}.json", 7)), b"{\"torn").unwrap();
        let answers = save(&mut keeper, b"fourth");
        let why = answers.iter().find_map(|event| match event {
            Event::Unemptied(why) => Some(why.clone()),
            _ => None,
        });
        assert!(
            why.as_deref().is_some_and(|why| why.contains("pending")),
            "{answers:?}"
        );
        assert_eq!(keeper.trash_now().len(), 1, "the trash keeps what it holds");
    }

    #[test]
    fn an_answer_after_one_the_app_lost_shows_the_whole_view() {
        let root = Temp::new();
        fs::write(root.at("a.ne5p"), b"a").unwrap();
        let mut keeper = keeper(&root);
        let shown = |answers: Vec<Event>| match &answers[..] {
            [Event::Scanned(Ok(viewed))] => viewed.shown.clone(),
            _ => panic!("{answers:?}"),
        };
        let since = shown(keeper.run_one(Cmd::Rescan));
        assert!(!matches!(since, Change::Whole(_)), "{since:?}");
        keeper.forget_shown();
        let whole = shown(keeper.run_one(Cmd::Rescan));
        assert!(matches!(whole, Change::Whole(_)), "{whole:?}");
    }

    #[test]
    fn the_scan_that_first_lists_many_files_shows_the_whole_view() {
        let root = Temp::new();
        for i in 0..1100 {
            fs::write(root.at(&format!("{i:04}.ne5p")), b"program").unwrap();
        }
        let clock = Box::new(SystemClock);
        let mut keeper = native::keeper(&root.0, &root.local(), Arc::default(), clock);
        keeper.run_one(Cmd::Open);
        let listed = keeper.run_one(Cmd::Rescan);
        let [Event::Scanned(Ok(viewed))] = &listed[..] else {
            panic!("{listed:?}")
        };
        assert!(matches!(viewed.shown, Change::Whole(_)));
        fs::write(root.at("new.ne5p"), b"program").unwrap();
        let listed = keeper.run_one(Cmd::Rescan);
        let [Event::Scanned(Ok(viewed))] = &listed[..] else {
            panic!("{listed:?}")
        };
        assert!(!matches!(viewed.shown, Change::Whole(_)), "one more file");
        let tagged = keeper.run_one(Cmd::Intent(Intent::Tag {
            files: vec![Key::File(path("new.ne5p"))],
            name: "Live".into(),
            make: true,
        }));
        assert!(
            matches!(tagged[..], [Event::Committed { result: Ok(_), .. }]),
            "{tagged:?}"
        );
        fs::create_dir(root.at("More")).unwrap();
        for i in 0..2300 {
            fs::write(root.at(&format!("More/{i:04}.wav")), b"wave").unwrap();
        }
        let listed = keeper.run_one(Cmd::Rescan);
        let [Event::Scanned(Ok(viewed))] = &listed[..] else {
            panic!("{listed:?}")
        };
        let Change::Whole(whole) = &viewed.shown else {
            panic!("most of it changed")
        };
        let shown = (whole.files.len(), whole.others().len());
        assert_eq!(shown, (1101, 2300), "every file, those before included");
    }

    #[test]
    fn the_scan_after_opening_counts_the_files_it_finds() {
        let root = Temp::new();
        fs::create_dir(root.at("Live")).unwrap();
        for name in ["a.ne5p", "Live/b.ne5p", "Live/c.wav", ".DS_Store"] {
            fs::write(root.at(name), name).unwrap();
        }
        let signs = Arc::new(Signs::default());
        let clock = Box::new(SystemClock);
        let mut keeper = native::keeper(&root.0, &root.local(), signs.clone(), clock);
        keeper.run_one(Cmd::Open);
        keeper.run_one(Cmd::Rescan);
        assert_eq!(signs.listed.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_rename_that_changes_only_case_goes_through() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"lower").unwrap();
        let mut keeper = keeper(&root);
        let expect = keeper.listed("c3.ne5p");
        let moved = keeper.run_one(Cmd::Intent(Intent::Move {
            moves: vec![Moving {
                from: path("c3.ne5p"),
                to: path("C3.ne5p"),
                expect,
            }],
        }));
        assert!(
            matches!(moved[0], Event::Moved { result: Ok(()), .. }),
            "{moved:?}"
        );
        assert_eq!(root.names(""), [".drawbar", "C3.ne5p"]);
        assert_eq!(root.read("C3.ne5p"), b"lower");
    }

    /// Only a disk that tells case apart can hold both names, so elsewhere this checks
    /// nothing.
    #[test]
    fn a_rename_onto_another_file_one_case_apart_is_refused() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"lower").unwrap();
        fs::write(root.at("C3.ne5p"), b"upper").unwrap();
        if root.names("").len() < 2 {
            return;
        }
        let mut keeper = keeper(&root);
        let expect = keeper.listed("c3.ne5p");
        let moved = keeper.run_one(Cmd::Intent(Intent::Move {
            moves: vec![Moving {
                from: path("c3.ne5p"),
                to: path("C3.ne5p"),
                expect,
            }],
        }));
        assert!(
            matches!(moved[0], Event::Moved { result: Err(_), .. }),
            "{moved:?}"
        );
        assert_eq!(root.read("c3.ne5p"), b"lower");
        assert_eq!(root.read("C3.ne5p"), b"upper");
    }

    /// A file that changed since drawbar read it is neither saved over nor deleted.
    #[test]
    fn a_file_changed_since_it_was_read_is_left_as_it_is() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"contents").unwrap();
        let mut keeper = keeper(&root);
        let read = Expect::Holds(keys::identity_of_bytes(&path("c3.ne5p"), b"contents").unwrap());
        fs::write(root.at("c3.ne5p"), b"changed!").unwrap();
        let saved = keeper.run_one(Cmd::Intent(Intent::Save {
            id: 1,
            path: path("c3.ne5p"),
            bytes: b"mine".to_vec(),
            expect: read,
            came: None,
        }));
        assert!(
            matches!(
                saved[0],
                Event::Saved {
                    result: Err(Failure::Moved),
                    ..
                }
            ),
            "{saved:?}"
        );
        let deleted = keeper.run_one(Cmd::Intent(Intent::Delete {
            files: vec![(path("c3.ne5p"), read)],
        }));
        assert!(matches!(deleted[0], Event::Failed(_)), "{deleted:?}");
        assert_eq!(root.read("c3.ne5p"), b"changed!");
    }

    /// A save the commit landed is answered as saved with what it wrote, though the file
    /// cannot be looked at right after.
    #[test]
    fn a_landed_save_that_cannot_be_looked_at_after_is_saved_with_what_it_wrote() {
        struct Unstated(toshokan::MemDisk);
        impl Fs for Unstated {
            fn capabilities(&self, root: Root) -> toshokan::io::Capabilities {
                self.0.capabilities(root)
            }

            async fn perform(&self, io: toshokan::Io) -> toshokan::io::IoResult {
                match io {
                    toshokan::Io::Stat { .. } => Err(IoError::Denied),
                    io => Fs::perform(&self.0, io).await,
                }
            }
        }
        let answering = Answering::Saved {
            id: 1,
            path: path("c3.ne5p"),
            len: 3,
            crc: 7,
            identity: None,
        };
        let landed = nord_usb::block_on(answering.landed(&Unstated(toshokan::MemDisk::new())));
        let Some(Event::Saved {
            result: Ok(print), ..
        }) = landed
        else {
            panic!("{landed:?}")
        };
        assert_eq!((print.len, print.modified, print.crc), (3, None, Some(7)));
    }

    /// Storage that runs `meddle` once, right after the first read of `at` in the folder
    /// once armed, as another program changing that file mid-copy would.
    struct Meddled {
        inner: Counted<Blocking>,
        at: RelPath,
        meddle: RefCell<Option<Box<dyn FnOnce()>>>,
    }

    impl Fs for Meddled {
        fn capabilities(&self, root: toshokan::io::Root) -> toshokan::io::Capabilities {
            self.inner.capabilities(root)
        }

        async fn perform(&self, io: toshokan::Io) -> toshokan::io::IoResult {
            let reads = matches!(&io, toshokan::Io::Read { root: Root::Folder, path, .. } if *path == self.at);
            let done = self.inner.perform(io).await;
            let meddle = reads.then(|| self.meddle.take()).flatten();
            if let Some(meddle) = meddle {
                meddle();
            }
            done
        }
    }

    /// A copy of a library file that another program rewrites while it is read, range by
    /// range, is refused as moved, and nothing lands.
    #[test]
    fn a_copy_of_a_file_rewritten_while_it_is_read_is_refused() {
        let root = Temp::new();
        let len = 2 * CHUNK as usize + 1;
        fs::write(root.at("Long.nsmp"), vec![b'a'; len]).unwrap();
        let mut counted = native::keeper(
            &root.0,
            &root.local(),
            Arc::default(),
            Box::new(SystemClock),
        );
        let (inner, env) = counted.unopened.take().unwrap();
        let fs = Meddled {
            inner,
            at: RelPath::new("Long.nsmp").unwrap(),
            meddle: RefCell::new(None),
        };
        let mut keeper = super::Keeper::new(fs, env);
        let run = |keeper: &mut super::Keeper<Meddled>, cmd| {
            let mut answers = Vec::new();
            nord_usb::block_on(keeper.run(cmd, &mut |event| answers.push(event)));
            answers
        };
        run(&mut keeper, Cmd::Open);
        run(&mut keeper, Cmd::Rescan);
        let listed = keeper.library.as_ref().unwrap().view();
        let listed = listed.listed(&RelPath::new("Long.nsmp").unwrap()).unwrap();
        let stat = Stat {
            len: listed.len,
            modified: listed.modified,
        };
        let rewrite = root.at("Long.nsmp");
        *storage_of(&keeper.library).unwrap().meddle.borrow_mut() = Some(Box::new(move || {
            fs::write(rewrite, vec![b'b'; len + 1]).unwrap();
        }));
        let answers = run(
            &mut keeper,
            Cmd::Intent(Intent::Import {
                id: 1,
                path: path("Copy.nsmp"),
                from: Source::Library(path("Long.nsmp"), Fingerprint::unread(stat)),
                expect: Expect::Absent,
                came: None,
            }),
        );
        assert!(
            matches!(
                answers[..],
                [
                    Event::Imported {
                        result: Err(Failure::Moved),
                        ..
                    },
                    ..
                ]
            ),
            "{answers:?}"
        );
        assert!(!root.at("Copy.nsmp").exists(), "nothing lands");
    }

    /// A folder of the library that is a link to one outside it is never written through:
    /// a save, a copy, a move and a delete whose path passes through it are each refused,
    /// and the folder it names keeps what it holds.
    #[cfg(unix)]
    #[test]
    fn a_path_through_a_linked_folder_is_refused() {
        let (root, outside) = (Temp::new(), Temp::new());
        fs::write(outside.at("x.ne5p"), b"outside").unwrap();
        std::os::unix::fs::symlink(&outside.0, root.at("Linked")).unwrap();
        fs::write(root.at("a.ne5p"), b"inside").unwrap();
        let mut keeper = keeper(&root);
        let a = keeper.listed("a.ne5p");
        let x = Expect::Holds(keys::identity_of_bytes(&path("Linked/x.ne5p"), b"outside").unwrap());
        let intents = [
            Intent::Save {
                id: 1,
                path: path("Linked/new.ne5p"),
                bytes: b"saved".to_vec(),
                expect: Expect::Absent,
                came: None,
            },
            Intent::Save {
                id: 2,
                path: path("Linked/x.ne5p"),
                bytes: b"saved".to_vec(),
                expect: x,
                came: None,
            },
            Intent::Import {
                id: 3,
                path: path("Linked/copy.ne5p"),
                from: Source::Library(path("a.ne5p"), Fingerprint::unread(keeper.stat("a.ne5p"))),
                expect: Expect::Absent,
                came: None,
            },
            Intent::Move {
                moves: vec![Moving {
                    from: path("a.ne5p"),
                    to: path("Linked/a.ne5p"),
                    expect: a,
                }],
            },
            Intent::Delete {
                files: vec![(path("Linked/x.ne5p"), x)],
            },
        ];
        for intent in intents {
            let label = format!("{intent:?}");
            let answers = keeper.run_one(Cmd::Intent(intent));
            let refused = match &answers[0] {
                Event::Saved { result, .. } => result.is_err(),
                Event::Imported { result, .. } => result.is_err(),
                Event::Moved { result, .. } => result.is_err(),
                Event::Failed(_) => true,
                _ => false,
            };
            assert!(refused, "{label}: {answers:?}");
            let read_only = answers.iter().any(|e| matches!(e, Event::ReadOnly(_)));
            assert!(
                !read_only,
                "{label}: the library stays writable: {answers:?}"
            );
        }
        assert_eq!(outside.names(""), ["x.ne5p"]);
        assert_eq!(outside.read("x.ne5p"), b"outside");
        assert_eq!(root.read("a.ne5p"), b"inside");
    }

    /// A piano library the size a vendor ships, in 64 strokes.
    fn large_piano() -> Vec<u8> {
        use nord_format::formats::npno::synthetic::{take, Build};
        use nord_format::formats::npno::Bank;

        Build {
            version: 0x464,
            channels: 1,
            takes: (21..85)
                .map(|root| take(root, Bank::Attack, 0, 3_000))
                .collect(),
            map: (21..85).map(|key| (key, key)).collect(),
        }
        .bytes()
        .expect("the builder lays out a library")
    }

    fn resting(root: &Temp, name: &str) -> Arc<OnDisk> {
        let file = File::open(root.at(name)).unwrap();
        Arc::new(OnDisk::open(file, None).unwrap().unwrap())
    }

    /// A plan over a piano library of a vendor's size is saved through its file: each
    /// kept stroke is read and written by its range, and nothing near the library's size
    /// is ever allocated.
    #[test]
    fn a_large_piano_plan_is_saved_with_no_buffer_near_its_size() {
        let root = Temp::new();
        let bytes = large_piano();
        let len = bytes.len();
        assert!(len > 190_000_000, "{len} bytes is a vendor's size");
        fs::write(root.at("Grand.npno"), &bytes).unwrap();
        drop(bytes);
        let mut keeper = keeper(&root);
        let from = resting(&root, "Grand.npno");
        let crate::ondisk::Index::Piano(index) = &from.index else {
            panic!("a piano library")
        };
        let mut library = index.library().clone();
        library.set_name("Trimmed").unwrap();
        library.retain_strokes(|stroke| stroke.root % 2 == 0);
        let pieces = Arc::new(Rewrite::Piano(library).pieces(&from).unwrap());
        let expect = keeper.listed("Grand.npno");

        let (answers, largest) = crate::testing::largest_allocation(|| {
            keeper.run_one(Cmd::Intent(Intent::Rewrite {
                id: 1,
                path: path("Grand.npno"),
                pieces,
                expect,
            }))
        });
        // One stroke's audio is the most held at once.
        assert!(
            (1 << 20..8 << 20).contains(&largest),
            "{largest} bytes held at once"
        );
        assert!(
            matches!(answers[0], Event::Rewritten { result: Ok(_), .. }),
            "{answers:?}"
        );
        let saved = resting(&root, "Grand.npno");
        let crate::ondisk::Index::Piano(index) = &saved.index else {
            panic!("a piano library")
        };
        assert_eq!(index.library().strokes().len(), 32);
        assert_eq!(index.library().name().0, "Trimmed");
        let info = nord_format::cbin::inspect(&mut File::open(root.at("Grand.npno")).unwrap());
        assert!(info.unwrap().checksum_ok);
        assert_eq!(
            root.names(""),
            [".drawbar", "Grand.npno"],
            "no copy is left"
        );
    }

    /// A piano library of a vendor's size is copied byte for byte, and copied with an
    /// edit written through it, with nothing near its size held at once.
    #[test]
    fn a_large_piano_is_copied_with_no_buffer_near_its_size() {
        let root = Temp::new();
        let bytes = large_piano();
        fs::write(root.at("Grand.npno"), &bytes).unwrap();
        let mut keeper = keeper(&root);
        let from = resting(&root, "Grand.npno");
        let crate::ondisk::Index::Piano(index) = &from.index else {
            panic!("a piano library")
        };
        let mut library = index.library().clone();
        library.set_name("Edited").unwrap();
        let stat = keeper.stat("Grand.npno");
        let import = |name: &str, from: Source| Intent::Import {
            id: 1,
            path: path(name),
            from,
            expect: Expect::Absent,
            came: None,
        };
        let copied = import(
            "Copy.npno",
            Source::Library(path("Grand.npno"), Fingerprint::unread(stat)),
        );
        let pieces = Rewrite::Piano(library).pieces(&from).unwrap();
        let edited = import(
            "Edited.npno",
            Source::Edited(
                path("Grand.npno"),
                Fingerprint::unread(stat),
                Arc::new(pieces),
            ),
        );

        for (name, intent) in [("Copy.npno", copied), ("Edited.npno", edited)] {
            let (answers, largest) =
                crate::testing::largest_allocation(|| keeper.run_one(Cmd::Intent(intent)));
            assert!(largest < 8 << 20, "{name}: {largest} bytes held at once");
            assert!(
                matches!(answers[0], Event::Imported { result: Ok(_), .. }),
                "{name}: {answers:?}"
            );
        }
        assert!(root.read("Copy.npno") == bytes);
        let info = nord_format::cbin::inspect(&mut File::open(root.at("Edited.npno")).unwrap());
        assert!(info.unwrap().checksum_ok);
    }
}
