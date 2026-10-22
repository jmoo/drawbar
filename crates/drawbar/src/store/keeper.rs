//! A library on the desktop: toshokan's blocking driver, run on the backend's thread.
//! Every change the app asks for is an intent here, and every file it writes is filled
//! from a [`Fill`] as toshokan stages it.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use toshokan::blocking::{Library, Native, Source as Filled, Staging};
use toshokan::env::{OsRandom, SystemClock};
use toshokan::error::Refusal;
use toshokan::io::IoError;
use toshokan::report::{Mode, Refreshed};
use toshokan::{EntityId, Env, Error, Expect, RelPath, View};

use super::keys::{self, Came, ORIGIN, TAGS};
use super::native::locate;
use super::{
    Cmd, Event, Failure, Fingerprint, Found, Intent, Key, LibPath, Opened, Part, Source, Stat,
    Viewed,
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
    library: Option<Library<Native>>,
}

impl Keeper {
    pub fn new(folder: PathBuf, local: PathBuf) -> Keeper {
        Keeper {
            folder,
            local,
            library: None,
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
                let result = self.commit(intent, answer);
                answer(Event::Committed { label, result });
            }
            Cmd::Undo => {
                let result = self.undone(Library::undo);
                answer(Event::Committed {
                    label: "Undo".to_string(),
                    result,
                });
            }
            Cmd::Redo => {
                let result = self.undone(Library::redo);
                answer(Event::Committed {
                    label: "Redo".to_string(),
                    result,
                });
            }
            Cmd::File(_) => unreachable!("the backend reads files itself"),
        }
    }

    /// Scan what a file command wrote at `paths`, and answer what the view shows now.
    pub fn touched(&mut self, paths: Vec<LibPath>, answer: &mut impl FnMut(Event)) {
        let Some(library) = &mut self.library else {
            return;
        };
        let paths = paths.iter().filter_map(relative).collect();
        match library.rescan_paths(paths) {
            Ok(refreshed) => answer(Event::Viewed(viewed(library, refreshed, None))),
            Err(e) => answer(Event::Failed(format!("looking at what was written: {e}"))),
        }
    }

    /// Let the library go cleanly, once every command has run.
    pub fn close(&mut self) {
        if let Some(library) = self.library.take() {
            let _ = library.close();
        }
    }

    fn open(&mut self) -> Result<Opened, String> {
        let env = Env {
            clock: Box::new(SystemClock),
            random: Box::new(OsRandom::new()),
            identify: Rc::new(keys::Contents),
            names: Box::new(keys::Volume),
            label: LABEL.to_string(),
        };
        let native = Native::new(&self.folder, &self.local);
        let (mut library, opened) = Library::open(native, keys::layout(), &keys::schema(), env)
            .map_err(|e| e.to_string())?;
        let writable = match opened.mode {
            Mode::Writable => Ok(()),
            Mode::ReadOnly(why) => Err(why.to_string()),
        };
        let refreshed = library.rescan().map_err(|e| e.to_string())?;
        let dirs = folders(&self.folder);
        let viewed = viewed(&library, refreshed, Some(dirs));
        self.library = Some(library);
        Ok(Opened { writable, viewed })
    }

    fn scanned(
        &mut self,
        scan: fn(&mut Library<Native>) -> toshokan::Result<Refreshed>,
        whole: bool,
    ) -> Result<Viewed, String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let refreshed = scan(library).map_err(|e| e.to_string())?;
        let dirs = whole.then(|| folders(&self.folder));
        Ok(viewed(library, refreshed, dirs))
    }

    /// Undo or redo, and the view after it, with the files no entity is bound to that it
    /// moved for the app to follow.
    fn undone(
        &mut self,
        commit: fn(&mut Library<Native>) -> toshokan::Result<toshokan::Committed>,
    ) -> Result<Viewed, String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let committed = commit(library).map_err(|e| e.to_string())?;
        let path = |path: Option<RelPath>| path.and_then(|path| LibPath::parse(path.as_str()));
        let paths = committed.paths.into_iter();
        let paths = paths.map(|moved| (path(moved.from), path(moved.to)));
        Ok(Viewed {
            undone: Some(paths.collect()),
            ..committed_view(library, committed.changes)
        })
    }

    /// Commit an intent. A change to a file answers how it went first, and is answered
    /// here with the view as it stands, whether it was made or not.
    fn commit(&mut self, intent: Intent, answer: &mut impl FnMut(Event)) -> Result<Viewed, String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let label = intent.label();
        let changes = match intent {
            Intent::Tag { files, name } => facts(library, label, |built, view| {
                tag(built, view, &files, &name)
            })?,
            Intent::Untag { entities, name } => facts(library, label, |mut built, _| {
                for entity in entities {
                    built = built.remove(entity, TAGS, &name);
                }
                Ok(built)
            })?,
            Intent::RenameTag { entities, from, to } => facts(library, label, |mut built, _| {
                for entity in entities {
                    built = built
                        .remove(entity, TAGS, &from)
                        .add(entity, TAGS, to.clone());
                }
                Ok(built)
            })?,
            Intent::Forget { entity } => {
                facts(library, label, |built, _| Ok(built.delete(entity)))?
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
                let result =
                    wrote.and_then(|(_, changes)| Ok((landed(&self.folder, &path)?, changes)));
                answer_with(answer, result, |result| Event::Rewritten {
                    id,
                    path,
                    result,
                })
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
                let result = import(library, &self.folder, label, to, from)
                    .and_then(|changes| Ok((landed(&self.folder, &path)?, changes)));
                answer_with(answer, result, |result| Event::Imported {
                    id,
                    path,
                    result,
                })
            }
            Intent::Move { from, to, expect } => {
                let moved = rename(library, label, &from, &to, expect);
                let (result, changes) = split(moved);
                answer(Event::Moved { from, to, result });
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
            Intent::Delete { path, expect } => {
                let trashed = facts(library, label, |built, view| {
                    let at = relative(&path).ok_or_else(|| no_path(&path))?;
                    Ok(match bound_at(view, &at) {
                        Some(entity) => built.trash(entity, expect).delete(entity),
                        None => built.trash_file(&at, expect),
                    })
                });
                trashed.unwrap_or_else(|why| {
                    answer(Event::Failed(format!("deleting {path}: {why}")));
                    Vec::new()
                })
            }
        };
        Ok(committed_view(library, changes))
    }
}

type Built<'l> = toshokan::intent::Intent<&'l mut Library<Native>>;

/// Commit an intent labeled `label` that `build` makes against the view as it stands,
/// and return what it changed.
fn facts<'a>(
    library: &'a mut Library<Native>,
    label: &str,
    build: impl FnOnce(Built<'a>, &View) -> Result<Built<'a>, String>,
) -> Result<Vec<toshokan::report::Change>, String> {
    let view = library.view();
    let built = build(library.intent(label), &view)?;
    let committed = built.commit().map_err(|e| e.to_string())?;
    Ok(committed.changes)
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
        if let Some(entity) = bound_at(view, &at) {
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
    library: &mut Library<Native>,
    folder: &Path,
    label: &str,
    to: To<'_>,
    fill: Fill,
) -> Result<(Stat, Vec<toshokan::report::Change>), Failure> {
    let To { path, expect, came } = to;
    let at = relative(path).ok_or_else(|| Failure::Io(no_path(path)))?;
    let view = library.view();
    let failed = Rc::new(RefCell::new(None));
    let source: Box<dyn Filled<Native>> = Box::new(Filling {
        fill,
        folder: folder.to_path_buf(),
        failed: failed.clone(),
    });
    let built = library.intent(label);
    let built = match (bound_at(&view, &at), came) {
        (Some(entity), _) => built.save_from(entity, &at, source, expect),
        (None, Some(came)) => {
            let (built, _) = built.create(|e| {
                e.set(ORIGIN, came).save_from(&at, source, expect);
            });
            built
        }
        (None, None) => built.save_file_from(&at, source, expect),
    };
    let committed = built.commit().map_err(|e| failure(e, failed.take()))?;
    let stat = stat_at(folder, path)?;
    Ok((stat, committed.changes))
}

/// Copy `from` to `path`, where the copy of a library file is refused once the file is
/// no longer what the app knew of it.
fn import(
    library: &mut Library<Native>,
    folder: &Path,
    label: &str,
    to: To<'_>,
    from: Source,
) -> Result<Vec<toshokan::report::Change>, Failure> {
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
fn rename(
    library: &mut Library<Native>,
    label: &str,
    from: &LibPath,
    to: &LibPath,
    expect: Expect,
) -> Result<Vec<toshokan::report::Change>, String> {
    facts(library, label, |built, view| {
        let (at, to) = (
            relative(from).ok_or_else(|| no_path(from))?,
            relative(to).ok_or_else(|| no_path(to))?,
        );
        Ok(match bound_at(view, &at) {
            Some(entity) => built.rename(entity, &to, expect),
            None => built.rename_file(&at, &to, expect),
        })
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
        return match rewrite::is_changed(&e) {
            true => Failure::Moved,
            false => Failure::Io(e.to_string()),
        };
    }
    match e {
        Error::Refused(Refusal::Changed(_) | Refusal::Directory(_)) => Failure::Moved,
        e => Failure::Io(e.to_string()),
    }
}

/// Answer how a change to a file went, and return what it changed.
fn answer_with<T>(
    answer: &mut impl FnMut(Event),
    result: Result<(T, Vec<toshokan::report::Change>), Failure>,
    event: impl FnOnce(Result<T, Failure>) -> Event,
) -> Vec<toshokan::report::Change> {
    let (result, changes) = match result {
        Ok((done, changes)) => (Ok(done), changes),
        Err(failure) => (Err(failure), Vec::new()),
    };
    answer(event(result));
    changes
}

fn split(
    result: Result<Vec<toshokan::report::Change>, String>,
) -> (Result<(), String>, Vec<toshokan::report::Change>) {
    match result {
        Ok(changes) => (Ok(()), changes),
        Err(why) => (Err(why), Vec::new()),
    }
}

/// The entity bound to the file at `path` in `view`, if one is.
fn bound_at(view: &View, path: &RelPath) -> Option<EntityId> {
    let entities = view.entities().into_iter();
    entities
        .filter(|entity| {
            entity.file().is_some_and(|file| {
                file.path == *path && file.state != toshokan::FileState::Missing
            })
        })
        .map(|entity| entity.id())
        .next()
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

impl Filled<Native> for Filling {
    fn fill(self: Box<Self>, staging: &mut Staging<'_, Native>) -> Result<(), IoError> {
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
    fn into(self, folder: &Path, staging: &mut Staging<'_, Native>) -> io::Result<()> {
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

fn committed_view(library: &Library<Native>, changes: Vec<toshokan::report::Change>) -> Viewed {
    Viewed {
        view: library.view(),
        dirs: None,
        changes,
        moves: Vec::new(),
        undone: None,
    }
}

fn viewed(library: &Library<Native>, refreshed: Refreshed, dirs: Option<Vec<LibPath>>) -> Viewed {
    let moves = refreshed.scan.moved.iter().filter_map(|moved| {
        let from = LibPath::parse(moved.from.as_str())?;
        Some((from, LibPath::parse(moved.to.as_str())?))
    });
    Viewed {
        view: library.view(),
        dirs,
        moves: moves.collect(),
        changes: refreshed.changes,
        undone: None,
    }
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
        let mut keeper = Keeper::new(root.0.clone(), root.local());
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

    #[test]
    fn a_rename_that_changes_only_case_goes_through() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"lower").unwrap();
        let mut keeper = keeper(&root);
        let expect = keeper.listed("c3.ne5p");
        let moved = keeper.run_one(Cmd::Intent(Intent::Move {
            from: path("c3.ne5p"),
            to: path("C3.ne5p"),
            expect,
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
            from: path("c3.ne5p"),
            to: path("C3.ne5p"),
            expect,
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
            path: path("c3.ne5p"),
            expect: read,
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
                from: path("a.ne5p"),
                to: path("Linked/a.ne5p"),
                expect: a,
            },
            Intent::Delete {
                path: path("Linked/x.ne5p"),
                expect: x,
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
