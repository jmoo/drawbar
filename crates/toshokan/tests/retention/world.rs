//! Entries, files and what a reader shows, in the vocabulary of `RETENTION.md`:
//! entities, their existence, registers and set members.

use std::collections::{BTreeMap, BTreeSet};

/// An entry's hash: its index in [`World::entries`].
pub type Hash = u32;
/// A writer's directory. A clone writes the same directory as its original.
pub type Dir = u8;

/// An entity's field, numbered within the entity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    Exist,
    Reg(u8),
    Member(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Field {
    pub entity: u8,
    pub kind: Kind,
}

impl Field {
    pub const fn exist(entity: u8) -> Self {
        Self {
            entity,
            kind: Kind::Exist,
        }
    }

    pub const fn reg(entity: u8, reg: u8) -> Self {
        Self {
            entity,
            kind: Kind::Reg(reg),
        }
    }

    pub const fn member(entity: u8, member: u8) -> Self {
        Self {
            entity,
            kind: Kind::Member(member),
        }
    }

    pub fn is_exist(&self) -> bool {
        self.kind == Kind::Exist
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Val {
    Created,
    Deleted,
    A,
    B,
    /// A set member's add.
    In,
}

/// A write to one field. `None` is a clear, or a set member's remove.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Write {
    pub field: Field,
    pub value: Option<Val>,
}

/// An entry another entry names: `[writer, n, hash]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Ref {
    pub dir: Dir,
    pub n: u32,
    pub hash: Hash,
}

/// A run, as a dismissal names it: its writer, its first entry and its last. A
/// tip run is named by the tip's head: its first entry is then known only by
/// number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Dismissal {
    pub dir: Dir,
    pub first: Ref,
    pub last: Ref,
    pub tip: Option<Ref>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Entry {
    pub hash: Hash,
    pub dir: Dir,
    pub n: u32,
    pub prev: Option<Hash>,
    pub write: Option<Write>,
    /// The writes of its field it replaced.
    pub replaces: Vec<Ref>,
    /// A delete's: the field writes it observed, which it kills.
    pub observed: Vec<Ref>,
    pub dismisses: Vec<Dismissal>,
    /// A take's: the run whose write it took. It settles its field for the
    /// run's writer below the run.
    pub takes: Option<Dismissal>,
    /// The fork points its writer knew of when it wrote it, by writer.
    pub forks: BTreeMap<Dir, u32>,
}

impl Entry {
    pub fn at(&self) -> Ref {
        Ref {
            dir: self.dir,
            n: self.n,
            hash: self.hash,
        }
    }
}

/// Every entry ever written, by hash. Only the oracle reads entries no visible
/// file holds.
#[derive(Clone, Default, Debug)]
pub struct World {
    pub entries: Vec<Entry>,
}

impl World {
    pub fn entry(&self, hash: Hash) -> &Entry {
        &self.entries[hash as usize]
    }

    /// The chain ending at `head`, from its first entry.
    pub fn chain(&self, head: Hash) -> Vec<Hash> {
        let mut chain = vec![head];
        while let Some(prev) = self.entry(*chain.last().unwrap()).prev {
            chain.push(prev);
        }
        chain.reverse();
        chain
    }

    pub fn ancestors_or_self(&self, hash: Hash) -> BTreeSet<Hash> {
        self.chain(hash).into_iter().collect()
    }
}

/// What a file holds: a segment's lines, or a snapshot of design `S`.
#[derive(Clone, Debug)]
pub enum Body<S> {
    Segment(Vec<Hash>),
    Snapshot(S),
}

#[derive(Clone, Debug)]
pub struct File<S> {
    pub id: usize,
    pub dir: Dir,
    pub body: Body<S>,
    pub sealed: bool,
    pub deleted: bool,
}

/// What a reader sees of one file: a segment's first `lines` lines, or all of a
/// snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Seen {
    pub file: usize,
    pub lines: usize,
}

/// The values each field shows.
pub type Values = BTreeMap<Field, BTreeSet<Val>>;

/// An item the user is shown apart from the merged state: a run of a forked
/// writer, entries held back after a gap, or lines the reader cannot place.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ItemKind {
    Run,
    Gap,
    /// The lines of an unsealed file shorter than a certificate: a version a
    /// sync client dropped, or a copy that diverged.
    Unplaced,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Item {
    pub dir: Dir,
    pub kind: ItemKind,
    /// Which run: dismissing the item names this.
    pub run: Dismissal,
    /// The run's net change: its latest write to each field, where the merged
    /// state shows something else.
    pub change: BTreeMap<Field, Option<Val>>,
    /// The run's latest write to each field in `change`, which taking it names.
    pub writes: BTreeMap<Field, Ref>,
    /// Fields in `change` whatever the merged state shows, because the reader
    /// cannot tell whether the run replaced a value it no longer holds.
    pub forgotten: BTreeSet<Field>,
}

impl Item {
    /// What a reader shows of the item, without the run's identity.
    pub fn view(&self) -> (Dir, ItemKind, BTreeMap<Field, Option<Val>>) {
        (self.dir, self.kind, self.change.clone())
    }
}

/// Whether an entity exists and is in a deletion conflict.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Existence {
    pub exists: bool,
    pub deletion_conflict: bool,
}

/// What a reader shows: the merged values, each entity's existence, the items
/// beside them, and the fields where it reports a contradicted reference.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Shown {
    pub values: Values,
    /// The live writes behind `values`, by field.
    pub writes: BTreeMap<Field, BTreeSet<Hash>>,
    pub entities: BTreeMap<u8, Existence>,
    pub items: BTreeSet<Item>,
    pub contradicted: BTreeSet<Field>,
    /// The lowest fork point of each writer it knows of.
    pub forks: BTreeMap<Dir, u32>,
}

impl Shown {
    pub fn exists(&self, entity: u8) -> bool {
        self.entities.get(&entity).is_some_and(|e| e.exists)
    }

    /// The items as a reader sees them, without the runs' identities.
    pub fn item_views(&self) -> Vec<(Dir, ItemKind, BTreeMap<Field, Option<Val>>)> {
        let mut views: Vec<_> = self.items.iter().map(Item::view).collect();
        views.sort();
        views
    }
}

/// A live write: who wrote it, where, and what.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Live {
    pub at: Ref,
    pub write: Write,
}

/// What a set of live writes shows.
pub fn show(live: &[Live], items: BTreeSet<Item>) -> Shown {
    let mut values: Values = BTreeMap::new();
    let mut writes: BTreeMap<Field, BTreeSet<Hash>> = BTreeMap::new();
    for l in live {
        writes.entry(l.write.field).or_default().insert(l.at.hash);
        if let Some(v) = l.write.value {
            values.entry(l.write.field).or_default().insert(v);
        }
    }
    let mut entities = BTreeMap::new();
    for entity in live
        .iter()
        .map(|l| l.write.field.entity)
        .collect::<BTreeSet<u8>>()
    {
        let exist = values.remove(&Field::exist(entity)).unwrap_or_default();
        let fields_live = live.iter().any(|l| {
            l.write.field.entity == entity && !l.write.field.is_exist() && l.write.value.is_some()
        });
        let deletion_conflict = exist.contains(&Val::Deleted) && fields_live;
        // No live existence write is left only where a forgotten basis or a lost
        // write took them all; the live fields still show.
        let exists =
            exist.contains(&Val::Created) || deletion_conflict || (exist.is_empty() && fields_live);
        if !exists {
            values.retain(|f, _| f.entity != entity);
            writes.retain(|f, _| f.entity != entity || f.is_exist());
        }
        entities.insert(
            entity,
            Existence {
                exists,
                deletion_conflict,
            },
        );
    }
    Shown {
        values,
        writes,
        entities,
        items,
        contradicted: BTreeSet::new(),
        forks: BTreeMap::new(),
    }
}

/// The net change of a run whose writes, in chain order, are `writes`, against
/// what `shown` shows.
pub fn change(writes: &[Live], shown: &Shown) -> BTreeMap<Field, Option<Val>> {
    let mut latest: BTreeMap<Field, Option<Val>> = BTreeMap::new();
    for w in writes {
        latest.insert(w.write.field, w.write.value);
    }
    latest
        .into_iter()
        .filter(|&(field, value)| differs(shown, field, value))
        .collect()
}

/// Whether writing `value` to `field` would change what `shown` shows.
pub fn differs(shown: &Shown, field: Field, value: Option<Val>) -> bool {
    if field.is_exist() {
        let v = if shown.exists(field.entity) {
            Val::Created
        } else {
            Val::Deleted
        };
        return value != Some(v);
    }
    let now = shown.values.get(&field).cloned().unwrap_or_default();
    match value {
        Some(v) => now != BTreeSet::from([v]),
        None => !now.is_empty(),
    }
}

/// The latest write of each field among `writes`, in chain order.
pub fn latest(writes: &[Live]) -> BTreeMap<Field, Ref> {
    writes.iter().map(|w| (w.write.field, w.at)).collect()
}
