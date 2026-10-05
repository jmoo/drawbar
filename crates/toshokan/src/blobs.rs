//! The content-addressed blob store: bytes named by their BLAKE3 hash.
//!
//! A blob is written under the writer's `tmp/` directory and renamed into
//! `blobs/<hash>`, so a reader never sees a partial blob, and it is never rewritten.

use std::collections::BTreeSet;

use crate::compact::compact;
use crate::effects::Stored;
use crate::error::{Error, Result};
use crate::fs::{ensure_dir, hash_file, Fs, RelPath};
use crate::ids::{Version, WriterId};
use crate::layout::Layout;
use crate::log::{read_log, read_logs, Kind, LogWriter};
use crate::merge::{merge, State};
use crate::value::BlobId;

/// The bytes of `blob`, refused as corrupt when they do not hash to its id.
pub async fn get<F: Fs>(fs: &F, layout: &Layout, blob: BlobId) -> Result<Vec<u8>> {
    let path = layout.blob(blob);
    let bytes = fs.read(&path).await?;
    let found = BlobId::of(&bytes);
    if found != blob {
        return Err(Error::Corrupt {
            path,
            reason: format!("its contents hash to {found}"),
        });
    }
    Ok(bytes)
}

/// Write `bytes` durably to `tmp/<writer>/<hash>`, replacing a file a crash left
/// there, and return the path.
pub(crate) async fn stage<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    bytes: &[u8],
) -> Result<RelPath> {
    let dir = layout.tmp(writer);
    ensure_dir(fs, &dir).await?;
    let path = staged(layout, writer, BlobId::of(bytes));
    match fs.create(&path, bytes).await {
        Err(Error::AlreadyExists { .. }) => {
            fs.remove_file(&path).await?;
            fs.create(&path, bytes).await?;
        }
        created => created?,
    }
    fs.sync(&path).await?;
    fs.sync(&dir).await?;
    Ok(path)
}

/// Where [`stage`] puts the bytes of `blob`.
pub(crate) fn staged(layout: &Layout, writer: WriterId, blob: BlobId) -> RelPath {
    layout
        .tmp(writer)
        .join(&blob.to_string())
        .expect("a blob id is one path component")
}

/// Move the file at `path`, whose bytes are `stored`, into the store, durably, or
/// remove it when the store holds those bytes already. First this writer's log
/// durably holds an add of them not since removed, appending one under an intent of
/// its own when it does not, so a collection that reads the logs after the file
/// enters the store sees the add. Making the file's absence at `path` durable is the
/// caller's step.
pub(crate) async fn displace<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    path: &RelPath,
    stored: Stored,
) -> Result<()> {
    let writer = log.writer();
    let own = merge(&[read_log(fs, layout, writer).await?]);
    let held = own
        .blob_adds()
        .get(&stored.blob)
        .and_then(|by| by.get(&writer));
    if !held.is_some_and(|add| !add.removed) {
        let intent = log.new_intent();
        let entries = [Kind::INTENT, stored.added()].map(|kind| log.stamp(intent, kind));
        log.append(fs, layout, &entries).await?;
    }
    release(fs, layout, writer, path, stored).await
}

/// Remove the file at `path`, whose bytes are `stored`, when the store holds them, or
/// else move it in.
///
/// ⚠️ Call this only once this writer's log durably holds `blob_added` for `stored`.
/// A collection that read the logs before then may remove the store copy this finds,
/// or the file this moves in.
pub(crate) async fn release<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    path: &RelPath,
    stored: Stored,
) -> Result<()> {
    match enter(fs, layout, writer, path, stored).await? {
        Entered::Moved => Ok(()),
        Entered::AlreadyStored => fs.remove_file(path).await,
    }
}

/// How [`enter`] left a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entered {
    /// Renamed into the store.
    Moved,
    /// Still at its path, because the store holds its bytes already.
    AlreadyStored,
}

/// Rename the file at `path`, whose bytes are `stored`, into the store, durably,
/// unless the store holds those bytes already. A store copy whose bytes are not
/// `stored` is moved into this writer's quarantine first, so the file takes its place.
async fn enter<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    path: &RelPath,
    stored: Stored,
) -> Result<Entered> {
    let target = layout.blob(stored.blob);
    ensure_dir(fs, &layout.blobs()).await?;
    for _ in 0..2 {
        match fs.rename(path, &target).await {
            Ok(()) => {
                fs.sync(&layout.blobs()).await?;
                return Ok(Entered::Moved);
            }
            Err(Error::AlreadyExists { .. }) => {}
            Err(error) => return Err(error),
        }
        match holds(fs, &target, stored).await? {
            Some(true) => return Ok(Entered::AlreadyStored),
            Some(false) => quarantine(fs, layout, writer, &target).await?,
            None => {}
        }
    }
    Err(Error::AlreadyExists { path: target })
}

/// Whether the file at `path` holds `stored`, by length and then by hash; `None` when
/// there is no file.
async fn holds<F: Fs>(fs: &F, path: &RelPath, stored: Stored) -> Result<Option<bool>> {
    let Some(found) = fs.metadata(path).await? else {
        return Ok(None);
    };
    if found.len != stored.len {
        return Ok(Some(false));
    }
    Ok(Some(
        hash_file(fs, path).await? == (stored.blob, stored.len),
    ))
}

/// Move a file whose bytes are not its name to `quarantine/<writer>/<hash>`, named
/// by the hash of its bytes.
pub(crate) async fn quarantine<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    path: &RelPath,
) -> Result<()> {
    let (found, _) = hash_file(fs, path).await?;
    let dir = layout.quarantine(writer);
    ensure_dir(fs, &dir).await?;
    match fs.rename(path, &dir.join(&found.to_string())?).await {
        Ok(()) => fs.sync(&dir).await?,
        // The quarantine keeps these bytes already.
        Err(Error::AlreadyExists { .. }) => fs.remove_file(path).await?,
        Err(error) => return Err(error),
    }
    fs.sync(&layout.blobs()).await
}

/// What a garbage collection removed and what it left.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Collection {
    pub removed: Vec<BlobId>,
    pub freed: u64,
    /// Bytes this writer's blobs still occupy.
    pub kept: u64,
}

/// Remove this writer's eligible blobs, oldest first, until its blobs occupy at most
/// `budget` bytes, logging `BlobRemoved` for each.
///
/// A blob is eligible when this writer holds it, no live value refers to it, no entry
/// inside any writer's retained undo window refers to it, no other writer holds it,
/// and it is not in `needed`. Eligibility is judged from every writer's log as read
/// now, and judged again once the chosen blobs are [`set_aside`]; a blob another
/// writer added or referred to in between goes back.
pub(crate) async fn collect<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    needed: &[BlobId],
    budget: u64,
) -> Result<Collection> {
    log.writable()?;
    let writer = log.writer();
    let state = merge(&read_logs(fs, layout).await?);
    let chosen = choose(writer, &state, needed, budget);
    let mut gone = BTreeSet::new();
    let mut aside = Vec::new();
    if !chosen.picked.is_empty() {
        ensure_dir(fs, &layout.tmp(writer)).await?;
    }
    for stored in &chosen.picked {
        let to = set_aside(layout, writer, stored.blob);
        match fs.rename(&layout.blob(stored.blob), &to).await {
            Ok(()) => aside.push((to, *stored)),
            Err(Error::NotFound { .. }) => {
                gone.insert(stored.blob);
            }
            Err(Error::AlreadyExists { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    if !aside.is_empty() {
        fs.sync(&layout.tmp(writer)).await?;
        fs.sync(&layout.blobs()).await?;
        let claimed = claimed(writer, &merge(&read_logs(fs, layout).await?), needed);
        for (path, stored) in aside {
            if claimed.contains(&stored.blob) {
                release(fs, layout, writer, &path, stored).await?;
            } else {
                fs.remove_file(&path).await?;
                gone.insert(stored.blob);
            }
        }
        fs.sync(&layout.tmp(writer)).await?;
    }
    let removed: Vec<Stored> = chosen
        .picked
        .into_iter()
        .filter(|stored| gone.contains(&stored.blob))
        .collect();
    let freed = removed.iter().map(|stored| stored.len).sum();
    if !removed.is_empty() {
        let intent = log.new_intent();
        let removals = removed
            .iter()
            .map(|stored| Kind::BlobRemoved { blob: stored.blob });
        let entries: Vec<_> = std::iter::once(Kind::INTENT)
            .chain(removals)
            .map(|kind| log.stamp(intent, kind))
            .collect();
        log.append(fs, layout, &entries).await?;
    }
    Ok(Collection {
        removed: removed.iter().map(|stored| stored.blob).collect(),
        freed,
        kept: chosen.held.saturating_sub(freed),
    })
}

/// Where a collection keeps `blob` while it judges it again: `tmp/<writer>/aside-<hash>`.
/// The writer's log still holds the blob's add until the collection logs its removal.
pub(crate) fn set_aside(layout: &Layout, writer: WriterId, blob: BlobId) -> RelPath {
    layout
        .tmp(writer)
        .join(&format!("{ASIDE}{blob}"))
        .expect("a blob id is one path component")
}

/// The blob a file name from [`set_aside`] names.
pub(crate) fn aside_blob(name: &str) -> Option<BlobId> {
    name.strip_prefix(ASIDE)?.parse().ok()
}

const ASIDE: &str = "aside-";

struct Chosen {
    /// The blobs to remove, oldest first.
    picked: Vec<Stored>,
    /// Bytes of every blob this writer holds.
    held: u64,
}

fn choose(writer: WriterId, state: &State, needed: &[BlobId], budget: u64) -> Chosen {
    let mut own: Vec<(Version, Stored)> = state
        .blob_adds()
        .iter()
        .filter_map(|(blob, by)| {
            let add = by.get(&writer).filter(|add| !add.removed)?;
            Some((
                add.version,
                Stored {
                    blob: *blob,
                    len: add.len,
                },
            ))
        })
        .collect();
    let held = own
        .iter()
        .fold(0u64, |sum, (_, stored)| sum.saturating_add(stored.len));
    let claimed = claimed(writer, state, needed);
    own.retain(|(_, stored)| !claimed.contains(&stored.blob));
    own.sort_by_key(|(version, stored)| (*version, stored.blob));
    let mut left = held;
    let mut picked = Vec::new();
    for (_, stored) in own {
        if left <= budget {
            break;
        }
        left -= stored.len;
        picked.push(stored);
    }
    Chosen { picked, held }
}

/// The blobs `writer` may not remove: those referred to, those `needed`, and those
/// another writer holds.
fn claimed(writer: WriterId, state: &State, needed: &[BlobId]) -> BTreeSet<BlobId> {
    let held_by_others = state
        .blob_adds()
        .iter()
        .filter(|(_, by)| {
            by.iter()
                .any(|(other, add)| *other != writer && !add.removed)
        })
        .map(|(blob, _)| *blob);
    let mut claimed = state.referenced_blobs();
    claimed.extend(needed);
    claimed.extend(held_by_others);
    claimed
}

/// Free space for a save the disk refused: collect every blob nothing needs but
/// `needed`, the saves' own sources, and with `evict_undo` first drop this writer's
/// undo history so the blobs only it kept can go too.
pub(crate) async fn make_room<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    needed: &[BlobId],
    evict_undo: bool,
) -> Result<()> {
    if evict_undo {
        let own = read_log(fs, layout, log.writer()).await?;
        compact(fs, layout, log, &own, 0).await?;
    }
    collect(fs, layout, log, needed, 0).await.map(drop)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Put `bytes` in the store under their hash, logging nothing.
    pub(crate) fn plant<F: Fs>(fs: &F, layout: &Layout, bytes: &[u8]) -> BlobId {
        let blob = BlobId::of(bytes);
        pollster::block_on(ensure_dir(fs, &layout.blobs())).unwrap();
        pollster::block_on(fs.create(&layout.blob(blob), bytes)).unwrap();
        blob
    }
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

    use super::*;
    use crate::fs::MemFs;
    use crate::log::testing::{field, logged, reopen, Session};
    use crate::value::Value;

    const WRITER: WriterId = WriterId::from_u128(0xa);
    const OTHER: WriterId = WriterId::from_u128(0xb);

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    #[test]
    fn a_blob_whose_bytes_do_not_match_its_name_is_corrupt() {
        let (fs, layout) = (MemFs::new(), Layout::default());
        let blob = BlobId::of(b"expected");
        block_on(ensure_dir(&fs, &layout.blobs())).unwrap();
        block_on(fs.create(&layout.blob(blob), b"other")).unwrap();
        let result = block_on(get(&fs, &layout, blob));
        assert!(matches!(result, Err(Error::Corrupt { .. })), "{result:?}");
    }

    #[test]
    fn a_new_directory_survives_a_crash_once_ensured() {
        let fs = MemFs::new();
        block_on(ensure_dir(&fs, &path("a/b/c"))).unwrap();
        let mutations = fs.mutations();
        block_on(ensure_dir(&fs, &path("a/b"))).unwrap();
        assert_eq!(
            fs.mutations(),
            mutations,
            "an existing directory is left alone"
        );
        assert_eq!(
            fs.restart().directories(),
            [path("a"), path("a/b"), path("a/b/c")].into()
        );
    }

    /// A blob store beside logs that writers append to as an app would.
    struct Store {
        fs: MemFs,
        layout: Layout,
    }

    impl Store {
        fn new() -> Self {
            Self {
                fs: MemFs::new(),
                layout: Layout::default(),
            }
        }

        /// Put `bytes` in the store, if they are not there, and log their add as
        /// `writer`.
        fn add(&self, writer: WriterId, bytes: &[u8]) -> BlobId {
            if !self.stored(BlobId::of(bytes)) {
                testing::plant(&self.fs, &self.layout, bytes);
            }
            self.log(writer, vec![Stored::of(bytes).added()]);
            BlobId::of(bytes)
        }

        fn log(&self, writer: WriterId, kinds: Vec<Kind>) {
            Session::open(&self.fs, writer).act(kinds);
        }

        /// Fold each writer's log with an empty undo window, so that no entry an undo
        /// could reach refers to a blob.
        fn fold(&self, writers: &[WriterId]) {
            for &writer in writers {
                let own = block_on(read_log(&self.fs, &self.layout, writer)).unwrap();
                let mut log = reopen(&self.fs, writer);
                block_on(compact(&self.fs, &self.layout, &mut log, &own, 0)).unwrap();
            }
        }

        fn collect(&self, needed: &[BlobId], budget: u64) -> Collection {
            let mut log = reopen(&self.fs, WRITER);
            block_on(collect(&self.fs, &self.layout, &mut log, needed, budget)).unwrap()
        }

        fn stored(&self, blob: BlobId) -> bool {
            self.fs.files().contains_key(&self.layout.blob(blob))
        }
    }

    #[test]
    fn collection_removes_the_oldest_eligible_blobs_until_under_budget() {
        let store = Store::new();
        let oldest = store.add(WRITER, b"oldest");
        let middle = store.add(WRITER, b"middle");
        let newest = store.add(WRITER, b"newest");
        store.fold(&[WRITER]);
        let collection = store.collect(&[], 6);
        assert_eq!(collection.removed, [oldest, middle]);
        assert_eq!((collection.freed, collection.kept), (12, 6));
        assert!(store.stored(newest));
        assert!(!store.stored(oldest) && !store.stored(middle));
        let kinds: Vec<Kind> = logged(&store.fs, WRITER)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                Kind::INTENT,
                Kind::BlobRemoved { blob: oldest },
                Kind::BlobRemoved { blob: middle },
            ]
        );
    }

    #[test]
    fn collection_keeps_what_another_writer_a_reference_or_the_caller_still_needs() {
        let store = Store::new();
        let referenced = store.add(WRITER, b"referenced");
        let mut session = Session::open(&store.fs, WRITER);
        let entity = session.log.new_entity();
        session.act(vec![
            Kind::Create { entity },
            field(entity, "sound", Some(Value::Blob(referenced))),
        ]);
        let shared = store.add(WRITER, b"shared");
        store.add(OTHER, b"shared");
        let foreign = store.add(OTHER, b"foreign");
        let released = store.add(WRITER, b"released");
        store.log(WRITER, vec![Kind::BlobRemoved { blob: released }]);
        let released_by_other = store.add(WRITER, b"released by other");
        store.add(OTHER, b"released by other");
        store.log(
            OTHER,
            vec![Kind::BlobRemoved {
                blob: released_by_other,
            }],
        );
        let needed = store.add(WRITER, b"needed");
        store.fold(&[WRITER, OTHER]);

        let collection = store.collect(&[needed], 0);
        assert_eq!(collection.removed, [released_by_other]);
        assert_eq!(
            collection.kept,
            (b"referenced".len() + b"shared".len() + b"needed".len()) as u64
        );
        for blob in [referenced, shared, foreign, released, needed] {
            assert!(store.stored(blob), "{blob:?} was removed");
        }
    }

    #[test]
    fn collection_under_budget_removes_and_logs_nothing() {
        let store = Store::new();
        store.add(WRITER, b"small");
        store.fold(&[WRITER]);
        let (mutations, entries) = (store.fs.mutations(), logged(&store.fs, WRITER));
        let collection = store.collect(&[], 5);
        assert_eq!(collection.removed, Vec::<BlobId>::new());
        assert_eq!(collection.kept, 5);
        assert_eq!(store.fs.mutations(), mutations);
        assert_eq!(logged(&store.fs, WRITER), entries);
    }

    #[test]
    fn collecting_a_blob_already_gone_still_logs_its_removal() {
        let store = Store::new();
        let gone = store.add(WRITER, b"gone");
        store.fold(&[WRITER]);
        block_on(store.fs.remove_file(&store.layout.blob(gone))).unwrap();
        assert_eq!(store.collect(&[], 0).removed, [gone]);
        assert!(logged(&store.fs, WRITER)
            .iter()
            .any(|e| e.kind == Kind::BlobRemoved { blob: gone }));
    }

    #[test]
    fn a_file_whose_blob_is_collected_after_the_check_and_before_the_add_takes_its_place() {
        let store = Store::new();
        let blob = store.add(WRITER, b"same");
        store.fold(&[WRITER]);
        let song = path("song");
        block_on(store.fs.create(&song, b"same")).unwrap();
        let stored = Stored::of(b"same");

        let entered = block_on(enter(&store.fs, &store.layout, OTHER, &song, stored));
        assert_eq!(entered.unwrap(), Entered::AlreadyStored);
        assert_eq!(store.collect(&[], 0).removed, [blob]);
        store.log(OTHER, vec![stored.added()]);
        block_on(release(&store.fs, &store.layout, OTHER, &song, stored)).unwrap();

        let files = store.fs.files();
        assert_eq!(files.get(&store.layout.blob(blob)), Some(&b"same".to_vec()));
        assert!(!files.contains_key(&song));
    }

    #[test]
    fn a_displaced_file_is_in_the_store_only_once_its_add_is_logged() {
        let (song, stored) = (path("song"), Stored::of(b"song"));
        let displaced = |crash: Option<u64>| {
            let store = Store::new();
            block_on(store.fs.create(&song, b"song")).unwrap();
            block_on(store.fs.sync(&song)).unwrap();
            block_on(store.fs.sync(&RelPath::ROOT)).unwrap();
            let mut log = reopen(&store.fs, WRITER);
            let start = store.fs.mutations();
            if let Some(crash) = crash {
                store.fs.crash_after(crash);
            }
            let result = block_on(displace(&store.fs, &store.layout, &mut log, &song, stored));
            (store, result, start)
        };
        let (clean, result, start) = displaced(None);
        result.unwrap();
        let crashes = (0..clean.fs.mutations() - start).map(Some);
        for crash in crashes.chain([None]) {
            let (crashed, result, _) = displaced(crash);
            assert_eq!(
                result.is_err(),
                crash.is_some(),
                "crash {crash:?}: {result:?}"
            );
            let disk = crashed.fs.restart();
            let arrived = disk.files().contains_key(&crashed.layout.blob(stored.blob));
            let logged = logged(&disk, WRITER)
                .iter()
                .any(|e| e.kind == stored.added());
            assert!(
                !arrived || logged,
                "crash {crash:?}: the blob arrived unlogged"
            );
        }
    }

    #[test]
    fn displacing_bytes_the_store_holds_logs_their_add_before_removing_the_file() {
        let store = Store::new();
        let blob = store.add(OTHER, b"same");
        let song = path("song");
        block_on(store.fs.create(&song, b"same")).unwrap();
        let mut log = reopen(&store.fs, WRITER);
        let stored = Stored::of(b"same");
        block_on(displace(&store.fs, &store.layout, &mut log, &song, stored)).unwrap();
        assert!(store.stored(blob));
        assert!(!store.fs.files().contains_key(&song));
        let kinds: Vec<Kind> = logged(&store.fs, WRITER)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, [Kind::INTENT, stored.added()]);
    }
}
