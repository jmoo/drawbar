//! Entries, files and what a reader shows, in the vocabulary of design R
//! (`RETENTION.md`): one entity, its existence, one register and set members.

use std::collections::{BTreeMap, BTreeSet};

/// An entry's hash: its index in [`World::entries`].
pub type Hash = u32;
/// A writer's directory. A clone writes the same directory as its original.
pub type Dir = u8;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Field {
    Exist,
    Reg,
    Member(u8),
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
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
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

/// A run the user dismissed: its writer, its first entry, and the last entry the
/// dismissal covers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Dismissal {
    pub dir: Dir,
    pub first: Ref,
    pub last: Ref,
}

#[derive(Clone, PartialEq, Eq, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Seen {
    pub file: usize,
    pub lines: usize,
}

/// The values a field shows, and the existence of the entity.
pub type Values = BTreeMap<Field, BTreeSet<Val>>;

/// An item the user is shown apart from the merged state: a run of a forked
/// writer, or entries held back after a gap.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ItemKind {
    Run,
    Gap,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Item {
    pub dir: Dir,
    pub kind: ItemKind,
    /// The run's net change: its latest write to each field, where the merged
    /// state shows something else.
    pub change: BTreeMap<Field, Option<Val>>,
    /// Fields in `change` whatever the merged state shows, because the reader
    /// cannot tell whether the run replaced a value it no longer holds.
    pub forgotten: BTreeSet<Field>,
}

/// What a reader shows: the merged values, whether the entity exists and is in
/// a deletion conflict, and the items beside it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Shown {
    pub values: Values,
    /// The live writes behind `values`, by field.
    pub writes: BTreeMap<Field, BTreeSet<Hash>>,
    pub exists: bool,
    pub deletion_conflict: bool,
    pub items: BTreeSet<Item>,
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
    let exist = values.remove(&Field::Exist).unwrap_or_default();
    let fields_live = live
        .iter()
        .any(|l| l.write.field != Field::Exist && l.write.value.is_some());
    let created = exist.contains(&Val::Created);
    let deleted = exist.contains(&Val::Deleted);
    let deletion_conflict = deleted && fields_live;
    // No live existence write is left only where a forgotten basis or a lost
    // write took them all; the live fields still show.
    let exists = created || deletion_conflict || (exist.is_empty() && fields_live);
    if !exists {
        values.clear();
        writes.retain(|f, _| *f == Field::Exist);
    }
    Shown {
        values,
        writes,
        exists,
        deletion_conflict,
        items,
    }
}

/// The net change of a run whose writes, in chain order, are `writes`, against
/// what `values` shows.
pub fn change(writes: &[Write], values: &Values, exists: bool) -> BTreeMap<Field, Option<Val>> {
    let mut latest: BTreeMap<Field, Option<Val>> = BTreeMap::new();
    for w in writes {
        latest.insert(w.field, w.value);
    }
    latest
        .into_iter()
        .filter(|(field, value)| {
            let now = match field {
                Field::Exist => {
                    let v = if exists { Val::Created } else { Val::Deleted };
                    return *value != Some(v);
                }
                _ => values.get(field).cloned().unwrap_or_default(),
            };
            match value {
                Some(v) => now != BTreeSet::from([*v]),
                None => !now.is_empty(),
            }
        })
        .collect()
}
