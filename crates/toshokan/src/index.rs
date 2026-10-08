//! What a view answers without scanning every entity: the entities holding each
//! value of each key, the entities by the path of their file, and the conflicts.
//!
//! An index is shared with the views made from it and changed only where entities
//! changed: [`Index::update`] compares each entity whose state or binding differs
//! from the last view's and moves its entries.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::binding::Bindings;
use crate::cow::CowMap;
use crate::ids::EntityId;
use crate::merge::{Folded, Shown};
use crate::path::RelPath;
use crate::schema::{KeyKind, Raw};
use crate::view::{Conflicted, FileRef, FileState};

/// A value's JSON text, shared by every entry naming it.
type Text = Arc<str>;

/// The values one entity held under a key, and those it holds.
type Moved<'a> = (Vec<&'a Raw>, Vec<&'a Raw>);

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Index {
    keys: BTreeMap<(KeyKind, String), Arc<KeyIndex>>,
    /// Every entity with a file, by the path its file is at or was logged at.
    paths: CowMap<(RelPath, EntityId), FileState>,
    conflicts: CowMap<Conflicted, ()>,
}

/// One key's values.
#[derive(Clone, Default, PartialEq, Debug)]
struct KeyIndex {
    /// Each value with each entity holding it.
    holding: CowMap<(Text, EntityId), ()>,
    /// Each value with how many entities hold it.
    counts: CowMap<Text, usize>,
    /// The entities holding any value.
    holders: CowMap<EntityId, ()>,
}

/// What one entity puts in the index.
#[derive(PartialEq, Default)]
struct Entries<'a> {
    /// Sorted.
    values: Vec<(KeyKind, &'a str, &'a Raw)>,
    file: Option<(&'a RelPath, FileState)>,
    conflicts: Vec<Conflicted>,
}

/// An index being built a slice of entities at a time, as [`Index::of`] builds
/// it.
#[derive(Default)]
pub(crate) struct Indexing {
    stage: Stage,
    holding: BTreeMap<(KeyKind, String), Holding>,
    keys: BTreeMap<(KeyKind, String), Arc<KeyIndex>>,
    paths: Vec<((RelPath, EntityId), FileState)>,
    conflicts: Vec<(Conflicted, ())>,
}

#[derive(Default)]
enum Stage {
    /// Taking entities, from the first.
    #[default]
    Entities,
    /// Taking entities, after this one.
    After(EntityId),
    /// Building the index of each key gathered.
    Keys,
    Done,
}

impl Indexing {
    /// Takes the next `slice` entities of `folded`, bound as `bindings` says, or
    /// once every one is taken builds the indexes of keys holding about `slice`
    /// values; true once every index is built. Each step must be given the same
    /// state.
    pub(crate) fn step(&mut self, folded: &Folded, bindings: &Bindings, slice: usize) -> bool {
        match self.stage {
            Stage::Entities => self.take(folded, bindings, None, slice),
            Stage::After(after) => self.take(folded, bindings, Some(after), slice),
            Stage::Keys => self.build(slice),
            Stage::Done => {}
        }
        matches!(self.stage, Stage::Done)
    }

    fn take(
        &mut self,
        folded: &Folded,
        bindings: &Bindings,
        after: Option<EntityId>,
        slice: usize,
    ) {
        let mut entities = folded.each_shown_after(after);
        for (entity, shown) in entities.by_ref().take(slice) {
            self.stage = Stage::After(entity);
            let Some(shown) = shown else {
                continue;
            };
            let entries = Entries::from(entity, shown, bindings.bound.get(&entity));
            for under in entries.values.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
                let (kind, key) = (under[0].0, under[0].1);
                let held = self.holding.entry((kind, key.to_owned())).or_default();
                held.holders.push(entity);
                for (_, _, value) in under {
                    let named = held.named.len();
                    let at = match held.named.get(value.as_str()) {
                        Some(at) => *at,
                        None => *held
                            .named
                            .entry(Text::from(value.as_str()))
                            .or_insert(named),
                    };
                    held.held.push((at, entity));
                }
            }
            if let Some((path, state)) = entries.file {
                self.paths.push(((path.clone(), entity), state));
            }
            let conflicts = entries.conflicts.into_iter();
            self.conflicts
                .extend(conflicts.map(|conflict| (conflict, ())));
        }
        if entities.next().is_none() {
            self.stage = Stage::Keys;
        }
    }

    /// Builds the indexes of the next keys gathered, until they hold `slice`
    /// values, then of the paths.
    fn build(&mut self, slice: usize) {
        let mut built = 0;
        while built < slice {
            let Some((key, held)) = self.holding.pop_first() else {
                self.paths.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                self.stage = Stage::Done;
                return;
            };
            built += held.held.len();
            self.keys.insert(key, Arc::new(KeyIndex::of(held)));
        }
    }

    /// ⚠️ Before [`Indexing::step`] returns true, an index lacking what is not
    /// built yet.
    pub(crate) fn finish(self) -> Index {
        Index {
            keys: self.keys,
            paths: CowMap::from_sorted(self.paths),
            conflicts: self.conflicts.into_iter().collect(),
        }
    }
}

impl Index {
    /// The index of a view of `folded` and `bindings`, built at once.
    pub fn of(folded: &Folded, bindings: &Bindings) -> Self {
        let mut indexing = Indexing::default();
        while !indexing.step(folded, bindings, usize::MAX) {}
        indexing.finish()
    }

    /// Moves the entries of each entity that changed between `before`, the folded
    /// state and bindings this index holds, and `after`. Builds the index again
    /// when most entities changed.
    pub fn update(&mut self, before: (&Folded, &Bindings), after: (&Folded, &Bindings)) {
        match Self::changed(before, after) {
            Some(entities) => self.move_entries(before, after, entities),
            None => *self = Self::of(after.0, after.1),
        }
    }

    /// The entities whose state or binding differs between `before` and
    /// `after`; `None` when so many do that building the index again costs less
    /// than moving their entries.
    pub fn changed(
        before: (&Folded, &Bindings),
        after: (&Folded, &Bindings),
    ) -> Option<Vec<EntityId>> {
        let mut entities = after.0.changed(before.0);
        if !std::ptr::eq(before.1, after.1) {
            entities.extend(rebound(&before.1.bound, &after.1.bound));
            entities.sort_unstable();
            entities.dedup();
        }
        (entities.len() <= REBUILT.max(after.0.entity_count() / 4)).then_some(entities)
    }

    /// Moves the entries of `entities`, which [`Index::changed`] gave, from what
    /// they were in `before` to what they are in `after`.
    pub fn move_entries(
        &mut self,
        before: (&Folded, &Bindings),
        after: (&Folded, &Bindings),
        entities: Vec<EntityId>,
    ) {
        for entity in entities {
            let was = Entries::of(before, entity);
            let now = Entries::of(after, entity);
            if was != now {
                self.replace(entity, was, now);
            }
        }
    }

    fn replace(&mut self, entity: EntityId, was: Entries, now: Entries) {
        let mut keys: BTreeMap<(KeyKind, &str), Moved> = BTreeMap::new();
        for (kind, key, value) in &was.values {
            keys.entry((*kind, key)).or_default().0.push(value);
        }
        for (kind, key, value) in &now.values {
            keys.entry((*kind, key)).or_default().1.push(value);
        }
        for ((kind, key), (was, now)) in keys {
            if was == now {
                continue;
            }
            let slot = self.keys.entry((kind, key.to_owned())).or_default();
            let index = Arc::make_mut(slot);
            index.replace(entity, &was, &now);
            if index.holders.is_empty() {
                self.keys.remove(&(kind, key.to_owned()));
            }
        }
        if was.file != now.file {
            if let Some((path, _)) = was.file {
                self.paths.remove(&(path.clone(), entity));
            }
            if let Some((path, state)) = now.file {
                self.paths.insert((path.clone(), entity), state);
            }
        }
        if was.conflicts != now.conflicts {
            for conflict in was.conflicts {
                self.conflicts.remove(&conflict);
            }
            for conflict in now.conflicts {
                self.conflicts.insert(conflict, ());
            }
        }
    }

    /// The entities holding `value` under `key`, by id.
    pub fn find(&self, kind: KeyKind, key: &str, value: &Raw) -> Vec<EntityId> {
        let Some(index) = self.keys.get(&(kind, key.to_owned())) else {
            return Vec::new();
        };
        index.holding_of(value.as_str()).collect()
    }

    /// The entities holding any value under `key`, by id.
    pub fn with(&self, kind: KeyKind, key: &str) -> Vec<EntityId> {
        let Some(index) = self.keys.get(&(kind, key.to_owned())) else {
            return Vec::new();
        };
        index.holders.iter().map(|(entity, _)| *entity).collect()
    }

    /// Each value held under `key`, by its text, with how many entities hold it.
    pub fn values(&self, kind: KeyKind, key: &str) -> Vec<(&str, usize)> {
        let Some(index) = self.keys.get(&(kind, key.to_owned())) else {
            return Vec::new();
        };
        index
            .counts
            .iter()
            .map(|(text, count)| (&**text, *count))
            .collect()
    }

    /// The entities holding each of `values`, which are texts held under `key`, by
    /// id and once.
    pub fn holding<'a>(
        &self,
        kind: KeyKind,
        key: &str,
        values: impl Iterator<Item = &'a str>,
    ) -> Vec<EntityId> {
        let Some(index) = self.keys.get(&(kind, key.to_owned())) else {
            return Vec::new();
        };
        let mut entities: Vec<EntityId> = values.flat_map(|text| index.holding_of(text)).collect();
        entities.sort_unstable();
        entities.dedup();
        entities
    }

    /// Each entity whose file is at `dir` or under it, with its file's state, in
    /// path order.
    pub fn under<'a>(
        &'a self,
        dir: &'a RelPath,
    ) -> Box<dyn Iterator<Item = (&'a RelPath, EntityId, FileState)> + 'a> {
        if dir.is_root() {
            return Box::new(
                self.paths
                    .iter()
                    .map(|((path, entity), state)| (path, *entity, *state)),
            );
        }
        let inside = format!("{dir}/");
        let below = self.paths.seek(move |(path, _)| path.as_str().cmp(&inside));
        let below = below.take_while(move |((path, _), _)| path.starts_with(dir));
        let entries = self.at_entries(dir).chain(below);
        Box::new(entries.map(|((path, entity), state)| (path, *entity, *state)))
    }

    /// The entities whose file is at `path` or was logged there, by id.
    pub fn at<'a>(&'a self, path: &'a RelPath) -> impl Iterator<Item = (EntityId, FileState)> + 'a {
        self.at_entries(path)
            .map(|((_, entity), state)| (*entity, *state))
    }

    fn at_entries<'a>(
        &'a self,
        path: &'a RelPath,
    ) -> impl Iterator<Item = &'a ((RelPath, EntityId), FileState)> + 'a {
        let found = self.paths.seek(move |(at, _)| at.cmp(path));
        found.take_while(move |((at, _), _)| at == path)
    }

    pub fn conflicts(&self) -> Vec<Conflicted> {
        self.conflicts
            .iter()
            .map(|(conflict, _)| conflict.clone())
            .collect()
    }
}

/// One key's values as [`Index::of`] gathers them, in order of entity.
#[derive(Default)]
struct Holding {
    /// Each value, numbered in the order first held.
    named: HashMap<Text, usize>,
    /// Each value held, by number, with its entity.
    held: Vec<(usize, EntityId)>,
    holders: Vec<EntityId>,
}

impl KeyIndex {
    /// The index of what was gathered in order of entity, so that each value's
    /// holders and the holders of any value are in order already.
    fn of(gathered: Holding) -> Self {
        let mut texts: Vec<(Text, usize)> = gathered.named.into_iter().collect();
        texts.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut rank = vec![0; texts.len()];
        for (at, (_, named)) in texts.iter().enumerate() {
            rank[*named] = at;
        }
        let mut ends = vec![0; texts.len()];
        for (named, _) in &gathered.held {
            ends[rank[*named]] += 1;
        }
        let mut counts = Vec::with_capacity(texts.len());
        let mut start = 0;
        for ((text, _), end) in texts.iter().zip(&mut ends) {
            counts.push((Arc::clone(text), *end));
            start += *end;
            *end = start;
        }
        let mut order: Vec<Option<EntityId>> = vec![None; gathered.held.len()];
        for (named, entity) in gathered.held.into_iter().rev() {
            let end = &mut ends[rank[named]];
            *end -= 1;
            order[*end] = Some(entity);
        }
        let mut holding = Vec::with_capacity(order.len());
        let mut placed = order.into_iter().flatten();
        for (text, count) in &counts {
            let entities = placed.by_ref().take(*count);
            holding.extend(entities.map(|entity| ((Arc::clone(text), entity), ())));
        }
        let holders = gathered.holders.into_iter().map(|entity| (entity, ()));
        Self {
            holding: CowMap::from_sorted(holding),
            counts: CowMap::from_sorted(counts),
            holders: CowMap::from_sorted(holders.collect()),
        }
    }

    fn holding_of<'a>(&'a self, text: &'a str) -> impl Iterator<Item = EntityId> + 'a {
        let found = self.holding.seek(move |(held, _)| (**held).cmp(text));
        found
            .take_while(move |((held, _), _)| **held == *text)
            .map(|((_, entity), _)| *entity)
    }

    /// Moves `entity` from holding `was` to holding `now`, both sorted.
    fn replace(&mut self, entity: EntityId, was: &[&Raw], now: &[&Raw]) {
        for value in was.iter().filter(|value| now.binary_search(value).is_err()) {
            self.remove(entity, value.as_str());
        }
        for value in now.iter().filter(|value| was.binary_search(value).is_err()) {
            self.add(entity, value.as_str());
        }
        match (was.is_empty(), now.is_empty()) {
            (true, false) => {
                self.holders.insert(entity, ());
            }
            (false, true) => {
                self.holders.remove(&entity);
            }
            _ => {}
        }
    }

    fn add(&mut self, entity: EntityId, value: &str) {
        let (text, count) = match self.counts.get_by(|text| (**text).cmp(value)) {
            Some((text, count)) => (Arc::clone(text), count + 1),
            None => (Text::from(value), 1),
        };
        self.counts.insert(Arc::clone(&text), count);
        self.holding.insert((text, entity), ());
    }

    fn remove(&mut self, entity: EntityId, value: &str) {
        let Some((text, count)) = self.counts.get_by(|text| (**text).cmp(value)) else {
            return;
        };
        let (text, count) = (Arc::clone(text), *count);
        self.holding.remove(&(Arc::clone(&text), entity));
        match count {
            1 => self.counts.remove(&text),
            _ => self.counts.insert(text, count - 1),
        };
    }
}

impl<'a> Entries<'a> {
    fn of((folded, bindings): (&'a Folded, &'a Bindings), entity: EntityId) -> Self {
        match folded.shown(entity) {
            Some(shown) => Self::from(entity, shown, bindings.bound.get(&entity)),
            None => Self::default(),
        }
    }

    /// The entries of `entity`, which shows `shown` and is bound as `bound` says.
    fn from(entity: EntityId, shown: Shown<'a>, bound: Option<&'a FileRef>) -> Self {
        let registers = shown
            .values
            .iter()
            .filter(|(kind, ..)| *kind == KeyKind::Register);
        let mut conflicts: Vec<Conflicted> = Vec::new();
        let mut last: Option<&str> = None;
        for (_, key, _) in registers {
            let repeated = last == Some(key);
            let noted = conflicts.last().is_some_and(
                |conflict| matches!(conflict, Conflicted::Field { key: noted, .. } if noted == key),
            );
            if repeated && !noted {
                conflicts.push(Conflicted::Field {
                    entity,
                    key: (*key).to_owned(),
                });
            }
            last = Some(key);
        }
        if shown.deletion_conflicted {
            conflicts.push(Conflicted::Existence { entity });
        }
        if shown.file_conflicted {
            conflicts.push(Conflicted::File { entity });
        }
        let file = match bound {
            Some(file) => Some((&file.path, file.state)),
            None => shown.logged.map(|path| (path, FileState::Missing)),
        };
        Self {
            values: shown.values,
            file,
            conflicts,
        }
    }
}

/// How many changed entities an update moves one at a time, at least.
const REBUILT: usize = 1024;

/// The entities whose binding differs between `was` and `now`.
fn rebound(was: &CowMap<EntityId, FileRef>, now: &CowMap<EntityId, FileRef>) -> Vec<EntityId> {
    let pairs = now.diff(was);
    let differ = pairs.filter_map(|pair| match pair {
        (Some((_, now)), Some((_, was))) if now == was => None,
        (Some((entity, _)), _) | (None, Some((entity, _))) => Some(*entity),
        (None, None) => None,
    });
    differ.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Random, SeededRandom};
    use crate::ids::{EntryHash, Hlc, Identity, WriterId};
    use crate::line::Line;
    use crate::log::{Entry, EntryKind, FileFact, Logged, Op};
    use crate::view::FileRef;

    fn below(random: &mut SeededRandom, n: usize) -> usize {
        (random.next_u128() % n as u128) as usize
    }

    /// Up to two earlier entries, for an op to replace or name.
    fn some(random: &mut SeededRandom, hashes: &[EntryHash]) -> Vec<EntryHash> {
        (0..below(random, 3))
            .filter_map(|_| hashes.get(below(random, hashes.len().max(1))).copied())
            .collect()
    }

    fn op(random: &mut SeededRandom, hashes: &[EntryHash]) -> Op {
        let entity = EntityId::from_u128(below(random, 12) as u128);
        let value = Raw::of(&below(random, 4)).unwrap();
        let path = RelPath::new(["a", "b", "b/c", "b-c"][below(random, 4)]).unwrap();
        let key = ["k", "j"][below(random, 2)].to_owned();
        match below(random, 7) {
            0 | 1 => Op::Create {
                entity,
                replaces: some(random, hashes),
            },
            2 => Op::Delete {
                entity,
                replaces: some(random, hashes),
                observed: some(random, hashes),
            },
            3 => Op::Write {
                entity,
                key,
                value: (below(random, 4) > 0).then_some(value),
                replaces: some(random, hashes),
            },
            4 => Op::Add { entity, key, value },
            5 => Op::Remove {
                entity,
                key,
                value,
                tags: some(random, hashes),
            },
            _ => Op::File {
                entity,
                file: Some(FileFact {
                    path,
                    identity: Identity::from_u128(below(random, 2) as u128),
                    len: 1,
                    modified: None,
                }),
                replaces: some(random, hashes),
            },
        }
    }

    fn bindings(random: &mut SeededRandom, folded: &Folded) -> Bindings {
        let states = [
            FileState::InSync,
            FileState::ChangedOutside,
            FileState::Missing,
        ];
        let bound = folded.files().into_keys().filter_map(|entity| {
            let path = RelPath::new(["a", "b/c", "d"][below(random, 3)]).unwrap();
            let state = states[below(random, 3)];
            (below(random, 3) > 0).then_some((entity, FileRef { path, state }))
        });
        Bindings {
            bound: bound.collect(),
            ..Bindings::default()
        }
    }

    #[test]
    fn an_index_kept_up_entry_by_entry_equals_one_built_at_once() {
        for seed in 0..200 {
            let mut random = SeededRandom::new(seed);
            let mut folded = Folded::default();
            let mut bound = Bindings::default();
            let mut index = Index::default();
            let mut hashes = Vec::new();
            for n in 0..40 {
                let ops = (0..1 + below(&mut random, 3))
                    .map(|_| op(&mut random, &hashes))
                    .collect();
                let line = Line::seal(format!(
                    r#"{{"prev":"{}","seed":{seed},"n":{n}}}"#,
                    EntryHash::ZERO
                ))
                .unwrap();
                let at = Hlc {
                    wall_ms: n,
                    counter: 0,
                };
                let kind = EntryKind::Intent(Logged {
                    label: String::new(),
                    ops,
                    displaced: Vec::new(),
                    reverses: None,
                });
                let entry = Entry::new(line, at, kind, false);
                hashes.push(entry.hash());
                let mut next = folded.clone();
                next.apply(
                    WriterId::from_u128(1 + below(&mut random, 2) as u128),
                    &entry,
                );
                let rebound = match below(&mut random, 2) {
                    0 => bindings(&mut random, &next),
                    _ => bound.clone(),
                };
                index.update((&folded, &bound), (&next, &rebound));
                assert_eq!(index, Index::of(&next, &rebound), "seed {seed}, entry {n}");
                (folded, bound) = (next, rebound);
            }
        }
    }

    #[test]
    fn an_index_built_a_slice_at_a_time_equals_one_built_at_once() {
        for seed in 0..100 {
            let mut random = SeededRandom::new(seed);
            let mut folded = Folded::default();
            let mut hashes = Vec::new();
            for n in 0..40 {
                let ops = (0..1 + below(&mut random, 3))
                    .map(|_| op(&mut random, &hashes))
                    .collect();
                let line = Line::seal(format!(r#"{{"prev":"{}","n":{n}}}"#, EntryHash::ZERO));
                let kind = EntryKind::Intent(Logged {
                    label: String::new(),
                    ops,
                    displaced: Vec::new(),
                    reverses: None,
                });
                let at = Hlc {
                    wall_ms: n,
                    counter: 0,
                };
                let entry = Entry::new(line.unwrap(), at, kind, false);
                hashes.push(entry.hash());
                folded.apply(WriterId::from_u128(1), &entry);
            }
            let bound = bindings(&mut random, &folded);
            let slice = 1 + below(&mut random, 5);
            let mut indexing = Indexing::default();
            while !indexing.step(&folded, &bound, slice) {}
            assert_eq!(
                indexing.finish(),
                Index::of(&folded, &bound),
                "seed {seed}, {slice}-entity slices"
            );
        }
    }
}
