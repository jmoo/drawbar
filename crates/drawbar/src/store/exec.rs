//! Running a [`Cmd`] against a library's files, the same way whichever backend holds
//! them.
//!
//! The crash-safety of every write rests on [`Fs::create`] and [`Fs::replace`]: a file is
//! written somewhere else first and appears at its path whole. Everything written in
//! flight is either under `.drawbar/tmp/` or a hidden `.<name>.drawbar-tmp` sibling, and
//! opening the library sweeps both.

use std::collections::BTreeMap;
use std::io;

use super::sidecar::{self, Read, Sidecar};
use super::{Cmd, Event, Failure, Fingerprint, Found, LibPath, Listing, Opened, Stat};

pub const INDEX: &str = ".drawbar/library.ron";
pub const TMP: &str = ".drawbar/tmp";
pub const WORKING: &str = ".drawbar/working";
/// What a save's temporary sibling ends in: `.<name>.drawbar-tmp`.
pub const TEMP: &str = ".drawbar-tmp";

/// What a listing says about one entry.
pub enum Kind {
    Dir,
    File(Stat),
}

/// One entry below a library's root.
pub struct Entry {
    /// Relative to the root, joined by `/`.
    pub path: String,
    pub kind: Kind,
}

/// A library's files, as one backend reaches them. Paths are relative to the root and
/// joined by `/`.
pub trait Fs {
    /// Create the root, `.drawbar/`, and its `tmp/` and `working/`, where missing.
    fn prepare(&mut self) -> io::Result<()>;
    /// Hold the one-writer lock for as long as this lives. `Ok(false)` when another
    /// drawbar holds it.
    fn lock(&mut self) -> io::Result<bool>;
    /// Every entry below the root, parents before children. A folder whose name starts
    /// with a dot is listed but not entered.
    fn list(&self) -> io::Result<Vec<Entry>>;
    /// The names in one folder.
    fn names(&self, dir: &str) -> io::Result<Vec<String>>;
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// `None` when nothing is there.
    fn stat(&self, path: &str) -> io::Result<Option<Stat>>;
    /// Write a file where none is. It appears whole or not at all, and a file that
    /// appeared there first is left alone and reported as
    /// [`io::ErrorKind::AlreadyExists`].
    fn create(&mut self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Write a file over whatever is there. Afterwards, or after a crash at any point,
    /// the path holds the old contents or the new, never part of either.
    fn replace(&mut self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Rename a file or folder. Refused when another entry is at `to`.
    fn rename(&mut self, from: &str, to: &str) -> io::Result<()>;
    fn make_dir(&mut self, path: &str) -> io::Result<()>;
    fn remove_file(&mut self, path: &str) -> io::Result<()>;
    /// Only an empty folder.
    fn remove_dir(&mut self, path: &str) -> io::Result<()>;
}

/// Run one command. Commands that answer only on failure return `None` when they
/// succeed.
pub fn execute(fs: &mut impl Fs, cmd: Cmd) -> Option<Event> {
    match cmd {
        Cmd::Open => Some(Event::Opened(open(fs))),
        Cmd::Scan { known } => Some(Event::Scanned(
            listing(fs, Some(&known))
                .map(|(listing, _)| listing)
                .map_err(|e| e.to_string()),
        )),
        Cmd::Commit {
            sidecar,
            working,
            drop,
        } => commit(fs, &sidecar, working, drop)
            .err()
            .map(|e| Event::Failed(format!("keeping the library's index: {e}"))),
        Cmd::Save {
            id,
            path,
            bytes,
            expect,
        } => {
            let result = save(fs, &path, &bytes, expect);
            Some(Event::Saved { id, path, result })
        }
        Cmd::Move { from, to } => fs
            .rename(from.as_str(), to.as_str())
            .err()
            .map(|e| Event::Failed(format!("moving {from} to {to}: {e}"))),
        Cmd::MakeDir(path) => fs
            .make_dir(path.as_str())
            .err()
            .map(|e| Event::Failed(format!("making the folder {path}: {e}"))),
        Cmd::RemoveFile { path, expect } => remove(fs, &path, expect)
            .err()
            .map(|why| Event::Failed(format!("deleting {path}: {why}"))),
        Cmd::RemoveDir(path) => remove_dir(fs, &path)
            .err()
            .map(|e| Event::Failed(format!("removing the folder {path}: {e}"))),
    }
}

fn open(fs: &mut impl Fs) -> Result<Opened, String> {
    let mut writable = fs
        .prepare()
        .map_err(|e| format!("drawbar cannot write here: {e}"))
        .and_then(|()| match fs.lock() {
            Ok(true) => Ok(()),
            Ok(false) => Err("another drawbar has this library open".to_string()),
            Err(e) => Err(format!("the library's lock could not be taken: {e}")),
        });
    let mut swept = 0;
    if writable.is_ok() {
        swept += sweep(fs, TMP, |_| true);
    }
    let sidecar = match fs.read(INDEX) {
        Ok(bytes) => match sidecar::read(&String::from_utf8_lossy(&bytes)) {
            Read::Known(sidecar) => sidecar,
            Read::Newer(version) => {
                writable = Err(format!(
                    "a newer drawbar wrote this library's index (version {version}), so this \
                     one only reads the library"
                ));
                Sidecar::default()
            }
            Read::Unreadable(why) => {
                writable = Err(format!(
                    "the library's index does not read ({why}), so drawbar leaves the \
                     library as it is"
                ));
                Sidecar::default()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Sidecar::default(),
        Err(e) => {
            writable = Err(format!("the library's index could not be read: {e}"));
            Sidecar::default()
        }
    };
    let (listing, temps) = listing(fs, None).map_err(|e| e.to_string())?;
    if writable.is_ok() {
        for temp in temps {
            if fs.remove_file(&temp).is_ok() {
                swept += 1;
            }
        }
    }
    let named: BTreeMap<String, u64> = sidecar
        .assets
        .iter()
        .filter_map(|(id, row)| Some((working_name(*id, row.working?), *id)))
        .collect();
    let working = named
        .iter()
        .filter_map(|(name, id)| Some((*id, fs.read(&format!("{WORKING}/{name}")).ok()?)))
        .collect();
    if writable.is_ok() {
        swept += sweep(fs, WORKING, |name| !named.contains_key(name));
    }
    Ok(Opened {
        writable,
        sidecar,
        listing,
        working,
        swept,
    })
}

/// A working copy's file name.
pub fn working_name(id: u64, generation: u64) -> String {
    format!("{id}-{generation}")
}

/// Remove the files in `dir` that `stale` picks, and return how many went.
fn sweep(fs: &mut impl Fs, dir: &str, stale: impl Fn(&str) -> bool) -> usize {
    let names = fs.names(dir).unwrap_or_default();
    names
        .iter()
        .filter(|name| stale(name))
        .filter(|name| fs.remove_file(&format!("{dir}/{name}")).is_ok())
        .count()
}

/// The tree, and the temporary siblings interrupted saves left in it.
///
/// With `known`, a file is read only when its [`Stat`] is not the one known under its
/// path; without, every file is read.
fn listing(
    fs: &impl Fs,
    known: Option<&BTreeMap<LibPath, Stat>>,
) -> io::Result<(Listing, Vec<String>)> {
    let mut listing = Listing::default();
    let mut temps = Vec::new();
    for entry in fs.list()? {
        let leaf = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        if entry.path.split('/').any(|part| part.starts_with('.')) {
            if leaf.starts_with('.') && leaf.ends_with(TEMP) && matches!(entry.kind, Kind::File(_))
            {
                temps.push(entry.path);
            }
            continue;
        }
        let Some(path) = LibPath::parse(&entry.path) else {
            continue;
        };
        let stat = match entry.kind {
            Kind::Dir => {
                listing.dirs.push(path);
                continue;
            }
            Kind::File(stat) => stat,
        };
        if known.is_some_and(|known| known.get(&path) == Some(&stat)) {
            listing.files.push(Found {
                path,
                stat,
                bytes: None,
            });
            continue;
        }
        match fs.read(path.as_str()) {
            Ok(bytes) => listing.files.push(Found {
                path,
                stat,
                bytes: Some(bytes),
            }),
            Err(e) => listing.unread.push((path, e.to_string())),
        }
    }
    listing.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((listing, temps))
}

fn commit(
    fs: &mut impl Fs,
    sidecar: &Sidecar,
    working: Vec<(String, Vec<u8>)>,
    drop: Vec<String>,
) -> Result<(), String> {
    for (name, bytes) in working {
        fs.replace(&format!("{WORKING}/{name}"), &bytes)
            .map_err(|e| e.to_string())?;
    }
    let text = sidecar::write(sidecar)?;
    fs.replace(INDEX, text.as_bytes())
        .map_err(|e| e.to_string())?;
    for name in drop {
        match fs.remove_file(&format!("{WORKING}/{name}")) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
    }
    Ok(())
}

/// Whether the file at `path` still holds what `expect` says, reading it only when its
/// [`Stat`] moved.
fn still(fs: &impl Fs, path: &LibPath, expect: &Fingerprint) -> io::Result<Option<bool>> {
    let Some(stat) = fs.stat(path.as_str())? else {
        return Ok(None);
    };
    if stat == expect.stat() {
        return Ok(Some(true));
    }
    Ok(Some(expect.holds(&fs.read(path.as_str())?)))
}

fn save(
    fs: &mut impl Fs,
    path: &LibPath,
    bytes: &[u8],
    expect: Option<Fingerprint>,
) -> Result<Fingerprint, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    match expect {
        None => match fs.create(path.as_str(), bytes) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(Failure::Moved),
            wrote => wrote.map_err(io)?,
        },
        Some(expect) => match still(fs, path, &expect).map_err(io)? {
            Some(true) => fs.replace(path.as_str(), bytes).map_err(io)?,
            Some(false) | None => return Err(Failure::Moved),
        },
    }
    let stat = fs
        .stat(path.as_str())
        .map_err(io)?
        .ok_or_else(|| Failure::Io("the file was gone as soon as it was written".into()))?;
    Ok(Fingerprint::of(stat, bytes))
}

/// Remove a folder drawbar has emptied. macOS leaves a `.DS_Store` in any folder Finder
/// has shown, which would otherwise keep it from being removed.
fn remove_dir(fs: &mut impl Fs, path: &LibPath) -> io::Result<()> {
    if fs.names(path.as_str())? == [".DS_Store"] {
        fs.remove_file(&format!("{path}/.DS_Store"))?;
    }
    fs.remove_dir(path.as_str())
}

fn remove(fs: &mut impl Fs, path: &LibPath, expect: Fingerprint) -> Result<(), String> {
    match still(fs, path, &expect).map_err(|e| e.to_string())? {
        None => Ok(()),
        Some(false) => Err("it changed on disk since drawbar read it, so it was left".into()),
        Some(true) => fs.remove_file(path.as_str()).map_err(|e| e.to_string()),
    }
}
