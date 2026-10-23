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
use toshokan::error::Refusal;
use toshokan::io::{IoError, Kind, Range, Root};
use toshokan::log::Settlement;
use toshokan::report::{By, Mode, Refreshed};
use toshokan::{EntryHash, Env, Error, Expect, Policy, RelPath, View};

use super::keys::{self, Came, ORIGIN, TAGS, TAG_NAMES};
use super::shown::Change;
use super::storage::{self, CHUNK};
use super::{
    slots, Cmd, Event, Failure, Fingerprint, Found, Intent, Kept, Key, LibPath, Opened, Part,
    Source, Stat, Viewed,
};
use crate::rewrite::{self, Pieces};

/// The most entries a listing of the library's folders looks at: a guard against a
/// folder no library is, such as a whole disk.
const MOST_ENTRIES: usize = 1_000_000;

/// What other writers see this one called. The folder may be shared, so it says no
/// more than which app wrote.
const LABEL: &str = "drawbar";

/// The most bytes a writer's trash holds in a session: the size toshokan's policy
/// keeps.
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

pub struct Keeper<F> {
    /// The storage and what toshokan is told, until the library opens over them.
    unopened: Option<(F, Env)>,
    library: Option<Library<F>>,
    /// The entries this session logged, whose undo ends with it.
    session: BTreeSet<EntryHash>,
    /// The most bytes this writer's trash holds before its oldest go.
    trash: u64,
    /// The view the app was last shown.
    shown: Option<View>,
}

impl<F: Fs> Keeper<F> {
    /// The library on `fs`, opened by [`Cmd::Open`].
    pub fn new(fs: F, env: Env) -> Keeper<F> {
        Keeper {
            unopened: Some((fs, env)),
            library: None,
            session: BTreeSet::new(),
            trash: TRASH,
            shown: None,
        }
    }

    /// Show the whole view in the next answer, as the app lost what it was last shown.
    #[cfg(any(target_arch = "wasm32", test))]
    pub fn forget_shown(&mut self) {
        self.shown = None;
    }

    /// The storage the library is kept on, once it is open.
    pub fn fs(&self) -> Option<&F> {
        self.library.as_ref().map(Library::fs)
    }

    /// Run a command on the library, and answer it.
    pub async fn run(&mut self, cmd: Cmd, answer: &mut impl FnMut(Event)) {
        match cmd {
            Cmd::Open => {
                let opened = self.open().await;
                answer(Event::Opened(opened));
            }
            Cmd::Rescan => answer(Event::Scanned(self.scanned(true).await)),
            Cmd::Refresh => answer(Event::Scanned(self.scanned(false).await)),
            Cmd::Intent(intent) => {
                let label = intent.label().to_string();
                let committed = self.commit(intent, answer).await;
                self.made(label, committed, answer).await;
            }
            Cmd::Undo => {
                let undone = self.undone(false).await;
                self.made("Undo".to_string(), undone, answer).await;
            }
            Cmd::Redo => {
                let redone = self.undone(true).await;
                self.made("Redo".to_string(), redone, answer).await;
            }
            Cmd::Settle { orphan, how } => {
                let settled = self.settled(orphan, how).await;
                self.made("Settle".to_string(), settled, answer).await;
            }
            Cmd::Draft {
                of,
                base,
                bytes,
                replaces,
            } => {
                let mut kept = self.put_draft(&of, base, bytes).await;
                for replaced in replaces {
                    if kept.is_err() {
                        break;
                    }
                    kept = self.discard(replaced).await;
                }
                if let Err(why) = kept {
                    answer(Event::Unkept(why));
                }
            }
            Cmd::Discard(kept) => {
                if let Err(why) = self.discard(kept).await {
                    answer(Event::Unkept(why));
                }
            }
            Cmd::KeepSlot { slot, base, bytes } => {
                let kept = match self.fs() {
                    Some(fs) => slots::keep(fs, &slot, base, &bytes).await,
                    None => Err(io::Error::other("the library is not open")),
                };
                if let Err(e) = kept {
                    answer(Event::Unkept(format!("keeping a slot's edit: {e}")));
                }
            }
            Cmd::DiscardSlot(slot) => {
                let discarded = match self.fs() {
                    Some(fs) => slots::discard(fs, &slot).await,
                    None => Err(io::Error::other("the library is not open")),
                };
                if let Err(e) = discarded {
                    answer(Event::Unkept(format!("letting go of a slot's edit: {e}")));
                }
            }
            Cmd::ReadSlot(slot) => {
                let kept = match self.fs() {
                    Some(fs) => slots::read(fs, &slot).await.map_err(|e| e.to_string()),
                    None => Err("the library is not open".to_string()),
                };
                answer(Event::SlotKept { slot, kept });
            }
            Cmd::Read { .. } => unreachable!("the backend reads files itself"),
        }
    }

    /// Answer a change, an undo or a redo, and keep this writer's trash within
    /// [`TRASH`] once it was made.
    async fn made(
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
            if let Some(trimmed) = self.trim().await {
                answer(trimmed);
            }
        }
    }

    /// Empty the oldest of this writer's trash where it holds more than [`TRASH`], and
    /// say which changes' undo needed what went.
    async fn trim(&mut self) -> Option<Event> {
        let library = self.library.as_mut()?;
        let held = library.trash().await.ok()?;
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
        let emptied = match library.empty_trash(policy).await {
            Ok(emptied) => emptied,
            Err(e) => return Some(Event::Unkept(format!("emptying the trash: {e}"))),
        };
        let gone = held
            .iter()
            .filter(|item| emptied.removed.contains(&item.item));
        Some(Event::Trimmed(gone.map(|item| item.by).collect()))
    }

    /// Let go of a draft this instance's writer, or an idle writer of the pool, kept.
    async fn discard(&mut self, kept: Kept) -> Result<(), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let of = drafted(&kept.of)?;
        let discarded = match kept.pooled {
            Some(writer) => library.discard_pooled_draft(writer, of).await,
            None => {
                let draft = match of {
                    toshokan::Drafted::Entity(entity) => library.draft(entity),
                    toshokan::Drafted::File(path) => library.draft_file(&path),
                };
                draft.discard().await
            }
        };
        discarded.map_err(|e| e.to_string())
    }

    /// Keep `bytes` as the draft of the file `of` names, made over `base`.
    async fn put_draft(
        &mut self,
        of: &Key,
        base: toshokan::Identity,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let draft = match drafted(of)? {
            toshokan::Drafted::Entity(entity) => library.draft(entity),
            toshokan::Drafted::File(path) => library.draft_file(&path),
        };
        draft.put(base, bytes).await.map_err(|e| e.to_string())
    }

    /// Let the library go cleanly, once every command has run: this session's undo
    /// ends now, and the trash lets go of what only it needed.
    pub async fn close(&mut self) {
        let Some(mut library) = self.library.take() else {
            return;
        };
        if let Ok(held) = library.trash().await {
            let ended = held
                .into_iter()
                .filter(|item| self.session.contains(&item.by));
            let _ = library
                .remove_trash(ended.map(|item| item.item).collect())
                .await;
        }
        let _ = library.close().await;
    }

    async fn open(&mut self) -> Result<Opened, String> {
        let (fs, env) = self.unopened.take().ok_or("the library opened before")?;
        let (mut library, opened) = Library::open(fs, keys::layout(), &keys::schema(), env)
            .await
            .map_err(|e| e.to_string())?;
        let writable = match &opened.mode {
            Mode::Writable => Ok(()),
            Mode::ReadOnly(why) => Err(why.to_string()),
        };
        let mut trouble = Vec::new();
        let let_go = match restored(library.fs(), &opened).await {
            true => opened.removed.len(),
            false => 0,
        };
        if let_go > 0 {
            library.let_go().await.map_err(|e| e.to_string())?;
        }
        if writable.is_ok() {
            // Also what a crash may have torn from the end of this writer's log, which
            // opening does not report.
            let settled = library.settle_interrupted().await;
            trouble.extend(settled.err().map(|e| format!("finishing a change: {e}")));
            // Another window of this install left these: they are ours to finish.
            for orphan in opened.orphaned.into_iter().filter(|orphan| orphan.ours) {
                let settled = library.settle(orphan, Settlement::Finished).await;
                trouble.extend(settled.err().map(|e| format!("finishing a change: {e}")));
            }
            // Nothing an earlier session displaced can be undone now.
            let emptied = library.empty_trash(Policy::default()).await;
            trouble.extend(emptied.err().map(|e| format!("emptying the trash: {e}")));
        }
        let viewed = committed_view(&library, &mut self.shown, Vec::new());
        self.library = Some(library);
        Ok(Opened {
            writable,
            viewed,
            drafts: opened.drafts,
            let_go,
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
        let mut refreshed = refreshed.map_err(|e| e.to_string())?;
        // Another window of this install left these and closed since: they are ours to
        // finish, as at open. One that will not finish is the user's to settle.
        let (ours, theirs) = std::mem::take(&mut refreshed.orphaned)
            .into_iter()
            .partition::<Vec<_>, _>(|orphan| orphan.ours);
        refreshed.orphaned = theirs;
        for mut orphan in ours {
            if library
                .settle(orphan.clone(), Settlement::Finished)
                .await
                .is_err()
            {
                orphan.ours = false;
                refreshed.orphaned.push(orphan);
            }
        }
        Ok(viewed(library, &mut self.shown, refreshed))
    }

    /// Undo, or redo where `again`, and the view after it, with the files no entity is
    /// bound to that it moved for the app to follow.
    async fn undone(&mut self, again: bool) -> Result<(Option<EntryHash>, Viewed), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let committed = match again {
            true => library.redo().await,
            false => library.undo().await,
        };
        let committed = committed.map_err(|e| e.to_string())?;
        let path = |path: Option<RelPath>| path.and_then(|path| LibPath::parse(path.as_str()));
        let trees = committed.trees.into_iter();
        let trees = trees.map(|(from, to)| (path(Some(from)), path(Some(to))));
        let paths = committed.paths.into_iter();
        let paths = paths.map(|moved| (path(moved.from), path(moved.to)));
        let paths = trees.chain(paths);
        let viewed = Viewed {
            undone: Some(paths.collect()),
            dirs: Some(folders(library.fs()).await),
            ..committed_view(library, &mut self.shown, committed.changes)
        };
        Ok((Some(committed.intent), viewed))
    }

    /// Settle another writer's unfinished change, and the view after it.
    async fn settled(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<(Option<EntryHash>, Viewed), String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let committed = library
            .settle(orphan, how)
            .await
            .map_err(|e| e.to_string())?;
        let viewed = Viewed {
            dirs: Some(folders(library.fs()).await),
            ..committed_view(library, &mut self.shown, committed.changes)
        };
        Ok((Some(committed.intent), viewed))
    }

    /// Commit an intent, and say which entry logged it, where it was made. A change to a
    /// file answers how it went first, and is answered here with the view as it stands,
    /// whether it was made or not.
    async fn commit(
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
                    changes = library.refresh().await.map_err(|e| e.to_string())?.changes;
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
            Intent::Batch(parts) => batch(library, label, parts, answer).await,
            part => batch(library, label, vec![part], answer).await,
        };
        let intent = changes.as_ref().map(|committed| committed.intent);
        let changes = changes.map(|committed| committed.changes);
        let viewed = committed_view(library, &mut self.shown, changes.unwrap_or_default());
        Ok((intent, viewed))
    }
}

/// The draft `of` names, as toshokan names it.
fn drafted(of: &Key) -> Result<toshokan::Drafted, String> {
    Ok(match of {
        Key::Entity(entity) => toshokan::Drafted::Entity(*entity),
        Key::File(path) => toshokan::Drafted::File(relative(path).ok_or_else(|| no_path(path))?),
    })
}

/// Whether what this install showed and the folder no longer holds was taken by a
/// restore of the folder, rather than not delivered yet by a sync: every writer whose
/// entries went has its directory in the folder.
async fn restored<F: Fs>(fs: &F, opened: &toshokan::Opened) -> bool {
    let layout = keys::layout();
    let writers = opened.removed.iter().filter_map(|change| match &change.by {
        By::Writer { writer, .. } => Some(*writer),
        By::This | By::Outside => None,
    });
    for writer in writers.collect::<BTreeSet<_>>() {
        let present = storage::meta(fs, Root::Folder, &layout.writer(writer)).await;
        let dir = present
            .ok()
            .flatten()
            .is_some_and(|meta| meta.kind == Kind::Directory);
        if !dir {
            return false;
        }
    }
    true
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
    built.commit().await.map_err(|e| e.to_string())
}

/// Add the tag `name` to the library's list of tags.
fn list_tag<'l, F: Fs + 'l>(built: Built<'l, F>, view: &View, name: String) -> Built<'l, F> {
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

/// How one part of a batch is answered once the batch is committed: the path that
/// tells whether it landed, and the event it answers with.
struct Owed {
    /// The part of the batch it is of, in order.
    part: usize,
    at: Option<Spot>,
    failed: Option<FillFailure>,
    answer: Answering,
}

/// Where a part of a batch puts a file or a folder, or takes one from.
enum Spot {
    Puts(RelPath),
    Takes(RelPath),
}

/// What of a batch that stopped partway landed: the paths its finished steps put a
/// file or a folder at, and those they took one from.
struct Reached {
    put: BTreeSet<RelPath>,
    taken: BTreeSet<RelPath>,
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
    async fn landed<F: Fs>(self, fs: &F) -> Option<Event> {
        Some(match self {
            Answering::Saved {
                id,
                path,
                crc,
                identity,
            } => {
                let result = stat_at(fs, &path).await.map(|stat| Fingerprint {
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

/// One try at a batch: what each part is owed, and how the commit went.
struct Tried {
    owed: Vec<Owed>,
    refused: Vec<Event>,
    reached: Option<Reached>,
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
                (Some(reached), Some(Spot::Puts(at))) => reached.put.contains(at),
                (Some(reached), Some(Spot::Takes(at))) => reached.taken.contains(at),
                (Some(_), None) => changes.is_some(),
            };
            let event = match landed {
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
    let (reached, changes, why) = match built.commit().await {
        Ok(committed) => (None, Some(committed), None),
        Err(Error::Partial(partial)) => {
            // ⚠️ Only a commit's report says what landed; a settlement's leaves it empty.
            let report = &partial.report;
            let reached = Reached {
                put: report.landed.iter().cloned().collect(),
                taken: report.taken.iter().cloned().collect(),
            };
            let why = format!("the change stopped at {}", partial.report.stopped);
            (
                Some(reached),
                Some(partial.committed),
                Some(Failure::Io(why)),
            )
        }
        Err(e) => {
            let nothing = Reached {
                put: BTreeSet::new(),
                taken: BTreeSet::new(),
            };
            (Some(nothing), None, Some(failure(e, None)))
        }
    };
    Tried {
        owed,
        refused,
        reached,
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
                    self.owe(Some(Spot::Puts(to)), None, answering);
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
                self.owe(Some(Spot::Puts(to)), None, answering);
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
                    self.owe(Some(Spot::Takes(at)), None, doing);
                }
                Ok(())
            }
            Intent::MakeDir { path } => {
                let at = at(&path)
                    .map_err(|why| Box::new(Event::Failed(format!("making {path}: {why}"))))?;
                self.built = Some(self.take().make_dir(&at));
                let doing = Answering::Quiet(format!("making {path}"));
                self.owe(Some(Spot::Puts(at)), None, doing);
                Ok(())
            }
            Intent::RemoveDir { path } => {
                let at = at(&path)
                    .map_err(|why| Box::new(Event::Failed(format!("removing {path}: {why}"))))?;
                self.built = Some(self.take().remove_dir(&at));
                let doing = Answering::Quiet(format!("removing {path}"));
                self.owe(Some(Spot::Takes(at)), None, doing);
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
                self.owe(None, None, doing);
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
        self.owe(Some(Spot::Puts(at)), Some(failed), answering);
        Ok(())
    }

    fn take(&mut self) -> Built<'l, F> {
        self.built.take().expect("each part puts the intent back")
    }

    fn owe(&mut self, at: Option<Spot>, failed: Option<FillFailure>, answer: Answering) {
        let part = self.part;
        self.owed.push(Owed {
            part,
            at,
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
        undone: None,
        unlisted: None,
        unfinished: None,
    }
}

/// What the answer to a scan shows, with every folder where it scanned every file.
fn viewed<F: Fs>(library: &Library<F>, shown: &mut Option<View>, refreshed: Refreshed) -> Viewed {
    let moves = refreshed.scan.moved.iter().filter_map(|moved| {
        let from = LibPath::parse(moved.from.as_str())?;
        Some((from, LibPath::parse(moved.to.as_str())?))
    });
    let unlisted = refreshed.unlisted.iter();
    let unlisted = unlisted.filter_map(|dir| LibPath::parse(dir.as_str()));
    let theirs = refreshed.orphaned.into_iter().filter(|orphan| !orphan.ours);
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
        undone: None,
        unfinished: Some(theirs.collect()),
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use toshokan::env::SystemClock;

    use super::super::counted::Counted;
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
        assert_eq!(
            trash(&mut second).len(),
            1,
            "a session stopped short leaves it"
        );
        save(&mut second, b"fourth");
        nord_usb::block_on(second.close());
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
    }

    #[test]
    fn the_scan_after_opening_counts_the_files_it_finds() {
        let root = Temp::new();
        fs::create_dir(root.at("Live")).unwrap();
        for name in ["a.ne5p", "Live/b.ne5p", "Live/c.wav", ".DS_Store"] {
            fs::write(root.at(name), name).unwrap();
        }
        let listed = Arc::new(AtomicUsize::new(0));
        let clock = Box::new(SystemClock);
        let mut keeper = native::keeper(&root.0, &root.local(), listed.clone(), clock);
        keeper.run_one(Cmd::Open);
        keeper.run_one(Cmd::Rescan);
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
        let trash = Keeper::trash_now;

        let mut soon = keeper(&root);
        assert_eq!(trash(&mut soon).len(), 1, "kept for now");
        nord_usb::block_on(soon.close());
        let mut later = keeper_at(&root, Box::new(Later));
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
        *keeper.fs().unwrap().meddle.borrow_mut() = Some(Box::new(move || {
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
        // One stroke's audio, about 3 MB, is the most held at once.
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
