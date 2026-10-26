//! The reference: a reader that knows every entry behind every file it sees,
//! never compacts, and merges by exact hash.
//!
//! An entry is merged when its whole chain is visible, it lies at or below its
//! writer's fork point, and no merged dismissal names it or an entry before it.
//! Entries beyond a fork point are runs; entries after a missing predecessor are
//! gaps; entries after a dismissed one are runs. A write is live when no merged
//! entry replaced or observed it.

use std::collections::{BTreeMap, BTreeSet};

use super::world::{
    change, latest, show, Body, Dir, Dismissal, Entry, Field, File, Hash, Item, ItemKind, Live,
    Seen, Shown, Val, World, Write,
};
use super::{Bound, Compaction, Design, Reading};

/// How an oracle places a set of entries.
#[derive(Clone, Debug, Default)]
pub struct Placement {
    pub merged: BTreeSet<Hash>,
    pub runs: BTreeSet<Hash>,
    pub gaps: BTreeSet<Hash>,
    pub dismissed: BTreeSet<Hash>,
    /// Fork points: entries with two successors.
    pub forks: BTreeSet<(Dir, Hash)>,
}

/// The lowest fork point of each directory.
pub fn fork_points(world: &World, forks: &BTreeSet<(Dir, Hash)>) -> BTreeMap<Dir, u32> {
    let mut lowest: BTreeMap<Dir, u32> = BTreeMap::new();
    for &(dir, at) in forks {
        let n = world.entry(at).n;
        let low = lowest.entry(dir).or_insert(n);
        *low = (*low).min(n);
    }
    lowest
}

/// Places `visible`, a set of entries.
pub fn place(world: &World, visible: &BTreeSet<Hash>) -> Placement {
    let complete = |e: Hash| world.chain(e).iter().all(|a| visible.contains(a));
    let mut successors: BTreeMap<Hash, BTreeSet<Hash>> = BTreeMap::new();
    for &e in visible {
        if let Some(prev) = world.entry(e).prev {
            successors.entry(prev).or_default().insert(e);
        }
    }
    let mut forks: BTreeSet<(Dir, Hash)> = BTreeSet::new();
    for (&prev, next) in &successors {
        if next.len() > 1 {
            forks.insert((world.entry(prev).dir, prev));
        }
    }
    let lowest = fork_points(world, &forks);
    let beyond = |e: Hash| {
        let entry = world.entry(e);
        lowest.get(&entry.dir).is_some_and(|f| entry.n > *f)
    };
    let candidates: BTreeSet<Hash> = visible
        .iter()
        .copied()
        .filter(|&e| complete(e) && !beyond(e))
        .collect();
    let mut dismissed = BTreeSet::new();
    for &e in &candidates {
        for d in &world.entry(e).dismisses {
            dismissed.extend(covered(world, d));
        }
    }
    let after: BTreeSet<Hash> = visible
        .iter()
        .copied()
        .filter(|&e| {
            !dismissed.contains(&e) && world.chain(e).iter().any(|a| dismissed.contains(a))
        })
        .collect();
    let merged: BTreeSet<Hash> = candidates
        .iter()
        .copied()
        .filter(|e| !dismissed.contains(e) && !after.contains(e))
        .collect();
    let runs: BTreeSet<Hash> = visible
        .iter()
        .copied()
        .filter(|&e| (beyond(e) || after.contains(&e)) && !dismissed.contains(&e))
        .collect();
    let gaps: BTreeSet<Hash> = visible
        .iter()
        .copied()
        .filter(|&e| !complete(e) && !beyond(e) && !dismissed.contains(&e) && !after.contains(&e))
        .collect();
    Placement {
        merged,
        runs,
        gaps,
        dismissed: dismissed.intersection(visible).copied().collect(),
        forks,
    }
}

/// The entries a dismissal names: from its first entry to its last, on the
/// last one's chain.
pub fn covered(world: &World, d: &super::world::Dismissal) -> BTreeSet<Hash> {
    let chain = world.chain(d.last.hash);
    if d.tip.is_none() && !chain.contains(&d.first.hash) {
        return BTreeSet::from([d.first.hash]);
    }
    chain
        .into_iter()
        .filter(|&a| world.entry(a).n >= d.first.n)
        .collect()
}

/// The live writes among `merged`: those no merged entry replaced or observed,
/// and that no later merged write of their own writer to their field, nor a
/// later own delete, supersedes.
pub fn live(world: &World, merged: &BTreeSet<Hash>) -> Vec<Live> {
    let mut killed = BTreeSet::new();
    for &e in merged {
        let entry = world.entry(e);
        killed.extend(entry.replaces.iter().map(|r| r.hash));
        killed.extend(entry.observed.iter().map(|r| r.hash));
    }
    // A take settles its field for the taken run's writer below the run.
    let settled = |e: &Hash| {
        let entry = world.entry(*e);
        let Some(w) = entry.write else { return false };
        merged.iter().any(|&o| {
            let other = world.entry(o);
            other
                .takes
                .is_some_and(|d| d.dir == entry.dir && entry.n < d.first.n)
                && other.write.is_some_and(|ow| ow.field == w.field)
        })
    };
    let superseded = |e: &Hash| {
        let entry = world.entry(*e);
        let Some(w) = entry.write else { return false };
        merged.iter().any(|&o| {
            let other = world.entry(o);
            other.dir == entry.dir
                && other.n > entry.n
                && other.write.is_some_and(|ow| {
                    ow.field == w.field
                        || (!w.field.is_exist()
                            && ow.field == Field::exist(w.field.entity)
                            && ow.value == Some(Val::Deleted))
                })
        })
    };
    merged
        .iter()
        .filter(|e| !killed.contains(e) && !superseded(e) && !settled(e))
        .filter_map(|&e| {
            let entry = world.entry(e);
            entry.write.map(|write| Live {
                at: entry.at(),
                write,
            })
        })
        .collect()
}

/// What an oracle shows of `placement`.
pub fn shown(world: &World, placement: &Placement) -> Shown {
    shown_within(world, placement, None)
}

/// What an oracle shows of `placement`, with only the entries in `held` showing
/// their own writes when given: the others still replace what they replaced.
pub fn shown_within(world: &World, placement: &Placement, held: Option<&BTreeSet<Hash>>) -> Shown {
    let keep = |e: &Hash| held.is_none_or(|held| held.contains(e));
    let mut live = live(world, &placement.merged);
    live.retain(|write| keep(&write.at.hash));
    let base = show(&live, BTreeSet::new());
    let mut items = BTreeSet::new();
    for (set, kind) in [
        (&placement.runs, ItemKind::Run),
        (&placement.gaps, ItemKind::Gap),
    ] {
        let set: BTreeSet<Hash> = set.iter().copied().filter(keep).collect();
        for group in groups(world, &set) {
            let writes: Vec<Live> = group
                .iter()
                .filter_map(|&e| {
                    let entry = world.entry(e);
                    entry.write.map(|write| Live {
                        at: entry.at(),
                        write,
                    })
                })
                .collect();
            let change = change(&writes, &base);
            if change.is_empty() {
                continue;
            }
            let (first, last) = (world.entry(group[0]), world.entry(*group.last().unwrap()));
            items.insert(Item {
                dir: first.dir,
                kind,
                run: Dismissal {
                    dir: first.dir,
                    first: first.at(),
                    last: last.at(),
                    tip: None,
                },
                writes: latest(&writes)
                    .into_iter()
                    .filter(|(f, _)| change.contains_key(f))
                    .collect(),
                change,
                forgotten: BTreeSet::new(),
            });
        }
    }
    Shown {
        items,
        forks: fork_points(world, &placement.forks),
        ..base
    }
}

/// `set` split into chains: each entry joins the group of its predecessor when
/// that is in `set`. Each group is in chain order.
pub fn groups(world: &World, set: &BTreeSet<Hash>) -> Vec<Vec<Hash>> {
    let mut by_root: BTreeMap<Hash, Vec<Hash>> = BTreeMap::new();
    for &e in set {
        let mut root = e;
        while let Some(prev) = world.entry(root).prev.filter(|p| set.contains(p)) {
            root = prev;
        }
        by_root.entry(root).or_default().push(e);
    }
    by_root
        .into_values()
        .map(|mut g| {
            g.sort_by_key(|&e| world.entry(e).n);
            g
        })
        .collect()
}

/// The oracle as a design: a snapshot is the whole chain it folds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Oracle;

#[derive(Clone, Debug)]
pub struct Folds {
    pub dir: Dir,
    pub head: Hash,
}

/// The entries the files a reader sees hold, the snapshots' whole chains included.
pub fn holds<S>(
    world: &World,
    reading: &Reading<'_, S>,
    head: impl Fn(&S) -> Hash,
) -> BTreeSet<Hash> {
    let mut held = BTreeSet::new();
    for seen in reading.seen {
        match &reading.files[seen.file].body {
            Body::Segment(lines) => held.extend(lines.iter().take(seen.lines)),
            Body::Snapshot(s) => held.extend(world.chain(head(s))),
        }
    }
    held
}

impl Design for Oracle {
    type Snapshot = Folds;

    fn compact(&self, c: &Compaction<'_, Folds>) -> Folds {
        Folds {
            dir: c.dir,
            head: *c.chain.last().unwrap(),
        }
    }

    fn read(&self, r: &Reading<'_, Folds>) -> Shown {
        let visible = holds(r.world, r, |s| s.head);
        shown(r.world, &place(r.world, &visible))
    }

    fn holds(&self, r: &Reading<'_, Folds>) -> BTreeSet<Hash> {
        holds(r.world, r, |s| s.head)
    }

    fn head(&self, _: &World, s: &Folds) -> Hash {
        s.head
    }

    fn tip_writes(&self, _: &World, _: &[File<Folds>], _: &[Seen]) -> BTreeSet<Hash> {
        BTreeSet::new()
    }

    fn dismissals(&self, _: &World, _: &Folds) -> Vec<Dismissal> {
        Vec::new()
    }

    fn bounded(&self, _: &Folds, _: Bound) -> bool {
        true
    }

    fn names_its_head(&self, _: &Folds) -> bool {
        true
    }
}

/// The value an entry's act writes, for display in failures.
pub fn describe(entry: &Entry) -> String {
    let what = match entry.write {
        Some(Write { field, value }) => {
            let value = value.map_or("-".to_string(), |v| format!("{v:?}"));
            format!("{}{:?}={value}", field.entity, field.kind)
        }
        None => "dismiss".into(),
    };
    format!("{}#{}:{what}", ["x", "y", "z"][entry.dir as usize], entry.n)
}
