//! The app's side of a library: the library's view projected onto the workspace and the
//! browser's tags, and the intents the user's changes become.
//!
//! What the facts say comes only from the view: a tag shows once its intent is
//! committed, and another writer's change shows once a refresh reads it. A change to a
//! file is asked for where the user makes it ([`crate::workspace::FileAsk`]), and
//! [`Store::sync`] makes each an intent in turn: a new asset's file, a save, a copy, a
//! move or a deletion, each checked against what drawbar last read of the file.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use toshokan::report::{By, What};

use super::diff;
use super::drafts::Draft;
use super::exec::too_much;
use super::ids::Ids;
use super::keys::{Came, TAGS, TAG_NAMES};
use super::shown::Shown;
use super::{
    names, older, Backend, Cmd, CopyOf, Event, Failure, Fingerprint, Found, Holds, Intent, Key,
    LibPath, Moving, Opened, Rescue, Source, Stat, Unwritable, Viewed, Waiting, MOST_BYTES,
    TAKE_IN,
};
use crate::browser::{Browser, Rescuing};
use crate::folders::{Folders, Op, Where};
use crate::log::Log;
use crate::queue::Queue;
use crate::tags::Asked;
use crate::workspace::{
    library_filename, precious, Content, FileAsk, Leaving, LocalEntity, Origin, Saved, Workspace,
};

/// How far opening has got.
/// How long a command may go without a sign of life, as an answer or a file found,
/// before the user is asked whether to close and reopen the library.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);

/// How long, in seconds, a change another drawbar left unfinished must stand before
/// the user is asked about it. ⚠️ A drawbar another install runs on this folder, as a
/// browser tab beside the desktop app, is no `live` writer here, and may be carrying
/// the change out still.
const UNFINISHED_STANDS: f64 = 5.0;

/// A change another drawbar left unfinished: when this session first found it, and
/// whether the user was told of it.
struct Unfinished {
    leftover: super::Leftover,
    seen: f64,
    told: bool,
}

enum Phase {
    Opening,
    Open,
    /// Opened, and nothing may be written, for this reason.
    ReadOnly(String),
    /// Opened, and nothing may be written to the folder until it opens again, since a
    /// change of drawbar's own could not be finished, for this reason. Unsaved edits
    /// are still kept.
    Unfinished(String),
    /// Not opened, for this reason.
    Failed(String),
}

/// How much one [`Store::sync`] writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The files: new ones, saves, moves and deletions.
    Files,
    /// The files, and the unsaved edits, on the autosave cadence.
    Full,
    /// A full pass at exit, which runs even while a rescan is in flight, since nothing
    /// will read its answer.
    Last,
}

/// What drawbar knows of one asset's file.
struct Record {
    path: Option<LibPath>,
    /// What the file held when drawbar last read or wrote it. `None` until the first
    /// save of a new file lands.
    fingerprint: Option<Fingerprint>,
    /// The [`crate::workspace::Baseline::stamp`] of the bytes the file holds, or is
    /// being written with.
    saved: u64,
    /// A save has been sent and not answered.
    saving: bool,
    /// The file went missing outside drawbar. Nothing writes it again until the asset
    /// is saved.
    missing: bool,
    /// How drawbar holds what the file holds, while nothing has been saved over it.
    holds: Holds,
}

use super::RESCUED;

/// Whether an asset's file changed since the draft put back over it was made.
#[derive(Clone, Copy)]
enum Since {
    Same,
    Changed,
    /// Changed unless the file, once read, holds the bytes of this hash.
    Unless(blake3::Hash),
}

/// A view of a slot, as its unsaved edit is kept.
struct SlotView {
    slot: super::Slot,
    /// The stamp of the bytes the view was opened over or last sent, and their hash.
    saved: u64,
    base: blake3::Hash,
    /// Whether the edit an earlier session kept of the slot was read.
    read: bool,
    /// The stamp of the edit kept, if one is, and the hash it was kept over, once the
    /// keep is answered.
    kept: Option<(u64, blake3::Hash)>,
}

/// The changes to files and folders one gesture asked for, sent as one change.
#[derive(Default)]
struct Batch {
    /// The take of the asks they came in.
    take: u64,
    parts: Vec<Intent>,
}

/// The take-in of the list an older drawbar kept, sent and not answered: the assets it
/// brings, what the user is told once it lands, and the answers of its parts that did
/// not land, held until its change answers, with the first reason among them.
struct TakingIn {
    ids: BTreeSet<u64>,
    said: String,
    held: Vec<Event>,
    why: Option<String>,
}

/// How many files one background [`Cmd::Read`] asks for at most, and how many of
/// their listed bytes. A read something needs runs after it.
const FETCHES: (usize, u64) = (32, 16 << 20);

pub struct Store {
    backend: Backend,
    phase: Phase,
    records: BTreeMap<u64, Record>,
    /// The key the library knows each asset's file by.
    ids: Ids,
    /// What the library shows, as the answers so far changed it.
    shown: Option<Shown>,
    /// Changes to the tags the user asked for that wait for a file to name.
    asked: Vec<Asked>,
    /// Changes to files the user asked for, each carried out once the one before it has
    /// gone and nothing it waits on is in flight.
    asks: VecDeque<(FileAsk, u64)>,
    /// How many passes sent what they gathered: the asks taken before a pass ends are
    /// one gesture's, and carry its number.
    passes: u64,
    /// The changes gathered to send as one change.
    batch: Batch,
    /// The asset each overwrite came from, by the asset it overwrites, whose file the
    /// overwrite's change deletes.
    deleting_with: BTreeMap<u64, u64>,
    /// The projection in hand follows this session's undo or redo, and says nothing of
    /// what it finds moved or changed.
    quiet: bool,
    /// Assets read again after an undo or redo changed their files, which take what the
    /// files hold now without a word.
    quietly: BTreeSet<u64>,
    /// The paths a change of drawbar's own names that its next change finishes.
    finishing: BTreeSet<LibPath>,
    /// Assets whose file waits for such a change, which take what it leaves.
    awaiting: BTreeSet<u64>,
    /// This session's changes, as the Edit menu names them.
    history: History,
    /// What each change, undo or redo sent and not answered is, in order.
    answering: VecDeque<Step>,
    /// Assets whose file changed outside drawbar while they held it, read again to see
    /// what it holds now.
    rechecking: BTreeSet<u64>,
    /// The assets whose files the rescan in flight found moved or changed.
    moved: BTreeSet<u64>,
    /// The unsaved edits kept as drafts, by asset: the key each is kept under, the stamp
    /// of what it holds, and the identity of the file it was made over.
    drafted: BTreeMap<u64, (Key, u64, toshokan::Identity)>,
    /// The drafts sent and not answered yet, in the order sent, as [`Store::drafted`]
    /// holds each once it is kept.
    drafting: VecDeque<(u64, Key, u64, toshokan::Identity)>,
    /// The drafts an earlier session kept, each put back over its asset once its file is
    /// read: the key it is kept under, what it holds, whether its file changed since the
    /// draft was made, and the clock it was kept at.
    restoring: BTreeMap<u64, (Key, Draft, Since, u64)>,
    /// The keys of drafts put back under a key the asset no longer goes by, and of the
    /// older of two drafts of one asset, each let go of once this instance keeps the
    /// edit itself.
    superseded: BTreeMap<u64, Vec<Key>>,
    /// The instrument attached, as [`super::Slot::instrument`] names it.
    instrument: Option<String>,
    /// Each view of a slot seen, with what its unsaved edit is kept by.
    views: BTreeMap<u64, SlotView>,
    /// The keeps of slot views' edits sent and not answered yet, in the order sent, as
    /// [`SlotView::kept`] holds each once it is kept, or `None` where the view's edit
    /// was let go of since.
    keeping: VecDeque<(u64, Option<(u64, blake3::Hash)>)>,
    /// Whether the log has said an unsaved edit could not be kept since a keep last
    /// succeeded.
    told_unkept: bool,
    /// The change answered next is the one that turned the library read-only, which
    /// the log has said already.
    turned_read_only: bool,
    /// What names were last compared in for duplicates: the workspace's layout, and the
    /// [`crate::folders::Folders::names_hash`].
    compared: Option<(u64, u64)>,
    /// The assets a projection passed over while a save or a rename of their files was
    /// in flight, which the next one looks at again.
    deferred: BTreeSet<u64>,
    /// Assets to remove now what was copied or saved of them over another has landed
    /// ([`Store::take_left`]).
    left: Vec<u64>,
    /// Whether the window had focus at the last [`Store::focus`].
    focused: bool,
    /// A rescan or a refresh is in flight.
    scanning: bool,
    /// The scan of every file that follows opening is in flight: the view lists no file
    /// yet.
    first_scan: bool,
    /// How long a command may go without a sign of life before the user is asked.
    patience: std::time::Duration,
    /// Whether the user was asked about the command that shows no life now.
    asked_stalled: bool,
    /// The runs of changes kept apart the log has told of.
    kept_runs: BTreeSet<toshokan::log::Run>,
    /// The unsaved edits an earlier session kept, taken up once a scan lists the files
    /// they were made over.
    drafts_found: Vec<Result<super::Kept, String>>,
    /// Whether a scan of every file has listed the files since the library opened.
    listed: bool,
    /// The changes other drawbars left unfinished that the last read found, by writer
    /// and record.
    unfinished: BTreeMap<(toshokan::WriterId, toshokan::Nonce), Unfinished>,
    /// When the changes left unfinished were last read, as
    /// [`eframe::egui::InputState::time`] counts.
    looked: f64,
    /// When to read them again, to ask about one that stood long enough.
    recheck: Option<f64>,
    /// What another tab or the folder said changed and is not read yet: whether it takes
    /// a rescan rather than a refresh.
    #[cfg(target_arch = "wasm32")]
    behind: Option<bool>,
    /// An intent, an undo or a redo is in flight.
    committing: usize,
    /// A send is waiting for the rescan in flight.
    send_waits: bool,
    /// The library's name for the browser, or `None` for This computer's own.
    name: Option<String>,
    /// The tracked assets whose file is still to be read in the background, and every
    /// one asked for so far, each asked for once unless it was refused for room. See
    /// [`Store::fetch_tracked`].
    unfetched: std::collections::VecDeque<u64>,
    fetched: BTreeSet<u64>,
    /// The assets of the background read in flight.
    fetching: BTreeSet<u64>,
    /// A read something needs is in flight. The next waits for its answer, so whatever is
    /// needed most by then goes first.
    reading: bool,
    /// Each rename sent that has not answered, from and to.
    moving: Vec<(LibPath, LibPath)>,
    /// The most bytes of the library's files the assets may hold whole: [`MOST_BYTES`].
    budget: u64,
    /// How many files have been asked of the backend to read.
    #[cfg(test)]
    pub(crate) asked_files: usize,
    /// The assets whose read was refused for want of room, each with its file's length,
    /// to ask for again once room can be made for it. One that could never fit is not.
    roomless: BTreeMap<u64, u64>,
    /// How many times room has been looked for by going through every asset.
    #[cfg(test)]
    pub(crate) looked_for_room: usize,
    /// How many writing commands have been sent.
    sent: u64,
    /// Where drawbar keeps a slot's former occupant no library could take, whose files
    /// are taken in with the library's own: see [`Rescue`].
    #[cfg(not(target_arch = "wasm32"))]
    shelf: Option<std::path::PathBuf>,
    /// The rescues being copied into the library, by the asset each becomes, each
    /// deleted once its copy lands.
    shelved: BTreeMap<u64, std::path::PathBuf>,
    /// The folders that keep the slots' former occupants are to be looked in: the
    /// library has been listed, or a write has left one since.
    rescues_due: bool,
    /// Whether this session took up the list an older drawbar kept: sent its take-in, or
    /// put back what a take-in left.
    took_up: bool,
    /// That take-in, until its change answers.
    taking_in: Option<TakingIn>,
    /// The assets the list brought this session, which the list is let go only once
    /// each holds durably: [`Store::handed_over`].
    brought: BTreeSet<u64>,
    /// Whether the library opened holding a copy that stopped partway as it was opened.
    opening_copy: bool,
}

impl Store {
    /// Open the copy `from` into the library the backend holds, which must hold no files,
    /// and then open the library, which reads what the copy held.
    #[cfg(target_arch = "wasm32")]
    pub fn restore(mut backend: Backend, from: super::Outside) -> Store {
        backend.send(Cmd::Restore(from));
        Store::start(backend)
    }

    /// Open the library the backend holds.
    pub fn start(mut backend: Backend) -> Store {
        backend.send(Cmd::Open);
        Store {
            backend,
            phase: Phase::Opening,
            records: BTreeMap::new(),
            ids: Ids::default(),
            shown: None,
            asked: Vec::new(),
            asks: VecDeque::new(),
            passes: 0,
            batch: Batch::default(),
            deleting_with: BTreeMap::new(),
            quiet: false,
            quietly: BTreeSet::new(),
            finishing: BTreeSet::new(),
            awaiting: BTreeSet::new(),
            history: History::default(),
            answering: VecDeque::new(),
            rechecking: BTreeSet::new(),
            moved: BTreeSet::new(),
            drafted: BTreeMap::new(),
            drafting: VecDeque::new(),
            restoring: BTreeMap::new(),
            superseded: BTreeMap::new(),
            instrument: None,
            views: BTreeMap::new(),
            keeping: VecDeque::new(),
            told_unkept: false,
            turned_read_only: false,
            compared: None,
            deferred: BTreeSet::new(),
            focused: true,
            scanning: false,
            first_scan: false,
            patience: PATIENCE,
            asked_stalled: false,
            kept_runs: BTreeSet::new(),
            drafts_found: Vec::new(),
            listed: false,
            unfinished: BTreeMap::new(),
            looked: 0.0,
            recheck: None,
            #[cfg(target_arch = "wasm32")]
            behind: None,
            committing: 0,
            send_waits: false,
            name: None,
            unfetched: Default::default(),
            fetched: BTreeSet::new(),
            fetching: BTreeSet::new(),
            reading: false,
            moving: Vec::new(),
            budget: MOST_BYTES,
            #[cfg(test)]
            asked_files: 0,
            roomless: BTreeMap::new(),
            #[cfg(test)]
            looked_for_room: 0,
            sent: 0,
            #[cfg(not(target_arch = "wasm32"))]
            shelf: None,
            shelved: BTreeMap::new(),
            rescues_due: false,
            took_up: false,
            taking_in: None,
            brought: BTreeSet::new(),
            opening_copy: false,
            left: Vec::new(),
        }
    }

    /// The same store, taking in on open the rescues left in `shelf`, drawbar's own data.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn shelving(self, shelf: Option<std::path::PathBuf>) -> Store {
        Store { shelf, ..self }
    }

    /// The same store, heading the browser under `name` rather than as This computer.
    pub fn named(self, name: String) -> Store {
        Store {
            name: Some(name),
            ..self
        }
    }

    /// The folder the library is.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn root(&self) -> &std::path::Path {
        self.backend.root()
    }

    /// Where the library is.
    #[cfg(target_arch = "wasm32")]
    pub fn root(&self) -> &super::Root {
        self.backend.root()
    }

    /// Where a slot's former occupant is kept while this library is open: this install's
    /// own folder for it, outside the library, from which it goes into the library.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tmp(&self) -> Option<std::path::PathBuf> {
        self.open().then(|| self.backend.local().join(RESCUED))
    }

    /// Where a slot's former occupant is kept while this library is open: this install's
    /// own folder for it in the browser's storage, from which it goes into the library.
    #[cfg(target_arch = "wasm32")]
    pub fn tmp(&self) -> Option<toshokan::RelPath> {
        let rescued = self.backend.root().local().join(RESCUED);
        self.open()
            .then(|| rescued.expect("a folder's name is one component"))
    }

    /// Send the saves the files need, then say whether no save or rescan waits for its
    /// answer. A library whose answers cannot be waited for is let go only once settled,
    /// so its last pass sends no save.
    pub fn settled(&mut self, workspace: &mut Workspace, browser: &mut Browser) -> bool {
        self.sync(workspace, browser, Pass::Files);
        let carried = self.asks.is_empty() || !self.open();
        let idle = !self.scanning && !self.saving() && self.moving.is_empty();
        carried && idle && self.committing == 0
    }

    fn saving(&self) -> bool {
        self.records.values().any(|record| record.saving)
    }

    /// The assets whose copy or overwrite over another has landed since the last call, to
    /// remove as the user would.
    pub fn take_left(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.left)
    }

    /// Whether anything sent to the disk has not landed yet: a command the backend has not
    /// run through, or a save or rename whose answer is not folded in.
    pub fn writing(&self) -> bool {
        let asked = !self.asks.is_empty() && self.open();
        self.backend.busy() || self.saving() || !self.moving.is_empty() || asked
    }

    /// Whether letting the library go now, with no last pass, would lose an edit: an
    /// unsaved one, an asset never written to a file, a write that has not landed, a
    /// slot view's edit not kept yet, or a change held in memory where nothing may be
    /// written. A browser tab may close without that pass.
    pub fn losing(&self, workspace: &Workspace, browser: &Browser) -> bool {
        let unheld = self.unheld(workspace).next().is_some();
        self.writing() || unheld || self.unkept_view(workspace) || self.held(browser)
    }

    /// Say that an unsaved edit could not be kept in this install's own state, or let go
    /// of, unless it was said since a keep last succeeded.
    fn say_unkept(&mut self, why: &str, log: &mut Log) {
        if std::mem::replace(&mut self.told_unkept, true) {
            return;
        }
        log.error(format!("keeping an unsaved edit: {why}"));
        log.trouble(format!(
            "An unsaved edit could not be kept for later: {why}."
        ));
    }

    /// Whether a slot view's unsaved edit is not kept as it stands, by a keep answered.
    fn unkept_view(&self, workspace: &Workspace) -> bool {
        let views = workspace.entities().iter().filter(|entity| !entity.kept);
        let mut unsaved = views.filter(|entity| entity.is_unsaved());
        unsaved.any(|entity| {
            let view = self.views.get(&entity.id);
            view.is_none_or(|view| view.kept != Some((entity.stamp, view.base)))
        })
    }

    /// Whether a change is held in memory only, where nothing may be written: a deletion,
    /// a save, a rename or move, or a folder change.
    fn held(&self, browser: &Browser) -> bool {
        let read_only = matches!(self.phase, Phase::ReadOnly(_) | Phase::Unfinished(_));
        read_only && (browser.folders.changed() || !self.asks.is_empty())
    }

    /// The names of the assets that letting this library go would lose: where nothing
    /// may be written here, each unsaved edit and each asset never written to a file, and
    /// otherwise each unsaved edit of a file drawbar has not read, which no draft can be
    /// made over. A library open for writing writes the rest, and keeps their edits as
    /// drafts, at its last pass.
    pub fn unkept(&self, workspace: &Workspace) -> Vec<String> {
        let open = self.open();
        let unheld = self.unheld(workspace);
        let lost = unheld.filter(|entity| {
            let undrafted = entity.is_unsaved() && self.base(entity.id).is_none();
            !open || undrafted
        });
        lost.map(|entity| entity.name.clone()).collect()
    }

    /// What a draft of an asset's unsaved edit is made over: the identity of its file, as
    /// drawbar last read or wrote it, with no write over it in flight.
    fn base(&self, id: u64) -> Option<toshokan::Identity> {
        let record = self.records.get(&id).filter(|record| !record.saving)?;
        record.fingerprint?.identity
    }

    /// Keep each unsaved edit as a draft where it changed since it was last kept, and let
    /// go of the drafts of assets saved, gone or known by another key since.
    fn keep_drafts(&mut self, workspace: &Workspace) {
        if !matches!(self.phase, Phase::Open | Phase::Unfinished(_)) {
            return;
        }
        for (id, kept) in std::mem::take(&mut self.drafted) {
            let unsaved = workspace.get(id).is_some_and(LocalEntity::is_unsaved);
            let saving = self.records.get(&id).is_some_and(|record| record.saving);
            match (unsaved || saving, self.ids.key(id) == Some(&kept.0)) {
                (true, true) => _ = self.drafted.insert(id, kept),
                // The draft under the key it went by goes once the one under its new
                // key is kept.
                (true, false) => self.superseded.entry(id).or_default().push(kept.0),
                (false, _) => self.send(Cmd::Discard(kept.0)),
            }
        }
        let held = |id: &u64| workspace.get(*id).is_some();
        let unsaved = |id: &u64| workspace.get(*id).is_some_and(LocalEntity::is_unsaved);
        let gone: Vec<u64> = self
            .restoring
            .keys()
            .copied()
            .filter(|id| !held(id))
            .collect();
        for id in gone {
            if let Some((kept, ..)) = self.restoring.remove(&id) {
                self.send(Cmd::Discard(kept));
            }
        }
        let saving = |id: &u64| self.records.get(id).is_some_and(|record| record.saving);
        let settled: Vec<u64> = self
            .superseded
            .keys()
            .copied()
            .filter(|id| !unsaved(id) && !saving(id))
            .collect();
        for id in settled {
            for kept in self.superseded.remove(&id).unwrap_or_default() {
                self.send(Cmd::Discard(kept));
            }
        }
        for entity in workspace.listed().filter(|entity| entity.is_unsaved()) {
            let (Some(key), Some(base)) = (self.ids.key(entity.id).cloned(), self.base(entity.id))
            else {
                continue;
            };
            let (draft, stamp) = match workspace.kept_edit(entity.id) {
                Some((edit, stamp)) => (Draft::Edit(edit.clone()), stamp),
                None => (Draft::Bytes(entity.bytes.to_vec()), entity.stamp),
            };
            let sent = self.drafting.iter().rev().find(|sent| sent.0 == entity.id);
            let latest = sent.map(|(_, key, stamp, base)| (key, *stamp, *base));
            let latest = latest.or_else(|| {
                let kept = self.drafted.get(&entity.id)?;
                Some((&kept.0, kept.1, kept.2))
            });
            if latest == Some((&key, stamp, base)) {
                continue;
            }
            let Ok(bytes) = draft.encode() else {
                continue;
            };
            let mut replaces = self.superseded.remove(&entity.id).unwrap_or_default();
            // A keep under the key lets go of what was kept under it before.
            replaces.retain(|kept| *kept != key);
            self.send(Cmd::Draft {
                of: key.clone(),
                base,
                bytes,
                replaces,
            });
            self.drafting.push_back((entity.id, key, stamp, base));
        }
    }

    /// What the library shows, as the answers so far changed it.
    #[cfg(test)]
    pub(crate) fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    /// Say which instrument is attached, which the views of its slots opened from now
    /// keep their unsaved edits by.
    pub fn attached(&mut self, instrument: Option<String>) {
        self.instrument = instrument;
    }

    /// Ask, for each view of a slot seen for the first time, for the edit an earlier
    /// session kept of its slot, and where `keep` says, keep each slot view's unsaved
    /// edit where it changed since it was last kept, and let go of the edits of views
    /// sent, reverted, closed or kept on this computer. A view keeps no edit of its
    /// own before the answer.
    fn keep_slot_edits(&mut self, workspace: &Workspace, keep: bool) {
        if !self.opened() {
            return;
        }
        let ended: Vec<u64> = self
            .views
            .keys()
            .copied()
            .filter(|id| !workspace.is_view(*id))
            .collect();
        for id in ended {
            let keeping = let_go_of_keeps(&mut self.keeping, id);
            if let Some(view) = self.views.remove(&id) {
                if view.kept.is_some() || keeping {
                    self.send(Cmd::DiscardSlot(view.slot));
                }
            }
        }
        let views = workspace.entities().iter().filter(|entity| !entity.kept);
        for entity in views {
            let Some((class, at)) = entity.origin.slot() else {
                continue;
            };
            let Some(class) = super::keys::class_code(class) else {
                continue;
            };
            let saved = entity.saved.stamp();
            let Some(view) = self.views.get_mut(&entity.id) else {
                let Some(instrument) = self.instrument.clone() else {
                    continue;
                };
                let slot = super::Slot {
                    instrument,
                    class,
                    bank: at.bank,
                    slot: at.slot,
                };
                let base = blake3::hash(entity.saved.bytes());
                let view = SlotView {
                    slot: slot.clone(),
                    saved,
                    base,
                    read: false,
                    kept: None,
                };
                self.views.insert(entity.id, view);
                self.send(Cmd::ReadSlot(slot));
                continue;
            };
            if !keep {
                continue;
            }
            if view.saved != saved {
                view.saved = saved;
                view.base = blake3::hash(entity.saved.bytes());
            }
            let held = (entity.stamp, view.base);
            let sent = self.keeping.iter().rev().find(|sent| sent.0 == entity.id);
            let latest = sent.map_or(view.kept, |sent| sent.1);
            let cmd = match (view.read, entity.is_unsaved()) {
                (false, _) => None,
                (true, true) if latest != Some(held) => {
                    self.keeping.push_back((entity.id, Some(held)));
                    Some(Cmd::KeepSlot {
                        slot: view.slot.clone(),
                        base: view.base,
                        bytes: entity.bytes.to_vec(),
                    })
                }
                (true, false) if latest.is_some() => {
                    view.kept = None;
                    let_go_of_keeps(&mut self.keeping, entity.id);
                    Some(Cmd::DiscardSlot(view.slot.clone()))
                }
                (true, _) => None,
            };
            if let Some(cmd) = cmd {
                self.send(cmd);
            }
        }
    }

    /// Put the edit an earlier session kept of a slot back over its view, unless the
    /// view was edited since it opened, and ask what to do where the slot changed since
    /// the edit was made.
    fn restore_slot_edit(
        &mut self,
        slot: &super::Slot,
        kept: Result<Option<(blake3::Hash, Vec<u8>)>, String>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let Some((&id, view)) = self
            .views
            .iter_mut()
            .find(|(_, view)| !view.read && view.slot == *slot)
        else {
            return;
        };
        view.read = true;
        let (base, bytes) = match kept {
            Ok(Some(kept)) => kept,
            Ok(None) => return,
            Err(why) => {
                log.trouble(format!(
                    "The unsaved edit drawbar kept of this slot could not be read: {why}."
                ));
                return;
            }
        };
        let Some(entity) = workspace.get(id) else {
            return;
        };
        if entity.is_unsaved() {
            return browser.ask_kept_slot_edit(id, &entity.name, bytes);
        }
        let changed = base != view.base;
        workspace.replace_bytes(id, bytes, log);
        let Some(entity) = workspace.get(id) else {
            return;
        };
        view.kept = Some((entity.stamp, base));
        match changed {
            true => slot_conflict(id, workspace, browser, log),
            false => log.say(format!(
                "“{}” has the unsaved edit it had when drawbar last let it go.",
                entity.name
            )),
        }
    }

    /// Let go of the drafts of an asset whose save landed, unless an edit was made
    /// since.
    fn saved_draft(&mut self, id: u64, workspace: &Workspace) {
        if workspace.get(id).is_some_and(LocalEntity::is_unsaved) {
            return;
        }
        let own = self.drafted.remove(&id).map(|(key, _, _)| key);
        let superseded = self.superseded.remove(&id).unwrap_or_default();
        for kept in own.into_iter().chain(superseded) {
            self.send(Cmd::Discard(kept));
        }
    }

    /// Take in the drafts an earlier session kept: each is put back over its asset once
    /// the asset's file is read, which is asked for now.
    fn take_drafts(
        &mut self,
        drafts: Vec<Result<super::Kept, String>>,
        workspace: &mut Workspace,
        log: &mut Log,
    ) {
        for draft in drafts {
            let Ok(super::Kept {
                of: key,
                at: kept_at,
                holds,
                bytes,
            }) = draft
            else {
                log.trouble("An unsaved edit drawbar kept could not be read.");
                continue;
            };
            let changed = match holds {
                true => Since::Same,
                false => Since::Changed,
            };
            // A file whose draft was kept by its path may have become an entity since.
            let at = |path: &LibPath| {
                let mut listed = workspace.listed();
                listed
                    .find(|entity| entity.path.as_ref() == Some(path))
                    .map(|entity| entity.id)
            };
            let id = match &key {
                Key::File(path) => self.ids.id(&key).or_else(|| at(path)),
                Key::Entity(_) => self.ids.id(&key),
            };
            let Some(id) = id.filter(|id| workspace.get(*id).is_some()) else {
                log.trouble(
                    "An unsaved edit drawbar kept is of a file no longer in the library \
                     folder. It is kept for when the file is back.",
                );
                continue;
            };
            match Draft::decode(&bytes) {
                Ok(draft) => {
                    let restoring = (key.clone(), draft, changed, kept_at);
                    let older = match self.restoring.remove(&id) {
                        Some(earlier) if earlier.3 > restoring.3 => {
                            self.restoring.insert(id, earlier);
                            Some(restoring.0)
                        }
                        earlier => {
                            self.restoring.insert(id, restoring);
                            earlier.map(|earlier| earlier.0)
                        }
                    };
                    if let Some(older) = older {
                        // ⚠️ Let go only once this instance keeps the newer: until
                        // then the older may be the only copy left.
                        self.superseded.entry(id).or_default().push(older);
                        let name = workspace.get(id).map(|entity| entity.name.clone());
                        log.say(format!(
                            "Two drawbar windows kept unsaved edits of “{}”: the newer is \
                             shown, and the older was let go.",
                            name.unwrap_or_default()
                        ));
                    }
                    workspace.hurry(id);
                }
                Err(why) => {
                    let name = workspace.get(id).map(|entity| entity.name.clone());
                    let name = name.unwrap_or_default();
                    log.trouble(format!(
                        "The unsaved edit of “{name}” could not be read: {why}."
                    ));
                }
            }
        }
    }

    /// Put back the draft kept of an asset whose file is read now, and ask what to do
    /// where the file changed since the draft was made.
    fn restore_draft(
        &mut self,
        id: u64,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        if workspace.get(id).is_none_or(LocalEntity::unread) {
            return;
        }
        let Some((key, draft, since, _)) = self.restoring.remove(&id) else {
            return;
        };
        let changed = match since {
            Since::Same => false,
            Since::Changed => true,
            Since::Unless(held) => workspace
                .get(id)
                .is_none_or(|entity| blake3::hash(entity.saved.bytes()) != held),
        };
        match draft {
            Draft::Bytes(bytes) => workspace.replace_bytes(id, bytes, log),
            Draft::Edit(edit) => workspace.hold_edit(id, Some(edit)),
        }
        let Some(entity) = workspace.get(id) else {
            return;
        };
        if !entity.is_unsaved() {
            return self.send(Cmd::Discard(key));
        }
        // This instance keeps the edit by the key the asset goes by now, and then lets
        // go of the draft it came from, where that is another.
        if self.ids.key(id) != Some(&key) {
            self.superseded.entry(id).or_default().push(key);
        }
        match changed {
            true => conflict(id, workspace, browser, log),
            false => log.say(format!(
                "“{}” has the unsaved edit it had when drawbar last let it go.",
                entity.name
            )),
        }
    }

    /// Each unsaved edit, and each asset never written to a file.
    fn unheld<'a>(&'a self, workspace: &'a Workspace) -> impl Iterator<Item = &'a LocalEntity> {
        workspace.listed().filter(|entity| {
            let record = self.records.get(&entity.id);
            let written = record.is_some_and(|record| record.fingerprint.is_some());
            !written || entity.is_unsaved()
        })
    }

    /// Where the library is, as the user would look for it.
    pub fn label(&self) -> String {
        self.backend.label()
    }

    /// What the browser says about keeping the library: its share of the browser's
    /// storage, and whether the browser may clear it. Empty on the desktop.
    #[cfg(target_arch = "wasm32")]
    pub fn kept(&self) -> String {
        match self.phase {
            Phase::Failed(_) => "kept only until this tab closes".to_string(),
            _ => self.backend.room(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn kept(&self) -> String {
        String::new()
    }

    /// What may delete the library, where it is the browser's own and something may.
    #[cfg(target_arch = "wasm32")]
    pub fn risk(&self) -> Option<super::Risk> {
        self.backend.risk()
    }

    /// Save a copy of the library as a ZIP the browser downloads, once what was sent
    /// before has run.
    #[cfg(target_arch = "wasm32")]
    pub fn save_copy(&mut self) {
        self.backend.save_copy();
    }

    fn open(&self) -> bool {
        matches!(self.phase, Phase::Open)
    }

    /// Whether a file from outside can be copied into the library: it is open, and
    /// nothing has said it may not be written.
    pub fn takes_files(&self) -> bool {
        self.open()
    }

    fn opened(&self) -> bool {
        matches!(
            self.phase,
            Phase::Open | Phase::ReadOnly(_) | Phase::Unfinished(_)
        )
    }

    /// Where the library is, and what state it is in, for the browser's header.
    fn place(&self) -> Where {
        Where {
            name: self.name.clone(),
            label: self.backend.label(),
            kept: self.kept(),
            reveal: self.backend.reveal(),
            note: match &self.phase {
                _ if self.listing() => Some(match self.backend.listed() {
                    0 => "opening…".to_string(),
                    1 => "opening… 1 file found".to_string(),
                    n => format!("opening… {n} files found"),
                }),
                Phase::Opening | Phase::Open => None,
                Phase::ReadOnly(why) | Phase::Unfinished(why) => Some(format!("read-only: {why}")),
                Phase::Failed(why) => Some(format!("not opened: {why}")),
            },
            read_only: matches!(self.phase, Phase::ReadOnly(_) | Phase::Unfinished(_)),
            opening: self.listing(),
        }
    }

    /// Fold in what the backend has answered. Returns whether a send held by
    /// [`Store::hold_send`] may now go ahead.
    pub fn poll(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        self.ask(workspace, browser, queue, log);
        self.ask_tags(workspace, browser, log);
        let mut released = false;
        while let Some(event) = self.backend.try_recv() {
            released |= self.react(event, workspace, browser, queue, log);
        }
        self.tell_unfinished(workspace.ctx(), browser, log);
        self.watch(browser);
        if self.backend.waiting().is_some() {
            workspace
                .ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
        #[cfg(target_arch = "wasm32")]
        self.hinted();
        #[cfg(target_arch = "wasm32")]
        {
            let found = self.backend.rescued();
            self.take_in_rescues(found, workspace, browser, log);
        }
        browser.folders.place = Some(self.place());
        // The count of files found moves with no answer to wake the window.
        if self.listing() {
            let soon = std::time::Duration::from_millis(250);
            workspace.ctx().request_repaint_after(soon);
        }
        released
    }

    /// Why the library's thread or worker stopped answering, once it has. The store then
    /// hears nothing more.
    pub fn stopped(&self) -> Option<String> {
        self.backend.stopped()
    }

    /// Whether the command the library runs now has gone [`Store::patience`] without a
    /// sign of life, as one that never settles does.
    fn stalled(&self) -> bool {
        self.backend
            .waiting()
            .is_some_and(|waited| waited >= self.patience)
    }

    /// Wait `patience` for a command to show life before asking whether to reopen.
    #[cfg(test)]
    pub fn patience(&mut self, patience: std::time::Duration) {
        self.patience = patience;
    }

    /// Make the library's thread fault, or never finish, with the next command it takes.
    #[cfg(test)]
    pub fn fault(&self, fault: super::Fault) {
        self.backend.fault(fault);
    }

    /// Ask, once, whether to close and reopen a library that has stopped answering.
    fn watch(&mut self, browser: &mut Browser) {
        match self.stalled() {
            true if !self.asked_stalled => {
                self.asked_stalled = true;
                browser.ask_stalled();
            }
            true => {}
            false => self.asked_stalled = false,
        }
    }

    /// The unsaved edits not kept as drafts since they were last made, by the names of
    /// their assets, which a library that stopped answering cannot keep.
    fn undrafted(&self, workspace: &Workspace) -> Vec<String> {
        let unsaved = workspace.listed().filter(|entity| entity.is_unsaved());
        let undrafted = unsaved.filter(|entity| {
            let stamp = match workspace.kept_edit(entity.id) {
                Some((_, stamp)) => stamp,
                None => entity.stamp,
            };
            self.drafted.get(&entity.id).map(|kept| kept.1) != Some(stamp)
        });
        undrafted.map(|entity| entity.name.clone()).collect()
    }

    /// Whether the browser no longer lets the page into the library's picked folder.
    #[cfg(target_arch = "wasm32")]
    pub fn denied(&self) -> bool {
        self.backend.denied()
    }

    /// Read again what another tab or the folder says changed: another tab's commit by a
    /// refresh, and anything else by a rescan, once no scan is in flight.
    #[cfg(target_arch = "wasm32")]
    fn hinted(&mut self) {
        use toshokan::web::Hint;

        while let Some(hint) = self.backend.hinted() {
            let rescan = !matches!(hint, Hint::Committed(_));
            self.behind = Some(self.behind.unwrap_or(false) || rescan);
        }
        let Some(rescan) = self.behind.filter(|_| self.opened() && !self.scanning) else {
            return;
        };
        self.behind = None;
        match rescan {
            true => self.rescan(),
            false => self.refresh(),
        }
    }

    /// Wait for the backend's next answer and fold it in, as [`Store::poll`] would.
    /// Returns `false` when none came.
    #[cfg(test)]
    pub fn next(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        self.ask_tags(workspace, browser, log);
        let Some(event) = self.backend.recv() else {
            return false;
        };
        self.react(event, workspace, browser, queue, log);
        browser.folders.place = Some(self.place());
        true
    }

    /// Read at most this much of the library's files whole, in place of [`MOST_BYTES`].
    #[cfg(test)]
    pub fn budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    /// Ask the backend for the files of the unread assets something needs, reading at
    /// most what keeps the assets' whole bytes within [`MOST_BYTES`]. An asset refused
    /// for want of room is asked for again once room can be made for it.
    pub(crate) fn ask(
        &mut self,
        workspace: &mut Workspace,
        browser: &Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        let waits = !workspace.wants() && self.roomless.is_empty();
        if !self.opened() || waits {
            return;
        }
        // Smallest first: where room cannot be made for one, it cannot for any larger,
        // so the rest wait without another pass over every asset.
        let mut waiting: Vec<(u64, u64)> = std::mem::take(&mut self.roomless)
            .into_iter()
            .map(|(id, len)| (len, id))
            .collect();
        waiting.sort_unstable();
        let mut rest = waiting.into_iter();
        for (len, id) in rest.by_ref() {
            if workspace.get(id).is_none() {
                continue;
            }
            if self.make_room(len, Evict::Any, workspace, browser, queue) {
                workspace.retry(id);
            } else {
                self.roomless.insert(id, len);
                break;
            }
        }
        self.roomless.extend(rest.map(|(len, id)| (id, len)));
        if self.reading {
            return;
        }
        let room = self.room(workspace);
        let mut files = Vec::new();
        for id in workspace.take_wanted() {
            let record = self.records.get(&id);
            let unread = record.filter(|record| record.holds == Holds::Unread);
            match unread.and_then(|record| Some((record.path.clone()?, record.fingerprint))) {
                Some((path, print)) => files.push((id, path, print)),
                None => {
                    let why = "drawbar knows no file that holds it".to_string();
                    workspace.unreadable(id, why, log);
                }
            }
        }
        if files.is_empty() {
            return;
        }
        self.reading = true;
        self.send(Cmd::Read { files, room });
    }

    /// How many more bytes the assets may hold whole: the budget, less what they hold and
    /// each read in flight at its file's listed length.
    fn room(&self, workspace: &Workspace) -> u64 {
        self.budget.saturating_sub(held(workspace))
    }

    /// Make room for `need` more bytes held whole by letting go of the clean assets
    /// needed least recently, each unread again until something needs it. Those the
    /// index tracks go after the rest, and only where `evict` allows. Lets go of nothing
    /// where that would not make the room. Returns whether `need` fits.
    ///
    /// ⚠️ Only an asset its file can give back goes: one whose record says drawbar holds
    /// the file as last read or written, with nothing in flight over it, no working copy
    /// and no send waiting. See [`Workspace::evictable`] for what the workspace keeps.
    fn make_room(
        &mut self,
        need: u64,
        evict: Evict,
        workspace: &mut Workspace,
        browser: &Browser,
        queue: &Queue,
    ) -> bool {
        let wanted = held(workspace).saturating_add(need);
        if wanted <= self.budget {
            return true;
        }
        if need > self.budget {
            return false;
        }
        #[cfg(test)]
        {
            self.looked_for_room += 1;
        }
        let mut clean: Vec<(bool, u64, u64, u64)> = self
            .records
            .iter()
            .filter(|(id, record)| {
                let saved = workspace.get(**id).map(|entity| entity.saved.stamp());
                saved.is_some_and(|saved| record.rereadable(saved, &self.moving))
                    && !queue.holds(**id)
            })
            .filter_map(|(id, _)| {
                let (seen, freed) = workspace.evictable(*id)?;
                let tracked = tracked(*id, workspace, browser);
                (evict == Evict::Any || !tracked).then_some((tracked, seen, *id, freed))
            })
            .collect();
        clean.sort_unstable();
        let mut short = wanted - self.budget;
        let mut going = Vec::new();
        for (_, _, id, freed) in clean {
            if short == 0 {
                break;
            }
            short = short.saturating_sub(freed);
            going.push(id);
        }
        if short > 0 {
            return false;
        }
        for id in going {
            let Some(stamp) = workspace.evict(id) else {
                continue;
            };
            if let Some(record) = self.records.get_mut(&id) {
                record.saved = stamp;
                record.holds = Holds::Unread;
            }
        }
        true
    }

    fn send(&mut self, cmd: Cmd) {
        #[cfg(test)]
        if let Cmd::Read { files, .. } = &cmd {
            self.asked_files += files.len();
        }
        self.backend.send(cmd);
    }

    /// Rename a folder. A file the library knows by its path is known by the new one from
    /// now on.
    fn rename_folder(&mut self, from: LibPath, to: LibPath) {
        self.ids.moved(&from, &to);
        self.moving.push((from.clone(), to.clone()));
        self.part(Intent::MoveFolder { from, to });
    }

    /// Put back where they were the paths a rename that failed had moved.
    fn unmoved(
        &mut self,
        from: &LibPath,
        to: &LibPath,
        workspace: &mut Workspace,
        browser: &mut Browser,
    ) {
        // A file's record went to `to` as its rename was sent; a folder holds no record.
        let file = self
            .records
            .values()
            .any(|record| record.path.as_ref() == Some(to));
        for record in self.records.values_mut() {
            if let Some(back) = record.path.as_ref().and_then(|at| at.moved(to, from)) {
                record.path = Some(back);
            }
        }
        self.ids.moved(to, from);
        // ⚠️ The folder changes not yet sent have moved the workspace on already. Those
        // after a folder's rename name the folder by its new name, and are moved back
        // with it; a file's rename is undone where those changes have put the file.
        match file {
            true => {
                let folders = &browser.folders;
                workspace.relocate(&folders.ahead(to), &folders.ahead(from));
            }
            false => {
                workspace.relocate(to, from);
                browser.folders.follow_back(to, from);
            }
        }
    }

    /// Forget a rename now answered. `false` for one this store did not send.
    fn answered_move(&mut self, from: &LibPath, to: &LibPath) -> bool {
        let sent = self.moving.iter().position(|(a, b)| a == from && b == to);
        sent.map(|at| self.moving.remove(at)).is_some()
    }

    /// Take in the files read for the assets that asked. Each comes back as it is now: a
    /// file whose length or time moved is held as what it holds, and its fingerprint
    /// keeps the CRC known before only where the contents are the same.
    fn took(
        &mut self,
        answers: Vec<(u64, Result<Found, Failure>)>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        let rechecked = answers.iter().all(|(id, _)| self.rechecking.contains(id));
        let background = answers.iter().any(|(id, _)| self.fetching.contains(id));
        for (id, answer) in answers {
            if self.rechecking.remove(&id) {
                match answer {
                    Ok(found) => self.changed(id, found, workspace, browser, log),
                    Err(_) => self.rescan(),
                }
                continue;
            }
            let behind = self.fetching.remove(&id);
            let unread = self.records.get(&id).map(|record| record.holds) == Some(Holds::Unread);
            let mut found = match answer {
                Ok(found) if unread => found,
                Ok(_) => {
                    workspace.took(id, None, None);
                    continue;
                }
                Err(Failure::Moved) => {
                    self.rescan();
                    let why = "it is gone from the library folder".to_string();
                    workspace.unreadable(id, why, log);
                    continue;
                }
                Err(Failure::Room(len)) => {
                    workspace.unasked(id);
                    let evict = match behind {
                        true => Evict::Untracked,
                        false => Evict::Any,
                    };
                    let made = self.make_room(len, evict, workspace, browser, queue);
                    match (made, behind) {
                        (true, true) => self.unfetched.push_front(id),
                        (true, false) => workspace.again(id),
                        (false, true) => _ = self.fetched.remove(&id),
                        (false, false) => {
                            if len <= self.budget {
                                self.roomless.insert(id, len);
                            }
                            workspace.unreadable(id, too_much(), log);
                        }
                    }
                    continue;
                }
                Err(Failure::Io(why)) => {
                    workspace.unreadable(id, why, log);
                    continue;
                }
                Err(Failure::Unheard) => {
                    workspace.unreadable(id, "its answer was lost".to_string(), log);
                    continue;
                }
            };
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            // ⚠️ Taken whether or not the stat moved: a file rewritten under the same
            // length and time holds what it holds now, not what its CRC said before.
            if let Some(bytes) = &found.bytes {
                found.crc = Some(nord_format::crc::crc32(bytes));
            }
            record.fingerprint = Some(diff::kept(record.fingerprint, &found));
            record.holds = found.holds();
            workspace.took(id, found.bytes, found.file);
        }
        for id in self.restoring.keys().copied().collect::<Vec<_>>() {
            self.restore_draft(id, workspace, browser, log);
        }
        if rechecked {
            return;
        }
        match background {
            true => self.next_fetches(workspace),
            false => {
                self.reading = false;
                self.ask(workspace, browser, queue, log);
            }
        }
    }

    /// Whether a rescan is waiting for its answer.
    #[cfg(test)]
    pub fn scanning(&self) -> bool {
        self.scanning
    }

    /// Whether the library is still opening, or listing its files after.
    pub fn listing(&self) -> bool {
        matches!(self.phase, Phase::Opening) || self.first_scan
    }

    /// The key the library knows an asset's file by.
    #[cfg(test)]
    pub fn key(&self, id: u64) -> Option<Key> {
        self.ids.key(id).cloned()
    }

    /// Whether a change to the facts waits to be sent or answered.
    #[cfg(test)]
    pub fn deciding(&self) -> bool {
        !self.asked.is_empty() || self.committing > 0
    }

    /// Whether a send is held for a rescan or check still in flight.
    #[cfg(test)]
    pub fn holds_send(&self) -> bool {
        self.send_waits
    }

    /// Whether nothing may be written, and why.
    #[cfg(test)]
    pub fn read_only(&self) -> Option<&str> {
        match &self.phase {
            Phase::ReadOnly(why) | Phase::Unfinished(why) => Some(why),
            _ => None,
        }
    }

    /// Fold in one answer, in its turn, or hold it for the take-in in flight. Returns
    /// whether a send held by [`Store::hold_send`] may now go ahead.
    fn react(
        &mut self,
        event: Event,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        for kind in event.faults() {
            crate::telemetry::fault("library", kind);
        }
        match self.hold(event) {
            Some(event) => self.fold(event, workspace, browser, queue, log),
            None => false,
        }
    }

    /// Hold `event` where it says a part of the take-in in flight did not land, as only
    /// the take-in's own answer says what that means.
    fn hold(&mut self, event: Event) -> Option<Event> {
        let Some(taking) = self.taking_in.as_mut() else {
            return Some(event);
        };
        let why = match &event {
            Event::Saved {
                id,
                result: Err(failure),
                ..
            } if taking.ids.contains(id) => failure.to_string(),
            Event::Failed(why) => why.clone(),
            _ => return Some(event),
        };
        taking.why.get_or_insert(why);
        taking.held.push(event);
        None
    }

    /// Fold in one answer.
    fn fold(
        &mut self,
        event: Event,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        match event {
            Event::Opened(Ok(opened)) => self.begin(opened, workspace, browser, queue, log),
            Event::Opened(Err(why)) => {
                log.error(format!("opening the library: {why}"));
                log.trouble(format!(
                    "This computer's library could not be opened ({why}). Nothing you \
                     make here is kept."
                ));
                self.phase = Phase::Failed(why);
            }
            Event::Scanned(Ok(viewed)) => {
                let whole = viewed.dirs.is_some();
                let moved = self.viewed(viewed, workspace, browser, queue, log);
                self.moved.extend(moved);
                self.first_scan = false;
                if whole && !self.listed {
                    self.listed_whole(workspace, browser, log);
                }
                return self.scanned(queue, workspace, log);
            }
            Event::Scanned(Err(why)) => {
                self.first_scan = false;
                self.scanning = false;
                self.send_waits = false;
                self.moved.clear();
                log.error(format!("reading the library again: {why}"));
                log.trouble("The library folder could not be read, so nothing was sent.");
            }
            Event::Committed {
                label,
                made,
                undoable,
                forgot,
                finished,
                waiting,
                result,
            } => {
                self.committing = self.committing.saturating_sub(1);
                let landed = made.is_some();
                let step = self.answering.pop_front();
                self.history.waited(finished, waiting, step.as_ref());
                let forgotten = match (&step, forgot) {
                    (Some(step), true) => self.history.forget(step),
                    _ => None,
                };
                if let (Some(entry), Some(step), true) = (made, step, undoable) {
                    self.history.took(step, entry);
                }
                let told = std::mem::take(&mut self.turned_read_only);
                // A take-in that failed partway lands once its change is finished.
                let refused = match self.taking_in.take_if(|_| label == TAKE_IN) {
                    Some(taking) if landed || waiting == Waiting::This => {
                        self.follow_take_in(taking, landed, workspace, browser, queue, log);
                        false
                    }
                    Some(taking) => {
                        let why = result.as_ref().err().cloned();
                        self.not_taken_in(taking, why, workspace, log);
                        true
                    }
                    None => false,
                };
                match result {
                    Ok(viewed) => {
                        self.viewed(viewed, workspace, browser, queue, log);
                    }
                    Err(why) => {
                        match forgotten {
                            Some(forgotten) => log.trouble(format!("{forgotten}: {why}.")),
                            None if refused => {}
                            None if told => log.error(format!("{label}: {why}")),
                            None => {
                                log.error(format!("{label}: {why}"));
                                log.trouble(format!("The library did not change as asked: {why}."));
                            }
                        }
                        if let Some(shown) = self.shown.take() {
                            self.project_tags(&shown, browser);
                            browser.forget_gone(workspace);
                            self.shown = Some(shown);
                        }
                    }
                }
            }
            Event::Unheard { label } => {
                self.committing = self.committing.saturating_sub(1);
                self.answering.pop_front();
                // The look below finds whether it landed, as it finds each of its files.
                if let Some(taking) = self.taking_in.take_if(|_| label == TAKE_IN) {
                    self.follow_take_in(taking, false, workspace, browser, queue, log);
                }
                // Whether the library took a step is not known, so the Edit menu offers
                // none it holds until the next one it hears of.
                self.history = History::default();
                log.say(format!(
                    "drawbar lost the library's answer to “{label}”, so it looks at the \
                     folder to see what changed."
                ));
                self.rescan();
            }
            Event::Read(answers) => {
                let rechecked = answers.iter().all(|(id, _)| self.rechecking.contains(id));
                self.took(answers, workspace, browser, queue, log);
                if rechecked && self.scanning {
                    return self.scanned(queue, workspace, log);
                }
            }
            Event::Moved { from, to, result } => match result {
                _ if !self.answered_move(&from, &to) => {}
                Ok(()) => {}
                // The look the change's answer asks for finds where the files are.
                Err(Failure::Unheard) => {}
                Err(why) => {
                    let why = format!("moving {from} to {to}: {why}");
                    log.error(why.clone());
                    log.trouble(format!(
                        "The library folder did not change as asked: {why}."
                    ));
                    self.unmoved(&from, &to, workspace, browser);
                    self.rescan();
                }
            },
            Event::Saved { id, path, result } => {
                self.saved(id, path, result, workspace, browser, log)
            }
            Event::Imported { id, path, result } => {
                self.imported(id, path, result, workspace, browser, log)
            }
            Event::Rewritten { id, path, result } => {
                self.rewritten(id, path, result, workspace, browser, log)
            }
            Event::Trimmed(entries) => {
                if let Some(change) = self.history.trim(&entries) {
                    log.say(format!(
                        "Undo no longer reaches “{change}” or what came before it: \
                         drawbar keeps at most 1 GiB of what your changes replaced."
                    ));
                }
            }
            Event::SlotKept { slot, kept } => {
                self.restore_slot_edit(&slot, kept, workspace, browser, log);
            }
            Event::Drafted { of, kept } => {
                let sent = self.drafting.pop_front();
                match (sent, kept) {
                    (Some((id, key, stamp, base)), Ok(())) if key == of => {
                        self.told_unkept = false;
                        self.drafted.insert(id, (key, stamp, base));
                    }
                    (_, Ok(())) => self.told_unkept = false,
                    (_, Err(why)) => self.say_unkept(&why, log),
                }
            }
            Event::SlotDrafted { slot, kept } => {
                let sent = self.keeping.pop_front();
                match (sent, kept) {
                    (Some((id, Some(held))), Ok(())) => {
                        self.told_unkept = false;
                        let view = self.views.get_mut(&id).filter(|view| view.slot == slot);
                        if let Some(view) = view {
                            view.kept = Some(held);
                        }
                    }
                    (_, Ok(())) => self.told_unkept = false,
                    (_, Err(why)) => self.say_unkept(&why, log),
                }
            }
            Event::Unkept(why) => self.say_unkept(&why, log),
            Event::Restarted => {
                self.history = History::default();
                // What waited to be finished never will be, but as the user settles it.
                self.awaiting.clear();
                log.say(
                    "Undo no longer reaches the changes made before: drawbar could not go on \
                     with its record of this library, so it began a new one.",
                );
                // What the record it gave up left unfinished is the user's to settle.
                self.refresh();
            }
            Event::Restored(Ok(files)) => log.say(match files {
                1 => "drawbar opened the copy of the library, which holds 1 file.".to_string(),
                n => format!("drawbar opened the copy of the library, which holds {n} files."),
            }),
            Event::Restored(Err(why)) => {
                log.error(format!("opening a copy of the library: {why}"));
                log.trouble(format!("The copy was not opened: {why}."));
            }
            Event::Copied(Ok((name, files))) => log.say(format!(
                "The browser is saving a copy of the library as {name}, with {}. Change \
                 nothing in the library until its download finishes, or the copy may not \
                 be whole.",
                match files {
                    1 => "1 file".to_string(),
                    n => format!("{n} files"),
                }
            )),
            Event::Copied(Err(why)) => {
                log.error(format!("saving a copy of the library: {why}"));
                log.trouble(format!("No copy of the library was saved: {why}."));
            }
            Event::Unemptied(why) => {
                log.error(format!("emptying the trash: {why}"));
                log.trouble(format!("drawbar's trash could not be emptied: {why}."));
            }
            Event::ReadOnly(why) if self.open() => {
                log.trouble(format!(
                    "This computer's library is read-only now: {why}. Unsaved edits are \
                     still kept."
                ));
                self.phase = Phase::Unfinished(why);
                self.turned_read_only = true;
            }
            Event::ReadOnly(_) => {}
            Event::Failed(why) => {
                log.error(why.as_str());
                log.trouble(format!(
                    "The library folder did not change as asked: {why}."
                ));
                self.rescan();
            }
        }
        false
    }

    /// Send a change to the files.
    fn write(&mut self, intent: Intent) {
        self.sent += 1;
        self.commit(intent);
    }

    /// Send a change to the facts.
    fn commit(&mut self, intent: Intent) {
        self.committing += 1;
        self.answering.push_back(Step::Change(intent.describe()));
        self.send(Cmd::Intent(intent));
    }

    /// Undo this session's last change not undone.
    pub fn undo(&mut self) {
        if self.opened() {
            self.committing += 1;
            self.answering.push_back(Step::Undo);
            self.send(Cmd::Undo);
        }
    }

    /// Redo the change undone last.
    pub fn redo(&mut self) {
        if self.opened() {
            self.committing += 1;
            self.answering.push_back(Step::Redo);
            self.send(Cmd::Redo);
        }
    }

    /// What Undo takes back now, as the Edit menu names it.
    pub fn undo_label(&self) -> Option<String> {
        let (last, _) = self.history.done.last()?;
        Some(format!("Undo {last}"))
    }

    /// What Redo makes again now, as the Edit menu names it.
    pub fn redo_label(&self) -> Option<String> {
        let (last, _) = self.history.undone.last()?;
        Some(format!("Redo {last}"))
    }

    /// Read what other drawbars wrote to the library since it was last read.
    pub fn refresh(&mut self) {
        if !self.opened() || self.scanning {
            return;
        }
        self.scanning = true;
        self.send(Cmd::Refresh);
    }

    /// Rescan when the window comes back into focus, since anything may have changed
    /// the files while it was away.
    pub fn focus(&mut self, focused: bool) {
        if focused && !self.focused {
            self.rescan();
        }
        self.focused = focused;
    }

    /// Hold a send until the files have been checked for changes made outside drawbar.
    /// Returns `false` when there is nothing to check and the send may go now.
    pub fn hold_send(&mut self) -> bool {
        if !self.opened() {
            return false;
        }
        self.send_waits = true;
        self.rescan();
        true
    }

    /// Scan the tree again, to fold in what changed outside drawbar and what other
    /// drawbars wrote.
    pub(crate) fn rescan(&mut self) {
        if !self.opened() || self.scanning {
            return;
        }
        self.scanning = true;
        self.send(Cmd::Rescan);
    }

    /// Write everything, waiting for the saves in flight to answer, then let the library
    /// go. It blocks, so it is for the end of a session.
    ///
    /// ⚠️ A pass that sent anything is followed by another once its saves have answered,
    /// since a save may leave another asset to remove.
    pub fn close(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        const ROUNDS: usize = 8;
        if self.stopped().is_some() || self.stalled() {
            let lost = self.undrafted(workspace);
            if !lost.is_empty() {
                const SHOWN: usize = 5;
                log.trouble(format!(
                    "Edits made since they were last kept could not be written, since the \
                     library stopped answering: {}.",
                    crate::strings::quoted_some(&lost, SHOWN)
                ));
            }
            self.backend.abandon();
            return;
        }
        // The editor of a new tag's name never closes at a quit, so the tag goes under
        // the name it has, and what waited for it is asked for now.
        browser.tags.named();
        self.ask_tags(workspace, browser, log);
        for _ in 0..ROUNDS {
            if !self.answered(workspace, browser, queue, log) {
                break;
            }
            let sent = self.sent;
            self.sync(workspace, browser, Pass::Last);
            if self.sent == sent {
                break;
            }
        }
        self.learn_crcs(workspace);
        self.backend.finish();
    }

    /// Wait for every save and rename in flight to answer, and fold the answers in.
    /// `false` when one did not come.
    fn answered(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        while self.saving() || !self.moving.is_empty() || self.committing > 0 {
            match self.backend.recv() {
                None => return false,
                // ⚠️ A scan taken before the last pass's moves would read each as a
                // deletion, and nothing needs it now.
                Some(Event::Scanned(_)) => self.scanning = false,
                Some(event) => {
                    self.react(event, workspace, browser, queue, log);
                }
            }
        }
        true
    }

    /// Write everything, as [`Store::close`] does, before another library takes this one's
    /// place.
    pub fn hand_over(
        mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        self.close(workspace, browser, queue, log);
    }

    /// A held send goes ahead unless something waiting to be sent changed on disk.
    fn release(
        &self,
        moved: &BTreeSet<u64>,
        queue: &Queue,
        workspace: &Workspace,
        log: &mut Log,
    ) -> bool {
        let changed: Vec<&str> = queue
            .entries()
            .iter()
            .filter(|held| moved.contains(&held.id))
            .filter_map(|held| workspace.get(held.id))
            .map(|entity| entity.name.as_str())
            .collect();
        if changed.is_empty() {
            return true;
        }
        log.trouble(format!(
            "{} changed on disk since it was queued, so nothing was sent. Check it and send \
             again.",
            changed
                .iter()
                .map(|name| format!("“{name}”"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        false
    }

    /// Take in what opening found: whether the library may be written, and every file
    /// its view lists.
    fn begin(
        &mut self,
        opened: Opened,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        let Opened {
            writable,
            opening_copy,
            viewed,
            drafts,
            ignored,
            trouble,
        } = opened;
        self.opening_copy = opening_copy;
        for why in trouble {
            log.error(format!("opening the library: {why}"));
        }
        if ignored > 0 {
            log.say(format!(
                "drawbar left alone {ignored} unfinished change{} of other drawbars whose \
                 record does not read.",
                if ignored == 1 { "" } else { "s" }
            ));
        }
        self.phase = match writable {
            Ok(()) => Phase::Open,
            Err(Unwritable::ReadOnly(why)) => {
                log.trouble(format!(
                    "This computer's library opened read-only: {why}. Edits stay in memory \
                     until you quit."
                ));
                Phase::ReadOnly(why)
            }
            Err(Unwritable::Unfinished(why)) => {
                log.trouble(format!(
                    "This computer's library opened read-only: {why}. Unsaved edits are \
                     still kept."
                ));
                Phase::Unfinished(why)
            }
        };
        self.viewed(viewed, workspace, browser, queue, log);
        self.drafts_found = drafts;
        if self.opened() {
            self.first_scan = true;
            self.rescan();
        }
    }

    /// Take in that a scan of every file listed them for the first time since opening.
    fn listed_whole(&mut self, workspace: &mut Workspace, browser: &mut Browser, log: &mut Log) {
        self.listed = true;
        let drafts = std::mem::take(&mut self.drafts_found);
        self.take_drafts(drafts, workspace, log);
        // The count This computer's badge shows, said even at 0 to replace the last library's.
        log.say(match browser.folders.count(None, workspace) {
            0 => "No files on this computer.".to_string(),
            1 => "1 file on this computer.".to_string(),
            n => format!("{n} files on this computer."),
        });
        self.fetch_tracked(workspace, browser);
        self.rescues_due = true;
    }

    /// Take in the changes other drawbars left unfinished as a read found them, read at
    /// `now`: one that is gone was finished or settled.
    fn found_unfinished(&mut self, now: f64, standing: Vec<super::Leftover>) {
        let key = |leftover: &super::Leftover| (leftover.orphan.writer, leftover.orphan.record);
        let keys: BTreeSet<_> = standing.iter().map(key).collect();
        self.unfinished.retain(|known, _| keys.contains(known));
        for leftover in standing {
            let known = self.unfinished.entry(key(&leftover));
            let known = known.or_insert_with(|| Unfinished {
                leftover: leftover.clone(),
                seen: now,
                told: false,
            });
            let (was, is) = (&known.leftover.orphan, &leftover.orphan);
            // A change whose paths moved since, or whose window has gone, may have
            // been in progress: it is asked about once it stands again.
            if is.progress != was.progress {
                known.seen = now;
            }
            if was.live && !is.live {
                (known.seen, known.told) = (now, false);
            }
            known.leftover = leftover;
        }
        self.looked = now;
        let due = self.unfinished.values().filter(|known| !known.told);
        self.recheck = due
            .map(|known| known.seen + UNFINISHED_STANDS)
            .reduce(f64::min);
    }

    /// Ask about each change other drawbars left unfinished that a read found standing
    /// [`UNFINISHED_STANDS`] after it was first found, reading again when one is due.
    fn tell_unfinished(
        &mut self,
        ctx: &eframe::egui::Context,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let now = ctx.input(|input| input.time);
        for known in self.unfinished.values_mut().filter(|known| !known.told) {
            let orphan = &known.leftover.orphan;
            if orphan.live {
                // ⚠️ Another window may be making it now: finishing or rolling it back
                // would race it.
                log.say(format!(
                    "“{}” is in progress in another drawbar window.",
                    orphan.label
                ));
                known.told = true;
            } else if self.looked >= known.seen + UNFINISHED_STANDS {
                browser.ask_unfinished(known.leftover.clone());
                known.told = true;
            }
        }
        let Some(due) = self.recheck else {
            return;
        };
        if now < due {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(due - now));
        } else if self.scanning {
            // The scan's answer reads them again, or else the next look after it.
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        } else {
            self.recheck = None;
            self.refresh();
        }
    }

    /// Take each of `changes` the library keeps apart in `run`, or every one where
    /// none is named, as one change undo takes back.
    pub fn take_kept(&mut self, run: toshokan::log::Run, changes: Vec<super::KeptKey>) {
        if !self.open() {
            return;
        }
        let intent = Intent::Take { run, changes };
        self.committing += 1;
        self.answering.push_back(Step::Change(intent.describe()));
        self.send(Cmd::Intent(intent));
    }

    /// Never show or merge the changes the library keeps apart in `run` again.
    pub fn dismiss_kept(&mut self, run: toshokan::log::Run) {
        if !self.open() {
            return;
        }
        let intent = Intent::Dismiss { run };
        self.committing += 1;
        self.answering.push_back(Step::Change(intent.describe()));
        self.send(Cmd::Intent(intent));
    }

    /// Show what the library keeps apart, and say once when new runs of it appear.
    fn keep_apart(
        &mut self,
        kept: Vec<super::KeptApart>,
        workspace: &Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let held = |entity| {
            let id = self.ids.id(&Key::Entity(entity))?;
            let asset = workspace.get(id)?;
            Some(Held {
                id,
                name: asset.name.clone(),
                path: asset.path.clone(),
            })
        };
        let shown: Vec<crate::folders::Kept> = kept
            .iter()
            .map(|kept| {
                let rows = kept_rows(&kept.changes, held);
                crate::folders::Kept {
                    kind: kept.kind,
                    run: kept.run,
                    title: kept_title(kept, rows.len()),
                    rows,
                }
            })
            // A run stays until it is dismissed; a gap or unplaced lines go once
            // nothing is left to review.
            .filter(|kept| kept.kind == super::KeptKind::Run || !kept.rows.is_empty())
            .collect();
        let mut new = shown
            .iter()
            .filter(|kept| !self.kept_runs.contains(&kept.run))
            .peekable();
        if new.peek().is_some() {
            log.say(match new.map(|kept| kept.rows.len()).sum() {
                0 => "Changes are kept apart from the library: see Kept apart.".to_string(),
                1 => "1 change is kept apart from the library: see Kept apart.".to_string(),
                n => format!("{n} changes are kept apart from the library: see Kept apart."),
            });
        }
        self.kept_runs = shown.iter().map(|kept| kept.run).collect();
        browser.folders.kept = shown;
    }

    /// Settle as `how` says a change another drawbar left unfinished.
    pub fn settle_unfinished(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: toshokan::log::Settlement,
    ) {
        if !self.open() {
            return;
        }
        let change = format!("{} “{}”", settling(how), orphan.label);
        self.committing += 1;
        self.answering.push_back(Step::Change(change));
        self.send(Cmd::Settle { orphan, how });
    }

    /// Take in what the library shows now: its files, their folders, its tags, and who
    /// changed what. Returns the assets whose files moved or changed.
    fn viewed(
        &mut self,
        viewed: Viewed,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> BTreeSet<u64> {
        let Viewed {
            shown: change,
            dirs,
            changes,
            moves: _,
            followed,
            unlisted,
            unfinished,
            kept,
            retired,
            left,
            finishing,
        } = viewed;
        self.finishing = finishing.into_iter().collect();
        if let (Some(standing), Phase::Open) = (unfinished, &self.phase) {
            self.found_unfinished(workspace.ctx().input(|input| input.time), standing);
        }
        // Until a scan has listed every file, what a view adds is what was there.
        let first = !self.listed;
        self.quiet = followed.is_some();
        for (from, to) in followed.into_iter().flatten() {
            if let (Some(from), Some(to)) = (from, to) {
                self.follow_own(&from, &to, workspace);
            }
        }
        let mut shown = self.shown.take().unwrap_or_default();
        let reach = shown.apply(change);
        if reach.as_ref().is_none_or(|keys| !keys.is_empty()) {
            browser.folders.others = shown.others();
        }
        let (touched, said) = self.project(&shown, reach.as_ref(), workspace, browser, queue, log);
        if let Some(said) = said.filter(|_| !first && !self.quiet) {
            log.say(said);
        }
        if let Some(dirs) = dirs {
            browser.folders.sync(&dirs);
        }
        if let Some(unlisted) = unlisted {
            let unlisted: BTreeSet<LibPath> = unlisted.into_iter().collect();
            const SHOWN: usize = 3;
            let new: Vec<&LibPath> = unlisted.difference(&browser.folders.unwalked).collect();
            if !new.is_empty() {
                log.trouble(format!(
                    "drawbar could not read {}, so what is in it is not shown.",
                    crate::strings::quoted_some(&new, SHOWN)
                ));
            }
            browser.folders.unwalked = unlisted;
        }
        self.lose(&shown, workspace, browser);
        self.project_tags(&shown, browser);
        browser.forget_gone(workspace);
        if retired {
            self.history = History::default();
            self.awaiting.clear();
            log.say(
                "drawbar's own data for this library was copied and used elsewhere too, so \
                 their histories split. This drawbar goes on with a history of its own; \
                 what either side changed since is under Kept apart.",
            );
        }
        if !left.is_empty() {
            let left: Vec<String> = left.iter().map(|dir| format!("“{dir}”")).collect();
            log.say(format!(
                "{} {} still there, since {} files drawbar did not know of.",
                crate::strings::listed(&left),
                match left.len() {
                    1 => "is",
                    _ => "are",
                },
                match left.len() {
                    1 => "it holds",
                    _ => "they hold",
                },
            ));
        }
        self.keep_apart(kept, workspace, browser, log);
        if !first {
            self.attribute(&changes, workspace, log);
        }
        let compared = (workspace.layout(), browser.folders.names_hash());
        if self.compared.replace(compared) != Some(compared) {
            flag_duplicates(workspace, browser, log);
        }
        self.shown = Some(shown);
        self.quiet = false;
        touched
    }

    /// Follow a tree, or a file no entity is bound to, that this session's undo, redo or
    /// settling moved from `from` to `to`: each asset in it keeps its id, and moves with it.
    fn follow_own(&mut self, from: &LibPath, to: &LibPath, workspace: &mut Workspace) {
        self.ids.moved(from, to);
        for record in self.records.values_mut() {
            if let Some(moved) = record.path.as_ref().and_then(|at| at.moved(from, to)) {
                record.path = Some(moved);
            }
        }
        workspace.relocate(from, to);
    }

    /// Bring the workspace level with the files the view lists. Each file keeps the id it
    /// had: by its entity, or by its path, or as the file drawbar wrote there. Returns
    /// the assets whose files moved or changed, and what to say of the files that changed
    /// outside drawbar.
    fn project(
        &mut self,
        listed: &Shown,
        reach: Option<&BTreeSet<Key>>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> (BTreeSet<u64>, Option<String>) {
        self.rekey(listed, reach);
        let mut touched = BTreeSet::new();
        let (mut moved, mut deleted, mut missing) = (0, 0, 0);
        let deferred = std::mem::take(&mut self.deferred);
        let known: Vec<(u64, Key)> = match reach {
            None => self.ids.iter().map(|(id, key)| (id, key.clone())).collect(),
            Some(keys) => {
                let ids = keys.iter().filter_map(|key| self.ids.id(key));
                let ids: BTreeSet<u64> = ids.chain(deferred).collect();
                let keyed = ids
                    .into_iter()
                    .filter_map(|id| Some((id, self.ids.key(id)?.clone())));
                keyed.collect()
            }
        };
        for (id, key) in known {
            // A lost row keeps its id while the library keeps its entity.
            let shown = key
                .entity()
                .is_some_and(|entity| listed.lost.contains_key(&entity));
            if workspace.get(id).is_none() && !self.records.contains_key(&id) && !shown {
                self.ids.remove(id);
                continue;
            }
            let Some(record) = self.records.get(&id) else {
                continue;
            };
            let unsettled_at = |path: &LibPath| unsettled(&self.moving, path);
            if record
                .path
                .as_ref()
                .is_some_and(|path| self.finishing.contains(path))
            {
                self.deferred.insert(id);
                self.awaiting.insert(id);
                continue;
            }
            if let (true, Some(path)) = (self.awaiting.remove(&id), record.path.clone()) {
                if let Some(entity) = workspace.get(id).filter(|entity| entity.is_unsaved()) {
                    log.say(format!("Finished saving “{}”.", entity.name));
                }
                self.read_unheard(id, path, workspace);
                continue;
            }
            if record.saving || record.path.as_ref().is_some_and(unsettled_at) {
                self.deferred.insert(id);
                continue;
            }
            let Some((path, stat)) = listed.files.get(&key) else {
                touched.insert(id);
                match self.vanished(id, workspace, browser, queue, log) {
                    Vanished::Deleted => {
                        deleted += 1;
                        self.ids.remove(id);
                    }
                    Vanished::Missing => missing += 1,
                    Vanished::Untouched => {}
                }
                continue;
            };
            if unsettled_at(path) {
                self.deferred.insert(id);
                continue;
            }
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            if std::mem::take(&mut record.missing) {
                browser.folders.missing.remove(&id);
            }
            if record.path.as_ref() != Some(path) {
                touched.insert(id);
                moved += 1;
                record.path = Some(path.clone());
                let before = workspace.get(id).map(|entity| entity.name.clone());
                workspace.follow(id, browser.folders.ahead(path));
                if let Some(before) = before.filter(|_| !self.quiet) {
                    log.say(format!(
                        "“{before}” was moved to {path} outside drawbar; its tags went with it."
                    ));
                }
            }
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            let known = record.fingerprint.map(|print| print.stat());
            if let Some(stat) = stat.filter(|stat| known != Some(*stat)) {
                touched.insert(id);
                if self.quiet {
                    self.quietly.insert(id);
                }
                self.recheck(id, path.clone(), stat, workspace);
            }
        }
        let appeared = self.arrive(listed, reach, workspace, browser, log);
        // A file gone missing keeps the status line, which asks for it to be saved.
        let said = changed_outside(appeared, deleted, moved).filter(|_| missing == 0);
        (touched, said)
    }

    /// Give each asset the key the library knows its file by now, where that changed: a
    /// file an intent made an entity of, an entity whose file was let go of, or a file
    /// drawbar wrote that the view lists for the first time.
    fn rekey(&mut self, listed: &Shown, reach: Option<&BTreeSet<Key>>) {
        let reached = |key: &Key| reach.is_none_or(|keys| keys.contains(key));
        let by_path: BTreeMap<&LibPath, &Key> = listed
            .files
            .iter()
            .filter(|(key, _)| reached(key))
            .map(|(key, (path, _))| (path, key))
            .collect();
        let unlisted: Vec<u64> = match reach {
            None => self.ids.iter().map(|(id, _)| id).collect(),
            Some(keys) => keys.iter().filter_map(|key| self.ids.id(key)).collect(),
        };
        let unlisted = unlisted.into_iter().filter(|id| {
            let key = self.ids.key(*id);
            key.is_some_and(|key| !listed.files.contains_key(key))
        });
        let unkeyed = self
            .records
            .keys()
            .copied()
            .filter(|id| self.ids.key(*id).is_none());
        let mut rekeyed = Vec::new();
        for id in unlisted.chain(unkeyed) {
            let path = self
                .records
                .get(&id)
                .and_then(|record| record.path.as_ref());
            let key = path.and_then(|path| by_path.get(path));
            if let Some(key) = key.filter(|key| self.ids.id(key).is_none()) {
                rekeyed.push((id, (*key).clone()));
            }
        }
        for (id, key) in rekeyed {
            self.ids.insert(id, key);
        }
    }

    /// Take in each file the view lists that no asset stands for. Returns how many.
    fn arrive(
        &mut self,
        listed: &Shown,
        reach: Option<&BTreeSet<Key>>,
        workspace: &mut Workspace,
        browser: &Browser,
        log: &mut Log,
    ) -> usize {
        let files: Vec<(&Key, &(LibPath, Option<Stat>))> = match reach {
            None => listed.files.iter().collect(),
            Some(keys) => keys
                .iter()
                .filter_map(|key| listed.files.get_key_value(key))
                .collect(),
        };
        let mut arrived: Vec<(LibPath, Key, Stat)> = files
            .into_iter()
            .filter(|(key, _)| self.ids.id(key).is_none())
            .filter_map(|(key, (path, stat))| Some((path.clone(), key.clone(), (*stat)?)))
            .collect();
        if arrived.is_empty() {
            return 0;
        }
        arrived.sort_by(|a, b| a.0.cmp(&b.0));
        let mut next = workspace.next_id();
        let mut back = Vec::new();
        for (path, key, stat) in arrived {
            let came = key.entity().and_then(|entity| listed.came.get(&entity));
            let mut saved = newcomer(next, Found::unread(path, stat), &mut self.records);
            if let Some(came) = came {
                saved.origin = came.origin();
            }
            back.push(saved);
            self.ids.insert(next, key);
            next = next.saturating_add(1);
        }
        let ids: Vec<u64> = back.iter().map(|saved| saved.id).collect();
        workspace.restore(back, Some(next), log);
        for id in &ids {
            self.settle(*id, workspace);
            self.place_ahead(*id, workspace, &browser.folders);
        }
        ids.len()
    }

    /// Show each entity whose file is nowhere, which no asset stands for, as lost, with
    /// its tags, until the user lets it go.
    fn lose(&mut self, listed: &Shown, workspace: &mut Workspace, browser: &mut Browser) {
        let mut lost = Vec::new();
        for (entity, path) in &listed.lost {
            let key = Key::Entity(*entity);
            let id = match self.ids.id(&key) {
                Some(id) if workspace.get(id).is_some() => continue,
                Some(id) => id,
                None => {
                    let id = workspace.next_id();
                    workspace.reserve(id.saturating_add(1));
                    self.ids.insert(id, key);
                    id
                }
            };
            lost.push(crate::folders::Lost {
                id,
                path: path.clone(),
            });
        }
        browser.folders.set_lost(lost);
    }

    /// Bring the browser's tags level with the view, which is all there is once no change
    /// to them waits to be answered.
    fn project_tags(&self, shown: &Shown, browser: &mut Browser) {
        if self.asked.is_empty() && self.committing == 0 {
            browser.tags.settle();
        }
        let worn_names = shown.worn.values().flatten().cloned();
        let names = worn_names.chain(shown.names.iter().cloned());
        let worn = shown.worn.iter().filter_map(|(entity, worn)| {
            let id = self.ids.id(&Key::Entity(*entity))?;
            Some((id, worn.iter().cloned().collect()))
        });
        browser.tags.project(names.collect(), worn.collect());
    }

    /// Say which tags another drawbar changed.
    fn attribute(
        &self,
        changes: &[toshokan::report::Change],
        workspace: &Workspace,
        log: &mut Log,
    ) {
        let tags = What::Field(TAGS.name().to_string());
        for change in changes.iter().filter(|change| change.what == tags) {
            let By::Writer { label, .. } = &change.by else {
                continue;
            };
            let id = self.ids.id(&Key::Entity(change.entity));
            let Some(entity) = id.and_then(|id| workspace.get(id)) else {
                continue;
            };
            log.say(format!(
                "Another {label} changed the tags of “{}”.",
                entity.name
            ));
        }
    }

    /// Turn the changes to the tags the user asked for into intents, in order, each once
    /// every asset it names has a file the library lists and the one before has answered.
    fn ask_tags(&mut self, workspace: &mut Workspace, browser: &mut Browser, log: &mut Log) {
        self.asked.extend(browser.take_tag_asks());
        // A tag waits on the file it names only while that file is still to be written.
        self.take_asks(workspace);
        if self.asked.is_empty() {
            return;
        }
        match &self.phase {
            Phase::Opening => return,
            Phase::Open => {}
            Phase::ReadOnly(why) | Phase::Unfinished(why) | Phase::Failed(why) => {
                let why = why.clone();
                self.asked.clear();
                return log.trouble(format!("The tags did not change: {why}."));
            }
        }
        // ⚠️ Each is made against the view the one before left, so they wait in turn: a
        // rename asked right after a tag renames what the tag put on.
        while self.committing == 0 {
            let (Some(shown), Some(asked)) = (&self.shown, self.asked.first()) else {
                return;
            };
            match self.intend(asked, shown, workspace) {
                None => return,
                Some(intent) => {
                    let asked = self.asked.remove(0);
                    self.tell_untagged(&asked, workspace, log);
                    if let Some(intent) = intent {
                        self.commit(intent);
                    }
                }
            }
        }
    }

    /// Whether the file of a kept asset is still to be written: placed and saved, or
    /// copied in, with nothing to stop it.
    fn writes(&self, id: u64, workspace: &Workspace) -> bool {
        let Some(entity) = workspace.get(id).filter(|entity| entity.kept) else {
            return false;
        };
        let saving = self.records.get(&id).is_some_and(|record| record.saving);
        let asked = self.asks.iter().any(|(ask, _)| *ask == FileAsk::Save(id));
        entity.path.is_none() || saving || asked || workspace.arriving(id).is_some()
    }

    /// Say which assets a tag that went did not reach: those with no file in the library
    /// and none on its way.
    fn tell_untagged(&self, asked: &Asked, workspace: &Workspace, log: &mut Log) {
        let Asked::Tag { assets, name, .. } = asked else {
            return;
        };
        let missed = assets.iter().filter(|id| self.ids.key(**id).is_none());
        let names: Vec<String> = missed
            .filter_map(|id| workspace.get(*id))
            .map(|entity| format!("“{}”", entity.name))
            .collect();
        if !names.is_empty() {
            log.trouble(format!(
                "{} has no file in the library, so “{name}” was not put on it.",
                names.join(", ")
            ));
        }
    }

    /// The intent `asked` makes against `view`: `Some(None)` where it changes nothing,
    /// and `None` while an asset it names waits for its file.
    fn intend(
        &self,
        asked: &Asked,
        shown: &Shown,
        workspace: &Workspace,
    ) -> Option<Option<Intent>> {
        let entities = |assets: &[u64]| -> Vec<toshokan::EntityId> {
            let keys = assets.iter().filter_map(|id| self.ids.key(*id));
            keys.filter_map(Key::entity).collect()
        };
        let wearing = |name: &String| shown.wearing(name);
        let intent = match asked {
            Asked::Tag { assets, name, make } => {
                let waits = |id: &u64| self.ids.key(*id).is_none() && self.writes(*id, workspace);
                if assets.iter().any(waits) {
                    return None;
                }
                let listed = shown.names.contains(name);
                let files: Vec<Key> = assets
                    .iter()
                    .filter_map(|id| self.ids.key(*id).cloned())
                    .filter(|key| match key {
                        Key::Entity(entity) => !wearing(name).contains(entity),
                        Key::File(_) => true,
                    })
                    .collect();
                let make = *make && !listed;
                match (files.is_empty(), make) {
                    (true, false) => None,
                    (true, true) => Some(Intent::MakeTag { name: name.clone() }),
                    (false, make) => Some(Intent::Tag {
                        files,
                        name: name.clone(),
                        make,
                    }),
                }
            }
            Asked::Untag { assets, name } => {
                let wear = wearing(name);
                let entities: Vec<_> = entities(assets)
                    .into_iter()
                    .filter(|entity| wear.contains(entity))
                    .collect();
                (!entities.is_empty()).then(|| Intent::Untag {
                    entities,
                    name: name.clone(),
                })
            }
            Asked::Make(name) => {
                let listed = shown.names.contains(name);
                (!listed).then(|| Intent::MakeTag { name: name.clone() })
            }
            Asked::Rename { from, to } => {
                let entities = wearing(from);
                let listed = shown.names.contains(from);
                (listed || !entities.is_empty()).then(|| Intent::RenameTag {
                    entities,
                    from: from.clone(),
                    to: to.clone(),
                })
            }
            Asked::Remove(name) => {
                let entities = wearing(name);
                let listed = shown.names.contains(name);
                (listed || !entities.is_empty()).then(|| Intent::RemoveTag {
                    entities,
                    name: name.clone(),
                })
            }
            Asked::Forget(id) => match self.ids.key(*id) {
                Some(Key::Entity(entity)) => Some(Intent::Forget { entity: *entity }),
                _ => None,
            },
        };
        Some(intent)
    }

    /// A rescan has answered: once each file it found changed has been read again, it
    /// is done, and a send held for it goes ahead unless what it would send moved.
    fn scanned(&mut self, queue: &Queue, workspace: &Workspace, log: &mut Log) -> bool {
        self.scanning = !self.rechecking.is_empty();
        if self.scanning {
            return false;
        }
        let moved = std::mem::take(&mut self.moved);
        std::mem::take(&mut self.send_waits) && self.release(&moved, queue, workspace, log)
    }

    /// Read again, quietly, the file of an asset whose save went unheard: one that holds
    /// the edit leaves the asset saved.
    fn read_unheard(&mut self, id: u64, path: LibPath, workspace: &mut Workspace) {
        let Some(print) = self.records.get(&id).and_then(|record| record.fingerprint) else {
            return;
        };
        self.quietly.insert(id);
        let stat = Stat {
            len: print.len,
            modified: print.modified,
        };
        self.recheck(id, path, stat, workspace);
    }

    /// A file drawbar knew changed on disk: an unread one is only known to be another,
    /// and one drawbar holds is read again to see what it holds now.
    fn recheck(&mut self, id: u64, path: LibPath, stat: Stat, workspace: &mut Workspace) {
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        if record.holds == Holds::Unread {
            record.fingerprint = Some(Fingerprint::unread(stat));
            workspace.stale(id);
            return;
        }
        self.rechecking.insert(id);
        let files = vec![(id, path, record.fingerprint)];
        let room = self.budget;
        self.send(Cmd::Read { files, room });
    }

    /// The rescues in this install's own folders, by name, each read whole on this thread
    /// to check it. A partial one is never a slot's only copy, and is deleted.
    #[cfg(not(target_arch = "wasm32"))]
    fn shelved_rescues(&self) -> Vec<Rescue> {
        let local = self.backend.local().join(RESCUED);
        let shelves = [Some(local), self.shelf.clone()];
        let entries = shelves
            .into_iter()
            .flatten()
            .filter_map(|shelf| std::fs::read_dir(shelf).ok())
            .flatten()
            .flatten();
        let mut found: Vec<Rescue> = entries
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let path = entry.path();
                if !name.starts_with(nord_usb::envelope::RESCUED) {
                    return None;
                }
                if !nord_usb::envelope::is_rescue(&name) {
                    let _ = std::fs::remove_file(&path);
                    return None;
                }
                let file = std::fs::File::open(&path).and_then(nord_usb::envelope::Positional::new);
                Some(Rescue {
                    slot: Rescue::slot_of(&name),
                    name,
                    broken: nord_usb::block_on(super::unwhole(file)),
                    at: path,
                })
            })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    }

    /// Whether the library opened holding a copy that stopped partway as it was opened,
    /// which a copy opened again may replace.
    pub fn opening_copy(&self) -> bool {
        self.opening_copy
    }

    /// Whether the library took in the list an older drawbar kept whose
    /// [`older::List::hash`] is `hash`.
    pub fn has_taken_in(&self, hash: &str) -> bool {
        let shown = self.shown.as_ref();
        shown.is_some_and(|shown| shown.taken.contains(hash))
    }

    /// Whether the list of hash `hash` is the library's to keep now: it took the list in,
    /// this session took the list up, and each asset the list brought this session is
    /// written, with its unsaved edit kept where it holds one, or is gone.
    pub fn handed_over(&self, hash: &str, workspace: &Workspace) -> bool {
        let kept = |id: &u64| {
            let Some(entity) = workspace.get(*id) else {
                return true;
            };
            let record = self.records.get(id);
            let written =
                record.is_some_and(|record| record.fingerprint.is_some() && !record.saving);
            let restored = !self.restoring.contains_key(id);
            written && restored && (!entity.is_unsaved() || self.drafted.contains_key(id))
        };
        self.took_up && self.has_taken_in(hash) && self.brought.iter().all(kept)
    }

    /// Take the list an older drawbar kept into the library as one change, which Undo
    /// takes back, once a session: once a scan has listed every file, while the library
    /// may be written and no change of drawbar's own waits to be finished. Each folder is
    /// a new one in the library's top level, and each asset a file in its folder, both
    /// under names nothing beside them has, the asset's with the extension its bytes call
    /// for. An unsaved edit stays unsaved over the file. `keep` is given, before the
    /// change is sent, where each asset that came with an unsaved edit goes, by its place
    /// in the list.
    ///
    /// Where the library took the list in already, nothing else is written: each unsaved
    /// edit is put back over the file the take-in gave it, as the list's record names it,
    /// or else brought in as a file of its own beside where it went.
    pub fn take_in(
        &mut self,
        list: &older::List,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
        keep: impl FnOnce(&[(usize, LibPath)]),
    ) {
        // Nothing else is in flight whose failures the take-in would hold.
        let idle = !self.scanning && self.committing == 0 && self.finishing.is_empty();
        let ready = self.open() && self.listed && idle;
        if !ready || list.is_empty() || self.took_up {
            return;
        }
        self.took_up = true;
        if self.has_taken_in(&list.hash) {
            return self.put_back(list, workspace, browser, log);
        }
        self.send_batch();
        self.part(Intent::TakeIn {
            tags: list.tags.iter().cloned().collect(),
            hash: list.hash.clone(),
        });
        let root = LibPath::root();
        let mut dirs = Vec::new();
        for folder in &list.folders {
            let name = browser
                .folders
                .free(&root, &names::portable(folder), workspace);
            let path = root.join(&name);
            browser.folders.add(path.clone());
            self.part(Intent::MakeDir { path: path.clone() });
            dirs.push(path);
        }
        let mut worn: BTreeMap<&str, Vec<Key>> = BTreeMap::new();
        let mut ids = BTreeSet::new();
        let mut placed = Vec::new();
        for (at, asset) in list.assets.iter().enumerate() {
            let id = workspace.next_id();
            let origin = asset
                .came
                .map_or_else(|| Origin::File(asset.name.clone()), Came::origin);
            let saved = Saved {
                id,
                name: asset.name.clone(),
                path: None,
                origin,
                content: Content::whole(asset.saved.clone()),
                unsaved: asset.unsaved.clone(),
            };
            workspace.restore(vec![saved], None, log);
            let Some(entity) = workspace.get(id) else {
                continue;
            };
            let dir = asset.folder.and_then(|at| dirs.get(at)).unwrap_or(&root);
            let name = browser
                .folders
                .free(dir, &library_filename(entity), workspace);
            let path = dir.join(&name);
            workspace.follow(id, path.clone());
            self.save(id, workspace, &[]);
            for tag in &asset.tags {
                worn.entry(tag).or_default().push(Key::File(path.clone()));
            }
            if asset.unsaved.is_some() {
                placed.push((at, path));
            }
            ids.insert(id);
        }
        for (name, files) in worn {
            let name = name.to_string();
            self.part(Intent::Tag {
                files,
                name,
                make: false,
            });
        }
        let said = list.said();
        self.brought.extend(&ids);
        self.taking_in = Some(TakingIn {
            ids,
            said,
            held: Vec::new(),
            why: None,
        });
        keep(&placed);
        self.send_batch();
    }

    /// Put back each unsaved edit of the list the library took in, which the session
    /// that took it in may have ended before keeping: over the file the take-in gave it,
    /// where the list's record says which that is and the library holds it, and as a
    /// file of its own beside where it went otherwise. An edit kept since is left as it
    /// is.
    fn put_back(
        &mut self,
        list: &older::List,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        for (at, asset) in list.assets.iter().enumerate() {
            let Some(edit) = &asset.unsaved else {
                continue;
            };
            let placed = list.placed.as_ref().and_then(|placed| placed.get(&at));
            let held = placed.and_then(|path| {
                let mut listed = workspace.listed();
                listed
                    .find(|entity| entity.path.as_ref() == Some(path))
                    .map(|entity| entity.id)
            });
            let edited = |id: &u64| {
                let unsaved = workspace.get(*id).is_some_and(LocalEntity::is_unsaved);
                self.restoring.contains_key(id) || unsaved
            };
            let key = held.and_then(|id| Some((id, self.ids.key(id)?.clone())));
            match (held, key) {
                (Some(id), _) if edited(&id) => _ = self.brought.insert(id),
                (_, Some((id, key))) => {
                    self.brought.insert(id);
                    let draft = Draft::Bytes(edit.clone());
                    let since = Since::Unless(blake3::hash(&asset.saved));
                    self.restoring.insert(id, (key, draft, since, 0));
                    workspace.hurry(id);
                    self.restore_draft(id, workspace, browser, log);
                }
                _ => self.bring_edit(list, at, placed, workspace, browser, log),
            }
        }
    }

    /// Bring in the unsaved edit of the asset at `at` in `list`, whose file the take-in
    /// gave it is not known or gone, as a file of its own: in the folder that file was
    /// in, where the library holds it, and otherwise in the top level.
    fn bring_edit(
        &mut self,
        list: &older::List,
        at: usize,
        placed: Option<&LibPath>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let asset = &list.assets[at];
        let Some(edit) = &asset.unsaved else {
            return;
        };
        let root = LibPath::root();
        let named = || {
            let folder = list.folders.get(asset.folder?)?;
            Some(root.join(&names::portable(folder)))
        };
        let dir = placed.map(LibPath::parent).or_else(named);
        let dir = dir
            .filter(|dir| browser.folders.id_of(dir).is_some())
            .unwrap_or(root);
        let id = workspace.next_id();
        let saved = Saved {
            id,
            name: asset.name.clone(),
            path: None,
            origin: asset
                .came
                .map_or_else(|| Origin::File(asset.name.clone()), Came::origin),
            content: Content::whole(edit.clone()),
            unsaved: None,
        };
        workspace.restore(vec![saved], None, log);
        let Some(entity) = workspace.get(id) else {
            return;
        };
        let name = browser
            .folders
            .free(&dir, &library_filename(entity), workspace);
        workspace.place(id, dir.join(&name));
        self.brought.insert(id);
        log.say(format!(
            "drawbar could not find the file “{}” was brought in as, so its unsaved edit \
             is a file of its own, “{name}”.",
            asset.name
        ));
    }

    /// Follow a take-in that wrote what it brings, or may have: each part that did not
    /// land is answered as it would be alone, and the user is told where it `landed`.
    fn follow_take_in(
        &mut self,
        taking: TakingIn,
        landed: bool,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        for event in taking.held {
            self.fold(event, workspace, browser, queue, log);
        }
        if landed {
            log.say(taking.said);
        }
    }

    /// Let go of what the take-in that wrote nothing brought, and say why: `why`, or the
    /// first reason its parts gave. The list stays where it was kept, for the next
    /// session to take in.
    fn not_taken_in(
        &mut self,
        taking: TakingIn,
        why: Option<String>,
        workspace: &mut Workspace,
        log: &mut Log,
    ) {
        for id in taking.ids {
            self.records.remove(&id);
            workspace.forget(id);
        }
        self.rescan();
        let why = why
            .or(taking.why)
            .unwrap_or_else(|| "it made nothing".to_string());
        log.error(format!("{TAKE_IN}: {why}"));
        log.trouble(format!(
            "drawbar could not bring the sounds an earlier version kept into the library: \
             {why}. They stay where they were, and drawbar tries again next time."
        ));
    }

    /// A write to the instrument has left a slot's former occupant on this computer:
    /// look for it once the instrument is idle.
    pub fn rescue_due(&mut self) {
        self.rescues_due = true;
    }

    /// Take in the slots' former occupants writes left, once the library is listed and
    /// something has said to look: on the desktop at once, in the browser once its storage
    /// answers.
    ///
    /// ⚠️ Only while the instrument is idle: a write in progress holds its slot's
    /// occupant under the name it takes as a rescue, and is not one.
    pub fn look_for_rescues(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        if !self.listed || !self.rescues_due {
            return;
        }
        self.rescues_due = false;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let found = self.shelved_rescues();
            self.take_in_rescues(found, workspace, browser, log);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (workspace, browser, log);
            self.backend.look_for_rescues();
        }
    }

    /// Put each slot's former occupant in the library as a file of its own, where the
    /// library may be written. One that does not read, or that a library which cannot be
    /// written leaves where it is, is asked about. One already on its way in is let be.
    fn take_in_rescues(
        &mut self,
        found: Vec<Rescue>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        for rescue in found {
            if self.shelved.values().any(|at| *at == rescue.at) {
                continue;
            }
            match (&rescue.broken, self.open()) {
                (None, true) => self.keep_rescue(rescue, workspace, browser, log),
                _ => browser.ask_rescue(rescue),
            }
        }
    }

    /// Do `what` with a slot's former occupant, and return the URL that shows it where
    /// that is what was asked.
    pub fn rescue(
        &self,
        rescue: Rescue,
        what: Rescuing,
        browser: &mut Browser,
        log: &mut Log,
    ) -> Option<String> {
        match what {
            Rescuing::Show => {
                let url = rescue_url(&rescue);
                browser.ask_rescue(rescue);
                return url;
            }
            Rescuing::Confirm => browser.ask_discard_rescue(rescue),
            Rescuing::Discard => discard_rescue(rescue, log),
        }
        None
    }

    /// Copy a rescue into the library's top level, under its own name or a free one, as
    /// one change that can be undone, and delete it once the copy lands.
    fn keep_rescue(
        &mut self,
        rescue: Rescue,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let free = browser
            .folders
            .free(&LibPath::root(), &rescue.name, workspace);
        let to = LibPath::root().join(&free);
        #[cfg(not(target_arch = "wasm32"))]
        let from = rescue.at.clone();
        #[cfg(target_arch = "wasm32")]
        let from = rescue.file;
        let Some(len) = super::outside_len(&from) else {
            return log.trouble(format!("“{}” could not be read.", rescue.name));
        };
        let origin = match rescue.slot {
            Some(at) => {
                log.say(format!(
                    "{} may be empty after a write to it failed: what it held goes into the                      library as “{free}”.",
                    crate::strings::shown(at)
                ));
                Origin::Rescued { at }
            }
            None => Origin::File(free),
        };
        let id = workspace.arrive(to, origin, CopyOf::Outside(from), len);
        self.shelved.insert(id, rescue.at);
    }

    /// Put an asset just restored where the folder changes not yet sent will put its
    /// file. Its record keeps the path the disk has now, and those changes move it.
    fn place_ahead(&self, id: u64, workspace: &mut Workspace, folders: &Folders) {
        let Some(path) = self
            .records
            .get(&id)
            .and_then(|record| record.path.as_ref())
        else {
            return;
        };
        let ahead = folders.ahead(path);
        if ahead != *path {
            workspace.follow(id, ahead);
        }
    }

    /// Record the stamp of an asset just restored, so nothing is written again for it.
    fn settle(&mut self, id: u64, workspace: &Workspace) {
        let (Some(entity), Some(record)) = (workspace.get(id), self.records.get_mut(&id)) else {
            return;
        };
        record.saved = entity.saved.stamp();
    }

    /// A file drawbar knew now holds something else. One drawbar has not read is only
    /// looked at again: what it holds is not known, and nothing shows it.
    fn changed(
        &mut self,
        id: u64,
        found: Found,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let quietly = self.quietly.remove(&id);
        if !found.read() {
            if let Some(record) = self.records.get_mut(&id) {
                record.fingerprint = Some(found.fingerprint());
            }
            workspace.stale(id);
            return;
        }
        let Some(entity) = workspace.get(id) else {
            return;
        };
        let mut print = found.fingerprint();
        if let (None, Some(bytes)) = (print.crc, &found.bytes) {
            print.crc = Some(nord_format::crc::crc32(bytes));
        }
        let name = entity.name.clone();
        let unsaved = entity.is_unsaved();
        let holds = found.holds();
        match (unsaved, found.bytes, found.file) {
            (true, Some(bytes), _) => workspace.rebase(id, bytes, log),
            (true, None, Some(file)) => workspace.rebase_file(id, file),
            (false, Some(bytes), _) => workspace.adopt(id, bytes, log),
            (false, None, Some(file)) => workspace.adopt_file(id, file),
            (_, None, None) => return,
        }
        // An edit held over the file the asset rests in is made again over the file as
        // it is now, and one that file already holds leaves nothing to choose between.
        match unsaved && workspace.get(id).is_some_and(LocalEntity::is_unsaved) {
            true => conflict(id, workspace, browser, log),
            false if quietly => {}
            false => log.say(format!(
                "“{name}” changed on disk, and drawbar now shows it as it is there."
            )),
        }
        browser.folders.missing.remove(&id);
        let saved = workspace.get(id).map(|entity| entity.saved.stamp());
        if let (Some(record), Some(saved)) = (self.records.get_mut(&id), saved) {
            record.fingerprint = Some(print);
            record.saved = saved;
            record.missing = false;
            record.holds = holds;
        }
    }

    /// A file drawbar knew is gone, and nothing on disk holds what it held.
    fn vanished(
        &mut self,
        id: u64,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> Vanished {
        let Some(record) = self.records.get_mut(&id) else {
            return Vanished::Untouched;
        };
        if record.missing {
            return Vanished::Untouched;
        }
        let Some(entity) = workspace.get(id) else {
            return Vanished::Untouched;
        };
        let name = entity.name.clone();
        let keep = precious(entity, queue)
            || !browser.tags.worn(id).is_empty()
            || entity.origin.slot().is_some();
        if keep {
            record.missing = true;
            browser.folders.missing.insert(id);
            log.trouble(format!(
                "“{name}” is missing from the library folder. drawbar still holds it; save \
                 it to write it back."
            ));
            return Vanished::Missing;
        }
        self.records.remove(&id);
        workspace.forget(id);
        if !self.quiet {
            log.say(format!("“{name}” was deleted outside drawbar."));
        }
        // The draft stays in this install's own state, and an open that finds the file
        // again puts it back.
        if self.restoring.remove(&id).is_some() {
            log.say(format!(
                "The unsaved edit of “{name}” is still kept, for when its file is back."
            ));
        }
        Vanished::Deleted
    }

    fn saved(
        &mut self,
        id: u64,
        path: LibPath,
        result: Result<Fingerprint, Failure>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        self.deleted_with(id, result.is_ok());
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        record.saving = false;
        // ⚠️ The asset an overwrite came from holds its bytes until they are saved here.
        if let Some(leaving) = workspace.saved_over(id, record.saved, result.is_ok()) {
            leave(&mut self.left, leaving, workspace, log);
        }
        let name = workspace
            .get(id)
            .map_or_else(|| path.leaf().to_string(), |entity| entity.name.clone());
        let why = match result {
            Ok(print) => {
                record.fingerprint = Some(print);
                record.missing = false;
                record.holds = Holds::Whole;
                browser.folders.missing.remove(&id);
                self.saved_draft(id, workspace);
                return;
            }
            Err(Failure::Moved) if record.fingerprint.is_none() => {
                // A file took the name first. The asset takes another at the next sync.
                self.records.remove(&id);
                workspace.unplace(id);
                self.rescan();
                Some("a file of that name appeared in the folder first".to_string())
            }
            Err(Failure::Moved) => {
                self.rescan();
                Some("it changed on disk since drawbar read it".to_string())
            }
            Err(Failure::Room(_)) => Some(too_much()),
            Err(Failure::Io(why)) => Some(why),
            // ⚠️ Kept unsaved until a read of the file finds it holds the edit, which
            // it may.
            Err(Failure::Unheard) => None,
        };
        workspace.unsave(id);
        if let (Some(record), Some(entity)) = (self.records.get_mut(&id), workspace.get(id)) {
            record.saved = entity.saved.stamp();
        }
        let Some(why) = why else {
            return self.read_unheard(id, path, workspace);
        };
        log.error(format!("saving {path}: {why}"));
        log.trouble(format!(
            "“{name}” was not saved, because {why}. The edit is kept and still unsaved."
        ));
    }

    /// Take in what the save of an edit of a file resting in the library wrote: the asset
    /// rests in the new file. An edit not saved stays, and so does the file it was over.
    fn rewritten(
        &mut self,
        id: u64,
        path: LibPath,
        result: Result<Found, Failure>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        record.saving = false;
        let why = match result {
            Ok(found) => {
                record.fingerprint = Some(found.fingerprint());
                record.missing = false;
                record.holds = found.holds();
                browser.folders.missing.remove(&id);
                match found.file {
                    Some(file) => {
                        workspace.edit_saved(id, file);
                        self.saved_draft(id, workspace);
                    }
                    // A file that no longer indexes is read again like any other.
                    None => {
                        workspace.edit_not_saved(id);
                        workspace.mark_pending(id, false);
                        workspace.relist(id, found.stat.len);
                    }
                }
                return;
            }
            Err(Failure::Moved) => {
                self.rescan();
                "it changed on disk since drawbar read it".to_string()
            }
            Err(Failure::Room(_)) => too_much(),
            Err(Failure::Io(why)) => why,
            Err(Failure::Unheard) => {
                workspace.edit_not_saved(id);
                return self.read_unheard(id, path, workspace);
            }
        };
        workspace.edit_not_saved(id);
        let name = workspace
            .get(id)
            .map_or_else(|| path.leaf().to_string(), |entity| entity.name.clone());
        log.error(format!("saving {path}: {why}"));
        log.trouble(format!(
            "“{name}” was not saved, because {why}. The edit is kept and still unsaved."
        ));
    }

    /// Send the saves asked for of edits of files resting in the library, each once the
    /// save or move before it has answered.
    ///
    /// ⚠️ An asset whose file is missing, or was never written, has nothing to rewrite, and
    /// its save is refused here.
    fn send_edits(&mut self, workspace: &mut Workspace, waiting: &[(LibPath, LibPath)]) {
        for id in workspace.edits_to_save() {
            let record = self.records.get_mut(&id);
            let held = record.and_then(|record| {
                let (path, expect) = (record.path.clone()?, record.fingerprint?);
                (!record.missing).then_some((record, path, expect))
            });
            let Some((record, path, expect)) = held else {
                workspace.edit_not_saved(id);
                continue;
            };
            if record.saving || unsettled(waiting, &path) {
                continue;
            }
            let Some((from, edit)) = workspace.send_edit(id) else {
                continue;
            };
            let Ok(pieces) = edit.pieces(&from) else {
                workspace.edit_not_saved(id);
                continue;
            };
            record.saving = true;
            self.part(Intent::Rewrite {
                id,
                path,
                pieces: Arc::new(pieces),
                expect: expect.expect(),
            });
        }
    }

    /// Take in what a copy from outside the library found where it landed. A copy whose
    /// name was taken first is placed again; one that failed is read into memory instead,
    /// as a file kept nowhere yet, and an overwrite that failed leaves the file as it was.
    fn imported(
        &mut self,
        id: u64,
        path: LibPath,
        result: Result<Found, Failure>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let from = workspace.arrived(id);
        self.deleted_with(id, result.is_ok());
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        record.saving = false;
        let name = path.leaf().to_string();
        let why = match result {
            Ok(found) => {
                record.fingerprint = Some(found.fingerprint());
                record.missing = false;
                record.holds = found.holds();
                browser.folders.missing.remove(&id);
                workspace.took(id, None, found.file);
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(shelved) = self.shelved.remove(&id) {
                    if let Err(e) = std::fs::remove_file(&shelved) {
                        log.error(format!(
                            "{} is in the library, and was not deleted: {e}",
                            shelved.display()
                        ));
                    }
                }
                #[cfg(target_arch = "wasm32")]
                if let Some(shelved) = self.shelved.remove(&id) {
                    super::remove_private(shelved);
                }
                workspace.landed_copy(id);
                // The asset the copy was moved from goes now it has landed, and its file
                // with it at the next pass.
                if let Some(leaving) = workspace.moved_over(id) {
                    leave(&mut self.left, leaving, workspace, log);
                }
                return log.say(format!("“{name}” is on this computer."));
            }
            Err(Failure::Moved) if record.fingerprint.is_none() => {
                // A file took the name first. The asset takes another at the next sync.
                self.records.remove(&id);
                workspace.unplace(id);
                if let Some(from) = from {
                    workspace.arrive_again(id, from);
                }
                return self.rescan();
            }
            Err(Failure::Moved) => "it changed on disk since drawbar read it".to_string(),
            Err(Failure::Room(_)) => too_much(),
            Err(Failure::Io(why)) => why,
            // A copy that landed is found by the look as a file of the library's.
            Err(Failure::Unheard) => "its answer was lost".to_string(),
        };
        log.error(format!("copying {name} into the library: {why}"));
        // A copy moved over this file from another that did not land leaves that one.
        workspace.moved_over(id);
        match record.fingerprint {
            Some(print) => {
                workspace.relist(id, print.len);
                log.trouble(format!("“{name}” was not overwritten, because {why}."));
                self.rescan();
            }
            None => {
                self.records.remove(&id);
                let fetched = workspace.fetched_copy_failed(id);
                if fetched && workspace.give_up_bundle() {
                    log.trouble("The bundle was not written: a copy did not land.");
                }
                workspace.forget(id);
                match from {
                    // A file the instrument was read into may be a large piano, which is
                    // never read whole.
                    Some(CopyOf::Outside(from)) if !fetched => {
                        log.trouble(format!(
                            "“{name}” was not copied into the library, because {why}. It is \
                             kept in memory instead."
                        ));
                        workspace.read_outside(name, from);
                    }
                    _ => log.trouble(format!("“{name}” was not made, because {why}.")),
                }
            }
        }
    }

    /// Send, in order, the changes to the folders and files the user asked for, and
    /// return whether every one went. One that must wait for the answer to one before it
    /// makes it return `false`, and the next sync sends it.
    ///
    /// ⚠️ Nothing is sent while a rescan is in flight: its listing must describe the
    /// files as the commands before it left them, or a file moved after it was sent would
    /// read as one deleted and another made.
    pub fn sync(&mut self, workspace: &mut Workspace, browser: &mut Browser, pass: Pass) -> bool {
        // A tag asked for with the files it waits on can go in their change.
        self.asked.extend(browser.take_tag_asks());
        if pass != Pass::Files {
            self.learn_crcs(workspace);
            self.keep_drafts(workspace);
        }
        self.keep_slot_edits(workspace, pass != Pass::Files);
        self.take_asks(workspace);
        if !self.open() {
            return true;
        }
        if self.scanning && pass != Pass::Last {
            return false;
        }
        let mut ops = browser.folders.take_ops();
        // ⚠️ A change that touches a rename still to answer waits for it, with every change
        // after it: were the rename refused, the change would act on whatever else has that
        // name on disk.
        let held = ops.iter().position(|op| {
            ends(op)
                .into_iter()
                .any(|path| unsettled(&self.moving, path))
        });
        let held = held.map(|at| ops.split_off(at)).unwrap_or_default();
        // The files those changes move wait with them.
        let held_moves: Vec<(LibPath, LibPath)> = held
            .iter()
            .map(|op| {
                let [from, to] = ends(op);
                (from.clone(), to.clone())
            })
            .collect();
        let holds = !held.is_empty();
        browser.folders.hold(held);
        let (removals, ops): (Vec<Op>, Vec<Op>) = ops
            .into_iter()
            .partition(|op| matches!(op, Op::RemoveDir(_)));
        // The folder changes are this pass's gesture's, as its asks are.
        self.batch.take = self.passes;
        for op in ops {
            self.tree_op(op);
        }
        crate::folders::place_new(workspace, &browser.folders);
        self.take_asks(workspace);
        let mut done = self.arrivals(workspace, &self.waiting(&held_moves));
        done &= self.carry_out(workspace, &held_moves);
        self.send_edits(workspace, &self.waiting(&held_moves));
        for op in removals {
            self.tree_op(op);
        }
        self.send_batch();
        self.passes += 1;
        if pass == Pass::Full {
            self.fetch_tracked(workspace, browser);
        }
        done && !holds
    }

    /// Read and decode in the background each unread file the index tracks (a tag, a
    /// working copy, the slot it came off), so that it matches its slot on the
    /// instrument without anything showing it, and its CRC is known. Once the listing is
    /// complete, a few files at a time, each within the room left: such a read lets go
    /// only of what the index does not track, and one refused for room is tried again at
    /// the next full pass. Files the index does not track are read once something needs
    /// them.
    fn fetch_tracked(&mut self, workspace: &mut Workspace, browser: &Browser) {
        if !self.opened() || !self.fetching.is_empty() {
            return;
        }
        if self.unfetched.is_empty() {
            let wanted: Vec<u64> = self
                .records
                .iter()
                .filter(|(id, record)| {
                    let waits = workspace
                        .get(**id)
                        .is_some_and(|entity| entity.unread() && entity.reading());
                    record.holds == Holds::Unread
                        && waits
                        && tracked(**id, workspace, browser)
                        && !self.fetched.contains(id)
                })
                .map(|(id, _)| *id)
                .collect();
            self.fetched.extend(&wanted);
            self.unfetched.extend(wanted);
        }
        self.next_fetches(workspace);
    }

    /// Give each fingerprint taken without its CRC the CRC of the file's contents, once
    /// they have been read through while the asset still holds what the file does.
    fn learn_crcs(&mut self, workspace: &Workspace) {
        for (id, record) in &mut self.records {
            let Some(entity) = workspace.get(*id) else {
                continue;
            };
            let Some(print) = record
                .fingerprint
                .as_mut()
                .filter(|print| print.crc.is_none())
            else {
                continue;
            };
            if !record.saving && entity.saved.stamp() == record.saved {
                print.crc = entity.saved.whole_crc();
            }
        }
    }

    /// Ask for the next few tracked files waiting to be read in the background.
    fn next_fetches(&mut self, workspace: &mut Workspace) {
        let needed = self.reading || workspace.wants();
        if !self.fetching.is_empty() || !self.opened() || needed {
            return;
        }
        let room = self.room(workspace);
        let (most, most_bytes) = FETCHES;
        let mut files = Vec::new();
        let mut bytes = 0;
        while files.len() < most && bytes < most_bytes {
            let Some(id) = self.unfetched.pop_front() else {
                break;
            };
            let unread = self
                .records
                .get(&id)
                .filter(|record| record.holds == Holds::Unread);
            let Some((path, print)) =
                unread.and_then(|record| Some((record.path.clone()?, record.fingerprint)))
            else {
                continue;
            };
            let Some(len) = workspace.get(id).map(LocalEntity::size) else {
                continue;
            };
            if !workspace.ask_behind(id) {
                continue;
            }
            bytes += len;
            self.fetching.insert(id);
            files.push((id, path, print));
        }
        if files.is_empty() {
            return;
        }
        self.send(Cmd::Read { files, room });
    }

    /// Send one change to the folders. One made or removed while the open's listing is in
    /// flight is one the listing's end keeps or drops with the rest.
    fn tree_op(&mut self, op: Op) {
        match op {
            Op::MakeDir(path) => self.part(Intent::MakeDir { path }),
            Op::RemoveDir(path) => self.part(Intent::RemoveDir { path }),
            Op::MoveDir { from, to } => {
                for record in self.records.values_mut() {
                    if let Some(moved) = record.path.as_ref().and_then(|at| at.moved(&from, &to)) {
                        record.path = Some(moved);
                    }
                }
                self.rename_folder(from, to);
            }
        }
    }

    /// Send the copy each arriving asset waits on. Returns `false` where one waits for a
    /// save or a rename to answer first.
    fn arrivals(&mut self, workspace: &Workspace, waiting: &[(LibPath, LibPath)]) -> bool {
        let mut done = true;
        let arriving: Vec<(u64, CopyOf)> = workspace
            .arrivals()
            .map(|(id, from)| (id, from.clone()))
            .collect();
        for (id, from) in arriving {
            let Some(entity) = workspace.get(id) else {
                continue;
            };
            let Some(path) = entity.path.clone().filter(|_| entity.kept) else {
                continue;
            };
            let record = self.records.get(&id);
            let at = record.and_then(|record| record.path.as_ref());
            let held = [Some(&path), at].into_iter().flatten();
            if held.into_iter().any(|at| unsettled(waiting, at)) {
                done = false;
                continue;
            }
            let sent = self.import(entity, &path, &from, waiting);
            if sent {
                self.delete_with(id, workspace);
            }
            done &= sent;
        }
        done
    }

    /// The renames not answered yet, and the folder changes `held` back, each from and
    /// to: what a change to a file under either waits for.
    fn waiting(&self, held: &[(LibPath, LibPath)]) -> Vec<(LibPath, LibPath)> {
        self.moving.iter().chain(held).cloned().collect()
    }

    /// Carry out the changes to files asked for, in order. One that waits for an answer
    /// holds back every one after it. Returns whether none waits.
    fn carry_out(&mut self, workspace: &mut Workspace, held: &[(LibPath, LibPath)]) -> bool {
        let mut carried = true;
        while let Some((ask, take)) = self.asks.front().cloned() {
            if self.batch.take != take {
                self.send_batch();
                self.batch.take = take;
            }
            // ⚠️ Taken afresh for each: a rename sent just before must answer before a
            // change to what it moves goes out.
            let waiting = self.waiting(held);
            carried = match ask {
                FileAsk::Save(id) => self.save(id, workspace, &waiting),
                FileAsk::Move { id, .. } => self.relocate(id, workspace, &waiting),
                FileAsk::Delete { id, path } => self.delete(id, &path, workspace, &waiting),
            };
            if !carried {
                break;
            }
            self.asks.pop_front();
        }
        carried
    }

    /// Take the changes to files the workspace asked for.
    fn take_asks(&mut self, workspace: &mut Workspace) {
        let asks = workspace.take_file_asks();
        if asks.is_empty() {
            return;
        }
        let take = self.passes;
        self.asks.extend(asks.into_iter().map(|ask| (ask, take)));
    }

    /// Send the changes gathered from one gesture as one change, with a tag waiting
    /// for none but the new files it saves.
    fn send_batch(&mut self) {
        let mut parts = std::mem::take(&mut self.batch.parts);
        self.absorb_tag(&mut parts);
        match parts.len() {
            0 => {}
            1 => self.write(parts.remove(0)),
            _ => self.write(Intent::Batch(parts)),
        }
    }

    /// Add a change to the one being gathered: a rename or a deletion to one of its
    /// kind just before it.
    fn part(&mut self, intent: Intent) {
        match (self.batch.parts.last_mut(), intent) {
            (Some(Intent::Move { moves }), Intent::Move { moves: more }) => moves.extend(more),
            (Some(Intent::Delete { files }), Intent::Delete { files: more }) => files.extend(more),
            (_, intent) => self.batch.parts.push(intent),
        }
    }

    /// Take into `parts` the next tag asked for, where every asset it names is one
    /// `parts` saves as a new file: the files are made wearing it.
    fn absorb_tag(&mut self, parts: &mut Vec<Intent>) {
        let made: BTreeMap<u64, LibPath> = parts
            .iter()
            .filter_map(|part| match part {
                Intent::Save {
                    id,
                    path,
                    expect: toshokan::Expect::Absent,
                    ..
                }
                | Intent::Import {
                    id,
                    path,
                    expect: toshokan::Expect::Absent,
                    ..
                } => Some((*id, path.clone())),
                _ => None,
            })
            .collect();
        let Some(Asked::Tag { assets, name, make }) = self.asked.first() else {
            return;
        };
        let Some(files) = assets
            .iter()
            .map(|id| made.get(id).map(|path| Key::File(path.clone())))
            .collect::<Option<Vec<Key>>>()
        else {
            return;
        };
        let (name, make) = (name.clone(), *make);
        self.asked.remove(0);
        parts.push(Intent::Tag { files, name, make });
    }

    /// Write an asset's file with what it was saved as: a new file where it has none, and
    /// nothing where the file holds it already. `false` while a write over the file waits
    /// for the one before it.
    fn save(&mut self, id: u64, workspace: &Workspace, waiting: &[(LibPath, LibPath)]) -> bool {
        let Some(entity) = workspace.get(id).filter(|entity| entity.kept) else {
            return true;
        };
        // An arriving asset's file is its copy's, and one with no file yet is asked for
        // again once it is placed.
        let Some(path) = entity
            .path
            .clone()
            .filter(|_| workspace.arriving(id).is_none())
        else {
            return true;
        };
        let stamp = entity.saved.stamp();
        let bytes = || entity.saved.bytes().to_vec();
        let Some(record) = self.records.get_mut(&id) else {
            self.records.insert(
                id,
                Record {
                    saved: stamp,
                    saving: true,
                    ..Record::of_file(path.clone(), None)
                },
            );
            self.part(Intent::Save {
                id,
                path,
                bytes: bytes(),
                expect: toshokan::Expect::Absent,
                came: Came::of(&entity.origin),
            });
            return true;
        };
        if record.saving {
            return false;
        }
        let at = record.path.clone().unwrap_or(path);
        if unsettled(waiting, &at) {
            return false;
        }
        // A baseline resting in its file is what the file holds, and needs no write.
        if entity.saved.file().is_some() {
            record.saved = stamp;
            return true;
        }
        if record.saved == stamp && !record.missing {
            return true;
        }
        let expect = match (record.missing, record.fingerprint) {
            (false, Some(print)) => print.expect(),
            _ => toshokan::Expect::Absent,
        };
        record.saved = stamp;
        record.saving = true;
        record.path = Some(at.clone());
        self.part(Intent::Save {
            id,
            path: at,
            bytes: bytes(),
            expect,
            came: None,
        });
        self.delete_with(id, workspace);
        true
    }

    /// Delete, in the change that saves or copies over the file of `id`, the file of
    /// the asset the overwrite came from, which goes once that change lands.
    fn delete_with(&mut self, id: u64, workspace: &Workspace) {
        let Some(from) = workspace.overwritten_from(id) else {
            return;
        };
        let Some(record) = self.records.get(&from).filter(|record| !record.saving) else {
            return;
        };
        let (Some(path), Some(print), false) =
            (record.path.clone(), record.fingerprint, record.missing)
        else {
            return;
        };
        self.part(Intent::Delete {
            files: vec![(path, print.expect())],
        });
        self.deleting_with.insert(id, from);
    }

    /// The overwrite of `id` answered: the asset it came from, whose file it deleted,
    /// has no file now where it landed.
    fn deleted_with(&mut self, id: u64, landed: bool) {
        let Some(from) = self.deleting_with.remove(&id).filter(|_| landed) else {
            return;
        };
        self.ids.remove(from);
        self.records.remove(&from);
    }

    /// Move an asset's file to where the asset is now. `false` while a write over the file
    /// or a rename it is under waits for its answer.
    fn relocate(&mut self, id: u64, workspace: &Workspace, waiting: &[(LibPath, LibPath)]) -> bool {
        let to = workspace.get(id).and_then(|entity| entity.path.clone());
        let (Some(to), Some(record)) = (to, self.records.get_mut(&id)) else {
            return true;
        };
        if record.saving {
            return false;
        }
        let Some(at) = record.path.clone().filter(|at| *at != to) else {
            return true;
        };
        if unsettled(waiting, &at) || unsettled(waiting, &to) {
            return false;
        }
        record.path = Some(to.clone());
        let expect = record.fingerprint.map(|print| print.expect());
        let (Some(expect), false) = (expect, record.missing) else {
            return true;
        };
        self.ids.moved(&at, &to);
        self.moving.push((at.clone(), to.clone()));
        self.part(Intent::Move {
            moves: vec![Moving {
                from: at,
                to,
                expect,
            }],
        });
        true
    }

    /// Move the file of an asset that is gone to this writer's trash, once no copy waits
    /// to be made of it. An asset never written, or one whose file is missing, has
    /// nothing to trash.
    fn delete(
        &mut self,
        id: u64,
        path: &LibPath,
        workspace: &Workspace,
        waiting: &[(LibPath, LibPath)],
    ) -> bool {
        if workspace.get(id).is_some() {
            return true;
        }
        let Some(record) = self.records.get(&id) else {
            return true;
        };
        // ⚠️ A file a copy is still to be made of waits until the copy answers: a copy
        // that fails has nothing else to make it from.
        let at = record.path.as_ref().unwrap_or(path);
        if record.saving || workspace.copies_of(id).next().is_some() || unsettled(waiting, at) {
            return false;
        }
        let Some(record) = self.records.remove(&id) else {
            return true;
        };
        self.ids.remove(id);
        let Some(at) = record.path else {
            return true;
        };
        if let (Some(print), false) = (record.fingerprint, record.missing) {
            self.part(Intent::Delete {
                files: vec![(at, print.expect())],
            });
        }
        true
    }

    /// Send the copy an arriving asset waits on: a new file at its path, or a copy over
    /// the file it has, unless a copy is in flight already. Returns `false` where the
    /// file it copies has a save or a rename to answer first, and the copy waits.
    fn import(
        &mut self,
        entity: &LocalEntity,
        path: &LibPath,
        from: &CopyOf,
        waiting: &[(LibPath, LibPath)],
    ) -> bool {
        let from = match from {
            CopyOf::Outside(file) => Source::Outside(file.clone()),
            CopyOf::Part(part) => Source::Part(part.clone()),
            CopyOf::Asset(source) | CopyOf::Edited(source, _) => {
                let source = self.records.get(source).filter(|source| !source.saving);
                let found =
                    source.and_then(|source| Some((source.path.clone()?, source.fingerprint?)));
                match (found, from) {
                    (Some((at, _)), _) if unsettled(waiting, &at) => return false,
                    (Some((at, print)), CopyOf::Edited(_, pieces)) => {
                        Source::Edited(at, print, pieces.clone())
                    }
                    (Some((at, print)), _) => Source::Library(at, print),
                    (None, _) => return false,
                }
            }
        };
        let record = self
            .records
            .entry(entity.id)
            .or_insert_with(|| Record::of_file(path.clone(), None));
        if record.saving {
            return true;
        }
        record.saving = true;
        record.saved = entity.saved.stamp();
        let path = record.path.get_or_insert_with(|| path.clone()).clone();
        let expect = match (record.missing, record.fingerprint) {
            (false, Some(print)) => print.expect(),
            _ => toshokan::Expect::Absent,
        };
        let came = Came::of(&entity.origin).filter(|_| record.fingerprint.is_none());
        self.part(Intent::Import {
            id: entity.id,
            path,
            from,
            expect,
            came,
        });
        true
    }
}

/// What the Edit menu calls settling another drawbar's unfinished change as `how`.
fn settling(how: toshokan::log::Settlement) -> &'static str {
    match how {
        toshokan::log::Settlement::Finished => "Finish",
        toshokan::log::Settlement::RolledBack => "Roll back",
        toshokan::log::Settlement::Dismissed => "Leave as is",
    }
}

/// One answer of the library this session's undo history follows.
#[derive(Clone, PartialEq, Debug)]
enum Step {
    /// A change, as the Edit menu names it.
    Change(String),
    Undo,
    Redo,
}

/// This session's changes the library made: those an undo takes back, last last, and
/// those a redo makes again, each as the Edit menu names it, with the entry the undo
/// or the redo reverses.
#[derive(Default)]
struct History {
    done: Vec<(String, toshokan::EntryHash)>,
    undone: Vec<(String, toshokan::EntryHash)>,
    /// The step that failed partway and waits for the next write to finish it.
    waiting: Option<Step>,
}

impl History {
    /// Follow what became of the step that waited, which the change `step` finished
    /// first where `finished` logged it, and which step waits now.
    fn waited(
        &mut self,
        finished: Option<toshokan::EntryHash>,
        waiting: Waiting,
        step: Option<&Step>,
    ) {
        let earlier = self.waiting.take();
        if let (Some(entry), Some(earlier)) = (finished, earlier.clone()) {
            self.took(earlier, entry);
        }
        self.waiting = match waiting {
            Waiting::Nothing => None,
            Waiting::Earlier => earlier,
            Waiting::This => step.cloned(),
        };
    }

    /// Follow a step the library made, which `entry` logged.
    fn took(&mut self, step: Step, entry: toshokan::EntryHash) {
        match step {
            Step::Change(change) => {
                self.done.push((change, entry));
                self.undone.clear();
            }
            Step::Undo => {
                if let Some((change, _)) = self.done.pop() {
                    self.undone.push((change, entry));
                }
            }
            Step::Redo => {
                if let Some((change, _)) = self.undone.pop() {
                    self.done.push((change, entry));
                }
            }
        }
    }

    /// Let go of the step a refused undo or redo `step` would have reversed, and say
    /// what can no longer be done with it.
    fn forget(&mut self, step: &Step) -> Option<String> {
        let (steps, cannot) = match step {
            Step::Undo => (&mut self.done, "cannot be undone"),
            Step::Redo => (&mut self.undone, "cannot be made again"),
            Step::Change(_) => return None,
        };
        let (change, _) = steps.pop()?;
        Some(format!("{change} {cannot}"))
    }

    /// Let go of the steps whose undo or redo needs what `entries` displaced, and of
    /// every step behind them. Returns the latest change Undo no longer reaches.
    fn trim(&mut self, entries: &[toshokan::EntryHash]) -> Option<String> {
        let cut = |steps: &mut Vec<(String, toshokan::EntryHash)>| {
            let at = steps
                .iter()
                .rposition(|(_, entry)| entries.contains(entry))?;
            let kept = steps.split_off(at + 1);
            let gone = std::mem::replace(steps, kept);
            gone.last().map(|(change, _)| change.clone())
        };
        cut(&mut self.undone);
        cut(&mut self.done)
    }
}

impl Record {
    fn of_file(path: LibPath, fingerprint: impl Into<Option<Fingerprint>>) -> Record {
        Record {
            path: Some(path),
            fingerprint: fingerprint.into(),
            saved: 0,
            saving: false,
            missing: false,
            holds: Holds::Whole,
        }
    }

    /// Whether drawbar holds its file whole as last read or written, under the baseline
    /// stamped `saved`, with no save or rename over it: what it holds can be read again
    /// from the file.
    fn rereadable(&self, saved: u64, moving: &[(LibPath, LibPath)]) -> bool {
        let still = self.saved == saved && self.fingerprint.is_some();
        let settled = self
            .path
            .as_ref()
            .is_some_and(|path| !unsettled(moving, path));
        let idle = !self.saving && !self.missing;
        self.holds == Holds::Whole && still && settled && idle
    }

    /// Recorded from what a listing found.
    fn of_found(found: &Found, fingerprint: Fingerprint) -> Record {
        Record {
            holds: found.holds(),
            ..Record::of_file(found.path.clone(), fingerprint)
        }
    }
}

/// Let the asset a copy or an overwrite came from go, onto `left`, unless it was edited
/// since: nothing that landed holds that edit.
fn leave(left: &mut Vec<u64>, leaving: Leaving, workspace: &Workspace, log: &mut Log) {
    if workspace.unchanged(&leaving) {
        return left.push(leaving.from);
    }
    if let Some(entity) = workspace.get(leaving.from) {
        log.say(format!(
            "“{}” was edited while it was moved, so it stays, with its edit.",
            entity.name
        ));
    }
}

/// The paths a change to the folders acts on, where it was and where it goes.
fn ends(op: &Op) -> [&LibPath; 2] {
    match op {
        Op::MakeDir(path) | Op::RemoveDir(path) => [path, path],
        Op::MoveDir { from, to } => [from, to],
    }
}

/// Which clean assets [`Store::make_room`] may let go of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Evict {
    Any,
    /// Only those the library does not track, for a read in the background.
    Untracked,
}

/// Whether drawbar knows something about this asset that no file says: a tag, or the
/// slot it came off.
fn tracked(id: u64, workspace: &Workspace, browser: &Browser) -> bool {
    let slot = workspace
        .get(id)
        .is_some_and(|entity| entity.origin.slot().is_some());
    slot || !browser.tags.worn(id).is_empty()
}

/// How many bytes the assets hold whole, counting each read in flight at its file's listed
/// length.
fn held(workspace: &Workspace) -> u64 {
    workspace
        .held_whole()
        .saturating_add(workspace.asked_whole())
}

/// Whether `path` is in, is, or holds either side of one of these renames.
fn unsettled(moving: &[(LibPath, LibPath)], path: &LibPath) -> bool {
    moving
        .iter()
        .any(|(from, to)| path.is_in(from) || from.is_in(path) || path.is_in(to) || to.is_in(path))
}

/// Flag the entries of one folder whose names are one name under [`names::key`], and
/// warn once about each new pair. Nothing is renamed: renaming a user's files on open
/// would be hostile, and either name may be the one other files refer to.
pub(crate) fn flag_duplicates(workspace: &Workspace, browser: &mut Browser, log: &mut Log) {
    let (flagged, groups) = duplicates(workspace, &browser.folders);
    for (dir, group) in groups {
        let ids: Vec<u64> = group.iter().filter_map(|(id, _)| *id).collect();
        if !ids.is_empty() && ids.iter().all(|id| browser.folders.duplicates.contains(id)) {
            continue;
        }
        let names: Vec<String> = group.iter().map(|(_, name)| format!("“{name}”")).collect();
        let place = match dir.is_root() {
            true => "the library".to_string(),
            false => dir.to_string(),
        };
        log.warn(format!(
            "{} in {place} differ only by case and would collide on macOS or Windows; rename \
             one of them.",
            names.join(" and ")
        ));
    }
    browser.folders.duplicates = flagged;
}

/// One entry of a folder, as [`duplicates`] compares it.
struct Entry {
    /// An asset's id, or `None` for a folder or a file drawbar does not hold.
    id: Option<u64>,
    name: String,
    /// A file drawbar does not hold.
    stranger: bool,
}

/// Entries sharing a name: each asset's id, or `None` for a folder or a file drawbar does
/// not hold, and its name.
type Sharing = Vec<(Option<u64>, String)>;

/// One folder's entries that share a name.
type Group = (LibPath, Sharing);

/// The assets sharing a name with another entry of their folder, and every such group
/// with its folder. A folder in a group has no id there.
pub(crate) fn duplicates(
    workspace: &Workspace,
    folders: &crate::folders::Folders,
) -> (BTreeSet<u64>, Vec<Group>) {
    let mut by_name: BTreeMap<(LibPath, String), Vec<Entry>> = BTreeMap::new();
    let assets = workspace
        .listed()
        .filter_map(|entity| Some((Some(entity.id), entity.path.as_ref()?, false)));
    let dirs = folders
        .all()
        .iter()
        .map(|folder| (None, &folder.path, false));
    let strangers = folders.strangers().map(|path| (None, path, true));
    for (id, path, stranger) in assets.chain(dirs).chain(strangers) {
        by_name
            .entry((path.parent(), names::key(path.leaf())))
            .or_default()
            .push(Entry {
                id,
                name: path.leaf().to_string(),
                stranger,
            });
    }
    // A group of files drawbar does not hold is none of its business.
    let groups: Vec<Group> = by_name
        .into_iter()
        .filter(|(_, group)| group.len() > 1 && group.iter().any(|entry| !entry.stranger))
        .map(|((dir, _), group)| {
            let group = group
                .into_iter()
                .map(|entry| (entry.id, entry.name))
                .collect();
            (dir, group)
        })
        .collect();
    let flagged = groups
        .iter()
        .flat_map(|(_, group)| group.iter().filter_map(|(id, _)| *id))
        .collect();
    (flagged, groups)
}

/// What became of an asset whose file a rescan found gone.
enum Vanished {
    Deleted,
    /// Kept, since it holds something its file did not, and shown missing.
    Missing,
    /// Already known to be missing, or no longer held.
    Untouched,
}

/// What a rescan found changed outside drawbar, in one line: `None` where nothing changed,
/// or where the one change was a delete or a move that its own line has said.
fn changed_outside(appeared: usize, deleted: usize, moved: usize) -> Option<String> {
    let changes = [
        (appeared, "appeared", "appeared"),
        (deleted, "was deleted", "were deleted"),
        (moved, "was moved", "were moved"),
    ];
    let said: Vec<String> = changes
        .into_iter()
        .filter(|(n, ..)| *n > 0)
        .enumerate()
        .map(|(at, (n, one, many))| match at {
            0 => crate::strings::counted(n, &format!("file {one}"), &format!("files {many}")),
            _ => crate::strings::counted(n, one, many),
        })
        .collect();
    let said = crate::strings::listed(&said);
    match (appeared, deleted + moved) {
        (0, 0 | 1) => None,
        (_, 0) => Some(format!("{said} in the library folder.")),
        _ => Some(format!("Outside drawbar, {said}.")),
    }
}

/// A file drawbar did not know, recorded under `id`, and the asset it becomes.
fn newcomer(id: u64, found: Found, records: &mut BTreeMap<u64, Record>) -> Saved {
    records.insert(id, Record::of_found(&found, found.fingerprint()));
    Saved {
        id,
        name: found.path.leaf().to_string(),
        origin: Origin::File(found.path.leaf().to_string()),
        content: Content::found(found.bytes, found.file, found.stat.len),
        path: Some(found.path),
        unsaved: None,
    }
}

/// Ask what to do about a file changed on disk under an unsaved edit.
fn conflict(id: u64, workspace: &Workspace, browser: &mut Browser, log: &mut Log) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    log.warn(format!(
        "{}: changed on disk while drawbar held an unsaved edit",
        entity.name
    ));
    if let Some(why) = workspace.unapplied(id) {
        log.warn(format!(
            "{}: the edit does not apply to the file as it is now: {why}",
            entity.name
        ));
    }
    browser.ask_conflict(id, &entity.name);
}

fn slot_conflict(id: u64, workspace: &Workspace, browser: &mut Browser, log: &mut Log) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    log.warn(format!(
        "{}: the slot changed since drawbar kept an unsaved edit of its view",
        entity.name
    ));
    browser.ask_slot_conflict(id, &entity.name);
}

/// Delete a slot's former occupant.
#[cfg(not(target_arch = "wasm32"))]
fn discard_rescue(rescue: Rescue, log: &mut Log) {
    let name = rescue.name;
    match std::fs::remove_file(&rescue.at) {
        Ok(()) => log.say(format!("Discarded “{name}”.")),
        Err(e) => log.trouble(format!("“{name}” could not be deleted: {e}.")),
    }
}

/// Delete a slot's former occupant.
#[cfg(target_arch = "wasm32")]
fn discard_rescue(rescue: Rescue, log: &mut Log) {
    super::remove_private(rescue.at);
    log.say(format!("Discarded “{}”.", rescue.name));
}

/// The URL that shows the folder a rescue is in.
#[cfg(not(target_arch = "wasm32"))]
fn rescue_url(rescue: &Rescue) -> Option<String> {
    let dir = rescue.at.parent()?;
    url::Url::from_directory_path(dir).ok().map(String::from)
}

#[cfg(target_arch = "wasm32")]
fn rescue_url(_: &Rescue) -> Option<String> {
    None
}

/// Who made what [`super::KeptApart`] keeps apart, and when, counting the `rows` Kept
/// apart lists of it.
fn kept_title(kept: &super::KeptApart, rows: usize) -> String {
    use toshokan::report::By;
    let who = match &kept.by {
        By::Writer { label, .. } if !label.is_empty() && label != super::keeper::LABEL => {
            label.clone()
        }
        By::Writer { .. } => "Another drawbar".to_string(),
        By::This | By::Outside => "This drawbar".to_string(),
    };
    let when = crate::when::span(kept.span.0, kept.span.1, crate::when::local_offset);
    let made = match rows {
        0 => "changes".to_string(),
        n => crate::strings::counted(n, "change", "changes"),
    };
    let what = match kept.kind {
        super::KeptKind::Run => "on a branch of its own",
        super::KeptKind::Gap => "after ones that have not synced yet",
        super::KeptKind::Unplaced => "in a version a sync dropped",
    };
    format!("{who} made {made} {what} {when}")
}

/// What the window holds of an asset a change kept apart names.
struct Held {
    id: u64,
    name: String,
    path: Option<LibPath>,
}

/// The rows Kept apart lists of `changes`, as `held` says the window holds each asset:
/// one for each change the user would tell apart. A tag's entry in the library's list
/// of tags goes with the row that puts the tag on an asset, which takes it too, the
/// list's own making goes with its first entry, and a file change that names where the
/// asset's file is already says nothing.
fn kept_rows(
    changes: &[super::KeptChange],
    held: impl Fn(toshokan::EntityId) -> Option<Held>,
) -> Vec<crate::folders::KeptRow> {
    use super::KeptDo;
    let catalog = What::Field(TAG_NAMES.name().to_string());
    let worn = What::Field(TAGS.name().to_string());
    let gained = |tag: &str| {
        changes.iter().any(|change| {
            change.what == worn && matches!(&change.takes, Some(KeptDo::Gain(gain)) if gain == tag)
        })
    };
    let lists: BTreeSet<toshokan::EntityId> = changes
        .iter()
        .filter(|change| change.what == catalog)
        .map(|change| change.entity)
        .collect();
    let rows = changes.iter().filter_map(|change| {
        let asset = held(change.entity);
        let said = match (&change.what, &change.takes) {
            (What::Created, _) if lists.contains(&change.entity) => return None,
            (what, Some(KeptDo::Gain(tag))) if *what == catalog && gained(tag) => return None,
            (what, Some(KeptDo::Gain(tag))) if *what == catalog => format!("Made the tag “{tag}”"),
            (what, Some(KeptDo::Lose(tag))) if *what == catalog => {
                format!("Removed the tag “{tag}”")
            }
            (what, _) if *what == catalog => "Changed the library's list of tags".to_string(),
            (_, Some(KeptDo::File(at))) => {
                let now = asset.as_ref().and_then(|asset| asset.path.as_ref());
                if now.is_some_and(|now| now.as_str() == at) {
                    return None;
                }
                let file = at.rsplit_once('/').map_or(at.as_str(), |(_, file)| file);
                let name = shown_name(asset.as_ref().map_or(file, |asset| &asset.name));
                match now {
                    Some(_) => format!("“{name}” moves to “{}”", shown_path(at)),
                    None => format!("“{name}” is at “{}”", shown_path(at)),
                }
            }
            (what, does) => {
                let name = asset.as_ref().map(|asset| shown_name(&asset.name));
                kept_said(name.as_deref(), what, does)
            }
        };
        Some(crate::folders::KeptRow {
            key: (change.entity, change.what.clone()),
            id: asset.map(|asset| asset.id),
            said,
            takes: change.takes.is_some(),
            forgotten: change.forgotten,
        })
    });
    rows.collect()
}

/// Let go of each keep of the view `id` in flight, so its answer marks nothing kept,
/// and say whether one was not let go of before.
fn let_go_of_keeps(keeping: &mut VecDeque<(u64, Option<(u64, blake3::Hash)>)>, id: u64) -> bool {
    let sent = keeping.iter_mut().filter(|sent| sent.0 == id);
    sent.filter_map(|sent| sent.1.take()).count() > 0
}

/// A name as the library list shows it.
fn shown_name(name: &str) -> String {
    crate::strings::display_name(name).to_string()
}

/// A library path as the library list names its file: its folders, then its name.
fn shown_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/{}", crate::strings::display_name(name)),
        None => shown_name(path),
    }
}

/// What one change to an asset kept apart does, to the asset named `name` where the
/// window holds it.
fn kept_said(name: Option<&str>, what: &What, does: &Option<super::KeptDo>) -> String {
    use super::KeptDo;
    let asset = name.map_or("An asset the library does not show".to_string(), |name| {
        format!("“{name}”")
    });
    let field = match what {
        What::Field(key) if key == TAGS.name() => "tag",
        What::Field(key) => key.as_str(),
        What::Created | What::Deleted | What::File => "",
    };
    match (what, does) {
        (What::Created, _) | (_, Some(KeptDo::Revive)) => format!("{asset} is made"),
        (What::Deleted, _) | (_, Some(KeptDo::Delete)) => format!("{asset} is removed"),
        (What::File, _) | (_, Some(KeptDo::File(_))) => {
            format!("{asset} is left without a file")
        }
        (_, Some(KeptDo::Gain(value))) => format!("{asset} gains the {field} “{value}”"),
        (_, Some(KeptDo::Lose(value))) => format!("{asset} loses the {field} “{value}”"),
        (_, Some(KeptDo::Set(value))) => format!("{asset}: its {field} becomes “{value}”"),
        (_, Some(KeptDo::Clear)) => format!("{asset}: its {field} is cleared"),
        (What::Field(_), None) => format!("{asset}: its {field} changes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u128) -> toshokan::EntryHash {
        toshokan::EntryHash::from_u128(n)
    }

    /// A step whose undo needs what the trash let go goes, with every step behind it,
    /// and the Edit menu names the latest change it no longer reaches.
    fn change(
        entity: u128,
        what: &str,
        takes: Option<super::super::KeptDo>,
    ) -> super::super::KeptChange {
        let what = match what {
            "file" => What::File,
            key => What::Field(key.to_string()),
        };
        super::super::KeptChange {
            entity: toshokan::EntityId::from_u128(entity),
            what,
            takes,
            forgotten: false,
        }
    }

    /// Each change kept apart, as Kept apart says it: assets by the names the library
    /// shows, a tag's catalog entry only where no asset in the run takes it, and a file
    /// only where it is not already where the asset's file is.
    #[test]
    fn each_change_kept_apart_is_said_as_the_library_names_it() {
        use super::super::KeptDo::{File, Gain, Lose};
        let at = |path: &str| LibPath::parse(path);
        let held = |entity: toshokan::EntityId| {
            let (name, path) = match entity.to_u128() {
                1 => ("Grand.ne5p", at("Grand.ne5p")),
                2 => ("Warm pad.ne5p", at("Warm pad.ne5p")),
                3 => ("Pad.ne5p", at("Pad.ne5p")),
                4 => ("Bass.ne5p", None),
                _ => return None,
            };
            let id = u64::try_from(entity.to_u128()).unwrap();
            Some(Held {
                id,
                name: name.to_string(),
                path,
            })
        };
        let catalog = TAG_NAMES.name();
        let cases = [
            (
                change(1, "tags", Some(Gain("Sunday".into()))),
                Some("“Grand” gains the tag “Sunday”"),
            ),
            (change(9, catalog, Some(Gain("Sunday".into()))), None),
            (
                change(9, catalog, Some(Gain("Spare".into()))),
                Some("Made the tag “Spare”"),
            ),
            (
                change(9, catalog, Some(Lose("Old".into()))),
                Some("Removed the tag “Old”"),
            ),
            (
                change(2, "file", Some(File("Pads/Warm pad.ne5p".into()))),
                Some("“Warm pad” moves to “Pads/Warm pad”"),
            ),
            (change(3, "file", Some(File("Pad.ne5p".into()))), None),
            (
                change(4, "file", Some(File("Bass.ne5p".into()))),
                Some("“Bass” is at “Bass”"),
            ),
            (
                change(5, "file", Some(File("Sets/Organ.ne5p".into()))),
                Some("“Organ” is at “Sets/Organ”"),
            ),
            (
                change(1, "file", None),
                Some("“Grand” is left without a file"),
            ),
            (
                change(6, "tags", Some(Gain("Live".into()))),
                Some("An asset the library does not show gains the tag “Live”"),
            ),
        ];
        let changes: Vec<_> = cases.iter().map(|(change, _)| change.clone()).collect();
        let said: Vec<String> = kept_rows(&changes, held)
            .into_iter()
            .map(|row| row.said)
            .collect();
        let expected: Vec<&str> = cases.iter().filter_map(|(_, said)| *said).collect();
        assert_eq!(said, expected);
    }

    /// A card's title names the other drawbar by its label where it has one of its own,
    /// and counts the rows the card lists.
    #[test]
    fn a_card_title_names_its_writer_and_counts_its_rows() {
        use toshokan::report::By;
        let writer = toshokan::WriterId::from_u128(7);
        let run = toshokan::log::Run {
            writer,
            from: 1,
            first: None,
            last: toshokan::log::Own {
                n: 1,
                hash: toshokan::EntryHash::from_u128(1),
            },
            tip: None,
        };
        let kept = |label: &str| super::super::KeptApart {
            kind: super::super::KeptKind::Run,
            run,
            by: By::Writer {
                writer,
                label: label.to_string(),
            },
            span: (0, 0),
            changes: Vec::new(),
        };
        let title = |label, rows| kept_title(&kept(label), rows);
        assert!(title("drawbar", 2).starts_with("Another drawbar made 2 changes on a branch"));
        assert!(title("drawbar on MacBook", 1).starts_with("drawbar on MacBook made 1 change on"));
        assert!(title("drawbar", 0).starts_with("Another drawbar made changes on"));
    }

    #[test]
    fn a_trimmed_step_and_every_one_behind_it_leave_the_history() {
        let mut history = History::default();
        for (n, change) in ["Save “a”", "Save “b”", "Rename “c”"]
            .into_iter()
            .enumerate()
        {
            history.took(Step::Change(change.to_string()), entry(n as u128));
        }
        history.took(Step::Undo, entry(9));
        assert_eq!(history.trim(&[entry(5)]), None, "nothing needed it");
        assert_eq!(
            history.trim(&[entry(1)]).as_deref(),
            Some("Save “b”"),
            "the latest change no longer reached"
        );
        assert!(history.done.is_empty());
        assert_eq!(history.undone.len(), 1, "the redo needs none of it");
    }

    #[test]
    fn an_undo_a_later_write_finishes_is_undone_and_can_be_redone() {
        let mut history = History::default();
        history.took(Step::Change("Save “a”".to_string()), entry(0));
        history.took(Step::Change("Save “b”".to_string()), entry(1));
        history.waited(None, Waiting::This, Some(&Step::Undo));
        history.waited(Some(entry(2)), Waiting::Nothing, None);
        let names = |steps: &[(String, toshokan::EntryHash)]| {
            steps
                .iter()
                .map(|(change, _)| change.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&history.done), ["Save “a”"]);
        assert_eq!(names(&history.undone), ["Save “b”"]);
        assert_eq!(history.undone[0].1, entry(2), "the redo reverses the undo");
    }

    #[test]
    fn a_step_that_waits_stays_while_the_next_change_is_not_tried_and_goes_where_it_never_landed() {
        let mut history = History::default();
        let save = Step::Change("Save “a”".to_string());
        history.waited(None, Waiting::This, Some(&save));
        history.waited(None, Waiting::Earlier, Some(&Step::Undo));
        assert_eq!(history.waiting, Some(save));
        history.waited(None, Waiting::Nothing, None);
        assert_eq!(history.waiting, None);
        assert!(
            history.done.is_empty(),
            "a step that never landed is not undone"
        );
    }
}
