//! What a change to the file facts or to the library's files can reach through the
//! rules of binding, so that binding after a few changes binds only that.
//!
//! An entity and a file take part in each other's binding only when a fact of the
//! entity names the file's path or a path the volume takes for it, or gives the
//! identity the file holds; two entities only through such a file or a shared
//! identity; and two files only through an entity or an identity. Binding is a
//! function of each group of entities and files these links join, so binding
//! again the groups a change touches, before and after it, and keeping every
//! other binding, binds what binding everything binds.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{Hash, Hasher};

use crate::binding::{self, Bindings, Fact, Facts, Scan, Scanned, Unscanned};
use crate::cow::CowMap;
use crate::env::Names;
use crate::ids::{EntityId, Identity};
use crate::log::{FileFact, Op};
use crate::path::RelPath;

/// The links between the facts and the files of the last binding of everything,
/// kept up as either changes, and what changed since the last binding.
#[derive(Default)]
pub(crate) struct Reach {
    /// `None` until a binding of every fact to a full scan.
    links: Option<Links>,
    due: Due,
    /// The lengths the facts gained since the files' identities were last
    /// resolved. Every file a resolve leaves without an identity has a length no
    /// fact gave, so only files of these lengths may need one.
    gained: BTreeSet<u64>,
}

/// What the next binding binds.
#[derive(Default, PartialEq, Debug)]
enum Due {
    #[default]
    Nothing,
    /// What changed reaches, before and after the change.
    Reached(Seeds),
    Everything,
}

#[derive(Default, PartialEq, Debug)]
pub(crate) struct Seeds {
    entities: BTreeSet<EntityId>,
    files: BTreeSet<RelPath>,
}

/// Who could take part in whose binding: the keys and identities each fact and
/// each file links.
#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct Links {
    /// Each fact's path's key on the volume, hashed, with its entity.
    keys: CowMap<(u64, EntityId), ()>,
    /// Each fact's identity, with its entity.
    identities: CowMap<(Identity, EntityId), ()>,
    /// Each file holding an identity, by it.
    held: CowMap<(Identity, RelPath), ()>,
    /// Each file whose key is not its path, by its key hashed.
    keyed: CowMap<(u64, RelPath), ()>,
    /// How many facts give each length.
    lengths: BTreeMap<u64, usize>,
}

/// The entities and files a change reaches.
#[derive(Default, Debug)]
struct Region {
    entities: BTreeSet<EntityId>,
    files: BTreeSet<RelPath>,
}

/// A key's hash: equal keys hash alike, and a collision only links more.
fn hashed(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// The facts `facts` holds of `entity`, none when it holds none.
fn facts_of<'a>(facts: &'a Facts, entity: &EntityId) -> &'a [Fact] {
    facts.get(entity).map_or(&[], |written| &written[..])
}

impl Links {
    /// The links of `facts` and `scan`, at once.
    pub(crate) fn of(facts: &Facts, scan: &Scan, names: &dyn Names) -> Self {
        let mut linking = Linking::default();
        while !linking.step(facts, scan, names, usize::MAX) {}
        linking.finish()
    }

    fn add_facts(&mut self, entity: EntityId, written: &[Fact], names: &dyn Names) {
        for fact in written.iter().map(|written| &written.value) {
            let key = hashed(&names.key(fact.path.as_str()));
            self.keys.insert((key, entity), ());
            self.identities.insert((fact.identity, entity), ());
            *self.lengths.entry(fact.len).or_default() += 1;
        }
    }

    fn remove_facts(&mut self, entity: EntityId, written: &[Fact], names: &dyn Names) {
        for fact in written.iter().map(|written| &written.value) {
            let key = hashed(&names.key(fact.path.as_str()));
            self.keys.remove(&(key, entity));
            self.identities.remove(&(fact.identity, entity));
            if let Some(count) = self.lengths.get_mut(&fact.len) {
                *count -= 1;
                if *count == 0 {
                    self.lengths.remove(&fact.len);
                }
            }
        }
    }

    fn add_file(&mut self, path: &RelPath, file: &Scanned, names: &dyn Names) {
        if let Some(identity) = file.identity {
            self.held.insert((identity, path.clone()), ());
        }
        let key = names.key(path.as_str());
        if key != path.as_str() {
            self.keyed.insert((hashed(&key), path.clone()), ());
        }
    }

    fn remove_file(&mut self, path: &RelPath, file: &Scanned, names: &dyn Names) {
        if let Some(identity) = file.identity {
            self.held.remove(&(identity, path.clone()));
        }
        let key = names.key(path.as_str());
        if key != path.as_str() {
            self.keyed.remove(&(hashed(&key), path.clone()));
        }
    }

    /// Whether some fact gives `len`.
    fn has_length(&self, len: u64) -> bool {
        self.lengths.contains_key(&len)
    }

    /// The entities with a fact whose path's key is `key`, in order, and maybe a
    /// few more.
    fn naming(&self, key: &str) -> impl Iterator<Item = EntityId> + '_ {
        let key = hashed(key);
        let found = self.keys.seek(move |(other, _)| other.cmp(&key));
        found
            .take_while(move |((other, _), _)| *other == key)
            .map(|((_, entity), _)| *entity)
    }

    /// Adds to `region` every entity and file the links join to `entities` and
    /// `files`, those included, in `facts` and `scan`.
    fn reach(
        &self,
        facts: &Facts,
        scan: &Scan,
        names: &dyn Names,
        entities: impl IntoIterator<Item = EntityId>,
        files: impl IntoIterator<Item = RelPath>,
        region: &mut Region,
    ) {
        let mut entities: Vec<EntityId> = entities.into_iter().collect();
        let mut files: Vec<RelPath> = files.into_iter().collect();
        let mut keys: HashSet<u64> = HashSet::new();
        let mut identities: HashSet<Identity> = HashSet::new();
        loop {
            if let Some(entity) = entities.pop() {
                if !region.entities.insert(entity) {
                    continue;
                }
                for fact in facts_of(facts, &entity).iter().map(|w| &w.value) {
                    if scan.files.contains_key(&fact.path) {
                        files.push(fact.path.clone());
                    }
                    let key = names.key(fact.path.as_str());
                    self.by_key(&key, scan, &mut keys, &mut entities, &mut files);
                    self.by_identity(fact.identity, &mut identities, &mut entities, &mut files);
                }
            } else if let Some(path) = files.pop() {
                if region.files.contains(&path) {
                    continue;
                }
                let file = scan.files.get(&path).copied();
                let key = names.key(path.as_str());
                region.files.insert(path);
                self.by_key(&key, scan, &mut keys, &mut entities, &mut files);
                if let Some(identity) = file.and_then(|file| file.identity) {
                    self.by_identity(identity, &mut identities, &mut entities, &mut files);
                }
            } else {
                return;
            }
        }
    }

    fn by_key(
        &self,
        key: &str,
        scan: &Scan,
        seen: &mut HashSet<u64>,
        entities: &mut Vec<EntityId>,
        files: &mut Vec<RelPath>,
    ) {
        let hash = hashed(key);
        if !seen.insert(hash) {
            return;
        }
        entities.extend(self.naming(key));
        let keyed = self.keyed.seek(|(other, _)| other.cmp(&hash));
        let keyed = keyed.take_while(|((other, _), _)| *other == hash);
        files.extend(keyed.map(|((_, path), _)| path.clone()));
        if let Some((path, _)) = scan.files.get_key_value(key) {
            files.push(path.clone());
        }
    }

    fn by_identity(
        &self,
        identity: Identity,
        seen: &mut HashSet<Identity>,
        entities: &mut Vec<EntityId>,
        files: &mut Vec<RelPath>,
    ) {
        if !seen.insert(identity) {
            return;
        }
        let holders = self.identities.seek(|(other, _)| other.cmp(&identity));
        let holders = holders.take_while(|((other, _), _)| *other == identity);
        entities.extend(holders.map(|((_, entity), _)| *entity));
        let held = self.held.seek(|(other, _)| other.cmp(&identity));
        let held = held.take_while(|((other, _), _)| *other == identity);
        files.extend(held.map(|((_, path), _)| path.clone()));
    }
}

/// [`Links::of`] a slice at a time.
#[derive(Default)]
pub(crate) struct Linking {
    stage: Stage,
    keys: Vec<((u64, EntityId), ())>,
    identities: Vec<((Identity, EntityId), ())>,
    held: Vec<((Identity, RelPath), ())>,
    keyed: Vec<((u64, RelPath), ())>,
    lengths: BTreeMap<u64, usize>,
    links: Links,
}

#[derive(Default)]
enum Stage {
    #[default]
    Facts,
    FactsAfter(EntityId),
    Files,
    FilesAfter(RelPath),
    Sort,
    Done,
}

impl Linking {
    /// Links about `slice` more facts or files; true once every one is linked.
    /// ⚠️ Each step must be given the same facts and scan.
    pub(crate) fn step(
        &mut self,
        facts: &Facts,
        scan: &Scan,
        names: &dyn Names,
        slice: usize,
    ) -> bool {
        self.stage = match std::mem::take(&mut self.stage) {
            Stage::Facts => self.facts(facts, None, names, slice),
            Stage::FactsAfter(after) => self.facts(facts, Some(after), names, slice),
            Stage::Files => self.files(scan, None, names, slice),
            Stage::FilesAfter(after) => self.files(scan, Some(after), names, slice),
            Stage::Sort => self.sort(),
            Stage::Done => Stage::Done,
        };
        matches!(self.stage, Stage::Done)
    }

    fn facts(
        &mut self,
        facts: &Facts,
        after: Option<EntityId>,
        names: &dyn Names,
        slice: usize,
    ) -> Stage {
        let mut entities = facts.after(after);
        let mut last = None;
        for (entity, written) in entities.by_ref().take(slice) {
            last = Some(*entity);
            for fact in written.iter().map(|written| &written.value) {
                let key = hashed(&names.key(fact.path.as_str()));
                self.keys.push(((key, *entity), ()));
                self.identities.push(((fact.identity, *entity), ()));
                *self.lengths.entry(fact.len).or_default() += 1;
            }
        }
        match (entities.next(), last) {
            (Some(_), Some(last)) => Stage::FactsAfter(last),
            _ => Stage::Files,
        }
    }

    fn files(
        &mut self,
        scan: &Scan,
        after: Option<RelPath>,
        names: &dyn Names,
        slice: usize,
    ) -> Stage {
        let from = after
            .as_ref()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
        let mut files = scan
            .files
            .range::<RelPath, _>((from, std::ops::Bound::Unbounded));
        let mut last = None;
        for (path, file) in files.by_ref().take(slice) {
            last = Some(path);
            if let Some(identity) = file.identity {
                self.held.push(((identity, path.clone()), ()));
            }
            let key = names.key(path.as_str());
            if key != path.as_str() {
                self.keyed.push(((hashed(&key), path.clone()), ()));
            }
        }
        match (files.next(), last) {
            (Some(_), Some(last)) => Stage::FilesAfter(last.clone()),
            _ => Stage::Sort,
        }
    }

    fn sort(&mut self) -> Stage {
        fn sorted<K: Ord + Clone>(mut list: Vec<(K, ())>) -> CowMap<K, ()> {
            list.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            list.dedup_by(|a, b| a.0 == b.0);
            CowMap::from_sorted(list)
        }
        self.links = Links {
            keys: sorted(std::mem::take(&mut self.keys)),
            identities: sorted(std::mem::take(&mut self.identities)),
            held: sorted(std::mem::take(&mut self.held)),
            keyed: sorted(std::mem::take(&mut self.keyed)),
            lengths: std::mem::take(&mut self.lengths),
        };
        Stage::Done
    }

    /// ⚠️ Before [`Linking::step`] returns true, links lacking what is not linked
    /// yet.
    pub(crate) fn finish(self) -> Links {
        self.links
    }
}

impl Reach {
    /// Whether the next binding binds anything.
    pub(crate) fn due(&self) -> bool {
        self.due != Due::Nothing
    }

    /// Whether a change can be bound and resolved by what it reaches: the links
    /// are those of the facts and scan as they are.
    pub(crate) fn ready(&self) -> bool {
        self.links.is_some() && self.due != Due::Everything
    }

    /// The facts or the scan changed in a way not followed link by link.
    pub(crate) fn everything(&mut self) {
        self.due = Due::Everything;
    }

    /// Everything was bound, and `links` are those of what was bound, or `None`
    /// when a failed scan left something unknown.
    pub(crate) fn bound(&mut self, links: Option<Links>) {
        self.links = links;
        self.due = Due::Nothing;
    }

    /// The identities of every file were resolved against every fact.
    pub(crate) fn resolved(&mut self) {
        self.gained.clear();
    }

    /// The lengths the facts gained since the last resolve.
    pub(crate) fn gained(&self) -> &BTreeSet<u64> {
        &self.gained
    }

    pub(crate) fn links(&self) -> Option<&Links> {
        self.links.as_ref()
    }

    /// Takes what the next binding binds: `None` for everything.
    pub(crate) fn take(&mut self) -> Option<Seeds> {
        match std::mem::take(&mut self.due) {
            Due::Nothing => Some(Seeds::default()),
            Due::Reached(seeds) => Some(seeds),
            Due::Everything => None,
        }
    }

    /// Notes what a change to `entity` or `file` reaches before it, so that the
    /// next binding binds it again.
    fn before(
        &mut self,
        facts: &Facts,
        scan: &Scan,
        names: &dyn Names,
        entity: Option<EntityId>,
        file: Option<RelPath>,
    ) {
        let (Some(links), false) = (&self.links, self.due == Due::Everything) else {
            self.due = Due::Everything;
            return;
        };
        if self.due == Due::Nothing {
            self.due = Due::Reached(Seeds::default());
        }
        let Due::Reached(seeds) = &mut self.due else {
            unreachable!("made above");
        };
        let mut region = Region::default();
        links.reach(facts, scan, names, entity, file, &mut region);
        seeds.entities.extend(region.entities);
        seeds.files.extend(region.files);
    }

    /// Gives `entity` the facts `now`; returns those it had.
    pub(crate) fn refile(
        &mut self,
        facts: &mut Facts,
        scan: &Scan,
        names: &dyn Names,
        entity: EntityId,
        now: Box<[Fact]>,
    ) -> Option<Box<[Fact]>> {
        self.before(facts, scan, names, Some(entity), None);
        let lengths = |written: &[Fact]| -> BTreeSet<u64> {
            written.iter().map(|written| written.value.len).collect()
        };
        let gained: Vec<u64> = match &self.links {
            Some(links) => lengths(&now)
                .into_iter()
                .filter(|len| !links.has_length(*len))
                .collect(),
            None => Vec::new(),
        };
        self.gained.extend(gained);
        let was = match now.is_empty() {
            true => facts.remove(&entity),
            false => facts.insert(entity, now),
        };
        if let (Some(links), true) = (&mut self.links, self.due != Due::Everything) {
            links.remove_facts(entity, was.as_deref().unwrap_or_default(), names);
            links.add_facts(entity, facts_of(facts, &entity), names);
        }
        was
    }

    /// Gives the file at `path` the entry `now`, or none; returns the one it had.
    pub(crate) fn rescan(
        &mut self,
        scan: &mut Scan,
        facts: &Facts,
        names: &dyn Names,
        path: RelPath,
        now: Option<Scanned>,
    ) -> Option<Scanned> {
        self.before(facts, scan, names, None, Some(path.clone()));
        let was = match now {
            Some(file) => scan.files.insert(path.clone(), file),
            None => scan.files.remove(&path),
        };
        if let (Some(links), true) = (&mut self.links, self.due != Due::Everything) {
            if let Some(was) = &was {
                links.remove_file(&path, was, names);
            }
            if let Some(now) = scan.files.get(&path) {
                links.add_file(&path, now, names);
            }
        }
        if let Due::Reached(seeds) = &mut self.due {
            seeds.files.insert(path);
        }
        was
    }
}

/// Binds again what `seeds` reach and keeps every other binding of `bindings`
/// and `pins`, the binding of `facts` and `scan` before the changes the seeds
/// note, so that they hold what [`binding::bind_known`] binds of every fact and
/// file. `None` when nothing links them, or they reach so much that binding
/// everything costs less.
pub(crate) fn rebind(
    reach: &Reach,
    seeds: Seeds,
    facts: &Facts,
    scan: &Scan,
    names: &dyn Names,
    bindings: &mut Bindings,
    pins: &mut Vec<Op>,
) -> Option<()> {
    let links = reach.links.as_ref()?;
    let mut region = Region::default();
    links.reach(facts, scan, names, seeds.entities, seeds.files, &mut region);
    if region.entities.len() > REACHED.max(facts.len() / 4) {
        return None;
    }
    let some: Facts = region
        .entities
        .iter()
        .filter_map(|entity| Some((*entity, facts.get(entity)?.clone())))
        .collect();
    let files = region
        .files
        .iter()
        .filter_map(|path| Some((path.clone(), *scan.files.get(path)?)));
    let some_files = Scan {
        files: files.collect(),
    };
    let (found, found_pins) = binding::bind_known(&some, &some_files, names, &Unscanned::default());
    splice(bindings, pins, &region, found, found_pins);
    Some(())
}

/// How many entities a change may reach and still be bound alone, at least.
const REACHED: usize = 1024;

/// Replaces in `bindings` and `pins` what `region` holds with `found` and
/// `found_pins`, the binding of the region alone.
fn splice(
    bindings: &mut Bindings,
    pins: &mut Vec<Op>,
    region: &Region,
    found: Bindings,
    found_pins: Vec<Op>,
) {
    let outside = |entity: &EntityId| !region.entities.contains(entity);
    for entity in &region.entities {
        bindings.bound.remove(entity);
    }
    for (entity, file) in found.bound.iter() {
        bindings.bound.insert(*entity, file.clone());
    }
    let mut unbound: Vec<RelPath> = bindings
        .unbound
        .iter()
        .filter(|path| !region.files.contains(*path))
        .cloned()
        .chain(found.unbound)
        .collect();
    unbound.sort();
    bindings.unbound = unbound;
    let report = &mut bindings.report;
    let found = found.report;
    report.arrived = bindings.unbound.clone();
    report.departed.retain(outside);
    report.departed.extend(found.departed);
    report.departed.sort();
    report.changed.retain(outside);
    report.changed.extend(found.changed);
    report.changed.sort();
    report.moved.retain(|moved| outside(&moved.entity));
    report.moved.extend(found.moved);
    report.moved.sort_by_key(|moved| moved.entity);
    report
        .ambiguous
        .retain(|ambiguous| outside(&ambiguous.entity));
    report.ambiguous.extend(found.ambiguous);
    report.ambiguous.sort_by_key(|ambiguous| ambiguous.entity);
    report.copied.retain(|copied| outside(&copied.entity));
    report.copied.extend(found.copied);
    report
        .copied
        .sort_by(|a, b| (&a.copy, a.entity).cmp(&(&b.copy, b.entity)));
    pins.retain(|op| op.entity().is_none_or(|entity| outside(&entity)));
    pins.extend(found_pins);
    pins.sort_by_key(Op::entity);
}

/// What a relisting changes in a scan: each path whose entry changes, with its
/// new entry, or `None` once it is gone.
pub(crate) type Changes = BTreeMap<RelPath, Option<Scanned>>;

/// What relisting changes in `scan`: for each path whose entry changes, its new
/// entry, or `None` once it is gone; and the files whose identities must be read.
/// `gone` are the files listed before under the paths relisted, and `found`
/// those listed now. Identities resolve as [`binding::resolve`] resolves them
/// when given every file: from the entry the path had, else from the last fact
/// naming the path, else read when some fact gives the length. Of the files not
/// relisted, it resolves only those of a length in `gained`, the lengths the
/// facts gained since the last resolve.
pub(crate) fn resolve(
    scan: &Scan,
    facts: &Facts,
    links: &Links,
    names: &dyn Names,
    gone: Vec<RelPath>,
    found: Scan,
    gained: &BTreeSet<u64>,
) -> (Changes, Vec<(RelPath, u64)>) {
    let mut changes = Changes::new();
    for path in gone {
        changes.insert(path, None);
    }
    let mut fresh: BTreeMap<RelPath, Scanned> = BTreeMap::new();
    for (path, mut file) in found.files {
        let earlier = scan.files.get(&path);
        let same = earlier.filter(|was| (was.len, was.modified) == (file.len, file.modified));
        file.identity = same.and_then(|was| was.identity);
        changes.remove(&path);
        fresh.insert(path, file);
    }
    let sized = scan
        .files
        .iter()
        .filter(|_| !gained.is_empty())
        .filter(|(_, file)| file.identity.is_none() && gained.contains(&file.len));
    for (path, file) in sized {
        if !fresh.contains_key(path) && !changes.contains_key(path) {
            fresh.insert(path.clone(), *file);
        }
    }
    let mut needed = Vec::new();
    for (path, mut file) in fresh {
        if file.identity.is_none() {
            let naming = links.naming(&names.key(path.as_str()));
            let said = naming.flat_map(|entity| {
                let written = facts_of(facts, &entity).iter();
                written.map(|written| &written.value)
            });
            let said: Vec<&FileFact> = said.filter(|fact| fact.path == path).collect();
            let same = said
                .iter()
                .rev()
                .find(|fact| (fact.len, fact.modified) == (file.len, file.modified));
            file.identity = same.map(|fact| fact.identity);
            if file.identity.is_none() && links.has_length(file.len) {
                needed.push((path.clone(), file.len));
            }
        }
        if scan.files.get(&path) != Some(&file) {
            changes.insert(path, Some(file));
        }
    }
    (changes, needed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{ExactNames, Random, SeededRandom};
    use crate::ids::EntryHash;

    /// Folds ASCII case.
    struct Folding;

    impl Names for Folding {
        fn key(&self, path: &str) -> String {
            path.to_lowercase()
        }
    }

    const PLACES: [&str; 8] = ["a", "A", "b", "c", "d/e", "d/E", "d", "f/g"];

    /// Contents by identity and length: two of one length, two of another.
    const CONTENTS: [(u128, u64); 4] = [(1, 10), (2, 10), (3, 20), (4, 20)];

    struct World {
        random: SeededRandom,
        entry: u128,
    }

    impl World {
        fn below(&mut self, n: usize) -> usize {
            (self.random.next_u128() % n as u128) as usize
        }

        fn path(&mut self) -> RelPath {
            RelPath::new(PLACES[self.below(PLACES.len())]).unwrap()
        }

        fn facts(&mut self) -> Box<[Fact]> {
            (0..self.below(3))
                .map(|_| {
                    let (identity, len) = CONTENTS[self.below(CONTENTS.len())];
                    self.entry += 1;
                    Fact {
                        value: FileFact {
                            path: self.path(),
                            identity: Identity::from_u128(identity),
                            len,
                            modified: Some(self.below(2) as u64),
                        },
                        entry: EntryHash::from_u128(self.entry),
                    }
                })
                .collect()
        }

        fn file(&mut self) -> Option<Scanned> {
            if self.below(3) == 0 {
                return None;
            }
            let (identity, len) = CONTENTS[self.below(CONTENTS.len())];
            Some(Scanned {
                len,
                modified: Some(self.below(2) as u64),
                identity: (self.below(4) > 0).then_some(Identity::from_u128(identity)),
            })
        }

        fn world(&mut self) -> (Facts, Scan) {
            let mut facts = Facts::new();
            for entity in 1..=self.below(7) as u128 {
                let written = self.facts();
                if !written.is_empty() {
                    facts.insert(EntityId::from_u128(entity), written);
                }
            }
            let mut scan = Scan::default();
            for place in PLACES {
                if let Some(file) = self.file() {
                    scan.files.insert(RelPath::new(place).unwrap(), file);
                }
            }
            (facts, scan)
        }
    }

    #[test]
    fn binding_what_changes_reach_binds_what_binding_everything_binds() {
        for seed in 0..1500 {
            let mut world = World {
                random: SeededRandom::new(seed),
                entry: 0,
            };
            let names: &dyn Names = match seed % 2 {
                0 => &ExactNames,
                _ => &Folding,
            };
            let (mut facts, mut scan) = world.world();
            let mut reach = Reach::default();
            let (mut bindings, mut pins) =
                binding::bind_known(&facts, &scan, names, &Unscanned::default());
            reach.bound(Some(Links::of(&facts, &scan, names)));
            for step in 0..12 {
                for _ in 0..1 + world.below(3) {
                    match world.below(2) {
                        0 => {
                            let entity = EntityId::from_u128(1 + world.below(8) as u128);
                            let now = world.facts();
                            reach.refile(&mut facts, &scan, names, entity, now);
                        }
                        _ => {
                            let (path, now) = (world.path(), world.file());
                            reach.rescan(&mut scan, &facts, names, path, now);
                        }
                    }
                }
                let seeds = reach.take().expect("followed link by link");
                rebind(
                    &reach,
                    seeds,
                    &facts,
                    &scan,
                    names,
                    &mut bindings,
                    &mut pins,
                )
                .expect("small worlds rebind what changes reach");
                let whole = binding::bind_known(&facts, &scan, names, &Unscanned::default());
                assert_eq!(
                    (&bindings, &pins),
                    (&whole.0, &whole.1),
                    "seed {seed} step {step}: {facts:?} {scan:?}"
                );
                assert_eq!(
                    reach.links.as_ref(),
                    Some(&Links::of(&facts, &scan, names)),
                    "seed {seed} step {step}: links"
                );
            }
        }
    }

    #[test]
    fn resolving_what_was_relisted_resolves_what_resolving_every_file_does() {
        for seed in 0..1500 {
            let mut world = World {
                random: SeededRandom::new(seed),
                entry: 0,
            };
            let names: &dyn Names = match seed % 2 {
                0 => &ExactNames,
                _ => &Folding,
            };
            let (mut facts, mut previous) = world.world();
            for file in previous.files.values_mut() {
                file.identity = None;
            }
            let earlier = previous.clone();
            let needed = binding::resolve(&mut previous, &facts, &earlier);
            for (path, len) in needed {
                let identity = CONTENTS.iter().find(|(_, l)| *l == len).unwrap().0;
                let file = previous.files.get_mut(&path).unwrap();
                file.identity = Some(Identity::from_u128(identity + world.below(2) as u128));
            }
            let mut reach = Reach::default();
            reach.bound(Some(Links::of(&facts, &previous, names)));
            for _ in 0..world.below(3) {
                let entity = EntityId::from_u128(1 + world.below(8) as u128);
                let now = world.facts();
                reach.refile(&mut facts, &previous, names, entity, now);
            }
            let relisted: Vec<RelPath> = (0..world.below(3)).map(|_| world.path()).collect();
            let mut found = Scan::default();
            for place in PLACES {
                let path = RelPath::new(place).unwrap();
                if relisted.iter().any(|dir| path.starts_with(dir)) {
                    if let Some(mut file) = world.file() {
                        file.identity = None;
                        found.files.insert(path, file);
                    }
                }
            }

            let mut whole = previous.clone();
            whole
                .files
                .retain(|path, _| !relisted.iter().any(|dir| path.starts_with(dir)));
            whole.files.extend(found.files.clone());
            let whole_needed = binding::resolve(&mut whole, &facts, &previous);

            let links = reach.links.as_ref().unwrap();
            let gone = binding::under_any(&previous, &relisted);
            let (changes, some_needed) =
                resolve(&previous, &facts, links, names, gone, found, &reach.gained);
            let mut some = previous.clone();
            for (path, file) in changes {
                match file {
                    Some(file) => some.files.insert(path, file),
                    None => some.files.remove(&path),
                };
            }
            assert_eq!(
                (some, some_needed),
                (whole, whole_needed),
                "seed {seed}: relisted {relisted:?}, {facts:?}"
            );
        }
    }
}
