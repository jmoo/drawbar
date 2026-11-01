//! The merged state: every live write, by entity and field, and what readers
//! show of them.
//!
//! Which writes are live is [`crate::derive`]'s business. Here a register shows
//! the values of its live writes, several of them a conflict; a set holds each
//! value with a live add; an entity exists while a create is live, and is in a
//! deletion conflict while a delete is live beside a live create or a live field
//! write that holds something. An entity with no live existence write, which
//! only a forgotten basis or a lost write leaves, shows its live fields.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::binding::{Fact, Facts};
use crate::contribution::{FieldKey, Name, Trashed, Value};
use crate::ids::{EntityId, EntryHash, Hlc, Nonce, WriterId};
use crate::log::{FileFact, Ref};
use crate::path::RelPath;
use crate::report::TrashItem;
use crate::schema::{KeyKind, Raw, Written};

/// A live write: who wrote it, where, when, and what.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Live {
    pub at: Ref,
    pub stamp: Hlc,
    pub field: FieldKey,
    pub value: Value,
}

/// One live write of one field, as queries give it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Write<V> {
    pub entry: EntryHash,
    pub by: WriterId,
    pub n: u64,
    pub at: Hlc,
    pub value: V,
}

impl<V> Write<V> {
    /// The reference that names this write.
    pub fn named(&self) -> Ref {
        Ref {
            writer: self.by,
            n: self.n,
            hash: self.entry,
        }
    }
}

/// One write of an entity's existence register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Exists {
    Created,
    Deleted,
}

/// The part of an entity one write changed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Part {
    Created,
    Deleted,
    Field(String),
    File,
}

/// A live write as an entity's state holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Held<V> {
    at: Ref,
    stamp: Hlc,
    value: V,
}

impl<V: Clone> Held<V> {
    fn write(&self) -> Write<V> {
        Write {
            entry: self.at.hash,
            by: self.at.writer,
            n: self.at.n,
            at: self.stamp,
            value: self.value.clone(),
        }
    }
}

/// The live writes of one field, oldest first.
type Writes<V> = Vec<Held<V>>;

fn writes<V>(writes: Option<&Writes<V>>) -> &[Held<V>] {
    writes.map_or(&[], Vec::as_slice)
}

/// A key's entry in each of two maps, as [`side_by_side`] gives it.
pub(crate) type Paired<'a, K, V> = (Option<(&'a K, &'a V)>, Option<(&'a K, &'a V)>);

/// Each key `now` or `was` holds, with its entry in each, in order of key.
pub(crate) fn side_by_side<'a, K: Ord, V>(
    now: &'a BTreeMap<K, V>,
    was: &'a BTreeMap<K, V>,
) -> impl Iterator<Item = Paired<'a, K, V>> {
    let (mut now, mut was) = (now.iter().peekable(), was.iter().peekable());
    std::iter::from_fn(move || {
        let order = match (now.peek(), was.peek()) {
            (None, None) => return None,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some((now, _)), Some((was, _))) => now.cmp(was),
        };
        let now = (order != Ordering::Greater).then(|| now.next()).flatten();
        let was = (order != Ordering::Less).then(|| was.next()).flatten();
        Some((now, was))
    })
}

/// The live writes in `now` that `was` does not hold, marked gained, or, where
/// there are none, those in `was` that `now` does not hold, marked lost.
fn changed<'a, V>(now: &'a [Held<V>], was: &'a [Held<V>]) -> Vec<(&'a Held<V>, bool)> {
    let missing = |writes: &'a [Held<V>], held: &'a [Held<V>]| {
        writes
            .iter()
            .filter(move |write| !held.iter().any(|kept| kept.at == write.at))
    };
    let gained: Vec<_> = missing(now, was).map(|write| (write, true)).collect();
    match gained.is_empty() {
        false => gained,
        true => missing(was, now).map(|write| (write, false)).collect(),
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
struct EntityState {
    existence: Writes<bool>,
    registers: BTreeMap<Name, Writes<Option<Raw>>>,
    /// Per key, per member, its live adds and removes.
    sets: BTreeMap<Name, BTreeMap<Raw, Writes<bool>>>,
    file: Writes<Option<FileFact>>,
}

/// The merged state. A clone shares each entity's state with the original until
/// one of them changes it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Folded {
    entities: BTreeMap<EntityId, Arc<EntityState>>,
    trash: BTreeMap<(WriterId, Nonce), Trashed>,
    settled: BTreeSet<(WriterId, Nonce)>,
    unknown: BTreeSet<(EntryHash, Raw)>,
    extended: BTreeSet<(EntryHash, Raw)>,
}

/// What every writer keeps beside its writes, merged by union.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Beside {
    pub trash: BTreeMap<(WriterId, Nonce), Trashed>,
    pub settled: BTreeSet<(WriterId, Nonce)>,
    pub unknown: BTreeSet<(EntryHash, Raw)>,
    pub extended: BTreeSet<(EntryHash, Raw)>,
}

/// What an entity shows that a view indexes.
#[derive(Clone, PartialEq, Debug)]
pub struct Shown<'a> {
    /// Each value it holds under each key: of a register, the values of its
    /// live writes; of a set, its members. Sorted and once each.
    pub values: Vec<(KeyKind, &'a str, &'a Raw)>,
    pub deletion_conflicted: bool,
    /// Whether its live file writes that name a file name different ones.
    pub file_conflicted: bool,
    /// The path of its latest live file write that names a file.
    pub logged: Option<&'a RelPath>,
}

fn oldest_first<V>(writes: &mut [Held<V>]) {
    writes.sort_by_key(|held| (held.stamp, held.at.writer, held.at.hash));
}

impl EntityState {
    /// Whether some live field write holds something: a value, an add, a file.
    fn fields_live(&self) -> bool {
        let registers = self
            .registers
            .values()
            .any(|writes| writes.iter().any(|held| held.value.is_some()));
        let sets = self.sets.values().any(|members| {
            members
                .values()
                .any(|writes| writes.iter().any(|held| held.value))
        });
        let file = self.file.iter().any(|held| held.value.is_some());
        registers || sets || file
    }

    fn created(&self) -> bool {
        self.existence.iter().any(|held| held.value)
    }

    fn deleted(&self) -> bool {
        self.existence.iter().any(|held| !held.value)
    }

    fn deletion_conflicted(&self) -> bool {
        self.deleted() && (self.created() || self.fields_live())
    }

    fn present(&self) -> bool {
        self.created()
            || self.deletion_conflicted()
            || (self.existence.is_empty() && self.fields_live())
    }

    /// The live file writes that say where the file is, oldest first.
    fn file_facts(&self) -> Box<[Fact]> {
        let facts = self.file.iter().filter_map(|held| {
            Some(Fact {
                value: held.value.clone()?,
                entry: held.at,
            })
        });
        facts.collect()
    }

    /// The live file write that names a file, latest by clock, writer and entry.
    fn latest_file(&self) -> Option<&FileFact> {
        let named = self.file.iter().filter_map(|held| {
            Some((
                (held.stamp, held.at.writer, held.at.hash),
                held.value.as_ref()?,
            ))
        });
        named.max_by_key(|(order, _)| *order).map(|(_, file)| file)
    }

    /// Each value a register or set holds.
    fn values(&self) -> Vec<(KeyKind, &str, &Raw)> {
        let registers = self.registers.iter().flat_map(|(key, writes)| {
            let values = writes.iter().filter_map(|held| held.value.as_ref());
            values.map(move |value| (KeyKind::Register, &**key, value))
        });
        let sets = self.sets.iter().flat_map(|(key, members)| {
            let held = members
                .iter()
                .filter(|(_, writes)| writes.iter().any(|held| held.value));
            held.map(move |(value, _)| (KeyKind::Set, &**key, value))
        });
        let mut values: Vec<_> = registers.chain(sets).collect();
        values.sort_unstable();
        values.dedup();
        values
    }

    fn shown(&self) -> Option<Shown<'_>> {
        if !self.present() {
            return None;
        }
        let mut files = self.file.iter().filter_map(|held| held.value.as_ref());
        let first = files.next();
        Some(Shown {
            values: self.values(),
            deletion_conflicted: self.deletion_conflicted(),
            file_conflicted: files.any(|file| first.is_some_and(|first| first != file)),
            logged: self.latest_file().map(|file| &file.path),
        })
    }

    fn insert(&mut self, live: Live) {
        let Live {
            at,
            stamp,
            field,
            value,
        } = live;
        match (field, value) {
            (FieldKey::Exist(_), Value::Exist(created)) => self.existence.push(Held {
                at,
                stamp,
                value: created,
            }),
            (FieldKey::Register(_, key), Value::Register(value)) => {
                let writes = self.registers.entry(key).or_default();
                writes.push(Held { at, stamp, value });
            }
            (FieldKey::Member(_, key, member), Value::Member(added)) => {
                let members = self.sets.entry(key).or_default();
                members.entry(member).or_default().push(Held {
                    at,
                    stamp,
                    value: added,
                });
            }
            (FieldKey::File(_), Value::File { fact, .. }) => self.file.push(Held {
                at,
                stamp,
                value: fact,
            }),
            _ => {}
        }
    }

    /// Its live writes.
    fn lives(&self, entity: EntityId) -> Vec<Live> {
        fn live<V: Clone>(field: FieldKey, held: &Held<V>, value: impl Fn(V) -> Value) -> Live {
            Live {
                at: held.at,
                stamp: held.stamp,
                field,
                value: value(held.value.clone()),
            }
        }
        let mut lives = Vec::new();
        for held in &self.existence {
            lives.push(live(FieldKey::Exist(entity), held, Value::Exist));
        }
        for (key, writes) in self.registers.iter() {
            let field = || FieldKey::Register(entity, key.clone());
            lives.extend(
                writes
                    .iter()
                    .map(|held| live(field(), held, Value::Register)),
            );
        }
        for (key, members) in self.sets.iter() {
            for (member, writes) in members.iter() {
                let field = || FieldKey::Member(entity, key.clone(), member.clone());
                lives.extend(writes.iter().map(|held| live(field(), held, Value::Member)));
            }
        }
        let file = |fact| Value::File { fact, pin: false };
        lives.extend(
            self.file
                .iter()
                .map(|held| live(FieldKey::File(entity), held, file)),
        );
        lives
    }

    fn sort(&mut self) {
        oldest_first(&mut self.existence);
        for writes in self.registers.values_mut() {
            oldest_first(writes);
        }
        for members in self.sets.values_mut() {
            for writes in members.values_mut() {
                oldest_first(writes);
            }
        }
        oldest_first(&mut self.file);
    }
}

impl Folded {
    /// The state of the live writes `live`, with what writers keep beside them.
    pub fn of(live: Vec<Live>, beside: Beside) -> Self {
        let mut entities: BTreeMap<EntityId, EntityState> = BTreeMap::new();
        for write in live {
            entities
                .entry(write.field.entity())
                .or_default()
                .insert(write);
        }
        let entities = entities.into_iter().map(|(entity, mut state)| {
            state.sort();
            (entity, Arc::new(state))
        });
        Self {
            entities: entities.collect(),
            trash: beside.trash,
            settled: beside.settled,
            unknown: beside.unknown,
            extended: beside.extended,
        }
    }

    /// The live writes of `entity`.
    pub(crate) fn lives(&self, entity: EntityId) -> Vec<Live> {
        let state = self.entities.get(&entity);
        state.map(|state| state.lives(entity)).unwrap_or_default()
    }

    /// Makes `live` the live writes of `entity`.
    pub(crate) fn set_lives(&mut self, entity: EntityId, live: Vec<Live>) {
        if live.is_empty() {
            self.entities.remove(&entity);
            return;
        }
        let mut state = EntityState::default();
        for write in live {
            state.insert(write);
        }
        state.sort();
        self.entities.insert(entity, Arc::new(state));
    }

    /// Adds what a writer keeps beside its writes.
    pub(crate) fn keep(&mut self, beside: Beside) {
        self.trash.extend(beside.trash);
        self.settled.extend(beside.settled);
        self.unknown.extend(beside.unknown);
        self.extended.extend(beside.extended);
    }

    /// Whether writing `value` to `field` would change what the state shows.
    pub fn differs(&self, field: &FieldKey, value: &Value) -> bool {
        let state = self.entities.get(&field.entity());
        let present = state.is_some_and(|state| state.present());
        match (field, value) {
            (FieldKey::Exist(_), Value::Exist(created)) => *created != present,
            (FieldKey::Register(entity, key), Value::Register(value)) => {
                let writes = match present {
                    true => self.register_writes(*entity, key),
                    false => Vec::new(),
                };
                let now: BTreeSet<Raw> =
                    writes.into_iter().filter_map(|write| write.value).collect();
                let now: Vec<Raw> = now.into_iter().collect();
                match value {
                    Some(value) => now != [value.clone()],
                    None => !now.is_empty(),
                }
            }
            (FieldKey::Member(entity, key, member), Value::Member(added)) => {
                let held = present && self.members(*entity, key).contains(member);
                held != *added
            }
            (FieldKey::File(entity), Value::File { fact, .. }) => {
                let now: Vec<FileFact> = self
                    .file(*entity)
                    .iter()
                    .map(|fact| fact.value.clone())
                    .collect();
                match fact {
                    Some(fact) => now != [fact.clone()],
                    None => !now.is_empty(),
                }
            }
            _ => true,
        }
    }

    /// Each part of an entity that changed since `before`, with the writer of
    /// each live write `before` does not hold, or, where the part gained none,
    /// of each live write it lost.
    pub fn since(&self, before: &Folded) -> Vec<(EntityId, Part, WriterId)> {
        let mut found = Vec::new();
        let gone = EntityState::default();
        for pair in side_by_side(&self.entities, &before.entities) {
            let (entity, now, old) = match pair {
                (Some((entity, now)), old) => (*entity, &**now, old.map(|(_, old)| &**old)),
                (None, Some((entity, old))) => (*entity, &gone, Some(&**old)),
                (None, None) => continue,
            };
            let old = old.unwrap_or(&gone);
            if std::ptr::eq(old, now) {
                continue;
            }
            for (write, gained) in changed(&now.existence, &old.existence) {
                let part = match write.value == gained {
                    true => Part::Created,
                    false => Part::Deleted,
                };
                found.push((entity, part, write.at.writer));
            }
            let keys: BTreeSet<&Name> = now.registers.keys().chain(old.registers.keys()).collect();
            for key in keys {
                let (now, was) = (now.registers.get(&**key), old.registers.get(&**key));
                for (write, _) in changed(writes(now), writes(was)) {
                    found.push((entity, Part::Field(key.to_string()), write.at.writer));
                }
            }
            let keys: BTreeSet<&Name> = now.sets.keys().chain(old.sets.keys()).collect();
            for key in keys {
                let (now, was) = (now.sets.get(&**key), old.sets.get(&**key));
                let members: BTreeSet<&Raw> = now
                    .iter()
                    .flat_map(|now| now.keys())
                    .chain(was.iter().flat_map(|was| was.keys()))
                    .collect();
                for member in members {
                    let now = now.and_then(|now| now.get(member));
                    let was = was.and_then(|was| was.get(member));
                    for (write, _) in changed(writes(now), writes(was)) {
                        found.push((entity, Part::Field(key.to_string()), write.at.writer));
                    }
                }
            }
            for (write, _) in changed(&now.file, &old.file) {
                found.push((entity, Part::File, write.at.writer));
            }
        }
        found
    }

    /// The entities whose state differs from `before`'s, by id. A state a clone
    /// still shares with `before` is unchanged without being compared.
    pub fn changed(&self, before: &Folded) -> Vec<EntityId> {
        let pairs = side_by_side(&self.entities, &before.entities);
        let differ = pairs.filter_map(|pair| match pair {
            (Some((_, now)), Some((_, was))) if Arc::ptr_eq(was, now) || was == now => None,
            (Some((entity, _)), _) | (None, Some((entity, _))) => Some(*entity),
            (None, None) => None,
        });
        differ.collect()
    }

    /// Takes the state of each entity `before` holds unchanged from it, so a
    /// clone shares it again.
    pub fn share(&mut self, before: &Folded) {
        let same: Vec<(EntityId, Arc<EntityState>)> =
            side_by_side(&self.entities, &before.entities)
                .filter_map(|pair| match pair {
                    (Some((entity, now)), Some((_, was)))
                        if !Arc::ptr_eq(was, now) && was == now =>
                    {
                        Some((*entity, Arc::clone(was)))
                    }
                    _ => None,
                })
                .collect();
        for (entity, was) in same {
            self.entities.insert(entity, was);
        }
    }

    /// What `entity` shows; `None` unless it is shown.
    pub fn shown(&self, entity: EntityId) -> Option<Shown<'_>> {
        self.entities.get(&entity)?.shown()
    }

    /// [`Folded::shown`] of every entity, by id.
    pub fn each_shown(&self) -> impl Iterator<Item = (EntityId, Option<Shown<'_>>)> {
        let entities = self.entities.iter();
        entities.map(|(entity, state)| (*entity, state.shown()))
    }

    /// The path of `entity`'s latest live file write that names a file, while it
    /// is shown.
    pub fn logged_path(&self, entity: EntityId) -> Option<&RelPath> {
        let state = self.entities.get(&entity).filter(|state| state.present())?;
        state.latest_file().map(|file| &file.path)
    }

    /// How many entities the state holds a live write of, shown or not.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Every entity that is shown.
    pub fn entities(&self) -> Vec<EntityId> {
        self.entities
            .iter()
            .filter(|(_, state)| state.present())
            .map(|(entity, _)| *entity)
            .collect()
    }

    /// Whether `entity` is shown: it exists, or its deletion is in conflict.
    pub fn present(&self, entity: EntityId) -> bool {
        self.entities
            .get(&entity)
            .is_some_and(|state| state.present())
    }

    /// Whether a live delete of `entity` stands beside a live create or a live
    /// field write that holds something.
    pub fn deletion_conflicted(&self, entity: EntityId) -> bool {
        self.entities
            .get(&entity)
            .is_some_and(|state| state.deletion_conflicted())
    }

    /// The live writes of `entity`'s existence register, oldest first.
    pub fn existence(&self, entity: EntityId) -> Vec<Write<Exists>> {
        let Some(state) = self.entities.get(&entity) else {
            return Vec::new();
        };
        let exists = |created: bool| {
            if created {
                Exists::Created
            } else {
                Exists::Deleted
            }
        };
        state
            .existence
            .iter()
            .map(|held| {
                let write = held.write();
                Write {
                    entry: write.entry,
                    by: write.by,
                    n: write.n,
                    at: write.at,
                    value: exists(write.value),
                }
            })
            .collect()
    }

    /// The live writes of `entity`'s fields, each with its field: what a delete
    /// now observes.
    pub fn live_writes(&self, entity: EntityId) -> Vec<(FieldKey, Write<()>)> {
        let Some(state) = self.entities.get(&entity) else {
            return Vec::new();
        };
        fn unit<V>(held: &Held<V>) -> Write<()> {
            Write {
                entry: held.at.hash,
                by: held.at.writer,
                n: held.at.n,
                at: held.stamp,
                value: (),
            }
        }
        let mut writes = Vec::new();
        for (key, held) in state.registers.iter() {
            for held in held {
                writes.push((FieldKey::Register(entity, key.clone()), unit(held)));
            }
        }
        for (key, members) in state.sets.iter() {
            for (member, held) in members.iter() {
                for held in held {
                    writes.push((
                        FieldKey::Member(entity, key.clone(), member.clone()),
                        unit(held),
                    ));
                }
            }
        }
        for held in &state.file {
            writes.push((FieldKey::File(entity), unit(held)));
        }
        writes
    }

    /// The live writes of a register that hold a value, as JSON; empty when
    /// unset. A live clear holds no value and is left out.
    pub fn register(&self, entity: EntityId, key: &str) -> Vec<Written<Raw>> {
        let writes = self.register_writes(entity, key).into_iter();
        let values = writes.filter_map(|write| {
            Some(Written {
                value: write.value?,
                by: write.by,
                at: write.at,
                entry: write.entry,
            })
        });
        values.collect()
    }

    /// The live writes of a register, clears included, oldest first: what a new
    /// write replaces.
    pub fn register_writes(&self, entity: EntityId, key: &str) -> Vec<Write<Option<Raw>>> {
        let Some(state) = self.entities.get(&entity) else {
            return Vec::new();
        };
        let writes = state.registers.get(key).into_iter().flatten();
        writes.map(Held::write).collect()
    }

    /// The members of a set as JSON, sorted and without duplicates.
    pub fn members(&self, entity: EntityId, key: &str) -> Vec<Raw> {
        let Some(members) = self
            .entities
            .get(&entity)
            .and_then(|state| state.sets.get(key))
        else {
            return Vec::new();
        };
        let held = members
            .iter()
            .filter(|(_, writes)| writes.iter().any(|held| held.value));
        held.map(|(member, _)| member.clone()).collect()
    }

    /// The live adds and removes of `value` in a set, oldest first: what a new
    /// add or remove of it replaces.
    pub fn tags(&self, entity: EntityId, key: &str, value: &Raw) -> Vec<Write<bool>> {
        let Some(members) = self
            .entities
            .get(&entity)
            .and_then(|state| state.sets.get(key))
        else {
            return Vec::new();
        };
        let writes = members.get(value).into_iter().flatten();
        writes.map(Held::write).collect()
    }

    /// The live writes of every shown entity's file register that say where its
    /// file is. More than one is a conflict.
    pub fn files(&self) -> Facts {
        let shown = self.entities.iter().filter(|(_, state)| state.present());
        let files = shown.map(|(entity, state)| (*entity, state.file_facts()));
        files.filter(|(_, writes)| !writes.is_empty()).collect()
    }

    /// [`Folded::files`] of one entity: empty unless it is shown.
    pub fn file(&self, entity: EntityId) -> Box<[Fact]> {
        self.entities
            .get(&entity)
            .filter(|state| state.present())
            .map(|state| state.file_facts())
            .unwrap_or_default()
    }

    /// The live writes of `entity`'s file register, `None` where a write says it
    /// has no file, oldest first: what a new file write replaces.
    pub fn file_writes(&self, entity: EntityId) -> Vec<Write<Option<FileFact>>> {
        self.entities
            .get(&entity)
            .map(|state| state.file.iter().map(Held::write).collect())
            .unwrap_or_default()
    }

    /// The names of `entity`'s registers with a live write.
    pub fn registers(&self, entity: EntityId) -> Vec<&str> {
        self.entities
            .get(&entity)
            .map(|state| state.registers.keys().map(|key| &**key).collect())
            .unwrap_or_default()
    }

    /// The bytes `writer` displaced into its trash and has not emptied, oldest
    /// first, as its entries logged them.
    pub fn trash(&self, writer: WriterId) -> Vec<TrashItem> {
        let mut items: Vec<_> = self
            .trash
            .range((writer, Nonce::from_u128(0))..=(writer, Nonce::from_u128(u128::MAX)))
            .map(|(&(_, item), trashed)| TrashItem {
                writer,
                item,
                len: trashed.len,
                from: trashed.from.clone(),
                at: trashed.at,
                by: trashed.entry,
            })
            .collect();
        items.sort_by_key(|item| (item.at, item.item));
        items
    }

    /// Every unfinished effect some writer settled, by its writer and record.
    pub fn settlements(&self) -> &BTreeSet<(WriterId, Nonce)> {
        &self.settled
    }

    /// Entries and ops this build does not know, with the entry that holds each.
    pub fn unknown(&self) -> impl Iterator<Item = &(EntryHash, Raw)> {
        self.unknown.iter()
    }

    /// Known entries holding members this build does not know, whole, by hash.
    pub fn extended(&self) -> impl Iterator<Item = &(EntryHash, Raw)> {
        self.extended.iter()
    }
}
