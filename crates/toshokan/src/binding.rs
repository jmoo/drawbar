//! Binding files to entities.
//!
//! A binding is a pure function of the logged facts, the files a reader sees and
//! the app's cheap identity: by path, then by identity, with the copy rule. When an
//! entity's path is gone, a file holding its identity is a move; while the path
//! still holds it, such a file is a copy, a new file without an entity until an
//! intent says something about it. When several files could be the one, or one
//! file could be several entities', nothing is bound and it is reported. Scans
//! never write; every commit pins the moves this writer found.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Bound;
use std::rc::Rc;

use crate::cow::CowMap;
use crate::env::{Identify, Names};
use crate::error::Result;
use crate::flow::{self, fold, ok};
use crate::ids::{EntityId, Identity};
use crate::io::{Kind, Meta, Root, Task};
use crate::layout::{is_swap_file, Layout};
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
    pub bound: CowMap<EntityId, FileRef>,
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

/// What a failed scan left unknown: library paths and what is under them, and
/// the entities then bound to them. Kept until a scan of every file succeeds.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Unscanned {
    pub paths: BTreeSet<RelPath>,
    pub entities: BTreeSet<EntityId>,
}

impl Unscanned {
    /// Adds `paths`, and the entities `bindings` binds to them or under them.
    pub fn add(&mut self, paths: Vec<RelPath>, bindings: &Bindings) {
        let named: BTreeSet<&str> = paths.iter().map(RelPath::as_str).collect();
        let bound = bindings.bound.iter();
        let bound = bound.filter(|(_, file)| under(&file.path, &named));
        self.entities.extend(bound.map(|(entity, _)| *entity));
        self.paths.extend(paths);
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.entities.is_empty()
    }

    /// Whether `entity`, whose file facts are `written`, is left unknown; `paths`
    /// are those of `self`.
    fn leaves_unknown(
        &self,
        paths: &BTreeSet<&str>,
        entity: EntityId,
        written: &[Written<FileFact>],
    ) -> bool {
        self.entities.contains(&entity) || written.iter().any(|w| under(&w.value.path, paths))
    }
}

/// Binds as [`bind`] does, with the pins of what it binds, but blind to what
/// `unscanned` leaves unknown: the files under its paths are left out of `scan`,
/// and each of its entities, or of those a file fact of which names such a path,
/// is bound as [`FileState::Unscanned`] at its latest logged path and pinned
/// nowhere.
pub fn bind_known(
    facts: &Facts,
    scan: &Scan,
    names: &dyn Names,
    unscanned: &Unscanned,
) -> (Bindings, Vec<Op>) {
    let mut binding = Binding::new(facts, scan, unscanned);
    binding.step(facts, scan, names, usize::MAX);
    binding.finish()
}

/// Lists every library file outside toshokan's root, with the paths whose
/// identities it read. Requests only [`crate::Io::ListStat`], one per directory,
/// and [`crate::Io::ReadMany`].
pub fn scan(
    layout: &Layout,
    identify: &Rc<dyn Identify>,
    facts: &Facts,
    previous: &Scan,
) -> Task<'static, Result<(Scan, Vec<RelPath>)>> {
    let (facts, previous) = (facts.clone(), previous.clone());
    let identify = Rc::clone(identify);
    list_all(layout)
        .and_then(move |mut scan| {
            let unknown = resolve(&mut scan, &facts, &previous);
            identified(scan, unknown, &identify)
        })
        .task()
}

/// Every library file outside toshokan's root, without identities. Requests only
/// [`crate::Io::ListStat`], one per directory.
pub(crate) fn list_all<'a>(layout: &Layout) -> flow::Fallible<'a, Scan> {
    walk(Rc::new(layout.clone()), RelPath::ROOT, Scan::default())
}

/// A scan of every library file a directory at a time, which keeps what it listed
/// so that a scan that stopped goes on where it stopped.
#[derive(Clone, PartialEq, Debug)]
pub struct Walk {
    /// Each directory listed, with the files and the directories it held.
    listed: BTreeMap<RelPath, Vec<(String, Meta)>>,
    /// The directories to list.
    queue: BTreeSet<RelPath>,
}

impl Default for Walk {
    fn default() -> Self {
        Self {
            listed: BTreeMap::new(),
            queue: BTreeSet::from([RelPath::ROOT]),
        }
    }
}

impl Walk {
    /// The next directory to list; `None` once every directory is listed.
    pub fn next(&self) -> Option<RelPath> {
        self.queue.first().cloned()
    }

    /// Keeps what listing `dir` found, but toshokan's root and swap files, and
    /// forgets the listings of directories under `dir` it no longer holds.
    pub fn listed(&mut self, layout: &Layout, dir: RelPath, entries: Vec<(String, Meta)>) {
        let entries: Vec<(String, Meta)> = entries
            .into_iter()
            .filter(|(name, meta)| match meta.kind {
                Kind::Directory => dir.join(name).is_ok_and(|path| !layout.owns(&path)),
                Kind::File => !is_swap_file(name),
            })
            .collect();
        let dirs: BTreeSet<RelPath> = entries
            .iter()
            .filter(|(_, meta)| meta.kind == Kind::Directory)
            .filter_map(|(name, _)| dir.join(name).ok())
            .collect();
        let gone: Vec<RelPath> = subtree(&self.listed, &dir)
            .filter(|path| **path != dir && !dirs.iter().any(|kept| path.starts_with(kept)))
            .cloned()
            .collect();
        for path in gone {
            self.listed.remove(&path);
        }
        self.queue.remove(&dir);
        let unlisted: Vec<RelPath> = dirs
            .into_iter()
            .filter(|sub| !self.listed.contains_key(sub))
            .collect();
        self.queue.extend(unlisted);
        self.listed.insert(dir, entries);
    }

    /// Lists again what may have changed at `paths`: each one's directory, and each
    /// directory at or under it.
    pub fn forget(&mut self, paths: &[RelPath]) {
        for path in paths {
            let stale: Vec<RelPath> = subtree(&self.listed, path).cloned().collect();
            for dir in stale {
                self.listed.remove(&dir);
            }
            let queued: Vec<RelPath> = self
                .queue
                .iter()
                .filter(|dir| dir.starts_with(path))
                .cloned()
                .collect();
            for dir in queued {
                self.queue.remove(&dir);
            }
            if let Some(parent) = path.parent() {
                if self.listed.remove(&parent).is_some() {
                    self.queue.insert(parent);
                }
            }
        }
        if self.listed.is_empty() {
            self.queue.insert(RelPath::ROOT);
        }
    }

    /// Every file listed, its identity unread.
    /// Adds to `scan` the files of the directories listed after `after`, until it
    /// has added about `slice`. Returns the last directory taken, and whether
    /// none is left.
    pub fn found_after(
        &self,
        scan: &mut Scan,
        after: Option<&RelPath>,
        slice: usize,
    ) -> (Option<RelPath>, bool) {
        let from = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut dirs = self.listed.range::<RelPath, _>((from, Bound::Unbounded));
        let (mut added, mut last) = (0, None);
        for (dir, entries) in dirs.by_ref() {
            let files = entries.iter().filter(|(_, meta)| meta.kind == Kind::File);
            for (name, meta) in files {
                let Ok(path) = dir.join(name) else {
                    continue;
                };
                let file = Scanned {
                    len: meta.len,
                    modified: meta.modified,
                    identity: None,
                };
                scan.files.insert(path, file);
                added += 1;
            }
            last = Some(dir);
            if added >= slice {
                break;
            }
        }
        (last.cloned(), dirs.next().is_none())
    }
}

/// The keys of `map` at `dir` or under it.
fn subtree<'a, V>(
    map: &'a BTreeMap<RelPath, V>,
    dir: &'a RelPath,
) -> impl Iterator<Item = &'a RelPath> + 'a {
    let prefixed = map.range(dir.clone()..).map(|(path, _)| path);
    prefixed
        .take_while(move |path| path.as_str().starts_with(dir.as_str()))
        .filter(move |path| path.starts_with(dir))
}

/// `previous` with each of `paths` listed again, without identities: the file at
/// a path, the files under a directory, or nothing. Requests only
/// [`crate::Io::Stat`] and [`crate::Io::ListStat`].
pub(crate) fn relist<'a>(
    layout: &Layout,
    previous: &Scan,
    paths: Vec<RelPath>,
) -> flow::Fallible<'a, Scan> {
    let touched: BTreeSet<&str> = paths.iter().map(RelPath::as_str).collect();
    let mut scan = previous.clone();
    scan.files.retain(|path, _| !under(path, &touched));
    let layout = Rc::new(layout.clone());
    fold(paths.into_iter(), scan, move |scan, path| {
        visit(Rc::clone(&layout), path, scan)
    })
}

/// The files of `scan` at or under each of `paths`.
pub(crate) fn under_any(scan: &Scan, paths: &[RelPath]) -> Vec<RelPath> {
    let mut found: BTreeSet<&RelPath> = BTreeSet::new();
    for dir in paths {
        let from = scan.files.range::<RelPath, _>(dir..);
        let prefixed = from.take_while(|(path, _)| path.as_str().starts_with(dir.as_str()));
        found.extend(
            prefixed
                .map(|(path, _)| path)
                .filter(|path| path.starts_with(dir)),
        );
    }
    found.into_iter().cloned().collect()
}

/// Whether `path` is one of `paths` or under one of them.
fn under(path: &RelPath, paths: &BTreeSet<&str>) -> bool {
    let text = path.as_str();
    paths.contains(RelPath::ROOT.as_str())
        || paths.contains(text)
        || text
            .match_indices('/')
            .any(|(at, _)| paths.contains(&text[..at]))
}

/// Gives each file of `scan` without an identity the one known for its path,
/// length and time: from `previous`, an earlier scan, else from the last of
/// `facts` that names them. Returns the files left without one whose length is a
/// fact's, whose identities must be read.
pub(crate) fn resolve(scan: &mut Scan, facts: &Facts, previous: &Scan) -> Vec<(RelPath, u64)> {
    let mut resolving = Resolving::default();
    while !resolving.step(scan, facts, previous, usize::MAX) {}
    resolving.needed
}

/// [`resolve`] a slice at a time.
#[derive(Default)]
pub(crate) struct Resolving {
    stage: Resolve,
    /// The files no earlier scan gives an identity, in order.
    unknown: Vec<RelPath>,
    /// What the facts naming an unknown file say of it, in order of the facts.
    logged: HashMap<RelPath, Vec<(u64, Option<u64>, Identity)>>,
    lengths: HashSet<u64>,
    pub(crate) needed: Vec<(RelPath, u64)>,
}

#[derive(Default)]
enum Resolve {
    /// Taking identities from the earlier scan, after a path.
    #[default]
    Earlier,
    EarlierAfter(RelPath),
    /// Gathering what the facts say, after an entity.
    Facts(Option<EntityId>),
    /// Giving the unknown files from this one on what the facts say.
    Logged(usize),
    Done,
}

impl Resolving {
    /// Resolves about `slice` more files or entities; true once every file is
    /// resolved. Each step must be given the same facts and earlier scan.
    pub(crate) fn step(
        &mut self,
        scan: &mut Scan,
        facts: &Facts,
        previous: &Scan,
        slice: usize,
    ) -> bool {
        self.stage = match std::mem::take(&mut self.stage) {
            Resolve::Earlier => self.earlier(scan, previous, Bound::Unbounded, slice),
            Resolve::EarlierAfter(after) => {
                self.earlier(scan, previous, Bound::Excluded(&after), slice)
            }
            Resolve::Facts(after) => self.facts(facts, after, slice),
            Resolve::Logged(from) => self.logged(scan, from, slice),
            Resolve::Done => Resolve::Done,
        };
        matches!(self.stage, Resolve::Done)
    }

    fn earlier(
        &mut self,
        scan: &mut Scan,
        previous: &Scan,
        after: Bound<&RelPath>,
        slice: usize,
    ) -> Resolve {
        let mut earlier = previous
            .files
            .range::<RelPath, _>((after, Bound::Unbounded));
        let mut earlier = earlier.by_ref().peekable();
        let mut files = scan
            .files
            .range_mut::<RelPath, _>((after, Bound::Unbounded));
        let mut last = None;
        for (path, file) in files.by_ref().take(slice) {
            last = Some(path);
            while earlier.next_if(|(before, _)| *before < path).is_some() {}
            if file.identity.is_some() {
                continue;
            }
            let same = earlier.peek().filter(|(before, was)| {
                *before == path && (was.len, was.modified) == (file.len, file.modified)
            });
            file.identity = same.and_then(|(_, was)| was.identity);
            if file.identity.is_none() {
                self.unknown.push(path.clone());
            }
        }
        let more = files.next().is_some();
        match (last.filter(|_| more), self.unknown.is_empty()) {
            (Some(last), _) => Resolve::EarlierAfter(last.clone()),
            (None, true) => Resolve::Done,
            (None, false) => Resolve::Facts(None),
        }
    }

    fn facts(&mut self, facts: &Facts, after: Option<EntityId>, slice: usize) -> Resolve {
        let from = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut entities = facts.range((from, Bound::Unbounded));
        let mut last = None;
        for (entity, written) in entities.by_ref().take(slice) {
            last = Some(*entity);
            for fact in written.iter().map(|fact| &fact.value) {
                self.lengths.insert(fact.len);
                if self.unknown.binary_search(&fact.path).is_ok() {
                    let said = (fact.len, fact.modified, fact.identity);
                    self.logged.entry(fact.path.clone()).or_default().push(said);
                }
            }
        }
        match entities.next() {
            Some(_) => Resolve::Facts(last),
            None => Resolve::Logged(0),
        }
    }

    fn logged(&mut self, scan: &mut Scan, from: usize, slice: usize) -> Resolve {
        let to = from.saturating_add(slice).min(self.unknown.len());
        for path in &self.unknown[from..to] {
            let file = scan.files.get_mut(path).expect("listed in an earlier step");
            let said = self.logged.get(path).into_iter().flatten().rev();
            let same =
                said.filter(|(len, modified, _)| (*len, *modified) == (file.len, file.modified));
            file.identity = same.map(|(_, _, identity)| *identity).next();
            if file.identity.is_none() && self.lengths.contains(&file.len) {
                self.needed.push((path.clone(), file.len));
            }
        }
        match to == self.unknown.len() {
            true => Resolve::Done,
            false => Resolve::Logged(to),
        }
    }
}

/// `scan` with the identities of `unknown` read; with the paths read. A file gone
/// before its read is left out.
pub(crate) fn identified<'a>(
    mut scan: Scan,
    unknown: Vec<(RelPath, u64)>,
    identify: &Rc<dyn Identify>,
) -> flow::Fallible<'a, (Scan, Vec<RelPath>)> {
    let paths: Vec<RelPath> = unknown.iter().map(|(path, _)| path.clone()).collect();
    flow::identities(Root::Folder, unknown, identify).map_ok(move |identities| {
        let mut read = Vec::new();
        for (path, identity) in paths.into_iter().zip(identities) {
            match identity {
                Some(identity) => {
                    if let Some(file) = scan.files.get_mut(&path) {
                        file.identity = Some(identity);
                    }
                    read.push(path);
                }
                None => {
                    scan.files.remove(&path);
                }
            }
        }
        (scan, read)
    })
}

/// `scan` with what is at `path`: a file, every file under a directory, or
/// nothing.
fn visit<'a>(layout: Rc<Layout>, path: RelPath, scan: Scan) -> flow::Fallible<'a, Scan> {
    if layout.owns(&path) {
        return ok(scan);
    }
    flow::stat(Root::Folder, &path).and_then(move |meta| match meta {
        Some(meta) if meta.kind == Kind::Directory => walk(layout, path, scan),
        Some(_) if path.name().is_some_and(is_swap_file) => ok(scan),
        Some(meta) => ok(found(scan, path, meta)),
        None => ok(scan),
    })
}

fn walk<'a>(layout: Rc<Layout>, dir: RelPath, scan: Scan) -> flow::Fallible<'a, Scan> {
    flow::list_stat(Root::Folder, &dir).and_then(move |entries| {
        fold(entries.into_iter(), scan, move |scan, (name, meta)| {
            let path = dir.join(&name).expect("a listed name is one component");
            match meta.kind {
                Kind::Directory if layout.owns(&path) => ok(scan),
                Kind::Directory => walk(Rc::clone(&layout), path, scan),
                Kind::File if is_swap_file(&name) => ok(scan),
                Kind::File => ok(found(scan, path, meta)),
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
    bind_known(facts, scan, names, &Unscanned::default()).0
}

/// File-register writes for the moves `facts` do not already say: a file found in
/// sync at another path. A new modification time alone is not pinned; it only
/// saves reading an identity. Conflicted registers, changed files and missing
/// ones are left for the user.
pub fn pins(facts: &Facts, bindings: &Bindings, scan: &Scan) -> Vec<Op> {
    let mut ops = Vec::new();
    for (&entity, file) in &bindings.bound {
        ops.extend(pin(facts, scan, entity, file));
    }
    ops
}

fn pin(facts: &Facts, scan: &Scan, entity: EntityId, file: &FileRef) -> Option<Op> {
    let ([written], FileState::InSync) = (
        facts.get(&entity).map_or(&[][..], Vec::as_slice),
        file.state,
    ) else {
        return None;
    };
    let found = scan.files.get(&file.path)?;
    if file.path == written.value.path {
        return None;
    }
    let pinned = FileFact {
        path: file.path.clone(),
        identity: found.identity.unwrap_or(written.value.identity),
        len: found.len,
        modified: found.modified,
    };
    Some(Op::Pin {
        entity,
        file: pinned,
        replaces: vec![written.entry],
    })
}

/// [`bind_known`] a slice at a time. It holds no reference to the facts or the
/// scan: each step is handed them, ⚠️ the same ones every time.
pub struct Binding {
    /// What is bound in place of the facts and the scan where a failed scan left
    /// something unknown, with what it left unknown.
    known: Option<(Facts, Scan, Unscanned)>,
    stage: Stage,
    progress: Progress,
}

/// Where a [`Binding`] is. A stage that walks the facts or the scan goes on after
/// the last key it took.
enum Stage {
    Candidates(Option<EntityId>),
    Claims,
    Free(Option<RelPath>),
    Departed,
    Moves,
    Unbound(Option<RelPath>),
    Pins(Option<EntityId>),
    /// Binding the entities a failed scan left unknown as unscanned.
    Unknown(Option<EntityId>),
    Done,
}

/// What a [`Binding`] has found so far. A fact is named by its entity and its
/// index among the entity's facts.
#[derive(Default)]
struct Progress {
    bindings: Bindings,
    /// What [`Bindings::bound`] will hold.
    bound: BTreeMap<EntityId, FileRef>,
    taken: BTreeSet<RelPath>,
    /// The entities naming each file, each by its one candidate.
    claims: BTreeMap<RelPath, Vec<(EntityId, usize)>>,
    /// The entities whose file was not found where they say, by the fact whose
    /// identity is looked for.
    departed: VecDeque<(EntityId, usize)>,
    by_key: Option<BTreeMap<String, Vec<RelPath>>>,
    /// Files bound by no path, by identity and length.
    free: BTreeMap<(Identity, u64), Vec<RelPath>>,
    /// How many departed entities each free file holds the identity of.
    wanted: BTreeMap<RelPath, usize>,
    moves: VecDeque<(EntityId, usize, Vec<RelPath>)>,
    in_sync: Option<BTreeMap<Identity, Vec<EntityId>>>,
    pins: Vec<Op>,
}

impl Binding {
    pub fn new(facts: &Facts, scan: &Scan, unscanned: &Unscanned) -> Self {
        let known = (!unscanned.is_empty()).then(|| {
            let paths: BTreeSet<&str> = unscanned.paths.iter().map(RelPath::as_str).collect();
            let mut known = scan.clone();
            known.files.retain(|path, _| !under(path, &paths));
            let facts = facts
                .iter()
                .filter(|(entity, written)| !unscanned.leaves_unknown(&paths, **entity, written));
            let facts = facts.map(|(entity, written)| (*entity, written.clone()));
            (facts.collect(), known, unscanned.clone())
        });
        Self {
            known,
            stage: Stage::Candidates(None),
            progress: Progress::default(),
        }
    }

    /// Binds about `slice` more entities or files of `facts` and `scan`; true once
    /// every one is bound.
    pub fn step(&mut self, facts: &Facts, scan: &Scan, names: &dyn Names, slice: usize) -> bool {
        let Self {
            known,
            stage,
            progress,
        } = self;
        let every = facts;
        let (facts, scan) = match &*known {
            Some((facts, scan, _)) => (facts, scan),
            None => (facts, scan),
        };
        let mut budget = slice;
        while budget > 0 {
            let budget = &mut budget;
            *stage = match std::mem::replace(stage, Stage::Done) {
                Stage::Candidates(from) => {
                    let taken = take_after(facts, from, budget, |&entity, written| {
                        progress.candidate(scan, names, entity, written);
                    });
                    taken.map_or(Stage::Claims, |last| Stage::Candidates(Some(last)))
                }
                Stage::Claims => match progress.claims.pop_first() {
                    Some((path, claimants)) => {
                        *budget -= 1;
                        progress.claim(facts, scan, path, claimants);
                        Stage::Claims
                    }
                    None => Stage::Free(None),
                },
                Stage::Free(from) => {
                    let taken = take_after(&scan.files, from, budget, |path, file| {
                        progress.free(path, file);
                    });
                    taken.map_or(Stage::Departed, |last| Stage::Free(Some(last)))
                }
                Stage::Departed => match progress.departed.pop_front() {
                    Some((entity, fact)) => {
                        *budget -= 1;
                        progress.depart(facts, entity, fact);
                        Stage::Departed
                    }
                    None => Stage::Moves,
                },
                Stage::Moves => match progress.moves.pop_front() {
                    Some((entity, fact, matches)) => {
                        *budget -= 1;
                        progress.moved(facts, entity, fact, matches);
                        Stage::Moves
                    }
                    None => Stage::Unbound(None),
                },
                Stage::Unbound(from) => {
                    let taken = take_after(&scan.files, from, budget, |path, file| {
                        progress.unbound(scan, path, file);
                    });
                    match taken {
                        Some(last) => Stage::Unbound(Some(last)),
                        None => {
                            progress.reported();
                            Stage::Pins(None)
                        }
                    }
                }
                Stage::Pins(from) => {
                    let Progress { bound, pins, .. } = progress;
                    let taken = take_after(bound, from, budget, |&entity, file| {
                        pins.extend(pin(facts, scan, entity, file));
                    });
                    taken.map_or(Stage::Unknown(None), |last| Stage::Pins(Some(last)))
                }
                Stage::Unknown(from) => {
                    let Some((_, _, unscanned)) = &*known else {
                        return true;
                    };
                    let paths = unscanned.paths.iter().map(RelPath::as_str).collect();
                    let bound = &mut progress.bound;
                    let taken = take_after(every, from, budget, |&entity, written| {
                        let latest = written.last();
                        let unknown =
                            latest.filter(|_| unscanned.leaves_unknown(&paths, entity, written));
                        if let Some(latest) = unknown {
                            let file = FileRef {
                                path: latest.value.path.clone(),
                                state: FileState::Unscanned,
                            };
                            bound.insert(entity, file);
                        }
                    });
                    taken.map_or(Stage::Done, |last| Stage::Unknown(Some(last)))
                }
                Stage::Done => return true,
            };
        }
        matches!(stage, Stage::Done)
    }

    /// ⚠️ Before [`Binding::step`] returns true, what is bound so far.
    pub fn finish(self) -> (Bindings, Vec<Op>) {
        let Progress {
            mut bindings,
            bound,
            pins,
            ..
        } = self.progress;
        bindings.bound = CowMap::from_sorted(bound.into_iter().collect());
        let report = &mut bindings.report;
        report.departed.sort();
        report.moved.sort_by_key(|moved| moved.entity);
        report.ambiguous.sort_by_key(|ambiguous| ambiguous.entity);
        (bindings, pins)
    }
}

/// Calls `take` on the entries of `map` after `from`, a unit of `budget` each,
/// at least one, while it lasts: the last key taken, or `None` once every entry
/// was.
fn take_after<K: Ord + Clone, V>(
    map: &BTreeMap<K, V>,
    from: Option<K>,
    budget: &mut usize,
    mut take: impl FnMut(&K, &V),
) -> Option<K> {
    let start = from.as_ref().map_or(Bound::Unbounded, Bound::Excluded);
    for (key, value) in map.range((start, Bound::Unbounded)) {
        take(key, value);
        *budget = budget.saturating_sub(1);
        if *budget == 0 {
            return Some(key.clone());
        }
    }
    None
}

impl Progress {
    /// Finds the files `entity`'s facts name: by path, else by the name the
    /// volume takes for it.
    fn candidate(
        &mut self,
        scan: &Scan,
        names: &dyn Names,
        entity: EntityId,
        written: &[Written<FileFact>],
    ) {
        let Some(first) = written.first() else {
            return;
        };
        let Self {
            by_key,
            claims,
            departed,
            bindings,
            bound,
            ..
        } = self;
        let gone = written
            .iter()
            .any(|w| !scan.files.contains_key(&w.value.path));
        if gone && by_key.is_none() {
            *by_key = Some(keyed(scan, names));
        }
        let mut candidates: Vec<(&RelPath, usize)> = Vec::new();
        for (index, fact) in written.iter().map(|w| &w.value).enumerate() {
            match scan.files.get_key_value(&fact.path) {
                Some((path, _)) => candidates.push((path, index)),
                None => {
                    let key = names.key(fact.path.as_str());
                    let same = by_key.as_ref().and_then(|by_key| by_key.get(&key));
                    candidates.extend(same.into_iter().flatten().map(|path| (path, index)));
                }
            }
        }
        candidates.sort_by(|a, b| a.0.cmp(b.0));
        candidates.dedup_by(|a, b| a.0 == b.0);
        match candidates.as_slice() {
            [] => departed.push_back((entity, 0)),
            [(path, index)] => claims
                .entry((*path).clone())
                .or_default()
                .push((entity, *index)),
            several => {
                bindings.report.ambiguous.push(Ambiguous {
                    entity,
                    candidates: several.iter().map(|(path, _)| (*path).clone()).collect(),
                });
                bound.insert(entity, missing(&first.value));
            }
        }
    }

    /// Several entities naming one file: the one whose identity it holds, if only
    /// one does, has it. The others, or all when that does not decide, are bound
    /// by identity like entities whose path is gone.
    fn claim(
        &mut self,
        facts: &Facts,
        scan: &Scan,
        path: RelPath,
        claimants: Vec<(EntityId, usize)>,
    ) {
        let file = &scan.files[&path];
        let held = |(entity, index): &(EntityId, usize)| holds(&facts[entity][*index].value, file);
        let holders: Vec<EntityId> = claimants
            .iter()
            .filter(|claimant| held(claimant))
            .map(|(entity, _)| *entity)
            .collect();
        let (entity, state) = match (claimants.as_slice(), holders.as_slice()) {
            ([only], _) if held(only) => (only.0, FileState::InSync),
            ([only], _) => (only.0, FileState::ChangedOutside),
            (_, [holder]) => (*holder, FileState::InSync),
            _ => {
                self.departed.extend(claimants);
                return;
            }
        };
        let others = claimants.into_iter().filter(|(other, _)| *other != entity);
        self.departed.extend(others);
        self.taken.insert(path.clone());
        self.bound.insert(entity, FileRef { path, state });
    }

    fn free(&mut self, path: &RelPath, file: &Scanned) {
        if let (Some(identity), false) = (file.identity, self.taken.contains(path)) {
            let free = self.free.entry((identity, file.len)).or_default();
            free.push(path.clone());
        }
    }

    /// Looks for the free files holding the identity `entity`'s fact gives.
    fn depart(&mut self, facts: &Facts, entity: EntityId, index: usize) {
        let fact = &facts[&entity][index].value;
        let matches = self.free.get(&(fact.identity, fact.len));
        let matches = matches.cloned().unwrap_or_default();
        for path in &matches {
            *self.wanted.entry(path.clone()).or_default() += 1;
        }
        self.moves.push_back((entity, index, matches));
    }

    /// Binds a departed entity to the one free file holding its identity, when no
    /// other departed entity wants that file.
    fn moved(&mut self, facts: &Facts, entity: EntityId, index: usize, matches: Vec<RelPath>) {
        let fact = &facts[&entity][index].value;
        let (bindings, bound) = (&mut self.bindings, &mut self.bound);
        match matches.as_slice() {
            [path] if self.wanted[path] == 1 => {
                self.taken.insert(path.clone());
                bindings.report.moved.push(Moved {
                    entity,
                    from: fact.path.clone(),
                    to: path.clone(),
                });
                let file = FileRef {
                    path: path.clone(),
                    state: FileState::InSync,
                };
                bound.insert(entity, file);
            }
            [] => {
                bindings.report.departed.push(entity);
                bound.insert(entity, missing(fact));
            }
            _ => {
                bindings.report.ambiguous.push(Ambiguous {
                    entity,
                    candidates: matches,
                });
                bound.insert(entity, missing(fact));
            }
        }
    }

    /// Keeps a file no entity is bound to, and reports it as a copy of each
    /// entity bound in sync to a file with its identity.
    fn unbound(&mut self, scan: &Scan, path: &RelPath, file: &Scanned) {
        if self.taken.contains(path) {
            return;
        }
        let bindings = &mut self.bindings;
        bindings.unbound.push(path.clone());
        let Some(identity) = file.identity else {
            return;
        };
        let bound = &self.bound;
        let in_sync = self.in_sync.get_or_insert_with(|| in_sync(bound, scan));
        for &entity in in_sync.get(&identity).into_iter().flatten() {
            bindings.report.copied.push(Copied {
                entity,
                copy: path.clone(),
            });
        }
    }

    fn reported(&mut self) {
        let bindings = &mut self.bindings;
        bindings.report.arrived = bindings.unbound.clone();
        bindings.report.changed = self
            .bound
            .iter()
            .filter(|(_, file)| file.state == FileState::ChangedOutside)
            .map(|(&entity, _)| entity)
            .collect();
    }
}

/// Every file of `scan` by the name the volume takes for its path.
fn keyed(scan: &Scan, names: &dyn Names) -> BTreeMap<String, Vec<RelPath>> {
    let mut by_key: BTreeMap<String, Vec<RelPath>> = BTreeMap::new();
    for path in scan.files.keys() {
        by_key
            .entry(names.key(path.as_str()))
            .or_default()
            .push(path.clone());
    }
    by_key
}

/// The entities bound in sync, by the identity of their file.
fn in_sync(bound: &BTreeMap<EntityId, FileRef>, scan: &Scan) -> BTreeMap<Identity, Vec<EntityId>> {
    let mut by = BTreeMap::new();
    for (&entity, file) in bound {
        if file.state != FileState::InSync {
            continue;
        }
        let Some(identity) = scan.files.get(&file.path).and_then(|found| found.identity) else {
            continue;
        };
        by.entry(identity).or_insert_with(Vec::new).push(entity);
    }
    by
}

fn missing(fact: &FileFact) -> FileRef {
    FileRef {
        path: fact.path.clone(),
        state: FileState::Missing,
    }
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

    /// Random worlds richer than the small ones: entities with conflicting file
    /// facts, names the volume folds, identities read or not, and what a failed
    /// scan left unknown.
    fn random_worlds(seed: u64, count: usize) -> Vec<(Facts, Scan, Unscanned)> {
        use crate::env::{Random, SeededRandom};
        let mut random = SeededRandom::new(seed);
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        let places = ["a", "A", "b", "c", "d/e", "d/E"];
        (0..count)
            .map(|_| {
                let mut facts = Facts::new();
                for e in 1..=1 + pick(6) as u128 {
                    let written: Vec<_> = (0..1 + pick(2))
                        .map(|i| written(fact(places[pick(6)], pick(3)), e * 10 + i as u128))
                        .collect();
                    facts.insert(EntityId::from_u128(e), written);
                }
                let mut files: Vec<(&str, usize)> = Vec::new();
                for place in places {
                    if pick(3) > 0 {
                        files.push((place, pick(3)));
                    }
                }
                let mut scan = scanned(&files, &facts);
                for file in scan.files.values_mut() {
                    if pick(4) == 0 {
                        file.identity = None;
                    }
                }
                let mut unscanned = Unscanned::default();
                if pick(4) == 0 {
                    let paths = [RelPath::ROOT, path("a"), path("d"), path("c")];
                    unscanned.paths.insert(paths[pick(4)].clone());
                    unscanned
                        .entities
                        .insert(EntityId::from_u128(1 + pick(3) as u128));
                }
                (facts, scan, unscanned)
            })
            .collect()
    }

    #[test]
    fn binding_in_slices_binds_what_binding_at_once_binds() {
        let small = worlds()
            .into_iter()
            .map(|(f, s)| (f, s, Unscanned::default()));
        for (facts, scan, unscanned) in small.chain(random_worlds(9, 3000)) {
            for names in [&ExactNames as &dyn Names, &Folding] {
                let whole = bind_known(&facts, &scan, names, &unscanned);
                for slice in 1..4 {
                    let mut binding = Binding::new(&facts, &scan, &unscanned);
                    while !binding.step(&facts, &scan, names, slice) {}
                    assert_eq!(
                        binding.finish(),
                        whole,
                        "{slice}-item slices of {facts:?} {scan:?} {unscanned:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn a_file_takes_the_identity_an_earlier_scan_else_the_last_fact_gives() {
        use crate::env::{Random, SeededRandom};
        let mut random = SeededRandom::new(12);
        for (mut facts, scan, _) in random_worlds(11, 1000) {
            let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
            if let Some(written) = facts.values_mut().nth(pick(2)) {
                let mut twin = written[0].clone();
                twin.value.identity = Identity::from_u128(99);
                written.push(twin);
            }
            let mut scan = scan;
            for file in scan.files.values_mut() {
                file.modified = Some(pick(2) as u64);
            }
            let mut previous = scan.clone();
            previous.files.retain(|_, _| pick(3) > 0);
            for file in previous.files.values_mut() {
                match pick(4) {
                    0 => file.identity = None,
                    1 => file.len += 1,
                    _ => {}
                }
            }
            let mut fresh = scan.clone();
            for file in fresh.files.values_mut() {
                if pick(2) == 0 {
                    file.identity = None;
                }
            }
            let mut expected = fresh.clone();
            let mut known: BTreeMap<(RelPath, u64, Option<u64>), Identity> = BTreeMap::new();
            for fact in facts.values().flatten() {
                let key = (fact.value.path.clone(), fact.value.len, fact.value.modified);
                known.insert(key, fact.value.identity);
            }
            for (path, file) in &previous.files {
                if let Some(identity) = file.identity {
                    known.insert((path.clone(), file.len, file.modified), identity);
                }
            }
            let lengths: BTreeSet<u64> = facts.values().flatten().map(|f| f.value.len).collect();
            let mut needed = Vec::new();
            for (path, file) in &mut expected.files {
                if file.identity.is_none() {
                    file.identity = known.get(&(path.clone(), file.len, file.modified)).copied();
                    if file.identity.is_none() && lengths.contains(&file.len) {
                        needed.push((path.clone(), file.len));
                    }
                }
            }
            let mut sliced = fresh.clone();
            let unknown = resolve(&mut fresh, &facts, &previous);
            assert_eq!(
                (fresh, unknown),
                (expected.clone(), needed.clone()),
                "{facts:?} {previous:?}"
            );
            let slice = 1 + pick(3);
            let mut resolving = Resolving::default();
            while !resolving.step(&mut sliced, &facts, &previous, slice) {}
            assert_eq!(
                (sliced, resolving.needed),
                (expected, needed),
                "{slice}-item slices"
            );
        }
    }

    #[test]
    fn a_walk_gives_its_files_in_slices_as_it_listed_them() {
        use crate::env::{Random, SeededRandom};
        let layout = Layout::new(".lib").unwrap();
        let mut random = SeededRandom::new(13);
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        for _ in 0..200 {
            let mut walk = Walk::default();
            let mut listed = BTreeMap::new();
            while let Some(dir) = walk.next() {
                let mut entries = Vec::new();
                for i in 0..pick(5) {
                    let kind = [Kind::File, Kind::File, Kind::Directory][pick(3)];
                    let meta = Meta {
                        kind,
                        len: i as u64,
                        modified: Some(1),
                    };
                    entries.push((format!("{}{i}", dir.as_str().len()), meta));
                }
                for (name, meta) in &entries {
                    if meta.kind == Kind::File {
                        let file = Scanned {
                            len: meta.len,
                            modified: meta.modified,
                            identity: None,
                        };
                        listed.insert(dir.join(name).unwrap(), file);
                    }
                }
                let deep = dir.components().count() > 2;
                entries.retain(|(_, meta)| !deep || meta.kind == Kind::File);
                walk.listed(&layout, dir, entries);
            }
            let slice = 1 + pick(4);
            let (mut found, mut after) = (Scan::default(), None);
            loop {
                let (last, done) = walk.found_after(&mut found, after.as_ref(), slice);
                after = last;
                if done {
                    break;
                }
            }
            assert_eq!(found.files, listed, "{slice}-file slices");
        }
    }

    #[test]
    fn with_every_path_unknown_each_entity_is_presumed_at_its_latest_path() {
        let everything = Unscanned {
            paths: BTreeSet::from([RelPath::ROOT]),
            entities: BTreeSet::new(),
        };
        for (facts, scan, _) in random_worlds(10, 300) {
            let (bindings, pins) = bind_known(&facts, &scan, &ExactNames, &everything);
            let presumed: BTreeMap<EntityId, FileRef> = facts
                .iter()
                .map(|(entity, written)| {
                    let path = written.last().unwrap().value.path.clone();
                    let state = FileState::Unscanned;
                    (*entity, FileRef { path, state })
                })
                .collect();
            assert_eq!(bindings.bound, presumed.into_iter().collect(), "{facts:?}");
            assert_eq!((bindings.unbound, pins), (Vec::new(), Vec::new()));
        }
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
                    FileState::Unscanned => panic!("{shown}: {entity} is left unscanned"),
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
