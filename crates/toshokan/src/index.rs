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

impl Index {
    /// The index of a view of `folded` and `bindings`, built at once.
    pub fn of(folded: &Folded, bindings: &Bindings) -> Self {
        let mut holding: BTreeMap<(KeyKind, &str), Holding> = BTreeMap::new();
        let (mut paths, mut conflicts) = (Vec::new(), Vec::new());
        for (entity, shown) in folded.each_shown() {
            let entries = Entries::from(entity, shown, bindings.bound.get(&entity));
            for under in entries.values.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
                let held = holding.entry((under[0].0, under[0].1)).or_default();
                held.holders.push(entity);
                for (_, _, value) in under {
                    let named = held.named.len();
                    let at = *held.named.entry(value.as_str()).or_insert(named);
                    held.held.push((at, entity));
                }
            }
            if let Some((path, state)) = entries.file {
                paths.push(((path.clone(), entity), state));
            }
            conflicts.extend(entries.conflicts.into_iter().map(|conflict| (conflict, ())));
        }
        let keys = holding.into_iter().map(|((kind, key), held)| {
            let index = KeyIndex::of(held);
            ((kind, key.to_owned()), Arc::new(index))
        });
        Self {
            keys: keys.collect(),
            paths: paths.into_iter().collect(),
            conflicts: conflicts.into_iter().collect(),
        }
    }

    /// Moves the entries of each entity that changed between `before`, the folded
    /// state and bindings this index holds, and `after`. Builds the index again
    /// when most entities changed.
    pub fn update(&mut self, before: (&Folded, &Bindings), after: (&Folded, &Bindings)) {
        let mut entities = after.0.changed(before.0);
        if !std::ptr::eq(before.1, after.1) {
            entities.extend(rebound(&before.1.bound, &after.1.bound));
            entities.sort_unstable();
            entities.dedup();
        }
        if entities.len() > REBUILT.max(after.0.entity_count() / 4) {
            *self = Self::of(after.0, after.1);
            return;
        }
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
struct Holding<'a> {
    /// Each value, numbered in the order first held.
    named: HashMap<&'a str, usize>,
    /// Each value held, by number, with its entity.
    held: Vec<(usize, EntityId)>,
    holders: Vec<EntityId>,
}

impl KeyIndex {
    fn of(gathered: Holding) -> Self {
        let mut texts: Vec<(&str, usize)> = gathered.named.into_iter().collect();
        texts.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let mut rank = vec![0; texts.len()];
        for (at, (_, named)) in texts.iter().enumerate() {
            rank[*named] = at;
        }
        let mut each: Vec<Vec<EntityId>> = vec![Vec::new(); texts.len()];
        for (named, entity) in gathered.held {
            each[rank[named]].push(entity);
        }
        let mut counts = Vec::with_capacity(texts.len());
        let mut holding = Vec::new();
        for ((value, _), entities) in texts.into_iter().zip(each) {
            let text = Text::from(value);
            counts.push((Arc::clone(&text), entities.len()));
            holding.extend(
                entities
                    .into_iter()
                    .map(|entity| ((Arc::clone(&text), entity), ())),
            );
        }
        Self {
            holding: holding.into_iter().collect(),
            counts: counts.into_iter().collect(),
            holders: gathered
                .holders
                .into_iter()
                .map(|entity| (entity, ()))
                .collect(),
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
fn rebound(
    was: &BTreeMap<EntityId, crate::view::FileRef>,
    now: &BTreeMap<EntityId, crate::view::FileRef>,
) -> Vec<EntityId> {
    let mut changed = Vec::new();
    let mut old = was.iter().peekable();
    for (entity, file) in now {
        while let Some((gone, _)) = old.next_if(|(id, _)| *id < entity) {
            changed.push(*gone);
        }
        match old.next_if(|(id, _)| *id == entity) {
            Some((_, before)) if before == file => {}
            _ => changed.push(*entity),
        }
    }
    changed.extend(old.map(|(gone, _)| *gone));
    changed
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
}
