//! The app's side of a library: what the files hold as far as drawbar knows, the
//! commands that bring them level with the workspace, and the answers folded back in.
//!
//! The workspace stays the model the rest of the app edits. [`Store::sync`] compares it
//! with what the files were last known to hold and sends what differs: a new asset's
//! file, a save, a deletion, a working copy, the index. Folder changes arrive in order
//! from [`crate::folders::Folders`], because a rename of a folder cannot be told from a
//! move of everything in it by comparing the two states.

use std::collections::{BTreeMap, BTreeSet};

use super::cache::{self, Cache};
use super::diff::{self, match_files, Known};
use super::exec::{too_much, working_name};
use super::sidecar::{Keeps, Row, Sidecar, Working, VERSION};
use super::{
    names, Backend, Cmd, Complete, CopyOf, Event, Failure, Fingerprint, Found, Holds, LibPath,
    Listing, Opened, Source, MOST_BYTES,
};
use crate::browser::Browser;
use crate::folders::{Folders, Op, Where};
use crate::log::Log;
use crate::queue::Queue;
use crate::rewrite::Edit;
use crate::summary::Summary;
use crate::workspace::{precious, LocalEntity, Origin, Saved, Workspace};

/// How far opening has got.
enum Phase {
    Opening,
    Open,
    /// Opened, and nothing may be written, for this reason.
    ReadOnly(String),
    /// Not opened, for this reason.
    Failed(String),
}

/// How much one [`Store::sync`] writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The files: new ones, saves, moves and deletions.
    Files,
    /// The files, the working copies and the index, on the autosave cadence.
    Full,
    /// A full pass at exit, which runs even while a rescan is in flight, since nothing
    /// will read its answer.
    Last,
}

/// What drawbar knows of one asset's file.
struct Record {
    /// `None` for a view of a slot, kept only as a working copy.
    path: Option<LibPath>,
    /// What the file held when drawbar last read or wrote it. `None` until the first
    /// save of a new file lands.
    fingerprint: Option<Fingerprint>,
    /// The [`crate::workspace::Baseline::stamp`] of the bytes the file holds, or is
    /// being written with.
    saved: u64,
    working: Option<Kept>,
    /// A save has been sent and not answered.
    saving: bool,
    /// The file went missing outside drawbar. Nothing writes it again until the asset
    /// is saved.
    missing: bool,
    /// How drawbar holds what the file holds, while nothing has been saved over it.
    holds: Holds,
    /// The [`crate::workspace::Baseline::stamp`] of the bytes whose summary the cache
    /// holds for the file, so a summary is taken once per set of bytes.
    summarized: Option<u64>,
}

/// An open whose listing is still arriving: the index's rows, as the files listed so far
/// have claimed them.
struct Loading {
    /// Under the ids this session gives them.
    rows: BTreeMap<u64, Row>,
    /// The row naming each path.
    by_path: BTreeMap<LibPath, u64>,
    /// The rows a listed file has claimed.
    claimed: BTreeSet<u64>,
    /// The working copies not yet taken up, by id.
    working: BTreeMap<u64, Vec<u8>>,
    /// The id the next file the index does not name takes, unless the workspace has
    /// given it out since.
    next: u64,
    /// Every folder listed so far.
    dirs: Vec<LibPath>,
    /// How many files have come back so far.
    files: usize,
    /// How many leftovers of interrupted writes the open removed.
    swept: usize,
    /// Each rename sent while the listing is in flight that has not failed. A part the
    /// backend gathered before it ran names what it moved where it was.
    moves: Vec<Rename>,
    /// Answers of the listing gathered before a rename that has not answered yet, in
    /// order. Each waits to know whether the rename moved what it names.
    held: std::collections::VecDeque<Event>,
}

/// A rename sent while the listing is in flight.
struct Rename {
    /// The count of commands sent since the open that it brought to.
    sent: u64,
    from: LibPath,
    to: LibPath,
    /// The rows no file had claimed that it took along.
    rows: Vec<u64>,
    /// It answered that it moved.
    moved: bool,
}

impl Loading {
    /// Bring a path the backend gave after `ran` commands to where it is now.
    ///
    /// ⚠️ Only for a path no rename still to answer may have moved: see
    /// [`Loading::waits`].
    fn now(&self, ran: u64, path: &mut LibPath) {
        for rename in self.moves.iter().filter(|rename| rename.sent > ran) {
            if let Some(moved) = path.moved(&rename.from, &rename.to) {
                *path = moved;
            }
        }
    }

    /// Whether an answer of the listing gathered after `ran` commands waits for a rename
    /// sent after them to answer.
    fn waits(&self, ran: u64) -> bool {
        self.moves
            .iter()
            .any(|rename| !rename.moved && rename.sent > ran)
    }

    /// The rename that answered, as it answered: one that moved stays to bring the
    /// parts gathered before it to where it put them, and one that failed goes.
    fn answered(&mut self, from: &LibPath, to: &LibPath, moved: bool) -> Option<Rename> {
        let at = self
            .moves
            .iter()
            .position(|rename| !rename.moved && rename.from == *from && rename.to == *to)?;
        match moved {
            true => {
                self.moves[at].moved = true;
                None
            }
            false => Some(self.moves.remove(at)),
        }
    }

    /// The next answer held back that no rename still to answer may have moved.
    fn ready(&mut self) -> Option<Event> {
        let ran = gathered(self.held.front()?)?;
        match self.waits(ran) {
            true => None,
            false => self.held.pop_front(),
        }
    }

    /// [`Loading::now`] for every path of a part, and forget the renames every part to
    /// come has seen.
    fn caught_up(&mut self, ran: u64, part: &mut Listing) {
        let Listing {
            dirs,
            files,
            unread,
            others,
            unwalked,
        } = part;
        let found = files.iter_mut().map(|found| &mut found.path);
        let unread = unread.iter_mut().map(|(path, _)| path);
        for path in dirs
            .iter_mut()
            .chain(found)
            .chain(unread)
            .chain(others)
            .chain(unwalked)
        {
            self.now(ran, path);
        }
        self.moves.retain(|rename| rename.sent > ran);
    }

    /// Follow a folder renamed while the listing is in flight: the rows no file has
    /// claimed yet, and the folders listed so far, move with it. Returns the rows it
    /// moved.
    fn relocate(&mut self, from: &LibPath, to: &LibPath) -> Vec<u64> {
        let mut moved = Vec::new();
        for (id, row) in &mut self.rows {
            if let Some(at) = row.path.as_ref().and_then(|at| at.moved(from, to)) {
                row.path = Some(at);
                moved.push(*id);
            }
        }
        self.by_path = by_path(&self.rows);
        for dir in &mut self.dirs {
            if let Some(at) = dir.moved(from, to) {
                *dir = at;
            }
        }
        moved
    }

    /// Put back what a rename that failed took along.
    fn unmoved(&mut self, rename: Rename) {
        let Rename { from, to, rows, .. } = rename;
        for id in rows {
            let Some(row) = self.rows.get_mut(&id) else {
                continue;
            };
            if let Some(at) = row.path.as_ref().and_then(|at| at.moved(&to, &from)) {
                row.path = Some(at);
            }
        }
        self.by_path = by_path(&self.rows);
        for dir in &mut self.dirs {
            if let Some(at) = dir.moved(&to, &from) {
                *dir = at;
            }
        }
    }
}

/// How many commands had run when the backend gathered an answer of the listing, or
/// `None` for any other answer.
fn gathered(event: &Event) -> Option<u64> {
    match event {
        Event::Listed { ran, .. } | Event::Walked { ran, .. } => Some(*ran),
        Event::Complete(complete) => Some(complete.ran),
        _ => None,
    }
}

/// The row naming each path: the first, where the index names one twice.
fn by_path(rows: &BTreeMap<u64, Row>) -> BTreeMap<LibPath, u64> {
    let mut by_path = BTreeMap::new();
    for (id, row) in rows {
        if let Some(path) = &row.path {
            by_path.entry(path.clone()).or_insert(*id);
        }
    }
    by_path
}

/// How many parts of a listing one [`Store::poll`] folds in, so a listing faster than the
/// frames fills the tree over several of them.
const PARTS_A_FRAME: usize = 4;

/// How many files one [`Cmd::Fingerprint`] reads at most, and how many of their bytes.
const PRINTS: (usize, u64) = (64, 64 << 20);

/// How many files one background [`Cmd::Read`] asks for at most, and how many of their
/// listed bytes. A read something needs runs after it.
const FETCHES: (usize, u64) = (32, 16 << 20);

/// An unsaved edit, as written to `working/`.
struct Kept {
    copy: Working,
    /// The stamp of what the copy holds: the [`LocalEntity::stamp`] of the bytes, or
    /// the stamp of the edit (see [`Workspace::kept_edit`]).
    stamp: u64,
}

pub struct Store {
    backend: Backend,
    phase: Phase,
    records: BTreeMap<u64, Record>,
    next_generation: u64,
    /// The index as last sent, so an unchanged one is not written again.
    committed: Option<Sidecar>,
    /// `.drawbar/` exists, or drawbar has written here. Until then the index is written
    /// only once it holds something no file does, so a folder opened and looked at is
    /// left as it was.
    indexed: bool,
    /// Whether the window had focus at the last [`Store::focus`].
    focused: bool,
    /// A rescan, or a check of the files read so far, is in flight.
    scanning: bool,
    loading: Option<Loading>,
    /// The paths the check in flight looks at.
    checking: BTreeSet<LibPath>,
    /// A rescan is owed once the open's listing is complete.
    owes: bool,
    /// A send is waiting for the rescan in flight.
    send_waits: bool,
    /// An unsaved view of a slot is kept here as a working copy. Off once the library is
    /// being handed over, since the view stays in the window and not in this library.
    keeps_views: bool,
    /// The library's name for the browser, or `None` for This computer's own.
    name: Option<String>,
    /// Working copies the index still names that nothing needs, to drop at the next
    /// full pass.
    stale: Vec<String>,
    /// The new id of each row the open gave one, with a working copy, and that copy's
    /// file, which the next full pass writes again under the new id.
    renamed: Vec<(u64, String)>,
    /// The assets whose file is still to be read for its CRC in the background, and
    /// every asset asked for so far, each of which is asked for once.
    unprinted: std::collections::VecDeque<u64>,
    printed: BTreeSet<u64>,
    /// A [`Cmd::Fingerprint`] is in flight.
    fingerprinting: bool,
    /// The tracked assets whose file is still to be read in the background, and every
    /// one asked for so far, each asked for once unless it was refused for room. See
    /// [`Store::fetch_tracked`].
    unfetched: std::collections::VecDeque<u64>,
    fetched: BTreeSet<u64>,
    /// The assets of the background read in flight.
    fetching: BTreeSet<u64>,
    /// Each rename sent that has not answered, from and to.
    moving: Vec<(LibPath, LibPath)>,
    /// The most bytes of the library's files the assets may hold whole: [`MOST_BYTES`].
    budget: u64,
    /// What was read of the library's files, this session and before.
    cache: Cache,
    /// The cache has forgotten the files the open's listing did not find.
    pruned: bool,
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
    /// How many commands have been sent since the open.
    issued: u64,
}

impl Store {
    /// Open the library the backend holds.
    pub fn start(mut backend: Backend) -> Store {
        backend.send(Cmd::Open);
        Store {
            backend,
            phase: Phase::Opening,
            records: BTreeMap::new(),
            next_generation: 1,
            committed: None,
            indexed: false,
            focused: true,
            scanning: false,
            loading: None,
            checking: BTreeSet::new(),
            owes: false,
            send_waits: false,
            keeps_views: true,
            name: None,
            stale: Vec::new(),
            renamed: Vec::new(),
            unprinted: Default::default(),
            printed: BTreeSet::new(),
            fingerprinting: false,
            unfetched: Default::default(),
            fetched: BTreeSet::new(),
            fetching: BTreeSet::new(),
            moving: Vec::new(),
            budget: MOST_BYTES,
            cache: Cache::memory(),
            pruned: false,
            #[cfg(test)]
            asked_files: 0,
            roomless: BTreeMap::new(),
            #[cfg(test)]
            looked_for_room: 0,
            sent: 0,
            issued: 0,
        }
    }

    /// The same store, heading the browser under `name` rather than as This computer.
    pub fn named(self, name: String) -> Store {
        Store {
            name: Some(name),
            ..self
        }
    }

    /// The same store, remembering what was read of the library's files in `cache`, which
    /// outlives the session. Otherwise it remembers them only for the session.
    pub fn remembering(self, cache: Cache) -> Store {
        Store { cache, ..self }
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

    /// The library's folder for temporary files, while it may be written. drawbar makes
    /// it at the library's first write, so it may not be there yet.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tmp(&self) -> Option<std::path::PathBuf> {
        self.open().then(|| self.root().join(super::TMP))
    }

    /// Send the saves the files need, then say whether no save or rescan waits for its
    /// answer. A library whose answers cannot be waited for is let go only once settled,
    /// so its last pass sends no save and the index it writes holds every save's answer.
    pub fn settled(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
    ) -> bool {
        self.sync(workspace, browser, queue, Pass::Files);
        !self.scanning && !self.saving() && self.moving.is_empty()
    }

    fn saving(&self) -> bool {
        self.records.values().any(|record| record.saving)
    }

    /// The names of the assets that letting this library go would lose, where nothing
    /// may be written here: each edit not already kept as a working copy, and each asset
    /// never written to a file. A library open for writing keeps every one at its last
    /// pass.
    pub fn unkept(&self, workspace: &Workspace) -> Vec<String> {
        if self.open() {
            return Vec::new();
        }
        workspace
            .listed()
            .filter(|entity| {
                let record = self.records.get(&entity.id);
                let written = record.is_some_and(|record| record.fingerprint.is_some());
                let stamp = workspace
                    .kept_edit(entity.id)
                    .map_or(entity.stamp, |(_, stamp)| stamp);
                let held = record
                    .and_then(|record| record.working.as_ref())
                    .is_some_and(|working| working.stamp == stamp);
                !written || (entity.is_unsaved() && !held)
            })
            .map(|entity| entity.name.clone())
            .collect()
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

    fn open(&self) -> bool {
        matches!(self.phase, Phase::Open)
    }

    /// Whether a file from outside can be copied into the library: it is open, and
    /// nothing has said it may not be written.
    pub fn takes_files(&self) -> bool {
        self.open()
    }

    fn opened(&self) -> bool {
        matches!(self.phase, Phase::Open | Phase::ReadOnly(_))
    }

    /// Where the library is, and what state it is in, for the browser's header.
    fn place(&self) -> Where {
        Where {
            name: self.name.clone(),
            label: self.backend.label(),
            reveal: self.backend.reveal(),
            note: match &self.phase {
                Phase::Opening => Some("opening…".to_string()),
                Phase::Open => None,
                Phase::ReadOnly(why) => Some(format!("read-only: {why}")),
                Phase::Failed(why) => Some(format!("not opened: {why}")),
            },
            opening: matches!(self.phase, Phase::Opening) || self.loading.is_some(),
            listing: self.loading.as_ref().map(|loading| loading.files),
        }
    }

    /// Fold in what the backend has answered, up to [`PARTS_A_FRAME`] parts of a
    /// listing. Returns whether a send held by [`Store::hold_send`] may now go ahead.
    pub fn poll(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        self.recalled(workspace, browser, log);
        self.ask(workspace, browser, queue, log);
        self.walk(browser);
        let mut released = false;
        let mut parts = 0;
        while parts < PARTS_A_FRAME {
            let (event, held) = match self.ready() {
                Some(event) => (event, true),
                None => match self.backend.try_recv() {
                    Some(event) => (event, false),
                    None => break,
                },
            };
            parts += usize::from(matches!(event, Event::Listed { .. }));
            released |= match held {
                true => self.react(event, workspace, browser, queue, log),
                false => self.handle(event, workspace, browser, queue, log),
            };
        }
        if parts == PARTS_A_FRAME {
            workspace.ctx().request_repaint();
        }
        browser.folders.place = Some(self.place());
        released
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
        match self.ready() {
            Some(event) => self.react(event, workspace, browser, queue, log),
            None => {
                let Some(event) = self.backend.recv() else {
                    return false;
                };
                self.handle(event, workspace, browser, queue, log)
            }
        };
        browser.folders.place = Some(self.place());
        true
    }

    /// The next answer of the listing held back that may now be folded in.
    fn ready(&mut self) -> Option<Event> {
        self.loading.as_mut()?.ready()
    }

    /// Read at most this much of the library's files whole, in place of [`MOST_BYTES`].
    #[cfg(test)]
    pub fn budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    /// Ask the backend for the files of the unread assets something needs, reading at
    /// most what keeps the assets' whole bytes within [`MOST_BYTES`]. An asset refused
    /// for want of room is asked for again once room can be made for it. Nothing is asked
    /// until the cache has said what it remembers, so a row it remembers is not read.
    pub(crate) fn ask(
        &mut self,
        workspace: &mut Workspace,
        browser: &Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        let waits = !workspace.wants() && self.roomless.is_empty();
        if !self.opened() || waits || !self.cache.loaded() {
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
                let saved = workspace.get(**id).map(|entity| entity.saved.stamp);
                saved.is_some_and(|saved| record.rereadable(saved, &self.moving))
                    && !queue.holds(**id)
            })
            .filter_map(|(id, record)| {
                let (seen, freed) = workspace.evictable(*id)?;
                let tracked = tracked(*id, record, workspace, browser);
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

    /// Ask the backend to list the folders whose removal waits on what is in them. Once
    /// the whole tree is listed, every folder is.
    fn walk(&mut self, browser: &mut Browser) {
        for dir in browser.folders.take_walks() {
            match self.loading.is_some() {
                true => self.send(Cmd::Walk(dir)),
                false => browser.folders.walked(dir),
            }
        }
    }

    /// Send a command, counting it.
    fn send(&mut self, cmd: Cmd) {
        self.issued += 1;
        #[cfg(test)]
        if let Cmd::Read { files, .. } = &cmd {
            self.asked_files += files.len();
        }
        self.backend.send(cmd);
    }

    /// Rename a file or folder, with the rows no file has claimed yet that it takes
    /// along.
    fn rename(&mut self, from: LibPath, to: LibPath, rows: Vec<u64>) {
        self.write(Cmd::Move {
            from: from.clone(),
            to: to.clone(),
        });
        self.moving.push((from.clone(), to.clone()));
        if let Some(loading) = &mut self.loading {
            loading.moves.push(Rename {
                sent: self.issued,
                from,
                to,
                rows,
                moved: false,
            });
        }
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
        self.cache.moved(to, from);
        if let Some(loading) = &mut self.loading {
            if let Some(rename) = loading.answered(from, to, false) {
                loading.unmoved(rename);
            }
        }
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
        browser: &Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        let background = answers.iter().any(|(id, _)| self.fetching.contains(id));
        for (id, answer) in answers {
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
            record.summarized = None;
            workspace.took(id, found.bytes, found.file);
        }
        if background {
            self.next_fetches(workspace);
        }
    }

    /// Wait for what the cache kept of earlier sessions, and take it in as
    /// [`Store::poll`] would.
    #[cfg(test)]
    pub fn recall_kept(&mut self, workspace: &mut Workspace, browser: &Browser, log: &mut Log) {
        if self.cache.arrive(log) {
            self.took_in(workspace, browser);
        }
    }

    /// What the cache holds now.
    #[cfg(test)]
    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Whether a rescan is waiting for its answer.
    #[cfg(test)]
    pub fn scanning(&self) -> bool {
        self.scanning
    }

    /// Whether the open's listing is still arriving.
    #[cfg(test)]
    pub fn listing(&self) -> bool {
        self.loading.is_some()
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
            Phase::ReadOnly(why) => Some(why),
            _ => None,
        }
    }

    fn handle(
        &mut self,
        event: Event,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        match self.hold(event) {
            Some(event) => self.react(event, workspace, browser, queue, log),
            None => false,
        }
    }

    /// Fold in one answer, in its turn. Returns whether a send held by
    /// [`Store::hold_send`] may now go ahead.
    fn react(
        &mut self,
        event: Event,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        match event {
            Event::Opened(Ok(opened)) => self.begin(opened, workspace, browser, log),
            Event::Listed { part, ran } => self.listed(part, ran, workspace, browser, log),
            Event::Complete(complete) => {
                self.complete(complete, workspace, browser, queue, log);
                self.pay();
            }
            Event::Walked { mut dir, ran } => {
                if let Some(loading) = &self.loading {
                    loading.now(ran, &mut dir);
                }
                browser.folders.walked(dir);
            }
            Event::Checked(Ok(files)) => {
                self.scanning = false;
                let moved = self.checked(files, workspace, browser, queue, log);
                self.pay();
                if std::mem::take(&mut self.send_waits) {
                    return self.release(&moved, queue, workspace, log);
                }
            }
            Event::Opened(Err(why)) => {
                log.error(format!("opening the library: {why}"));
                log.trouble(format!(
                    "This computer's library could not be opened ({why}). Nothing you \
                     make here is kept."
                ));
                self.phase = Phase::Failed(why);
            }
            Event::Scanned(Ok(listing)) => {
                self.scanning = false;
                let moved = self.rescanned(listing, workspace, browser, queue, log);
                if std::mem::take(&mut self.send_waits) {
                    return self.release(&moved, queue, workspace, log);
                }
            }
            Event::Scanned(Err(why)) | Event::Checked(Err(why)) => {
                self.scanning = false;
                self.send_waits = false;
                log.error(format!("reading the library again: {why}"));
                log.trouble("The library folder could not be read, so nothing was sent.");
            }
            Event::Read(answers) => self.took(answers, workspace, browser, queue, log),
            Event::Fingerprinted(files) => self.fingerprinted(files),
            Event::Moved { from, to, result } => match result {
                _ if !self.answered_move(&from, &to) => {}
                Ok(()) => {
                    if let Some(loading) = &mut self.loading {
                        loading.answered(&from, &to, true);
                    }
                }
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
            Event::ReadOnly(why) => self.refused(why, workspace, log),
            Event::Failed(why) => {
                log.error(why.clone());
                log.trouble(format!(
                    "The library folder did not change as asked: {why}."
                ));
                // Whatever failed may have been the index or a working copy, so both are
                // written again, whole, at the next full pass, and that pass drops the
                // copies it replaces.
                self.committed = None;
                for (id, record) in &mut self.records {
                    if let Some(old) = record.working.take() {
                        self.stale.push(working_name(*id, old.copy.generation));
                    }
                }
                self.rescan();
            }
        }
        false
    }

    /// Hold back an answer of the listing gathered before a rename that has not
    /// answered, and any behind one held already, so the answers stay in order. Returns
    /// the answer where it is not held.
    fn hold(&mut self, event: Event) -> Option<Event> {
        let Some(loading) = &mut self.loading else {
            return Some(event);
        };
        let Some(ran) = gathered(&event) else {
            return Some(event);
        };
        if loading.held.is_empty() && !loading.waits(ran) {
            return Some(event);
        }
        loading.held.push_back(event);
        None
    }

    /// A write found the library closed to it: another drawbar took the lock first, or
    /// the folder refused the sidecar. Every save in flight counts as unsaved again.
    fn refused(&mut self, why: String, workspace: &mut Workspace, log: &mut Log) {
        if !self.open() {
            return;
        }
        log.trouble(format!(
            "This computer's library is read-only now: {why}. Edits stay in memory until \
             you quit."
        ));
        for (id, record) in &mut self.records {
            if std::mem::take(&mut record.saving) {
                workspace.unsave(*id);
                if let Some(entity) = workspace.get(*id) {
                    record.saved = entity.saved.stamp;
                }
            }
        }
        self.phase = Phase::ReadOnly(why);
    }

    /// Send a command that writes. The first one makes `.drawbar/`.
    fn write(&mut self, cmd: Cmd) {
        self.indexed = true;
        self.sent += 1;
        self.send(cmd);
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

    /// List the tree again, to fold in what changed outside drawbar. While the open's
    /// listing is in flight, the files read so far are looked at again instead, and the
    /// rescan is owed until the listing is complete.
    pub(crate) fn rescan(&mut self) {
        if !self.opened() || self.scanning {
            return;
        }
        if self.loading.is_some() {
            self.owes = true;
            let known: BTreeMap<LibPath, Fingerprint> = self
                .records
                .values()
                .filter(|record| record.holds != Holds::Unread)
                .filter_map(|record| Some((record.path.clone()?, record.fingerprint?)))
                .collect();
            self.checking = known.keys().cloned().collect();
            self.scanning = true;
            return self.send(Cmd::Check { known });
        }
        let mut known = BTreeMap::new();
        for record in self.records.values() {
            let (Some(path), Some(print)) = (&record.path, record.fingerprint) else {
                continue;
            };
            known.insert(path.clone(), (print, record.holds));
        }
        self.scanning = true;
        self.send(Cmd::Scan { known });
    }

    /// Send the rescan owed, once nothing stands in its way.
    fn pay(&mut self) {
        if self.owes && self.loading.is_none() && !self.scanning {
            self.owes = false;
            self.rescan();
        }
    }

    /// Write everything, waiting for the saves in flight to answer, then let the library
    /// go. It blocks, so it is for the end of a session.
    ///
    /// ⚠️ An index sent beside a save carries the file's fingerprint from before it, so a
    /// pass that sent anything is followed by another once the saves have answered.
    pub fn close(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        const ROUNDS: usize = 8;
        for _ in 0..ROUNDS {
            if !self.answered(workspace, browser, queue, log) {
                break;
            }
            let sent = self.sent;
            self.sync(workspace, browser, queue, Pass::Last);
            if self.sent == sent {
                break;
            }
        }
        self.summarize(workspace);
        self.cache.finish(log);
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
        while self.saving() || !self.moving.is_empty() {
            match self.backend.recv() {
                None => return false,
                // ⚠️ A listing taken before the last pass's moves would read each as a
                // deletion, and nothing needs it now.
                Some(Event::Scanned(_) | Event::Checked(_)) => self.scanning = false,
                Some(event) => {
                    self.handle(event, workspace, browser, queue, log);
                }
            }
        }
        true
    }

    /// Write everything, as [`Store::close`] does, before another library takes this one's
    /// place. The views of slots stay in the window, so their working copies leave this
    /// library.
    pub fn hand_over(
        mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        self.keeps_views = false;
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

    /// Take in what an open found before its listing: the index, and the working copies.
    fn begin(
        &mut self,
        opened: Opened,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let Opened {
            writable,
            indexed,
            sidecar,
            mut working,
            swept,
            stranded,
        } = opened;
        if !stranded.is_empty() {
            let named: Vec<String> = stranded.iter().map(|dir| format!("“{dir}”")).collect();
            log.trouble(format!(
                "An interrupted rename left {} under the name it moved through, and another \
                 folder has its name, so it was left as it is.",
                named.join(", ")
            ));
        }
        self.phase = match writable {
            Ok(()) => Phase::Open,
            Err(why) => {
                log.trouble(format!(
                    "This computer's library opened read-only: {why}. Edits stay in memory \
                     until you quit."
                ));
                Phase::ReadOnly(why)
            }
        };
        self.indexed = indexed;
        self.next_generation = sidecar.next_generation.max(1);
        let Sidecar { tags, assets, .. } = sidecar;

        // A row under an id this session has already given out, to an asset made while
        // the library was opening or one of a library open before, takes the next free
        // one, so nothing still naming that id reaches this asset.
        let floor = workspace.next_id();
        let mut next = floor
            .max(sidecar.next_id)
            .max(assets.keys().max().map_or(0, |id| id.saturating_add(1)));
        let mut rows: BTreeMap<u64, Row> = BTreeMap::new();
        for (id, row) in assets {
            if id >= floor {
                rows.insert(id, row);
                continue;
            }
            let moved = next;
            next = next.saturating_add(1);
            if let Some(bytes) = working.remove(&id) {
                working.insert(moved, bytes);
            }
            if let Some(copy) = row.working {
                self.renamed
                    .push((moved, working_name(id, copy.generation)));
            }
            rows.insert(moved, row);
        }
        workspace.reserve(next);
        browser.tags.restore(
            tags,
            rows.iter().map(|(id, row)| (*id, row.tags.iter().copied())),
        );
        self.loading = Some(Loading {
            by_path: by_path(&rows),
            rows,
            claimed: BTreeSet::new(),
            working,
            next,
            dirs: Vec::new(),
            files: 0,
            swept,
            moves: Vec::new(),
            held: Default::default(),
        });
    }

    /// Fold in one part of the open's listing, gathered after `ran` commands: each file
    /// the index names comes back as its asset, and each it does not as a new one.
    fn listed(
        &mut self,
        mut part: Listing,
        ran: u64,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let Some(mut loading) = self.loading.take() else {
            return;
        };
        loading.next = loading.next.max(workspace.next_id());
        loading.caught_up(ran, &mut part);
        let Listing {
            dirs,
            files,
            unread,
            others,
            unwalked,
        } = part;
        // The folder changes not yet sent have moved what the browser shows on already.
        let ahead = |paths: Vec<LibPath>| -> Vec<LibPath> {
            paths
                .iter()
                .map(|path| browser.folders.ahead(path))
                .collect()
        };
        let (shown, others, unwalked) = (ahead(dirs.clone()), ahead(others), ahead(unwalked));
        let unread: Vec<(LibPath, String)> = unread
            .into_iter()
            .map(|(path, why)| (browser.folders.ahead(&path), why))
            .collect();
        beside(&browser.folders, &unread, &unwalked, log);
        let folders = &mut browser.folders;
        folders.unread.extend(unread);
        folders.others.extend(others);
        folders.unwalked.extend(unwalked);
        folders.add(&shown);
        loading.dirs.extend(dirs);

        let mut back = Vec::new();
        let mut edits = Vec::new();
        let mut conflicts = Vec::new();
        loading.files += files.len();
        for found in files {
            let Some(id) = loading.by_path.get(&found.path).copied() else {
                let id = loading.next;
                loading.next = id.saturating_add(1);
                back.push(newcomer(id, found, &mut self.records));
                continue;
            };
            loading.claimed.insert(id);
            let print = loading.rows.get(&id).and_then(|row| row.fingerprint);
            let changed = !diff::same(print, &found);
            if let Some((saved, edit, conflicted)) = self.claim(&mut loading, id, found, changed) {
                back.push(saved);
                edits.extend(edit.map(|edit| (id, edit)));
                if conflicted {
                    conflicts.push(id);
                }
            }
        }
        self.restored(back, edits, &loading, workspace, &browser.folders, log);
        // An edit the file already holds was let go as it came back.
        for id in conflicts {
            if workspace.get(id).is_some_and(LocalEntity::is_unsaved) {
                conflict(id, workspace, browser, log);
            }
        }
        self.loading = Some(loading);
    }

    /// A listed file at the path of a row, or one matched to a row it moved from: the
    /// asset it comes back as, with the bytes or the edit its working copy holds over it,
    /// and whether that is over a file that changed.
    fn claim(
        &mut self,
        loading: &mut Loading,
        id: u64,
        found: Found,
        changed: bool,
    ) -> Option<(Saved, Option<Edit>, bool)> {
        let row = loading.rows.get(&id)?;
        let print = diff::kept(row.fingerprint, &found);
        let copy = loading.working.remove(&id);
        let (mine, edit) = match (row.working.map(|copy| copy.keeps), copy) {
            // The open read every edit's copy through, so this one reads.
            (Some(Keeps::Edit), Some(copy)) => (None, Edit::from_working(&copy).ok()),
            (_, mine) => {
                let mine = mine.filter(|mine| match (&found.bytes, &found.file) {
                    (Some(bytes), _) => mine != bytes,
                    (None, Some(file)) => !file.holds(mine),
                    (None, None) => found.crc != Some(nord_format::crc::crc32(mine)),
                });
                (mine, None)
            }
        };
        let conflicted = changed && (mine.is_some() || edit.is_some());
        self.records.insert(id, Record::of_found(&found, print));
        let saved = Saved {
            id,
            name: found.path.leaf().to_string(),
            unread: (!found.read()).then_some(found.stat.len),
            path: Some(found.path),
            origin: Origin::from(&row.origin),
            saved: found.bytes.unwrap_or_default(),
            file: found.file,
            unsaved: mine,
        };
        Some((saved, edit, conflicted))
    }

    /// Put assets back in the workspace, with the edits their working copies kept over
    /// their files, and record what each was restored as.
    fn restored(
        &mut self,
        back: Vec<Saved>,
        edits: Vec<(u64, Edit)>,
        loading: &Loading,
        workspace: &mut Workspace,
        folders: &Folders,
        log: &mut Log,
    ) {
        let ids: Vec<u64> = back.iter().map(|saved| saved.id).collect();
        workspace.restore(back, Some(loading.next), log);
        for (id, edit) in edits {
            workspace.restore_edit(id, edit);
        }
        for id in ids {
            self.settle(id, workspace, &loading.rows);
            self.place_ahead(id, workspace, folders);
            self.recall(id, workspace);
        }
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
            workspace.place(id, ahead);
        }
    }

    /// The open's listing is complete: the files that may be ones moved are matched to
    /// the rows no file claimed, and whatever is left of the index is let go or kept.
    fn complete(
        &mut self,
        mut complete: Complete,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        let Some(mut loading) = self.loading.take() else {
            return;
        };
        loading.next = loading.next.max(workspace.next_id());
        let strangers = complete.strangers.iter_mut().map(|found| &mut found.path);
        for path in strangers.chain(&mut complete.gone) {
            loading.now(complete.ran, path);
        }
        let swept = loading.swept + complete.swept;
        if swept > 0 {
            log.info(format!("removed {swept} leftovers of interrupted writes"));
        }
        let known = loading
            .rows
            .iter()
            .filter(|(id, _)| !loading.claimed.contains(*id))
            .filter_map(|(id, row)| {
                Some((
                    *id,
                    Known {
                        path: row.path.clone()?,
                        fingerprint: row.fingerprint,
                    },
                ))
            })
            .collect();
        // Each file that may be a row moved was listed as an asset of its own, which it
        // gives up to the row it turns out to be, unless something has changed it since.
        let listed: BTreeMap<LibPath, u64> = self
            .records
            .iter()
            .filter(|(id, _)| !loading.rows.contains_key(*id))
            .filter_map(|(id, record)| Some((record.path.clone()?, *id)))
            .collect();
        let strangers = complete.strangers.into_iter().filter(|found| {
            let entity = listed.get(&found.path).and_then(|id| workspace.get(*id));
            entity.is_some_and(|entity| !precious(entity, queue))
        });
        let matched = match_files(&known, strangers.collect());
        let mut back = Vec::new();
        let mut edits = Vec::new();
        for (id, found) in matched.renamed {
            let Some(stranger) = listed.get(&found.path).copied() else {
                continue;
            };
            if let Some(from) = loading.rows.get(&id).and_then(|row| row.path.as_ref()) {
                self.cache.moved(from, &found.path);
            }
            self.records.remove(&stranger);
            workspace.forget(stranger);
            for tag in browser.tags.worn(stranger).clone() {
                browser.tags.set(id, tag, true);
            }
            browser.tags.forget(stranger);
            if let Some((saved, edit, _)) = self.claim(&mut loading, id, found, false) {
                back.push(saved);
                edits.extend(edit.map(|edit| (id, edit)));
            }
        }
        let mut missing = Vec::new();
        for id in matched.vanished {
            let Some(row) = loading.rows.get(&id) else {
                continue;
            };
            // An edit's copy has no file to be made over, so its row is kept, naming the
            // copy, as any precious row is.
            let keeps = row.working.map(|copy| copy.keeps);
            match (keeps, loading.working.remove(&id)) {
                (Some(Keeps::Bytes), Some(bytes)) => {
                    let mut record = Record::of_file(row.path.clone().unwrap_or_default(), None);
                    record.fingerprint = row.fingerprint;
                    record.missing = true;
                    self.records.insert(id, record);
                    missing.push(id);
                    back.push(saved_from(id, row, bytes));
                }
                _ if row.precious() => browser.folders.lose(id, row.clone()),
                _ => {}
            }
        }
        // Rows with no path are views of slots that held an edit; each comes back as a
        // file on this computer. Its working copy stays until that file is written.
        let mut viewed = Vec::new();
        for (id, row) in loading.rows.iter().filter(|(_, row)| row.path.is_none()) {
            if let (Some(bytes), Some(copy)) = (loading.working.remove(id), row.working) {
                viewed.push((*id, copy));
                back.push(saved_from(*id, row, bytes));
            }
        }
        self.restored(back, edits, &loading, workspace, &browser.folders, log);
        for path in &complete.gone {
            if let Some(id) = listed.get(path) {
                self.vanished(*id, workspace, browser, queue, log);
            }
        }
        // A file gone since the walk listed it went somewhere the walk may not have
        // looked, so the tree is listed again.
        self.owes |= !complete.gone.is_empty();
        for id in &missing {
            workspace.unsave(*id);
            browser.folders.missing.insert(*id);
        }
        for (id, copy) in viewed {
            let stamp = workspace.get(id).map_or(0, |entity| entity.stamp);
            let mut record = Record::of_file(LibPath::root(), None);
            record.path = None;
            record.working = Some(Kept { copy, stamp });
            self.records.insert(id, record);
        }
        browser.folders.sync(&loading.dirs);
        flag_duplicates(workspace, browser, log);
        let count = self.records.values().filter(|record| record.path.is_some());
        let count = count.count();
        if count > 0 {
            log.say(match count {
                1 => "1 file on this computer.".to_string(),
                n => format!("{n} files on this computer."),
            });
        }
        self.prune(browser);
        self.fetch_tracked(workspace, browser);
    }

    /// Record the stamps of an asset just restored, and the working copy it came back
    /// with, so nothing is written again for it. A copy whose edit is no longer held is
    /// dropped at the next full pass.
    fn settle(&mut self, id: u64, workspace: &Workspace, rows: &BTreeMap<u64, Row>) {
        let (Some(entity), Some(record)) = (workspace.get(id), self.records.get_mut(&id)) else {
            return;
        };
        record.saved = entity.saved.stamp;
        let Some(copy) = rows.get(&id).and_then(|row| row.working) else {
            record.working = None;
            return;
        };
        let stamp = match copy.keeps {
            Keeps::Bytes => entity.is_unsaved().then_some(entity.stamp),
            Keeps::Edit => workspace.kept_edit(id).map(|(_, stamp)| stamp),
        };
        record.working = stamp.map(|stamp| Kept { copy, stamp });
        if record.working.is_none() {
            self.stale.push(working_name(id, copy.generation));
        }
    }

    /// Fold a new listing in, and return the assets whose files moved or changed.
    fn rescanned(
        &mut self,
        listing: Listing,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> BTreeSet<u64> {
        let Listing {
            dirs,
            files,
            unread,
            others,
            unwalked,
        } = listing;
        beside(&browser.folders, &unread, &unwalked, log);
        browser.folders.unread = unread;
        browser.folders.others = others;
        browser.folders.unwalked = unwalked.into_iter().collect();
        let touched = self.fold(|_| true, files, workspace, browser, queue, log);
        browser.folders.sync(&dirs);
        flag_duplicates(workspace, browser, log);
        touched
    }

    /// Fold in what a check found of the files read so far, and return the assets whose
    /// files moved or changed.
    fn checked(
        &mut self,
        files: Vec<Found>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> BTreeSet<u64> {
        let checking = std::mem::take(&mut self.checking);
        let looked = |path: &LibPath| checking.contains(path);
        self.fold(looked, files, workspace, browser, queue, log)
    }

    /// Match the files found to the records whose paths `looked` at, and fold in where
    /// each landed. Returns the assets whose files moved or changed.
    fn fold(
        &mut self,
        looked: impl Fn(&LibPath) -> bool,
        files: Vec<Found>,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> BTreeSet<u64> {
        let known = self
            .records
            .iter()
            .filter_map(|(id, record)| {
                let path = record.path.clone().filter(|path| looked(path))?;
                Some((
                    *id,
                    Known {
                        path,
                        fingerprint: record.fingerprint,
                    },
                ))
            })
            .collect();
        let matched = match_files(&known, files);
        let mut touched = BTreeSet::new();
        for (id, found) in matched.same {
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            if let Some(print) = &mut record.fingerprint {
                print.len = found.stat.len;
                print.modified = found.stat.modified;
            }
            if std::mem::take(&mut record.missing) {
                browser.folders.missing.remove(&id);
            }
        }
        for (id, found) in matched.changed {
            touched.insert(id);
            self.changed(id, found, workspace, browser, log);
        }
        for (id, found) in matched.renamed {
            touched.insert(id);
            let before = workspace.get(id).map(|entity| entity.name.clone());
            workspace.place(id, browser.folders.ahead(&found.path));
            if let Some(record) = self.records.get_mut(&id) {
                if let Some(from) = record.path.replace(found.path.clone()) {
                    self.cache.moved(&from, &found.path);
                }
                if let Some(print) = &mut record.fingerprint {
                    *print = diff::kept(Some(*print), &found);
                }
            }
            if let Some(before) = before {
                log.say(format!(
                    "“{before}” was moved to {} outside drawbar; its tags went with it.",
                    found.path
                ));
            }
        }
        for id in matched.vanished {
            touched.insert(id);
            self.vanished(id, workspace, browser, queue, log);
        }
        let arrived: Vec<Found> = matched.arrived;
        if !arrived.is_empty() {
            let mut next = workspace.next_id();
            let mut back = Vec::new();
            for found in arrived {
                back.push(newcomer(next, found, &mut self.records));
                next = next.saturating_add(1);
            }
            let ids: Vec<u64> = back.iter().map(|saved| saved.id).collect();
            log.say(match ids.len() {
                1 => "1 file appeared in the library folder.".to_string(),
                n => format!("{n} files appeared in the library folder."),
            });
            workspace.restore(back, Some(next), log);
            for id in ids {
                self.settle(id, workspace, &BTreeMap::new());
                self.place_ahead(id, workspace, &browser.folders);
                self.recall(id, workspace);
            }
        }
        touched
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
        if !found.read() {
            if let Some(record) = self.records.get_mut(&id) {
                record.fingerprint = Some(found.fingerprint());
                record.summarized = None;
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
            false => log.say(format!(
                "“{name}” changed on disk, and drawbar now shows it as it is there."
            )),
        }
        browser.folders.missing.remove(&id);
        let saved = workspace.get(id).map(|entity| entity.saved.stamp);
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
    ) {
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        if record.missing {
            return;
        }
        let Some(entity) = workspace.get(id) else {
            return;
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
            return;
        }
        if let Some(path) = self.records.remove(&id).and_then(|record| record.path) {
            self.cache.forget(&path);
        }
        browser.tags.forget(id);
        workspace.remove(id, log);
        log.say(format!("“{name}” was deleted outside drawbar."));
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
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        record.saving = false;
        let name = workspace
            .get(id)
            .map_or_else(|| path.leaf().to_string(), |entity| entity.name.clone());
        let why = match result {
            Ok(print) => {
                record.fingerprint = Some(print);
                record.missing = false;
                record.holds = Holds::Whole;
                browser.folders.missing.remove(&id);
                return;
            }
            Err(Failure::Moved) if record.fingerprint.is_none() => {
                // A file took the name first. The asset takes another at the next sync.
                self.records.remove(&id);
                workspace.unplace(id);
                self.rescan();
                "a file of that name appeared in the folder first".to_string()
            }
            Err(Failure::Moved) => {
                self.rescan();
                "it changed on disk since drawbar read it".to_string()
            }
            Err(Failure::Room(_)) => too_much(),
            Err(Failure::Io(why)) => why,
        };
        workspace.unsave(id);
        if let (Some(record), Some(entity)) = (self.records.get_mut(&id), workspace.get(id)) {
            record.saved = entity.saved.stamp;
        }
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
                record.summarized = None;
                browser.folders.missing.remove(&id);
                match found.file {
                    Some(file) => workspace.edit_saved(id, file),
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
            record.saving = true;
            self.write(Cmd::Rewrite {
                id,
                path,
                from,
                edit,
                expect,
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
                record.summarized = None;
                browser.folders.missing.remove(&id);
                workspace.took(id, None, found.file);
                // The asset the copy was moved from goes now it has landed, and its file
                // with it at the next pass.
                if let Some(from) = workspace.moved_over(id) {
                    browser.tags.forget(from);
                    browser.folders.missing.remove(&from);
                    workspace.remove(from, log);
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
                workspace.forget(id);
                match from {
                    Some(CopyOf::Outside(from)) => {
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

    /// Bring the files level with the workspace, and return whether it did. A save that
    /// must wait for the one before it to answer makes it return `false`, and the next
    /// sync sends it.
    ///
    /// ⚠️ Nothing is sent while a rescan is in flight: its listing must describe the
    /// files as the commands before it left them, or a file moved after it was sent would
    /// read as one deleted and another made.
    pub fn sync(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        pass: Pass,
    ) -> bool {
        if pass != Pass::Files {
            self.summarize(workspace);
            self.cache.write();
        }
        if !self.open() {
            return true;
        }
        if self.scanning && pass != Pass::Last {
            return false;
        }
        let full = pass != Pass::Files;
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
        let mut waiting = self.moving.clone();
        for op in &held {
            let [from, to] = ends(op);
            waiting.push((from.clone(), to.clone()));
        }
        let holds = !held.is_empty();
        browser.folders.hold(held);
        let (removals, ops): (Vec<Op>, Vec<Op>) = ops
            .into_iter()
            .partition(|op| matches!(op, Op::RemoveDir(_)));
        for op in ops {
            self.tree_op(op);
        }
        crate::folders::place_new(workspace, &browser.folders);
        let mut writes = Vec::new();
        if full {
            self.renumber(&mut writes);
        }
        let mut drops = match full {
            true => std::mem::take(&mut self.stale),
            false => Vec::new(),
        };
        let mut done = true;
        for entity in workspace.entities() {
            done &= self.file(entity, workspace.arriving(entity.id), &waiting);
            if full {
                let edit = workspace.kept_edit(entity.id);
                self.working(entity, edit, queue, &mut writes, &mut drops);
            }
        }
        self.send_edits(workspace, &waiting);
        self.forget_gone(workspace, &mut drops);
        for op in removals {
            self.tree_op(op);
        }
        if full {
            let sidecar = self.sidecar(workspace, browser);
            let wanted = self.indexed || !writes.is_empty() || beyond_files(&sidecar);
            let changed = self.committed.as_ref() != Some(&sidecar)
                || !writes.is_empty()
                || !drops.is_empty();
            if wanted && changed {
                self.write(Cmd::Commit {
                    sidecar: sidecar.clone(),
                    working: writes,
                    drop: drops,
                });
                self.committed = Some(sidecar);
            }
        }
        if pass == Pass::Full {
            self.fingerprint_precious(workspace, browser);
            self.fetch_tracked(workspace, browser);
        }
        done && !holds
    }

    /// Read in the background, for its CRC, each file that holds something no file can
    /// say (a tag, an unsaved edit, the slot it came off) where no CRC is known, so that
    /// a rename made outside drawbar keeps it. Once the listing is complete, a few files
    /// at a time. A file waiting for [`Store::fetch_tracked`] to read it whole is left to
    /// that read.
    fn fingerprint_precious(&mut self, workspace: &Workspace, browser: &Browser) {
        if self.loading.is_some() || self.fingerprinting || !self.cache.loaded() {
            return;
        }
        if self.unprinted.is_empty() {
            let wanted: Vec<u64> = self
                .records
                .iter()
                .filter(|(id, record)| {
                    tracked(**id, record, workspace, browser)
                        && record.unprinted()
                        && !self.printed.contains(id)
                        && !self.fetching.contains(id)
                        && !self.unfetched.contains(id)
                })
                .map(|(id, _)| *id)
                .collect();
            self.printed.extend(&wanted);
            self.unprinted.extend(wanted);
        }
        self.next_prints();
    }

    /// Read and decode in the background each unread file the index tracks (a tag, a
    /// working copy, the slot it came off), so that it matches its slot on the
    /// instrument without anything showing it, and its CRC is known. Once the listing is
    /// complete, a few files at a time, each within the room left: such a read lets go
    /// only of what the index does not track, and one refused for room is tried again at
    /// the next full pass. Files the index does not track are read once something needs
    /// them.
    fn fetch_tracked(&mut self, workspace: &mut Workspace, browser: &Browser) {
        let waits = self.loading.is_some() || !self.cache.loaded();
        if waits || !self.opened() || !self.fetching.is_empty() {
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
                        && tracked(**id, record, workspace, browser)
                        && !self.fetched.contains(id)
                })
                .map(|(id, _)| *id)
                .collect();
            self.fetched.extend(&wanted);
            self.unfetched.extend(wanted);
        }
        self.next_fetches(workspace);
    }

    /// Take in what the cache kept of earlier sessions, once it arrives: each unread file
    /// it knows draws as it says, and the background reads wait for it.
    fn recalled(&mut self, workspace: &mut Workspace, browser: &Browser, log: &mut Log) {
        if self.cache.poll(log) {
            self.took_in(workspace, browser);
        }
    }

    /// Recall every unread file now that the cache's entries have arrived, and start what
    /// waited for them.
    fn took_in(&mut self, workspace: &mut Workspace, browser: &Browser) {
        let unread: Vec<u64> = self
            .records
            .iter()
            .filter(|(_, record)| record.holds == Holds::Unread)
            .map(|(id, _)| *id)
            .collect();
        for id in unread {
            self.recall(id, workspace);
        }
        self.prune(browser);
        self.fetch_tracked(workspace, browser);
    }

    /// Give an unread asset what the cache holds for its file, where the file's length
    /// and time are still the ones it was read at.
    fn recall(&mut self, id: u64, workspace: &mut Workspace) {
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        let (Some(path), Some(print), Holds::Unread) =
            (&record.path, record.fingerprint, record.holds)
        else {
            return;
        };
        let Some(entry) = self.cache.fresh(path, print.stat()) else {
            return;
        };
        workspace.remember(id, entry.summary.clone());
        record.summarized = workspace.get(id).map(|entity| entity.saved.stamp);
    }

    /// Keep in the cache a summary of each file whose asset holds what the file does and
    /// has been read and decoded, once per set of bytes.
    fn summarize(&mut self, workspace: &Workspace) {
        for (id, record) in &mut self.records {
            let (Some(path), Some(print)) = (&record.path, record.fingerprint) else {
                continue;
            };
            let Some(entity) = workspace.get(*id) else {
                continue;
            };
            let stamp = entity.saved.stamp;
            let clean = record.saved == stamp && !entity.is_unsaved();
            if record.summarized == Some(stamp) || !clean || record.saving || record.missing {
                continue;
            }
            let Some(summary) = Summary::of(entity) else {
                continue;
            };
            self.cache
                .put(path.clone(), cache::Entry::of(print.stat(), summary));
            record.summarized = Some(stamp);
        }
    }

    /// Forget in the cache every file the open's listing did not find, once both are
    /// here and the listing reached every folder.
    fn prune(&mut self, browser: &Browser) {
        let listed = self.opened() && self.loading.is_none();
        let whole = browser.folders.unwalked.is_empty();
        if self.pruned || !listed || !self.cache.loaded() || !whole {
            return;
        }
        self.pruned = true;
        let listed = self
            .records
            .values()
            .filter_map(|record| record.path.clone());
        self.cache.keep_only(listed.collect());
    }

    /// Ask for the next few tracked files waiting to be read in the background.
    fn next_fetches(&mut self, workspace: &mut Workspace) {
        if !self.fetching.is_empty() || !self.opened() {
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

    /// Ask for the CRCs of the next few files waiting for one.
    fn next_prints(&mut self) {
        let (most, most_bytes) = PRINTS;
        let mut files = Vec::new();
        let mut bytes = 0;
        while files.len() < most && bytes < most_bytes {
            let Some(id) = self.unprinted.pop_front() else {
                break;
            };
            let Some(record) = self.records.get(&id).filter(|record| record.unprinted()) else {
                continue;
            };
            let (Some(path), Some(print)) = (&record.path, record.fingerprint) else {
                continue;
            };
            bytes += print.len;
            files.push((id, path.clone(), print));
        }
        if files.is_empty() {
            return;
        }
        self.fingerprinting = true;
        self.send(Cmd::Fingerprint(files));
    }

    /// Take in the CRCs of files whose fingerprint is still the one sent.
    fn fingerprinted(&mut self, files: Vec<(u64, LibPath, Fingerprint)>) {
        self.fingerprinting = false;
        for (id, path, print) in files {
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            let same = record.fingerprint.map(|known| known.stat()) == Some(print.stat());
            if record.unprinted() && same && record.path.as_ref() == Some(&path) {
                record.fingerprint = Some(print);
            }
        }
        if self.opened() {
            self.next_prints();
        }
    }

    /// Write again under its new id each working copy of a row the open gave one, and
    /// drop the old copy once the index no longer names it. A row a listed file has
    /// claimed writes its edit afresh; one no file has claimed yet is written as it was
    /// read, since the index keeps that row under the new id.
    fn renumber(&mut self, writes: &mut Vec<(String, Vec<u8>)>) {
        for (id, old) in std::mem::take(&mut self.renamed) {
            match self.records.get_mut(&id) {
                Some(record) => record.working = None,
                None => writes.extend(self.loading.as_ref().and_then(|loading| {
                    let generation = loading.rows.get(&id)?.working?.generation;
                    Some((
                        working_name(id, generation),
                        loading.working.get(&id)?.clone(),
                    ))
                })),
            }
            self.stale.push(old);
        }
    }

    /// Send one change to the folders. One made or removed while the open's listing is in
    /// flight is one the listing's end keeps or drops with the rest.
    fn tree_op(&mut self, op: Op) {
        match op {
            Op::MakeDir(path) => {
                if let Some(loading) = &mut self.loading {
                    loading.dirs.push(path.clone());
                }
                self.write(Cmd::MakeDir(path))
            }
            Op::RemoveDir(path) => {
                if let Some(loading) = &mut self.loading {
                    loading.dirs.retain(|dir| *dir != path);
                }
                self.write(Cmd::RemoveDir(path))
            }
            Op::MoveDir { from, to } => {
                self.cache.moved(&from, &to);
                for record in self.records.values_mut() {
                    if let Some(moved) = record.path.as_ref().and_then(|at| at.moved(&from, &to)) {
                        record.path = Some(moved);
                    }
                }
                let rows = self
                    .loading
                    .as_mut()
                    .map(|loading| loading.relocate(&from, &to));
                self.rename(from, to, rows.unwrap_or_default());
            }
        }
    }

    /// Send what one asset's file needs: its first write, a move, a save, or the copy of
    /// the file from outside it arrives from. Returns `false` when a save or a move has
    /// to wait. `waiting` are the renames and other folder changes not settled yet, each
    /// from and to.
    fn file(
        &mut self,
        entity: &LocalEntity,
        arriving: Option<&CopyOf>,
        waiting: &[(LibPath, LibPath)],
    ) -> bool {
        let (true, Some(path)) = (entity.kept, &entity.path) else {
            return true;
        };
        // ⚠️ A file written or moved into, or out of, a folder a rename has not answered
        // for could land in whatever else has that name on disk.
        let record = self.records.get(&entity.id);
        let from = record.and_then(|record| record.path.as_ref());
        let moves = from.is_none_or(|from| from != path);
        if moves
            && [Some(path), from]
                .into_iter()
                .flatten()
                .any(|at| unsettled(waiting, at))
        {
            return false;
        }
        if let Some(from) = arriving {
            return self.import(entity, path, from, waiting);
        }
        let bytes = || entity.saved.bytes.to_vec();
        if !self
            .records
            .get(&entity.id)
            .is_some_and(|record| record.path.is_some())
        {
            // A view that held an edit comes with its working copy.
            let working = self
                .records
                .remove(&entity.id)
                .and_then(|record| record.working);
            self.records.insert(
                entity.id,
                Record {
                    saved: entity.saved.stamp,
                    saving: true,
                    working,
                    ..Record::of_file(path.clone(), None)
                },
            );
            self.write(Cmd::Save {
                id: entity.id,
                path: path.clone(),
                bytes: bytes(),
                expect: None,
            });
            return true;
        }
        let Some(record) = self.records.get_mut(&entity.id) else {
            return true;
        };
        // A fingerprint taken without its CRC learns it once the contents have been read
        // through, while the baseline is still what the file holds.
        if let Some(print) = record
            .fingerprint
            .as_mut()
            .filter(|print| print.crc.is_none())
        {
            if !record.saving && entity.saved.stamp == record.saved {
                print.crc = entity.saved.whole_crc();
            }
        }
        let missing = record.missing;
        let from = record
            .path
            .replace(path.clone())
            .filter(|from| from != path && !missing);
        // A baseline resting in its file is what the file holds, and needs no write.
        if entity.saved.file.is_some() {
            record.saved = entity.saved.stamp;
        }
        let unsaved = entity.saved.stamp != record.saved;
        // ⚠️ The file's fingerprint is known only once the save before answers, and a
        // save sent without it would be refused as a write over someone else's file.
        let waits = unsaved && record.saving;
        let save = (unsaved && !record.saving).then(|| {
            record.saved = entity.saved.stamp;
            record.saving = true;
            record.fingerprint.filter(|_| !missing)
        });
        if let Some(from) = from {
            self.cache.moved(&from, path);
            self.rename(from, path.clone(), Vec::new());
        }
        if let Some(expect) = save {
            self.write(Cmd::Save {
                id: entity.id,
                path: path.clone(),
                bytes: bytes(),
                expect,
            });
        }
        !waits
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
            CopyOf::Edited(file, edit) => Source::Edited(file.clone(), edit.clone()),
            CopyOf::Asset(source) => {
                let source = self.records.get(source).filter(|source| !source.saving);
                let found =
                    source.and_then(|source| Some((source.path.clone()?, source.fingerprint?)));
                match found {
                    Some((at, print)) if !unsettled(waiting, &at) => Source::Library(at, print),
                    _ => return false,
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
        record.saved = entity.saved.stamp;
        let path = record.path.get_or_insert_with(|| path.clone()).clone();
        let expect = record.fingerprint.filter(|_| !record.missing);
        self.write(Cmd::Import {
            id: entity.id,
            path,
            from,
            expect,
        });
        true
    }

    /// Write a working copy for an edit not yet saved, or drop the one a save made
    /// unnecessary. `edit` is the edit held over the file the asset rests in, and its
    /// stamp, which the copy keeps in place of the bytes.
    fn working(
        &mut self,
        entity: &LocalEntity,
        edit: Option<(&Edit, u64)>,
        queue: &Queue,
        writes: &mut Vec<(String, Vec<u8>)>,
        drops: &mut Vec<String>,
    ) {
        // A view is kept only while it holds something the slot does not.
        let unsaved = match entity.kept {
            true => entity.stamp != entity.saved.stamp,
            false => self.keeps_views && precious(entity, queue),
        };
        let needs = match edit {
            Some((_, stamp)) => Some((Keeps::Edit, stamp)),
            None => unsaved.then_some((Keeps::Bytes, entity.stamp)),
        };
        if !entity.kept && needs.is_some() && !self.records.contains_key(&entity.id) {
            self.records.insert(
                entity.id,
                Record {
                    path: None,
                    ..Record::of_file(LibPath::root(), None)
                },
            );
        }
        let Some(record) = self.records.get_mut(&entity.id) else {
            return;
        };
        let held = record
            .working
            .as_ref()
            .map(|held| (held.copy.keeps, held.stamp));
        match needs {
            Some(needs) if held == Some(needs) => {}
            Some((keeps, stamp)) => {
                let bytes = match edit {
                    Some((edit, _)) => edit.working(),
                    None => Ok(entity.bytes.to_vec()),
                };
                // An edit that does not write is left to the copy before it.
                let Ok(bytes) = bytes else {
                    return;
                };
                let generation = self.next_generation;
                self.next_generation += 1;
                writes.push((working_name(entity.id, generation), bytes));
                let copy = Working { generation, keeps };
                if let Some(old) = record.working.replace(Kept { copy, stamp }) {
                    drops.push(working_name(entity.id, old.copy.generation));
                }
            }
            None if !record.saving => {
                if let Some(old) = record.working.take() {
                    drops.push(working_name(entity.id, old.copy.generation));
                }
                if record.path.is_none() {
                    self.records.remove(&entity.id);
                }
            }
            None => {}
        }
    }

    /// Delete the files of assets the workspace no longer holds.
    fn forget_gone(&mut self, workspace: &Workspace, drops: &mut Vec<String>) {
        let gone: Vec<u64> = self
            .records
            .keys()
            .copied()
            .filter(|id| workspace.get(*id).is_none())
            .collect();
        for id in gone {
            // ⚠️ A file a copy is still to be made of waits until the copy answers: a
            // copy that fails has nothing else to make it from.
            if workspace.copies_of(id).next().is_some() {
                continue;
            }
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            // ⚠️ The file is deleted by the pass after its save answers, since only then
            // is its fingerprint known, and a pass is where nothing is sent while a rescan
            // is in flight.
            if record.saving {
                continue;
            }
            if let Some(old) = record.working.take() {
                drops.push(working_name(id, old.copy.generation));
            }
            let file = (record.path.clone(), record.fingerprint, record.missing);
            self.records.remove(&id);
            if let Some(path) = &file.0 {
                self.cache.forget(path);
            }
            if let (Some(path), Some(expect), false) = file {
                self.write(Cmd::RemoveFile { path, expect });
            }
        }
    }

    fn sidecar(&self, workspace: &Workspace, browser: &Browser) -> Sidecar {
        let mut assets = BTreeMap::new();
        for (id, record) in &self.records {
            let Some(entity) = workspace.get(*id) else {
                continue;
            };
            assets.insert(
                *id,
                Row {
                    path: record.path.clone(),
                    name: entity.name.clone(),
                    fingerprint: record.fingerprint,
                    tags: browser.tags.worn(*id).clone(),
                    origin: (&entity.origin).into(),
                    working: record.working.as_ref().map(|held| held.copy),
                },
            );
        }
        for lost in browser.folders.lost() {
            let mut row = lost.row.clone();
            row.tags = browser.tags.worn(lost.id).clone();
            assets.insert(lost.id, row);
        }
        // ⚠️ An index written before the listing is complete keeps the rows no file has
        // claimed yet, or a quit while opening would forget them.
        let unclaimed = self.loading.iter().flat_map(|loading| {
            let claimed = &loading.claimed;
            loading.rows.iter().filter(|(id, _)| !claimed.contains(*id))
        });
        for (id, row) in unclaimed {
            let mut row = row.clone();
            row.tags = browser.tags.worn(*id).clone();
            assets.entry(*id).or_insert(row);
        }
        Sidecar {
            version: VERSION,
            next_id: workspace.next_id(),
            next_generation: self.next_generation,
            tags: browser
                .tags
                .all()
                .iter()
                .map(|tag| (tag.id, tag.name.clone()))
                .collect(),
            assets,
        }
    }
}

impl Record {
    fn of_file(path: LibPath, fingerprint: impl Into<Option<Fingerprint>>) -> Record {
        Record {
            path: Some(path),
            fingerprint: fingerprint.into(),
            saved: 0,
            working: None,
            saving: false,
            missing: false,
            holds: Holds::Whole,
            summarized: None,
        }
    }

    /// Whether drawbar holds its file whole as last read or written, under the baseline
    /// stamped `saved`, with no save, working copy or rename over it: what it holds can
    /// be read again from the file.
    fn rereadable(&self, saved: u64, moving: &[(LibPath, LibPath)]) -> bool {
        let still = self.saved == saved && self.fingerprint.is_some();
        let settled = self
            .path
            .as_ref()
            .is_some_and(|path| !unsettled(moving, path));
        let idle = !self.saving && !self.missing && self.working.is_none();
        self.holds == Holds::Whole && still && settled && idle
    }

    /// Whether its file is one drawbar knows, has not seen go missing, and holds
    /// contents whose CRC was never taken.
    fn unprinted(&self) -> bool {
        let print = self.fingerprint.filter(|print| print.crc.is_none());
        self.path.is_some() && print.is_some() && !self.missing
    }

    /// Recorded from what a listing found.
    fn of_found(found: &Found, fingerprint: Fingerprint) -> Record {
        Record {
            holds: found.holds(),
            ..Record::of_file(found.path.clone(), fingerprint)
        }
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
    /// Only those the index does not track, for a read in the background.
    Untracked,
}

/// Whether the index holds something about this asset that no file says: a tag, an
/// unsaved edit kept as a working copy, or the slot it came off.
fn tracked(id: u64, record: &Record, workspace: &Workspace, browser: &Browser) -> bool {
    let slot = workspace
        .get(id)
        .is_some_and(|entity| entity.origin.slot().is_some());
    slot || record.working.is_some() || !browser.tags.worn(id).is_empty()
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

/// Say what is new in what a listing found beside the assets: a file that did not read,
/// and a library too large to list whole.
fn beside(
    folders: &crate::folders::Folders,
    unread: &[(LibPath, String)],
    unwalked: &[LibPath],
    log: &mut Log,
) {
    for (path, why) in unread {
        if !folders.unread.contains(&(path.clone(), why.clone())) {
            log.warn(format!("{path}: {why}"));
        }
    }
    if !unwalked.is_empty() && folders.unwalked.is_empty() {
        log.say(
            "Some of the library folder is not listed: it holds more than drawbar lists, or \
             folders drawbar cannot read.",
        );
    }
}

/// Whether an index holds something no file in the library says: a tag, an unsaved edit,
/// or the slot an asset came off.
fn beyond_files(sidecar: &Sidecar) -> bool {
    !sidecar.tags.is_empty() || sidecar.assets.values().any(Row::precious)
}

/// Flag the entries of one folder whose names are one name under [`names::key`], and
/// warn once about each new pair. Nothing is renamed: renaming a user's files on open
/// would be hostile, and either name may be the one other files refer to.
fn flag_duplicates(workspace: &Workspace, browser: &mut Browser, log: &mut Log) {
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
            "{} in {place} are one name on a disk that ignores case; rename one of them.",
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

/// A file drawbar did not know, recorded under `id`, and the asset it becomes.
fn newcomer(id: u64, found: Found, records: &mut BTreeMap<u64, Record>) -> Saved {
    records.insert(id, Record::of_found(&found, found.fingerprint()));
    Saved {
        id,
        name: found.path.leaf().to_string(),
        origin: Origin::File(found.path.leaf().to_string()),
        unread: (!found.read()).then_some(found.stat.len),
        path: Some(found.path),
        saved: found.bytes.unwrap_or_default(),
        file: found.file,
        unsaved: None,
    }
}

/// An asset brought back from its working copy alone: the working copy is all there is,
/// so it is also what the asset counts as saved.
fn saved_from(id: u64, row: &Row, bytes: Vec<u8>) -> Saved {
    let name = match &row.path {
        Some(path) => path.leaf().to_string(),
        None => row.name.clone(),
    };
    Saved {
        id,
        name,
        path: row.path.clone(),
        origin: Origin::from(&row.origin),
        saved: bytes,
        file: None,
        unread: None,
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
