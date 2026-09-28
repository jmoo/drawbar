//! The app's side of a library: what the files hold as far as drawbar knows, the
//! commands that bring them level with the workspace, and the answers folded back in.
//!
//! The workspace stays the model the rest of the app edits. [`Store::sync`] compares it
//! with what the files were last known to hold and sends what differs: a new asset's
//! file, a save, a deletion, a working copy, the index. Folder changes arrive in order
//! from [`crate::folders::Folders`], because a rename of a folder cannot be told from a
//! move of everything in it by comparing the two states.

use std::collections::{BTreeMap, BTreeSet};

use super::diff::{match_files, Known};
use super::exec::working_name;
use super::sidecar::{Row, Sidecar, VERSION};
use super::{names, Backend, Cmd, Event, Failure, Fingerprint, Found, LibPath, Listing, Opened};
use crate::browser::Browser;
use crate::folders::{Op, Where};
use crate::log::Log;
use crate::queue::Queue;
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
    working: Option<Working>,
    /// A save has been sent and not answered.
    saving: bool,
    /// The file went missing outside drawbar. Nothing writes it again until the asset
    /// is saved.
    missing: bool,
    /// The last listing that read the file left it resting in place rather than read it
    /// whole, and nothing has been saved over it since.
    rests: bool,
    /// The asset left the workspace while its first save was in flight; the file goes
    /// once that save lands.
    removed: bool,
}

/// An unsaved edit's bytes, as written to `working/`.
struct Working {
    generation: u64,
    /// The [`LocalEntity::stamp`] of the bytes it holds.
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
    scanning: bool,
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
            send_waits: false,
            keeps_views: true,
            name: None,
            stale: Vec::new(),
        }
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

    /// Whether no save or rescan waits for its answer, so a library whose answers cannot
    /// be waited for can be let go without losing one.
    pub fn settled(&self) -> bool {
        !self.scanning && !self.records.values().any(|record| record.saving)
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
            opening: matches!(self.phase, Phase::Opening),
        }
    }

    /// Fold in whatever the backend has answered. Returns whether a send held by
    /// [`Store::hold_send`] may now go ahead.
    pub fn poll(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) -> bool {
        let mut released = false;
        while let Some(event) = self.backend.try_recv() {
            released |= self.handle(event, workspace, browser, queue, log);
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
        let Some(event) = self.backend.recv() else {
            return false;
        };
        self.handle(event, workspace, browser, queue, log);
        browser.folders.place = Some(self.place());
        true
    }

    /// Whether a rescan is waiting for its answer.
    #[cfg(test)]
    pub fn scanning(&self) -> bool {
        self.scanning
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
        match event {
            Event::Opened(Ok(opened)) => self.load(opened, workspace, browser, log),
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
            Event::Scanned(Err(why)) => {
                self.scanning = false;
                self.send_waits = false;
                log.error(format!("reading the library again: {why}"));
                log.trouble("The library folder could not be read, so nothing was sent.");
            }
            Event::Saved { id, path, result } => {
                self.saved(id, path, result, workspace, browser, log)
            }
            Event::ReadOnly(why) => self.refused(why, workspace, log),
            Event::Failed(why) => {
                log.error(why.clone());
                log.trouble(format!(
                    "The library folder did not change as asked: {why}."
                ));
                // Whatever failed may have been the index or a working copy, so both are
                // written again, whole, at the next full pass.
                self.committed = None;
                for record in self.records.values_mut() {
                    record.working = None;
                }
                self.rescan();
            }
        }
        false
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
                workspace.unsave(*id, log);
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
        self.backend.send(cmd);
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

    pub(crate) fn rescan(&mut self) {
        if !self.opened() || self.scanning {
            return;
        }
        let mut known = BTreeMap::new();
        let mut resting = BTreeSet::new();
        for record in self.records.values() {
            let (Some(path), Some(print)) = (&record.path, record.fingerprint) else {
                continue;
            };
            if record.rests {
                resting.insert(path.clone());
            }
            known.insert(path.clone(), print.stat());
        }
        self.scanning = true;
        self.backend.send(Cmd::Scan { known, resting });
    }

    /// Write everything, waiting for the saves in flight to answer, then let the library
    /// go. It blocks, so it is for the end of a session.
    pub fn close(
        &mut self,
        workspace: &mut Workspace,
        browser: &mut Browser,
        queue: &Queue,
        log: &mut Log,
    ) {
        // Each round sends the saves that waited on the round before.
        for _ in 0..3 {
            if self.sync(workspace, browser, queue, Pass::Last) {
                break;
            }
            while self.records.values().any(|record| record.saving) {
                let Some(event) = self.backend.recv() else {
                    break;
                };
                self.handle(event, workspace, browser, queue, log);
            }
        }
        self.backend.finish();
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

    fn load(
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
            listing,
            mut working,
            swept,
        } = opened;
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
        if swept > 0 {
            log.info(format!("removed {swept} leftovers of interrupted writes"));
        }
        let Listing {
            dirs,
            files,
            unread,
            others,
            unwalked,
        } = listing;
        beside(&mut browser.folders, unread, others, unwalked, log);
        self.next_generation = sidecar.next_generation.max(1);
        let Sidecar { tags, assets, .. } = sidecar;

        // A row under an id this session has already given out, to an asset made while
        // the library was opening or one of a library open before, takes the next free
        // one, so nothing still naming that id reaches this asset.
        let floor = workspace.next_id();
        let mut next = floor
            .max(sidecar.next_id)
            .max(assets.keys().max().map_or(0, |id| id.saturating_add(1)));
        let mut fresh = || {
            let id = next;
            next = next.saturating_add(1);
            id
        };
        let mut rows: BTreeMap<u64, Row> = BTreeMap::new();
        // The new id of each asset that moved with a working copy, and that copy's file.
        let mut renamed = Vec::new();
        for (id, row) in assets {
            if id >= floor {
                rows.insert(id, row);
                continue;
            }
            let moved = fresh();
            if let Some(bytes) = working.remove(&id) {
                working.insert(moved, bytes);
            }
            if let Some(generation) = row.working {
                renamed.push((moved, working_name(id, generation)));
            }
            rows.insert(moved, row);
        }

        let known = rows
            .iter()
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
        let matched = match_files(&known, files);
        let mut back = Vec::new();
        let mut conflicts = Vec::new();
        let mut missing = Vec::new();
        let on_disk = matched.same.into_iter().chain(matched.renamed);
        let changed = matched
            .changed
            .into_iter()
            .map(|(id, found)| (id, found, true));
        for (id, found, changed) in on_disk.map(|(id, found)| (id, found, false)).chain(changed) {
            let (Some(row), Some(print)) = (rows.get(&id), found.fingerprint()) else {
                continue;
            };
            let mine = working.remove(&id).filter(|mine| match &found.bytes {
                Some(bytes) => mine != bytes,
                None => !print.holds(mine),
            });
            if changed && mine.is_some() {
                conflicts.push(id);
            }
            self.records.insert(id, Record::of_found(&found, print));
            back.push(Saved {
                id,
                name: found.path.leaf().to_string(),
                path: Some(found.path),
                origin: Origin::from(&row.origin),
                saved: found.bytes.unwrap_or_default(),
                file: found.file,
                unsaved: mine,
            });
        }
        for id in matched.vanished {
            let Some(row) = rows.get(&id) else {
                continue;
            };
            match working.remove(&id) {
                Some(bytes) => {
                    let mut record = Record::of_file(row.path.clone().unwrap_or_default(), None);
                    record.fingerprint = row.fingerprint;
                    record.missing = true;
                    self.records.insert(id, record);
                    missing.push(id);
                    back.push(saved_from(id, row, bytes));
                }
                None if row.precious() => browser.folders.lose(id, row.clone()),
                None => {}
            }
        }
        // Rows with no path are views of slots that held an edit; each comes back as a
        // file on this computer. Its working copy stays until that file is written.
        let mut viewed = Vec::new();
        for (id, row) in rows.iter().filter(|(_, row)| row.path.is_none()) {
            if let (Some(bytes), Some(generation)) = (working.remove(id), row.working) {
                viewed.push((*id, generation));
                back.push(saved_from(*id, row, bytes));
            }
        }
        for found in matched.arrived {
            let Some(saved) = newcomer(fresh(), found, &mut self.records) else {
                continue;
            };
            back.push(saved);
        }

        let ids: Vec<u64> = back.iter().map(|saved| saved.id).collect();
        let count = ids.len();
        workspace.restore(back, Some(next), log);
        for id in &missing {
            workspace.unsave(*id, log);
            browser.folders.missing.insert(*id);
        }
        for id in &ids {
            self.settle(*id, workspace, &rows);
        }
        for (id, generation) in viewed {
            let stamp = workspace.get(id).map_or(0, |entity| entity.stamp);
            let mut record = Record::of_file(LibPath::root(), None);
            record.path = None;
            record.working = Some(Working { generation, stamp });
            self.records.insert(id, record);
        }
        // A working copy is named by its asset's id, so one whose asset moved is written
        // again under the new id, and the old one dropped, at the next full pass.
        for (id, old) in renamed {
            if let Some(record) = self.records.get_mut(&id) {
                record.working = None;
            }
            self.stale.push(old);
        }
        browser.tags.restore(
            tags,
            rows.iter().map(|(id, row)| (*id, row.tags.iter().copied())),
        );
        browser.folders.sync(&dirs);
        flag_duplicates(workspace, browser, log);
        for id in conflicts {
            conflict(id, workspace, browser, log);
        }
        if count > 0 {
            log.say(match count {
                1 => "1 file on this computer.".to_string(),
                n => format!("{n} files on this computer."),
            });
        }
    }

    /// Record the stamps of an asset just restored, and the working copy it came back
    /// with, so nothing is written again for it.
    fn settle(&mut self, id: u64, workspace: &Workspace, rows: &BTreeMap<u64, Row>) {
        let (Some(entity), Some(record)) = (workspace.get(id), self.records.get_mut(&id)) else {
            return;
        };
        record.saved = entity.saved.stamp;
        let generation = rows.get(&id).and_then(|row| row.working);
        record.working = match (entity.is_unsaved(), generation) {
            (true, Some(generation)) => Some(Working {
                generation,
                stamp: entity.stamp,
            }),
            (false, Some(generation)) => {
                self.stale.push(working_name(id, generation));
                None
            }
            (_, None) => None,
        };
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
        beside(&mut browser.folders, unread, others, unwalked, log);
        let known = self
            .records
            .iter()
            .filter_map(|(id, record)| {
                Some((
                    *id,
                    Known {
                        path: record.path.clone()?,
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
            workspace.place(id, found.path.clone());
            if let Some(record) = self.records.get_mut(&id) {
                record.path = Some(found.path.clone());
                if let (Some(print), Some(read)) = (&mut record.fingerprint, found.fingerprint()) {
                    *print = read;
                    record.rests = found.file.is_some();
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
                let Some(saved) = newcomer(next, found, &mut self.records) else {
                    continue;
                };
                next = next.saturating_add(1);
                back.push(saved);
            }
            let ids: Vec<u64> = back.iter().map(|saved| saved.id).collect();
            log.say(match ids.len() {
                1 => "1 file appeared in the library folder.".to_string(),
                n => format!("{n} files appeared in the library folder."),
            });
            workspace.restore(back, Some(next), log);
            for id in ids {
                self.settle(id, workspace, &BTreeMap::new());
            }
        }
        browser.folders.sync(&dirs);
        flag_duplicates(workspace, browser, log);
        touched
    }

    /// A file drawbar knew now holds something else.
    fn changed(
        &mut self,
        id: u64,
        found: Found,
        workspace: &mut Workspace,
        browser: &mut Browser,
        log: &mut Log,
    ) {
        let (Some(print), Some(entity)) = (found.fingerprint(), workspace.get(id)) else {
            return;
        };
        let name = entity.name.clone();
        let unsaved = entity.is_unsaved();
        let rests = found.file.is_some();
        match (unsaved, found.bytes, found.file) {
            (true, Some(bytes), _) => workspace.rebase(id, bytes, log),
            (true, None, Some(file)) => workspace.rebase_file(id, file, log),
            (false, Some(bytes), _) => workspace.adopt(id, bytes, log),
            (false, None, Some(file)) => workspace.adopt_file(id, file),
            (_, None, None) => return,
        }
        match unsaved {
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
            record.rests = rests;
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
        self.records.remove(&id);
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
                record.rests = false;
                browser.folders.missing.remove(&id);
                if record.removed {
                    self.records.remove(&id);
                    self.write(Cmd::RemoveFile {
                        path,
                        expect: print,
                    });
                }
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
            Err(Failure::Io(why)) => why,
        };
        workspace.unsave(id, log);
        if let (Some(record), Some(entity)) = (self.records.get_mut(&id), workspace.get(id)) {
            record.saved = entity.saved.stamp;
        }
        log.error(format!("saving {path}: {why}"));
        log.trouble(format!(
            "“{name}” was not saved, because {why}. The edit is kept and still unsaved."
        ));
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
        if !self.open() {
            return true;
        }
        if self.scanning && pass != Pass::Last {
            return false;
        }
        let full = pass != Pass::Files;
        let ops = browser.folders.take_ops();
        let (removals, ops): (Vec<Op>, Vec<Op>) = ops
            .into_iter()
            .partition(|op| matches!(op, Op::RemoveDir(_)));
        for op in ops {
            self.tree_op(op);
        }
        crate::folders::place_new(workspace, &browser.folders);
        let mut writes = Vec::new();
        let mut drops = match full {
            true => std::mem::take(&mut self.stale),
            false => Vec::new(),
        };
        let mut done = true;
        for entity in workspace.entities() {
            done &= self.file(entity);
            if full {
                self.working(entity, queue, &mut writes, &mut drops);
            }
        }
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
        done
    }

    fn tree_op(&mut self, op: Op) {
        match op {
            Op::MakeDir(path) => self.write(Cmd::MakeDir(path)),
            Op::RemoveDir(path) => self.write(Cmd::RemoveDir(path)),
            Op::MoveDir { from, to } => {
                for record in self.records.values_mut() {
                    if let Some(moved) = record.path.as_ref().and_then(|at| at.moved(&from, &to)) {
                        record.path = Some(moved);
                    }
                }
                self.write(Cmd::Move { from, to });
            }
        }
    }

    /// Send what one asset's file needs: its first write, a move, or a save. Returns
    /// `false` when a save has to wait.
    fn file(&mut self, entity: &LocalEntity) -> bool {
        let (true, Some(path)) = (entity.kept, &entity.path) else {
            return true;
        };
        let bytes = || entity.saved.bytes.clone();
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
            self.write(Cmd::Move {
                from,
                to: path.clone(),
            });
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

    /// Write a working copy for an edit not yet saved, or drop the one a save made
    /// unnecessary.
    fn working(
        &mut self,
        entity: &LocalEntity,
        queue: &Queue,
        writes: &mut Vec<(String, Vec<u8>)>,
        drops: &mut Vec<String>,
    ) {
        // A view is kept only while it holds something the slot does not.
        let needs = match entity.kept {
            true => entity.stamp != entity.saved.stamp,
            false => self.keeps_views && precious(entity, queue),
        };
        if !entity.kept && needs && !self.records.contains_key(&entity.id) {
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
        match (needs, &record.working) {
            (true, Some(held)) if held.stamp == entity.stamp => {}
            (true, _) => {
                let generation = self.next_generation;
                self.next_generation += 1;
                writes.push((working_name(entity.id, generation), entity.bytes.clone()));
                if let Some(old) = record.working.replace(Working {
                    generation,
                    stamp: entity.stamp,
                }) {
                    drops.push(working_name(entity.id, old.generation));
                }
            }
            (false, Some(_)) if !record.saving => {
                if let Some(old) = record.working.take() {
                    drops.push(working_name(entity.id, old.generation));
                }
                if record.path.is_none() {
                    self.records.remove(&entity.id);
                }
            }
            (false, _) => {}
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
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            if record.saving {
                record.removed = true;
                continue;
            }
            if let Some(old) = record.working.take() {
                drops.push(working_name(id, old.generation));
            }
            let file = (record.path.clone(), record.fingerprint, record.missing);
            self.records.remove(&id);
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
                    working: record.working.as_ref().map(|held| held.generation),
                },
            );
        }
        for lost in browser.folders.lost() {
            let mut row = lost.row.clone();
            row.tags = browser.tags.worn(lost.id).clone();
            assets.insert(lost.id, row);
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
            removed: false,
            rests: false,
        }
    }

    /// Recorded from what a listing read.
    fn of_found(found: &Found, fingerprint: Fingerprint) -> Record {
        Record {
            rests: found.file.is_some(),
            ..Record::of_file(found.path.clone(), fingerprint)
        }
    }
}

/// Take what a listing found beside the assets, and say what is new in it: a file that
/// did not read, and a library too large to list whole.
fn beside(
    folders: &mut crate::folders::Folders,
    unread: Vec<(LibPath, String)>,
    others: Vec<LibPath>,
    unwalked: Vec<LibPath>,
    log: &mut Log,
) {
    for (path, why) in &unread {
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
    folders.unread = unread;
    folders.others = others;
    folders.unwalked = unwalked.into_iter().collect();
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

/// A file drawbar did not know, recorded under `id`, and the asset it becomes. `None` for
/// a file the listing did not read.
fn newcomer(id: u64, found: Found, records: &mut BTreeMap<u64, Record>) -> Option<Saved> {
    let print = found.fingerprint()?;
    records.insert(id, Record::of_found(&found, print));
    Some(Saved {
        id,
        name: found.path.leaf().to_string(),
        origin: Origin::File(found.path.leaf().to_string()),
        path: Some(found.path),
        saved: found.bytes.unwrap_or_default(),
        file: found.file,
        unsaved: None,
    })
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
    browser.ask_conflict(id, &entity.name);
}
