//! Binding files to entities.
//!
//! A binding is a pure function of the logged facts, the files a reader sees and
//! the app's cheap identity: by path, then by identity, with the copy rule. When an
//! entity's path is gone, a file holding its identity is a move; while the path
//! still holds it, such a file is a copy, a new file without an entity until an
//! intent says something about it. When several files could be the one, or one
//! file could be several entities', nothing is bound and it is reported. Scans
//! never write; every commit pins the moves this writer found.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::env::{Identify, Names};
use crate::error::Result;
use crate::flow::{self, fold, ok};
use crate::ids::{EntityId, Identity};
use crate::io::{Kind, Meta, Root, Task};
use crate::layout::Layout;
use crate::log::{FileFact, Op};
use crate::path::RelPath;
use crate::report::{Ambiguous, Copied, Moved, ScanReport};
use crate::schema::Written;
use crate::view::{FileRef, FileState};

/// Every entity's surviving file-register writes, as the merge gives them. More
/// than one is a conflict.
pub type Facts = BTreeMap<EntityId, Vec<Written<FileFact>>>;

/// One library file as a scan found it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Scanned {
    pub len: u64,
    pub modified: Option<u64>,
    /// Read only where length and time could not decide: when the length is that
    /// of a file some entity holds, and no earlier scan or fact gives the identity
    /// for this length and time.
    pub identity: Option<Identity>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    pub files: BTreeMap<RelPath, Scanned>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Bindings {
    /// Every entity with a file fact, with where its file is.
    pub bound: BTreeMap<EntityId, FileRef>,
    /// Library files no entity is bound to, sorted.
    pub unbound: Vec<RelPath>,
    pub report: ScanReport,
}

/// One identity as `identities.json` keeps it: of the file at a path, length and
/// time.
type Remembered = (RelPath, u64, Option<u64>, Identity);

impl Scan {
    /// The identities this scan holds, as `identities.json` keeps them.
    pub fn identities(&self) -> Vec<u8> {
        let rows: Vec<Remembered> = self
            .files
            .iter()
            .filter_map(|(path, file)| {
                Some((path.clone(), file.len, file.modified, file.identity?))
            })
            .collect();
        serde_json::to_vec(&rows).expect("identities are JSON")
    }

    /// The files whose identities `identities.json` keeps; `None` when `bytes` are
    /// not such a file.
    pub fn of_identities(bytes: &[u8]) -> Option<Self> {
        let rows: Vec<Remembered> = serde_json::from_slice(bytes).ok()?;
        let files = rows.into_iter().map(|(path, len, modified, identity)| {
            let file = Scanned {
                len,
                modified,
                identity: Some(identity),
            };
            (path, file)
        });
        Some(Self {
            files: files.collect(),
        })
    }
}

/// Lists every library file outside toshokan's root, with the paths whose
/// identities it read. Requests only [`crate::Io::List`], [`crate::Io::Stat`] and
/// [`crate::Io::Read`].
pub fn scan(
    layout: &Layout,
    identify: &Rc<dyn Identify>,
    facts: &Facts,
    previous: &Scan,
) -> Task<'static, Result<(Scan, Vec<RelPath>)>> {
    let known = Known::of(facts, previous, |_| true);
    let identify = Rc::clone(identify);
    walk(Rc::new(layout.clone()), RelPath::ROOT, Scan::default())
        .and_then(move |scan| identified(scan, known, identify))
        .task()
}

/// `previous` with each of `paths` scanned again: the file at a path, the files
/// under a directory, or nothing; with the paths whose identities it read.
/// Requests only [`crate::Io::List`], [`crate::Io::Stat`] and [`crate::Io::Read`].
pub fn rescan(
    layout: &Layout,
    identify: &Rc<dyn Identify>,
    facts: &Facts,
    previous: &Scan,
    paths: Vec<RelPath>,
) -> Task<'static, Result<(Scan, Vec<RelPath>)>> {
    let touched: BTreeSet<&str> = paths.iter().map(RelPath::as_str).collect();
    let mut scan = previous.clone();
    scan.files.retain(|path, _| !under(path, &touched));
    let unread = |path: &RelPath| {
        under(path, &touched) || scan.files.get(path).is_some_and(|f| f.identity.is_none())
    };
    let known = Known::of(facts, previous, unread);
    let identify = Rc::clone(identify);
    let layout = Rc::new(layout.clone());
    fold(paths.into_iter(), scan, move |scan, path| {
        visit(Rc::clone(&layout), path, scan)
    })
    .and_then(move |scan| identified(scan, known, identify))
    .task()
}

/// Whether `path` is one of `paths` or under one of them.
fn under(path: &RelPath, paths: &BTreeSet<&str>) -> bool {
    let text = path.as_str();
    paths.contains(text)
        || text
            .match_indices('/')
            .any(|(at, _)| paths.contains(&text[..at]))
}

/// What a scan knows without reading: the lengths of the files facts name, and
/// the identity of a file at a path, length and time, from a fact or an earlier
/// scan.
struct Known {
    lengths: BTreeSet<u64>,
    identities: BTreeMap<(RelPath, u64, Option<u64>), Identity>,
}

impl Known {
    /// Keeps identities only for paths `wanted` takes.
    fn of(facts: &Facts, previous: &Scan, wanted: impl Fn(&RelPath) -> bool) -> Self {
        let facts = facts.values().flatten().map(|fact| &fact.value);
        let lengths = facts.clone().map(|fact| fact.len).collect();
        let logged = facts
            .filter(|fact| wanted(&fact.path))
            .map(|fact| ((fact.path.clone(), fact.len, fact.modified), fact.identity));
        let read = previous.files.iter().filter(|(path, _)| wanted(path));
        let read = read.filter_map(|(path, file)| {
            let identity = file.identity?;
            Some(((path.clone(), file.len, file.modified), identity))
        });
        Self {
            lengths,
            identities: logged.chain(read).collect(),
        }
    }
}

/// `scan` with the identities `known` gives, and those it must read, of each file
/// whose length is a fact's; with the paths whose identities it read.
fn identified<'a>(
    mut scan: Scan,
    known: Known,
    identify: Rc<dyn Identify>,
) -> flow::Fallible<'a, (Scan, Vec<RelPath>)> {
    let unknown: Vec<(RelPath, u64)> = scan
        .files
        .iter_mut()
        .filter(|(_, file)| file.identity.is_none())
        .filter_map(|(path, file)| {
            let key = (path.clone(), file.len, file.modified);
            file.identity = known.identities.get(&key).copied();
            let needed = file.identity.is_none() && known.lengths.contains(&file.len);
            needed.then(|| (path.clone(), file.len))
        })
        .collect();
    let read = unknown.iter().map(|(path, _)| path.clone()).collect();
    let identified = fold(unknown.into_iter(), scan, move |mut scan, (path, len)| {
        flow::identity(Root::Folder, path.clone(), len, &identify).map_ok(move |identity| {
            if let Some(file) = scan.files.get_mut(&path) {
                file.identity = Some(identity);
            }
            scan
        })
    });
    identified.map_ok(move |scan| (scan, read))
}

/// `scan` with what is at `path`: a file, every file under a directory, or
/// nothing.
fn visit<'a>(layout: Rc<Layout>, path: RelPath, scan: Scan) -> flow::Fallible<'a, Scan> {
    if layout.owns(&path) {
        return ok(scan);
    }
    flow::stat(Root::Folder, &path).and_then(move |meta| match meta {
        Some(meta) if meta.kind == Kind::Directory => walk(layout, path, scan),
        Some(meta) => ok(found(scan, path, meta)),
        None => ok(scan),
    })
}

fn walk<'a>(layout: Rc<Layout>, dir: RelPath, scan: Scan) -> flow::Fallible<'a, Scan> {
    flow::list(Root::Folder, &dir).and_then(move |entries| {
        fold(entries.into_iter(), scan, move |scan, entry| {
            let path = dir
                .join(&entry.name)
                .expect("a listed name is one component");
            match entry.kind {
                Kind::Directory if layout.owns(&path) => ok(scan),
                Kind::Directory => walk(Rc::clone(&layout), path, scan),
                Kind::File => flow::stat(Root::Folder, &path).map_ok(move |meta| match meta {
                    Some(meta) => found(scan, path, meta),
                    None => scan,
                }),
            }
        })
    })
}

/// `scan` with the file `meta` describes at `path`; unchanged unless it is a file.
fn found(mut scan: Scan, path: RelPath, meta: Meta) -> Scan {
    if meta.kind == Kind::File {
        let file = Scanned {
            len: meta.len,
            modified: meta.modified,
            identity: None,
        };
        scan.files.insert(path, file);
    }
    scan
}

/// Whether `file` holds what `fact` says.
fn holds(fact: &FileFact, file: &Scanned) -> bool {
    file.len == fact.len
        && match file.identity {
            Some(identity) => identity == fact.identity,
            None => file.modified == fact.modified,
        }
}

/// Binds every entity with a file fact to a file of `scan`, or to none.
pub fn bind(facts: &Facts, scan: &Scan, names: &dyn Names) -> Bindings {
    let mut bindings = Bindings::default();
    let mut taken: BTreeMap<&RelPath, EntityId> = BTreeMap::new();
    let mut departed: Vec<(EntityId, &FileFact)> = Vec::new();
    let mut claims: BTreeMap<&RelPath, Vec<(EntityId, &FileFact)>> = BTreeMap::new();
    let by_key = OnceCell::new();
    let by_key = || {
        by_key.get_or_init(|| {
            let mut by_key: BTreeMap<String, Vec<&RelPath>> = BTreeMap::new();
            for path in scan.files.keys() {
                let key = names.key(path.as_str());
                by_key.entry(key).or_default().push(path);
            }
            by_key
        })
    };

    for (&entity, written) in facts {
        let Some(first) = written.first() else {
            continue;
        };
        let mut candidates: Vec<(&RelPath, &FileFact)> = Vec::new();
        for fact in written.iter().map(|w| &w.value) {
            let same = match scan.files.get_key_value(&fact.path) {
                Some((path, _)) => vec![path],
                None => by_key()
                    .get(&names.key(fact.path.as_str()))
                    .cloned()
                    .unwrap_or_default(),
            };
            candidates.extend(same.into_iter().map(|path| (path, fact)));
        }
        candidates.sort_by_key(|(path, _)| *path);
        candidates.dedup_by_key(|(path, _)| *path);
        match candidates.as_slice() {
            [] => departed.push((entity, &first.value)),
            [(path, fact)] => claims.entry(path).or_default().push((entity, fact)),
            several => {
                bindings.report.ambiguous.push(Ambiguous {
                    entity,
                    candidates: several.iter().map(|(path, _)| (*path).clone()).collect(),
                });
                bindings.bound.insert(entity, missing(&first.value));
            }
        }
    }

    // Several entities naming one file: the one whose identity it holds, if only
    // one does, has it. The others, or all when that does not decide, are bound
    // by identity like entities whose path is gone.
    for (path, claimants) in claims {
        let file = &scan.files[path];
        let holders: Vec<&(EntityId, &FileFact)> = claimants
            .iter()
            .filter(|(_, fact)| holds(fact, file))
            .collect();
        let (entity, state) = match (claimants.as_slice(), holders.as_slice()) {
            ([(entity, fact)], _) => (
                *entity,
                if holds(fact, file) {
                    FileState::InSync
                } else {
                    FileState::ChangedOutside
                },
            ),
            (_, [(entity, _)]) => (*entity, FileState::InSync),
            _ => {
                departed.extend(claimants);
                continue;
            }
        };
        departed.extend(
            claimants
                .iter()
                .filter(|(other, _)| *other != entity)
                .copied(),
        );
        taken.insert(path, entity);
        bindings.bound.insert(
            entity,
            FileRef {
                path: path.clone(),
                state,
            },
        );
    }

    let mut free: BTreeMap<(Identity, u64), Vec<&RelPath>> = BTreeMap::new();
    for (path, file) in &scan.files {
        if let (Some(identity), false) = (file.identity, taken.contains_key(path)) {
            free.entry((identity, file.len)).or_default().push(path);
        }
    }
    let mut claims: BTreeMap<&RelPath, Vec<EntityId>> = BTreeMap::new();
    let mut moves: Vec<(EntityId, &FileFact, &[&RelPath])> = Vec::new();
    for (entity, fact) in departed {
        let matches = free
            .get(&(fact.identity, fact.len))
            .map_or(&[][..], Vec::as_slice);
        for path in matches {
            claims.entry(path).or_default().push(entity);
        }
        moves.push((entity, fact, matches));
    }
    for (entity, fact, matches) in moves {
        match matches {
            [path] if claims[path].len() == 1 => {
                taken.insert(path, entity);
                bindings.report.moved.push(Moved {
                    entity,
                    from: fact.path.clone(),
                    to: (*path).clone(),
                });
                bindings.bound.insert(
                    entity,
                    FileRef {
                        path: (*path).clone(),
                        state: FileState::InSync,
                    },
                );
            }
            [] => {
                bindings.report.departed.push(entity);
                bindings.bound.insert(entity, missing(fact));
            }
            several => {
                bindings.report.ambiguous.push(Ambiguous {
                    entity,
                    candidates: several.iter().map(|path| (*path).clone()).collect(),
                });
                bindings.bound.insert(entity, missing(fact));
            }
        }
    }

    let in_sync: BTreeMap<Identity, Vec<EntityId>> = bindings
        .bound
        .iter()
        .filter(|(_, file)| file.state == FileState::InSync)
        .filter_map(|(&entity, file)| Some((scan.files.get(&file.path)?.identity?, entity)))
        .fold(BTreeMap::new(), |mut by, (identity, entity)| {
            by.entry(identity).or_insert_with(Vec::new).push(entity);
            by
        });
    for (path, file) in &scan.files {
        if taken.contains_key(path) {
            continue;
        }
        bindings.unbound.push(path.clone());
        let copied = file.identity.and_then(|identity| in_sync.get(&identity));
        for &entity in copied.into_iter().flatten() {
            bindings.report.copied.push(Copied {
                entity,
                copy: path.clone(),
            });
        }
    }
    bindings.report.arrived = bindings.unbound.clone();
    bindings.report.changed = bindings
        .bound
        .iter()
        .filter(|(_, file)| file.state == FileState::ChangedOutside)
        .map(|(&entity, _)| entity)
        .collect();
    bindings
}

fn missing(fact: &FileFact) -> FileRef {
    FileRef {
        path: fact.path.clone(),
        state: FileState::Missing,
    }
}

/// File-register writes for the moves `facts` do not already say: a file found in
/// sync at another path. A new modification time alone is not pinned; it only
/// saves reading an identity. Conflicted registers, changed files and missing
/// ones are left for the user.
pub fn pins(facts: &Facts, bindings: &Bindings, scan: &Scan) -> Vec<Op> {
    let mut ops = Vec::new();
    for (&entity, file) in &bindings.bound {
        let ([written], FileState::InSync) = (
            facts.get(&entity).map_or(&[][..], Vec::as_slice),
            file.state,
        ) else {
            continue;
        };
        let Some(found) = scan.files.get(&file.path) else {
            continue;
        };
        if file.path == written.value.path {
            continue;
        }
        let pinned = FileFact {
            path: file.path.clone(),
            identity: found.identity.unwrap_or(written.value.identity),
            len: found.len,
            modified: found.modified,
        };
        ops.push(Op::Pin {
            entity,
            file: pinned,
            replaces: vec![written.entry],
        });
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::MemDisk;
    use crate::env::{ExactNames, PrefixIdentity};
    use crate::ids::{EntryHash, Hlc, WriterId};
    use crate::io::Io;

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    /// Contents by identity: two of one length, one of another.
    const CONTENTS: [(u128, u64); 3] = [(1, 10), (2, 10), (3, 20)];

    fn fact(at: &str, content: usize) -> FileFact {
        let (identity, len) = CONTENTS[content];
        FileFact {
            path: path(at),
            identity: Identity::from_u128(identity),
            len,
            modified: Some(0),
        }
    }

    fn written(fact: FileFact, entry: u128) -> Written<FileFact> {
        Written {
            value: fact,
            by: WriterId::from_u128(1),
            at: Hlc::ZERO,
            entry: EntryHash::from_u128(entry),
        }
    }

    /// A scan as [`scan`] reports it: identities only for lengths some fact has.
    fn scanned(files: &[(&str, usize)], facts: &Facts) -> Scan {
        let lengths: BTreeSet<u64> = facts.values().flatten().map(|w| w.value.len).collect();
        let files = files
            .iter()
            .map(|&(at, content)| {
                let (identity, len) = CONTENTS[content];
                let file = Scanned {
                    len,
                    modified: Some(1),
                    identity: lengths
                        .contains(&len)
                        .then_some(Identity::from_u128(identity)),
                };
                (path(at), file)
            })
            .collect();
        Scan { files }
    }

    fn holding(scan: &Scan, fact: &FileFact) -> Vec<RelPath> {
        scan.files
            .iter()
            .filter(|(_, file)| file.identity == Some(fact.identity) && file.len == fact.len)
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// Every way to put up to two entities' files and three files in three places.
    fn worlds() -> Vec<(Facts, Scan)> {
        let places = ["a", "b", "c"];
        let mut facts_all: Vec<Facts> = vec![Facts::new()];
        for first in 0..6 {
            let one = fact(places[first / 2], first % 2);
            facts_all.push(Facts::from([(
                EntityId::from_u128(1),
                vec![written(one.clone(), 1)],
            )]));
            for second in 0..6 {
                let two = fact(places[second / 2], second % 2);
                facts_all.push(Facts::from([
                    (EntityId::from_u128(1), vec![written(one.clone(), 1)]),
                    (EntityId::from_u128(2), vec![written(two, 2)]),
                ]));
            }
        }
        let mut worlds = Vec::new();
        for facts in facts_all {
            for layout in 0..4u32.pow(3) {
                let files: Vec<(&str, usize)> = (0..3)
                    .filter_map(|i| match (layout / 4u32.pow(i)) % 4 {
                        0 => None,
                        content => Some((places[i as usize], content as usize - 1)),
                    })
                    .collect();
                let scan = scanned(&files, &facts);
                worlds.push((facts.clone(), scan));
            }
        }
        worlds
    }

    #[test]
    fn binding_never_guesses_in_any_small_world() {
        for (facts, scan) in worlds() {
            let shown = format!("{facts:?} {scan:?}");
            let bindings = bind(&facts, &scan, &ExactNames);
            let mut owners: BTreeMap<&RelPath, EntityId> = BTreeMap::new();
            for (entity, file) in &bindings.bound {
                let fact = &facts[entity][0].value;
                match file.state {
                    FileState::Missing => {
                        let alone = holding(&scan, fact);
                        let sole = facts
                            .values()
                            .filter(|w| w[0].value.path == fact.path)
                            .count()
                            == 1;
                        assert!(
                            !(sole && scan.files.contains_key(&fact.path)),
                            "{shown}: {entity} is missing though its path holds a file"
                        );
                        let rivals = facts
                            .iter()
                            .filter(|(other, w)| {
                                *other != entity && w[0].value.identity == fact.identity
                            })
                            .count();
                        assert!(
                            alone.len() != 1
                                || rivals > 0
                                || bindings
                                    .bound
                                    .values()
                                    .any(|f| f.path == alone[0] && f.state != FileState::Missing),
                            "{shown}: {entity} could only be {alone:?}"
                        );
                    }
                    FileState::InSync | FileState::ChangedOutside => {
                        assert!(
                            owners.insert(&file.path, *entity).is_none(),
                            "{shown}: {} bound twice",
                            file.path
                        );
                        let found = &scan.files[&file.path];
                        let holds = found.identity == Some(fact.identity) && found.len == fact.len;
                        assert_eq!(
                            file.state == FileState::InSync,
                            holds,
                            "{shown}: {entity} state"
                        );
                        if file.path != fact.path {
                            assert!(
                                holds,
                                "{shown}: {entity} moved to a file without its identity"
                            );
                            let shared = facts
                                .iter()
                                .any(|(other, w)| other != entity && w[0].value.path == fact.path);
                            assert!(
                                !scan.files.contains_key(&fact.path) || shared,
                                "{shown}: moved while its own path holds a file"
                            );
                            let candidates: Vec<RelPath> = holding(&scan, fact)
                                .into_iter()
                                .filter(|p| !facts.values().any(|w| w[0].value.path == *p))
                                .collect();
                            assert_eq!(
                                candidates,
                                std::slice::from_ref(&file.path),
                                "{shown}: {entity} moved among several"
                            );
                        }
                    }
                }
            }
            assert_eq!(
                bindings.bound.len(),
                facts.len(),
                "{shown}: an entity is unaccounted for"
            );
            let unbound: BTreeSet<&RelPath> = bindings.unbound.iter().collect();
            for path in scan.files.keys() {
                assert_ne!(
                    owners.contains_key(path),
                    unbound.contains(path),
                    "{shown}: {path}"
                );
            }
            for (entity, file) in &bindings.bound {
                if file.state != FileState::InSync {
                    continue;
                }
                let identity = scan.files[&file.path].identity;
                for copy in bindings
                    .unbound
                    .iter()
                    .filter(|p| scan.files[*p].identity == identity)
                {
                    let copied = Copied {
                        entity: *entity,
                        copy: copy.clone(),
                    };
                    assert!(
                        bindings.report.copied.contains(&copied),
                        "{shown}: {copy} is a copy"
                    );
                }
            }

            let swap = |id: &EntityId| EntityId::from_u128(3 - id.to_u128());
            let swapped: Facts = facts.iter().map(|(id, w)| (swap(id), w.clone())).collect();
            let by_path = |b: &Bindings,
                           rename: &dyn Fn(&EntityId) -> EntityId|
             -> BTreeMap<EntityId, FileRef> {
                b.bound
                    .iter()
                    .map(|(id, f)| (rename(id), f.clone()))
                    .collect()
            };
            assert_eq!(
                by_path(&bind(&swapped, &scan, &ExactNames), &swap),
                by_path(&bindings, &|id| *id),
                "{shown}: binding depends on entity ids"
            );
        }
    }

    /// Folds ASCII case and composes `e` with a combining acute accent.
    struct Folding;

    impl Names for Folding {
        fn key(&self, path: &str) -> String {
            path.replace("e\u{301}", "\u{e9}").to_lowercase()
        }
    }

    fn one(at: &str) -> Facts {
        Facts::from([(EntityId::from_u128(1), vec![written(fact(at, 0), 1)])])
    }

    #[test]
    fn names_the_volume_takes_for_one_bind_by_path() {
        let e = EntityId::from_u128(1);
        for (logged, found) in [("Song.syx", "song.syx"), ("Cafe\u{301}", "caf\u{e9}")] {
            let facts = one(logged);
            let scan = scanned(&[(found, 0)], &facts);
            let folded = bind(&facts, &scan, &Folding);
            assert_eq!(folded.bound[&e].path, path(found));
            assert!(
                folded.report.moved.is_empty(),
                "{logged:?} is the same name"
            );
            let exact = bind(&facts, &scan, &ExactNames);
            assert_eq!(
                exact.report.moved.len(),
                1,
                "{logged:?} differs byte for byte"
            );
        }
    }

    #[test]
    fn an_exact_name_wins_over_a_folded_one_and_two_claims_bind_neither() {
        let e = EntityId::from_u128(1);
        let facts = one("A");
        let scan = scanned(&[("A", 0), ("a", 0)], &facts);
        let bindings = bind(&facts, &scan, &Folding);
        assert_eq!(bindings.bound[&e].path, path("A"));
        assert_eq!(bindings.unbound, [path("a")]);

        let mut both = one("a");
        both.insert(EntityId::from_u128(2), vec![written(fact("A", 0), 2)]);
        let bindings = bind(&both, &scanned(&[("a", 0)], &both), &Folding);
        assert!(
            bindings
                .bound
                .values()
                .all(|f| f.state == FileState::Missing),
            "{bindings:?}"
        );
        assert_eq!(bindings.report.ambiguous.len(), 2);
    }

    #[test]
    fn a_conflicted_file_register_binds_only_where_one_survivor_is_found() {
        let e = EntityId::from_u128(1);
        let facts = Facts::from([(e, vec![written(fact("a", 0), 1), written(fact("b", 0), 2)])]);
        let found = |files: &[(&str, usize)]| bind(&facts, &scanned(files, &facts), &ExactNames);
        assert_eq!(found(&[("b", 0)]).bound[&e].path, path("b"));
        let both = found(&[("a", 0), ("b", 0)]);
        assert_eq!(both.bound[&e].state, FileState::Missing);
        assert_eq!(both.report.ambiguous[0].candidates, [path("a"), path("b")]);
        let scan = scanned(&[("b", 0)], &facts);
        assert!(
            pins(&facts, &bind(&facts, &scan, &ExactNames), &scan).is_empty(),
            "the user resolves it"
        );
    }

    #[test]
    fn a_pin_records_a_move_and_nothing_else() {
        let e = EntityId::from_u128(1);
        let facts = one("a");
        let pins_for = |files: &[(&str, usize)]| {
            let scan = scanned(files, &facts);
            pins(&facts, &bind(&facts, &scan, &ExactNames), &scan)
        };
        let [Op::Pin {
            entity,
            file: moved,
            replaces,
        }] = &pins_for(&[("b", 0)])[..]
        else {
            panic!("a move is pinned");
        };
        assert_eq!(
            (*entity, moved.path.as_str(), replaces.as_slice()),
            (e, "b", &[EntryHash::from_u128(1)][..])
        );
        assert!(pins_for(&[("a", 0)]).is_empty(), "a new time");
        assert!(pins_for(&[("a", 1)]).is_empty(), "changed outside");
        assert!(pins_for(&[]).is_empty(), "missing");
        let mut in_sync = one("a");
        in_sync.get_mut(&e).unwrap()[0].value.modified = Some(1);
        let scan = scanned(&[("a", 0)], &in_sync);
        assert!(pins(&in_sync, &bind(&in_sync, &scan, &ExactNames), &scan).is_empty());
    }

    #[test]
    fn a_scan_reads_identities_only_where_length_and_time_cannot_decide() {
        let disk = MemDisk::new();
        let layout = Layout::new(".t").unwrap();
        let identify: Rc<dyn Identify> = Rc::new(PrefixIdentity { prefix: 4 });
        for (at, bytes) in [
            ("d/a", &b"0123456789"[..]),
            ("b", b"01234567890123456789"),
            (".t/x", b"x"),
        ] {
            let file = path(at);
            disk.perform(Io::MakeDir {
                root: Root::Folder,
                path: file.parent().unwrap(),
            })
            .unwrap();
            disk.perform(Io::Create {
                root: Root::Folder,
                path: file,
                bytes: bytes.to_vec(),
            })
            .unwrap();
        }
        let facts = one("elsewhere");
        let run = |previous: &Scan| {
            crate::blocking::run(
                &mut disk.clone(),
                scan(&layout, &identify, &facts, previous),
            )
            .unwrap()
        };
        let (first, read) = run(&Scan::default());
        assert_eq!(
            first.files.keys().collect::<Vec<_>>(),
            [&path("b"), &path("d/a")]
        );
        assert_eq!(read, [path("d/a")]);
        assert!(
            first.files[&path("d/a")].identity.is_some(),
            "its length is a fact's"
        );
        assert!(first.files[&path("b")].identity.is_none());
        let mut previous = first.clone();
        let remembered = Identity::from_u128(42);
        previous.files.get_mut(&path("d/a")).unwrap().identity = Some(remembered);
        let (again, read) = run(&previous);
        assert_eq!(
            (again.files[&path("d/a")].identity, read),
            (Some(remembered), Vec::new()),
            "same length and time"
        );
    }
}
