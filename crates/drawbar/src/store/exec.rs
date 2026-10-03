//! Running a [`Cmd`] against a library's files, the same way whichever backend holds
//! them.
//!
//! The crash-safety of every write rests on [`Fs::create`] and [`Fs::replace`]: a file is
//! written somewhere else first and appears at its path whole. Everything written in
//! flight is either under `.drawbar/tmp/` or a hidden `.<name>.drawbar-tmp` sibling, and
//! opening the library sweeps both.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::Arc;

use super::sidecar::{self, Read, Sidecar};
use super::{Cmd, Complete, Event, Failure, Fingerprint, Found, LibPath, Listing, Opened, Stat};
use crate::ondisk::OnDisk;

/// The sidecar. It is made at drawbar's first write to a library, never on open.
pub const DIR: &str = ".drawbar";
pub const INDEX: &str = ".drawbar/library.ron";
pub const TMP: &str = ".drawbar/tmp";
pub const WORKING: &str = ".drawbar/working";
/// What a save's temporary sibling ends in: `.<name>.drawbar-tmp`.
pub const TEMP: &str = ".drawbar-tmp";

/// The most entries, files and folders alike, one listing looks at: a guard against a
/// folder no library is, such as a whole disk. A folder it reaches past that is listed
/// without its contents, and said to be not all listed.
pub const MOST_ENTRIES: usize = 1_000_000;

/// The most bytes one listing reads whole of files drawbar does not hold yet, counting
/// what it holds whole already. Whatever drawbar holds whole is in memory; a file left
/// resting in place, read by range, is not counted.
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
    /// A file [`opens`] takes that could not be looked at, and why.
    Unread(String),
    /// A file [`opens`] does not take, listed by name without a look at it.
    Other,
}

/// One folder's entries, as [`Fs::children`] gives them, and whether it holds more.
pub type Children = (Vec<(String, Option<Kind>)>, bool);

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
    /// Whether the library has been let go, so a listing need not finish.
    fn stopped(&self) -> bool {
        false
    }
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
    /// The first `room` entries of the folder at `dir`, by name, each with its kind, and
    /// whether the folder holds more. `None` leaves an entry out of the listing, though it
    /// still counts toward [`MOST_ENTRIES`]. Never [`Kind::Unwalked`].
    async fn children(&self, dir: &str, room: usize) -> io::Result<Children>;
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
    /// The file at `path` indexed and left on disk, to be read by range, where this
    /// backend reads files that way and the file is a piano or sample instrument. `None`
    /// has it read whole. `known` is its fingerprint where its [`Stat`] has not moved
    /// since, so its CRC need not be taken again.
    async fn rest(
        &self,
        _path: &str,
        _known: Option<Fingerprint>,
    ) -> io::Result<Option<Arc<OnDisk>>> {
        Ok(None)
    }
}

/// Run one command on a thread that may wait for it, as [`run`] does.
#[cfg(not(target_arch = "wasm32"))]
pub fn execute(fs: &mut impl Fs, cmd: Cmd, answer: &mut impl FnMut(Event)) {
    nord_usb::block_on(run(fs, cmd, answer))
}

/// Run one command, handing each answer to `answer` as it is ready. An open answers in
/// parts; the commands that answer only on failure say nothing when they succeed.
pub async fn run(fs: &mut impl Fs, cmd: Cmd, answer: &mut impl FnMut(Event)) {
    if !matches!(cmd, Cmd::Open | Cmd::Scan { .. }) {
        if let Err(why) = take(fs).await {
            return answer(Event::ReadOnly(why));
        }
    }
    let answered = match cmd {
        Cmd::Open => open(fs, answer)
            .await
            .err()
            .map(|why| Event::Opened(Err(why))),
        Cmd::Scan { known, resting } => Some(Event::Scanned(
            scan(fs, known, &resting).await.map_err(|e| e.to_string()),
        )),
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
    };
    if let Some(event) = answered {
        answer(event);
    }
}

/// Open the library: answer [`Event::Opened`], then the listing in [`Event::Listed`]
/// parts, then [`Event::Complete`]. An error is why nothing opened, and nothing was
/// answered.
async fn open(fs: &mut impl Fs, answer: &mut impl FnMut(Event)) -> Result<(), String> {
    let indexed = matches!(fs.stat(DIR).await, Ok(Some(_)));
    // ⚠️ The index is read before anything is written: one a newer drawbar wrote keeps
    // its `.drawbar/` as that drawbar left it.
    let (sidecar, mut writable) = index(fs).await;
    let named: BTreeMap<String, u64> = sidecar
        .assets
        .iter()
        .filter_map(|(id, row)| Some((working_name(*id, row.working?), *id)))
        .collect();
    let mut working = BTreeMap::new();
    for (name, id) in &named {
        match fs.read(&format!("{WORKING}/{name}")).await {
            Ok(bytes) => {
                working.insert(*id, bytes);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            // ⚠️ A write would drop the edit the copy holds, and it may read next time.
            Err(e) => {
                writable = writable.and(Err(format!(
                    "an unsaved edit drawbar kept could not be read ({e}), so drawbar leaves \
                     the library as it is until it is opened again"
                )));
            }
        }
    }
    if writable.is_ok() {
        writable = match indexed {
            // drawbar has written here before, so it takes the lock now and a second
            // drawbar finds it taken.
            true => take(fs).await,
            false => fs
                .probe()
                .await
                .map_err(|e| format!("drawbar cannot write here: {e}")),
        };
    }
    // A library drawbar has never written holds nothing of drawbar's to sweep, and one it
    // must not write keeps what a newer drawbar left.
    let sweeps = indexed && writable.is_ok();
    let mut swept = 0;
    if sweeps {
        swept += sweep(fs, TMP, |_| true).await;
        swept += sweep(fs, WORKING, |name| !named.contains_key(name)).await;
    }
    let known: BTreeMap<LibPath, Option<Fingerprint>> = sidecar
        .assets
        .values()
        .filter_map(|row| Some((row.path.clone()?, row.fingerprint)))
        .collect();
    let walk = Walk::start(fs).await.map_err(|e| e.to_string())?;
    answer(Event::Opened(Ok(Opened {
        writable,
        indexed,
        sidecar,
        working,
        swept,
    })));
    let resting = BTreeSet::new();
    let mut lister = Lister::new(&known, false, &resting);
    lister
        .walk(fs, walk, &mut |part| answer(Event::Listed(part)))
        .await;
    let (last, strangers) = lister.finish(fs).await;
    answer(Event::Listed(last));
    let mut swept = 0;
    if sweeps {
        for temp in std::mem::take(&mut lister.temps) {
            if fs.remove_file(&temp).await.is_ok() {
                swept += 1;
            }
        }
    }
    answer(Event::Complete(Complete { strangers, swept }));
    Ok(())
}

/// The whole tree again, in one listing.
async fn scan(
    fs: &impl Fs,
    known: BTreeMap<LibPath, Fingerprint>,
    resting: &BTreeSet<LibPath>,
) -> io::Result<Listing> {
    let walk = Walk::start(fs).await?;
    let known = known
        .into_iter()
        .map(|(path, print)| (path, Some(print)))
        .collect();
    let mut lister = Lister::new(&known, true, resting);
    let mut listing = Listing::default();
    lister
        .walk(fs, walk, &mut |part| listing.extend(part))
        .await;
    let (last, strangers) = lister.finish(fs).await;
    listing.extend(last);
    listing.files.extend(strangers);
    listing.sort();
    Ok(listing)
}

/// The index, or an empty one and why nothing may be written where the index is one this
/// build must not read or rewrite.
async fn index(fs: &impl Fs) -> (Sidecar, Result<(), String>) {
    let why = match fs.read(INDEX).await {
        Ok(bytes) => match sidecar::read(&String::from_utf8_lossy(&bytes)) {
            Read::Known(sidecar) => return (sidecar, Ok(())),
            Read::Newer(version) => format!(
                "a newer drawbar wrote this library's index (version {version}), so this one \
                 only reads the library"
            ),
            Read::Unreadable(why) => format!(
                "the library's index does not read ({why}), so drawbar leaves the library as \
                 it is"
            ),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Sidecar::default(), Ok(())),
        Err(e) => format!("the library's index could not be read: {e}"),
    };
    (Sidecar::default(), Err(why))
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

/// A breadth-first walk of the tree, one folder at a time, so the top of a large tree is
/// listed first. A folder whose name starts with a dot is listed but not entered.
struct Walk {
    /// The root's entries, listed when the walk started.
    root: Option<Vec<Entry>>,
    folders: VecDeque<String>,
    /// Entries looked at so far, and the most it looks at: [`MOST_ENTRIES`].
    looked: usize,
    most: usize,
}

impl Walk {
    /// A walk whose first folder, the root, has been listed. A root that cannot be read
    /// fails it.
    async fn start(fs: &impl Fs) -> io::Result<Walk> {
        Walk::within(fs, MOST_ENTRIES).await
    }

    async fn within(fs: &impl Fs, most: usize) -> io::Result<Walk> {
        let mut walk = Walk {
            root: None,
            folders: VecDeque::new(),
            looked: 0,
            most,
        };
        walk.root = Some(walk.folder(fs, String::new()).await?);
        Ok(walk)
    }

    /// The entries of the next folder, or `None` once every folder has been listed. A
    /// folder inside that cannot be read is left unlisted, not the library.
    async fn next(&mut self, fs: &impl Fs) -> Option<Vec<Entry>> {
        if let Some(root) = self.root.take() {
            return Some(root);
        }
        let prefix = self.folders.pop_front()?;
        let unwalked = |path| Entry {
            path,
            kind: Kind::Unwalked,
        };
        if self.looked >= self.most {
            return Some(vec![unwalked(prefix)]);
        }
        Some(match self.folder(fs, prefix.clone()).await {
            Ok(entries) => entries,
            Err(_) => vec![unwalked(prefix)],
        })
    }

    async fn folder(&mut self, fs: &impl Fs, prefix: String) -> io::Result<Vec<Entry>> {
        let (found, more) = fs.children(&prefix, self.most - self.looked).await?;
        let mut entries = Vec::new();
        if more {
            entries.push(Entry {
                path: prefix.clone(),
                kind: Kind::Unwalked,
            });
        }
        for (name, kind) in found {
            self.looked += 1;
            let Some(kind) = kind else {
                continue;
            };
            let path = match prefix.is_empty() {
                true => name.clone(),
                false => format!("{prefix}/{name}"),
            };
            if matches!(kind, Kind::Dir) && !name.starts_with('.') {
                self.folders.push_back(path.clone());
            }
            entries.push(Entry { path, kind });
        }
        Ok(entries)
    }
}

/// How many entries one part of a listing covers before it is sent.
const PART: usize = 256;

/// How many bytes one part of a listing reads whole before it is sent.
const PART_BYTES: u64 = 32 << 20;

/// One listing of the tree, as its parts are gathered.
///
/// Every file `known` names is listed and read, whatever its kind and wherever the walk
/// went, except, where `loaded` says drawbar holds those files already, one whose
/// [`Stat`] is the known one, which is listed unread. Every other file is read only when
/// [`opens`] takes it and, unless the backend leaves it resting in place, it fits in
/// [`MOST_BYTES`] with what the listing holds whole already, except the files `resting`
/// names. A file the backend leaves on disk reuses the CRC `known` holds for it while its
/// [`Stat`] is the known one.
///
/// A CRC is taken only where the contents decide: a known file whose [`Stat`] moved from
/// one with a CRC, and a file at a new path whose length is that of a known file that is
/// gone. Such a file may be the known one moved, so it is held back until the walk ends.
struct Lister<'a> {
    known: &'a BTreeMap<LibPath, Option<Fingerprint>>,
    loaded: bool,
    resting: &'a BTreeSet<LibPath>,
    /// The lengths of the known files with a CRC.
    lens: BTreeSet<u64>,
    /// The known files looked at, and those of them found.
    looked: BTreeSet<LibPath>,
    found: BTreeSet<LibPath>,
    dirs: BTreeSet<LibPath>,
    /// Bytes read whole, or held whole already.
    holding: u64,
    part: Listing,
    /// Entries looked at, and bytes read whole, for the part being gathered.
    taken: usize,
    read: u64,
    /// Files at new paths that may be known ones moved.
    strangers: Vec<Found>,
    /// The temporary siblings interrupted saves left.
    temps: Vec<String>,
}

impl<'a> Lister<'a> {
    fn new(
        known: &'a BTreeMap<LibPath, Option<Fingerprint>>,
        loaded: bool,
        resting: &'a BTreeSet<LibPath>,
    ) -> Lister<'a> {
        Lister {
            lens: known
                .values()
                .filter_map(|print| Some(print.as_ref()?.contents()?.0))
                .collect(),
            known,
            loaded,
            resting,
            looked: BTreeSet::new(),
            found: BTreeSet::new(),
            dirs: BTreeSet::new(),
            holding: 0,
            part: Listing::default(),
            taken: 0,
            read: 0,
            strangers: Vec::new(),
            temps: Vec::new(),
        }
    }

    /// Take every entry the walk lists, handing `part` each part once it is full.
    async fn walk(&mut self, fs: &impl Fs, mut walk: Walk, part: &mut impl FnMut(Listing)) {
        while let Some(entries) = walk.next(fs).await.filter(|_| !fs.stopped()) {
            for entry in entries {
                self.take(fs, entry).await;
                if self.taken >= PART || self.read >= PART_BYTES {
                    part(self.cut());
                }
            }
        }
    }

    /// The part gathered so far, in path order.
    fn cut(&mut self) -> Listing {
        self.taken = 0;
        self.read = 0;
        let mut part = std::mem::take(&mut self.part);
        part.sort();
        part
    }

    async fn take(&mut self, fs: &impl Fs, entry: Entry) {
        self.taken += 1;
        let leaf = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        if entry.path.split('/').any(|part| part.starts_with('.')) {
            let file = matches!(entry.kind, Kind::File(_) | Kind::Unread(_) | Kind::Other);
            if file && leaf.starts_with('.') && leaf.ends_with(TEMP) {
                self.temps.push(entry.path);
            }
            return;
        }
        let Some(path) = LibPath::parse(&entry.path) else {
            return;
        };
        match entry.kind {
            Kind::Dir => {
                if self.dirs.insert(path.clone()) {
                    self.part.dirs.push(path);
                }
            }
            Kind::Unwalked => self.part.unwalked.push(path),
            Kind::File(stat) => match self.known.get(&path) {
                Some(print) => self.held(fs, path, stat, *print).await,
                None => self.arrived(fs, path, stat).await,
            },
            // A file drawbar holds is looked at again, whatever the walk made of it.
            Kind::Unread(_) | Kind::Other if self.known.contains_key(&path) => {
                self.restat(fs, path).await
            }
            Kind::Unread(why) => self.part.unread.push((path, why)),
            Kind::Other => self.part.others.push(path),
        }
    }

    /// Look at a known file the walk listed without a [`Stat`], or did not reach. Its
    /// folders are listed, though the walk may not have listed inside them.
    async fn restat(&mut self, fs: &impl Fs, path: LibPath) {
        let print = self.known.get(&path).copied().flatten();
        match fs.stat(path.as_str()).await {
            Ok(Some(stat)) => {
                let mut dir = path.parent();
                while !dir.is_root() && self.dirs.insert(dir.clone()) {
                    self.part.dirs.push(dir.clone());
                    dir = dir.parent();
                }
                self.held(fs, path, stat, print).await
            }
            Ok(None) => {
                self.looked.insert(path);
            }
            Err(e) => {
                self.looked.insert(path.clone());
                self.part.unread.push((path, e.to_string()));
            }
        }
    }

    async fn held(&mut self, fs: &impl Fs, path: LibPath, stat: Stat, print: Option<Fingerprint>) {
        self.looked.insert(path.clone());
        let unmoved = print.filter(|print| print.stat() == stat);
        if self.loaded && unmoved.is_some() {
            if !self.resting.contains(&path) {
                self.holding = self.holding.saturating_add(stat.len);
            }
            self.found.insert(path.clone());
            self.push(Found {
                path,
                stat,
                bytes: None,
                file: None,
                crc: None,
            });
            return;
        }
        let (bytes, file) = match read(fs, &path, unmoved).await {
            Ok(read) => read,
            Err(e) => return self.part.unread.push((path, e.to_string())),
        };
        if bytes.is_some() {
            self.holding = self.holding.saturating_add(stat.len);
        }
        let mut found = Found {
            path,
            stat,
            bytes,
            file,
            crc: None,
        };
        if unmoved.is_none() && print.is_some_and(|print| print.crc.is_some()) {
            found.crc = crc(&found);
        }
        self.found.insert(found.path.clone());
        self.push(found);
    }

    async fn arrived(&mut self, fs: &impl Fs, path: LibPath, stat: Stat) {
        if !opens(path.leaf()) {
            return self.part.others.push(path);
        }
        let file = match fs.rest(path.as_str(), None).await {
            Ok(file) => file,
            Err(e) => return self.part.unread.push((path, e.to_string())),
        };
        let bytes = match file {
            Some(_) => None,
            None if self.holding.saturating_add(stat.len) > MOST_BYTES => {
                return self.part.unread.push((path, too_much()));
            }
            None => match fs.read(path.as_str()).await {
                Ok(bytes) => Some(bytes),
                Err(e) => return self.part.unread.push((path, e.to_string())),
            },
        };
        if bytes.is_some() {
            self.holding = self.holding.saturating_add(stat.len);
        }
        let found = Found {
            path,
            stat,
            bytes,
            file,
            crc: None,
        };
        match self.lens.contains(&stat.len) {
            true => self.strangers.push(found),
            false => self.push(found),
        }
    }

    fn push(&mut self, found: Found) {
        let read = found.bytes.as_ref().map_or(0, |bytes| bytes.len() as u64);
        self.read = self.read.saturating_add(read);
        self.part.files.push(found);
    }

    /// Look for the known files the walk did not reach, and take the CRC of each file
    /// held back whose length is that of a known file still not found. Returns the last
    /// part, and the files held back.
    async fn finish(&mut self, fs: &impl Fs) -> (Listing, Vec<Found>) {
        let unlooked: Vec<LibPath> = self
            .known
            .keys()
            .filter(|path| !self.looked.contains(*path))
            .filter(|path| !path.components().any(|part| part.starts_with('.')))
            .cloned()
            .collect();
        for path in unlooked {
            self.restat(fs, path).await;
        }
        let missing: BTreeSet<u64> = self
            .known
            .iter()
            .filter(|(path, _)| !self.found.contains(*path))
            .filter_map(|(_, print)| Some(print.as_ref()?.contents()?.0))
            .collect();
        let mut strangers = std::mem::take(&mut self.strangers);
        for found in &mut strangers {
            if missing.contains(&found.stat.len) {
                found.crc = crc(found);
            }
        }
        (self.cut(), strangers)
    }
}

/// A file read for a listing: left resting in place where the backend reads it by range,
/// with the CRC of `known` where its stat says that is still the file, and otherwise read
/// whole.
async fn read(
    fs: &impl Fs,
    path: &LibPath,
    known: Option<Fingerprint>,
) -> io::Result<(Option<Vec<u8>>, Option<Arc<OnDisk>>)> {
    match fs.rest(path.as_str(), known).await? {
        Some(file) => Ok((None, Some(file))),
        None => Ok((Some(fs.read(path.as_str()).await?), None)),
    }
}

/// CRC-32 over the whole of what a listing read, or `None` where the file could not be
/// read through.
fn crc(found: &Found) -> Option<u32> {
    match (&found.bytes, &found.file) {
        (Some(bytes), _) => Some(nord_format::crc::crc32(bytes)),
        (None, Some(file)) => file.crc().ok(),
        (None, None) => None,
    }
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

#[cfg(test)]
mod tests {
    use std::future::Future;

    use super::*;
    use crate::testing::{on_disk, sample_bytes, Temp};

    /// A library whose files claim whatever size they are given, without taking it. The
    /// ones in `rests` are left in place, as the desktop leaves a sample instrument, and
    /// the folders in `unreadable` cannot be read.
    struct Claimed {
        files: BTreeMap<String, u64>,
        rests: BTreeMap<String, Arc<OnDisk>>,
        unreadable: BTreeSet<String>,
    }

    impl Claimed {
        fn of(files: impl IntoIterator<Item = (String, u64)>) -> Claimed {
            Claimed {
                files: files.into_iter().collect(),
                rests: BTreeMap::new(),
                unreadable: BTreeSet::new(),
            }
        }
    }

    fn stat(len: u64) -> Stat {
        Stat {
            len,
            modified: Some(1),
        }
    }

    fn refused() -> io::Error {
        io::ErrorKind::Unsupported.into()
    }

    impl Fs for Claimed {
        async fn prepare(&mut self) -> io::Result<()> {
            Err(refused())
        }
        async fn lock(&mut self) -> io::Result<bool> {
            Err(refused())
        }
        async fn children(&self, dir: &str, room: usize) -> io::Result<Children> {
            if self.unreadable.contains(dir) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let mut found = BTreeMap::new();
            for (path, len) in &self.files {
                let inside = match dir.is_empty() {
                    true => Some(path.as_str()),
                    false => path.strip_prefix(dir).and_then(|at| at.strip_prefix('/')),
                };
                let Some(inside) = inside else {
                    continue;
                };
                let (name, kind) = match inside.split_once('/') {
                    Some((folder, _)) => (folder, Kind::Dir),
                    None => (inside, Kind::File(stat(*len))),
                };
                found.insert(name.to_string(), Some(kind));
            }
            let more = found.len() > room;
            Ok((found.into_iter().take(room).collect(), more))
        }
        async fn names(&self, _: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
        async fn read(&self, path: &str) -> io::Result<Vec<u8>> {
            Ok(path.as_bytes().to_vec())
        }
        async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
            Ok(self.files.get(path).map(|len| stat(*len)))
        }
        async fn create(&mut self, _: &str, _: &[u8]) -> io::Result<()> {
            Err(refused())
        }
        async fn replace(&mut self, _: &str, _: &[u8]) -> io::Result<()> {
            Err(refused())
        }
        async fn rename(&mut self, _: &str, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn make_dir(&mut self, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn remove_file(&mut self, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn remove_dir(&mut self, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn rest(
            &self,
            path: &str,
            _: Option<Fingerprint>,
        ) -> io::Result<Option<Arc<OnDisk>>> {
            Ok(self.rests.get(path).cloned())
        }
    }

    /// The answer of a future that never waits.
    fn now<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(answer) => answer,
            std::task::Poll::Pending => panic!("the stand-in never waits"),
        }
    }

    /// A sample instrument of the whole budget's size, left in its file, and a small
    /// program after it in path order.
    fn library(dir: &Temp) -> Claimed {
        let file = on_disk(dir, "Marimba.nsmp", &sample_bytes());
        Claimed {
            rests: BTreeMap::from([("Marimba.nsmp".to_string(), file)]),
            ..Claimed::of([
                ("Marimba.nsmp".to_string(), MOST_BYTES),
                ("Small.ne5p".to_string(), 10),
            ])
        }
    }

    fn paths(unread: &[(LibPath, String)]) -> Vec<&str> {
        unread.iter().map(|(path, _)| path.as_str()).collect()
    }

    /// Every part of one listing, as an open sends them, the files held back last.
    fn parts(
        fs: &Claimed,
        known: &BTreeMap<LibPath, Option<Fingerprint>>,
        loaded: bool,
        resting: &BTreeSet<LibPath>,
    ) -> Vec<Listing> {
        now(async {
            let walk = Walk::start(fs).await.unwrap();
            let mut lister = Lister::new(known, loaded, resting);
            let mut parts = Vec::new();
            lister.walk(fs, walk, &mut |part| parts.push(part)).await;
            let (last, strangers) = lister.finish(fs).await;
            parts.push(last);
            parts.push(Listing {
                files: strangers,
                ..Listing::default()
            });
            parts
        })
    }

    /// One listing, whole.
    fn listed(fs: &Claimed, known: &BTreeMap<LibPath, Option<Fingerprint>>) -> Listing {
        let mut listing = Listing::default();
        for part in parts(fs, known, false, &BTreeSet::new()) {
            listing.extend(part);
        }
        listing.sort();
        listing
    }

    #[test]
    fn a_file_left_in_place_takes_none_of_what_a_listing_reads() {
        let dir = Temp::new();
        let fs = library(&dir);
        let listed = listed(&fs, &BTreeMap::new());
        assert_eq!(paths(&listed.unread), [] as [&str; 0]);
        let read: Vec<(&str, bool, bool)> = listed
            .files
            .iter()
            .map(|found| {
                (
                    found.path.as_str(),
                    found.bytes.is_some(),
                    found.file.is_some(),
                )
            })
            .collect();
        assert_eq!(
            read,
            [("Marimba.nsmp", false, true), ("Small.ne5p", true, false)],
            "(path, read whole, left in place)"
        );
    }

    /// On a rescan the listing does not look at a file it holds again, so the app says
    /// which of them are left in place.
    #[test]
    fn a_held_file_counts_against_a_rescan_only_where_it_is_held_whole() {
        let dir = Temp::new();
        let fs = library(&dir);
        let marimba = LibPath::parse("Marimba.nsmp").unwrap();
        let known = BTreeMap::from([(marimba.clone(), Fingerprint::unread(stat(MOST_BYTES)))]);

        let resting = BTreeSet::from([marimba]);
        let listed = now(scan(&fs, known.clone(), &resting)).unwrap();
        assert_eq!(paths(&listed.unread), [] as [&str; 0], "left in place");

        let listed = now(scan(&fs, known, &BTreeSet::new())).unwrap();
        assert_eq!(paths(&listed.unread), ["Small.ne5p"], "held whole");
    }

    fn walked(fs: &Claimed, most: usize) -> Vec<(String, bool)> {
        let entries = now(async {
            let mut walk = Walk::within(fs, most).await.unwrap();
            let mut entries = Vec::new();
            while let Some(folder) = walk.next(fs).await {
                entries.extend(folder);
            }
            entries
        });
        entries
            .into_iter()
            .map(|entry| (entry.path, matches!(entry.kind, Kind::Unwalked)))
            .collect()
    }

    /// Breadth first up to the bound: the folder the bound cuts short, and every folder
    /// past it, is marked not walked in full. A folder whose name starts with a dot is
    /// listed but not entered.
    #[test]
    fn the_walk_looks_at_most_entries_breadth_first() {
        const MOST: usize = 20;
        let deep = (0..MOST).map(|n| (format!("a/{n:05}.ne5p"), 1));
        let top = ["top.ne5p", "b/deep.ne5p", ".hidden/x.ne5p"].map(|path| (path.to_string(), 1));
        let listed = walked(&Claimed::of(deep.chain(top)), MOST);
        let unwalked: Vec<&str> = listed
            .iter()
            .filter(|(_, unwalked)| *unwalked)
            .map(|(path, _)| path.as_str())
            .collect();
        assert_eq!(unwalked, ["a", "b"]);
        assert_eq!(listed.len() - unwalked.len(), MOST, "entries looked at");
        assert!(listed.iter().any(|(path, _)| path == ".hidden"));
        assert!(!listed.iter().any(|(path, _)| path.starts_with(".hidden/")));
        assert!(!listed.iter().any(|(path, _)| path == "b/deep.ne5p"));
    }

    /// A folder inside that cannot be read is listed as not walked in full, and the rest
    /// of the library still lists. A root that cannot be read fails the listing.
    #[test]
    fn an_unreadable_folder_is_left_unwalked_and_an_unreadable_root_fails() {
        let files = [("a/x.ne5p", 1), ("top.ne5p", 1)].map(|(path, len)| (path.to_string(), len));
        let mut fs = Claimed::of(files);
        fs.unreadable.insert("a".to_string());
        let listed = walked(&fs, MOST_ENTRIES);
        let listed: Vec<(&str, bool)> = listed
            .iter()
            .map(|(path, unwalked)| (path.as_str(), *unwalked))
            .collect();
        assert_eq!(listed, [("a", false), ("top.ne5p", false), ("a", true)]);

        fs.unreadable.insert(String::new());
        assert!(now(Walk::start(&fs)).is_err());
    }

    /// An open's listing comes in parts of a bounded size, breadth first: no entry comes
    /// before one nearer the root, and the first part is the top of the tree.
    #[test]
    fn a_listing_comes_in_parts_breadth_first() {
        let levels = ["", "a/", "a/b/", "a/b/c/"];
        let files = levels
            .iter()
            .flat_map(|level| (0..PART).map(move |n| (format!("{level}{n:04}.ne5p"), 1)));
        let fs = Claimed::of(files);
        let parts = parts(&fs, &BTreeMap::new(), false, &BTreeSet::new());
        assert!(parts.len() > levels.len(), "{} parts", parts.len());
        let depths: Vec<Vec<usize>> = parts
            .iter()
            .map(|part| {
                let files = part.files.iter().map(|found| &found.path);
                files.chain(&part.dirs).map(LibPath::depth).collect()
            })
            .collect();
        let mut deepest = 0;
        for (at, part) in depths.iter().enumerate() {
            assert!(part.len() <= PART, "part {at} has {} entries", part.len());
            let least = part.iter().min().copied().unwrap_or(deepest);
            assert!(least >= deepest, "part {at} goes back up to depth {least}");
            deepest = part.iter().max().copied().unwrap_or(deepest);
        }
        assert!(depths[0].iter().all(|depth| *depth == 0), "{:?}", depths[0]);
        let listed: usize = parts.iter().map(|part| part.files.len()).sum();
        assert_eq!(listed, levels.len() * PART, "every file");
    }

    /// A file at a new path waits for the end of the listing only where its length is that
    /// of a file drawbar knew, and has its CRC taken only where that file is gone.
    #[test]
    fn a_file_that_may_be_one_moved_waits_for_the_end_and_is_fingerprinted() {
        let fs = Claimed::of(["Grand.ne5p", "Moved.ne5p", "Other.ne5p"].map(|path| {
            let len = match path {
                "Other.ne5p" => 3,
                _ => path.len() as u64,
            };
            (path.to_string(), len)
        }));
        let print = |len| {
            Some(Fingerprint {
                crc: Some(7),
                ..Fingerprint::unread(stat(len))
            })
        };
        let known = BTreeMap::from([
            (LibPath::parse("Gone.ne5p").unwrap(), print(10)),
            (LibPath::parse("Grand.ne5p").unwrap(), print(10)),
        ]);
        let parts = parts(&fs, &known, false, &BTreeSet::new());
        let strangers = &parts[parts.len() - 1].files;
        let held: Vec<(&str, bool)> = strangers
            .iter()
            .map(|found| (found.path.as_str(), found.crc.is_some()))
            .collect();
        assert_eq!(held, [("Moved.ne5p", true)], "(path, fingerprinted)");
        let first: Vec<&str> = parts[0]
            .files
            .iter()
            .map(|found| found.path.as_str())
            .collect();
        assert_eq!(first, ["Grand.ne5p", "Other.ne5p"]);
        assert!(parts[0].files.iter().all(|found| found.crc.is_none()));
    }
}
