//! A library's facts on the desktop: toshokan's blocking driver, run on the backend's
//! thread between the commands that write its files.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use toshokan::blocking::{Library, Native};
use toshokan::env::{OsRandom, SystemClock};
use toshokan::report::{Mode, Refreshed};
use toshokan::{Env, Expect, RelPath};

use super::keys::{self, TAGS};
use super::{Cmd, Event, Intent, Key, LibPath, Opened, Viewed};

/// The most entries a listing of the library's folders looks at: a guard against a
/// folder no library is, such as a whole disk.
const MOST_ENTRIES: usize = 1_000_000;

/// What other writers see this one called. The folder may be shared, so it says no
/// more than which app wrote.
const LABEL: &str = "drawbar";

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

    /// Run a command on the library's facts, and answer it.
    pub fn run(&mut self, cmd: Cmd, answer: &mut impl FnMut(Event)) {
        match cmd {
            Cmd::Open => answer(Event::Opened(self.open())),
            Cmd::Rescan => answer(Event::Scanned(self.scanned(Library::rescan, true))),
            Cmd::Refresh => answer(Event::Scanned(self.scanned(Library::refresh, false))),
            Cmd::Intent(intent) => {
                let label = intent.label().to_string();
                let result = self.commit(intent);
                answer(Event::Committed { label, result });
            }
            Cmd::Undo => {
                let result = self.committed(Library::undo);
                answer(Event::Committed {
                    label: "Undo".to_string(),
                    result,
                });
            }
            Cmd::Redo => {
                let result = self.committed(Library::redo);
                answer(Event::Committed {
                    label: "Redo".to_string(),
                    result,
                });
            }
            Cmd::File(_) => unreachable!("the backend writes files itself"),
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

    fn committed(
        &mut self,
        commit: fn(&mut Library<Native>) -> toshokan::Result<toshokan::Committed>,
    ) -> Result<Viewed, String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let committed = commit(library).map_err(|e| e.to_string())?;
        Ok(Viewed {
            view: library.view(),
            dirs: None,
            changes: committed.changes,
            moves: Vec::new(),
        })
    }

    fn commit(&mut self, intent: Intent) -> Result<Viewed, String> {
        let library = self.library.as_mut().ok_or("the library is not open")?;
        let mut adopted = Vec::new();
        if let Intent::Tag { files, .. } = &intent {
            for key in files {
                let Key::File(path) = key else { continue };
                let at = relative(path).ok_or_else(|| format!("{path} is no library path"))?;
                let identity = identity(&self.folder, path).map_err(|e| format!("{path}: {e}"))?;
                adopted.push((at, identity));
            }
        }
        let deleted = match &intent {
            Intent::Delete { entity } => {
                let view = library.view();
                let file = view.entity(*entity).and_then(|entity| entity.file());
                let path = file.ok_or("it has no file")?.path;
                let path = LibPath::parse(path.as_str()).ok_or("it has no file")?;
                Some(identity(&self.folder, &path).map_err(|e| format!("{path}: {e}"))?)
            }
            _ => None,
        };
        let mut built = library.intent(intent.label());
        match intent {
            Intent::Tag { files, name } => {
                for entity in files.iter().filter_map(Key::entity) {
                    built = built.add(entity, TAGS, name.clone());
                }
                for (at, identity) in adopted {
                    let (next, _) = built.create(|e| {
                        e.adopt(&at, Expect::Holds(identity))
                            .add(TAGS, name.clone());
                    });
                    built = next;
                }
            }
            Intent::Untag { entities, name } => {
                for entity in entities {
                    built = built.remove(entity, TAGS, &name);
                }
            }
            Intent::RenameTag { entities, from, to } => {
                for entity in entities {
                    built = built
                        .remove(entity, TAGS, &from)
                        .add(entity, TAGS, to.clone());
                }
            }
            Intent::Delete { entity } => {
                let expect = Expect::Holds(deleted.expect("a delete reads its file"));
                built = built.trash(entity, expect).delete(entity);
            }
            Intent::Forget { entity } => built = built.delete(entity),
        }
        let committed = built.commit().map_err(|e| e.to_string())?;
        Ok(Viewed {
            view: library.view(),
            dirs: None,
            changes: committed.changes,
            moves: Vec::new(),
        })
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
    }
}

fn relative(path: &LibPath) -> Option<RelPath> {
    RelPath::new(path.as_str()).ok()
}

/// The identity of the library's file at `path`, as it is now.
fn identity(folder: &Path, path: &LibPath) -> std::io::Result<toshokan::Identity> {
    let mut at = folder.to_path_buf();
    at.extend(path.components());
    let relative = relative(path).ok_or(std::io::ErrorKind::InvalidInput)?;
    keys::identity_of(&relative, &mut std::fs::File::open(at)?)
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
