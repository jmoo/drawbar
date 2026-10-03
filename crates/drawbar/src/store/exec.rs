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
use super::{
    Cmd, Complete, Event, Failure, Fingerprint, Found, Holds, LibPath, Listing, Opened, Stat,
};
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
    fn waiting(&mut self) -> Option<Cmd> {
        None
    }
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
    match cmd {
        Cmd::Open => {
            if let Err(why) = open(fs, answer).await {
                answer(Event::Opened(Err(why)));
            }
        }
        cmd => step(fs, cmd, answer).await,
    }
}

/// Whether a command writes, and so first takes the library.
fn writes(cmd: &Cmd) -> bool {
    !matches!(
        cmd,
        Cmd::Open | Cmd::Scan { .. } | Cmd::Check { .. } | Cmd::Walk(_) | Cmd::Read { .. }
    )
}

/// Run any command but an open, which runs once, first.
async fn step(fs: &mut impl Fs, cmd: Cmd, answer: &mut impl FnMut(Event)) {
    if writes(&cmd) {
        if let Err(why) = take(fs).await {
            return answer(Event::ReadOnly(why));
        }
    }
    let answered = match cmd {
        Cmd::Open => Some(Event::Opened(
            Err("the library is open already".to_string()),
        )),
        Cmd::Scan { known } => Some(Event::Scanned(
            scan(fs, &known, answer).await.map_err(|e| e.to_string()),
        )),
        Cmd::Check { known } => Some(Event::Checked(Ok(check(fs, &known).await))),
        // Outside an open's listing the whole tree has been listed already.
        Cmd::Walk(dir) => Some(Event::Walked { dir, ran: 0 }),
        Cmd::Read { files, room } => Some(Event::Read(read_all(fs, files, room).await)),
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

/// Run the reads waiting, each as it comes, until a command that is not one is next.
/// That one is left to run after the rescan, and so is every command behind it.
async fn reads_between(fs: &mut impl Fs, answer: &mut impl FnMut(Event)) {
    while let Some(cmd) = fs.waiting() {
        match cmd {
            Cmd::Read { files, room } => answer(Event::Read(read_all(fs, files, room).await)),
            cmd => return fs.hold(cmd),
        }
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
    let walk = Walk::start(fs).await.map_err(|e| e.to_string())?;
    answer(Event::Opened(Ok(Opened {
        writable,
        indexed,
        sidecar,
        working,
        swept,
    })));
    let mut lister = Lister::default();
    lister.rows(fs, rows, answer).await;
    lister.walk(fs, walk, answer).await;
    let (last, strangers) = lister.finish(fs).await;
    let ran = lister.ran;
    answer(Event::Listed { part: last, ran });
    let mut swept = 0;
    if sweeps {
        for temp in std::mem::take(&mut lister.temps) {
            if fs.remove_file(&temp).await.is_ok() {
                swept += 1;
            }
        }
    }
    answer(Event::Complete(Complete {
        strangers,
        swept,
        ran,
    }));
    Ok(())
}

/// The whole tree again, in one listing. The reads sent while it runs are answered
/// between its folders.
async fn scan(
    fs: &mut impl Fs,
    known: &BTreeMap<LibPath, (Fingerprint, Holds)>,
    answer: &mut impl FnMut(Event),
) -> io::Result<Listing> {
    let mut walk = Walk::start(fs).await?;
    let mut rescan = Rescan::new(known);
    while let Some(entries) = walk.next(fs).await.filter(|_| !fs.stopped()) {
        for entry in entries {
            rescan.take(fs, entry).await;
        }
        reads_between(fs, answer).await;
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
        walk.root = Some(walk.folder(fs, String::new()).await?);
        Ok(walk)
    }

    /// The entries of the next folder, or `None` once every folder has been listed. A
    /// folder inside that cannot be read is left unlisted, not the library.
    async fn next(&mut self, fs: &impl Fs) -> Option<Vec<Entry>> {
        self.next_of(fs, &[]).await
    }

    /// [`Walk::next`], taking first a folder in one of `urgent`, where any is waiting.
    async fn next_of(&mut self, fs: &impl Fs, urgent: &[LibPath]) -> Option<Vec<Entry>> {
        if let Some(root) = self.root.take() {
            return Some(root);
        }
        let first = self
            .folders
            .iter()
            .position(|folder| urgent.iter().any(|dir| within(folder, dir.as_str())));
        let prefix = self.folders.remove(first.unwrap_or(0))?;
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
            if matches!(kind, Kind::Dir) && !name.starts_with('.') && self.seen.insert(path.clone())
            {
                self.folders.push_back(path.clone());
            }
            entries.push(Entry { path, kind });
        }
        Ok(entries)
    }
}

impl Walk {
    /// Whether a folder in `dir`, or `dir` itself, is still to be listed.
    fn waits_in(&self, dir: &LibPath) -> bool {
        let dir = dir.as_str();
        self.root.is_some() || self.folders.iter().any(|folder| within(folder, dir))
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

/// How many entries one part of a listing covers before it is sent.
const PART: usize = 256;

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
/// file moved, so it is held back until the walk ends and its CRC taken.
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
    /// Files at new paths that may be rows moved.
    strangers: Vec<Found>,
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
            Cmd::Save { path, .. } => Follow::Wrote(path.clone()),
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
        Event::Failed(_) | Event::ReadOnly(_) | Event::Saved { result: Err(_), .. }
    )
}

impl Lister {
    /// Look at every row where the index says it is, and send what is there. A row's
    /// folders are sent with it, though the walk has not listed them yet.
    async fn rows(&mut self, fs: &impl Fs, rows: Vec<Row>, answer: &mut impl FnMut(Event)) {
        for row in rows {
            let hidden = row.path.components().any(|part| part.starts_with('.'));
            if hidden || !self.told.insert(row.path.clone()) {
                continue;
            }
            self.taken += 1;
            match fs.stat(row.path.as_str()).await {
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
                found.crc = crc(&found);
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
    async fn walk(&mut self, fs: &mut impl Fs, mut walk: Walk, answer: &mut impl FnMut(Event)) {
        loop {
            let next = walk.next_of(fs, &self.urgent).await;
            let Some(entries) = next.filter(|_| !fs.stopped()) else {
                break;
            };
            for entry in entries {
                self.take(entry);
                if self.taken >= PART {
                    self.send(answer);
                }
            }
            self.walked(&walk, answer);
            self.between(fs, &mut walk, answer).await;
        }
    }

    /// Run each command waiting, in order, after sending the part gathered before it.
    async fn between(&mut self, fs: &mut impl Fs, walk: &mut Walk, answer: &mut impl FnMut(Event)) {
        while let Some(cmd) = fs.waiting() {
            self.send(answer);
            self.ran += 1;
            if let Cmd::Walk(dir) = cmd {
                self.urgent.push(dir);
                self.walked(walk, answer);
                continue;
            }
            let follow = Follow::of(&cmd);
            let mut done = true;
            step(fs, cmd, &mut |event| {
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
                for found in &mut self.strangers {
                    if let Some(moved) = found.path.moved(&from, &to) {
                        found.path = moved;
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
                match self.lens.contains(&stat.len) {
                    true => self.strangers.push(found),
                    false => self.part.files.push(found),
                }
            }
            Kind::Unread(why) => self.part.unread.push((path, why)),
            Kind::Other => self.part.others.push(path),
        }
    }

    /// Read each file held back and take its CRC. Returns the last part, and the files
    /// held back.
    async fn finish(&mut self, fs: &impl Fs) -> (Listing, Vec<Found>) {
        let mut strangers = std::mem::take(&mut self.strangers);
        for found in strangers.iter_mut().filter(|_| !fs.stopped()) {
            fingerprint(fs, found).await;
        }
        (self.cut(), strangers)
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
        found.crc = crc(found);
    }
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
            found.crc = crc(&found);
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
                found.crc = crc(&found);
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
        None if stat.len > room => return Err(Failure::Io(too_much())),
        None => Some(fs.read(path.as_str()).await.map_err(io)?),
    };
    Ok(Found {
        bytes,
        file,
        ..Found::unread(path, stat)
    })
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
        "drawbar holds at most {} GiB of one library's files in memory, and the files read \
         already come to that",
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
    use std::cell::Cell;
    use std::future::Future;

    use super::*;
    use crate::testing::{on_disk, sample_bytes, Temp};

    /// A library whose files claim whatever size they are given, without taking it. The
    /// ones in `rests` are left in place, as the desktop leaves a sample instrument, and
    /// the folders in `unreadable` cannot be read. It counts the files it reads whole. The
    /// commands in `waiting` were sent while a listing is in flight.
    struct Claimed {
        files: BTreeMap<String, u64>,
        rests: BTreeMap<String, Arc<OnDisk>>,
        unreadable: BTreeSet<String>,
        reads: Cell<usize>,
        waiting: VecDeque<Cmd>,
    }

    impl Claimed {
        fn of(files: impl IntoIterator<Item = (String, u64)>) -> Claimed {
            Claimed {
                files: files.into_iter().collect(),
                rests: BTreeMap::new(),
                unreadable: BTreeSet::new(),
                reads: Cell::new(0),
                waiting: VecDeque::new(),
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
            self.reads.set(self.reads.get() + 1);
            Ok(path.as_bytes().to_vec())
        }
        async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
            Ok(self.files.get(path).map(|len| stat(*len)))
        }
        async fn create(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
            self.files.insert(path.to_string(), bytes.len() as u64);
            Ok(())
        }
        async fn replace(&mut self, _: &str, _: &[u8]) -> io::Result<()> {
            Err(refused())
        }
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

    /// Every answer of an open's listing over these rows of the index, with the files
    /// held back last, as a part of their own.
    fn listing(fs: &mut Claimed, rows: Vec<Row>) -> Vec<Event> {
        now(async {
            let walk = Walk::start(fs).await.unwrap();
            let mut lister = Lister::default();
            let mut events = Vec::new();
            let mut answer = |event| events.push(event);
            lister.rows(fs, rows, &mut answer).await;
            lister.walk(fs, walk, &mut answer).await;
            let (last, strangers) = lister.finish(fs).await;
            let ran = lister.ran;
            answer(Event::Listed { part: last, ran });
            let strangers = Listing {
                files: strangers,
                ..Listing::default()
            };
            answer(Event::Listed {
                part: strangers,
                ran,
            });
            events
        })
    }

    /// Every part of an open's listing, where nothing else is answered.
    fn parts(fs: &mut Claimed, rows: Vec<Row>) -> Vec<Listing> {
        listing(fs, rows)
            .into_iter()
            .map(|event| match event {
                Event::Listed { part, .. } => part,
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

    /// A read asked for leaves a sample instrument in its file, which takes none of the
    /// room, and refuses a file that does not fit what room is left.
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
        let listing = now(scan(&mut fs, &known, &mut |event| panic!("{event:?}"))).unwrap();
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

    /// A file at a new path waits for the end of the listing only where its length is that
    /// of a file the index names that is not where the index says, and is read there, its
    /// CRC taken.
    #[test]
    fn a_file_that_may_be_one_moved_waits_for_the_end_and_is_fingerprinted() {
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
        let parts = parts(&mut fs, vec![row("Gone.ne5p"), row("Grand.ne5p")]);
        let strangers = &parts[parts.len() - 1].files;
        let held: Vec<(&str, bool)> = strangers
            .iter()
            .map(|found| (found.path.as_str(), found.crc.is_some()))
            .collect();
        assert_eq!(held, [("Moved.ne5p", true)], "(path, fingerprinted)");
        assert_eq!(files(&parts[0]), ["Grand.ne5p"]);
        let walked: Vec<&str> = parts[1..parts.len() - 1].iter().flat_map(files).collect();
        assert_eq!(walked, ["Other.ne5p"]);
        assert!(parts[..parts.len() - 1]
            .iter()
            .flat_map(|part| &part.files)
            .all(|found| found.crc.is_none() && !found.read()));
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
            let Event::Listed { part, ran } = event else {
                panic!("{event:?}");
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

    /// Where among the answers the part listing `name` is.
    fn listed(events: &[Event], name: &str) -> Option<usize> {
        events.iter().position(|event| match event {
            Event::Listed { part, .. } => named(part).iter().any(|named| named == name),
            _ => false,
        })
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
