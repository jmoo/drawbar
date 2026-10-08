//! The merge: a join over entries.
//!
//! Folding an entry and joining two states commute, associate and are idempotent,
//! so readers holding the same entries compute the same state whatever was
//! compacted and in whatever order files arrived. Every piece of state is keyed by
//! the entry that wrote it, and joins by union.
//!
//! - A register keeps every write with a grow-only set of replaced writes. The
//!   writes no write replaces survive; several surviving values are a conflict. A
//!   snapshot never resurrects a write another log still holds, because the
//!   replaced set travels with it.
//! - A set is observed-remove: an add is tagged by its entry and value, and a
//!   remove takes away the tags it names, so a concurrent add survives.
//! - Existence is a register of creates and deletes. A delete names the field
//!   writes it observed; a field write it did not observe keeps the entity, with
//!   its deletion in conflict. Writes in the delete's own entry count as observed.
//! - Unknown entries and ops are kept verbatim, and so is a known entry holding
//!   members this build does not know. Every record of a snapshot's state keeps
//!   the members this build does not know.

use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::btree_map::Entry as Slot;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use serde::de::{DeserializeOwned, Error as _};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::binding::{Fact, Facts};
use crate::cow::CowMap;
use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::log::{Displaced, Entry, EntryKind, FileFact, Op};
use crate::pack::{bad_variant, pack_struct, Bad, In, Pack, Unpack, Unpacked};
use crate::path::RelPath;
use crate::reader::WriterLog;
use crate::report::TrashItem;
use crate::schema::{KeyKind, Raw, Written};
use crate::small::{SmallMap, SmallSet};
use crate::snapshot::Snapshot;

/// The folded state of a set of entries. A clone shares each entity's state with
/// the original until one of them changes it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Folded {
    entities: CowMap<EntityId, Arc<EntityState>>,
    trash: BTreeMap<(WriterId, Nonce), Trashed>,
    settled: BTreeSet<(WriterId, Nonce)>,
    unknown: BTreeSet<(EntryHash, Raw)>,
    /// Known entries that hold members this build does not know, whole.
    extended: BTreeSet<(EntryHash, Raw)>,
    extra: Extra,
}

/// The members of a record of a snapshot's state that this build does not know,
/// by name. Few records have any, so they are held apart, and only while there
/// are some.
// Boxed, a pointer's width where a map is three.
#[allow(clippy::box_collection)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct Extra(Option<Box<BTreeMap<String, Raw>>>);

/// The [`Extra`] of each record under a key, where it has any; held apart, and
/// only while some record has any.
#[allow(clippy::box_collection)]
#[derive(Clone, PartialEq, Debug)]
struct Extras<K>(Option<Box<BTreeMap<K, Extra>>>);

/// A key's name, shared by every record that holds it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Key(Arc<str>);

/// How many names [`Key::of`] keeps to share. Past them a name is held alone, so
/// a log naming ever more keys cannot grow what this thread keeps.
const SHARED_NAMES: usize = 1024;

impl Key {
    /// The key named `name`, sharing the text of every other key of that name.
    fn of(name: &str) -> Self {
        thread_local! {
            static NAMES: RefCell<BTreeSet<Arc<str>>> = const { RefCell::new(BTreeSet::new()) };
        }
        NAMES.with(|names| {
            let mut names = names.borrow_mut();
            if let Some(shared) = names.get(name) {
                return Self(Arc::clone(shared));
            }
            let named: Arc<str> = name.into();
            if names.len() < SHARED_NAMES {
                names.insert(Arc::clone(&named));
            }
            Self(named)
        })
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Key {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// The value under the key named `name`, made empty when there is none.
fn named<'a, V: Default>(map: &'a mut SmallMap<Key, V>, name: &str) -> &'a mut V {
    map.slot(name, || (Key::of(name), V::default()))
}

/// As [`named`], for a key another state holds.
fn keyed<'a, V: Default>(map: &'a mut SmallMap<Key, V>, key: &Key) -> &'a mut V {
    map.slot(key.as_str(), || (key.clone(), V::default()))
}

/// Who wrote something, ordered for display: by clock, then by writer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Stamp {
    at: Hlc,
    by: WriterId,
}

/// A multi-value register.
#[derive(Clone, PartialEq, Debug)]
struct Register<V> {
    writes: SmallMap<EntryHash, (Stamp, V)>,
    replaced: SmallSet<EntryHash>,
    extra: Extra,
    write_extra: Extras<EntryHash>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Existence {
    Created,
    /// Sorted, without repeats.
    Deleted {
        observed: Box<[EntryHash]>,
    },
}

/// An observed-remove set. A tag is the entry that added the value.
#[derive(Clone, PartialEq, Debug, Default)]
struct OrSet {
    /// Only adds no remove names.
    adds: SmallMap<(Raw, EntryHash), Stamp>,
    /// Each removed tag, with the earliest remove naming it.
    removed: SmallMap<(Raw, EntryHash), (Stamp, EntryHash)>,
    extra: Extra,
    add_extra: Extras<(Raw, EntryHash)>,
    removed_extra: Extras<(Raw, EntryHash)>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct FileValue {
    path: RelPath,
    identity: Identity,
    len: u64,
    modified: Option<u64>,
    extra: Extra,
}

#[derive(Clone, PartialEq, Debug, Default)]
struct EntityState {
    existence: Register<Existence>,
    registers: SmallMap<Key, Register<Option<Raw>>>,
    sets: SmallMap<Key, OrSet>,
    file: Register<Option<FileValue>>,
    extra: Extra,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Trashed {
    entry: EntryHash,
    at: Hlc,
    from: RelPath,
    identity: Identity,
    len: u64,
    extra: Extra,
}

/// One write: of a register, surviving or not, or a set's add or remove.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Write<V> {
    pub entry: EntryHash,
    pub by: WriterId,
    pub at: Hlc,
    pub value: V,
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

/// One way a state shows more than a state of fewer entries: what an entity's
/// existence, a register, a set or its file shows, with the op that makes the
/// smaller state show the same.
#[derive(Clone, PartialEq, Debug)]
pub struct Beyond {
    pub entity: EntityId,
    pub part: Part,
    /// The writer of the latest write behind the difference.
    pub by: WriterId,
    pub op: Op,
}

/// The values of `now` whose keys `old` does not hold.
fn added<'a, K: Ord, V>(
    now: &'a SmallMap<K, V>,
    old: Option<&'a SmallMap<K, V>>,
) -> impl Iterator<Item = &'a V> {
    now.iter()
        .filter(move |(key, _)| !old.is_some_and(|old| old.contains_key(key)))
        .map(|(_, value)| value)
}

impl Extra {
    /// Keeps the greater text under each name, so joins commute.
    fn join(&mut self, other: &Self) {
        let Some(theirs) = &other.0 else {
            return;
        };
        let mine = self.0.get_or_insert_with(Box::default);
        for (name, raw) in theirs.iter() {
            let kept = mine.entry(name.clone()).or_insert_with(|| raw.clone());
            if raw > kept {
                *kept = raw.clone();
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    fn members(&self) -> impl Iterator<Item = (&String, &Raw)> {
        self.0.iter().flat_map(|members| members.iter())
    }
}

impl From<BTreeMap<String, Raw>> for Extra {
    fn from(members: BTreeMap<String, Raw>) -> Self {
        Self((!members.is_empty()).then(|| Box::new(members)))
    }
}

impl Serialize for Extra {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.members())
    }
}

impl<K> Default for Extras<K> {
    fn default() -> Self {
        Self(None)
    }
}

impl<K: Ord + Clone> Extras<K> {
    fn join_one(&mut self, key: &K, extra: &Extra) {
        if !extra.is_empty() {
            let extras = self.0.get_or_insert_with(Box::default);
            extras.entry(key.clone()).or_default().join(extra);
        }
    }

    fn join(&mut self, other: &Self) {
        for (key, extra) in other.0.iter().flat_map(|extras| extras.iter()) {
            self.join_one(key, extra);
        }
    }

    fn of(&self, key: &K) -> Extra {
        let extra = self.0.as_ref().and_then(|extras| extras.get(key));
        extra.cloned().unwrap_or_default()
    }

    fn remove(&mut self, key: &K) {
        if let Some(extras) = &mut self.0 {
            extras.remove(key);
            if extras.is_empty() {
                self.0 = None;
            }
        }
    }
}

impl Trashed {
    /// Keeps the greater item and every member either holds that this build does
    /// not know.
    fn join(&mut self, other: &Self) {
        let mut extra = self.extra.clone();
        extra.join(&other.extra);
        if other > self {
            *self = other.clone();
        }
        self.extra = extra;
    }
}

impl Stamp {
    fn write<V>(self, entry: EntryHash, value: V) -> Write<V> {
        Write {
            entry,
            by: self.by,
            at: self.at,
            value,
        }
    }
}

impl<V> Default for Register<V> {
    fn default() -> Self {
        Self {
            writes: SmallMap::default(),
            replaced: SmallSet::default(),
            extra: Extra::default(),
            write_extra: Extras::default(),
        }
    }
}

impl<V: Ord + Clone> Register<V> {
    fn write(&mut self, entry: EntryHash, stamp: Stamp, value: V, replaces: &[EntryHash]) {
        self.insert(entry, stamp, value);
        self.replaced.extend(replaces.iter().copied());
    }

    /// Two writes under one entry hash only come from a forged or colliding log;
    /// the greater is kept so the join stays order-independent.
    fn insert(&mut self, entry: EntryHash, stamp: Stamp, value: V) {
        match self.writes.get_mut(&entry) {
            None => {
                self.writes.insert(entry, (stamp, value));
            }
            Some(held) => {
                if (stamp, &value) > (held.0, &held.1) {
                    *held = (stamp, value);
                }
            }
        }
    }

    fn join(&mut self, other: &Self) {
        for (&entry, (stamp, value)) in other.writes.iter() {
            self.insert(entry, *stamp, value.clone());
        }
        self.replaced.extend(other.replaced.keys().copied());
        self.extra.join(&other.extra);
        self.write_extra.join(&other.write_extra);
    }

    fn survivors(&self) -> impl Iterator<Item = (EntryHash, Stamp, &V)> {
        self.writes
            .iter()
            .filter(|(entry, _)| !self.replaced.contains_key(*entry))
            .map(|(&entry, (stamp, value))| (entry, *stamp, value))
    }

    fn get(&self, entry: EntryHash) -> Option<Write<V>> {
        let (stamp, value) = self.writes.get(&entry)?;
        Some(stamp.write(entry, value.clone()))
    }

    /// The writer of the latest write this register holds and `old` does not.
    fn newest_beyond(&self, old: &Self) -> Option<WriterId> {
        let stamps = self.beyond(old).map(|(stamp, _)| *stamp);
        stamps.max().map(|stamp| stamp.by)
    }

    /// The writes this register holds and `old` does not.
    fn beyond<'a>(&'a self, old: &'a Self) -> impl Iterator<Item = &'a (Stamp, V)> {
        added(&self.writes, Some(&old.writes))
    }

    /// The surviving writes, oldest first.
    fn surviving(&self) -> Vec<Write<V>> {
        oldest_first(
            self.survivors()
                .map(|(entry, stamp, value)| stamp.write(entry, value.clone())),
        )
    }
}

impl<V: Ord + Clone> Register<Option<V>> {
    /// The write that makes `old`, a register of fewer writes, show the values this
    /// one shows, with the writer behind the difference: the latest value `old`
    /// lacks, or else the latest this one shows, or a clear, replacing every
    /// surviving write of `old` whose value this one does not show. `None` when
    /// both show the same values.
    fn rewrite(&self, old: &Self) -> Option<(Option<WriterId>, Option<V>, Vec<EntryHash>)> {
        let values = |register: &Self| -> BTreeSet<V> {
            let shown = register
                .survivors()
                .filter_map(|(_, _, value)| value.clone());
            shown.collect()
        };
        let (shown, held) = (values(self), values(old));
        if shown == held {
            return None;
        }
        let latest = |new: bool| {
            self.survivors()
                .filter_map(|(_, stamp, value)| Some((stamp, value.as_ref()?)))
                .filter(|(_, value)| !new || !held.contains(*value))
                .max_by_key(|(stamp, _)| *stamp)
                .map(|(_, value)| value.clone())
        };
        let value = latest(true).or_else(|| latest(false));
        let replaces = old
            .survivors()
            .filter(|(_, _, value)| value.as_ref().is_none_or(|value| !shown.contains(value)))
            .map(|(entry, ..)| entry)
            .collect();
        Some((self.newest_beyond(old), value, replaces))
    }
}

/// The surviving writes that hold a value, oldest first.
fn written<V: Ord + Clone>(register: &Register<Option<V>>) -> Vec<Written<V>> {
    register
        .surviving()
        .into_iter()
        .filter_map(|write| {
            Some(Written {
                value: write.value?,
                by: write.by,
                at: write.at,
                entry: write.entry,
            })
        })
        .collect()
}

impl OrSet {
    fn add(&mut self, value: Raw, tag: EntryHash, stamp: Stamp, extra: &Extra) {
        let key = (value, tag);
        if self.removed.contains_key(&key) {
            return;
        }
        self.add_extra.join_one(&key, extra);
        match self.adds.get_mut(&key) {
            Some(kept) => *kept = (*kept).max(stamp),
            None => {
                self.adds.insert(key, stamp);
            }
        }
    }

    fn remove(&mut self, value: &Raw, tags: &[EntryHash], by: (Stamp, EntryHash)) {
        for &tag in tags {
            let key = (value.clone(), tag);
            self.adds.remove(&key);
            self.add_extra.remove(&key);
            match self.removed.get_mut(&key) {
                Some(kept) => *kept = (*kept).min(by),
                None => {
                    self.removed.insert(key, by);
                }
            }
        }
    }

    /// The adds and removes that make `old`, a set of fewer entries, hold the
    /// members this one holds, each with the writer behind it.
    fn rewrite(&self, old: &Self, entity: EntityId, key: &str) -> Vec<(Option<WriterId>, Op)> {
        let members = |set: &Self| -> BTreeSet<Raw> {
            set.adds.keys().map(|(value, _)| value.clone()).collect()
        };
        let (shown, held) = (members(self), members(old));
        let added = shown.difference(&held).map(|value| {
            let adds = self.adds.iter().filter(|((member, _), _)| member == value);
            let by = adds.map(|(_, stamp)| *stamp).max().map(|stamp| stamp.by);
            let op = Op::Add {
                entity,
                key: key.to_owned(),
                value: value.clone(),
            };
            (by, op)
        });
        let removed = held.difference(&shown).map(|value| {
            let tags: Vec<EntryHash> = old
                .adds
                .keys()
                .filter(|(member, _)| member == value)
                .map(|(_, tag)| *tag)
                .collect();
            let removes = tags
                .iter()
                .filter_map(|tag| self.removed.get(&(value.clone(), *tag)));
            let by = removes.map(|(stamp, _)| *stamp).max().map(|stamp| stamp.by);
            let op = Op::Remove {
                entity,
                key: key.to_owned(),
                value: value.clone(),
                tags,
            };
            (by, op)
        });
        added.chain(removed).collect()
    }

    fn join(&mut self, other: &Self) {
        for ((value, tag), &by) in other.removed.iter() {
            self.remove(value, &[*tag], by);
        }
        self.removed_extra.join(&other.removed_extra);
        for (key, &stamp) in other.adds.iter() {
            let (value, tag) = key;
            self.add(value.clone(), *tag, stamp, &other.add_extra.of(key));
        }
        self.extra.join(&other.extra);
    }
}

impl EntityState {
    fn join(&mut self, other: &Self) {
        self.existence.join(&other.existence);
        for (key, register) in other.registers.iter() {
            keyed(&mut self.registers, key).join(register);
        }
        for (key, set) in other.sets.iter() {
            keyed(&mut self.sets, key).join(set);
        }
        self.file.join(&other.file);
        self.extra.join(&other.extra);
    }

    /// The field writes that keep the entity from being deleted unobserved: values
    /// that survive, adds no remove names, and files that survive.
    fn live(&self) -> BTreeMap<EntryHash, Stamp> {
        let registers = self.registers.values().flat_map(|register| {
            register
                .survivors()
                .filter(|(_, _, value)| value.is_some())
                .map(|(entry, stamp, _)| (entry, stamp))
        });
        let sets = self
            .sets
            .values()
            .flat_map(|set| set.adds.iter().map(|((_, tag), &stamp)| (*tag, stamp)));
        let file = self
            .file
            .survivors()
            .filter(|(_, _, value)| value.is_some())
            .map(|(entry, stamp, _)| (entry, stamp));
        registers.chain(sets).chain(file).collect()
    }

    /// Writes no surviving delete observed: surviving creates, and live field
    /// writes outside every surviving delete's observed set and own entry. Empty
    /// unless a delete survives.
    fn unobserved(&self) -> BTreeMap<EntryHash, Stamp> {
        let mut observed = BTreeSet::new();
        let mut creates = BTreeMap::new();
        for (entry, stamp, existence) in self.existence.survivors() {
            match existence {
                Existence::Created => {
                    creates.insert(entry, stamp);
                }
                Existence::Deleted { observed: seen } => {
                    observed.insert(entry);
                    observed.extend(seen.iter().copied());
                }
            }
        }
        if observed.is_empty() {
            return BTreeMap::new();
        }
        let mut unobserved = self.live();
        unobserved.retain(|entry, _| !observed.contains(entry));
        unobserved.extend(creates);
        unobserved
    }

    fn deletion_conflicted(&self) -> bool {
        !self.unobserved().is_empty()
    }

    /// The writer of the latest write this state holds and `old` does not.
    fn newest_beyond(&self, old: &Self) -> Option<WriterId> {
        let none = Register::default();
        let registers = self.registers.iter().flat_map(|(key, register)| {
            let old = old.registers.get(key.as_str()).unwrap_or(&none);
            register.beyond(old).map(|(stamp, _)| *stamp)
        });
        let empty = OrSet::default();
        let sets = self.sets.iter().flat_map(|(key, set)| {
            let old = old.sets.get(key.as_str()).unwrap_or(&empty);
            let adds = added(&set.adds, Some(&old.adds)).copied();
            adds.chain(added(&set.removed, Some(&old.removed)).map(|(stamp, _)| *stamp))
        });
        let existence = self
            .existence
            .beyond(&old.existence)
            .map(|(stamp, _)| *stamp);
        let file = self.file.beyond(&old.file).map(|(stamp, _)| *stamp);
        let stamps = existence.chain(registers).chain(sets).chain(file);
        stamps.max().map(|stamp| stamp.by)
    }

    /// The surviving file writes that say where the file is, oldest first.
    fn file_facts(&self) -> Box<[Fact]> {
        let surviving = self.file.surviving().into_iter();
        let facts = surviving.filter_map(|write| {
            Some(Fact {
                value: FileFact::from(write.value?),
                entry: write.entry,
            })
        });
        facts.collect()
    }

    fn present(&self) -> bool {
        let created = self
            .existence
            .survivors()
            .any(|(_, _, existence)| *existence == Existence::Created);
        created || self.deletion_conflicted()
    }

    /// The surviving file write that names a file, latest by clock, writer and
    /// entry: the last of [`EntityState::file_facts`].
    fn latest_file(&self) -> Option<&FileValue> {
        let named = self.file.survivors().filter_map(|(entry, stamp, value)| {
            Some(((stamp.at, stamp.by, entry), value.as_ref()?))
        });
        named.max_by_key(|(order, _)| *order).map(|(_, file)| file)
    }

    fn shown(&self) -> Option<Shown<'_>> {
        let (mut created, mut deleted) = (false, false);
        for (_, _, existence) in self.existence.survivors() {
            match existence {
                Existence::Created => created = true,
                Existence::Deleted { .. } => deleted = true,
            }
        }
        let deletion_conflicted = deleted && self.deletion_conflicted();
        if !created && !deletion_conflicted {
            return None;
        }
        let registers = self.registers.iter().flat_map(|(key, register)| {
            let values = register
                .survivors()
                .filter_map(|(_, _, value)| value.as_ref());
            values.map(|value| (KeyKind::Register, key.as_str(), value))
        });
        let sets = self.sets.iter().flat_map(|(key, set)| {
            let members = set.adds.keys().map(|(value, _)| value);
            members.map(|value| (KeyKind::Set, key.as_str(), value))
        });
        let mut values: Vec<_> = registers.chain(sets).collect();
        values.sort_unstable();
        values.dedup();
        let mut files = self
            .file
            .survivors()
            .filter_map(|(_, _, value)| value.as_ref());
        let first = files.next();
        Some(Shown {
            values,
            deletion_conflicted,
            file_conflicted: files.any(|file| first.is_some_and(|first| !same_fact(first, file))),
            logged: self.latest_file().map(|file| &file.path),
        })
    }
}

/// Whether two file writes say the same, members this build does not know aside.
fn same_fact(a: &FileValue, b: &FileValue) -> bool {
    (&a.path, a.identity, a.len, a.modified) == (&b.path, b.identity, b.len, b.modified)
}

/// What an entity shows that a view indexes.
#[derive(Clone, PartialEq, Debug)]
pub struct Shown<'a> {
    /// Each value it holds under each key: of a register, the values of its
    /// surviving writes; of a set, its members. Sorted and once each.
    pub values: Vec<(KeyKind, &'a str, &'a Raw)>,
    pub deletion_conflicted: bool,
    /// Whether its surviving file writes that name a file name different ones.
    pub file_conflicted: bool,
    /// The path of its latest surviving file write that names a file.
    pub logged: Option<&'a RelPath>,
}

impl From<&FileFact> for FileValue {
    fn from(fact: &FileFact) -> Self {
        Self {
            path: fact.path.clone(),
            identity: fact.identity,
            len: fact.len,
            modified: fact.modified,
            extra: Extra::default(),
        }
    }
}

impl From<FileValue> for FileFact {
    fn from(value: FileValue) -> Self {
        Self {
            path: value.path,
            identity: value.identity,
            len: value.len,
            modified: value.modified,
        }
    }
}

impl Folded {
    /// Folds one entry `writer` logged.
    pub fn apply(&mut self, writer: WriterId, entry: &Entry) {
        self.fold(writer, entry, &entry.kind());
    }

    /// As [`Folded::apply`], with the entry's kind unpacked already.
    pub fn fold(&mut self, writer: WriterId, entry: &Entry, kind: &EntryKind) {
        let hash = entry.hash();
        let stamp = Stamp {
            at: entry.at(),
            by: writer,
        };
        match kind {
            EntryKind::Genesis(_) => {}
            EntryKind::Intent(logged) => {
                for op in &logged.ops {
                    self.apply_op(hash, stamp, op);
                }
                for displaced in &logged.displaced {
                    self.trash_item(writer, hash, entry.at(), displaced, &Extra::default());
                }
            }
            EntryKind::Settle(settle) => {
                self.settled.insert((settle.writer, settle.record));
            }
            EntryKind::Bind(bound) => {
                for op in &bound.ops {
                    self.apply_op(hash, stamp, op);
                }
            }
            EntryKind::Unknown(raw) => {
                self.unknown.insert((hash, raw.clone()));
            }
        }
        if entry.unknown_members() {
            let whole = Raw::new(&entry.json()).expect("a verified line holds JSON");
            self.extended.insert((hash, whole));
        }
    }

    fn apply_op(&mut self, hash: EntryHash, stamp: Stamp, op: &Op) {
        match op {
            Op::Create { entity, replaces } => {
                self.entity(*entity)
                    .existence
                    .write(hash, stamp, Existence::Created, replaces);
            }
            Op::Delete {
                entity,
                replaces,
                observed,
            } => {
                let observed = sorted(observed.clone());
                self.entity(*entity).existence.write(
                    hash,
                    stamp,
                    Existence::Deleted { observed },
                    replaces,
                );
            }
            Op::Write {
                entity,
                key,
                value,
                replaces,
            } => {
                let register = named(&mut self.entity(*entity).registers, key);
                register.write(hash, stamp, value.clone(), replaces);
            }
            Op::Add { entity, key, value } => {
                let set = named(&mut self.entity(*entity).sets, key);
                set.add(value.clone(), hash, stamp, &Extra::default());
            }
            Op::Remove {
                entity,
                key,
                value,
                tags,
            } => {
                let set = named(&mut self.entity(*entity).sets, key);
                set.remove(value, tags, (stamp, hash));
            }
            Op::File {
                entity,
                file,
                replaces,
            } => {
                let value = file.as_ref().map(FileValue::from);
                self.entity(*entity)
                    .file
                    .write(hash, stamp, value, replaces);
            }
            Op::Pin {
                entity,
                file,
                replaces,
            } => {
                let value = Some(FileValue::from(file));
                self.entity(*entity)
                    .file
                    .write(hash, stamp, value, replaces);
            }
            Op::Unknown(raw) => {
                self.unknown.insert((hash, raw.clone()));
            }
        }
    }

    fn trash_item(
        &mut self,
        writer: WriterId,
        entry: EntryHash,
        at: Hlc,
        item: &Displaced,
        extra: &Extra,
    ) {
        let trashed = Trashed {
            entry,
            at,
            from: item.from.clone(),
            identity: item.identity,
            len: item.len,
            extra: extra.clone(),
        };
        match self.trash.entry((writer, item.item)) {
            Slot::Vacant(slot) => {
                slot.insert(trashed);
            }
            Slot::Occupied(mut slot) => slot.get_mut().join(&trashed),
        }
    }

    fn entity(&mut self, entity: EntityId) -> &mut EntityState {
        Arc::make_mut(self.entities.get_or_insert_with(entity, Arc::default))
    }

    pub fn join(&mut self, other: &Folded) {
        for (entity, state) in other.entities.iter() {
            match self.entities.get(entity) {
                None => {
                    self.entities.insert(*entity, Arc::clone(state));
                }
                Some(held) if Arc::ptr_eq(held, state) => {}
                Some(_) => {
                    let held = self.entities.get_mut(entity).expect("held above");
                    Arc::make_mut(held).join(state);
                }
            }
        }
        for (&key, trashed) in &other.trash {
            match self.trash.entry(key) {
                Slot::Vacant(slot) => {
                    slot.insert(trashed.clone());
                }
                Slot::Occupied(mut slot) => slot.get_mut().join(trashed),
            }
        }
        self.settled.extend(&other.settled);
        self.unknown.extend(other.unknown.iter().cloned());
        self.extended.extend(other.extended.iter().cloned());
        self.extra.join(&other.extra);
    }

    /// Each part of an entity that a write `before` does not hold changed, with
    /// the writer of that write, whether the write came in an entry or a snapshot.
    pub fn since(&self, before: &Folded) -> Vec<(EntityId, Part, WriterId)> {
        let mut found = Vec::new();
        for pair in self.entities.diff(&before.entities) {
            let (Some((entity, now)), old) = pair else {
                continue;
            };
            let (entity, old) = (*entity, old.map(|(_, old)| old));
            if old.is_some_and(|old| Arc::ptr_eq(old, now)) {
                continue;
            }
            let mut note = |part: Part, stamp: &Stamp| found.push((entity, part, stamp.by));
            for (stamp, existence) in added(&now.existence.writes, old.map(|o| &o.existence.writes))
            {
                let part = match existence {
                    Existence::Created => Part::Created,
                    Existence::Deleted { .. } => Part::Deleted,
                };
                note(part, stamp);
            }
            for (key, register) in now.registers.iter() {
                let old = old.and_then(|o| o.registers.get(key.as_str()));
                for (stamp, _) in added(&register.writes, old.map(|o| &o.writes)) {
                    note(Part::Field(key.as_str().to_owned()), stamp);
                }
            }
            for (key, set) in now.sets.iter() {
                let old = old.and_then(|o| o.sets.get(key.as_str()));
                for stamp in added(&set.adds, old.map(|o| &o.adds)) {
                    note(Part::Field(key.as_str().to_owned()), stamp);
                }
                for (stamp, _) in added(&set.removed, old.map(|o| &o.removed)) {
                    note(Part::Field(key.as_str().to_owned()), stamp);
                }
            }
            for (stamp, _) in added(&now.file.writes, old.map(|o| &o.file.writes)) {
                note(Part::File, stamp);
            }
        }
        found
    }

    /// What this state shows that `kept`, the state of some of its entries, does
    /// not, each with the op that makes `kept` show it too. A register or file
    /// showing several values `kept` lacks comes back as the latest of them.
    pub fn beyond(&self, kept: &Folded) -> Vec<Beyond> {
        let none = Arc::default();
        let mut found = Vec::new();
        for (entity, now) in self.entities.iter() {
            let entity = *entity;
            let old = kept.entities.get(&entity).unwrap_or(&none);
            let Some(latest) = now.newest_beyond(old) else {
                continue;
            };
            let mut note = |part: Part, by: Option<WriterId>, op: Op| {
                let by = by.unwrap_or(latest);
                found.push(Beyond {
                    entity,
                    part,
                    by,
                    op,
                });
            };
            let replaces = old.existence.survivors().map(|(entry, ..)| entry).collect();
            let by = now.existence.newest_beyond(&old.existence);
            match (now.present(), old.present()) {
                (false, false) => continue,
                (false, true) => {
                    let observed = old.live().into_keys().collect();
                    let op = Op::Delete {
                        entity,
                        replaces,
                        observed,
                    };
                    note(Part::Deleted, by, op);
                    continue;
                }
                (true, false) => note(Part::Created, by, Op::Create { entity, replaces }),
                (true, true) => {}
            }
            let empty = Register::default();
            let keys = now.registers.keys().chain(old.registers.keys());
            let keys: BTreeSet<&str> = keys.map(Key::as_str).collect();
            for key in keys {
                let n = now.registers.get(key).unwrap_or(&empty);
                let o = old.registers.get(key).unwrap_or(&empty);
                if let Some((by, value, replaces)) = n.rewrite(o) {
                    let op = Op::Write {
                        entity,
                        key: key.to_owned(),
                        value,
                        replaces,
                    };
                    note(Part::Field(key.to_owned()), by, op);
                }
            }
            let empty = OrSet::default();
            let keys = now.sets.keys().chain(old.sets.keys());
            let keys: BTreeSet<&str> = keys.map(Key::as_str).collect();
            for key in keys {
                let n = now.sets.get(key).unwrap_or(&empty);
                let o = old.sets.get(key).unwrap_or(&empty);
                for (by, op) in n.rewrite(o, entity, key) {
                    note(Part::Field(key.to_owned()), by, op);
                }
            }
            if let Some((by, file, replaces)) = now.file.rewrite(&old.file) {
                let op = match file {
                    Some(file) => Op::Pin {
                        entity,
                        file: file.into(),
                        replaces,
                    },
                    None => Op::File {
                        entity,
                        file: None,
                        replaces,
                    },
                };
                note(Part::File, by, op);
            }
        }
        found
    }

    /// The entities whose state differs from `before`'s, by id. A state a clone
    /// still shares with `before` is unchanged without being compared, so this
    /// costs a step per entity.
    pub fn changed(&self, before: &Folded) -> Vec<EntityId> {
        let pairs = self.entities.diff(&before.entities);
        let differ = pairs.filter_map(|pair| match pair {
            (Some((_, now)), Some((_, was))) if Arc::ptr_eq(was, now) || was == now => None,
            (Some((entity, _)), _) | (None, Some((entity, _))) => Some(*entity),
            (None, None) => None,
        });
        differ.collect()
    }

    /// What `entity` shows that a view indexes; `None` unless it is shown.
    pub fn shown(&self, entity: EntityId) -> Option<Shown<'_>> {
        self.entities.get(&entity)?.shown()
    }

    /// [`Folded::shown`] of every entity shown after `after`, by id, each with
    /// the id of every entity the iterator passed.
    pub fn each_shown_after(
        &self,
        after: Option<EntityId>,
    ) -> impl Iterator<Item = (EntityId, Option<Shown<'_>>)> {
        let entities = self.entities.after(after);
        entities.map(|(entity, state)| (*entity, state.shown()))
    }

    /// The path of `entity`'s latest surviving file write that names a file, while
    /// it is shown.
    pub fn logged_path(&self, entity: EntityId) -> Option<&RelPath> {
        let state = self.entities.get(&entity).filter(|state| state.present())?;
        state.latest_file().map(|file| &file.path)
    }

    /// How many entities the state holds anything of, shown or not.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Every entity that exists, or whose deletion is in conflict.
    pub fn entities(&self) -> Vec<EntityId> {
        self.entities
            .iter()
            .filter(|(_, state)| state.present())
            .map(|(entity, _)| *entity)
            .collect()
    }

    /// Whether `entity` exists, or its deletion is in conflict.
    pub fn present(&self, entity: EntityId) -> bool {
        self.entities
            .get(&entity)
            .is_some_and(|state| state.present())
    }

    /// Whether a surviving delete of `entity` and a write it did not observe are
    /// both in effect: a surviving create or a live field write.
    pub fn deletion_conflicted(&self, entity: EntityId) -> bool {
        self.entities
            .get(&entity)
            .is_some_and(|state| state.deletion_conflicted())
    }

    /// The surviving writes of `entity`'s existence register, oldest first.
    pub fn existence(&self, entity: EntityId) -> Vec<Write<Exists>> {
        let Some(state) = self.entities.get(&entity) else {
            return Vec::new();
        };
        let survivors = state.existence.survivors().map(|(entry, stamp, value)| {
            let exists = match value {
                Existence::Created => Exists::Created,
                Existence::Deleted { .. } => Exists::Deleted,
            };
            stamp.write(entry, exists)
        });
        oldest_first(survivors)
    }

    /// The live field writes of `entity`, which a delete now would observe, oldest
    /// first.
    pub fn live_writes(&self, entity: EntityId) -> Vec<Write<()>> {
        let Some(state) = self.entities.get(&entity) else {
            return Vec::new();
        };
        oldest_first(
            state
                .live()
                .into_iter()
                .map(|(entry, stamp)| stamp.write(entry, ())),
        )
    }

    /// The surviving writes of a register as JSON; empty when unset. A surviving
    /// clear holds no value and is left out.
    pub fn register(&self, entity: EntityId, key: &str) -> Vec<Written<Raw>> {
        self.register_of(entity, key)
            .map(written)
            .unwrap_or_default()
    }

    /// The surviving writes of a register, clears included, oldest first: what a
    /// new write replaces.
    pub fn register_writes(&self, entity: EntityId, key: &str) -> Vec<Write<Option<Raw>>> {
        self.register_of(entity, key)
            .map(Register::surviving)
            .unwrap_or_default()
    }

    /// Any write of a register, surviving or replaced.
    pub fn register_write(
        &self,
        entity: EntityId,
        key: &str,
        entry: EntryHash,
    ) -> Option<Write<Option<Raw>>> {
        self.register_of(entity, key)?.get(entry)
    }

    fn register_of(&self, entity: EntityId, key: &str) -> Option<&Register<Option<Raw>>> {
        self.entities.get(&entity)?.registers.get(key)
    }

    /// The members of a set as JSON, sorted and without duplicates.
    pub fn members(&self, entity: EntityId, key: &str) -> Vec<Raw> {
        let Some(set) = self.set_of(entity, key) else {
            return Vec::new();
        };
        let members: BTreeSet<&Raw> = set.adds.keys().map(|(value, _)| value).collect();
        members.into_iter().cloned().collect()
    }

    /// The live adds of `value` to a set, oldest first. Each add's entry is its
    /// tag.
    pub fn tags(&self, entity: EntityId, key: &str, value: &Raw) -> Vec<Write<()>> {
        let Some(set) = self.set_of(entity, key) else {
            return Vec::new();
        };
        let adds = set
            .adds
            .iter()
            .filter(|((member, _), _)| member == value)
            .map(|((_, tag), stamp)| stamp.write(*tag, ()));
        oldest_first(adds)
    }

    /// The earliest remove of the add of `value` tagged `tag`, if one is folded.
    pub fn removal(
        &self,
        entity: EntityId,
        key: &str,
        value: &Raw,
        tag: EntryHash,
    ) -> Option<Write<()>> {
        let set = self.set_of(entity, key)?;
        let (stamp, remove) = set.removed.get(&(value.clone(), tag))?;
        Some(stamp.write(*remove, ()))
    }

    fn set_of(&self, entity: EntityId, key: &str) -> Option<&OrSet> {
        self.entities.get(&entity)?.sets.get(key)
    }

    /// The surviving writes of every existing entity's file register that say
    /// where its file is. More than one is a conflict.
    pub fn files(&self) -> Facts {
        let mut files = Facts::default();
        self.files_slice(&mut files, &mut None, usize::MAX);
        files
    }

    /// [`Folded::files`] a slice at a time: adds to `files` those of at most
    /// `slice` entities after `after`, and moves `after` on; true once no entity
    /// is left.
    pub(crate) fn files_slice(
        &self,
        files: &mut Facts,
        after: &mut Option<EntityId>,
        slice: usize,
    ) -> bool {
        let from = *after;
        let mut entities = self.entities.after(from);
        for (entity, state) in entities.by_ref().take(slice) {
            let entity = *entity;
            *after = Some(entity);
            if !state.present() {
                continue;
            }
            let writes = state.file_facts();
            if !writes.is_empty() {
                files.insert(entity, writes);
            }
        }
        entities.next().is_none()
    }

    /// [`Folded::files`] of one entity: empty unless it exists.
    pub fn file(&self, entity: EntityId) -> Box<[Fact]> {
        self.entities
            .get(&entity)
            .filter(|state| state.present())
            .map(|state| state.file_facts())
            .unwrap_or_default()
    }

    /// The surviving writes of `entity`'s file register, `None` where a write says
    /// it has no file, oldest first: what a new file write replaces.
    pub fn file_writes(&self, entity: EntityId) -> Vec<Write<Option<FileFact>>> {
        self.entities
            .get(&entity)
            .map(|state| state.file.surviving().into_iter().map(file_write).collect())
            .unwrap_or_default()
    }

    /// Any write of `entity`'s file register, surviving or replaced.
    pub fn file_write(
        &self,
        entity: EntityId,
        entry: EntryHash,
    ) -> Option<Write<Option<FileFact>>> {
        self.entities.get(&entity)?.file.get(entry).map(file_write)
    }

    /// The names of `entity`'s registers that were ever written.
    pub fn registers(&self, entity: EntityId) -> Vec<&str> {
        self.entities
            .get(&entity)
            .map(|state| state.registers.keys().map(Key::as_str).collect())
            .unwrap_or_default()
    }

    /// The bytes `writer` displaced into its trash, oldest first, as its entries
    /// logged them.
    pub fn trash(&self, writer: WriterId) -> Vec<TrashItem> {
        let mut items: Vec<_> = self
            .trash
            .range((writer, Nonce::from_u128(0))..=(writer, Nonce::from_u128(u128::MAX)))
            .map(|(&(_, item), trashed)| TrashItem {
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

    /// Whether some writer settled `writer`'s unfinished effect `record`.
    pub fn settled(&self, writer: WriterId, record: Nonce) -> bool {
        self.settled.contains(&(writer, record))
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

/// `hashes` sorted, without repeats.
fn sorted(mut hashes: Vec<EntryHash>) -> Box<[EntryHash]> {
    hashes.sort_unstable();
    hashes.dedup();
    hashes.into()
}

fn oldest_first<V>(writes: impl Iterator<Item = Write<V>>) -> Vec<Write<V>> {
    let mut writes: Vec<_> = writes.collect();
    writes.sort_by_key(|write| (write.at, write.by, write.entry));
    writes
}

fn file_write(write: Write<Option<FileValue>>) -> Write<Option<FileFact>> {
    Write {
        entry: write.entry,
        by: write.by,
        at: write.at,
        value: write.value.map(FileFact::from),
    }
}

/// The join of every writer's snapshots and placed entries.
pub fn merge<'a>(logs: impl IntoIterator<Item = &'a WriterLog>) -> Folded {
    let mut merging = Merging::new(logs);
    merging.step(usize::MAX);
    merging.folded
}

/// What [`merge`] folds, taken from the logs, so that it can be folded a slice at
/// a time.
pub(crate) struct Merging {
    folded: Folded,
    parts: std::vec::IntoIter<Fold>,
}

/// One part of what a merge folds.
pub(crate) enum Fold {
    Join(Rc<Snapshot>),
    Apply(WriterId, Rc<Entry>),
}

impl Merging {
    pub(crate) fn new<'a>(logs: impl IntoIterator<Item = &'a WriterLog>) -> Self {
        let mut parts = Vec::new();
        for log in logs {
            parts.extend(log.snapshots().iter().cloned().map(Fold::Join));
            let entries = log.entries().iter().cloned();
            parts.extend(entries.map(|entry| Fold::Apply(log.writer(), entry)));
        }
        Self::onto(Folded::default(), parts)
    }

    /// Folds `parts`, in order, into `folded`.
    pub(crate) fn onto(folded: Folded, parts: Vec<Fold>) -> Self {
        Self {
            folded,
            parts: parts.into_iter(),
        }
    }

    /// Folds what is next, until its weight reaches `slice` or nothing is left;
    /// true once nothing is.
    pub(crate) fn step(&mut self, slice: usize) -> bool {
        let mut done = 0;
        while done < slice {
            let Some(part) = self.parts.next() else {
                return true;
            };
            // A snapshot weighs its entities, and an entry its ops.
            let weight = match part {
                Fold::Join(snapshot) => {
                    self.folded.join(&snapshot.state);
                    snapshot.state.entities.len()
                }
                Fold::Apply(writer, entry) => {
                    let kind = entry.kind();
                    self.folded.fold(writer, &entry, &kind);
                    match &kind {
                        EntryKind::Intent(logged) => logged.ops.len() + logged.displaced.len(),
                        _ => 1,
                    }
                }
            };
            done = done.saturating_add(weight.max(1));
        }
        self.parts.len() == 0
    }

    /// ⚠️ Before [`Merging::step`] returns true, the state of what it folded so far.
    pub(crate) fn folded(self) -> Folded {
        self.folded
    }
}

impl<K: Pack + Ord, V: Pack> Pack for SmallMap<K, V> {
    fn pack(&self, out: &mut Vec<u8>) {
        self.len().pack(out);
        for (key, value) in self.iter() {
            (key, value).pack(out);
        }
    }
}

/// Refuses keys out of order, which no packing writes.
impl<K: Unpack + Ord, V: Unpack> Unpack for SmallMap<K, V> {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        SmallMap::from_sorted(Unpack::unpack(input)?).ok_or(Bad("keys out of order"))
    }
}

impl Pack for Key {
    fn pack(&self, out: &mut Vec<u8>) {
        self.as_str().pack(out);
    }
}

impl Unpack for Key {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        input.str().map(Key::of)
    }
}

impl Pack for Extra {
    fn pack(&self, out: &mut Vec<u8>) {
        let members: Vec<(&String, &Raw)> = self.members().collect();
        members.pack(out);
    }
}

impl Unpack for Extra {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        BTreeMap::unpack(input).map(Extra::from)
    }
}

impl<K: Pack> Pack for Extras<K> {
    fn pack(&self, out: &mut Vec<u8>) {
        let extras: Vec<(&K, &Extra)> = self.0.iter().flat_map(|extras| extras.iter()).collect();
        extras.pack(out);
    }
}

impl<K: Unpack + Ord> Unpack for Extras<K> {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let extras = BTreeMap::<K, Extra>::unpack(input)?;
        if extras.values().any(Extra::is_empty) {
            return Err(Bad("an empty record of unknown members"));
        }
        Ok(Self((!extras.is_empty()).then(|| Box::new(extras))))
    }
}

pack_struct!(Stamp { at, by });

impl Pack for Existence {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Created => out.push(0),
            Self::Deleted { observed } => {
                out.push(1);
                observed.pack(out);
            }
        }
    }
}

impl Unpack for Existence {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        match input.byte()? {
            0 => Ok(Self::Created),
            1 => {
                let observed = Vec::<EntryHash>::unpack(input)?;
                if !observed.windows(2).all(|pair| pair[0] < pair[1]) {
                    return Err(Bad("observed writes out of order"));
                }
                Ok(Self::Deleted {
                    observed: observed.into(),
                })
            }
            _ => bad_variant(),
        }
    }
}

impl<V: Pack> Pack for Register<V> {
    fn pack(&self, out: &mut Vec<u8>) {
        (&self.writes, &self.replaced).pack(out);
        (&self.extra, &self.write_extra).pack(out);
    }
}

impl<V: Unpack> Unpack for Register<V> {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let (writes, replaced) = Unpack::unpack(input)?;
        let (extra, write_extra) = Unpack::unpack(input)?;
        Ok(Self {
            writes,
            replaced,
            extra,
            write_extra,
        })
    }
}

pack_struct!(OrSet {
    adds,
    removed,
    extra,
    add_extra,
    removed_extra
});
pack_struct!(FileValue {
    path,
    identity,
    len,
    modified,
    extra
});
pack_struct!(EntityState {
    existence,
    registers,
    sets,
    file,
    extra
});
pack_struct!(Trashed {
    entry,
    at,
    from,
    identity,
    len,
    extra
});

impl Pack for Arc<EntityState> {
    fn pack(&self, out: &mut Vec<u8>) {
        (**self).pack(out);
    }
}

impl Unpack for Arc<EntityState> {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        EntityState::unpack(input).map(Arc::new)
    }
}

/// How an install keeps a merged state for itself, beside snapshots and in its
/// cached view; a snapshot in the folder is JSON.
impl Pack for Folded {
    fn pack(&self, out: &mut Vec<u8>) {
        self.entities.len().pack(out);
        self.pack_entities(None, usize::MAX, out);
        self.pack_rest(out);
    }
}

impl Folded {
    /// Packs the next `slice` entities after `after`, as [`Pack::pack`] packs
    /// them once it packed how many there are. Returns the last one packed;
    /// `None` once none is left.
    pub(crate) fn pack_entities(
        &self,
        after: Option<EntityId>,
        slice: usize,
        out: &mut Vec<u8>,
    ) -> Option<EntityId> {
        let mut last = None;
        for (entity, state) in self.entities.after(after).take(slice) {
            (entity, state).pack(out);
            last = Some(*entity);
        }
        last
    }

    /// Packs what [`Pack::pack`] packs after the entities.
    pub(crate) fn pack_rest(&self, out: &mut Vec<u8>) {
        self.trash.pack(out);
        let settled: Vec<&(WriterId, Nonce)> = self.settled.iter().collect();
        let unknown: Vec<&(EntryHash, Raw)> = self.unknown.iter().collect();
        let extended: Vec<&(EntryHash, Raw)> = self.extended.iter().collect();
        (settled, (unknown, extended)).pack(out);
        self.extra.pack(out);
    }
}

impl Unpack for Folded {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let mut unpacking = Unpacking::start(input)?;
        while !unpacking.step(input, usize::MAX)? {}
        unpacking.finish(input)
    }
}

/// A merged state being unpacked a slice of entities at a time.
pub(crate) struct Unpacking {
    entities: Vec<(EntityId, Arc<EntityState>)>,
    left: usize,
}

impl Unpacking {
    /// Starts unpacking a state [`Folded::pack`] packed: reads how many entities
    /// it holds.
    pub(crate) fn start(input: &mut In<'_>) -> Unpacked<Self> {
        let left = input.len()?;
        let entities = Vec::with_capacity(left.min(crate::pack::RESERVED));
        Ok(Self { entities, left })
    }

    /// Unpacks up to `slice` more entities, refusing them out of order; true once
    /// every one is.
    pub(crate) fn step(&mut self, input: &mut In<'_>, slice: usize) -> Unpacked<bool> {
        for _ in 0..slice.min(self.left) {
            let (entity, state) = <(EntityId, Arc<EntityState>)>::unpack(input)?;
            if self
                .entities
                .last()
                .is_some_and(|(last, _)| *last >= entity)
            {
                return Err(Bad("keys out of order"));
            }
            self.entities.push((entity, state));
            self.left -= 1;
        }
        Ok(self.left == 0)
    }

    /// ⚠️ Before [`Unpacking::step`] returns true, a state lacking entities.
    /// Unpacks what follows the entities.
    pub(crate) fn finish(self, input: &mut In<'_>) -> Unpacked<Folded> {
        let trash = Unpack::unpack(input)?;
        let (settled, (unknown, extended)) = Unpack::unpack(input)?;
        Ok(Folded {
            entities: CowMap::from_sorted(self.entities),
            trash,
            settled: distinct(settled)?,
            unknown: distinct(unknown)?,
            extended: distinct(extended)?,
            extra: Extra::unpack(input)?,
        })
    }
}

/// The set of `list`, refused when it names a member twice.
fn distinct<T: Ord>(list: Vec<T>) -> Unpacked<BTreeSet<T>> {
    let len = list.len();
    let set: BTreeSet<T> = list.into_iter().collect();
    match set.len() == len {
        true => Ok(set),
        false => Err(Bad("a member twice")),
    }
}

const ENTITIES: &str = "entities";
const TRASH: &str = "trash";
const SETTLED: &str = "settled";
const UNKNOWN: &str = "unknown";
const EXTENDED: &str = "extended";

/// A record of a snapshot's state with the members this build does not know.
struct Record<T> {
    known: T,
    extra: Extra,
}

/// The members of a record that this build knows.
trait Members {
    const MEMBERS: &'static [&'static str];
}

impl<T> Record<T> {
    fn new(known: T, extra: Extra) -> Self {
        Self { known, extra }
    }
}

impl<T: Default> Default for Record<T> {
    fn default() -> Self {
        Self::new(T::default(), Extra::default())
    }
}

impl<T: Serialize> Serialize for Record<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a, T> {
            #[serde(flatten)]
            known: &'a T,
            #[serde(flatten)]
            extra: &'a Extra,
        }
        Out {
            known: &self.known,
            extra: &self.extra,
        }
        .serialize(serializer)
    }
}

/// Reads the record's text twice, as `T` and as members: `#[serde(flatten)]`
/// cannot hold a [`Raw`].
impl<'de, T: DeserializeOwned + Members> Deserialize<'de> for Record<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Raw::deserialize(deserializer)?;
        let known = raw.decode().map_err(D::Error::custom)?;
        let mut members: BTreeMap<String, Raw> = raw.decode().map_err(D::Error::custom)?;
        members.retain(|name, _| !T::MEMBERS.contains(&name.as_str()));
        Ok(Self::new(known, Extra::from(members)))
    }
}

#[derive(Serialize, Deserialize)]
struct EntityRecord {
    #[serde(default, skip_serializing_if = "Record::is_empty")]
    existence: Record<RegisterRecord<ExistenceWrite>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    registers: BTreeMap<String, Record<RegisterRecord<ValueWrite>>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sets: BTreeMap<String, Record<SetRecord>>,
    #[serde(default, skip_serializing_if = "Record::is_empty")]
    file: Record<RegisterRecord<FileWrite>>,
}

#[derive(Serialize, Deserialize)]
#[serde(bound(
    serialize = "W: Serialize",
    deserialize = "W: DeserializeOwned + Members"
))]
struct RegisterRecord<W> {
    #[serde(default = "Vec::new")]
    writes: Vec<Record<W>>,
    #[serde(default)]
    replaced: Vec<EntryHash>,
}

/// A create, or a delete with the writes it observed.
#[derive(Serialize, Deserialize)]
struct ExistenceWrite {
    entry: EntryHash,
    by: WriterId,
    at: Hlc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deleted: Option<Vec<EntryHash>>,
}

/// A register write; `value` is absent for a clear, and a JSON `null` is a value.
#[derive(Serialize, Deserialize)]
struct ValueWrite {
    entry: EntryHash,
    by: WriterId,
    at: Hlc,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    value: Option<Raw>,
}

#[derive(Serialize, Deserialize)]
struct FileWrite {
    entry: EntryHash,
    by: WriterId,
    at: Hlc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<Record<FileFact>>,
}

/// The serialized form of one register write.
trait WriteRecord {
    type Value;

    fn new(entry: EntryHash, stamp: Stamp, value: &Self::Value) -> Self;

    fn into_parts(self) -> (EntryHash, Stamp, Self::Value);
}

impl WriteRecord for ExistenceWrite {
    type Value = Existence;

    fn new(entry: EntryHash, stamp: Stamp, value: &Existence) -> Self {
        let deleted = match value {
            Existence::Created => None,
            Existence::Deleted { observed } => Some(observed.to_vec()),
        };
        Self {
            entry,
            by: stamp.by,
            at: stamp.at,
            deleted,
        }
    }

    fn into_parts(self) -> (EntryHash, Stamp, Existence) {
        let value = match self.deleted {
            None => Existence::Created,
            Some(observed) => Existence::Deleted {
                observed: sorted(observed),
            },
        };
        let stamp = Stamp {
            at: self.at,
            by: self.by,
        };
        (self.entry, stamp, value)
    }
}

impl WriteRecord for ValueWrite {
    type Value = Option<Raw>;

    fn new(entry: EntryHash, stamp: Stamp, value: &Option<Raw>) -> Self {
        Self {
            entry,
            by: stamp.by,
            at: stamp.at,
            value: value.clone(),
        }
    }

    fn into_parts(self) -> (EntryHash, Stamp, Option<Raw>) {
        let stamp = Stamp {
            at: self.at,
            by: self.by,
        };
        (self.entry, stamp, self.value)
    }
}

impl WriteRecord for FileWrite {
    type Value = Option<FileValue>;

    fn new(entry: EntryHash, stamp: Stamp, value: &Option<FileValue>) -> Self {
        let file = value.as_ref().map(|value| {
            let fact = FileFact::from(value.clone());
            Record::new(fact, value.extra.clone())
        });
        Self {
            entry,
            by: stamp.by,
            at: stamp.at,
            file,
        }
    }

    fn into_parts(self) -> (EntryHash, Stamp, Option<FileValue>) {
        let stamp = Stamp {
            at: self.at,
            by: self.by,
        };
        let file = self.file.map(|record| FileValue {
            extra: record.extra,
            ..FileValue::from(&record.known)
        });
        (self.entry, stamp, file)
    }
}

#[derive(Serialize, Deserialize)]
struct SetRecord {
    #[serde(default)]
    adds: Vec<Record<AddRecord>>,
    #[serde(default)]
    removed: Vec<Record<RemovedRecord>>,
}

#[derive(Serialize, Deserialize)]
struct AddRecord {
    value: Raw,
    entry: EntryHash,
    by: WriterId,
    at: Hlc,
}

/// A removed tag, with the earliest remove naming it.
#[derive(Serialize, Deserialize)]
struct RemovedRecord {
    value: Raw,
    entry: EntryHash,
    by: WriterId,
    at: Hlc,
    remove: EntryHash,
}

#[derive(Serialize, Deserialize)]
struct TrashRecord {
    writer: WriterId,
    item: Nonce,
    entry: EntryHash,
    at: Hlc,
    from: RelPath,
    identity: Identity,
    len: u64,
}

impl Members for EntityRecord {
    const MEMBERS: &'static [&'static str] = &["existence", "registers", "sets", "file"];
}

impl<W> Members for RegisterRecord<W> {
    const MEMBERS: &'static [&'static str] = &["writes", "replaced"];
}

impl Members for ExistenceWrite {
    const MEMBERS: &'static [&'static str] = &["entry", "by", "at", "deleted"];
}

impl Members for ValueWrite {
    const MEMBERS: &'static [&'static str] = &["entry", "by", "at", "value"];
}

impl Members for FileWrite {
    const MEMBERS: &'static [&'static str] = &["entry", "by", "at", "file"];
}

impl Members for FileFact {
    const MEMBERS: &'static [&'static str] = &["path", "identity", "len", "modified"];
}

impl Members for SetRecord {
    const MEMBERS: &'static [&'static str] = &["adds", "removed"];
}

impl Members for AddRecord {
    const MEMBERS: &'static [&'static str] = &["value", "entry", "by", "at"];
}

impl Members for RemovedRecord {
    const MEMBERS: &'static [&'static str] = &["value", "entry", "by", "at", "remove"];
}

impl Members for TrashRecord {
    const MEMBERS: &'static [&'static str] =
        &["writer", "item", "entry", "at", "from", "identity", "len"];
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Raw>, D::Error> {
    Raw::deserialize(deserializer).map(Some)
}

impl<T: Empty> Record<T> {
    fn is_empty(&self) -> bool {
        self.known.is_empty() && self.extra.is_empty()
    }
}

/// A record that serializes as nothing worth keeping.
trait Empty {
    fn is_empty(&self) -> bool;
}

impl<W> Empty for RegisterRecord<W> {
    fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.replaced.is_empty()
    }
}

impl<W> Default for RegisterRecord<W> {
    fn default() -> Self {
        Self {
            writes: Vec::new(),
            replaced: Vec::new(),
        }
    }
}

impl<W: WriteRecord<Value: Ord + Clone>> From<&Register<W::Value>> for Record<RegisterRecord<W>> {
    fn from(register: &Register<W::Value>) -> Self {
        let writes = register.writes.iter().map(|(entry, (stamp, value))| {
            let write = W::new(*entry, *stamp, value);
            Record::new(write, register.write_extra.of(entry))
        });
        let known = RegisterRecord {
            writes: writes.collect(),
            replaced: register.replaced.keys().copied().collect(),
        };
        Record::new(known, register.extra.clone())
    }
}

impl<W: WriteRecord<Value: Ord + Clone>> From<Record<RegisterRecord<W>>> for Register<W::Value> {
    fn from(record: Record<RegisterRecord<W>>) -> Self {
        let mut register = Register::default();
        for write in record.known.writes {
            let (entry, stamp, value) = write.known.into_parts();
            register.insert(entry, stamp, value);
            register.write_extra.join_one(&entry, &write.extra);
        }
        register.replaced.extend(record.known.replaced);
        register.extra = record.extra;
        register
    }
}

impl From<&EntityState> for Record<EntityRecord> {
    fn from(state: &EntityState) -> Self {
        let known = EntityRecord {
            existence: (&state.existence).into(),
            registers: state
                .registers
                .iter()
                .map(|(key, register)| (key.as_str().to_owned(), register.into()))
                .collect(),
            sets: state
                .sets
                .iter()
                .map(|(key, set)| (key.as_str().to_owned(), set.into()))
                .collect(),
            file: (&state.file).into(),
        };
        Record::new(known, state.extra.clone())
    }
}

impl From<Record<EntityRecord>> for EntityState {
    fn from(record: Record<EntityRecord>) -> Self {
        let known = record.known;
        Self {
            existence: known.existence.into(),
            registers: known
                .registers
                .into_iter()
                .map(|(key, register)| (Key::of(&key), register.into()))
                .collect(),
            sets: known
                .sets
                .into_iter()
                .map(|(key, set)| (Key::of(&key), set.into()))
                .collect(),
            file: known.file.into(),
            extra: record.extra,
        }
    }
}

impl From<&OrSet> for Record<SetRecord> {
    fn from(set: &OrSet) -> Self {
        let adds = set.adds.iter().map(|(key, stamp)| {
            let (value, entry) = key;
            let add = AddRecord {
                value: value.clone(),
                entry: *entry,
                by: stamp.by,
                at: stamp.at,
            };
            Record::new(add, set.add_extra.of(key))
        });
        let removed = set.removed.iter().map(|(key, (stamp, remove))| {
            let (value, entry) = key;
            let removed = RemovedRecord {
                value: value.clone(),
                entry: *entry,
                by: stamp.by,
                at: stamp.at,
                remove: *remove,
            };
            Record::new(removed, set.removed_extra.of(key))
        });
        let known = SetRecord {
            adds: adds.collect(),
            removed: removed.collect(),
        };
        Record::new(known, set.extra.clone())
    }
}

impl From<Record<SetRecord>> for OrSet {
    fn from(record: Record<SetRecord>) -> Self {
        let mut set = OrSet::default();
        for Record { known, extra } in record.known.removed {
            let stamp = Stamp {
                at: known.at,
                by: known.by,
            };
            set.remove(&known.value, &[known.entry], (stamp, known.remove));
            set.removed_extra
                .join_one(&(known.value, known.entry), &extra);
        }
        for Record { known, extra } in record.known.adds {
            let stamp = Stamp {
                at: known.at,
                by: known.by,
            };
            set.add(known.value, known.entry, stamp, &extra);
        }
        set.extra = record.extra;
        set
    }
}

/// Snapshots and the cached view store this; members this build does not know
/// are kept, in the state and in each of its records. It is JSON only: values are
/// kept as their JSON text.
impl Serialize for Folded {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let entities: BTreeMap<EntityId, Record<EntityRecord>> = self
            .entities
            .iter()
            .map(|(entity, state)| (*entity, Record::from(&**state)))
            .collect();
        let trash: Vec<Record<TrashRecord>> = self
            .trash
            .iter()
            .map(|(&(writer, item), trashed)| {
                let record = TrashRecord {
                    writer,
                    item,
                    entry: trashed.entry,
                    at: trashed.at,
                    from: trashed.from.clone(),
                    identity: trashed.identity,
                    len: trashed.len,
                };
                Record::new(record, trashed.extra.clone())
            })
            .collect();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry(ENTITIES, &entities)?;
        map.serialize_entry(TRASH, &trash)?;
        map.serialize_entry(SETTLED, &self.settled)?;
        map.serialize_entry(UNKNOWN, &self.unknown)?;
        map.serialize_entry(EXTENDED, &self.extended)?;
        for (name, raw) in self.extra.members() {
            map.serialize_entry(name, raw)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Folded {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut members = BTreeMap::<String, Raw>::deserialize(deserializer)?;
        let mut folded = Folded::default();
        if let Some(raw) = members.remove(ENTITIES) {
            let entities: BTreeMap<EntityId, Record<EntityRecord>> =
                raw.decode().map_err(D::Error::custom)?;
            folded.entities = entities
                .into_iter()
                .map(|(entity, record)| (entity, Arc::new(EntityState::from(record))))
                .collect();
        }
        if let Some(raw) = members.remove(TRASH) {
            let trash: Vec<Record<TrashRecord>> = raw.decode().map_err(D::Error::custom)?;
            for Record { known, extra } in trash {
                let displaced = Displaced {
                    item: known.item,
                    from: known.from,
                    identity: known.identity,
                    len: known.len,
                };
                folded.trash_item(known.writer, known.entry, known.at, &displaced, &extra);
            }
        }
        if let Some(raw) = members.remove(SETTLED) {
            folded.settled = raw.decode().map_err(D::Error::custom)?;
        }
        if let Some(raw) = members.remove(UNKNOWN) {
            folded.unknown = raw.decode().map_err(D::Error::custom)?;
        }
        if let Some(raw) = members.remove(EXTENDED) {
            folded.extended = raw.decode().map_err(D::Error::custom)?;
        }
        folded.extra = Extra::from(members);
        Ok(folded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Random, SeededRandom};
    use crate::log::{Genesis, Logged};
    use crate::pack::{packed, unpacked};

    fn h(n: u8) -> String {
        format!("\"{n:032x}\"")
    }

    /// A state holding a record of every kind, each with a member no build knows.
    fn every_record() -> String {
        let (e, w, x) = (h(0xe), h(0xa), h(1));
        let write = |more: &str| format!(r#"{{"entry":{x},"by":{w},"at":[1,0]{more},"z":1}}"#);
        format!(
            concat!(
                r#"{{"entities":{{{e}:{{"#,
                r#""existence":{{"writes":[{created},{deleted}],"replaced":[{x}],"y":2}},"#,
                r#""registers":{{"name":{{"writes":[{value},{clear}],"replaced":[],"w":[1]}}}},"#,
                r#""sets":{{"tags":{{"adds":[{{"value":"a","entry":{x},"by":{w},"at":[2,0],"q":1}}],"#,
                r#""removed":[{{"value":"b","entry":{x},"by":{w},"at":[2,1],"remove":{w},"r":1}}],"s":1}}}},"#,
                r#""file":{{"writes":[{file}],"replaced":[]}},"u":{{"v":1}}}}}},"#,
                r#""trash":[{{"writer":{w},"item":{x},"entry":{x},"at":[3,0],"from":"a.bin","#,
                r#""identity":{x},"len":4,"t":true}}],"#,
                r#""settled":[[{w},{x}]],"unknown":[[{x},{{"op":"later"}}]],"#,
                r#""extended":[[{w},{{"prev":{x}}}]],"top":null}}"#
            ),
            e = e,
            w = w,
            x = x,
            created = write(""),
            deleted = write(&format!(r#","deleted":[{w},{x}]"#))
                .replace(&format!("\"entry\":{x}"), &format!("\"entry\":{w}")),
            value = write(r#","value":{"b":1,"a":2}"#),
            clear = write("").replace(&format!("\"entry\":{x}"), &format!("\"entry\":{e}")),
            file = write(
                r#","file":{"path":"a.bin","identity":"00000000000000000000000000000007","len":3,"modified":4,"p":0}"#
            ),
        )
    }

    #[test]
    fn a_state_packs_and_unpacks_whole_with_members_no_build_knows() {
        let state: Folded = serde_json::from_str(&every_record()).unwrap();
        let text = serde_json::to_string(&state).unwrap();
        for name in [
            "\"y\"", "\"w\"", "\"z\"", "\"q\"", "\"r\"", "\"s\"", "\"u\"", "\"t\"", "\"p\"",
            "\"top\"",
        ] {
            assert!(text.contains(name), "{name} kept in {text}");
        }
        let bytes = packed(&state);
        let read: Folded = unpacked(&bytes).unwrap();
        assert_eq!(read, state);
        assert_eq!(serde_json::to_string(&read).unwrap(), text);
        for cut in 0..bytes.len() {
            assert!(unpacked::<Folded>(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    /// Logs of three writers with random ops on a few entities, each with a
    /// snapshot of a random prefix of its entries.
    fn logs(random: &mut SeededRandom) -> Vec<WriterLog> {
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        let mut logs = Vec::new();
        let mut seen: Vec<EntryHash> = Vec::new();
        for w in 0..3u128 {
            let writer = WriterId::from_u128(w + 1);
            let at = |n: usize| Hlc {
                wall_ms: n as u64,
                counter: 0,
            };
            let genesis = EntryKind::Genesis(Genesis {
                writer,
                label: "w".into(),
            });
            let mut entries = vec![Entry::encode(EntryHash::ZERO, at(0), genesis).unwrap()];
            for n in 1..1 + pick(40) {
                let entity = EntityId::from_u128(1 + pick(4) as u128);
                let replaces: Vec<EntryHash> = (0..pick(3))
                    .filter_map(|_| seen.get(pick(seen.len().max(1))).copied())
                    .collect();
                let value = Raw::of(&format!("v{}", pick(3))).unwrap();
                let ops = (0..1 + pick(3))
                    .map(|_| match pick(5) {
                        0 => Op::Create {
                            entity,
                            replaces: replaces.clone(),
                        },
                        1 => Op::Delete {
                            entity,
                            replaces: replaces.clone(),
                            observed: replaces.clone(),
                        },
                        2 => Op::Write {
                            entity,
                            key: "k".into(),
                            value: Some(value.clone()),
                            replaces: replaces.clone(),
                        },
                        3 => Op::Add {
                            entity,
                            key: "s".into(),
                            value: value.clone(),
                        },
                        _ => Op::Remove {
                            entity,
                            key: "s".into(),
                            value: value.clone(),
                            tags: replaces.clone(),
                        },
                    })
                    .collect();
                let logged = Logged {
                    label: "e".into(),
                    ops,
                    displaced: Vec::new(),
                    reverses: None,
                };
                let prev = entries.last().unwrap().hash();
                let entry = Entry::encode(prev, at(n * 3 + w as usize), EntryKind::Intent(logged));
                let entry = entry.unwrap();
                seen.push(entry.hash());
                entries.push(entry);
            }
            let cut = 1 + pick(entries.len());
            let mut prefix = Folded::default();
            for entry in &entries[..cut] {
                prefix.apply(writer, entry);
            }
            let snapshot = Snapshot {
                writer,
                label: "w".into(),
                at: entries[cut - 1].at(),
                folded: entries[..cut].iter().map(Entry::hash).collect(),
                state: prefix,
                unknown: BTreeMap::new(),
            };
            let mut log = WriterLog::new(writer);
            let rest = entries[cut..].iter().cloned().map(Rc::new).collect();
            log.place(vec![Rc::new(snapshot)], rest);
            logs.push(log);
        }
        logs
    }

    #[test]
    fn merging_and_filing_in_slices_find_what_they_find_at_once() {
        let mut random = SeededRandom::new(5);
        for _ in 0..100 {
            let logs = logs(&mut random);
            let mut whole = Folded::default();
            for log in &logs {
                for snapshot in log.snapshots() {
                    whole.join(&snapshot.state);
                }
                for entry in log.entries() {
                    whole.apply(log.writer(), entry);
                }
            }
            let slice = 1 + (random.next_u128() % 8) as usize;
            let mut merging = Merging::new(&logs);
            while !merging.step(slice) {}
            assert_eq!(merging.folded(), whole, "{slice}-op slices");
            assert_eq!(merge(&logs), whole);
            let (mut files, mut after) = (Facts::new(), None);
            while !whole.files_slice(&mut files, &mut after, slice) {}
            let present = whole.entities.iter().filter(|(_, state)| state.present());
            let expected: Facts = present
                .map(|(entity, state)| (*entity, state.file_facts()))
                .filter(|(_, writes)| !writes.is_empty())
                .collect();
            assert_eq!(files, expected, "{slice}-entity slices of files");
        }
    }
}
