//! The state every writer's log adds up to.
//!
//! Merging is a pure function of the logs: commutative, associative and idempotent,
//! so any order of reading, and any overlap between a snapshot and the segments it
//! folded, gives the same state.
//!
//! - A field is a last-writer-wins register: the write with the highest version
//!   decides, including a write that clears it.
//! - Existence is a last-writer-wins register too. A field write does not touch it,
//!   so a write concurrent with a delete leaves the entity deleted.
//! - A set is add-wins: each add is tagged with its entry's version, a remove names
//!   the tags it observed, and a value is a member while one of its tags is not
//!   removed.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::ids::{EntityId, Version, WriterId};
use crate::log::{Entry, Kind, WriterLog};
use crate::value::{BlobId, Value};

/// The merged facts: fields, sets, existence and blob adds.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct State {
    exists: BTreeMap<EntityId, Stamped<bool>>,
    fields: BTreeMap<(EntityId, String), Stamped<Option<Value>>>,
    sets: BTreeMap<(EntityId, String), BTreeMap<Value, Tags>>,
    blobs: BTreeMap<BlobId, BTreeMap<WriterId, BlobAdd>>,
    /// Each writer's highest entity and intent counters.
    allocated: BTreeMap<WriterId, Allocated>,
    lamport: u64,
    /// What the entries applied one by one name: a writer's undo window and the
    /// segments after its snapshot. A snapshot's folded state names nothing here.
    window: Window,
}

/// A register's value with the version that wrote it. The derived order, version
/// first, picks the winner; the value breaks a tie only between forged entries that
/// share a version.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Stamped<T> {
    version: Version,
    value: T,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct Tags {
    added: BTreeSet<Version>,
    removed: BTreeSet<Version>,
}

impl Tags {
    fn live(&self) -> impl Iterator<Item = &Version> {
        self.added.difference(&self.removed)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Allocated {
    entity: Option<u64>,
    intent: Option<u64>,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct Window {
    blobs: BTreeSet<BlobId>,
    entities: BTreeSet<EntityId>,
}

/// One writer's record of a blob it added.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlobAdd {
    pub len: u64,
    /// The writer's latest `BlobAdded` or `BlobRemoved` for this blob.
    pub version: Version,
    /// Whether that latest entry was a removal.
    pub removed: bool,
}

impl BlobAdd {
    /// The latest record decides `removed`; the length is the blob's, which only an
    /// add states, so the larger length is kept.
    fn join(&mut self, other: &BlobAdd) {
        let len = self.len.max(other.len);
        if (other.version, other.removed) > (self.version, self.removed) {
            *self = *other;
        }
        self.len = len;
    }
}

/// The state of every writer's log.
pub fn merge(logs: &[WriterLog]) -> State {
    let mut state = State::default();
    for log in logs {
        if let Some(snapshot) = &log.snapshot {
            state.join(&snapshot.state);
        }
        log.all_entries().for_each(|entry| state.apply(entry));
    }
    state
}

fn keep_latest<K: Ord, T: Ord>(map: &mut BTreeMap<K, T>, key: K, value: T) {
    match map.get_mut(&key) {
        Some(current) if *current >= value => {}
        Some(current) => *current = value,
        None => {
            map.insert(key, value);
        }
    }
}

fn max_option(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    a.max(b)
}

impl State {
    /// Fold one entry in. Applying an entry twice changes nothing. An entry of an
    /// unknown kind changes nothing.
    pub fn apply(&mut self, entry: &Entry) {
        if let Kind::Unknown { .. } = entry.kind {
            return;
        }
        let version = entry.version;
        self.lamport = self.lamport.max(version.lamport);
        self.allocate_intent(entry.intent.writer, entry.intent.counter);
        for entity in entry.kind.entities() {
            self.allocate_entity(entity);
        }
        match &entry.kind {
            Kind::Intent { .. } | Kind::Unknown { .. } => {}
            Kind::Create { entity } | Kind::Delete { entity } => {
                let value = matches!(entry.kind, Kind::Create { .. });
                keep_latest(&mut self.exists, *entity, Stamped { version, value });
                self.window.entities.insert(*entity);
            }
            Kind::Field {
                entity,
                name,
                value,
                prior,
            } => {
                let key = (*entity, name.clone());
                let stamped = Stamped {
                    version,
                    value: value.clone(),
                };
                keep_latest(&mut self.fields, key, stamped);
                self.window.entities.insert(*entity);
                let blobs = [value, prior]
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_blob);
                self.window.blobs.extend(blobs);
            }
            Kind::SetAdd {
                entity,
                name,
                value,
            } => {
                self.tags_mut(*entity, name, value).added.insert(version);
                self.window.entities.insert(*entity);
                self.window.blobs.extend(value.as_blob());
            }
            Kind::SetRemove {
                entity,
                name,
                value,
                observed,
            } => {
                let tags = self.tags_mut(*entity, name, value);
                tags.removed.extend(observed.iter().copied());
                self.window.entities.insert(*entity);
                self.window.blobs.extend(value.as_blob());
            }
            Kind::BlobAdded { blob, len } => {
                self.record_blob(
                    *blob,
                    BlobAdd {
                        len: *len,
                        version,
                        removed: false,
                    },
                );
                self.window.blobs.insert(*blob);
            }
            Kind::BlobRemoved { blob } => self.record_blob(
                *blob,
                BlobAdd {
                    len: 0,
                    version,
                    removed: true,
                },
            ),
        }
    }

    fn tags_mut(&mut self, entity: EntityId, name: &str, value: &Value) -> &mut Tags {
        self.sets
            .entry((entity, name.to_owned()))
            .or_default()
            .entry(value.clone())
            .or_default()
    }

    fn record_blob(&mut self, blob: BlobId, add: BlobAdd) {
        self.blobs
            .entry(blob)
            .or_default()
            .entry(add.version.writer)
            .and_modify(|current| current.join(&add))
            .or_insert(add);
    }

    fn allocate_intent(&mut self, writer: WriterId, counter: u64) {
        let allocated = self.allocated.entry(writer).or_default();
        allocated.intent = max_option(allocated.intent, Some(counter));
    }

    fn allocate_entity(&mut self, entity: EntityId) {
        let allocated = self.allocated.entry(entity.writer).or_default();
        allocated.entity = max_option(allocated.entity, Some(entity.counter));
    }

    /// Fold another state in.
    pub fn join(&mut self, other: &State) {
        for (entity, stamped) in &other.exists {
            keep_latest(&mut self.exists, *entity, stamped.clone());
        }
        for (key, stamped) in &other.fields {
            keep_latest(&mut self.fields, key.clone(), stamped.clone());
        }
        for ((entity, name), members) in &other.sets {
            for (value, tags) in members {
                let mine = self.tags_mut(*entity, name, value);
                mine.added.extend(tags.added.iter().copied());
                mine.removed.extend(tags.removed.iter().copied());
            }
        }
        for (blob, adds) in &other.blobs {
            for add in adds.values() {
                self.record_blob(*blob, *add);
            }
        }
        for (writer, theirs) in &other.allocated {
            let mine = self.allocated.entry(*writer).or_default();
            mine.entity = max_option(mine.entity, theirs.entity);
            mine.intent = max_option(mine.intent, theirs.intent);
        }
        self.lamport = self.lamport.max(other.lamport);
        self.window.blobs.extend(other.window.blobs.iter().copied());
        self.window
            .entities
            .extend(other.window.entities.iter().copied());
    }

    /// Forget what the entries applied so far name, as a snapshot folds them.
    pub(crate) fn fold_window(&mut self) {
        self.window = Window::default();
    }

    /// `writer`'s highest entity and intent counters.
    pub(crate) fn allocated(&self, writer: WriterId) -> (Option<u64>, Option<u64>) {
        self.allocated
            .get(&writer)
            .map_or((None, None), |a| (a.entity, a.intent))
    }

    pub fn exists(&self, entity: EntityId) -> bool {
        self.exists
            .get(&entity)
            .is_some_and(|stamped| stamped.value)
    }

    /// Every entity that exists, in order.
    pub fn entities(&self) -> Vec<EntityId> {
        self.exists
            .iter()
            .filter(|(_, stamped)| stamped.value)
            .map(|(entity, _)| *entity)
            .collect()
    }

    /// The field's value, whether or not the entity exists.
    pub fn field(&self, entity: EntityId, name: &str) -> Option<&Value> {
        self.fields
            .get(&(entity, name.to_owned()))
            .and_then(|stamped| stamped.value.as_ref())
    }

    /// The version of the write that decided the field, including a write that
    /// cleared it.
    pub fn field_version(&self, entity: EntityId, name: &str) -> Option<Version> {
        self.fields
            .get(&(entity, name.to_owned()))
            .map(|stamped| stamped.version)
    }

    /// The fields an entity holds, by name.
    pub fn fields(&self, entity: EntityId) -> BTreeMap<&str, &Value> {
        self.fields
            .range((entity, String::new())..)
            .take_while(|((owner, _), _)| *owner == entity)
            .filter_map(|((_, name), stamped)| Some((name.as_str(), stamped.value.as_ref()?)))
            .collect()
    }

    /// Existing entities whose field `name` holds `value`, in order.
    pub fn find(&self, name: &str, value: &Value) -> Vec<EntityId> {
        self.fields
            .iter()
            .filter(|((entity, field), stamped)| {
                field == name && stamped.value.as_ref() == Some(value) && self.exists(*entity)
            })
            .map(|((entity, _), _)| *entity)
            .collect()
    }

    /// The members of a set, whether or not the entity exists.
    pub fn members(&self, entity: EntityId, name: &str) -> BTreeSet<&Value> {
        self.sets
            .get(&(entity, name.to_owned()))
            .into_iter()
            .flatten()
            .filter(|(_, tags)| tags.live().next().is_some())
            .map(|(value, _)| value)
            .collect()
    }

    /// The live add tags of `value` in a set: what a remove must observe.
    pub fn tags(&self, entity: EntityId, name: &str, value: &Value) -> BTreeSet<Version> {
        self.sets
            .get(&(entity, name.to_owned()))
            .and_then(|members| members.get(value))
            .map_or_else(BTreeSet::new, |tags| tags.live().copied().collect())
    }

    /// The highest Lamport time of any entry folded in.
    pub fn max_lamport(&self) -> u64 {
        self.lamport
    }

    /// Every blob some writer has added, with each writer's record of it.
    pub fn blob_adds(&self) -> &BTreeMap<BlobId, BTreeMap<WriterId, BlobAdd>> {
        &self.blobs
    }

    /// Every blob a live value refers to, or an entry inside a writer's retained undo
    /// window refers to.
    ///
    /// A live value is a field or set member of an existing entity. Inside the window,
    /// a blob is referred to by a field's value or prior, a set member, or a
    /// `BlobAdded`, and by any field or set member of an entity an entry there names,
    /// so undoing a delete can restore the entity's file.
    pub fn referenced_blobs(&self) -> BTreeSet<BlobId> {
        let held =
            |entity: &EntityId| self.exists(*entity) || self.window.entities.contains(entity);
        let fields = self
            .fields
            .iter()
            .filter(|((entity, _), _)| held(entity))
            .filter_map(|(_, stamped)| stamped.value.as_ref()?.as_blob());
        let members = self
            .sets
            .iter()
            .filter(|((entity, _), _)| held(entity))
            .flat_map(|(_, members)| members)
            .filter(|(_, tags)| tags.live().next().is_some())
            .filter_map(|(value, _)| value.as_blob());
        fields
            .chain(members)
            .chain(self.window.blobs.iter().copied())
            .collect()
    }
}

/// A state as a snapshot stores it: every register and tag, without the window.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StateFile {
    lamport: u64,
    exists: Vec<ExistsRecord>,
    fields: Vec<FieldRecord>,
    sets: Vec<SetRecord>,
    blobs: Vec<BlobRecord>,
    allocated: Vec<AllocatedRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExistsRecord {
    entity: EntityId,
    version: Version,
    exists: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldRecord {
    entity: EntityId,
    name: String,
    version: Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetRecord {
    entity: EntityId,
    name: String,
    value: Value,
    added: BTreeSet<Version>,
    removed: BTreeSet<Version>,
}

/// The writer is the version's.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobRecord {
    blob: BlobId,
    len: u64,
    version: Version,
    removed: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AllocatedRecord {
    writer: WriterId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entity: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intent: Option<u64>,
}

impl From<&State> for StateFile {
    fn from(state: &State) -> Self {
        Self {
            lamport: state.lamport,
            exists: state
                .exists
                .iter()
                .map(|(entity, stamped)| ExistsRecord {
                    entity: *entity,
                    version: stamped.version,
                    exists: stamped.value,
                })
                .collect(),
            fields: state
                .fields
                .iter()
                .map(|((entity, name), stamped)| FieldRecord {
                    entity: *entity,
                    name: name.clone(),
                    version: stamped.version,
                    value: stamped.value.clone(),
                })
                .collect(),
            sets: state
                .sets
                .iter()
                .flat_map(|((entity, name), members)| {
                    members.iter().map(|(value, tags)| SetRecord {
                        entity: *entity,
                        name: name.clone(),
                        value: value.clone(),
                        added: tags.added.clone(),
                        removed: tags.removed.clone(),
                    })
                })
                .collect(),
            blobs: state
                .blobs
                .iter()
                .flat_map(|(blob, adds)| {
                    adds.values().map(|add| BlobRecord {
                        blob: *blob,
                        len: add.len,
                        version: add.version,
                        removed: add.removed,
                    })
                })
                .collect(),
            allocated: state
                .allocated
                .iter()
                .map(|(writer, allocated)| AllocatedRecord {
                    writer: *writer,
                    entity: allocated.entity,
                    intent: allocated.intent,
                })
                .collect(),
        }
    }
}

/// Records are joined, so a repeated record changes nothing.
impl From<StateFile> for State {
    fn from(file: StateFile) -> Self {
        let mut state = State {
            lamport: file.lamport,
            ..State::default()
        };
        for record in file.exists {
            let stamped = Stamped {
                version: record.version,
                value: record.exists,
            };
            keep_latest(&mut state.exists, record.entity, stamped);
        }
        for record in file.fields {
            let stamped = Stamped {
                version: record.version,
                value: record.value,
            };
            keep_latest(&mut state.fields, (record.entity, record.name), stamped);
        }
        for record in file.sets {
            let tags = state.tags_mut(record.entity, &record.name, &record.value);
            tags.added.extend(record.added);
            tags.removed.extend(record.removed);
        }
        for record in file.blobs {
            state.record_blob(
                record.blob,
                BlobAdd {
                    len: record.len,
                    version: record.version,
                    removed: record.removed,
                },
            );
        }
        for record in file.allocated {
            let allocated = state.allocated.entry(record.writer).or_default();
            allocated.entity = max_option(allocated.entity, record.entity);
            allocated.intent = max_option(allocated.intent, record.intent);
        }
        state
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ids::IntentId;
    use crate::log::testing::{facts, field, text, writer};

    pub(crate) fn entry(writer: WriterId, lamport: u64, kind: Kind) -> Entry {
        Entry {
            version: Version::new(lamport, writer),
            intent: IntentId::new(writer, lamport),
            kind,
        }
    }

    fn applied<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> State {
        let mut state = State::default();
        entries.into_iter().for_each(|entry| state.apply(entry));
        state
    }

    fn joined(states: &[State]) -> State {
        let mut state = State::default();
        states.iter().for_each(|other| state.join(other));
        state
    }

    /// Every ordering of `items`, by Heap's algorithm.
    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        fn heap<T: Clone>(k: usize, items: &mut Vec<T>, out: &mut Vec<Vec<T>>) {
            if k <= 1 {
                out.push(items.clone());
                return;
            }
            for i in 0..k {
                heap(k - 1, items, out);
                items.swap(if k.is_multiple_of(2) { i } else { 0 }, k - 1);
            }
        }
        let mut out = Vec::new();
        heap(items.len(), &mut items.to_vec(), &mut out);
        out
    }

    /// A deterministic xorshift64 generator.
    pub(crate) struct Rng(u64);

    impl Rng {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed.max(1))
        }

        pub(crate) fn below(&mut self, bound: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % bound as u64) as usize
        }

        fn shuffle<T>(&mut self, items: &mut [T]) {
            for i in (1..items.len()).rev() {
                items.swap(i, self.below(i + 1));
            }
        }
    }

    /// Entries from three writers over two entities, two names, three values and
    /// two blobs, with removes that observe earlier adds.
    pub(crate) fn random_entries(rng: &mut Rng, count: usize) -> Vec<Entry> {
        let entities = [EntityId::new(writer(1), 0), EntityId::new(writer(2), 0)];
        let names = ["a", "b"];
        let values = [text("x"), text("y"), Value::Int(1)];
        let blobs = [BlobId::of(b"1"), BlobId::of(b"2")];
        let mut adds: Vec<Version> = Vec::new();
        let mut entries = Vec::new();
        for lamport in 1..=count as u64 {
            let by = writer(1 + rng.below(3) as u128);
            let entity = entities[rng.below(2)];
            let name = names[rng.below(2)].to_owned();
            let value = values[rng.below(3)].clone();
            let blob = blobs[rng.below(2)];
            let kind = match rng.below(7) {
                0 => Kind::Create { entity },
                1 => Kind::Delete { entity },
                2 => Kind::Field {
                    entity,
                    name,
                    value: (rng.below(4) > 0).then_some(value),
                    prior: None,
                },
                3 => Kind::SetAdd {
                    entity,
                    name,
                    value,
                },
                4 => Kind::SetRemove {
                    entity,
                    name,
                    value,
                    observed: adds.iter().filter(|_| rng.below(2) == 0).copied().collect(),
                },
                5 => Kind::BlobAdded {
                    blob,
                    len: blob.as_bytes().len() as u64,
                },
                _ => Kind::BlobRemoved { blob },
            };
            let entry = entry(by, lamport / 2 + 1, kind);
            if matches!(entry.kind, Kind::SetAdd { .. }) {
                adds.push(entry.version);
            }
            entries.push(entry);
        }
        entries.sort_by_key(|entry| entry.version);
        entries.dedup_by_key(|entry| entry.version);
        entries
    }

    fn conflicting_entries() -> Vec<Entry> {
        let (a, b) = (writer(1), writer(2));
        let entity = EntityId::new(a, 0);
        let x = text("x");
        vec![
            entry(a, 1, Kind::Create { entity }),
            entry(b, 2, Kind::Delete { entity }),
            entry(a, 2, field(entity, "name", Some(text("A")))),
            entry(b, 3, field(entity, "name", Some(text("B")))),
            entry(
                a,
                3,
                Kind::SetAdd {
                    entity,
                    name: "tags".into(),
                    value: x.clone(),
                },
            ),
            entry(
                b,
                4,
                Kind::SetRemove {
                    entity,
                    name: "tags".into(),
                    value: x,
                    observed: [Version::new(3, a)].into(),
                },
            ),
            entry(
                a,
                4,
                Kind::BlobRemoved {
                    blob: BlobId::of(b"1"),
                },
            ),
        ]
    }

    #[test]
    fn merging_in_any_order_gives_the_same_state() {
        let entries = conflicting_entries();
        let expected = applied(&entries);
        for order in permutations(&entries) {
            assert_eq!(applied(&order), expected, "{order:#?}");
        }
    }

    #[test]
    fn joining_is_associative_commutative_and_idempotent() {
        let entries = conflicting_entries();
        let expected = applied(&entries);
        for assignment in 0..3usize.pow(entries.len() as u32) {
            let mut parts = [Vec::new(), Vec::new(), Vec::new()];
            let mut digits = assignment;
            for entry in &entries {
                parts[digits % 3].push(entry);
                digits /= 3;
            }
            let [a, b, c] = parts.map(applied);
            let left = joined(&[joined(&[a.clone(), b.clone()]), c.clone()]);
            let right = joined(&[a.clone(), joined(&[b.clone(), c.clone()])]);
            assert_eq!(left, expected, "assignment {assignment}");
            assert_eq!(right, expected, "assignment {assignment}");
            assert_eq!(joined(&[c, b, a]), expected, "assignment {assignment}");
        }
        assert_eq!(joined(&[expected.clone(), expected.clone()]), expected);
        assert_eq!(applied(entries.iter().chain(&entries)), expected);
    }

    #[test]
    fn random_entry_sets_merge_the_same_in_any_order_or_grouping() {
        let mut rng = Rng::new(0x5eed);
        for trial in 0..200 {
            let mut entries = random_entries(&mut rng, 24);
            let expected = applied(&entries);
            rng.shuffle(&mut entries);
            assert_eq!(applied(&entries), expected, "trial {trial}");
            let mut parts = [Vec::new(), Vec::new(), Vec::new()];
            for entry in &entries {
                parts[rng.below(3)].push(entry);
            }
            let states = parts.map(applied);
            assert_eq!(joined(&states), expected, "trial {trial}");
            assert_eq!(
                joined(&[expected.clone(), states[0].clone()]),
                expected,
                "trial {trial}"
            );
            assert_eq!(
                State::from(StateFile::from(&expected)),
                facts(expected.clone()),
                "trial {trial}"
            );
        }
    }

    #[test]
    fn the_highest_version_decides_a_field() {
        let state = applied(&conflicting_entries());
        let entity = EntityId::new(writer(1), 0);
        assert_eq!(state.field(entity, "name"), Some(&text("B")));
        assert_eq!(
            state.field_version(entity, "name"),
            Some(Version::new(3, writer(2)))
        );
        let cleared = applied(&[entry(writer(1), 9, field(entity, "name", None))]);
        let mut both = state.clone();
        both.join(&cleared);
        assert_eq!(both.field(entity, "name"), None);
        assert_eq!(
            both.field_version(entity, "name"),
            Some(Version::new(9, writer(1)))
        );
        assert!(both.fields(entity).is_empty());
    }

    #[test]
    fn a_remove_spares_the_adds_it_did_not_observe() {
        let (a, b) = (writer(1), writer(2));
        let entity = EntityId::new(a, 0);
        let add = |by, lamport| {
            entry(
                by,
                lamport,
                Kind::SetAdd {
                    entity,
                    name: "tags".into(),
                    value: text("x"),
                },
            )
        };
        let remove = entry(
            b,
            3,
            Kind::SetRemove {
                entity,
                name: "tags".into(),
                value: text("x"),
                observed: [Version::new(1, a)].into(),
            },
        );
        let removed = applied(&[add(a, 1), remove.clone()]);
        assert!(removed.members(entity, "tags").is_empty());
        let concurrent = applied(&[add(a, 1), remove, add(a, 2)]);
        assert_eq!(concurrent.members(entity, "tags"), [&text("x")].into());
        assert_eq!(
            concurrent.tags(entity, "tags", &text("x")),
            [Version::new(2, a)].into()
        );
    }

    #[test]
    fn a_write_concurrent_with_a_delete_does_not_resurrect_the_entity() {
        let (a, b) = (writer(1), writer(2));
        let entity = EntityId::new(a, 0);
        let mut entries = vec![
            entry(a, 1, Kind::Create { entity }),
            entry(b, 2, Kind::Delete { entity }),
            entry(a, 3, field(entity, "name", Some(text("late")))),
        ];
        let deleted = applied(&entries);
        assert!(!deleted.exists(entity));
        assert!(deleted.entities().is_empty());
        assert!(deleted.find("name", &text("late")).is_empty());
        assert_eq!(deleted.field(entity, "name"), Some(&text("late")));

        entries.push(entry(b, 4, Kind::Create { entity }));
        let recreated = applied(&entries);
        assert_eq!(recreated.entities(), [entity]);
        assert_eq!(recreated.find("name", &text("late")), [entity]);
        assert_eq!(recreated.fields(entity), [("name", &text("late"))].into());
    }

    #[test]
    fn a_blob_record_keeps_its_length_and_latest_state_in_any_order() {
        let a = writer(1);
        let blob = BlobId::of(b"abc");
        let added = entry(a, 1, Kind::BlobAdded { blob, len: 3 });
        let removed = entry(a, 2, Kind::BlobRemoved { blob });
        let expected = BlobAdd {
            len: 3,
            version: Version::new(2, a),
            removed: true,
        };
        for order in [[&added, &removed], [&removed, &added]] {
            assert_eq!(applied(order).blob_adds()[&blob][&a], expected);
        }
    }

    #[test]
    fn blobs_are_referenced_by_live_values_and_by_the_window() {
        let a = writer(1);
        let (live, gone, old) = (BlobId::of(b"live"), BlobId::of(b"gone"), BlobId::of(b"old"));
        let (kept, deleted) = (EntityId::new(a, 0), EntityId::new(a, 1));
        let entries = [
            entry(a, 1, Kind::Create { entity: kept }),
            entry(a, 2, Kind::Create { entity: deleted }),
            entry(
                a,
                3,
                Kind::Field {
                    entity: kept,
                    name: "content".into(),
                    value: Some(Value::Blob(live)),
                    prior: Some(Value::Blob(old)),
                },
            ),
            entry(a, 4, field(deleted, "content", Some(Value::Blob(gone)))),
            entry(a, 5, Kind::Delete { entity: deleted }),
        ];
        let state = applied(&entries);
        assert_eq!(state.referenced_blobs(), [live, gone, old].into());
        assert_eq!(facts(state).referenced_blobs(), [live].into());
    }
}
