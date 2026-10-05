//! What drawbar read of a library's files, kept in its own app data between sessions.
//!
//! Each file read and decoded leaves a [`Summary`], kept under its path with the length
//! and time the file had then. At the next open a file listed with that length and time
//! draws as its summary says without being read; one whose length or time moved is
//! ignored until it is read again.
//!
//! It is never kept in the library. On the desktop it is one file beside eframe's
//! `app.ron`, each library's entries under its folder's path. In the browser it is one
//! IndexedDB record per file in drawbar's database, under the library's id and the
//! file's path. Everything in it can be read again from the library, so one that is
//! missing, does not read, or was written by another version is taken as empty.

use std::collections::{BTreeMap, BTreeSet};

use nord_format::formats::nsmp::codec::Layout;
use serde::{Deserialize, Serialize};

use super::{LibPath, Stat};
use crate::browser::Kind;
use crate::log::Log;
use crate::summary::{Plays, Summary, Verdict};

/// The version of what this build writes. Anything else is discarded unread.
pub const VERSION: u32 = 5;

/// What one file held when it was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Packed", from = "Packed")]
pub struct Entry {
    pub len: u64,
    /// Nanoseconds since the Unix epoch. See [`Stat::modified`].
    pub modified: Option<u64>,
    pub summary: Summary,
}

/// An [`Entry`] as it is written: its fields in order, unnamed, since there are tens of
/// thousands of them.
#[derive(Serialize, Deserialize)]
struct Packed(
    u64,
    Option<u64>,
    Kind,
    String,
    Option<u32>,
    Option<Plays>,
    Verdict,
    Option<String>,
    Vec<String>,
);

impl From<Entry> for Packed {
    fn from(entry: Entry) -> Packed {
        let Summary {
            kind,
            tag,
            crc32,
            plays,
            verdict,
            generation,
            wavs,
        } = entry.summary;
        Packed(
            entry.len,
            entry.modified,
            kind,
            tag,
            crc32,
            plays,
            verdict,
            generation.map(str::to_string),
            wavs,
        )
    }
}

impl From<Packed> for Entry {
    fn from(packed: Packed) -> Entry {
        let Packed(len, modified, kind, tag, crc32, plays, verdict, generation, wavs) = packed;
        Entry {
            len,
            modified,
            summary: Summary {
                kind,
                tag,
                crc32,
                plays,
                verdict,
                generation: generation.and_then(|said| {
                    Layout::ALL
                        .into_iter()
                        .map(Layout::generation)
                        .find(|known| *known == said)
                }),
                wavs,
            },
        }
    }
}

/// How the cache is written: an optional value as the bare value.
fn ron() -> ron::Options {
    ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
}

impl Entry {
    /// What a file with length and time `stat` holds, as `summary` says.
    pub fn of(stat: Stat, summary: Summary) -> Entry {
        Entry {
            len: stat.len,
            modified: stat.modified,
            summary,
        }
    }

    pub fn stat(&self) -> Stat {
        Stat {
            len: self.len,
            modified: self.modified,
        }
    }
}

type Entries = BTreeMap<LibPath, Entry>;

/// A change made while the entries kept were still loading, to make to them once they
/// arrive.
enum Change {
    Moved(LibPath, LibPath),
    Forgot(LibPath),
    Kept(BTreeSet<LibPath>),
}

/// One library's entries, and where they are kept.
pub struct Cache {
    entries: Entries,
    /// Paths put or dropped since they were last kept.
    changed: BTreeSet<LibPath>,
    /// `None` for entries kept only in memory.
    shelf: Option<Shelf>,
    /// The changes to make to the entries kept, while they are loading.
    replay: Option<Vec<Change>>,
}

impl Cache {
    /// Entries held for this session only.
    pub fn memory() -> Cache {
        Cache {
            entries: BTreeMap::new(),
            changed: BTreeSet::new(),
            shelf: None,
            replay: None,
        }
    }

    fn kept(shelf: Shelf) -> Cache {
        Cache {
            shelf: Some(shelf),
            replay: Some(Vec::new()),
            ..Cache::memory()
        }
    }

    /// Whether the entries kept have arrived, or there are none to wait for.
    pub fn loaded(&self) -> bool {
        self.replay.is_none()
    }

    /// Take in the entries kept, once they arrive. Returns whether they arrived now.
    ///
    /// An entry put since the cache opened stands over the one kept for its path.
    pub fn poll(&mut self, log: &mut Log) -> bool {
        match self.shelf.as_mut().and_then(Shelf::loaded) {
            Some(answer) => self.arrived(answer, log),
            None => false,
        }
    }

    /// [`Cache::poll`], waiting where the desktop can.
    fn wait(&mut self, log: &mut Log) -> bool {
        match self.shelf.as_mut().and_then(Shelf::wait) {
            Some(answer) => self.arrived(answer, log),
            None => false,
        }
    }

    /// Take in the entries kept, after the changes made while they loaded, which are
    /// then kept in turn.
    fn arrived(&mut self, answer: Result<Entries, String>, log: &mut Log) -> bool {
        let Some(replay) = self.replay.take() else {
            return false;
        };
        let mut kept = match answer {
            Ok(kept) => kept,
            Err(why) => {
                log.info(format!("what was read before is read again: {why}"));
                Entries::new()
            }
        };
        let changed = &mut self.changed;
        for change in replay {
            match change {
                Change::Moved(from, to) => moved(&mut kept, changed, &from, &to),
                Change::Forgot(path) => {
                    if kept.remove(&path).is_some() {
                        changed.insert(path);
                    }
                }
                Change::Kept(listed) => kept.retain(|path, _| {
                    let stays = listed.contains(path);
                    if !stays {
                        changed.insert(path.clone());
                    }
                    stays
                }),
            }
        }
        for (path, entry) in kept {
            self.entries.entry(path).or_insert(entry);
        }
        true
    }

    /// What was read of the file at `path`, where it still has the length and time
    /// `stat` gives.
    pub fn fresh(&self, path: &LibPath, stat: Stat) -> Option<&Entry> {
        self.entries.get(path).filter(|entry| entry.stat() == stat)
    }

    /// Keep `entry` for the file at `path`.
    pub fn put(&mut self, path: LibPath, entry: Entry) {
        if self.entries.get(&path) == Some(&entry) {
            return;
        }
        self.entries.insert(path.clone(), entry);
        self.changed.insert(path);
    }

    /// Forget the file at `path`.
    pub fn forget(&mut self, path: &LibPath) {
        if self.entries.remove(path).is_some() {
            self.changed.insert(path.clone());
        }
        if let Some(replay) = &mut self.replay {
            replay.push(Change::Forgot(path.clone()));
        }
    }

    /// Follow a rename of the file or folder `from` to `to`.
    pub fn moved(&mut self, from: &LibPath, to: &LibPath) {
        moved(&mut self.entries, &mut self.changed, from, to);
        if let Some(replay) = &mut self.replay {
            replay.push(Change::Moved(from.clone(), to.clone()));
        }
    }

    /// Forget every file a whole listing of the library did not find.
    pub fn keep_only(&mut self, listed: BTreeSet<LibPath>) {
        let gone: Vec<LibPath> = self
            .entries
            .keys()
            .filter(|path| !listed.contains(*path))
            .cloned()
            .collect();
        for path in gone {
            self.entries.remove(&path);
            self.changed.insert(path);
        }
        if let Some(replay) = &mut self.replay {
            replay.push(Change::Kept(listed));
        }
    }

    /// Keep what changed, off the frame, where nothing kept is still loading or being
    /// written.
    pub fn write(&mut self) {
        let Some(shelf) = self.shelf.as_mut().filter(|_| self.replay.is_none()) else {
            return;
        };
        if self.changed.is_empty() || shelf.busy() {
            return;
        }
        let changed = std::mem::take(&mut self.changed);
        shelf.write(&self.entries, changed);
    }

    /// Keep what changed before the library is let go, waiting where the desktop can.
    pub fn finish(&mut self, log: &mut Log) {
        self.wait(log);
        self.settle();
        self.write();
        self.settle();
    }

    /// Wait for a write in flight, where the desktop can.
    fn settle(&mut self) {
        if let Some(shelf) = &mut self.shelf {
            shelf.settle();
        }
    }

    /// Wait for the entries kept to arrive, and take them in.
    #[cfg(test)]
    pub fn arrive(&mut self, log: &mut Log) -> bool {
        self.wait(log)
    }

    /// Every entry, for a test to look at.
    #[cfg(test)]
    pub fn entries(&self) -> &BTreeMap<LibPath, Entry> {
        &self.entries
    }
}

/// Move every entry at or under `from` to the same place under `to`.
fn moved(entries: &mut Entries, changed: &mut BTreeSet<LibPath>, from: &LibPath, to: &LibPath) {
    let under: Vec<LibPath> = entries
        .range(from.clone()..)
        .map(|(path, _)| path)
        .take_while(|path| path.as_str().starts_with(from.as_str()))
        .filter(|path| path.is_in(from))
        .cloned()
        .collect();
    for path in under {
        let (Some(entry), Some(there)) = (entries.remove(&path), path.moved(from, to)) else {
            continue;
        };
        changed.insert(path);
        changed.insert(there.clone());
        entries.insert(there, entry);
    }
}

/// One entry as a record of its own: the version, then the entry.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn record(entry: &Entry) -> Result<String, String> {
    ron()
        .to_string(&(VERSION, entry))
        .map_err(|e| e.to_string())
}

/// The entry a record holds, or `None` for one that does not read or is of another
/// version.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn unrecord(text: &str) -> Option<Entry> {
    let (version, entry) = ron().from_str::<(u32, Entry)>(text).ok()?;
    (version == VERSION).then_some(entry)
}

#[cfg(not(target_arch = "wasm32"))]
pub use disk::Shelf;
#[cfg(target_arch = "wasm32")]
pub use idb::Shelf;

#[cfg(not(target_arch = "wasm32"))]
mod disk {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use eframe::egui;
    use serde::{Deserialize, Serialize};

    use super::{Cache, Entries, VERSION};
    use crate::store::LibPath;
    use crate::work::{self, Answer, Job};

    /// The file beside eframe's `app.ron`.
    const FILE: &str = "library-cache.ron";

    /// How many libraries the file keeps entries for: those written most lately.
    pub const LIBRARIES: usize = 8;

    /// One write of the file at a time, from every library this process opens.
    static WRITING: Mutex<()> = Mutex::new(());

    /// The file as written.
    #[derive(Default, Serialize, Deserialize)]
    pub(super) struct Shelved {
        pub version: u32,
        /// Each library's entries, under its folder's path.
        pub libraries: BTreeMap<String, Section>,
    }

    #[derive(Serialize, Deserialize)]
    pub(super) struct Section {
        /// Higher for a library written more lately.
        pub written: u64,
        pub entries: Entries,
    }

    /// Where one library's entries are kept on the desktop.
    pub struct Shelf {
        file: PathBuf,
        library: String,
        ctx: egui::Context,
        loading: Option<Job<Result<Entries, String>>>,
        writing: Option<Job<()>>,
    }

    impl Cache {
        /// The entries of the library at `root`, kept beside eframe's own store, or in
        /// memory only where the system names no place for it.
        pub fn open(ctx: &egui::Context, root: &Path) -> Cache {
            match eframe::storage_dir(crate::APP) {
                Some(dir) => Cache::at(ctx, dir.join(FILE), root),
                None => Cache::memory(),
            }
        }

        /// The entries of the library at `root`, kept in `file`.
        ///
        /// ⚠️ Kept in memory only where `file` would be inside the library: nothing of
        /// the cache is ever written into one.
        pub fn at(ctx: &egui::Context, file: PathBuf, root: &Path) -> Cache {
            if inside(&file, root) {
                return Cache::memory();
            }
            let library = root.to_string_lossy().into_owned();
            let (from, of) = (file.clone(), library.clone());
            let loading = work::run(ctx, move |_| load(&from, &of));
            Cache::kept(Shelf {
                file,
                library,
                ctx: ctx.clone(),
                loading: Some(loading),
                writing: None,
            })
        }
    }

    impl Shelf {
        /// The entries kept, once they have been read.
        pub(super) fn loaded(&mut self) -> Option<Result<Entries, String>> {
            let answer = match self.loading.as_ref()?.poll() {
                Answer::Running => return None,
                Answer::Answered(answer) => answer,
                Answer::Died => Err("the read stopped without an answer".to_string()),
            };
            self.loading = None;
            Some(answer)
        }

        /// [`Shelf::loaded`], waiting for the read in flight.
        pub(super) fn wait(&mut self) -> Option<Result<Entries, String>> {
            loop {
                match self.loading.as_ref()?.poll() {
                    Answer::Running => std::thread::yield_now(),
                    Answer::Answered(answer) => {
                        self.loading = None;
                        return Some(answer);
                    }
                    Answer::Died => {
                        self.loading = None;
                        return Some(Err("the read stopped without an answer".to_string()));
                    }
                }
            }
        }

        /// Whether a write is still in flight.
        pub(super) fn busy(&mut self) -> bool {
            let done = self
                .writing
                .as_ref()
                .is_none_or(|job| job.poll() != Answer::Running);
            if done {
                self.writing = None;
            }
            !done
        }

        pub(super) fn write(&mut self, entries: &Entries, _changed: BTreeSet<LibPath>) {
            let (file, library, entries) =
                (self.file.clone(), self.library.clone(), entries.clone());
            // An entry not kept is only read again.
            self.writing = Some(work::run(&self.ctx, move |_| {
                let _ = write(&file, &library, entries);
            }));
        }

        /// Wait for the write in flight.
        pub(super) fn settle(&mut self) {
            if let Some(job) = self.writing.take() {
                while job.poll() == Answer::Running {
                    std::thread::yield_now();
                }
            }
        }
    }

    /// Whether `file` is at or inside `root`, after links are followed where they can be.
    fn inside(file: &Path, root: &Path) -> bool {
        let real = |path: &Path| {
            let mut at = path;
            let mut rest = Vec::new();
            loop {
                if let Ok(real) = at.canonicalize() {
                    return rest
                        .iter()
                        .rev()
                        .fold(real, |path: PathBuf, part| path.join(part));
                }
                let (Some(parent), Some(name)) = (at.parent(), at.file_name()) else {
                    return path.to_path_buf();
                };
                rest.push(name.to_os_string());
                at = parent;
            }
        };
        file.starts_with(root) || real(file).starts_with(real(root))
    }

    /// The entries `file` keeps for `library`. A file that is not there is no entries.
    fn load(file: &Path, library: &str) -> Result<Entries, String> {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Entries::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut shelved = read(&text)?;
        Ok(shelved
            .libraries
            .remove(library)
            .map(|section| section.entries)
            .unwrap_or_default())
    }

    /// What a file of this text keeps.
    pub(super) fn read(text: &str) -> Result<Shelved, String> {
        let shelved = super::ron()
            .from_str::<Shelved>(text)
            .map_err(|e| e.to_string())?;
        match shelved.version {
            VERSION => Ok(shelved),
            other => Err(format!("it was written as version {other}")),
        }
    }

    /// Put `entries` in `file` as `library`'s, beside the other libraries' entries, and
    /// drop the libraries written least lately past [`LIBRARIES`].
    fn write(file: &Path, library: &str, entries: Entries) -> std::io::Result<()> {
        let _one = WRITING.lock().unwrap_or_else(|held| held.into_inner());
        let mut shelved = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| read(&text).ok())
            .unwrap_or_default();
        shelved.version = VERSION;
        let latest = shelved.libraries.values().map(|held| held.written).max();
        let written = latest.map_or(1, |latest| latest.saturating_add(1));
        shelved
            .libraries
            .insert(library.to_string(), Section { written, entries });
        while shelved.libraries.len() > LIBRARIES {
            let oldest = shelved
                .libraries
                .iter()
                .min_by_key(|(_, held)| held.written)
                .map(|(name, _)| name.clone());
            let Some(oldest) = oldest else { break };
            shelved.libraries.remove(&oldest);
        }
        let text = super::ron()
            .to_string(&shelved)
            .map_err(std::io::Error::other)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut temp = file.as_os_str().to_owned();
        temp.push(".tmp");
        let temp = PathBuf::from(temp);
        let wrote = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, file));
        if wrote.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        wrote
    }
}

#[cfg(target_arch = "wasm32")]
mod idb {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::rc::Rc;

    use eframe::egui;
    use js_sys::{Array, Promise};
    use wasm_bindgen::{JsCast as _, JsValue};
    use wasm_bindgen_futures::{spawn_local, JsFuture};
    use web_sys::{IdbKeyRange, IdbTransactionMode};

    use super::{record, unrecord, Cache, Entries};
    use crate::idb::{committed, database, done, FILES};
    use crate::js::describe;
    use crate::store::{LibPath, Root};

    /// Where one library's entries are kept in the browser: one record per file, keyed
    /// `<library id>/<path>`.
    pub struct Shelf {
        library: u32,
        loaded: Rc<RefCell<Option<Result<Entries, String>>>>,
        writing: Rc<Cell<bool>>,
    }

    impl Cache {
        /// The entries of the library at `root`, read from IndexedDB off the frame.
        pub fn open(ctx: &egui::Context, root: &Root) -> Cache {
            let library = match root {
                Root::Private => 0,
                Root::Picked(picked) => picked.id,
            };
            let loaded = Rc::new(RefCell::new(None));
            let answered = Rc::new(Cell::new(false));
            let answer = {
                let (into, answered, ctx) = (loaded.clone(), answered.clone(), ctx.clone());
                move |answer| {
                    if !answered.replace(true) {
                        *into.borrow_mut() = Some(answer);
                        ctx.request_repaint();
                    }
                }
            };
            let late = answer.clone();
            spawn_local(async move { answer(load(library).await) });
            spawn_local(async move {
                patience().await;
                late(Err("IndexedDB did not answer".to_string()));
            });
            Cache::kept(Shelf {
                library,
                loaded,
                writing: Rc::default(),
            })
        }
    }

    impl Shelf {
        pub(super) fn loaded(&mut self) -> Option<Result<Entries, String>> {
            self.loaded.borrow_mut().take()
        }

        /// The page cannot wait for IndexedDB: only what has arrived.
        pub(super) fn wait(&mut self) -> Option<Result<Entries, String>> {
            self.loaded()
        }

        pub(super) fn busy(&mut self) -> bool {
            self.writing.get()
        }

        /// Put each changed path's entry, and delete each dropped one, in one transaction.
        pub(super) fn write(&mut self, entries: &Entries, changed: BTreeSet<LibPath>) {
            let records: Vec<(String, Option<String>)> = changed
                .into_iter()
                .map(|path| {
                    let held = entries.get(&path).and_then(|entry| record(entry).ok());
                    (key(self.library, &path), held)
                })
                .collect();
            let writing = self.writing.clone();
            writing.set(true);
            spawn_local(async move {
                // An entry not kept is only read again.
                let _ = write(records).await;
                writing.set(false);
            });
        }

        /// The page cannot wait: what was sent is written after the library goes.
        pub(super) fn settle(&mut self) {}
    }

    /// Once the reads held for the cache have waited long enough: what it remembers then
    /// arrives as nothing, and the files are read.
    async fn patience() {
        const WAIT_MS: i32 = 2000;
        let waited = Promise::new(&mut |resolve, _| {
            if let Some(window) = web_sys::window() {
                let _ =
                    window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, WAIT_MS);
            }
        });
        let _ = JsFuture::from(waited).await;
    }

    fn key(library: u32, path: &LibPath) -> String {
        format!("{library}/{path}")
    }

    /// Every entry kept for `library`. A record that does not read is deleted.
    async fn load(library: u32) -> Result<Entries, String> {
        let db = database().await?;
        let read = async {
            let prefix = format!("{library}/");
            // `0` follows `/`, so the range holds exactly the keys under the prefix.
            let range = IdbKeyRange::bound_with_lower_open_and_upper_open(
                &prefix.clone().into(),
                &format!("{library}0").into(),
                false,
                true,
            )
            .map_err(|e| describe(&e))?;
            let store = db
                .transaction_with_str(FILES)
                .and_then(|transaction| transaction.object_store(FILES))
                .map_err(|e| describe(&e))?;
            let keys = store
                .get_all_keys_with_key(&range)
                .map_err(|e| describe(&e))?;
            let values = store.get_all_with_key(&range).map_err(|e| describe(&e))?;
            let (keys, values) = (done(&keys), done(&values));
            let keys: Array = keys.await?.unchecked_into();
            let values: Array = values.await?.unchecked_into();
            let mut entries = Entries::new();
            let mut bad = Vec::new();
            for (key, value) in keys.iter().zip(values.iter()) {
                let held = key.as_string().and_then(|key| {
                    let path = LibPath::parse(key.strip_prefix(&prefix)?)?;
                    Some((path, unrecord(&value.as_string()?)?))
                });
                match held {
                    Some((path, entry)) => _ = entries.insert(path, entry),
                    None => bad.push((key.as_string().unwrap_or_default(), None)),
                }
            }
            Ok::<_, String>((entries, bad))
        }
        .await;
        db.close();
        let (entries, bad) = read?;
        if !bad.is_empty() {
            write(bad).await?;
        }
        Ok(entries)
    }

    /// Put each record that has text, and delete each that has none.
    async fn write(records: Vec<(String, Option<String>)>) -> Result<(), String> {
        let db = database().await?;
        let written = async {
            let transaction = db
                .transaction_with_str_and_mode(FILES, IdbTransactionMode::Readwrite)
                .map_err(|e| describe(&e))?;
            let store = transaction.object_store(FILES).map_err(|e| describe(&e))?;
            for (key, text) in records {
                let key = JsValue::from(key);
                let sent = match text {
                    Some(text) => store.put_with_key(&text.into(), &key),
                    None => store.delete(&key),
                };
                sent.map_err(|e| describe(&e))?;
            }
            committed(&transaction).await
        }
        .await;
        db.close();
        written
    }

    /// Delete the records of every library but `kept` and the browser's own.
    pub async fn keep_libraries(kept: Vec<u32>) -> Result<(), String> {
        let db = database().await?;
        let keys = async {
            let all = db
                .transaction_with_str(FILES)
                .and_then(|transaction| transaction.object_store(FILES))
                .and_then(|store| store.get_all_keys())
                .map_err(|e| describe(&e))?;
            done(&all).await
        }
        .await;
        db.close();
        let keys: Array = keys?.unchecked_into();
        let gone: Vec<(String, Option<String>)> = keys
            .iter()
            .filter_map(|key| key.as_string())
            .filter(|key| {
                let library = key
                    .split_once('/')
                    .and_then(|(id, _)| id.parse::<u32>().ok());
                library.is_none_or(|id| id != 0 && !kept.contains(&id))
            })
            .map(|key| (key, None))
            .collect();
        match gone.is_empty() {
            true => Ok(()),
            false => write(gone).await,
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use idb::keep_libraries;

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    fn entry(len: u64) -> Entry {
        Entry {
            len,
            modified: Some(1_700_000_000_000_000_000),
            summary: Summary {
                kind: Kind::Program,
                tag: "ne5p".into(),
                crc32: Some(0x9abc_def0),
                plays: None,
                verdict: Verdict::Ok,
                generation: None,
                wavs: Vec::new(),
            },
        }
    }

    #[test]
    fn an_entry_stands_only_for_the_length_and_time_it_was_read_at() {
        let mut cache = Cache::memory();
        cache.put(path("Grand.ne5p"), entry(100));
        let stat = entry(100).stat();
        assert!(cache.fresh(&path("Grand.ne5p"), stat).is_some());
        let longer = Stat { len: 101, ..stat };
        let touched = Stat {
            modified: Some(1),
            ..stat
        };
        assert!(cache.fresh(&path("Grand.ne5p"), longer).is_none());
        assert!(cache.fresh(&path("Grand.ne5p"), touched).is_none());
        assert!(cache.fresh(&path("Other.ne5p"), stat).is_none());
    }

    #[test]
    fn a_rename_carries_the_entries_at_and_under_it_and_none_beside_it() {
        let mut cache = Cache::memory();
        for at in [
            "Sets",
            "Sets/A.ne5p",
            "Sets/Deep/B.ne5p",
            "Sets b/C.ne5p",
            "D.ne5p",
        ] {
            cache.put(path(at), entry(1));
        }
        cache.moved(&path("Sets"), &path("Gigs"));
        let paths: Vec<&str> = cache.entries().keys().map(LibPath::as_str).collect();
        assert_eq!(
            paths,
            [
                "D.ne5p",
                "Gigs",
                "Gigs/A.ne5p",
                "Gigs/Deep/B.ne5p",
                "Sets b/C.ne5p"
            ]
        );
    }

    #[test]
    fn files_a_whole_listing_did_not_find_are_forgotten() {
        let mut cache = Cache::memory();
        cache.put(path("Kept.ne5p"), entry(1));
        cache.put(path("Gone.ne5p"), entry(2));
        cache.keep_only([path("Kept.ne5p"), path("New.ne5p")].into());
        let paths: Vec<&str> = cache.entries().keys().map(LibPath::as_str).collect();
        assert_eq!(paths, ["Kept.ne5p"]);
    }

    #[test]
    fn a_record_reads_back_only_under_this_version() {
        let held = entry(7);
        let text = record(&held).unwrap();
        assert_eq!(unrecord(&text), Some(held));
        let newer = text.replacen(&format!("({VERSION},"), &format!("({},", VERSION + 1), 1);
        assert_ne!(newer, text);
        assert_eq!(unrecord(&newer), None);
        assert_eq!(unrecord("not a record"), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_rename_made_while_the_entries_load_carries_the_kept_entry() {
        let dir = crate::testing::Temp::new();
        let (file, root) = (dir.at("library-cache.ron"), dir.at("library"));
        let ctx = eframe::egui::Context::default();
        let mut log = Log::default();
        let mut first = Cache::at(&ctx, file.clone(), &root);
        first.finish(&mut log);
        first.put(path("Sets/Grand.ne5p"), entry(1));
        first.put(path("Gone.ne5p"), entry(2));
        first.finish(&mut log);

        let mut second = Cache::at(&ctx, file.clone(), &root);
        second.moved(&path("Sets"), &path("Gigs"));
        second.forget(&path("Gone.ne5p"));
        assert!(!second.loaded());
        second.finish(&mut log);
        let paths: Vec<&str> = second.entries().keys().map(LibPath::as_str).collect();
        assert_eq!(paths, ["Gigs/Grand.ne5p"]);

        let mut third = Cache::at(&ctx, file, &root);
        third.finish(&mut log);
        let paths: Vec<&str> = third.entries().keys().map(LibPath::as_str).collect();
        assert_eq!(paths, ["Gigs/Grand.ne5p"], "and keeps it so");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_file_keeps_the_libraries_written_most_lately() {
        let dir = crate::testing::Temp::new();
        let file = dir.at("library-cache.ron");
        let ctx = eframe::egui::Context::default();
        let mut log = Log::default();
        for n in 0..=disk::LIBRARIES {
            let root = dir.at(&format!("library-{n}"));
            let mut cache = Cache::at(&ctx, file.clone(), &root);
            cache.finish(&mut log);
            cache.put(path("Grand.ne5p"), entry(n as u64));
            cache.finish(&mut log);
        }
        let shelved = disk::read(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(shelved.libraries.len(), disk::LIBRARIES);
        let first = dir.at("library-0").to_string_lossy().into_owned();
        assert!(
            !shelved.libraries.contains_key(&first),
            "written least lately"
        );
    }
}
