//! The content-addressed blob store: bytes named by their BLAKE3 hash.
//!
//! A blob is written under the writer's `tmp/` directory and renamed into
//! `blobs/<hash>`, so a reader never sees a partial blob, and it is never rewritten.

use std::collections::{BTreeMap, BTreeSet};

use crate::compact::compact;
use crate::error::{Error, Result};
use crate::fs::{ensure_dir, hash_file, sync_parent, Fs, RelPath};
use crate::ids::{Version, WriterId};
use crate::layout::Layout;
use crate::log::{read_log, read_logs, Kind, LogWriter};
use crate::merge::{merge, BlobAdd, State};
use crate::value::BlobId;

/// Store `bytes` and return their id. Storing bytes already present changes nothing.
/// The caller logs `BlobAdded`.
pub async fn put<F: Fs>(fs: &F, layout: &Layout, writer: WriterId, bytes: &[u8]) -> Result<BlobId> {
    let blob = BlobId::of(bytes);
    if fs.metadata(&layout.blob(blob)).await?.is_some() {
        return Ok(blob);
    }
    let staged = stage(fs, layout, writer, bytes).await?;
    displace(fs, layout, &staged, blob).await?;
    sync_parent(fs, &staged).await?;
    Ok(blob)
}

/// Move the file at `path` into the store by rename, hashing it first, and return
/// its id and length. The caller logs `BlobAdded`.
///
/// ⚠️ A program writing the file between the hash and the rename leaves a blob whose
/// name is not its hash.
pub async fn adopt<F: Fs>(fs: &F, layout: &Layout, path: &RelPath) -> Result<(BlobId, u64)> {
    let (blob, len) = hash_file(fs, path).await?;
    displace(fs, layout, path, blob).await?;
    sync_parent(fs, path).await?;
    Ok((blob, len))
}

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

/// Move the file at `path`, whose contents hash to `blob`, into the store. When the
/// store already holds `blob` the file is removed instead. The move is durable in
/// the store; making the file's absence at `path` durable is the caller's step.
pub(crate) async fn displace<F: Fs>(
    fs: &F,
    layout: &Layout,
    path: &RelPath,
    blob: BlobId,
) -> Result<()> {
    ensure_dir(fs, &layout.blobs()).await?;
    match fs.rename(path, &layout.blob(blob)).await {
        Err(Error::AlreadyExists { .. }) => fs.remove_file(path).await,
        moved => {
            moved?;
            fs.sync(&layout.blobs()).await
        }
    }
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
/// A blob is eligible when this writer added it, no live value refers to it, no entry
/// inside any writer's retained undo window refers to it, and no other writer's
/// retained log has added it.
pub async fn collect<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    state: &State,
    budget: u64,
) -> Result<Collection> {
    let referenced = state.referenced_blobs();
    collect_with(fs, layout, log, state.blob_adds(), &referenced, budget).await
}

pub(crate) async fn collect_with<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    adds: &BTreeMap<BlobId, BTreeMap<WriterId, BlobAdd>>,
    referenced: &BTreeSet<BlobId>,
    budget: u64,
) -> Result<Collection> {
    log.writable()?;
    let chosen = choose(log.writer(), adds, referenced, budget);
    if chosen.removed.is_empty() {
        return Ok(Collection {
            removed: Vec::new(),
            freed: 0,
            kept: chosen.kept,
        });
    }
    let mut removed_any = false;
    for (blob, _) in &chosen.removed {
        match fs.remove_file(&layout.blob(*blob)).await {
            Ok(()) => removed_any = true,
            Err(Error::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    if removed_any {
        fs.sync(&layout.blobs()).await?;
    }
    let intent = log.new_intent();
    let removals = chosen
        .removed
        .iter()
        .map(|(blob, _)| Kind::BlobRemoved { blob: *blob });
    let entries: Vec<_> = std::iter::once(Kind::INTENT)
        .chain(removals)
        .map(|kind| log.stamp(intent, kind))
        .collect();
    log.append(fs, layout, &entries).await?;
    Ok(Collection {
        removed: chosen.removed.iter().map(|(blob, _)| *blob).collect(),
        freed: chosen.removed.iter().map(|(_, len)| len).sum(),
        kept: chosen.kept,
    })
}

struct Chosen {
    removed: Vec<(BlobId, u64)>,
    kept: u64,
}

fn choose(
    writer: WriterId,
    adds: &BTreeMap<BlobId, BTreeMap<WriterId, BlobAdd>>,
    referenced: &BTreeSet<BlobId>,
    budget: u64,
) -> Chosen {
    let held = |add: &BlobAdd| !add.removed;
    let own: Vec<(BlobId, &BlobAdd)> = adds
        .iter()
        .filter_map(|(blob, by)| {
            by.get(&writer)
                .filter(|add| held(add))
                .map(|add| (*blob, add))
        })
        .collect();
    let mut kept = own
        .iter()
        .fold(0u64, |sum, (_, add)| sum.saturating_add(add.len));
    let mut eligible: Vec<(Version, BlobId, u64)> = own
        .iter()
        .filter(|(blob, _)| !referenced.contains(blob))
        .filter(|(blob, _)| {
            !adds[blob]
                .iter()
                .any(|(other, add)| *other != writer && held(add))
        })
        .map(|(blob, add)| (add.version, *blob, add.len))
        .collect();
    eligible.sort();
    let mut removed = Vec::new();
    for (_, blob, len) in eligible {
        if kept <= budget {
            break;
        }
        kept -= len;
        removed.push((blob, len));
    }
    Chosen { removed, kept }
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
    let state = merge(&read_logs(fs, layout).await?);
    let mut referenced = state.referenced_blobs();
    referenced.extend(needed.iter().copied());
    collect_with(fs, layout, log, state.blob_adds(), &referenced, 0)
        .await
        .map(drop)
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

    use super::*;
    use crate::fs::MemFs;
    use crate::log::testing::{logged, reopen};
    use crate::log::Entry;

    const WRITER: WriterId = WriterId::from_u128(0xa);
    const OTHER: WriterId = WriterId::from_u128(0xb);

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn store(fs: &MemFs, layout: &Layout) -> BTreeMap<RelPath, Vec<u8>> {
        fs.files()
            .into_iter()
            .filter(|(path, _)| path.starts_with(&layout.blobs()))
            .collect()
    }

    #[test]
    fn put_names_a_blob_by_its_hash_and_leaves_nothing_in_tmp() {
        let (fs, layout) = (MemFs::new(), Layout::default());
        let blob = block_on(put(&fs, &layout, WRITER, b"bytes")).unwrap();
        assert_eq!(blob, BlobId::of(b"bytes"));
        assert_eq!(block_on(get(&fs, &layout, blob)).unwrap(), b"bytes");
        assert_eq!(
            fs.files().keys().cloned().collect::<Vec<_>>(),
            [layout.blob(blob)]
        );
    }

    #[test]
    fn putting_bytes_already_stored_changes_nothing() {
        let (fs, layout) = (MemFs::new(), Layout::default());
        block_on(put(&fs, &layout, WRITER, b"bytes")).unwrap();
        let before = fs.mutations();
        block_on(put(&fs, &layout, OTHER, b"bytes")).unwrap();
        assert_eq!(fs.mutations(), before);
    }

    #[test]
    fn a_put_interrupted_anywhere_leaves_no_partial_blob() {
        let layout = Layout::default();
        let clean = MemFs::new();
        block_on(put(&clean, &layout, WRITER, b"bytes")).unwrap();
        for crash in 0..clean.mutations() {
            let fs = MemFs::new();
            fs.crash_after(crash);
            assert!(block_on(put(&fs, &layout, WRITER, b"bytes")).is_err());
            let disk = fs.restart();
            for (path, bytes) in store(&disk, &layout) {
                assert_eq!(path, layout.blob(BlobId::of(&bytes)), "crash {crash}");
            }
        }
        assert_eq!(
            store(&clean.restart(), &layout)
                .into_values()
                .collect::<Vec<_>>(),
            [b"bytes".to_vec()],
            "a completed put is durable"
        );
    }

    #[test]
    fn adopting_a_file_moves_it_into_the_store() {
        let (fs, layout) = (MemFs::new(), Layout::default());
        block_on(fs.create(&path("song.npno"), b"old")).unwrap();
        let adopted = block_on(adopt(&fs, &layout, &path("song.npno"))).unwrap();
        assert_eq!(adopted, (BlobId::of(b"old"), 3));
        assert_eq!(
            fs.files(),
            [(layout.blob(adopted.0), b"old".to_vec())].into()
        );
    }

    #[test]
    fn adopting_bytes_already_stored_removes_the_file_and_keeps_the_blob() {
        let (fs, layout) = (MemFs::new(), Layout::default());
        let blob = block_on(put(&fs, &layout, WRITER, b"same")).unwrap();
        block_on(fs.create(&path("copy"), b"same")).unwrap();
        assert_eq!(
            block_on(adopt(&fs, &layout, &path("copy"))).unwrap(),
            (blob, 4)
        );
        assert_eq!(fs.files(), [(layout.blob(blob), b"same".to_vec())].into());
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

    fn add(len: u64, lamport: u64, writer: WriterId, removed: bool) -> BlobAdd {
        BlobAdd {
            len,
            version: Version::new(lamport, writer),
            removed,
        }
    }

    struct Library {
        fs: MemFs,
        layout: Layout,
        adds: BTreeMap<BlobId, BTreeMap<WriterId, BlobAdd>>,
    }

    impl Library {
        fn new() -> Self {
            Self {
                fs: MemFs::new(),
                layout: Layout::default(),
                adds: BTreeMap::new(),
            }
        }

        fn blob(&mut self, bytes: &[u8], adds: &[(WriterId, u64, bool)]) -> BlobId {
            let blob = block_on(put(&self.fs, &self.layout, WRITER, bytes)).unwrap();
            let len = bytes.len() as u64;
            self.adds.insert(
                blob,
                adds.iter()
                    .map(|&(writer, lamport, removed)| (writer, add(len, lamport, writer, removed)))
                    .collect(),
            );
            blob
        }

        fn collect(&self, log: &mut LogWriter, referenced: &[BlobId], budget: u64) -> Collection {
            let referenced = referenced.iter().copied().collect();
            block_on(collect_with(
                &self.fs,
                &self.layout,
                log,
                &self.adds,
                &referenced,
                budget,
            ))
            .unwrap()
        }

        fn stored(&self, blob: BlobId) -> bool {
            self.fs.files().contains_key(&self.layout.blob(blob))
        }
    }

    #[test]
    fn collection_removes_the_oldest_eligible_blobs_until_under_budget() {
        let mut library = Library::new();
        let newest = library.blob(b"newest", &[(WRITER, 9, false)]);
        let oldest = library.blob(b"oldest", &[(WRITER, 1, false)]);
        let middle = library.blob(b"middle", &[(WRITER, 5, false)]);
        let mut log = reopen(&library.fs, WRITER);
        let collection = library.collect(&mut log, &[], 6);
        assert_eq!(collection.removed, [oldest, middle]);
        assert_eq!((collection.freed, collection.kept), (12, 6));
        assert!(library.stored(newest));
        assert!(!library.stored(oldest) && !library.stored(middle));
        let kinds: Vec<Kind> = logged(&library.fs, WRITER)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                Kind::Intent {
                    label: None,
                    reverses: None
                },
                Kind::BlobRemoved { blob: oldest },
                Kind::BlobRemoved { blob: middle },
            ]
        );
    }

    #[test]
    fn collection_keeps_what_another_writer_or_a_reference_still_needs() {
        let mut library = Library::new();
        let referenced = library.blob(b"referenced", &[(WRITER, 1, false)]);
        let shared = library.blob(b"shared", &[(WRITER, 2, false), (OTHER, 3, false)]);
        let foreign = library.blob(b"foreign", &[(OTHER, 4, false)]);
        let released = library.blob(b"released", &[(WRITER, 5, true)]);
        let released_by_other = library.blob(
            b"released by other",
            &[(WRITER, 6, false), (OTHER, 7, true)],
        );
        let mut log = reopen(&library.fs, WRITER);
        let collection = library.collect(&mut log, &[referenced], 0);
        assert_eq!(collection.removed, [released_by_other]);
        assert_eq!(
            collection.kept,
            (b"referenced".len() + b"shared".len()) as u64
        );
        for blob in [referenced, shared, foreign, released] {
            assert!(library.stored(blob), "{blob:?} was removed");
        }
    }

    #[test]
    fn collection_under_budget_removes_and_logs_nothing() {
        let mut library = Library::new();
        library.blob(b"small", &[(WRITER, 1, false)]);
        let mut log = reopen(&library.fs, WRITER);
        let mutations = library.fs.mutations();
        let collection = library.collect(&mut log, &[], 5);
        assert_eq!(collection.removed, Vec::<BlobId>::new());
        assert_eq!(collection.kept, 5);
        assert_eq!(library.fs.mutations(), mutations);
        assert_eq!(logged(&library.fs, WRITER), Vec::<Entry>::new());
    }

    #[test]
    fn collecting_a_blob_already_gone_still_logs_its_removal() {
        let mut library = Library::new();
        let gone = library.blob(b"gone", &[(WRITER, 1, false)]);
        block_on(library.fs.remove_file(&library.layout.blob(gone))).unwrap();
        let mut log = reopen(&library.fs, WRITER);
        assert_eq!(library.collect(&mut log, &[], 0).removed, [gone]);
        assert!(logged(&library.fs, WRITER)
            .iter()
            .any(|e| e.kind == Kind::BlobRemoved { blob: gone }));
    }
}
