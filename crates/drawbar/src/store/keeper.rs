//! A library on the desktop: toshokan's blocking driver, run on the backend's thread.
//! Every change the app asks for is an intent here, and every file it writes is filled
//! from a [`Fill`] as toshokan stages it.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use toshokan::blocking::{Backend, Library, Native, Source as Filled, Staging};
use toshokan::env::{OsRandom, SystemClock};
use toshokan::error::Refusal;
use toshokan::io::IoError;
use toshokan::io::{Capabilities, Io, IoResult, Kind, Reply, Root};
use toshokan::log::Settlement;
use toshokan::report::{By, Mode, Refreshed};
use toshokan::{EntryHash, Env, Error, Expect, Policy, RelPath, View};

use super::keys::{self, Came, ORIGIN, TAGS, TAG_NAMES};
use super::native::locate;
use super::shown::Change;
use super::{
    Cmd, Event, Failure, Fingerprint, Found, Intent, Kept, Key, LibPath, Moving, Opened, Part,
    Source, Stat, Viewed,
};
use crate::ondisk::OnDisk;
use crate::rewrite::{self, Rewrite};

/// The most entries a listing of the library's folders looks at: a guard against a
/// folder no library is, such as a whole disk.
const MOST_ENTRIES: usize = 1_000_000;

/// What other writers see this one called. The folder may be shared, so it says no
/// more than which app wrote.
const LABEL: &str = "drawbar";

/// How much of a file a copy reads at once.
const CHUNK: usize = 4 << 20;

pub struct Keeper {
    folder: PathBuf,
    /// This install's own state for the library, never synced.
    local: PathBuf,
    library: Option<Library<Counted>>,
    /// The time the library is opened by.
    clock: fn() -> Box<dyn toshokan::Clock>,
    /// How many files the listings of the folder have found.
    listed: Arc<AtomicUsize>,
    /// The entries this session logged, whose undo ends with it.
    session: BTreeSet<EntryHash>,
    /// The most bytes this writer's trash holds before its oldest go.
    trash: u64,
    /// The view the app was last shown.
    shown: Option<View>,
}

/// The most bytes a writer's trash holds in a session: the size toshokan's policy
/// keeps.
const TRASH: u64 = 1 << 30;

impl Keeper {
    /// The library in `folder`, with this install's own state for it in `local`, counting
    /// into `listed` the files its listings find.
    pub fn new(folder: PathBuf, local: PathBuf, listed: Arc<AtomicUsize>) -> Keeper {
        Keeper {
            folder,
            local,
            library: None,
            clock: || Box::new(SystemClock),
            listed,
            session: BTreeSet::new(),
            trash: TRASH,
            shown: None,
        }
    }

    /// Run a command on the library, and answer it.
    pub fn run(&mut self, cmd: Cmd, answer: &mut impl FnMut(Event)) {
        match cmd {
            Cmd::Open => answer(Event::Opened(self.open())),
            Cmd::Rescan => answer(Event::Scanned(self.scanned(Library::rescan, true))),
            Cmd::Refresh => answer(Event::Scanned(self.scanned(Library::refresh, false))),
            Cmd::Intent(intent) => {
                let label = intent.label().to_string();
                let committed = self.commit(intent, answer);
                self.made(label, committed, answer);
            }
            Cmd::Undo => {
                let undone = self.undone(Library::undo);
                self.made("Undo".to_string(), undone, answer);
            }
            Cmd::Redo => {
                let redone = self.undone(Library::redo);
                self.made("Redo".to_string(), redone, answer);
            }
            Cmd::Settle { orphan, how } => {
                let settled = self.settled(orphan, how);
                self.made("Settle".to_string(), settled, answer);
            }
            Cmd::Draft {
                of,
                base,
                bytes,
                replaces,
            } => {
                let kept = self.draft(&of, |draft| draft.put(base, bytes));
                let replaced = kept.and_then(|()| {
                    replaces
                        .into_iter()
                        .try_for_each(|replaced| self.discard(replaced))
                });
                if let Err(why) = replaced {
                    answer(Event::Unkept(why));
                }
            }
            Cmd::Discard(kept) => {
                if let Err(why) = self.discard(kept) {
                    answer(Event::Unkept(why));
                }
            }
            Cmd::KeepSlot { slot, base, bytes } => {
                if let Err(e) = super::slots::keep(&self.local, &slot, base, &bytes) {
                    answer(Event::Unkept(format!("keeping a slot's edit: {e}")));
                }
            }
            Cmd::DiscardSlot(slot) => {
                if let Err(e) = super::slots::discard(&self.local, &slot) {
                    answer(Event::Unkept(format!("letting go of a slot's edit: {e}")));
                }
            }
            Cmd::ReadSlot(slot) => {
                let kept = super::slots::read(&self.local, &slot).map_err(|e| e.to_string());
                answer(Event::SlotKept { slot, kept });
            }
            Cmd::Read { .. } => unreachable!("the backend reads files itself"),
        }
    }

    /// Answer a change, an undo or a redo, and keep this writer's trash within
    /// [`TRASH`] once it was made.
    fn made(
        &mut self,
        label: String,
        committed: Result<(Option<EntryHash>, Viewed), String>,
        answer: &mut impl FnMut(Event),
    ) {
        let made = committed.as_ref().ok().and_then(|(made, _)| *made);
        self.session.extend(made);
        answer(Event::Committed {
            label,
            made,
            result: committed.map(|(_, viewed)| viewed),
        });
        if made.is_some() {
            if let Some(trimmed) = self.trim() {
                answer(trimmed);
            }
        }
    }

    /// Empty the oldest of this writer's trash where it holds more than [`TRASH`], and
    /// say which changes' undo needed what went.
    fn trim(&mut self) -> Option<Event> {
        let library = self.library.as_mut()?;
        let held = library.trash().ok()?;
        let bytes = held
            .iter()
            .map(|item| item.len)
            .fold(0, u64::saturating_add);
        if bytes <= self.trash {
            return None;
        }
        let policy = Policy {
            max_age_ms: u64::MAX,
            max_bytes: self.trash,
        };
        let emptied = match library.empty_trash(policy) {
            Ok(emptied) => emptied,
            Err(e) => return Some(Event::Unkept(format!("emptying the trash: {e}"))),
        };
        let gone = held
            .iter()
            .filter(|item| emptied.removed.contains(&item.item));
        Some(Event::Trimmed(gone.map(|item| item.by).collect()))
    }

    /// Let go of a draft this instance's writer, or an idle writer of the pool, kept.
    fn discard(&mut self, kept: Kept) -> Result<(), String> {
        let Some(writer) = kept.pooled else {
            return self.draft(&kept.of, |draft| draft.discard());
        };
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let of = match &kept.of {
            Key::Entity(entity) => toshokan::Drafted::Entity(*entity),
            Key::File(path) => {
                toshokan::Drafted::File(relative(path).ok_or_else(|| no_path(path))?)
            }
        };
        library
            .discard_pooled_draft(writer, of)
            .map_err(|e| e.to_string())
    }

    /// Put or let go of the draft of the file `of` names.
    fn draft(
        &mut self,
        of: &Key,
        change: impl FnOnce(toshokan::drafts::Draft<&mut Library<Counted>>) -> toshokan::Result<()>,
    ) -> Result<(), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let draft = match of {
            Key::Entity(entity) => library.draft(*entity),
            Key::File(path) => library.draft_file(&relative(path).ok_or_else(|| no_path(path))?),
        };
        change(draft).map_err(|e| e.to_string())
    }

    /// Let the library go cleanly, once every command has run: this session's undo
    /// ends now, and the trash lets go of what only it needed.
    pub fn close(&mut self) {
        let Some(mut library) = self.library.take() else {
            return;
        };
        if let Ok(held) = library.trash() {
            let ended = held
                .into_iter()
                .filter(|item| self.session.contains(&item.by));
            let _ = library.remove_trash(ended.map(|item| item.item).collect());
        }
        let _ = library.close();
    }

    fn open(&mut self) -> Result<Opened, String> {
        let env = Env {
            clock: (self.clock)(),
            random: Box::new(OsRandom::new()),
            identify: Rc::new(keys::Contents),
            names: Box::new(keys::Volume::probe(&self.folder)),
            label: LABEL.to_string(),
        };
        let native = Counted {
            native: Native::new(&self.folder, &self.local),
            listed: self.listed.clone(),
        };
        let (mut library, opened) = Library::open(native, keys::layout(), &keys::schema(), env)
            .map_err(|e| e.to_string())?;
        let writable = match &opened.mode {
            Mode::Writable => Ok(()),
            Mode::ReadOnly(why) => Err(why.to_string()),
        };
        let mut trouble = Vec::new();
        let let_go = match self.restored(&opened) {
            true => opened.removed.len(),
            false => 0,
        };
        if let_go > 0 {
            library.let_go().map_err(|e| e.to_string())?;
        }
        let mut unfinished = Vec::new();
        if writable.is_ok() {
            // Also what a crash may have torn from the end of this writer's log, which
            // opening does not report.
            let settled = library.settle_interrupted();
            trouble.extend(settled.err().map(|e| format!("finishing a change: {e}")));
            // Another window of this install left these: they are ours to finish.
            for orphan in opened.orphaned {
                match orphan.ours {
                    true => {
                        let settled = library.settle(orphan, Settlement::Finished);
                        trouble.extend(settled.err().map(|e| format!("finishing a change: {e}")));
                    }
                    false => unfinished.push(orphan),
                }
            }
            // Nothing an earlier session displaced can be undone now.
            let emptied = library.empty_trash(Policy::default());
            trouble.extend(emptied.err().map(|e| format!("emptying the trash: {e}")));
        }
        let refreshed = library.rescan().map_err(|e| e.to_string())?;
        let dirs = folders(&self.folder);
        let viewed = viewed(&library, &mut self.shown, refreshed, Some(dirs));
        self.library = Some(library);
        Ok(Opened {
            writable,
            viewed,
            drafts: opened.drafts,
            let_go,
            unfinished,
            ignored: opened.ignored.len(),
            trouble,
        })
    }

    /// Whether what this install showed and the folder no longer holds was taken by a
    /// restore of the folder, rather than not delivered yet by a sync: every writer
    /// whose entries went has its directory in the folder.
    fn restored(&self, opened: &toshokan::Opened) -> bool {
        let layout = keys::layout();
        let writers = opened.removed.iter().filter_map(|change| match &change.by {
            By::Writer { writer, .. } => Some(*writer),
            By::This | By::Outside => None,
        });
        let present = |writer| {
            let mut at = self.folder.clone();
            at.extend(layout.writer(writer).components());
            at.is_dir()
        };
        writers.collect::<BTreeSet<_>>().into_iter().all(present)
    }

    fn scanned(
        &mut self,
        scan: fn(&mut Library<Counted>) -> toshokan::Result<Refreshed>,
        whole: bool,
    ) -> Result<Viewed, String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let refreshed = scan(library).map_err(|e| e.to_string())?;
        let dirs = whole.then(|| folders(&self.folder));
        Ok(viewed(library, &mut self.shown, refreshed, dirs))
    }

    /// Undo or redo, and the view after it, with the files no entity is bound to that it
    /// moved for the app to follow.
    fn undone(
        &mut self,
        commit: fn(&mut Library<Counted>) -> toshokan::Result<toshokan::Committed>,
    ) -> Result<(Option<EntryHash>, Viewed), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let committed = commit(library).map_err(|e| e.to_string())?;
        let path = |path: Option<RelPath>| path.and_then(|path| LibPath::parse(path.as_str()));
        let trees = committed.trees.into_iter();
        let trees = trees.map(|(from, to)| (path(Some(from)), path(Some(to))));
        let paths = committed.paths.into_iter();
        let paths = paths.map(|moved| (path(moved.from), path(moved.to)));
        let paths = trees.chain(paths);
        let viewed = Viewed {
            undone: Some(paths.collect()),
            dirs: Some(folders(&self.folder)),
            ..committed_view(library, &mut self.shown, committed.changes)
        };
        Ok((Some(committed.intent), viewed))
    }

    /// Settle another writer's unfinished change, and the view after it.
    fn settled(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<(Option<EntryHash>, Viewed), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let committed = library.settle(orphan, how).map_err(|e| e.to_string())?;
        let viewed = Viewed {
            dirs: Some(folders(&self.folder)),
            ..committed_view(library, &mut self.shown, committed.changes)
        };
        Ok((Some(committed.intent), viewed))
    }

    /// Commit an intent, and say which entry logged it, where it was made. A change to a
    /// file answers how it went first, and is answered here with the view as it stands,
    /// whether it was made or not.
    fn commit(
        &mut self,
        intent: Intent,
        answer: &mut impl FnMut(Event),
    ) -> Result<(Option<EntryHash>, Viewed), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let label = intent.label();
        let changes = match intent {
            Intent::Tag { files, name, make } => {
                // ⚠️ A file another drawbar made an entity of since the last refresh would
                // be adopted twice, as two entities in conflict. Reading what others wrote
                // first leaves that only to one that adopts it in the moment between.
                let mut changes = Vec::new();
                if files.iter().any(|key| matches!(key, Key::File(_))) {
                    changes = library.refresh().map_err(|e| e.to_string())?.changes;
                }
                let mut committed = facts(library, label, |built, view| {
                    let built = tag(built, view, &files, &name)?;
                    Ok(match make {
                        true => list_tag(built, view, name),
                        false => built,
                    })
                })?;
                changes.append(&mut committed.changes);
                Some(toshokan::Committed {
                    changes,
                    ..committed
                })
            }
            Intent::Untag { entities, name } => Some(facts(library, label, |mut built, _| {
                for entity in entities {
                    built = built.remove(entity, TAGS, &name);
                }
                Ok(built)
            })?),
            Intent::MakeTag { name } => Some(facts(library, label, |built, view| {
                Ok(list_tag(built, view, name))
            })?),
            Intent::RenameTag { entities, from, to } => {
                Some(facts(library, label, |mut built, view| {
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
                })?)
            }
            Intent::RemoveTag { entities, name } => {
                Some(facts(library, label, |mut built, view| {
                    for entity in entities {
                        built = built.remove(entity, TAGS, &name);
                    }
                    for list in view.find(TAG_NAMES, &name) {
                        built = built.remove(list, TAG_NAMES, &name);
                    }
                    Ok(built)
                })?)
            }
            Intent::Forget { entity } => {
                Some(facts(library, label, |built, _| Ok(built.delete(entity)))?)
            }
            Intent::Save {
                id,
                path,
                bytes,
                expect,
                came,
            } => {
                let crc = nord_format::crc::crc32(&bytes);
                let identity = keys::identity_of_bytes(&path, &bytes);
                let to = To {
                    path: &path,
                    expect,
                    came,
                };
                let wrote = write(library, &self.folder, label, to, Fill::Bytes(bytes));
                let result = wrote.map(|(stat, changes)| {
                    let print = Fingerprint {
                        crc: Some(crc),
                        identity,
                        ..Fingerprint::unread(stat)
                    };
                    (print, changes)
                });
                answer_with(answer, result, |result| Event::Saved { id, path, result })
            }
            Intent::Rewrite {
                id,
                path,
                from,
                edit,
                expect,
            } => {
                let fill = Fill::Edited(from, edit);
                let to = To {
                    path: &path,
                    expect,
                    came: None,
                };
                let wrote = write(library, &self.folder, label, to, fill);
                let made = wrote.as_ref().ok().map(|(_, changes)| changes.clone());
                let result = wrote.and_then(|_| landed(&self.folder, &path));
                answer(Event::Rewritten { id, path, result });
                made
            }
            Intent::Import {
                id,
                path,
                from,
                expect,
                came,
            } => {
                let to = To {
                    path: &path,
                    expect,
                    came,
                };
                let copied = import(library, &self.folder, label, to, from);
                let made = copied.as_ref().ok().cloned();
                let result = copied.and_then(|_| landed(&self.folder, &path));
                answer(Event::Imported { id, path, result });
                made
            }
            Intent::Move { moves } => {
                let moved = facts(library, label, |mut built, view| {
                    for moving in &moves {
                        built = rename(built, view, moving)?;
                    }
                    Ok(built)
                });
                let (result, changes) = split(moved);
                for Moving { from, to, .. } in moves {
                    let result = result.clone();
                    answer(Event::Moved { from, to, result });
                }
                changes
            }
            Intent::MoveFolder { from, to } => {
                let moved = facts(library, label, |built, _| {
                    let at = |path: &LibPath| relative(path).ok_or_else(|| no_path(path));
                    Ok(built.move_tree(&at(&from)?, &at(&to)?))
                });
                let (result, changes) = split(moved);
                answer(Event::Moved { from, to, result });
                changes
            }
            Intent::MakeDir { path } => {
                let made = facts(library, label, |built, _| {
                    Ok(built.make_dir(&relative(&path).ok_or_else(|| no_path(&path))?))
                });
                made.map_or_else(
                    |why| {
                        answer(Event::Failed(format!("making {path}: {why}")));
                        None
                    },
                    Some,
                )
            }
            Intent::RemoveDir { path } => {
                let removed = facts(library, label, |built, _| {
                    Ok(built.remove_dir(&relative(&path).ok_or_else(|| no_path(&path))?))
                });
                removed.map_or_else(
                    |why| {
                        answer(Event::Failed(format!("removing {path}: {why}")));
                        None
                    },
                    Some,
                )
            }
            Intent::Batch(parts) => batch(library, &self.folder, label, parts, answer),
            Intent::Delete { files } => {
                let trashed = facts(library, label, |mut built, view| {
                    for (path, expect) in &files {
                        let at = relative(path).ok_or_else(|| no_path(path))?;
                        built = match view.at(&at).map(|entity| entity.id()) {
                            Some(entity) => built.trash(entity, *expect).delete(entity),
                            None => built.trash_file(&at, *expect),
                        };
                    }
                    Ok(built)
                });
                trashed.map_or_else(
                    |why| {
                        let paths: Vec<String> =
                            files.iter().map(|(path, _)| path.to_string()).collect();
                        let paths = paths.join(", ");
                        answer(Event::Failed(format!("deleting {paths}: {why}")));
                        None
                    },
                    Some,
                )
            }
        };
        let intent = changes.as_ref().map(|committed| committed.intent);
        let changes = changes.map(|committed| committed.changes);
        Ok((
            intent,
            committed_view(library, &mut self.shown, changes.unwrap_or_default()),
        ))
    }
}

/// toshokan's native backend, counting the files its listings of the folder find, so
/// opening can say how far it has got.
struct Counted {
    native: Native,
    listed: Arc<AtomicUsize>,
}

impl Backend for Counted {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.native.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let folder = io.root() == Root::Folder;
        let reply = self.native.perform(io);
        let found = match &reply {
            Ok(Reply::ListedStat(entries)) if folder => entries
                .iter()
                .filter(|(name, meta)| meta.kind == Kind::File && !name.starts_with('.'))
                .count(),
            _ => 0,
        };
        self.listed.fetch_add(found, Ordering::Relaxed);
        reply
    }
}

type Built<'l> = toshokan::intent::Intent<&'l mut Library<Counted>>;

/// Commit an intent labeled `label` that `build` makes against the view as it stands,
/// and return what it changed.
fn facts<'a>(
    library: &'a mut Library<Counted>,
    label: &str,
    build: impl FnOnce(Built<'a>, &View) -> Result<Built<'a>, String>,
) -> Result<toshokan::Committed, String> {
    let view = library.view();
    let built = build(library.intent(label), &view)?;
    let committed = built.commit().map_err(|e| e.to_string())?;
    Ok(committed)
}

/// Add the tag `name` to the library's list of tags.
fn list_tag<'l>(built: Built<'l>, view: &View, name: String) -> Built<'l> {
    match view.with(TAG_NAMES).into_iter().min() {
        Some(list) => built.add(list, TAG_NAMES, name),
        None => {
            built
                .create(|list| {
                    list.add(TAG_NAMES, name);
                })
                .0
        }
    }
}

/// Put a tag on each file of `files`: on its entity, the one bound to its path in `view`
/// where another writer made one since the app looked, or a new one that adopts it.
fn tag<'l>(
    mut built: Built<'l>,
    view: &View,
    files: &[Key],
    name: &str,
) -> Result<Built<'l>, String> {
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

/// Write the file `to` names from `fill`: as the file of the entity bound there, as a new
/// entity's file where `to` says where it came from, or as one no entity is bound to.
/// Returns the file's stat as it is found there, and what the commit changed.
fn write(
    library: &mut Library<Counted>,
    folder: &Path,
    label: &str,
    to: To<'_>,
    fill: Fill,
) -> Result<(Stat, toshokan::Committed), Failure> {
    let at = relative(to.path).ok_or_else(|| Failure::Io(no_path(to.path)))?;
    let view = library.view();
    let built = library.intent(label);
    let (built, failed) = save_part(built, &view, folder, &at, to, fill, &[]);
    let committed = built.commit().map_err(|e| failure(e, failed.take()))?;
    let stat = stat_at(folder, to.path)?;
    Ok((stat, committed))
}

/// What a fill that failed found, kept to say why its write did not land.
type FillFailure = Rc<RefCell<Option<io::Error>>>;

/// `built` with the write of the file `to` names from `fill`: as the file of the
/// entity bound there, as a new entity's file where `to` says where it came from, or
/// as one no entity is bound to, unless `tags` are to go on it, which makes it an
/// entity's.
fn save_part<'l>(
    built: Built<'l>,
    view: &View,
    folder: &Path,
    at: &RelPath,
    to: To<'_>,
    fill: Fill,
    tags: &[String],
) -> (Built<'l>, FillFailure) {
    let To { expect, came, .. } = to;
    let failed = Rc::new(RefCell::new(None));
    let source: Box<dyn Filled<Counted>> = Box::new(Filling {
        fill,
        folder: folder.to_path_buf(),
        failed: failed.clone(),
    });
    let entity = view
        .at(at)
        .map(|entity| entity.id())
        .or_else(|| missing_at(view, at));
    let built = match (entity, came) {
        (Some(entity), _) => {
            let built = built.save_from(entity, at, source, expect);
            tags.iter()
                .fold(built, |built, name| built.add(entity, TAGS, name.clone()))
        }
        (None, Some(came)) => {
            let (built, _) = built.create(|e| {
                e.set(ORIGIN, came).save_from(at, source, expect);
                for name in tags {
                    e.add(TAGS, name.clone());
                }
            });
            built
        }
        (None, None) if tags.is_empty() => built.save_file_from(at, source, expect),
        (None, None) => {
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

/// How one part of a batch is answered once the batch is committed: where it puts a
/// file or a folder, by which it landed, and the event it answers with.
struct Owed {
    /// The part of the batch it is of, in order.
    part: usize,
    at: Option<RelPath>,
    failed: Option<FillFailure>,
    answer: Answering,
}

enum Answering {
    Saved {
        id: u64,
        path: LibPath,
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
        let why = || match &failure {
            Failure::Io(why) => why.clone(),
            Failure::Moved => "the file changed on disk".to_string(),
            Failure::Room(len) => format!("{len} bytes do not fit"),
        };
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
                result: Err(why()),
            },
            Answering::Quiet(doing) => Event::Failed(format!("{doing}: {}", why())),
        })
    }

    /// The event that says the part landed; none for one answered only on failure.
    fn landed(self, folder: &Path) -> Option<Event> {
        Some(match self {
            Answering::Saved {
                id,
                path,
                crc,
                identity,
            } => {
                let result = stat_at(folder, &path).map(|stat| Fingerprint {
                    crc: Some(crc),
                    identity,
                    ..Fingerprint::unread(stat)
                });
                Event::Saved { id, path, result }
            }
            Answering::Rewritten { id, path } => {
                let result = landed(folder, &path);
                Event::Rewritten { id, path, result }
            }
            Answering::Imported { id, path } => {
                let result = landed(folder, &path);
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
fn batch(
    library: &mut Library<Counted>,
    folder: &Path,
    label: &str,
    mut parts: Vec<Intent>,
    answer: &mut impl FnMut(Event),
) -> Option<toshokan::Committed> {
    loop {
        let again = parts.iter().any(Intent::reads).then(|| parts.clone());
        let tried = attempt(library, folder, label, parts);
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
            return tried.answer(folder, answer);
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

/// One try at a batch: what each part is owed, and how the commit went.
struct Tried {
    owed: Vec<Owed>,
    refused: Vec<Event>,
    reached: Option<BTreeSet<RelPath>>,
    changes: Option<toshokan::Committed>,
    why: Option<Failure>,
}

impl Tried {
    /// Answer each part, and return what the commit changed.
    fn answer(self, folder: &Path, answer: &mut impl FnMut(Event)) -> Option<toshokan::Committed> {
        let Tried {
            owed,
            refused,
            reached,
            changes,
            why,
        } = self;
        for event in refused {
            answer(event);
        }
        for Owed {
            at,
            failed,
            answer: answering,
            ..
        } in owed
        {
            let landed = match (&reached, &at) {
                (None, _) => true,
                (Some(reached), Some(at)) => reached.contains(at),
                (Some(_), None) => changes.is_some(),
            };
            let event = match landed {
                true => answering.landed(folder),
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

fn attempt(
    library: &mut Library<Counted>,
    folder: &Path,
    label: &str,
    parts: Vec<Intent>,
) -> Tried {
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
    let mut parting = Parting {
        built: Some(library.intent(label)),
        view: &view,
        folder,
        tags: &tags,
        part: 0,
        owed: Vec::new(),
    };
    let mut refused = Vec::new();
    for (at, part) in parts.into_iter().enumerate() {
        parting.part = at;
        if let Err(event) = parting.add(part) {
            refused.push(*event);
        }
    }
    let Parting { built, owed, .. } = parting;
    let built = built.expect("each part puts the intent back");
    let (reached, changes, why) = match built.commit() {
        Ok(committed) => (None, Some(committed), None),
        Err(Error::Partial(partial)) => {
            let reached: BTreeSet<RelPath> = partial.report.landed.iter().cloned().collect();
            let why = format!("the change stopped at {}", partial.report.stopped);
            (
                Some(reached),
                Some(partial.committed),
                Some(Failure::Io(why)),
            )
        }
        Err(e) => (Some(BTreeSet::new()), None, Some(failure(e, None))),
    };
    Tried {
        owed,
        refused,
        reached,
        changes,
        why,
    }
}

fn answer_some(answer: &mut impl FnMut(Event), event: Option<Event>) {
    if let Some(event) = event {
        answer(event);
    }
}

/// One intent being built from the parts of a batch.
struct Parting<'l, 'v> {
    /// Taken while a part is built into it, and put back.
    built: Option<Built<'l>>,
    view: &'v View,
    folder: &'v Path,
    tags: &'v BTreeMap<LibPath, Vec<String>>,
    /// The part being built.
    part: usize,
    owed: Vec<Owed>,
}

impl<'l> Parting<'l, '_> {
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
                from,
                edit,
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
                self.save(to, Fill::Edited(from, edit), answering)
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
                    Source::Edited(from, edit) => Fill::Edited(from, edit),
                    Source::Library(source, known) => {
                        let still = still(self.folder, &source, &known).unwrap_or(false);
                        if !still {
                            let changed = Failure::Io("the file it copies changed on disk".into());
                            return Err(Box::new(
                                answering.refused(changed).expect("a copy answers"),
                            ));
                        }
                        Fill::Library(source)
                    }
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
                    self.owe(Some(to), None, answering);
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
                self.owe(Some(to), None, answering);
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
                    self.owe(None, None, doing);
                }
                Ok(())
            }
            Intent::MakeDir { path } => {
                let at = at(&path)
                    .map_err(|why| Box::new(Event::Failed(format!("making {path}: {why}"))))?;
                self.built = Some(self.take().make_dir(&at));
                self.owe(Some(at), None, Answering::Quiet(format!("making {path}")));
                Ok(())
            }
            Intent::RemoveDir { path } => {
                let at = at(&path)
                    .map_err(|why| Box::new(Event::Failed(format!("removing {path}: {why}"))))?;
                self.built = Some(self.take().remove_dir(&at));
                self.owe(None, None, Answering::Quiet(format!("removing {path}")));
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
                self.owe(
                    None,
                    None,
                    Answering::Quiet(format!("tagging with “{name}”")),
                );
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
        let (built, failed) = save_part(built, self.view, self.folder, &at, to, fill, tags);
        self.built = Some(built);
        self.owe(Some(at), Some(failed), answering);
        Ok(())
    }

    fn take(&mut self) -> Built<'l> {
        self.built.take().expect("each part puts the intent back")
    }

    fn owe(&mut self, at: Option<RelPath>, failed: Option<FillFailure>, answer: Answering) {
        let part = self.part;
        self.owed.push(Owed {
            part,
            at,
            failed,
            answer,
        });
    }
}

/// Copy `from` to `path`, where the copy of a library file is refused once the file is
/// no longer what the app knew of it.
fn import(
    library: &mut Library<Counted>,
    folder: &Path,
    label: &str,
    to: To<'_>,
    from: Source,
) -> Result<toshokan::Committed, Failure> {
    let changed = || Failure::Io("the file it copies changed on disk".to_string());
    let fill = match from {
        Source::Outside(from) => Fill::Outside(from),
        Source::Part(part) => Fill::Part(part),
        Source::Edited(from, edit) => Fill::Edited(from, edit),
        Source::Library(source, known) => {
            if !still(folder, &source, &known).map_err(|e| Failure::Io(e.to_string()))? {
                return Err(changed());
            }
            Fill::Library(source)
        }
    };
    match write(library, folder, label, to, fill) {
        Ok((_, changes)) => Ok(changes),
        Err(Failure::Moved) if to.expect == Expect::Absent => Err(Failure::Moved),
        Err(Failure::Moved) => Err(changed()),
        Err(failure) => Err(failure),
    }
}

/// Rename the file at `from` to `to`, as the entity bound there or as a file no entity
/// is bound to.
/// `built` with the rename `moving` names: of its entity's file, or of the file no
/// entity is bound to.
fn rename<'l>(built: Built<'l>, view: &View, moving: &Moving) -> Result<Built<'l>, String> {
    let Moving { from, to, expect } = moving;
    let (at, to) = (
        relative(from).ok_or_else(|| no_path(from))?,
        relative(to).ok_or_else(|| no_path(to))?,
    );
    Ok(match view.at(&at).map(|entity| entity.id()) {
        Some(entity) => built.rename(entity, &to, *expect),
        None => built.rename_file(&at, &to, *expect),
    })
}

/// Whether the library file at `path` still holds what `known` says: its stat has not
/// moved, or its identity is the one known.
fn still(folder: &Path, path: &LibPath, known: &Fingerprint) -> io::Result<bool> {
    let stat = stat_at(folder, path).map_err(|_| io::Error::from(io::ErrorKind::NotFound))?;
    if stat == known.stat() {
        return Ok(true);
    }
    let (Some(identity), Some(at)) = (known.identity, relative(path)) else {
        return Ok(false);
    };
    let mut file = File::open(locate(folder, path.as_str())?)?;
    Ok(keys::identity_of(&at, &mut file)? == identity)
}

/// What failed to fill a file, or why toshokan refused the change, as the app tells it.
fn failure(e: Error, filled: Option<io::Error>) -> Failure {
    if let Some(e) = filled {
        return fill_failure(e);
    }
    match e {
        Error::Refused(Refusal::Changed(_) | Refusal::Directory(_)) => Failure::Moved,
        e => Failure::Io(e.to_string()),
    }
}

/// Why a fill that failed did not land.
fn fill_failure(e: io::Error) -> Failure {
    match rewrite::is_changed(&e) {
        true => Failure::Moved,
        false => Failure::Io(e.to_string()),
    }
}

/// Answer how a change to a file went, and return what it changed.
fn answer_with<T>(
    answer: &mut impl FnMut(Event),
    result: Result<(T, toshokan::Committed), Failure>,
    event: impl FnOnce(Result<T, Failure>) -> Event,
) -> Option<toshokan::Committed> {
    let (result, changes) = match result {
        Ok((done, changes)) => (Ok(done), Some(changes)),
        Err(failure) => (Err(failure), None),
    };
    answer(event(result));
    changes
}

fn split(
    result: Result<toshokan::Committed, String>,
) -> (Result<(), String>, Option<toshokan::Committed>) {
    match result {
        Ok(changes) => (Ok(()), Some(changes)),
        Err(why) => (Err(why), None),
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

/// The file just written at `path`, as a listing finds it: resting, where it is a piano
/// or sample instrument, and otherwise unread.
fn landed(folder: &Path, path: &LibPath) -> Result<Found, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    let stat = stat_at(folder, path)?;
    let at = locate(folder, path.as_str()).map_err(io)?;
    let file = OnDisk::open(File::open(&at).map_err(io)?, None).map_err(io)?;
    let identity = match (&file, relative(path)) {
        (Some(_), Some(relative)) => {
            Some(keys::identity_of(&relative, &mut File::open(&at).map_err(io)?).map_err(io)?)
        }
        _ => None,
    };
    Ok(Found {
        file: file.map(Arc::new),
        identity,
        ..Found::unread(path.clone(), stat)
    })
}

fn stat_at(folder: &Path, path: &LibPath) -> Result<Stat, Failure> {
    let gone = || Failure::Io("the file was gone as soon as it was written".into());
    let at = locate(folder, path.as_str()).map_err(|e| Failure::Io(e.to_string()))?;
    let meta = std::fs::symlink_metadata(at).map_err(|_| gone())?;
    if !meta.is_file() {
        return Err(gone());
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| u64::try_from(since.as_nanos()).ok());
    Ok(Stat {
        len: meta.len(),
        modified,
    })
}

/// What a file toshokan stages is filled from.
enum Fill {
    Bytes(Vec<u8>),
    /// A file outside the library, never held whole.
    Outside(PathBuf),
    /// Some bytes of a file outside the library, which must have their CRC.
    Part(Part),
    /// The library's own file at this path, never held whole.
    Library(LibPath),
    /// The file an edit makes of a piano or sample instrument resting in the library,
    /// read by range and never held whole.
    Edited(Arc<OnDisk>, Arc<Rewrite>),
}

/// A [`Fill`] as toshokan takes it. What failed is kept in `failed`, since toshokan
/// carries only its text.
struct Filling {
    fill: Fill,
    folder: PathBuf,
    failed: Rc<RefCell<Option<io::Error>>>,
}

impl Filled<Counted> for Filling {
    fn fill(self: Box<Self>, staging: &mut Staging<'_, Counted>) -> Result<(), IoError> {
        let Filling {
            fill,
            folder,
            failed,
        } = *self;
        fill.into(&folder, staging).map_err(|e| {
            let why = IoError::Other(e.to_string());
            *failed.borrow_mut() = Some(e);
            why
        })
    }
}

impl Fill {
    fn into(self, folder: &Path, staging: &mut Staging<'_, Counted>) -> io::Result<()> {
        let mut put = |at: u64, bytes: Vec<u8>| {
            staging
                .write(at, bytes)
                .map_err(|e| io::Error::other(e.to_string()))
        };
        match self {
            Fill::Bytes(bytes) => put(0, bytes),
            Fill::Outside(from) => copy(&mut File::open(from)?, None, &mut put).map(drop),
            Fill::Library(from) => {
                let mut from = File::open(locate(folder, from.as_str())?)?;
                copy(&mut from, None, &mut put).map(drop)
            }
            Fill::Part(part) => {
                let mut source = File::open(&part.from)?;
                source.seek(SeekFrom::Start(part.bytes.start))?;
                let len = part.bytes.end - part.bytes.start;
                let crc = copy(&mut source.take(len), Some(len), &mut put)?;
                match crc == Some(part.crc32) {
                    true => Ok(()),
                    false => Err(part.mismatch()),
                }
            }
            Fill::Edited(from, edit) => {
                let mut out = Out {
                    put: &mut put,
                    at: 0,
                    end: 0,
                    start: 0,
                    held: Vec::new(),
                };
                edit.write(&from, &mut out)?;
                io::Write::flush(&mut out)
            }
        }
    }
}

/// The staged file as a file that seeks, for a writer that patches what it wrote: what
/// runs on is held and handed to `put` a chunk at a time.
struct Out<'p, P> {
    put: &'p mut P,
    at: u64,
    /// How far anything was written.
    end: u64,
    /// Where `held` goes.
    start: u64,
    held: Vec<u8>,
}

impl<P: FnMut(u64, Vec<u8>) -> io::Result<()>> io::Write for Out<'_, P> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.held.is_empty() {
            self.start = self.at;
        }
        self.held.extend_from_slice(bytes);
        self.at += bytes.len() as u64;
        self.end = self.end.max(self.at);
        if self.held.len() >= CHUNK {
            self.flush()?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.held.is_empty() {
            return Ok(());
        }
        (self.put)(self.start, std::mem::take(&mut self.held))
    }
}

impl<P: FnMut(u64, Vec<u8>) -> io::Result<()>> io::Seek for Out<'_, P> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        io::Write::flush(self)?;
        let at = match to {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(delta) => self.end.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.at.checked_add_signed(delta),
        };
        self.at = at.ok_or_else(|| io::Error::other("a seek before the start"))?;
        Ok(self.at)
    }
}

/// Copy `from` through `put` a chunk at a time. With `len`, the copy must be that long,
/// and its CRC-32 is returned.
fn copy(
    from: &mut impl io::Read,
    len: Option<u64>,
    put: &mut impl FnMut(u64, Vec<u8>) -> io::Result<()>,
) -> io::Result<Option<u32>> {
    let mut crc = nord_format::crc::Crc32Stream::new();
    let mut at = 0;
    loop {
        let mut chunk = vec![0; CHUNK];
        let mut filled = 0;
        while filled < CHUNK {
            match from.read(&mut chunk[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        if filled == 0 {
            break;
        }
        chunk.truncate(filled);
        crc.update(&chunk);
        put(at, chunk)?;
        at += filled as u64;
    }
    match len {
        Some(len) if len != at => Ok(None),
        Some(_) => Ok(Some(crc.value())),
        None => Ok(None),
    }
}

fn committed_view(
    library: &Library<Counted>,
    shown: &mut Option<View>,
    changes: Vec<toshokan::report::Change>,
) -> Viewed {
    Viewed {
        shown: show(shown, library.view(), false),
        dirs: None,
        changes,
        moves: Vec::new(),
        undone: None,
        unlisted: None,
    }
}

/// What the answer to a scan shows: everything, where it scanned every file and listed
/// every folder in `dirs`.
fn viewed(
    library: &Library<Counted>,
    shown: &mut Option<View>,
    refreshed: Refreshed,
    dirs: Option<Vec<LibPath>>,
) -> Viewed {
    let moves = refreshed.scan.moved.iter().filter_map(|moved| {
        let from = LibPath::parse(moved.from.as_str())?;
        Some((from, LibPath::parse(moved.to.as_str())?))
    });
    let unlisted = refreshed.unlisted.iter();
    let unlisted = unlisted.filter_map(|dir| LibPath::parse(dir.as_str()));
    Viewed {
        shown: show(shown, library.view(), dirs.is_some()),
        unlisted: dirs.is_some().then(|| unlisted.collect()),
        dirs,
        moves: moves.collect(),
        changes: refreshed.changes,
        undone: None,
    }
}

/// What `view` changed of what the app was last `shown`: everything where `whole`, or
/// where the app was shown nothing yet.
fn show(shown: &mut Option<View>, view: View, whole: bool) -> Change {
    let change = Change::since(&view, shown.as_ref().filter(|_| !whole));
    *shown = Some(view);
    change
}

fn relative(path: &LibPath) -> Option<RelPath> {
    RelPath::new(path.as_str()).ok()
}

fn no_path(path: &LibPath) -> String {
    format!("{path} is no library path")
}

/// Every folder below `root` drawbar lists, in path order: none whose name starts with a
/// dot, none reached through a link, and none past [`MOST_ENTRIES`] entries.
fn folders(root: &Path) -> Vec<LibPath> {
    let mut found = Vec::new();
    let mut waiting = std::collections::VecDeque::from([LibPath::root()]);
    let mut looked = 0;
    while let Some(dir) = waiting.pop_front() {
        let mut at = root.to_path_buf();
        at.extend(dir.components());
        let Ok(entries) = std::fs::read_dir(at) else {
            continue;
        };
        for entry in entries.flatten() {
            looked += 1;
            if looked > MOST_ENTRIES {
                break;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let real = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if !real || name.starts_with('.') {
                continue;
            }
            let text = match dir.is_root() {
                true => name,
                false => format!("{dir}/{name}"),
            };
            let Some(path) = LibPath::parse(&text) else {
                continue;
            };
            found.push(path.clone());
            waiting.push_back(path);
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::Temp;

    /// A keeper of the library in `root`, open.
    fn keeper(root: &Temp) -> Keeper {
        let mut keeper = Keeper::new(root.0.clone(), root.local(), Arc::default());
        let opened = keeper.run_one(Cmd::Open);
        assert!(matches!(opened[..], [Event::Opened(Ok(_))]), "{opened:?}");
        keeper
    }

    impl Keeper {
        /// Run one command, and every answer it gives.
        fn run_one(&mut self, cmd: Cmd) -> Vec<Event> {
            let mut answers = Vec::new();
            self.run(cmd, &mut |event| answers.push(event));
            answers
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
    /// oldest past it, naming the change whose undo needed them. Closing empties what
    /// only this session's undo needed, and keeps what a session stopped short left.
    #[test]
    fn the_trash_keeps_what_undo_needs_within_its_size_and_no_longer() {
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
        let trash = |keeper: &mut Keeper| keeper.library.as_mut().unwrap().trash().unwrap();
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
        assert_eq!(
            trash(&mut second).len(),
            1,
            "a session stopped short leaves it"
        );
        save(&mut second, b"fourth");
        second.close();
        let mut third = keeper(&root);
        assert_eq!(
            trash(&mut third).len(),
            1,
            "only what the closed session's undo needed went"
        );
    }

    /// The time a month and a day from now.
    struct Later;

    impl toshokan::Clock for Later {
        fn now_ms(&mut self) -> u64 {
            SystemClock.now_ms() + 31 * 24 * 60 * 60 * 1000
        }
    }

    /// Opening counts the files the scan finds as it goes, leaving out drawbar's own and
    /// every other name that starts with a dot.
    #[test]
    fn opening_counts_the_files_it_finds() {
        let root = Temp::new();
        fs::create_dir(root.at("Live")).unwrap();
        for name in ["a.ne5p", "Live/b.ne5p", "Live/c.wav", ".DS_Store"] {
            fs::write(root.at(name), name).unwrap();
        }
        let listed = Arc::new(AtomicUsize::new(0));
        let mut keeper = Keeper::new(root.0.clone(), root.local(), listed.clone());
        keeper.run_one(Cmd::Open);
        assert_eq!(listed.load(Ordering::Relaxed), 3);
    }

    /// What a session stopped short left in the trash stays a month, and opening
    /// empties what is older.
    #[test]
    fn opening_empties_the_trash_of_what_is_older_than_a_month() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"contents").unwrap();
        let mut first = keeper(&root);
        let expect = first.listed("c3.ne5p");
        first.run_one(Cmd::Intent(Intent::Delete {
            files: vec![(path("c3.ne5p"), expect)],
        }));
        drop(first);
        let trash = |keeper: &mut Keeper| keeper.library.as_mut().unwrap().trash().unwrap();

        let mut soon = keeper(&root);
        assert_eq!(trash(&mut soon).len(), 1, "kept for now");
        soon.close();
        let mut later = Keeper {
            clock: || Box::new(Later),
            ..Keeper::new(root.0.clone(), root.local(), Arc::default())
        };
        later.run_one(Cmd::Open);
        assert_eq!(trash(&mut later), []);
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
                from: Source::Library(
                    path("a.ne5p"),
                    Fingerprint::unread(stat_at(&root.0, &path("a.ne5p")).unwrap()),
                ),
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
        }
        assert_eq!(outside.names(""), ["x.ne5p"]);
        assert_eq!(outside.read("x.ne5p"), b"outside");
        assert_eq!(root.read("a.ne5p"), b"inside");
    }

    /// A piano library the size a vendor ships, about 200 MB in 64 strokes of about 3 MB.
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
        let edit = Arc::new(Rewrite::Piano(library));
        let expect = keeper.listed("Grand.npno");

        let (answers, largest) = crate::testing::largest_allocation(|| {
            keeper.run_one(Cmd::Intent(Intent::Rewrite {
                id: 1,
                path: path("Grand.npno"),
                from,
                edit,
                expect,
            }))
        });
        // One stroke's audio, about 3 MB, is the most held at once.
        assert!(
            (1 << 20..8 << 20).contains(&largest),
            "{largest} bytes held at once"
        );
        let Event::Rewritten {
            result: Ok(found), ..
        } = &answers[0]
        else {
            panic!("{answers:?}")
        };
        let saved = found
            .file
            .as_ref()
            .expect("the saved library rests in its file");
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
        let stat = stat_at(&root.0, &path("Grand.npno")).unwrap();
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
        let edited = import(
            "Edited.npno",
            Source::Edited(from, Arc::new(Rewrite::Piano(library))),
        );

        for (name, intent) in [("Copy.npno", copied), ("Edited.npno", edited)] {
            let (answers, largest) =
                crate::testing::largest_allocation(|| keeper.run_one(Cmd::Intent(intent)));
            assert!(largest < 8 << 20, "{name}: {largest} bytes held at once");
            let Event::Imported {
                result: Ok(found), ..
            } = &answers[0]
            else {
                panic!("{name}: {answers:?}")
            };
            assert!(found.file.is_some(), "{name} rests in its file");
        }
        assert!(root.read("Copy.npno") == bytes);
        let info = nord_format::cbin::inspect(&mut File::open(root.at("Edited.npno")).unwrap());
        assert!(info.unwrap().checksum_ok);
    }
}
