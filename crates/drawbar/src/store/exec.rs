//! Running a [`Cmd`] against a library's files, the same way whichever backend holds
//! them.
//!
//! The crash-safety of every write rests on [`Fs::create`] and [`Fs::replace`]: a file is
//! written somewhere else first and appears at its path whole. Everything written in
//! flight is either under `.drawbar/tmp/` or a hidden `.<name>.drawbar-tmp` sibling, and
//! opening the library sweeps both.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use super::sidecar::{self, Read, Sidecar};
use super::{Cmd, Event, Failure, Fingerprint, Found, LibPath, Listing, Opened, Stat};

/// The sidecar. It is made at drawbar's first write to a library, never on open.
pub const DIR: &str = ".drawbar";
pub const INDEX: &str = ".drawbar/library.ron";
pub const TMP: &str = ".drawbar/tmp";
pub const WORKING: &str = ".drawbar/working";
/// What a save's temporary sibling ends in: `.<name>.drawbar-tmp`.
pub const TEMP: &str = ".drawbar-tmp";

/// The most entries, files and folders alike, one listing looks at. A folder it reaches
/// past that is listed without its contents, so a folder the size of a whole Music
/// folder opens as quickly as a small one.
pub const MOST_ENTRIES: usize = 10_000;

/// The most bytes one listing reads of files drawbar does not hold yet, counting what it
/// holds already. Whatever drawbar holds is in memory, and a folder of pianos would
/// otherwise be read whole.
pub const MOST_BYTES: u64 = 1 << 30;

/// The extensions drawbar opens besides the Nord formats' own tags.
const OPENS: [&str; 5] = [
    nord_format::formats::nsmpproj::FORMAT,
    crate::document::text::EXTENSION,
    "mid",
    "syx",
    "cn3",
];

/// Whether drawbar opens a file of this name, by its extension: a Nord file, a Sample
/// Editor project, a note, a MIDI file or a SysEx dump. A listing reads no other file it
/// does not already hold.
pub fn opens(name: &str) -> bool {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    nord_format::cbin_formats()
        .map(|tag| tag.trim_end_matches('\0'))
        .chain(OPENS)
        .any(|opened| opened.eq_ignore_ascii_case(extension))
}

/// What a listing says about one entry.
pub enum Kind {
    Dir,
    /// A folder whose contents were not all listed: the listing had already looked at
    /// [`MOST_ENTRIES`] entries, or the folder could not be read. It is also listed as a
    /// [`Kind::Dir`].
    Unwalked,
    File(Stat),
    /// A file [`opens`] does not take, listed by name without a look at it.
    Other,
}

/// One entry below a library's root.
pub struct Entry {
    /// Relative to the root, joined by `/`.
    pub path: String,
    pub kind: Kind,
}

/// A library's files, as one backend reaches them. Paths are relative to the root and
/// joined by `/`.
///
/// The desktop's calls finish before they return; a browser's storage answers only
/// later, so every call is a future.
pub trait Fs {
    /// Create the root, `.drawbar/`, and its `tmp/` and `working/`, where missing.
    async fn prepare(&mut self) -> io::Result<()>;
    /// Hold the one-writer lock for as long as this lives. `Ok(false)` when another
    /// drawbar holds it. Taking a lock already held here answers `Ok(true)`.
    async fn lock(&mut self) -> io::Result<bool>;
    /// Whether a file could be written at the root, found out without leaving anything
    /// there. A backend that cannot tell answers `Ok(())`, and the first write finds out.
    async fn probe(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// Every entry below the root, parents before children, up to [`MOST_ENTRIES`] of
    /// them. A folder whose name starts with a dot is listed but not entered.
    async fn list(&self) -> io::Result<Vec<Entry>>;
    /// The names in one folder.
    async fn names(&self, dir: &str) -> io::Result<Vec<String>>;
    async fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// `None` when nothing is there.
    async fn stat(&self, path: &str) -> io::Result<Option<Stat>>;
    /// Write a file where none is. It appears whole or not at all, and a file that
    /// appeared there first is left alone and reported as
    /// [`io::ErrorKind::AlreadyExists`].
    async fn create(&mut self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Write a file over whatever is there. Afterwards, or after a crash at any point,
    /// the path holds the old contents or the new, never part of either.
    async fn replace(&mut self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Rename a file or folder. Refused when another entry is at `to`.
    async fn rename(&mut self, from: &str, to: &str) -> io::Result<()>;
    async fn make_dir(&mut self, path: &str) -> io::Result<()>;
    async fn remove_file(&mut self, path: &str) -> io::Result<()>;
    /// Only an empty folder.
    async fn remove_dir(&mut self, path: &str) -> io::Result<()>;
}

/// Run one command on a thread that may wait for it, as [`run`] does.
#[cfg(not(target_arch = "wasm32"))]
pub fn execute(fs: &mut impl Fs, cmd: Cmd) -> Option<Event> {
    nord_usb::block_on(run(fs, cmd))
}

/// Run one command. Commands that answer only on failure return `None` when they
/// succeed.
pub async fn run(fs: &mut impl Fs, cmd: Cmd) -> Option<Event> {
    if !matches!(cmd, Cmd::Open | Cmd::Scan { .. }) {
        if let Err(why) = take(fs).await {
            return Some(Event::ReadOnly(why));
        }
    }
    match cmd {
        Cmd::Open => Some(Event::Opened(open(fs).await)),
        Cmd::Scan { known } => {
            let held = known
                .into_iter()
                .map(|(path, stat)| (path, Some(stat)))
                .collect();
            Some(Event::Scanned(
                listing(fs, &held)
                    .await
                    .map(|(listing, _)| listing)
                    .map_err(|e| e.to_string()),
            ))
        }
        Cmd::Commit {
            sidecar,
            working,
            drop,
        } => commit(fs, &sidecar, working, drop)
            .await
            .err()
            .map(|e| Event::Failed(format!("keeping the library's index: {e}"))),
        Cmd::Save {
            id,
            path,
            bytes,
            expect,
        } => {
            let result = save(fs, &path, &bytes, expect).await;
            Some(Event::Saved { id, path, result })
        }
        Cmd::Move { from, to } => fs
            .rename(from.as_str(), to.as_str())
            .await
            .err()
            .map(|e| Event::Failed(format!("moving {from} to {to}: {e}"))),
        Cmd::MakeDir(path) => fs
            .make_dir(path.as_str())
            .await
            .err()
            .map(|e| Event::Failed(format!("making the folder {path}: {e}"))),
        Cmd::RemoveFile { path, expect } => remove(fs, &path, expect)
            .await
            .err()
            .map(|why| Event::Failed(format!("deleting {path}: {why}"))),
        Cmd::RemoveDir(path) => remove_dir(fs, &path)
            .await
            .err()
            .map(|e| Event::Failed(format!("removing the folder {path}: {e}"))),
    }
}

async fn open(fs: &mut impl Fs) -> Result<Opened, String> {
    let indexed = matches!(fs.stat(DIR).await, Ok(Some(_)));
    let mut writable = match indexed {
        // drawbar has written here before, so it takes the lock now and a second drawbar
        // finds it taken.
        true => take(fs).await,
        false => fs
            .probe()
            .await
            .map_err(|e| format!("drawbar cannot write here: {e}")),
    };
    let mut swept = 0;
    if indexed && writable.is_ok() {
        swept += sweep(fs, TMP, |_| true).await;
    }
    let sidecar = match fs.read(INDEX).await {
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
    let held = sidecar
        .assets
        .values()
        .filter_map(|row| Some((row.path.clone()?, None)))
        .collect();
    let (listing, temps) = listing(fs, &held).await.map_err(|e| e.to_string())?;
    // A library drawbar has never written holds nothing of drawbar's to sweep, and one it
    // must not write keeps what a newer drawbar left.
    let sweeps = indexed && writable.is_ok();
    if sweeps {
        for temp in temps {
            if fs.remove_file(&temp).await.is_ok() {
                swept += 1;
            }
        }
    }
    let named: BTreeMap<String, u64> = sidecar
        .assets
        .iter()
        .filter_map(|(id, row)| Some((working_name(*id, row.working?), *id)))
        .collect();
    let mut working = BTreeMap::new();
    for (name, id) in &named {
        if let Ok(bytes) = fs.read(&format!("{WORKING}/{name}")).await {
            working.insert(*id, bytes);
        }
    }
    if sweeps {
        swept += sweep(fs, WORKING, |name| !named.contains_key(name)).await;
    }
    Ok(Opened {
        writable,
        indexed,
        sidecar,
        listing,
        working,
        swept,
    })
}

/// Make the sidecar where there is none yet, and hold the library's lock, or say why
/// nothing may be written.
async fn take(fs: &mut impl Fs) -> Result<(), String> {
    fs.prepare()
        .await
        .map_err(|e| format!("drawbar cannot write here: {e}"))?;
    match fs.lock().await {
        Ok(true) => Ok(()),
        Ok(false) => Err("another drawbar has this library open".to_string()),
        Err(e) => Err(format!("the library's lock could not be taken: {e}")),
    }
}

/// A working copy's file name.
pub fn working_name(id: u64, generation: u64) -> String {
    format!("{id}-{generation}")
}

/// Remove the files in `dir` that `stale` picks, and return how many went.
async fn sweep(fs: &mut impl Fs, dir: &str, stale: impl Fn(&str) -> bool) -> usize {
    let mut swept = 0;
    for name in fs.names(dir).await.unwrap_or_default() {
        if stale(&name) && fs.remove_file(&format!("{dir}/{name}")).await.is_ok() {
            swept += 1;
        }
    }
    swept
}

/// The tree, and the temporary siblings interrupted saves left in it.
///
/// Every file in `held` is listed and read, whatever its kind and however far the walk
/// went, except one whose [`Stat`] is the one `held` gives, which is listed unread.
/// Every other file is read only when [`opens`] takes it and it fits in [`MOST_BYTES`].
async fn listing(
    fs: &impl Fs,
    held: &BTreeMap<LibPath, Option<Stat>>,
) -> io::Result<(Listing, Vec<String>)> {
    let mut listing = Listing::default();
    let mut temps = Vec::new();
    let mut dirs = BTreeSet::new();
    // Every file, with its `Stat` where the walk took one.
    let mut files: BTreeMap<LibPath, Option<Stat>> = BTreeMap::new();
    for entry in fs.list().await? {
        let leaf = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        if entry.path.split('/').any(|part| part.starts_with('.')) {
            let file = matches!(entry.kind, Kind::File(_) | Kind::Other);
            if file && leaf.starts_with('.') && leaf.ends_with(TEMP) {
                temps.push(entry.path);
            }
            continue;
        }
        let Some(path) = LibPath::parse(&entry.path) else {
            continue;
        };
        match entry.kind {
            Kind::Dir => {
                dirs.insert(path);
            }
            Kind::Unwalked => listing.unwalked.push(path),
            Kind::File(stat) => {
                files.insert(path, Some(stat));
            }
            Kind::Other => {
                files.insert(path, None);
            }
        }
    }
    for path in held.keys() {
        let hidden = path.components().any(|part| part.starts_with('.'));
        if hidden || matches!(files.get(path), Some(Some(_))) {
            continue;
        }
        match fs.stat(path.as_str()).await {
            Ok(Some(stat)) => {
                files.insert(path.clone(), Some(stat));
                let mut dir = path.parent();
                while !dir.is_root() && dirs.insert(dir.clone()) {
                    dir = dir.parent();
                }
            }
            Ok(None) => {}
            Err(e) => listing.unread.push((path.clone(), e.to_string())),
        }
    }
    listing.dirs = dirs.into_iter().collect();

    let holding: u64 = files
        .iter()
        .filter(|(path, _)| held.contains_key(*path))
        .filter_map(|(_, stat)| stat.map(|stat| stat.len))
        .fold(0, u64::saturating_add);
    let mut room = MOST_BYTES.saturating_sub(holding);
    for (path, stat) in files {
        let known = held.get(&path);
        let stat = match (stat, known) {
            (Some(stat), Some(_)) => stat,
            (Some(stat), None) if opens(path.leaf()) => {
                if stat.len > room {
                    listing.unread.push((path, too_much()));
                    continue;
                }
                room -= stat.len;
                stat
            }
            _ => {
                listing.others.push(path);
                continue;
            }
        };
        if known == Some(&Some(stat)) {
            listing.files.push(Found {
                path,
                stat,
                bytes: None,
            });
            continue;
        }
        match fs.read(path.as_str()).await {
            Ok(bytes) => listing.files.push(Found {
                path,
                stat,
                bytes: Some(bytes),
            }),
            Err(e) => listing.unread.push((path, e.to_string())),
        }
    }
    listing.unread.sort();
    listing.unwalked.sort();
    Ok((listing, temps))
}

fn too_much() -> String {
    format!(
        "drawbar reads at most {} GiB from one library, and the files before this one \
         already came to that",
        MOST_BYTES >> 30
    )
}

async fn commit(
    fs: &mut impl Fs,
    sidecar: &Sidecar,
    working: Vec<(String, Vec<u8>)>,
    drop: Vec<String>,
) -> Result<(), String> {
    for (name, bytes) in working {
        fs.replace(&format!("{WORKING}/{name}"), &bytes)
            .await
            .map_err(|e| e.to_string())?;
    }
    let text = sidecar::write(sidecar)?;
    fs.replace(INDEX, text.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    for name in drop {
        match fs.remove_file(&format!("{WORKING}/{name}")).await {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
    }
    Ok(())
}

/// Whether the file at `path` still holds what `expect` says, reading it only when its
/// [`Stat`] moved.
async fn still(fs: &impl Fs, path: &LibPath, expect: &Fingerprint) -> io::Result<Option<bool>> {
    let Some(stat) = fs.stat(path.as_str()).await? else {
        return Ok(None);
    };
    if stat == expect.stat() {
        return Ok(Some(true));
    }
    Ok(Some(expect.holds(&fs.read(path.as_str()).await?)))
}

async fn save(
    fs: &mut impl Fs,
    path: &LibPath,
    bytes: &[u8],
    expect: Option<Fingerprint>,
) -> Result<Fingerprint, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    match expect {
        None => match fs.create(path.as_str(), bytes).await {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(Failure::Moved),
            wrote => wrote.map_err(io)?,
        },
        Some(expect) => match still(fs, path, &expect).await.map_err(io)? {
            Some(true) => fs.replace(path.as_str(), bytes).await.map_err(io)?,
            Some(false) | None => return Err(Failure::Moved),
        },
    }
    let stat = fs
        .stat(path.as_str())
        .await
        .map_err(io)?
        .ok_or_else(|| Failure::Io("the file was gone as soon as it was written".into()))?;
    Ok(Fingerprint::of(stat, bytes))
}

/// Remove a folder drawbar has emptied. macOS leaves a `.DS_Store` in any folder Finder
/// has shown, which would otherwise keep it from being removed.
async fn remove_dir(fs: &mut impl Fs, path: &LibPath) -> io::Result<()> {
    if fs.names(path.as_str()).await? == [".DS_Store"] {
        fs.remove_file(&format!("{path}/.DS_Store")).await?;
    }
    fs.remove_dir(path.as_str()).await
}

async fn remove(fs: &mut impl Fs, path: &LibPath, expect: Fingerprint) -> Result<(), String> {
    match still(fs, path, &expect).await.map_err(|e| e.to_string())? {
        None => Ok(()),
        Some(false) => Err("it changed on disk since drawbar read it, so it was left".into()),
        Some(true) => fs
            .remove_file(path.as_str())
            .await
            .map_err(|e| e.to_string()),
    }
}
