//! Running a [`Cmd`] against a library's files, the same way whichever backend holds
//! them.
//!
//! The crash-safety of every write rests on [`Fs::stage`] and [`Fs::place`]: a file is
//! written somewhere else first and appears at its path whole. Everything written in
//! flight is either under `.drawbar/tmp/` or a hidden `.<name>.drawbar-tmp` sibling, and
//! opening the library sweeps both.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::Arc;

use super::sidecar::{self, Keeps, Read, Sidecar};
use super::{
    names, Cmd, Complete, Event, Failure, Fingerprint, Found, Holds, LibPath, Listing, Opened,
    Outside, Source, Stat,
};
use crate::ondisk::OnDisk;
use crate::rewrite::{self, Rewrite};

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

/// The most bytes of a library's files drawbar reads whole into memory, counting what it
/// holds whole already. A file left resting in place, read by range, is not counted.
pub const MOST_BYTES: u64 = 1 << 30;

/// Whether drawbar opens a file of this name, by its extension: a Nord file, a Sample
/// Editor project, a note, a MIDI file or a SysEx dump. Any other file is listed by its
/// name only.
pub fn opens(name: &str) -> bool {
    crate::browser::tagged(name).is_some()
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
    /// The next command sent and not yet run, where one is already waiting. A listing
    /// takes the ones it may run between two folders.
    fn waiting(&mut self) -> Option<Cmd>;
    /// Put back a command [`Fs::waiting`] gave, to run before any other.
    fn hold(&mut self, cmd: Cmd);
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
    /// still counts toward [`MOST_ENTRIES`]: one gone, or a file `known` names, which is
    /// not looked at. Never [`Kind::Unwalked`].
    async fn children(
        &self,
        dir: &str,
        room: usize,
        known: &dyn Fn(&str) -> bool,
    ) -> io::Result<Children>;
    /// The names in one folder.
    async fn names(&self, dir: &str) -> io::Result<Vec<String>>;
    async fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// CRC-32 over the whole file at `path`, taken in one streaming pass that never holds
    /// it whole.
    async fn crc(&self, path: &str) -> io::Result<u32>;
    /// The file at `path`, or `None` where no file is: nothing, a folder, or a link.
    async fn stat(&self, path: &str) -> io::Result<Option<Stat>>;
    /// [`Fs::stat`] of each of `paths`, in order.
    async fn stats(&self, paths: &[&str]) -> Vec<io::Result<Option<Stat>>> {
        let mut stats = Vec::with_capacity(paths.len());
        for path in paths {
            stats.push(self.stat(path).await);
        }
        stats
    }
    /// A temporary a write is staged in, until it is put at its path or let go.
    type Temp;
    /// Write `what` into a new temporary for `path`, flushed, never through anything
    /// already at the temporary's name. Nothing is left where it fails. A source that
    /// changed since it was read is refused with [`crate::rewrite::changed`].
    async fn stage(&mut self, path: &str, what: Staged<'_>) -> io::Result<Self::Temp>;
    /// Put a staged temporary at `path`: over whatever is there where `over` is set, and
    /// otherwise only where nothing is, refused as [`io::ErrorKind::AlreadyExists`].
    /// Afterwards, or after a crash at any point, the path holds the old contents or the
    /// new, never part of either. A temporary not put is let go.
    async fn place(&mut self, temp: Self::Temp, path: &str, over: bool) -> io::Result<()>;
    /// Let go of a staged temporary that is not to be put anywhere.
    async fn discard(&mut self, temp: Self::Temp);
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

/// What a write stages before it puts the file at its path.
pub enum Staged<'a> {
    Bytes(&'a [u8]),
    /// A copy of a file outside the library, never held whole.
    Outside(&'a Outside),
    /// A copy of the library's own file at this path, never held whole.
    Library(&'a str),
    /// The file an edit makes of a piano or sample instrument resting in the library,
    /// read by range and never held whole.
    Edited(&'a OnDisk, &'a Rewrite),
}

/// What a write may put its file over.
#[derive(Clone, Copy)]
enum Over {
    /// Nothing: the path must be free.
    Nothing,
    /// Whatever is there.
    Anything,
    /// The file there, while it has this [`Stat`].
    Held(Stat),
}

/// Write `what` to `path` through a temporary, over what `over` allows. A held file whose
/// stat moved by the time the temporary is written is refused with [`moved`], and nothing
/// is put over it.
///
/// ⚠️ A window remains between that last look and the rename, since no portable call
/// renames only over a file that is still the one looked at. It is one stat and one
/// rename long, after the whole write rather than before it.
async fn put(fs: &mut impl Fs, path: &str, what: Staged<'_>, over: Over) -> io::Result<()> {
    let temp = fs.stage(path, what).await?;
    if let Over::Held(held) = over {
        if !matches!(fs.stat(path).await, Ok(Some(now)) if now == held) {
            fs.discard(temp).await;
            return Err(moved());
        }
    }
    fs.place(temp, path, !matches!(over, Over::Nothing)).await
}

/// What a write may put its file over, where the file at `path` must still hold what
/// `expect` says: refused with [`Failure::Moved`] where it does not.
async fn held(fs: &impl Fs, path: &LibPath, expect: &Fingerprint) -> Result<Over, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    if still(fs, path, expect).await.map_err(io)? != Some(true) {
        return Err(Failure::Moved);
    }
    let stat = fs.stat(path.as_str()).await.map_err(io)?;
    Ok(Over::Held(stat.ok_or(Failure::Moved)?))
}

/// Why a write refused to put its file over one whose stat moved while it was staged.
#[derive(Debug)]
struct Moved;

impl std::fmt::Display for Moved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the file changed on disk while its new contents were written")
    }
}

impl std::error::Error for Moved {}

fn moved() -> io::Error {
    io::Error::other(Moved)
}

/// A write refused for a target that is not the file it was to go over, or a name taken
/// first, as [`Failure::Moved`]; any other as why.
fn refusal(e: io::Error) -> Failure {
    let taken = e.kind() == io::ErrorKind::AlreadyExists;
    match taken || e.get_ref().is_some_and(|inner| inner.is::<Moved>()) {
        true => Failure::Moved,
        false => Failure::Io(e.to_string()),
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
    match cmd {
        Cmd::Open => {
            if let Err(why) = open(fs, answer).await {
                answer(Event::Opened(Err(why)));
            }
        }
        cmd => step(fs, cmd, &mut 0, answer).await,
    }
}

/// Whether a command writes, and so first takes the library.
fn writes(cmd: &Cmd) -> bool {
    !matches!(
        cmd,
        Cmd::Open
            | Cmd::Scan { .. }
            | Cmd::Check { .. }
            | Cmd::Walk(_)
            | Cmd::Read { .. }
            | Cmd::Fingerprint(_)
    )
}

/// Run any command but an open, which runs once, first. `ran` counts it among the
/// commands run, and any it runs while it lasts.
async fn step(fs: &mut impl Fs, cmd: Cmd, ran: &mut u64, answer: &mut impl FnMut(Event)) {
    *ran += 1;
    if writes(&cmd) {
        if let Err(why) = take(fs).await {
            match cmd {
                Cmd::Move { from, to } => {
                    let result = Err(why.clone());
                    answer(Event::Moved { from, to, result });
                }
                Cmd::Import { id, path, .. } => {
                    let result = Err(Failure::Io(why.clone()));
                    answer(Event::Imported { id, path, result });
                }
                Cmd::Rewrite { id, path, .. } => {
                    let result = Err(Failure::Io(why.clone()));
                    answer(Event::Rewritten { id, path, result });
                }
                _ => {}
            }
            return answer(Event::ReadOnly(why));
        }
    }
    let answered = match cmd {
        Cmd::Open => Some(Event::Opened(
            Err("the library is open already".to_string()),
        )),
        Cmd::Scan { known } => Some(Event::Scanned(
            scan(fs, &known, ran, answer)
                .await
                .map_err(|e| e.to_string()),
        )),
        Cmd::Check { known } => Some(Event::Checked(Ok(check(fs, &known).await))),
        // Outside an open's listing the whole tree has been listed already.
        Cmd::Walk(dir) => Some(Event::Walked { dir, ran: 0 }),
        Cmd::Read { files, room } => Some(Event::Read(read_all(fs, files, room).await)),
        Cmd::Fingerprint(files) => Some(Event::Fingerprinted(fingerprints(fs, files).await)),
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
        Cmd::Rewrite {
            id,
            path,
            from,
            edit,
            expect,
        } => {
            let result = rewrite_over(fs, &path, &from, &edit, expect).await;
            Some(Event::Rewritten { id, path, result })
        }
        Cmd::Move { from, to } => {
            let result = fs.rename(from.as_str(), to.as_str()).await;
            let result = result.map_err(|e| e.to_string());
            Some(Event::Moved { from, to, result })
        }
        Cmd::Import {
            id,
            path,
            from,
            expect,
        } => {
            let result = import(fs, &path, &from, expect).await;
            Some(Event::Imported { id, path, result })
        }
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

/// Run the reads waiting, each as it comes, until a command that is not one is next.
/// That one is left to run after the rescan, and so is every command behind it.
async fn reads_between(fs: &mut impl Fs, ran: &mut u64, answer: &mut impl FnMut(Event)) {
    while let Some(cmd) = fs.waiting() {
        match cmd {
            Cmd::Read { files, room } => {
                *ran += 1;
                answer(Event::Read(read_all(fs, files, room).await))
            }
            cmd => return fs.hold(cmd),
        }
    }
}

/// Open the library: answer [`Event::Opened`], then the listing in [`Event::Listed`]
/// parts, then [`Event::Complete`]. An error is why nothing opened, and nothing was
/// answered.
async fn open(fs: &mut impl Fs, answer: &mut impl FnMut(Event)) -> Result<(), String> {
    let indexed = fs.names(DIR).await.is_ok();
    // ⚠️ The index is read before anything is written: one a newer drawbar wrote keeps
    // its `.drawbar/` as that drawbar left it.
    let (sidecar, mut writable) = index(fs).await;
    let named: BTreeMap<String, (u64, Keeps)> = sidecar
        .assets
        .iter()
        .filter_map(|(id, row)| {
            let copy = row.working?;
            Some((working_name(*id, copy.generation), (*id, copy.keeps)))
        })
        .collect();
    let mut working = BTreeMap::new();
    for (name, (id, keeps)) in &named {
        let read = fs.read(&format!("{WORKING}/{name}")).await;
        // An edit's copy is read through here, so one this build cannot read is not
        // taken for no edit at all.
        let read = read.and_then(|bytes| match keeps {
            Keeps::Bytes => Ok(bytes),
            Keeps::Edit => rewrite::Edit::from_working(&bytes)
                .map(|_| bytes)
                .map_err(io::Error::other),
        });
        match read {
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
    let mut rescued = Vec::new();
    if sweeps {
        // ⚠️ A rescue is a slot's only copy, left by a write that did not finish.
        let rescue = |name: &str| name.starts_with(nord_usb::envelope::RESCUED);
        swept += sweep(fs, TMP, |name| !rescue(name)).await;
        swept += sweep(fs, WORKING, |name| !named.contains_key(name)).await;
        let names = fs.names(TMP).await.unwrap_or_default();
        for name in names.into_iter().filter(|name| rescue(name)) {
            if let Ok(Some(stat)) = fs.stat(&format!("{TMP}/{name}")).await {
                rescued.push((name, stat));
            }
        }
    }
    let rows: Vec<Row> = sidecar
        .assets
        .values()
        .filter_map(|row| {
            Some(Row {
                path: row.path.clone()?,
                print: row.fingerprint,
                working: row.working.is_some(),
            })
        })
        .collect();
    let stranded = match sweeps {
        true => unstrand(fs, &rows).await,
        false => Vec::new(),
    };
    let walk = Walk::start(fs).await.map_err(|e| e.to_string())?;
    answer(Event::Opened(Ok(Opened {
        writable,
        indexed,
        sidecar,
        working,
        swept,
        stranded,
        rescued,
    })));
    let mut lister = Lister::default();
    lister.list(fs, rows, walk, answer).await;
    let ran = lister.ran;
    let mut swept = 0;
    if sweeps {
        for temp in std::mem::take(&mut lister.temps) {
            if fs.remove_file(&temp).await.is_ok() {
                swept += 1;
            }
        }
    }
    answer(Event::Complete(Complete {
        strangers: lister.recognized,
        gone: lister.gone,
        swept,
        ran,
    }));
    Ok(())
}

/// The whole tree again, in one listing. The reads sent while it runs are answered
/// between its folders.
/// Put back each folder an interrupted rename left under the name it moved it through
/// ([`names::aside`]), in the folders where the index's rows lie: under the spelling of
/// its name the rows use, so they match it, or under its own. Returns those left where
/// they are, because another entry already has the name.
///
/// ⚠️ Only the folders above a row are looked in. One no row lies under is listed under
/// the name it moved through, which loses nothing, since no row is to match it.
async fn unstrand(fs: &mut impl Fs, rows: &[Row]) -> Vec<LibPath> {
    let mut named: BTreeMap<String, BTreeSet<String>> =
        BTreeMap::from([(String::new(), BTreeSet::new())]);
    for row in rows {
        let parts: Vec<&str> = row.path.components().collect();
        for at in 0..parts.len().saturating_sub(1) {
            let dir = parts[..at].join("/");
            named.entry(dir).or_default().insert(parts[at].to_string());
        }
    }
    let mut stranded = Vec::new();
    for (dir, folders) in named {
        let Ok(held) = fs.names(&dir).await else {
            continue;
        };
        for name in &held {
            let Some(folder) = names::moved_through(name) else {
                continue;
            };
            let from = joined(&dir, name);
            if fs.stat(&from).await.ok().flatten().is_some() {
                continue;
            }
            let key = names::key(folder);
            let wanted = folders.iter().find(|row| names::key(row) == key);
            let wanted = wanted.map_or(folder, String::as_str);
            let taken = held
                .iter()
                .any(|other| other != name && names::key(other) == key);
            let to = joined(&dir, wanted);
            if taken || fs.rename(&from, &to).await.is_err() {
                stranded.extend(LibPath::parse(&from));
            }
        }
    }
    stranded
}

/// `name` in the folder `dir`, both joined by `/`.
fn joined(dir: &str, name: &str) -> String {
    match dir.is_empty() {
        true => name.to_string(),
        false => format!("{dir}/{name}"),
    }
}

async fn scan(
    fs: &mut impl Fs,
    known: &BTreeMap<LibPath, (Fingerprint, Holds)>,
    ran: &mut u64,
    answer: &mut impl FnMut(Event),
) -> io::Result<Listing> {
    let mut walk = Walk::start(fs).await?;
    let mut rescan = Rescan::new(known);
    while let Some(entries) = walk.next(fs).await.filter(|_| !fs.stopped()) {
        for entry in entries {
            rescan.take(fs, entry).await;
        }
        reads_between(fs, ran, answer).await;
    }
    let mut listing = rescan.finish(fs).await;
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
    /// Every folder listed or waiting to be, so none is listed twice.
    seen: BTreeSet<String>,
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
            seen: BTreeSet::from([String::new()]),
            looked: 0,
            most,
        };
        walk.root = Some(walk.folder(fs, String::new(), &BTreeSet::new()).await?);
        Ok(walk)
    }

    /// The entries of the next folder, or `None` once every folder has been listed. A
    /// folder inside that cannot be read is left unlisted, not the library.
    async fn next(&mut self, fs: &impl Fs) -> Option<Vec<Entry>> {
        self.next_of(fs, &[], &BTreeSet::new()).await
    }

    /// [`Walk::next`], taking first a folder in one of `urgent`, where any is waiting, and
    /// leaving out the files in `told` unlooked at.
    async fn next_of(
        &mut self,
        fs: &impl Fs,
        urgent: &[LibPath],
        told: &BTreeSet<LibPath>,
    ) -> Option<Vec<Entry>> {
        if let Some(root) = self.root.take() {
            return Some(root);
        }
        let first = self
            .folders
            .iter()
            .position(|folder| urgent.iter().any(|dir| nested(folder, dir.as_str())));
        let prefix = self.folders.remove(first.unwrap_or(0))?;
        let unwalked = |path| Entry {
            path,
            kind: Kind::Unwalked,
        };
        if self.looked >= self.most {
            return Some(vec![unwalked(prefix)]);
        }
        Some(match self.folder(fs, prefix.clone(), told).await {
            Ok(entries) => entries,
            Err(_) => vec![unwalked(prefix)],
        })
    }

    async fn folder(
        &mut self,
        fs: &impl Fs,
        prefix: String,
        told: &BTreeSet<LibPath>,
    ) -> io::Result<Vec<Entry>> {
        let known = |name: &str| {
            let path = || LibPath::parse(&joined(&prefix, name));
            !told.is_empty() && path().is_some_and(|path| told.contains(&path))
        };
        let (found, more) = fs
            .children(&prefix, self.most - self.looked, &known)
            .await?;
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
            if matches!(kind, Kind::Dir) && !name.starts_with('.') && self.seen.insert(path.clone())
            {
                self.folders.push_back(path.clone());
            }
            entries.push(Entry { path, kind });
        }
        Ok(entries)
    }

    /// Whether `dir`, a folder in it, or a folder it is in is still to be listed. A
    /// folder `dir` is in may hold the way down to it.
    fn waits_in(&self, dir: &LibPath) -> bool {
        let dir = dir.as_str();
        self.root.is_some() || self.folders.iter().any(|folder| nested(folder, dir))
    }

    /// Follow a folder renamed while the walk is in flight.
    fn moved(&mut self, from: &str, to: &str) {
        let moved = |path: &String| match within(path, from) {
            true => format!("{to}{}", &path[from.len()..]),
            false => path.clone(),
        };
        self.folders = self.folders.iter().map(moved).collect();
        self.seen = self.seen.iter().map(moved).collect();
    }

    /// Follow a folder made or removed while the walk is in flight.
    fn made(&mut self, dir: &str) {
        self.seen.insert(dir.to_string());
    }

    fn removed(&mut self, dir: &str) {
        self.folders.retain(|folder| !within(folder, dir));
        self.seen.retain(|folder| !within(folder, dir));
    }
}

/// Whether `path` is `dir` or inside it, both joined by `/`. Everything is inside the
/// root.
fn within(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Whether one of two folders is the other or inside it.
fn nested(a: &str, b: &str) -> bool {
    within(a, b) || within(b, a)
}

/// How many entries one part of a listing covers before it is sent.
const PART: usize = 256;

/// How many files the CRCs taken after the walk read at most between two runs of the
/// commands waiting, and how many of their bytes.
const RECOGNIZE: (usize, u64) = (64, 16 << 20);

/// One row of the index, as an open looks for it.
struct Row {
    path: LibPath,
    print: Option<Fingerprint>,
    /// It holds a working copy, which is compared with the file, so the file is read.
    working: bool,
}

/// The listing an open sends in parts. It reads nothing but the files under working
/// copies and, at the end, the files that may be rows of the index moved.
///
/// Every row of the index is looked at first, where the index says it is, and sent
/// before the walk begins; the walk leaves those files out. A file the walk finds whose
/// length is that of a row not found, where that row's CRC is known, may be that row's
/// file moved: it is sent like any other, and once the walk ends its CRC is taken, a few
/// files at a time.
///
/// The commands sent meanwhile run between folders, each after the part gathered before
/// it is sent, and the listing follows what each did to the tree.
#[derive(Default)]
struct Lister {
    /// Files already sent, or written by a command since the walk began, which the walk
    /// leaves out.
    told: BTreeSet<LibPath>,
    /// The lengths of the rows not found whose CRC is known.
    lens: BTreeSet<u64>,
    /// Every folder sent so far.
    dirs: BTreeSet<LibPath>,
    part: Listing,
    /// Entries looked at for the part being gathered.
    taken: usize,
    /// Files at new paths that may be rows moved, whose CRC is still to be taken, those
    /// whose CRC has been, and those gone by then.
    strangers: VecDeque<Found>,
    recognized: Vec<Found>,
    gone: Vec<LibPath>,
    /// The temporary siblings interrupted saves left.
    temps: Vec<String>,
    /// How many commands sent since the open have run.
    ran: u64,
    /// The folders [`Cmd::Walk`] asked for, listed ahead of the rest.
    urgent: Vec<LibPath>,
}

/// What a command a listing ran did to the tree, which the rest of the listing follows.
enum Follow {
    Moved { from: LibPath, to: LibPath },
    Wrote(LibPath),
    Made(LibPath),
    Removed(LibPath),
    Nothing,
}

impl Follow {
    fn of(cmd: &Cmd) -> Follow {
        match cmd {
            Cmd::Move { from, to } => Follow::Moved {
                from: from.clone(),
                to: to.clone(),
            },
            Cmd::Save { path, .. } | Cmd::Import { path, .. } | Cmd::Rewrite { path, .. } => {
                Follow::Wrote(path.clone())
            }
            Cmd::MakeDir(path) => Follow::Made(path.clone()),
            Cmd::RemoveDir(path) => Follow::Removed(path.clone()),
            _ => Follow::Nothing,
        }
    }
}

/// Whether an answer says the command did not do what it was sent to.
fn failed(event: &Event) -> bool {
    matches!(
        event,
        Event::Failed(_)
            | Event::ReadOnly(_)
            | Event::Saved { result: Err(_), .. }
            | Event::Imported { result: Err(_), .. }
            | Event::Rewritten { result: Err(_), .. }
            | Event::Moved { result: Err(_), .. }
    )
}

impl Lister {
    /// Look at every row where the index says it is, and send what is there. A row's
    /// folders are sent with it, though the walk has not listed them yet.
    ///
    /// A row in the root takes the stat the root's listing found, where it found a file
    /// there, so that file is not looked at twice.
    async fn rows(
        &mut self,
        fs: &impl Fs,
        rows: Vec<Row>,
        root: &[Entry],
        answer: &mut impl FnMut(Event),
    ) {
        let listed: BTreeMap<&str, Stat> = root
            .iter()
            .filter_map(|entry| match entry.kind {
                Kind::File(stat) => Some((entry.path.as_str(), stat)),
                _ => None,
            })
            .collect();
        let rows: Vec<Row> = rows
            .into_iter()
            .filter(|row| !row.path.components().any(|part| part.starts_with('.')))
            .filter(|row| self.told.insert(row.path.clone()))
            .collect();
        let mut rows = rows.into_iter();
        loop {
            let part: Vec<Row> = rows.by_ref().take(PART).collect();
            if part.is_empty() {
                break;
            }
            let paths: Vec<&str> = part
                .iter()
                .map(|row| row.path.as_str())
                .filter(|path| !listed.contains_key(path))
                .collect();
            let mut stats = fs.stats(&paths).await.into_iter();
            for row in part {
                let stat = match listed.get(row.path.as_str()) {
                    Some(stat) => Ok(Some(*stat)),
                    None => match stats.next() {
                        Some(stat) => stat,
                        None => break,
                    },
                };
                self.taken += 1;
                match stat {
                    Ok(Some(stat)) => self.row(fs, row, stat).await,
                    Ok(None) => {
                        self.told.remove(&row.path);
                        if let Some((len, _)) = row.print.and_then(|print| print.contents()) {
                            self.lens.insert(len);
                        }
                    }
                    Err(e) => self.part.unread.push((row.path, e.to_string())),
                }
                if self.taken >= PART {
                    self.send(answer);
                }
            }
        }
        self.send(answer);
    }

    /// Send the part gathered so far, where it holds anything.
    fn send(&mut self, answer: &mut impl FnMut(Event)) {
        if self.taken > 0 {
            let part = self.cut();
            answer(Event::Listed {
                part,
                ran: self.ran,
            });
        }
    }

    /// A row found where the index says, read where it holds a working copy.
    async fn row(&mut self, fs: &impl Fs, row: Row, stat: Stat) {
        let mut dir = row.path.parent();
        while !dir.is_root() && self.dirs.insert(dir.clone()) {
            self.part.dirs.push(dir.clone());
            dir = dir.parent();
        }
        let mut found = Found::unread(row.path, stat);
        if row.working {
            let unmoved = row.print.filter(|print| print.stat() == stat);
            match read(fs, &found.path, unmoved).await {
                Ok((bytes, file)) => (found.bytes, found.file) = (bytes, file),
                Err(e) => return self.part.unread.push((found.path, e.to_string())),
            }
            if unmoved.is_none() && row.print.is_some_and(|print| print.crc.is_some()) {
                found.crc = crc(&found).await;
            }
        }
        self.part.files.push(found);
    }

    /// Take every entry the walk lists, sending each part once it is full, and run the
    /// commands sent meanwhile between folders.
    ///
    /// ⚠️ A folder's entries are taken before any command runs after they were listed, the
    /// root's among them, which are listed before the open answers: a command may rename
    /// what they name.
    async fn walk(&mut self, fs: &mut impl Fs, walk: &mut Walk, answer: &mut impl FnMut(Event)) {
        loop {
            let next = walk.next_of(fs, &self.urgent, &self.told).await;
            let Some(entries) = next.filter(|_| !fs.stopped()) else {
                break;
            };
            for entry in entries {
                self.take(entry);
                if self.taken >= PART {
                    self.send(answer);
                }
            }
            self.walked(walk, answer);
            self.between(fs, walk, answer).await;
        }
    }

    /// Run each command waiting, in order, after sending the part gathered before it.
    async fn between(&mut self, fs: &mut impl Fs, walk: &mut Walk, answer: &mut impl FnMut(Event)) {
        while let Some(cmd) = fs.waiting() {
            self.send(answer);
            if let Cmd::Walk(dir) = cmd {
                self.ran += 1;
                self.urgent.push(dir);
                self.walked(walk, answer);
                continue;
            }
            let follow = Follow::of(&cmd);
            let mut done = true;
            step(fs, cmd, &mut self.ran, &mut |event| {
                done &= !failed(&event);
                answer(event)
            })
            .await;
            if done {
                self.follow(follow, walk);
            }
        }
    }

    /// Answer each folder asked for that has no folder left to list in it.
    fn walked(&mut self, walk: &Walk, answer: &mut impl FnMut(Event)) {
        let (waiting, done): (Vec<LibPath>, Vec<LibPath>) = std::mem::take(&mut self.urgent)
            .into_iter()
            .partition(|dir| walk.waits_in(dir));
        self.urgent = waiting;
        if done.is_empty() {
            return;
        }
        self.send(answer);
        for dir in done {
            answer(Event::Walked { dir, ran: self.ran });
        }
    }

    /// Follow what a command did to the tree, so nothing is listed twice and nothing
    /// under a renamed folder is lost.
    fn follow(&mut self, follow: Follow, walk: &mut Walk) {
        match follow {
            Follow::Moved { from, to } => {
                let folder = self.dirs.contains(&from);
                for paths in [&mut self.told, &mut self.dirs] {
                    *paths = std::mem::take(paths)
                        .into_iter()
                        .map(|path| path.moved(&from, &to).unwrap_or(path))
                        .collect();
                }
                let found = self.strangers.iter_mut().chain(&mut self.recognized);
                let paths = found.map(|found| &mut found.path).chain(&mut self.gone);
                for path in paths {
                    if let Some(moved) = path.moved(&from, &to) {
                        *path = moved;
                    }
                }
                for dir in &mut self.urgent {
                    if let Some(moved) = dir.moved(&from, &to) {
                        *dir = moved;
                    }
                }
                match folder {
                    true => walk.moved(from.as_str(), to.as_str()),
                    false => {
                        self.told.insert(to);
                    }
                }
            }
            Follow::Wrote(path) => {
                self.strangers.retain(|found| found.path != path);
                self.recognized.retain(|found| found.path != path);
                self.told.insert(path);
            }
            Follow::Made(dir) => {
                walk.made(dir.as_str());
                self.dirs.insert(dir);
            }
            Follow::Removed(dir) => {
                walk.removed(dir.as_str());
                self.dirs.remove(&dir);
            }
            Follow::Nothing => {}
        }
    }

    /// The part gathered so far, in path order.
    fn cut(&mut self) -> Listing {
        self.taken = 0;
        let mut part = std::mem::take(&mut self.part);
        part.sort();
        part
    }

    fn take(&mut self, entry: Entry) {
        self.taken += 1;
        let Some(path) = gathered(entry.path, &entry.kind, &mut self.temps) else {
            return;
        };
        match entry.kind {
            Kind::Dir => {
                if self.dirs.insert(path.clone()) {
                    self.part.dirs.push(path);
                }
            }
            Kind::Unwalked => self.part.unwalked.push(path),
            _ if self.told.contains(&path) => {}
            Kind::File(_) if !opens(path.leaf()) => self.part.others.push(path),
            Kind::File(stat) => {
                let found = Found::unread(path, stat);
                if self.lens.contains(&stat.len) {
                    self.strangers.push_back(found.clone());
                }
                self.part.files.push(found);
            }
            Kind::Unread(why) => self.part.unread.push((path, why)),
            Kind::Other => self.part.others.push(path),
        }
    }

    /// Send the rows of the index, then the walk, then take the CRC of each file that
    /// may be a row moved, and send the last part.
    async fn list(
        &mut self,
        fs: &mut impl Fs,
        rows: Vec<Row>,
        mut walk: Walk,
        answer: &mut impl FnMut(Event),
    ) {
        let root = walk.root.take().unwrap_or_default();
        self.rows(fs, rows, &root, answer).await;
        walk.root = Some(root);
        self.walk(fs, &mut walk, answer).await;
        self.recognize(fs, &mut walk, answer).await;
        let part = self.cut();
        answer(Event::Listed {
            part,
            ran: self.ran,
        });
    }

    /// Take the CRC of each file that may be a row moved, as it is now, and run the
    /// commands waiting after every few. One gone by then is noted as gone.
    async fn recognize(
        &mut self,
        fs: &mut impl Fs,
        walk: &mut Walk,
        answer: &mut impl FnMut(Event),
    ) {
        let (most, most_bytes) = RECOGNIZE;
        while !self.strangers.is_empty() && !fs.stopped() {
            let (mut files, mut bytes) = (0, 0);
            while files < most && bytes < most_bytes {
                let Some(mut found) = self.strangers.pop_front() else {
                    break;
                };
                files += 1;
                bytes += found.stat.len;
                match fs.stat(found.path.as_str()).await {
                    Ok(Some(stat)) => found.stat = stat,
                    Ok(None) => {
                        self.gone.push(found.path);
                        continue;
                    }
                    Err(_) => continue,
                }
                fingerprint(fs, &mut found).await;
                (found.bytes, found.file) = (None, None);
                if found.crc.is_some() {
                    self.recognized.push(found);
                }
            }
            self.between(fs, walk, answer).await;
        }
    }
}

/// The path of an entry a listing takes, or `None` for one it leaves out: anything under
/// a name starting with a dot, of which the temporary siblings interrupted saves left are
/// gathered into `temps`, and a name that cannot be a path in a library.
fn gathered(path: String, kind: &Kind, temps: &mut Vec<String>) -> Option<LibPath> {
    if path.split('/').any(|part| part.starts_with('.')) {
        let leaf = path.rsplit('/').next().unwrap_or(&path);
        let file = matches!(kind, Kind::File(_) | Kind::Unread(_) | Kind::Other);
        if file && leaf.starts_with('.') && leaf.ends_with(TEMP) {
            temps.push(path);
        }
        return None;
    }
    LibPath::parse(&path)
}

/// Read a file a listing found, and take its CRC, where it reads.
async fn fingerprint(fs: &impl Fs, found: &mut Found) {
    if let Ok((bytes, file)) = read(fs, &found.path, None).await {
        (found.bytes, found.file) = (bytes, file);
        found.crc = crc(found).await;
    }
}

/// The CRC of each file whose [`Stat`] is the one its fingerprint gives.
async fn fingerprints(
    fs: &impl Fs,
    files: Vec<(u64, LibPath, Fingerprint)>,
) -> Vec<(u64, LibPath, Fingerprint)> {
    let mut taken = Vec::new();
    for (id, path, print) in files {
        if !matches!(fs.stat(path.as_str()).await, Ok(Some(stat)) if stat == print.stat()) {
            continue;
        }
        let mut found = Found::unread(path, print.stat());
        fingerprint(fs, &mut found).await;
        if let Some(crc) = found.crc {
            let print = Fingerprint {
                crc: Some(crc),
                ..print
            };
            taken.push((id, found.path, print));
        }
    }
    taken
}

/// A rescan's listing.
///
/// A file `known` names is looked at wherever the walk went. Where drawbar holds its
/// contents and its [`Stat`] is not the known one, it is read again, with its CRC where
/// the known one has a CRC; any other file is listed unread. A file at a new path whose
/// length is that of a known file with a CRC is held back until the walk ends, and read
/// for its CRC where that length is one of a known file not found.
struct Rescan<'a> {
    known: &'a BTreeMap<LibPath, (Fingerprint, Holds)>,
    /// The lengths of the known files with a CRC.
    lens: BTreeSet<u64>,
    /// The known files looked at, and those of them found.
    looked: BTreeSet<LibPath>,
    found: BTreeSet<LibPath>,
    dirs: BTreeSet<LibPath>,
    listing: Listing,
    strangers: Vec<Found>,
    /// Never swept: only an open sweeps.
    temps: Vec<String>,
}

impl<'a> Rescan<'a> {
    fn new(known: &'a BTreeMap<LibPath, (Fingerprint, Holds)>) -> Rescan<'a> {
        Rescan {
            lens: known
                .values()
                .filter_map(|(print, _)| Some(print.contents()?.0))
                .collect(),
            known,
            looked: BTreeSet::new(),
            found: BTreeSet::new(),
            dirs: BTreeSet::new(),
            listing: Listing::default(),
            strangers: Vec::new(),
            temps: Vec::new(),
        }
    }

    async fn take(&mut self, fs: &impl Fs, entry: Entry) {
        let Some(path) = gathered(entry.path, &entry.kind, &mut self.temps) else {
            return;
        };
        match entry.kind {
            Kind::Dir => {
                if self.dirs.insert(path.clone()) {
                    self.listing.dirs.push(path);
                }
            }
            Kind::Unwalked => self.listing.unwalked.push(path),
            Kind::File(stat) => match self.known.get(&path) {
                Some(held) => self.held(fs, path, stat, *held).await,
                None => self.arrived(path, stat),
            },
            // A file drawbar holds is looked at again, whatever the walk made of it.
            Kind::Unread(_) | Kind::Other if self.known.contains_key(&path) => {
                self.restat(fs, path).await
            }
            Kind::Unread(why) => self.listing.unread.push((path, why)),
            Kind::Other => self.listing.others.push(path),
        }
    }

    /// Look at a known file the walk listed without a [`Stat`], or did not reach. Its
    /// folders are listed, though the walk may not have listed inside them.
    async fn restat(&mut self, fs: &impl Fs, path: LibPath) {
        let Some(held) = self.known.get(&path).copied() else {
            return;
        };
        match fs.stat(path.as_str()).await {
            Ok(Some(stat)) => {
                let mut dir = path.parent();
                while !dir.is_root() && self.dirs.insert(dir.clone()) {
                    self.listing.dirs.push(dir.clone());
                    dir = dir.parent();
                }
                self.held(fs, path, stat, held).await
            }
            Ok(None) => {
                self.looked.insert(path);
            }
            Err(e) => {
                self.looked.insert(path.clone());
                self.listing.unread.push((path, e.to_string()));
            }
        }
    }

    async fn held(&mut self, fs: &impl Fs, path: LibPath, stat: Stat, held: (Fingerprint, Holds)) {
        let (print, holds) = held;
        self.looked.insert(path.clone());
        let unmoved = print.stat() == stat;
        if unmoved || holds == Holds::Unread {
            self.found.insert(path.clone());
            return self.listing.files.push(Found::unread(path, stat));
        }
        let (bytes, file) = match read(fs, &path, None).await {
            Ok(read) => read,
            Err(e) => return self.listing.unread.push((path, e.to_string())),
        };
        let mut found = Found {
            bytes,
            file,
            ..Found::unread(path, stat)
        };
        if print.crc.is_some() {
            found.crc = crc(&found).await;
        }
        self.found.insert(found.path.clone());
        self.listing.files.push(found);
    }

    fn arrived(&mut self, path: LibPath, stat: Stat) {
        if !opens(path.leaf()) {
            return self.listing.others.push(path);
        }
        let found = Found::unread(path, stat);
        match self.lens.contains(&stat.len) {
            true => self.strangers.push(found),
            false => self.listing.files.push(found),
        }
    }

    /// Look for the known files the walk did not reach, and take the CRC of each file
    /// held back whose length is that of a known file still not found.
    async fn finish(mut self, fs: &impl Fs) -> Listing {
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
            .filter_map(|(_, (print, _))| Some(print.contents()?.0))
            .collect();
        for mut found in std::mem::take(&mut self.strangers) {
            if missing.contains(&found.stat.len) {
                fingerprint(fs, &mut found).await;
            }
            self.listing.files.push(found);
        }
        self.listing
    }
}

/// What [`Cmd::Check`] finds: each file `known` names that is still where it says, read
/// again where its [`Stat`] is not the known one, with its CRC where the known one has a
/// CRC. A file that is gone, or that cannot be looked at or read again, is left out.
async fn check(fs: &impl Fs, known: &BTreeMap<LibPath, Fingerprint>) -> Vec<Found> {
    let mut checked = Vec::new();
    for (path, print) in known {
        let Ok(Some(stat)) = fs.stat(path.as_str()).await else {
            continue;
        };
        let mut found = Found::unread(path.clone(), stat);
        if stat != print.stat() {
            let Ok((bytes, file)) = read(fs, path, None).await else {
                continue;
            };
            (found.bytes, found.file) = (bytes, file);
            if print.crc.is_some() {
                found.crc = crc(&found).await;
            }
        }
        checked.push(found);
    }
    checked
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

/// The files [`Cmd::Read`] asks for, read in order until `room` runs out.
async fn read_all(
    fs: &impl Fs,
    files: Vec<(u64, LibPath, Option<Fingerprint>)>,
    mut room: u64,
) -> Vec<(u64, Result<Found, Failure>)> {
    let mut answers = Vec::new();
    for (id, path, print) in files {
        let answer = read_one(fs, path, print, room).await;
        if let Ok(found) = &answer {
            let read = found.bytes.as_ref().map_or(0, |bytes| bytes.len() as u64);
            room = room.saturating_sub(read);
        }
        answers.push((id, answer));
    }
    answers
}

async fn read_one(
    fs: &impl Fs,
    path: LibPath,
    print: Option<Fingerprint>,
    room: u64,
) -> Result<Found, Failure> {
    let io = |e: io::Error| match e.kind() {
        io::ErrorKind::NotFound => Failure::Moved,
        _ => Failure::Io(e.to_string()),
    };
    let stat = fs.stat(path.as_str()).await.map_err(io)?;
    let stat = stat.ok_or(Failure::Moved)?;
    let unmoved = print.filter(|print| print.stat() == stat);
    let file = fs.rest(path.as_str(), unmoved).await.map_err(io)?;
    let bytes = match file {
        Some(_) => None,
        None if stat.len > room => return Err(Failure::Room(stat.len)),
        None => Some(fs.read(path.as_str()).await.map_err(io)?),
    };
    Ok(Found {
        bytes,
        file,
        ..Found::unread(path, stat)
    })
}

/// CRC-32 over the whole of what a listing read, or `None` where the file could not be
/// read through. A file left resting is read in one streaming pass.
async fn crc(found: &Found) -> Option<u32> {
    match (&found.bytes, &found.file) {
        (Some(bytes), _) => Some(nord_format::crc::crc32(bytes)),
        (None, Some(file)) => file.crc_now().await.ok(),
        (None, None) => None,
    }
}

/// Why a read [`Failure::Room`] refused did not run.
pub fn too_much() -> String {
    format!(
        "drawbar holds at most {} GiB of one library's files in memory, and the files open, \
         selected, in view, unsaved or waiting to be sent already come to that",
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
        let path = format!("{WORKING}/{name}");
        put(fs, &path, Staged::Bytes(&bytes), Over::Anything)
            .await
            .map_err(|e| e.to_string())?;
    }
    let text = sidecar::write(sidecar)?;
    put(fs, INDEX, Staged::Bytes(text.as_bytes()), Over::Anything)
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

/// Whether the file at `path` still holds what `expect` says, taking its CRC only when its
/// [`Stat`] moved to one of the same length, and `expect` has a CRC to compare.
async fn still(fs: &impl Fs, path: &LibPath, expect: &Fingerprint) -> io::Result<Option<bool>> {
    let Some(stat) = fs.stat(path.as_str()).await? else {
        return Ok(None);
    };
    if stat == expect.stat() {
        return Ok(Some(true));
    }
    let Some((len, crc)) = expect.contents().filter(|(len, _)| *len == stat.len) else {
        return Ok(Some(false));
    };
    Ok(Some((stat.len, fs.crc(path.as_str()).await?) == (len, crc)))
}

async fn save(
    fs: &mut impl Fs,
    path: &LibPath,
    bytes: &[u8],
    expect: Option<Fingerprint>,
) -> Result<Fingerprint, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    let over = match expect {
        None => Over::Nothing,
        Some(expect) => held(fs, path, &expect).await?,
    };
    put(fs, path.as_str(), Staged::Bytes(bytes), over)
        .await
        .map_err(refusal)?;
    let stat = fs
        .stat(path.as_str())
        .await
        .map_err(io)?
        .ok_or_else(|| Failure::Io("the file was gone as soon as it was written".into()))?;
    Ok(Fingerprint::of(stat, bytes))
}

/// Write the file `edit` makes of `from` over the file at `path`, which must still hold
/// what `expect` says, and find it there as a listing would.
async fn rewrite_over(
    fs: &mut impl Fs,
    path: &LibPath,
    from: &OnDisk,
    edit: &Rewrite,
    expect: Fingerprint,
) -> Result<Found, Failure> {
    let over = held(fs, path, &expect).await?;
    let wrote = put(fs, path.as_str(), Staged::Edited(from, edit), over).await;
    wrote.map_err(|e| match rewrite::is_changed(&e) {
        true => Failure::Moved,
        false => refusal(e),
    })?;
    landed(fs, path).await
}

/// The file just written at `path`, as a listing finds it: resting, where it is a piano
/// or sample instrument, and otherwise unread.
async fn landed(fs: &impl Fs, path: &LibPath) -> Result<Found, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    let stat = fs
        .stat(path.as_str())
        .await
        .map_err(io)?
        .ok_or_else(|| Failure::Io("the file was gone as soon as it was written".into()))?;
    let file = fs.rest(path.as_str(), None).await.map_err(io)?;
    Ok(Found {
        file,
        ..Found::unread(path.clone(), stat)
    })
}

/// Copy `from` into the library at `path`, and find it there as a listing would:
/// resting, where it is a piano or sample instrument, and otherwise unread.
async fn import(
    fs: &mut impl Fs,
    path: &LibPath,
    from: &Source,
    expect: Option<Fingerprint>,
) -> Result<Found, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    let changed = || Failure::Io("the file it copies changed on disk".to_string());
    if let Source::Library(source, held) = from {
        if still(fs, source, held).await.map_err(io)? != Some(true) {
            return Err(changed());
        }
    }
    let over = match expect {
        None => Over::Nothing,
        Some(expect) => held(fs, path, &expect).await?,
    };
    let what = match from {
        Source::Outside(file) => Staged::Outside(file),
        Source::Library(source, _) => Staged::Library(source.as_str()),
        Source::Edited(file, edit) => Staged::Edited(file, edit),
    };
    match put(fs, path.as_str(), what, over).await {
        Err(e) if rewrite::is_changed(&e) => return Err(changed()),
        copied => copied.map_err(refusal)?,
    }
    landed(fs, path).await
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
    use std::cell::Cell;
    use std::future::Future;
    use std::rc::Rc;

    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use crate::testing::{on_disk, sample_bytes, Temp};

    /// A library whose files claim whatever size they are given, without taking it. The
    /// ones in `rests` are left in place, as the desktop leaves a sample instrument, the
    /// folders in `unreadable` cannot be read, and the files in `vanished` are listed but
    /// gone when looked at. It counts the files it reads whole. The commands in `waiting`
    /// were sent while a listing is in flight, and `later` is sent once that many files
    /// have been read. A write staged for the path `meddles` names changes that file to the
    /// length it gives, as a program outside drawbar would while the write runs.
    struct Claimed {
        files: BTreeMap<String, u64>,
        meddles: Option<(String, u64)>,
        rests: BTreeMap<String, Arc<OnDisk>>,
        unreadable: BTreeSet<String>,
        vanished: BTreeSet<String>,
        reads: Rc<Cell<usize>>,
        /// How many times a file has been looked at, by a stat or a listing.
        looked: Cell<usize>,
        waiting: VecDeque<Cmd>,
        later: Option<(usize, Cmd)>,
    }

    impl Claimed {
        fn of(files: impl IntoIterator<Item = (String, u64)>) -> Claimed {
            Claimed {
                files: files.into_iter().collect(),
                meddles: None,
                rests: BTreeMap::new(),
                unreadable: BTreeSet::new(),
                vanished: BTreeSet::new(),
                reads: Rc::default(),
                looked: Cell::default(),
                waiting: VecDeque::new(),
                later: None,
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
        fn waiting(&mut self) -> Option<Cmd> {
            if self
                .later
                .as_ref()
                .is_some_and(|(after, _)| self.reads.get() >= *after)
            {
                return self.later.take().map(|(_, cmd)| cmd);
            }
            self.waiting.pop_front()
        }
        fn hold(&mut self, cmd: Cmd) {
            self.waiting.push_front(cmd);
        }
        async fn prepare(&mut self) -> io::Result<()> {
            Ok(())
        }
        async fn lock(&mut self) -> io::Result<bool> {
            Ok(true)
        }
        async fn children(
            &self,
            dir: &str,
            room: usize,
            known: &dyn Fn(&str) -> bool,
        ) -> io::Result<Children> {
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
                    Some((folder, _)) => (folder, Some(Kind::Dir)),
                    None if known(inside) => (inside, None),
                    None => {
                        self.looked.set(self.looked.get() + 1);
                        (inside, Some(Kind::File(stat(*len))))
                    }
                };
                found.insert(name.to_string(), kind);
            }
            let more = found.len() > room;
            Ok((found.into_iter().take(room).collect(), more))
        }
        async fn names(&self, _: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
        async fn crc(&self, path: &str) -> io::Result<u32> {
            self.read(path)
                .await
                .map(|bytes| nord_format::crc::crc32(&bytes))
        }

        async fn read(&self, path: &str) -> io::Result<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            Ok(path.as_bytes().to_vec())
        }
        async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
            self.looked.set(self.looked.get() + 1);
            let here = self
                .files
                .get(path)
                .filter(|_| !self.vanished.contains(path));
            Ok(here.map(|len| stat(*len)))
        }
        type Temp = (String, u64);

        async fn stage(&mut self, path: &str, what: Staged<'_>) -> io::Result<(String, u64)> {
            let Staged::Bytes(bytes) = what else {
                return Err(refused());
            };
            if let Some((at, len)) = self.meddles.take_if(|(at, _)| at == path) {
                self.files.insert(at, len);
            }
            Ok((path.to_string(), bytes.len() as u64))
        }

        async fn place(
            &mut self,
            (_, len): (String, u64),
            path: &str,
            over: bool,
        ) -> io::Result<()> {
            if !over && self.files.contains_key(path) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            self.files.insert(path.to_string(), len);
            Ok(())
        }

        async fn discard(&mut self, _: (String, u64)) {}

        async fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
            let moved: Vec<String> = self
                .files
                .keys()
                .filter(|path| within(path, from))
                .cloned()
                .collect();
            for path in moved {
                let len = self.files.remove(&path).unwrap_or(0);
                self.files
                    .insert(format!("{to}{}", &path[from.len()..]), len);
            }
            Ok(())
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

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    /// Every answer of an open's listing over these rows of the index, its end among
    /// them.
    fn listing(fs: &mut Claimed, rows: Vec<Row>) -> Vec<Event> {
        now(async {
            let walk = Walk::start(fs).await.unwrap();
            let mut lister = Lister::default();
            let mut events = Vec::new();
            let mut answer = |event| events.push(event);
            lister.list(fs, rows, walk, &mut answer).await;
            answer(Event::Complete(Complete {
                strangers: lister.recognized,
                gone: lister.gone,
                swept: 0,
                ran: lister.ran,
            }));
            events
        })
    }

    /// Every part of an open's listing, where nothing else is answered but its end.
    fn parts(fs: &mut Claimed, rows: Vec<Row>) -> Vec<Listing> {
        listing(fs, rows)
            .into_iter()
            .filter_map(|event| match event {
                Event::Listed { part, .. } => Some(part),
                Event::Complete(_) => None,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// Each folder and file a part of a listing names, by name.
    fn named(part: &Listing) -> Vec<String> {
        let files = part.files.iter().map(|found| &found.path);
        part.dirs
            .iter()
            .chain(files)
            .map(LibPath::to_string)
            .collect()
    }

    fn files(listing: &Listing) -> Vec<&str> {
        listing
            .files
            .iter()
            .map(|found| found.path.as_str())
            .collect()
    }

    /// An open lists every file by name, length and time, and reads none of them, the
    /// files the index names among them.
    #[test]
    fn an_open_lists_every_file_and_reads_none() {
        let mut fs = Claimed::of(
            ["Grand.ne5p", "a/Strings.ne5p", "a/b/Pad.ne5p", "notes.pdf"]
                .map(|path| (path.to_string(), path.len() as u64)),
        );
        let rows = vec![Row {
            path: path("a/Strings.ne5p"),
            print: Some(Fingerprint {
                crc: Some(7),
                ..Fingerprint::unread(stat(14))
            }),
            working: false,
        }];
        let mut listing = Listing::default();
        for part in parts(&mut fs, rows) {
            listing.extend(part);
        }
        listing.sort();
        assert_eq!(
            files(&listing),
            ["Grand.ne5p", "a/Strings.ne5p", "a/b/Pad.ne5p"]
        );
        assert!(listing.files.iter().all(|found| !found.read()));
        assert_eq!(listing.others, [path("notes.pdf")]);
        assert_eq!(fs.reads.get(), 0, "files read");
    }

    /// The files the index names come first, before the walk lists anything, with the
    /// folders they are in. A row the index names twice is listed once.
    #[test]
    fn the_files_the_index_names_come_before_the_walk() {
        let mut fs =
            Claimed::of(["Top.ne5p", "deep/er/Known.ne5p"].map(|path| (path.to_string(), 1)));
        let row = || Row {
            path: path("deep/er/Known.ne5p"),
            print: None,
            working: false,
        };
        let parts = parts(&mut fs, vec![row(), row()]);
        assert_eq!(files(&parts[0]), ["deep/er/Known.ne5p"]);
        assert_eq!(parts[0].dirs, [path("deep"), path("deep/er")]);
        let rest: Vec<&str> = parts[1..].iter().flat_map(files).collect();
        assert_eq!(rest, ["Top.ne5p"], "the walk leaves the known file out");
    }

    #[test]
    fn an_open_looks_at_each_file_once_those_the_index_names_included() {
        let paths = ["Top.ne5p", "a/One.ne5p", "a/Two.ne5p", "a/b/Three.ne5p"];
        let mut fs = Claimed::of(paths.map(|at| (at.to_string(), 1)));
        let rows = ["Top.ne5p", "a/One.ne5p", "a/b/Three.ne5p"]
            .into_iter()
            .map(|at| Row {
                path: path(at),
                print: None,
                working: false,
            })
            .collect();
        let mut listing = Listing::default();
        for part in parts(&mut fs, rows) {
            listing.extend(part);
        }
        listing.sort();
        assert_eq!(files(&listing), paths);
        assert_eq!(fs.looked.get(), paths.len(), "files looked at");
    }

    /// The file under a working copy is read, since the copy is compared with it.
    #[test]
    fn the_file_under_a_working_copy_is_read_at_open() {
        let mut fs = Claimed::of([("Grand.ne5p".to_string(), 10)]);
        let rows = vec![Row {
            path: path("Grand.ne5p"),
            print: None,
            working: true,
        }];
        let parts = parts(&mut fs, rows);
        assert_eq!(parts[0].files[0].bytes.as_deref(), Some(&b"Grand.ne5p"[..]));
        assert_eq!(fs.reads.get(), 1);
    }

    /// A file changed outside drawbar while a save's new contents are written is not
    /// written over: the save is refused as moved, and what was written there stays.
    #[test]
    fn a_save_over_a_file_changed_while_it_was_staged_is_refused() {
        let mut fs = Claimed {
            meddles: Some(("Grand.ne5p".to_string(), 99)),
            ..Claimed::of([("Grand.ne5p".to_string(), 10)])
        };
        let expect = Fingerprint::unread(stat(10));
        let saved = now(save(&mut fs, &path("Grand.ne5p"), b"new", Some(expect)));
        assert_eq!(saved, Err(Failure::Moved));
        assert_eq!(fs.files["Grand.ne5p"], 99, "what was written there stays");
    }

    /// A read asked for leaves a sample instrument in its file, which takes none of the
    /// room, and refuses a file that does not fit what room is left.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_read_leaves_an_instrument_in_place_and_stops_at_the_room_left() {
        let dir = Temp::new();
        let file = on_disk(&dir, "Marimba.nsmp", &sample_bytes());
        let fs = Claimed {
            rests: BTreeMap::from([("Marimba.nsmp".to_string(), file)]),
            ..Claimed::of([
                ("Marimba.nsmp".to_string(), MOST_BYTES),
                ("Small.ne5p".to_string(), 10),
                ("Large.ne5p".to_string(), 20),
            ])
        };
        let asked = ["Marimba.nsmp", "Small.ne5p", "Large.ne5p"]
            .into_iter()
            .enumerate()
            .map(|(id, at)| (id as u64, path(at), None))
            .collect();
        let read = now(read_all(&fs, asked, 25));
        let what: Vec<(u64, &str)> = read
            .iter()
            .map(|(id, answer)| {
                let held = match answer {
                    Ok(found) if found.file.is_some() => "resting",
                    Ok(found) if found.bytes.is_some() => "whole",
                    Ok(_) => "nothing",
                    Err(_) => "refused",
                };
                (*id, held)
            })
            .collect();
        assert_eq!(what, [(0, "resting"), (1, "whole"), (2, "refused")]);
    }

    /// A look for CRCs reads only the files still as their fingerprints say, and takes
    /// each one's CRC.
    #[test]
    fn a_look_for_crcs_reads_only_the_files_that_did_not_move() {
        let fs = Claimed::of(["Same.ne5p", "Moved.ne5p"].map(|path| (path.to_string(), 9)));
        let print = |len| Fingerprint::unread(stat(len));
        let asked = vec![
            (1, path("Same.ne5p"), print(9)),
            (2, path("Moved.ne5p"), print(8)),
            (3, path("Gone.ne5p"), print(9)),
        ];
        let taken = now(fingerprints(&fs, asked));
        let crc = nord_format::crc::crc32(b"Same.ne5p");
        assert_eq!(
            taken,
            [(
                1,
                path("Same.ne5p"),
                Fingerprint {
                    crc: Some(crc),
                    ..print(9)
                }
            )]
        );
        assert_eq!(fs.reads.get(), 1, "files read");
    }

    /// A rescan reads a file drawbar holds again only where its length or time moved,
    /// and never one it has not read.
    #[test]
    fn a_rescan_reads_again_only_what_drawbar_holds_and_what_moved() {
        let mut fs = Claimed::of(
            ["Held.ne5p", "Moved.ne5p", "Unread.ne5p", "New.ne5p"]
                .map(|path| (path.to_string(), 10)),
        );
        let print = |len| Fingerprint::unread(stat(len));
        let known = BTreeMap::from([
            (path("Held.ne5p"), (print(10), Holds::Whole)),
            (path("Moved.ne5p"), (print(9), Holds::Whole)),
            (path("Unread.ne5p"), (print(9), Holds::Unread)),
        ]);
        let listing = now(scan(&mut fs, &known, &mut 0, &mut |event| {
            panic!("{event:?}")
        }))
        .unwrap();
        let read: Vec<(&str, bool)> = listing
            .files
            .iter()
            .map(|found| (found.path.as_str(), found.read()))
            .collect();
        assert_eq!(
            read,
            [
                ("Held.ne5p", false),
                ("Moved.ne5p", true),
                ("New.ne5p", false),
                ("Unread.ne5p", false)
            ]
        );
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
        let mut fs = Claimed::of(files);
        let parts = parts(&mut fs, Vec::new());
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

    /// A file at a new path is listed at once, unread, whatever its length. Where its
    /// length is that of a file the index names that is not where the index says, its
    /// CRC is taken once the walk is done, and the end of the listing names it with that
    /// CRC.
    #[test]
    fn a_file_that_may_be_one_moved_is_listed_at_once_and_fingerprinted_at_the_end() {
        let mut fs = Claimed::of(["Grand.ne5p", "Moved.ne5p", "Other.ne5p"].map(|path| {
            let len = match path {
                "Other.ne5p" => 3,
                _ => path.len() as u64,
            };
            (path.to_string(), len)
        }));
        let row = |at: &str| Row {
            path: path(at),
            print: Some(Fingerprint {
                crc: Some(7),
                ..Fingerprint::unread(stat(10))
            }),
            working: false,
        };
        let events = listing(&mut fs, vec![row("Gone.ne5p"), row("Grand.ne5p")]);
        let Some(Event::Complete(complete)) = events.last() else {
            panic!("{events:?}");
        };
        let recognized: Vec<(&str, Option<u32>, bool)> = complete
            .strangers
            .iter()
            .map(|found| (found.path.as_str(), found.crc, found.read()))
            .collect();
        let crc = nord_format::crc::crc32(b"Moved.ne5p");
        assert_eq!(
            recognized,
            [("Moved.ne5p", Some(crc), false)],
            "(path, CRC, read)"
        );
        let parts = parts(&mut fs, vec![row("Gone.ne5p"), row("Grand.ne5p")]);
        assert_eq!(files(&parts[0]), ["Grand.ne5p"]);
        let walked: Vec<&str> = parts[1..].iter().flat_map(files).collect();
        assert_eq!(walked, ["Moved.ne5p", "Other.ne5p"]);
        assert!(parts
            .iter()
            .flat_map(|part| &part.files)
            .all(|found| found.crc.is_none() && !found.read()));
    }

    /// The CRCs taken after the walk are taken a few at a time, and a command sent
    /// meanwhile runs between them. A file gone by the time its CRC is to be taken is
    /// named as gone.
    #[test]
    fn the_crcs_after_the_walk_let_commands_run_between_them() {
        const MANY: usize = 3 * RECOGNIZE.0;
        let files = (0..MANY).map(|n| (format!("{n:03}.ne5p"), 10));
        let mut fs = Claimed::of(files);
        fs.later = Some((1, Cmd::Walk(path("anywhere"))));
        fs.vanished.insert("001.ne5p".to_string());
        let rows = vec![Row {
            path: path("Gone.ne5p"),
            print: Some(Fingerprint {
                crc: Some(7),
                ..Fingerprint::unread(stat(10))
            }),
            working: false,
        }];
        let reads = fs.reads.clone();
        let events = now(async {
            let walk = Walk::start(&fs).await.unwrap();
            let mut lister = Lister::default();
            let mut answered = Vec::new();
            let mut answer = |event| answered.push((event, reads.get()));
            lister.list(&mut fs, rows, walk, &mut answer).await;
            assert_eq!(lister.recognized.len(), MANY - 1);
            assert_eq!(lister.gone, [path("001.ne5p")]);
            answered
        });
        let walked = events
            .iter()
            .find(|(event, _)| matches!(event, Event::Walked { .. }))
            .map(|(_, reads)| *reads);
        assert!(
            walked.is_some_and(|reads| reads < MANY - 1),
            "{walked:?} files read first"
        );
    }

    /// A folder renamed while the walk is in flight is listed under its new name. What
    /// was listed before the rename comes in parts that say so, and nothing after it
    /// names the old one.
    #[test]
    fn a_folder_renamed_while_the_walk_is_in_flight_is_listed_where_it_went() {
        let files = ["a/x.ne5p", "a/sub/y.ne5p", "top.ne5p"];
        let mut fs = Claimed::of(files.map(|path| (path.to_string(), 1)));
        fs.waiting.push_back(Cmd::Move {
            from: path("a"),
            to: path("b"),
        });
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for event in listing(&mut fs, Vec::new()) {
            let (part, ran) = match event {
                Event::Listed { part, ran } => (part, ran),
                Event::Moved { result: Ok(()), .. } | Event::Complete(_) => continue,
                other => panic!("{other:?}"),
            };
            match ran {
                0 => before.extend(named(&part)),
                _ => after.extend(named(&part)),
            }
        }
        before.sort();
        after.sort();
        assert_eq!(before, ["a", "top.ne5p"]);
        assert_eq!(after, ["b/sub", "b/sub/y.ne5p", "b/x.ne5p"]);
    }

    /// A file renamed while the walk is in flight, into a folder still to be listed, is
    /// listed once, where it was: the walk does not bring it back from where it went.
    #[test]
    fn a_file_renamed_while_the_walk_is_in_flight_is_not_listed_again() {
        let files = ["a/x.ne5p", "top.ne5p"];
        let mut fs = Claimed::of(files.map(|path| (path.to_string(), 1)));
        fs.waiting.push_back(Cmd::Move {
            from: path("top.ne5p"),
            to: path("a/top.ne5p"),
        });
        let mut listed = Vec::new();
        for event in listing(&mut fs, Vec::new()) {
            match event {
                Event::Listed { part, ran } => {
                    listed.extend(named(&part).into_iter().map(|name| (name, ran)))
                }
                Event::Moved { result: Ok(()), .. } | Event::Complete(_) => {}
                other => panic!("{other:?}"),
            }
        }
        listed.sort();
        let listed: Vec<(&str, u64)> = listed
            .iter()
            .map(|(name, ran)| (name.as_str(), *ran))
            .collect();
        assert_eq!(listed, [("a", 0), ("a/x.ne5p", 1), ("top.ne5p", 0)]);
    }

    /// A file a command wrote while the walk is in flight is not listed again where the
    /// walk then comes to it.
    #[test]
    fn a_file_written_while_the_walk_is_in_flight_is_not_listed_again() {
        let mut fs = Claimed::of([("a/x.ne5p".to_string(), 1)]);
        fs.waiting.push_back(Cmd::Save {
            id: 1,
            path: path("a/new.ne5p"),
            bytes: vec![0; 3],
            expect: None,
        });
        let mut listed = Vec::new();
        let mut saved = false;
        for event in listing(&mut fs, Vec::new()) {
            match event {
                Event::Listed { part, .. } => listed.extend(named(&part)),
                Event::Saved { result, .. } => saved = result.is_ok(),
                Event::Complete(_) => {}
                other => panic!("{other:?}"),
            }
        }
        assert!(saved);
        listed.sort();
        assert_eq!(listed, ["a", "a/x.ne5p"]);
    }

    /// A folder asked for is listed whole ahead of the rest, and answered once it is.
    #[test]
    fn a_folder_asked_for_is_listed_whole_ahead_of_the_rest() {
        let files = ["a/1.ne5p", "b/2.ne5p", "c/deep/3.ne5p", "c/4.ne5p"];
        let mut fs = Claimed::of(files.map(|path| (path.to_string(), 1)));
        fs.waiting.push_back(Cmd::Walk(path("c")));
        let events = listing(&mut fs, Vec::new());
        let walked = events
            .iter()
            .position(|event| matches!(event, Event::Walked { dir, .. } if dir.as_str() == "c"))
            .expect("the folder is answered");
        assert!(
            listed(&events, "c/deep/3.ne5p") < Some(walked),
            "{events:?}"
        );
        assert!(listed(&events, "a/1.ne5p") > Some(walked), "{events:?}");
    }

    /// A folder asked for below the folders the walk has reached is answered only once
    /// the walk has come down to it and listed it whole, and the way down comes first.
    #[test]
    fn a_folder_asked_for_deep_below_the_walk_is_answered_once_it_is_listed() {
        let files = ["a/b/c/deep.ne5p", "a/top.ne5p", "z/other.ne5p"];
        let mut fs = Claimed::of(files.map(|path| (path.to_string(), 1)));
        fs.waiting.push_back(Cmd::Walk(path("a/b/c")));
        let events = listing(&mut fs, Vec::new());
        let walked = events
            .iter()
            .position(|event| matches!(event, Event::Walked { dir, .. } if dir.as_str() == "a/b/c"))
            .expect("the folder is answered");
        let deep = listed(&events, "a/b/c/deep.ne5p");
        assert!(deep < Some(walked), "{events:?}");
        assert!(listed(&events, "z/other.ne5p") > Some(walked), "{events:?}");
    }

    /// Where among the answers the part listing `name` is.
    fn listed(events: &[Event], name: &str) -> Option<usize> {
        events.iter().position(|event| match event {
            Event::Listed { part, .. } => named(part).iter().any(|named| named == name),
            _ => false,
        })
    }

    /// A read a rescan answers while the walk is in flight counts among the commands
    /// run, so the parts after it say how many have.
    #[test]
    fn a_read_a_rescan_answers_while_the_walk_is_in_flight_counts_as_run() {
        let mut fs = Claimed::of([("a/x.ne5p".to_string(), 1)]);
        fs.waiting.push_back(Cmd::Scan {
            known: BTreeMap::new(),
        });
        fs.waiting.push_back(Cmd::Read {
            files: Vec::new(),
            room: 0,
        });
        let events = listing(&mut fs, Vec::new());
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::Scanned(Ok(_)))));
        assert!(events.iter().any(|event| matches!(event, Event::Read(_))));
        let Some(Event::Complete(complete)) = events.last() else {
            panic!("{events:?}");
        };
        assert_eq!(complete.ran, 2, "commands run");
    }

    /// A check sent while the walk is in flight is answered between two folders, and a
    /// file whose length moved is read for it.
    #[test]
    fn a_check_sent_while_the_walk_is_in_flight_is_answered_between_folders() {
        let files = [("a/x.ne5p", 1), ("Held.ne5p", 10)];
        let mut fs = Claimed::of(files.map(|(path, len)| (path.to_string(), len)));
        let known = BTreeMap::from([(path("Held.ne5p"), Fingerprint::unread(stat(9)))]);
        fs.waiting.push_back(Cmd::Check { known });
        let events = listing(&mut fs, Vec::new());
        let checked = events
            .iter()
            .position(|event| matches!(event, Event::Checked(_)))
            .expect("the check is answered");
        let Event::Checked(Ok(found)) = &events[checked] else {
            panic!("{:?}", events[checked]);
        };
        let read: Vec<(&str, bool)> = found
            .iter()
            .map(|found| (found.path.as_str(), found.read()))
            .collect();
        assert_eq!(read, [("Held.ne5p", true)], "(path, read)");
        assert!(Some(checked) < listed(&events, "a/x.ne5p"), "{events:?}");
    }
}
