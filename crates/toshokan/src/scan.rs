//! Finding the library's files and binding them to entities.
//!
//! A scan lists every file outside toshokan's root, fingerprints it, and binds it to
//! an entity: by the entity's `path` field first, then by its `content` blob, which
//! follows a file renamed outside the app. A file is hashed only when its length and
//! modification time, against those toshokan recorded in the entity's `length` and
//! `modified` fields, cannot decide. A scan writes nothing, and what it finds is never
//! logged.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::fs::{hash_file, FileKind, Fingerprint, Fs, RelPath, Sameness};
use crate::ids::EntityId;
use crate::layout::Layout;
use crate::merge::State;
use crate::value::{BlobId, Value};
use crate::{CONTENT_FIELD, LENGTH_FIELD, MODIFIED_FIELD, PATH_FIELD};

/// An entity whose file was found at another path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Move {
    pub entity: EntityId,
    pub from: RelPath,
    pub to: RelPath,
}

/// What a scan found, each list in order.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    /// Every library file with its fingerprint, hashed where the scan needed its
    /// contents.
    pub files: BTreeMap<RelPath, Fingerprint>,
    /// The entity each bound file belongs to.
    pub bound: BTreeMap<RelPath, EntityId>,
    /// Paths more than one existing entity names, with every such entity. The path is
    /// bound to the first.
    pub contested: BTreeMap<RelPath, Vec<EntityId>>,
    /// Files no entity claims.
    pub arrivals: Vec<RelPath>,
    /// Existing entities with a `path` whose file was found nowhere. An entity whose
    /// contents match those of another missing entity, or of several unclaimed files,
    /// departs rather than be bound by a guess.
    pub departures: Vec<EntityId>,
    /// Entities whose file was found at another path.
    pub moves: Vec<Move>,
    /// Entities whose file is at its path with contents other than toshokan last
    /// wrote or bound.
    pub changed: Vec<EntityId>,
}

/// Scan the library for the entities of `state`.
///
/// `previous` is an earlier scan of the same library, or [`Scan::default`]. A file
/// whose length and modification time match its fingerprint there keeps that
/// fingerprint's hash instead of being read again.
pub async fn scan<F: Fs>(fs: &F, layout: &Layout, state: &State, previous: &Scan) -> Result<Scan> {
    let (claims, unplaced) = claims(state);
    let mut scan = survey(fs, layout, &claims, previous).await?;
    scan.departures.extend(unplaced);
    scan.departures.sort();
    Ok(scan)
}

/// An existing entity's claim on a library file.
struct Claim {
    entity: EntityId,
    path: RelPath,
    content: Option<BlobId>,
    /// The file's length and modification time as toshokan last wrote or bound it.
    len: Option<u64>,
    modified: Option<u64>,
}

/// The claims of every existing entity with a `path`, and the entities whose `path`
/// is not a library path.
fn claims(state: &State) -> (Vec<Claim>, Vec<EntityId>) {
    let mut claims = Vec::new();
    let mut unplaced = Vec::new();
    for entity in state.entities() {
        let Some(path) = state.field(entity, PATH_FIELD) else {
            continue;
        };
        let Some(path) = text_path(path) else {
            unplaced.push(entity);
            continue;
        };
        let count = |name| match state.field(entity, name) {
            Some(Value::Int(n)) => u64::try_from(*n).ok(),
            _ => None,
        };
        claims.push(Claim {
            entity,
            path,
            content: state.field(entity, CONTENT_FIELD).and_then(Value::as_blob),
            len: count(LENGTH_FIELD),
            modified: count(MODIFIED_FIELD),
        });
    }
    (claims, unplaced)
}

fn text_path(value: &Value) -> Option<RelPath> {
    match value {
        Value::Text(text) => RelPath::new(text).ok(),
        Value::Int(_) | Value::Bool(_) | Value::Ref(_) | Value::Blob(_) => None,
    }
}

async fn survey<F: Fs>(fs: &F, layout: &Layout, claims: &[Claim], previous: &Scan) -> Result<Scan> {
    let mut scan = Scan {
        files: list_files(fs, layout).await?,
        ..Scan::default()
    };
    for (path, print) in &mut scan.files {
        if let Some(earlier) = previous.files.get(path) {
            if earlier.hash.is_some() && print.compare(earlier) == Sameness::Same {
                print.hash = earlier.hash;
            }
        }
    }
    let missing = bind_paths(fs, &mut scan, claims).await?;
    follow_contents(fs, &mut scan, missing).await?;
    scan.arrivals = scan
        .files
        .keys()
        .filter(|path| !scan.bound.contains_key(*path))
        .cloned()
        .collect();
    scan.departures.sort();
    scan.moves.sort_by_key(|moved| moved.entity);
    scan.changed.sort();
    Ok(scan)
}

async fn list_files<F: Fs>(fs: &F, layout: &Layout) -> Result<BTreeMap<RelPath, Fingerprint>> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![RelPath::ROOT];
    while let Some(dir) = dirs.pop() {
        for entry in fs.list(&dir).await? {
            let path = dir.join(&entry.name)?;
            if layout.owns(&path) {
                continue;
            }
            match entry.kind {
                FileKind::Directory => dirs.push(path),
                FileKind::File => {
                    if let Some(metadata) = fs.metadata(&path).await? {
                        files.insert(path, Fingerprint::of(&metadata));
                    }
                }
            }
        }
    }
    Ok(files)
}

/// Bind each file an entity's `path` names, and return the claims whose file is
/// missing.
async fn bind_paths<'c, F: Fs>(
    fs: &F,
    scan: &mut Scan,
    claims: &'c [Claim],
) -> Result<Vec<&'c Claim>> {
    let mut by_path: BTreeMap<&RelPath, Vec<&Claim>> = BTreeMap::new();
    for claim in claims {
        by_path.entry(&claim.path).or_default().push(claim);
    }
    let mut missing = Vec::new();
    for (path, mut claimants) in by_path {
        let Some(print) = scan.files.get_mut(path) else {
            missing.extend(claimants);
            continue;
        };
        claimants.sort_by_key(|claim| claim.entity);
        let first = claimants[0];
        scan.bound.insert(path.clone(), first.entity);
        if claimants.len() > 1 {
            let entities = claimants.iter().map(|claim| claim.entity).collect();
            scan.contested.insert(path.clone(), entities);
        }
        if let Some(blob) = first.content {
            if !holds(fs, path, print, blob, first).await? {
                scan.changed.push(first.entity);
            }
        }
    }
    Ok(missing)
}

/// Bind each missing entity to the one unclaimed file holding its contents, when
/// exactly one missing entity and one such file share them.
async fn follow_contents<F: Fs>(fs: &F, scan: &mut Scan, missing: Vec<&Claim>) -> Result<()> {
    let mut wanted: BTreeMap<BlobId, Vec<&Claim>> = BTreeMap::new();
    for claim in missing {
        match claim.content {
            Some(blob) => wanted.entry(blob).or_default().push(claim),
            None => scan.departures.push(claim.entity),
        }
    }
    let might_match = |len: u64| {
        wanted
            .values()
            .flatten()
            .any(|claim| claim.len.is_none_or(|wanted| wanted == len))
    };
    let mut found: BTreeMap<BlobId, Vec<&RelPath>> = BTreeMap::new();
    for (path, print) in &mut scan.files {
        if scan.bound.contains_key(path) || !might_match(print.len) {
            continue;
        }
        let hash = match print.hash {
            Some(hash) => hash,
            None => hashed(fs, path, print).await?,
        };
        if wanted.contains_key(&hash) {
            found.entry(hash).or_default().push(path);
        }
    }
    for (blob, claimants) in &wanted {
        match (claimants.as_slice(), found.get(blob).map(Vec::as_slice)) {
            ([claim], Some([to])) => {
                scan.moves.push(Move {
                    entity: claim.entity,
                    from: claim.path.clone(),
                    to: (*to).clone(),
                });
            }
            _ => scan
                .departures
                .extend(claimants.iter().map(|claim| claim.entity)),
        }
    }
    for moved in &scan.moves {
        scan.bound.insert(moved.to.clone(), moved.entity);
    }
    Ok(())
}

/// Whether the file at `path` holds `blob`, hashing it only when its length and
/// modification time, against the claim's, cannot decide.
async fn holds<F: Fs>(
    fs: &F,
    path: &RelPath,
    print: &mut Fingerprint,
    blob: BlobId,
    claim: &Claim,
) -> Result<bool> {
    if let Some(hash) = print.hash {
        return Ok(hash == blob);
    }
    let recorded = claim.len.map(|len| Fingerprint {
        len,
        modified: claim.modified,
        hash: None,
    });
    match recorded.map(|recorded| recorded.compare(print)) {
        Some(Sameness::Same) => Ok(true),
        Some(Sameness::Different) => Ok(false),
        Some(Sameness::Unknown) | None => Ok(hashed(fs, path, print).await? == blob),
    }
}

async fn hashed<F: Fs>(fs: &F, path: &RelPath, print: &mut Fingerprint) -> Result<BlobId> {
    let (hash, len) = hash_file(fs, path).await?;
    print.hash = Some(hash);
    print.len = len;
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

    use super::*;
    use crate::fs::MemFs;
    use crate::ids::WriterId;

    const WRITER: WriterId = WriterId::from_u128(1);

    fn entity(counter: u64) -> EntityId {
        EntityId::new(WRITER, counter)
    }

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn paths(texts: &[&str]) -> Vec<RelPath> {
        texts.iter().map(|text| path(text)).collect()
    }

    /// Entity `counter` at `at`, last written or bound holding `bytes`, at a time the
    /// scan does not know.
    fn claim(counter: u64, at: &str, bytes: &[u8]) -> Claim {
        Claim {
            entity: entity(counter),
            path: path(at),
            content: Some(BlobId::of(bytes)),
            len: Some(bytes.len() as u64),
            modified: None,
        }
    }

    fn library(files: &[(&str, &[u8])]) -> MemFs {
        let fs = MemFs::new();
        for (name, bytes) in files {
            let file = path(name);
            block_on(fs.create_dir_all(&file.parent().unwrap())).unwrap();
            block_on(fs.create(&file, bytes)).unwrap();
        }
        fs
    }

    fn run(fs: &impl Fs, claims: &[Claim]) -> Scan {
        block_on(survey(fs, &Layout::default(), claims, &Scan::default())).unwrap()
    }

    fn bound(pairs: &[(&str, u64)]) -> BTreeMap<RelPath, EntityId> {
        pairs
            .iter()
            .map(|&(at, counter)| (path(at), entity(counter)))
            .collect()
    }

    #[test]
    fn an_untouched_file_stays_bound_to_its_entity() {
        let fs = library(&[("a/x", b"one")]);
        let scan = run(&fs, &[claim(1, "a/x", b"one")]);
        assert_eq!(scan.bound, bound(&[("a/x", 1)]));
        assert!(
            scan.arrivals.is_empty() && scan.departures.is_empty(),
            "{scan:?}"
        );
        assert!(scan.moves.is_empty() && scan.changed.is_empty(), "{scan:?}");
    }

    #[test]
    fn a_file_renamed_outside_the_app_is_followed() {
        let fs = library(&[("a/x", b"one"), ("other", b"two")]);
        block_on(fs.create_dir_all(&path("c"))).unwrap();
        block_on(fs.rename(&path("a/x"), &path("c/y"))).unwrap();
        let scan = run(&fs, &[claim(1, "a/x", b"one")]);
        let moved = Move {
            entity: entity(1),
            from: path("a/x"),
            to: path("c/y"),
        };
        assert_eq!(scan.moves, [moved]);
        assert_eq!(scan.bound, bound(&[("c/y", 1)]));
        assert_eq!(scan.arrivals, paths(&["other"]));
        assert!(
            scan.departures.is_empty() && scan.changed.is_empty(),
            "{scan:?}"
        );
    }

    #[test]
    fn a_file_replaced_under_its_name_is_a_change_not_a_move() {
        let fs = library(&[("a", b"one")]);
        block_on(fs.rename(&path("a"), &path("aside"))).unwrap();
        block_on(fs.create(&path("a"), b"two")).unwrap();
        let scan = run(&fs, &[claim(1, "a", b"one")]);
        assert_eq!(scan.changed, [entity(1)]);
        assert_eq!(scan.bound, bound(&[("a", 1)]));
        assert_eq!(scan.arrivals, paths(&["aside"]));
        assert!(
            scan.moves.is_empty() && scan.departures.is_empty(),
            "{scan:?}"
        );
    }

    #[test]
    fn identical_files_keep_their_own_entities() {
        let fs = library(&[("x", b"same"), ("y", b"same"), ("copy", b"same")]);
        let scan = run(&fs, &[claim(1, "x", b"same"), claim(2, "y", b"same")]);
        assert_eq!(scan.bound, bound(&[("x", 1), ("y", 2)]));
        assert_eq!(scan.arrivals, paths(&["copy"]));
        assert!(scan.moves.is_empty() && scan.changed.is_empty(), "{scan:?}");
    }

    #[test]
    fn identical_contents_found_more_than_once_depart_rather_than_be_guessed() {
        let fs = library(&[("p", b"same"), ("q", b"same")]);
        let scan = run(&fs, &[claim(1, "x", b"same"), claim(2, "y", b"same")]);
        assert_eq!(scan.departures, [entity(1), entity(2)]);
        assert_eq!(scan.arrivals, paths(&["p", "q"]));
        assert!(scan.moves.is_empty() && scan.bound.is_empty(), "{scan:?}");

        let scan = run(&fs, &[claim(1, "x", b"same")]);
        assert_eq!(scan.departures, [entity(1)]);
        assert_eq!(scan.arrivals, paths(&["p", "q"]));
        assert!(scan.moves.is_empty(), "{scan:?}");
    }

    #[test]
    fn a_missing_file_departs_and_an_unclaimed_file_arrives() {
        let fs = library(&[("new", b"fresh")]);
        let unknown = Claim {
            content: None,
            len: None,
            ..claim(2, "unknown", b"")
        };
        let scan = run(&fs, &[claim(1, "gone", b"lost"), unknown]);
        assert_eq!(scan.departures, [entity(1), entity(2)]);
        assert_eq!(scan.arrivals, paths(&["new"]));
        assert!(scan.bound.is_empty() && scan.moves.is_empty(), "{scan:?}");
    }

    #[test]
    fn entities_naming_one_path_are_reported_and_the_first_is_bound() {
        let fs = library(&[("a", b"one")]);
        let scan = run(&fs, &[claim(2, "a", b"one"), claim(1, "a", b"one")]);
        assert_eq!(scan.bound, bound(&[("a", 1)]));
        assert_eq!(
            scan.contested,
            BTreeMap::from([(path("a"), vec![entity(1), entity(2)])])
        );
    }

    #[test]
    fn toshokan_files_are_never_listed() {
        let fs = library(&[
            (".toshokan/blobs/b", b""),
            (".toshokan/tmp/w/t", b""),
            (".toshokanx", b""),
            ("a/.toshokan/f", b""),
        ]);
        let listed = |layout: &Layout| {
            let scan = block_on(survey(&fs, layout, &[], &Scan::default())).unwrap();
            scan.files.into_keys().collect::<Vec<_>>()
        };
        assert_eq!(
            listed(&Layout::default()),
            paths(&[".toshokanx", "a/.toshokan/f"])
        );
        let nested = Layout::new(path("a/.toshokan")).unwrap();
        assert_eq!(
            listed(&nested),
            paths(&[".toshokan/blobs/b", ".toshokan/tmp/w/t", ".toshokanx"])
        );
    }

    #[test]
    fn a_scan_writes_nothing() {
        let fs = library(&[("a", b"one"), ("b", b"two"), ("c", b"three")]);
        let before = (fs.files(), fs.directories(), fs.mutations());
        let claims = [claim(1, "a", b"changed"), claim(2, "moved", b"two")];
        let scan = run(&fs, &claims);
        assert_eq!((scan.changed.len(), scan.moves.len()), (1, 1), "{scan:?}");
        assert_eq!((fs.files(), fs.directories(), fs.mutations()), before);
    }

    #[test]
    fn a_file_unchanged_in_length_and_time_is_not_read_again() {
        let fs = library(&[("a", b"one")]);
        let claims = [claim(1, "a", b"one")];
        let mut previous = run(&fs, &claims);
        let print = previous.files.get_mut(&path("a")).unwrap();
        print.hash = Some(BlobId::of(b"two"));
        let modified = print.modified.unwrap();
        let rescan =
            |previous: &Scan| block_on(survey(&fs, &Layout::default(), &claims, previous)).unwrap();
        assert_eq!(
            rescan(&previous).changed,
            [entity(1)],
            "the earlier hash was trusted"
        );
        fs.set_modified(&path("a"), modified + 1).unwrap();
        assert!(
            rescan(&previous).changed.is_empty(),
            "a new time is read again"
        );
    }

    #[test]
    fn a_file_with_the_length_and_time_toshokan_recorded_is_not_read() {
        let fs = library(&[("a", b"one"), ("b", b"two")]);
        let time = |at: &str| block_on(fs.metadata(&path(at))).unwrap().unwrap().modified;
        let recorded = |counter, at: &str, bytes: &[u8]| Claim {
            modified: time(at),
            ..claim(counter, at, bytes)
        };
        let claims = [recorded(1, "a", b"one"), recorded(2, "b", b"owt")];
        let scan = run(&fs, &claims);
        assert_eq!(scan.changed, [], "the recorded time is trusted");
        for (at, print) in &scan.files {
            assert_eq!(print.hash, None, "{at} was hashed");
        }
        fs.set_modified(&path("b"), time("b").unwrap() + 1).unwrap();
        assert_eq!(run(&fs, &claims).changed, [entity(2)], "a new time is read");
    }

    #[test]
    fn a_length_that_differs_decides_without_reading() {
        let fs = library(&[("a", b"four"), ("b", b"else")]);
        let scan = run(&fs, &[claim(1, "a", b"one"), claim(2, "gone", b"two")]);
        assert_eq!(scan.changed, [entity(1)]);
        assert_eq!(scan.departures, [entity(2)]);
        for (at, print) in &scan.files {
            assert_eq!(print.hash, None, "{at} was hashed");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_scan_of_a_native_folder_follows_a_rename_and_writes_nothing() {
        use std::path::Path;

        use crate::native::NativeFs;

        fn tree(dir: &Path) -> BTreeMap<String, (Option<Vec<u8>>, std::time::SystemTime)> {
            let mut found = BTreeMap::new();
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                let name = entry.path().display().to_string();
                let bytes = metadata
                    .is_file()
                    .then(|| std::fs::read(entry.path()).unwrap());
                found.insert(name, (bytes, metadata.modified().unwrap()));
                if metadata.is_dir() {
                    found.extend(tree(&entry.path()));
                }
            }
            found
        }

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (name, bytes) in [("a/x", b"one".as_slice()), (".toshokan/blobs/b", b"one")] {
            let file = root.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        std::fs::rename(root.join("a/x"), root.join("y")).unwrap();
        let before = tree(root);
        let scan = run(&NativeFs::new(root), &[claim(1, "a/x", b"one")]);
        assert_eq!(tree(root), before);
        assert_eq!(
            scan.files.keys().cloned().collect::<Vec<_>>(),
            paths(&["y"])
        );
        assert_eq!(
            scan.moves,
            [Move {
                entity: entity(1),
                from: path("a/x"),
                to: path("y"),
            }]
        );
    }
}
