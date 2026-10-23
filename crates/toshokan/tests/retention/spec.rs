//! Design R's rules, as `RETENTION.md` states them, over what a reader can see:
//! snapshots that hold contributions and certificates, and the lines of the
//! segments it sees. This is the reference the implementation must match; it
//! looks into [`World`] only for the entries of lines it sees or remembers.

use std::collections::{BTreeMap, BTreeSet};

use super::world::{
    change, show, Body, Dir, Dismissal, Entry, Field, Hash, Item, ItemKind, Live, Ref, Shown, Val,
    World, Write,
};
use super::{Compaction, Design, Reading};

/// A reference a contribution keeps: `[own n, own hash, writer, n, hash]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Reference {
    /// The own entry that made it.
    pub own: Ref,
    pub target: Ref,
}

/// A segment's first line, and the last of its lines a snapshot folds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SegmentCert {
    pub first: Ref,
    pub last: Ref,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Certs {
    pub segments: BTreeSet<SegmentCert>,
    pub hashes: BTreeSet<Ref>,
    pub heads: BTreeSet<Ref>,
}

impl Certs {
    /// The entries these certificates name.
    fn named(&self) -> impl Iterator<Item = Ref> + '_ {
        let ends = self.segments.iter().flat_map(|c| [c.first, c.last]);
        ends.chain(self.hashes.iter().copied())
            .chain(self.heads.iter().copied())
    }
}

#[derive(Clone, Debug)]
pub struct Contribution {
    pub dir: Dir,
    pub head: Ref,
    /// The latest own write to each field, live or not.
    pub own: BTreeMap<Field, (Ref, Option<Val>)>,
    /// The number of the first own write to each field.
    pub first: BTreeMap<Field, u32>,
    /// Per field and other writer, the latest target replaced, removed or, by a
    /// delete, observed.
    pub refs: BTreeMap<(Field, Dir), Reference>,
    /// Per own field write it holds, the first own delete above it.
    pub killed: BTreeMap<Field, Ref>,
    pub certs: Certs,
    pub dismissals: BTreeSet<(Ref, Dismissal)>,
}

impl Contribution {
    fn empty(dir: Dir, head: Ref) -> Self {
        Self {
            dir,
            head,
            own: BTreeMap::new(),
            first: BTreeMap::new(),
            refs: BTreeMap::new(),
            killed: BTreeMap::new(),
            certs: Certs::default(),
            dismissals: BTreeSet::new(),
        }
    }

    /// Whether it certifies `at`, by any certificate.
    pub fn certifies(&self, at: Ref) -> bool {
        self.head == at || self.certs.named().any(|c| c == at)
    }
}

/// The writes `e` replaced or, as a delete, observed, each on its own field: a
/// delete is a reference on every field it saw.
fn references<'a>(world: &'a World, e: &'a Entry) -> impl Iterator<Item = (Field, Ref)> + 'a {
    let field = |r: &Ref| world.entry(r.hash).write.map(|w| w.field);
    e.replaces
        .iter()
        .chain(&e.observed)
        .filter_map(move |r| Some((field(r)?, *r)))
}

fn later(slot: &mut Option<Reference>, r: Reference) {
    if slot.is_none_or(|s| (r.target.n, r.own.n) > (s.target.n, s.own.n)) {
        *slot = Some(r);
    }
}

/// Design R, with rules that tests switch off to show what each prevents.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// A forked writer's references apply only when made at or below its fork
    /// point.
    pub branch_references: bool,
    /// Below where several tips part, the union of their own writes is merged;
    /// without it, only what every tip holds.
    pub union: bool,
    /// A writer's latest own write kills its own earlier writes, and a newer tip
    /// replaces the entries a reader placed before it.
    pub supersede: bool,
    /// What a dismissal names is never merged.
    pub quarantine: bool,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            branch_references: true,
            union: true,
            supersede: true,
            quarantine: true,
        }
    }
}

/// What a design-R reader remembers.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub tips: Vec<Contribution>,
    /// The entries it placed after its tips, or shows.
    pub entries: BTreeSet<Hash>,
    pub forks: BTreeSet<Ref>,
    /// After the grace step, the highest number the files show of each writer:
    /// a reference above it kills only its exact target.
    pub limit: BTreeMap<Dir, u32>,
    /// After the grace step, what the files show: a reference to another entry
    /// at a number they show also kills only its exact target.
    pub shown: BTreeSet<Ref>,
}

impl Design for Spec {
    type Snapshot = Contribution;
    type Memory = Memory;

    fn compact(&self, c: &Compaction<'_, Contribution>) -> Contribution {
        let world = c.world;
        let head = world.entry(*c.chain.last().unwrap()).at();
        let start = c
            .previous
            .and_then(|p| c.chain.iter().position(|&h| h == p.head.hash))
            .map_or(0, |i| i + 1);
        let since = &c.chain[start..];
        let mut out = c
            .previous
            .cloned()
            .unwrap_or_else(|| Contribution::empty(c.dir, head));
        if let Some(p) = c.previous {
            out.certs.heads.insert(p.head);
        }
        out.head = head;
        for &h in since {
            let e = world.entry(h);
            out.dismissals
                .extend(e.dismisses.iter().map(|d| (e.at(), *d)));
            let Some(w) = e.write else { continue };
            out.own.insert(w.field, (e.at(), w.value));
            out.killed.remove(&w.field);
            out.first.entry(w.field).or_insert(e.n);
            if w.value == Some(Val::Deleted) {
                for (&field, &(at, _)) in &out.own {
                    if field != Field::Exist && at.n < e.n {
                        out.killed.entry(field).or_insert(e.at());
                    }
                }
            }
            for (field, r) in references(world, e).filter(|(_, r)| r.dir != c.dir) {
                let mut slot = out.refs.get(&(field, r.dir)).copied();
                later(
                    &mut slot,
                    Reference {
                        own: e.at(),
                        target: r,
                    },
                );
                out.refs.insert((field, r.dir), slot.unwrap());
            }
        }
        let chain: BTreeSet<Hash> = c.chain.iter().copied().collect();
        let mut covered = BTreeSet::new();
        for f in c.files.iter().filter(|f| f.dir == c.dir) {
            match &f.body {
                Body::Segment(lines) if !f.deleted || c.recorded.contains(&f.id) => {
                    let on: Vec<Hash> = lines
                        .iter()
                        .copied()
                        .take_while(|h| chain.contains(h))
                        .collect();
                    if let (Some(&first), Some(&last)) = (on.first(), on.last()) {
                        out.certs.segments.insert(SegmentCert {
                            first: world.entry(first).at(),
                            last: world.entry(last).at(),
                        });
                        covered.extend(on);
                    }
                }
                Body::Snapshot(s) if !f.deleted && chain.contains(&s.head.hash) => {
                    out.certs.heads.insert(s.head);
                    out.certs.segments.extend(&s.certs.segments);
                    out.certs.hashes.extend(&s.certs.hashes);
                    out.certs.heads.extend(&s.certs.heads);
                }
                _ => {}
            }
        }
        for &h in since {
            if !covered.contains(&h) {
                out.certs.hashes.insert(world.entry(h).at());
            }
        }
        // A reader matches a dismissal to this chain by the hash of its last entry.
        for (_, d) in &out.dismissals.clone() {
            if chain.contains(&d.last.hash) {
                out.certs.hashes.insert(d.last);
            }
        }
        out
    }

    fn read(&self, r: &Reading<'_, Contribution>, memory: &Memory) -> (Shown, Memory) {
        derive(self, r, memory)
    }

    fn grace(&self, r: &Reading<'_, Contribution>, memory: &Memory) -> (Shown, Memory) {
        let world = r.world;
        let mut held: Vec<Ref> = Vec::new();
        for seen in r.seen {
            match &r.files[seen.file].body {
                Body::Segment(lines) => {
                    held.extend(lines.iter().take(seen.lines).map(|&h| world.entry(h).at()))
                }
                Body::Snapshot(s) => held.push(s.head),
            }
        }
        // The guard: something visible descends from what the reader remembers,
        // a line by its chain of predecessors or a snapshot by its certificates.
        let snapshots: Vec<&Contribution> = r
            .seen
            .iter()
            .filter_map(|s| match &r.files[s.file].body {
                Body::Snapshot(c) => Some(c),
                Body::Segment(_) => None,
            })
            .collect();
        let known: BTreeSet<Hash> = memory
            .entries
            .iter()
            .copied()
            .chain(memory.tips.iter().map(|t| t.head.hash))
            .collect();
        let mut passed: BTreeSet<Hash> = BTreeSet::new();
        for line in held
            .iter()
            .filter(|h| !snapshots.iter().any(|s| s.head == **h))
        {
            let mut at = world.entry(line.hash).prev;
            while let Some(p) = at.filter(|p| known.contains(p) && passed.insert(*p)) {
                at = world.entry(p).prev;
            }
        }
        let newer = |at: Ref| {
            passed.contains(&at.hash) || snapshots.iter().any(|s| s.head != at && s.certifies(at))
        };
        let visible_snapshot = |t: &Contribution| snapshots.iter().any(|c| c.head == t.head);
        let kept = Memory {
            tips: memory
                .tips
                .iter()
                .filter(|t| visible_snapshot(t) || newer(t.head))
                .cloned()
                .collect(),
            entries: memory
                .entries
                .iter()
                .copied()
                .filter(|&e| {
                    let at = world.entry(e).at();
                    held.contains(&at) || newer(at)
                })
                .collect(),
            forks: BTreeSet::new(),
            shown: held.iter().copied().collect(),
            limit: {
                let mut limit: BTreeMap<Dir, u32> = BTreeMap::new();
                for h in &held {
                    let l = limit.entry(h.dir).or_insert(h.n);
                    *l = (*l).max(h.n);
                }
                limit
            },
        };
        derive(self, r, &kept)
    }

    fn holds(&self, r: &Reading<'_, Contribution>) -> BTreeSet<Hash> {
        super::oracle::holds(r.world, r, |s| s.head.hash)
    }
}

/// How a reader classifies one directory's lines against its tips.
struct Dirs<'a> {
    world: &'a World,
    tips: BTreeMap<Dir, Vec<Contribution>>,
    folded: BTreeSet<Hash>,
    waiting: BTreeSet<Hash>,
    certified: BTreeMap<Dir, BTreeSet<Ref>>,
}

impl Dirs<'_> {
    fn tip_n(&self, dir: Dir) -> Option<u32> {
        self.tips.get(&dir)?.iter().map(|t| t.head.n).max()
    }

    fn is_head(&self, h: Hash) -> bool {
        self.tips.values().flatten().any(|t| t.head.hash == h)
    }

    fn certified(&self, at: Ref) -> bool {
        self.certified.get(&at.dir).is_some_and(|c| c.contains(&at))
            || self.folded.contains(&at.hash)
    }
}

fn derive(spec: &Spec, r: &Reading<'_, Contribution>, memory: &Memory) -> (Shown, Memory) {
    let world = r.world;
    // Tips: snapshots no other of their directory certifies; one per head.
    let mut snapshots: Vec<Contribution> = memory.tips.clone();
    let mut segments: Vec<Vec<Hash>> = Vec::new();
    for seen in r.seen {
        match &r.files[seen.file].body {
            Body::Snapshot(s) => snapshots.push(s.clone()),
            Body::Segment(lines) => segments.push(lines.iter().copied().take(seen.lines).collect()),
        }
    }
    let mut tips: BTreeMap<Dir, Vec<Contribution>> = BTreeMap::new();
    for s in &snapshots {
        let superseded = snapshots
            .iter()
            .any(|o| o.dir == s.dir && o.head != s.head && o.certifies(s.head));
        let list = tips.entry(s.dir).or_default();
        if !superseded && !list.iter().any(|t| t.head == s.head) {
            list.push(s.clone());
        }
    }
    let mut dirs = Dirs {
        world,
        tips,
        folded: BTreeSet::new(),
        waiting: BTreeSet::new(),
        certified: BTreeMap::new(),
    };
    for (dir, list) in &dirs.tips {
        let named = dirs.certified.entry(*dir).or_default();
        for t in list {
            named.insert(t.head);
            named.extend(t.certs.named());
        }
    }
    // Lines a tip certifies, by hash, by head, or by matching a segment.
    for lines in &segments {
        let Some(&first) = lines.first() else {
            continue;
        };
        let dir = world.entry(first).dir;
        let certs: Vec<SegmentCert> = dirs
            .tips
            .get(&dir)
            .into_iter()
            .flatten()
            .flat_map(|t| t.certs.segments.iter().copied())
            .filter(|c| c.first.hash == first)
            .collect();
        let matched = certs
            .iter()
            .filter_map(|c| {
                let pos = (c.last.n - c.first.n) as usize;
                (lines.len() > pos && lines[pos] == c.last.hash).then_some(pos)
            })
            .max();
        match matched {
            Some(pos) => dirs.folded.extend(&lines[..=pos]),
            None if certs
                .iter()
                .any(|c| lines.len() <= (c.last.n - c.first.n) as usize) =>
            {
                dirs.waiting.extend(lines);
            }
            None if !certs.is_empty() => {
                dirs.folded.insert(first);
            }
            None => {}
        }
        for &h in lines {
            if dirs.certified(world.entry(h).at()) {
                dirs.folded.insert(h);
            }
        }
    }
    let seen_lines: BTreeSet<Hash> = segments.iter().flatten().copied().collect();
    // Candidates: lines neither folded nor waiting, and remembered entries no
    // tip has since passed.
    let mut candidates: BTreeSet<Hash> = seen_lines
        .iter()
        .copied()
        .filter(|h| !dirs.folded.contains(h) && !dirs.waiting.contains(h))
        .collect();
    for &e in &memory.entries {
        let at = world.entry(e).at();
        if seen_lines.contains(&e) || dirs.certified(at) {
            continue;
        }
        candidates.insert(e);
    }
    // Placement from the tips' heads.
    let mut placed = BTreeSet::new();
    loop {
        let before = placed.len();
        for &e in &candidates {
            let entry = world.entry(e);
            let ok = match entry.prev {
                None => dirs.tip_n(entry.dir).is_none(),
                Some(p) => dirs.is_head(p) || placed.contains(&p),
            };
            if ok {
                placed.insert(e);
            }
        }
        if placed.len() == before {
            break;
        }
    }
    // Remembered entries a tip has since passed are folded into it.
    let remembered_folded: BTreeSet<Hash> = candidates
        .iter()
        .copied()
        .filter(|&e| {
            let entry = world.entry(e);
            spec.supersede
                && !seen_lines.contains(&e)
                && !placed.contains(&e)
                && dirs.tip_n(entry.dir).is_some_and(|n| entry.n <= n)
        })
        .collect();
    candidates.retain(|e| !remembered_folded.contains(e));
    // Fork points.
    let mut forks: BTreeSet<Ref> = memory.forks.clone();
    let mut by_prev: BTreeMap<Hash, BTreeSet<Hash>> = BTreeMap::new();
    for &e in &candidates {
        if let Some(p) = world.entry(e).prev {
            by_prev.entry(p).or_default().insert(e);
        }
    }
    for (&p, next) in &by_prev {
        let e = world.entry(*next.iter().next().unwrap());
        let at = Ref {
            dir: e.dir,
            n: e.n - 1,
            hash: p,
        };
        if next.len() > 1 || (dirs.certified(at) && !dirs.is_head(p)) {
            forks.insert(at);
        }
    }
    // Tip certificates are complete, so a line no tip folds, at or below a tip's
    // head, is off that tip's chain: its writer forked at its predecessor.
    for &e in candidates.difference(&placed) {
        let entry = world.entry(e);
        if let Some(hash) = entry
            .prev
            .filter(|_| dirs.tip_n(entry.dir).is_some_and(|n| entry.n <= n))
        {
            forks.insert(Ref {
                dir: entry.dir,
                n: entry.n - 1,
                hash,
            });
        }
    }
    for (dir, list) in &dirs.tips {
        if list.len() > 1 {
            let mut common: Option<BTreeSet<Ref>> = None;
            for t in list {
                let own = t.own.values().map(|(at, _)| *at);
                let made = t
                    .refs
                    .values()
                    .map(|r| r.own)
                    .chain(t.killed.values().copied());
                let dismissed = t.dismissals.iter().map(|(own, _)| *own);
                let named: BTreeSet<Ref> = t
                    .certs
                    .named()
                    .chain([t.head])
                    .chain(own)
                    .chain(made)
                    .chain(dismissed)
                    .collect();
                common = Some(match common {
                    None => named,
                    Some(c) => c.intersection(&named).copied().collect(),
                });
            }
            let parting = common.unwrap_or_default().into_iter().max_by_key(|r| r.n);
            forks.insert(parting.unwrap_or(Ref {
                dir: *dir,
                n: 0,
                hash: Hash::MAX,
            }));
        }
    }
    let lowest: BTreeMap<Dir, u32> = {
        let mut lowest = BTreeMap::new();
        for f in &forks {
            let n: &mut u32 = lowest.entry(f.dir).or_insert(f.n);
            *n = (*n).min(f.n);
        }
        lowest
    };
    let beyond = |dir: Dir, n: u32| lowest.get(&dir).is_some_and(|f| n > *f);
    let limit_of = |dir: Dir| lowest.get(&dir).copied();
    // Dismissals from what is merged so far.
    let mut dismissals: Vec<Dismissal> = Vec::new();
    for (dir, list) in &dirs.tips {
        for t in list {
            for &(own, d) in &t.dismissals {
                if limit_of(*dir).is_none_or(|f| own.n <= f) {
                    dismissals.push(d);
                }
            }
        }
    }
    for &e in &placed {
        let entry = world.entry(e);
        if !beyond(entry.dir, entry.n) {
            dismissals.extend(&entry.dismisses);
        }
    }
    let known: BTreeSet<Hash> = candidates.union(&seen_lines).copied().collect();
    let walk = |from: Hash, to: Hash| {
        let mut at = Some(from);
        while let Some(h) = at {
            if h == to {
                return true;
            }
            at = world
                .entry(h)
                .prev
                .filter(|p| known.contains(p) || *p == to);
        }
        false
    };
    let is_covered = |e: Hash| {
        let entry = world.entry(e);
        dismissals.iter().any(|d| {
            d.dir == entry.dir
                && entry.n >= d.first.n
                && entry.n <= d.last.n
                && (walk(e, d.first.hash) || walk(d.last.hash, e))
        })
    };
    let covered: BTreeSet<Hash> = candidates
        .iter()
        .copied()
        .filter(|&e| is_covered(e))
        .collect();
    let after: BTreeSet<Hash> = candidates
        .iter()
        .copied()
        .filter(|&e| {
            if covered.contains(&e) {
                return false;
            }
            let mut at = world.entry(e).prev;
            while let Some(p) = at.filter(|p| known.contains(p)) {
                if covered.contains(&p) {
                    return true;
                }
                at = world.entry(p).prev;
            }
            false
        })
        .collect();
    // Classification of what is not folded.
    let held: BTreeSet<Hash> = candidates.difference(&placed).copied().collect();
    let mut runs: BTreeSet<Hash> = candidates
        .iter()
        .copied()
        .filter(|&e| {
            let entry = world.entry(e);
            !covered.contains(&e) && (beyond(entry.dir, entry.n) || after.contains(&e))
        })
        .collect();
    for &e in &held {
        let entry = world.entry(e);
        if covered.contains(&e) || runs.contains(&e) {
            continue;
        }
        if dirs.tip_n(entry.dir).is_some_and(|n| entry.n <= n + 1) {
            runs.insert(e);
        }
    }
    loop {
        let before = runs.len();
        for &e in &held {
            if world.entry(e).prev.is_some_and(|p| runs.contains(&p)) && !covered.contains(&e) {
                runs.insert(e);
            }
        }
        if runs.len() == before {
            break;
        }
    }
    let gaps: BTreeSet<Hash> = held
        .iter()
        .copied()
        .filter(|e| !runs.contains(e) && !covered.contains(e))
        .collect();
    // Merged writes, and what kills them.
    let tip_covered = |t: &Contribution, at: Ref| {
        dismissals
            .iter()
            .any(|d| d.dir == t.dir && at.n >= d.first.n && at.n <= d.last.n && t.certifies(d.last))
    };
    let mut writes: BTreeMap<Hash, Live> = BTreeMap::new();
    // After the grace step, a reference kills only its exact hash when it or
    // another reference of its entry to the same writer names a number the
    // files no longer show, or one at which they show only another entry.
    let contradicted = |r: &Ref| {
        let other = memory
            .shown
            .iter()
            .any(|s| s.dir == r.dir && s.n == r.n && s.hash != r.hash);
        memory.limit.get(&r.dir).is_some_and(|&l| r.n > l) || (other && !memory.shown.contains(r))
    };
    let mut refs: Vec<(Field, Ref, bool)> = Vec::new();
    let mut own_deletes: Vec<(Dir, u32)> = Vec::new();
    for (dir, list) in &dirs.tips {
        let limit = limit_of(*dir);
        let ok = |n: u32| limit.is_none_or(|f| n <= f);
        let referenced = |n: u32| !spec.branch_references || ok(n);
        let held_by_all =
            |at: Ref| spec.union || list.iter().all(|o| o.own.values().any(|(a, _)| *a == at));
        for t in list {
            for (&field, &(at, value)) in &t.own {
                if ok(at.n) && !tip_covered(t, at) && held_by_all(at) {
                    writes.insert(
                        at.hash,
                        Live {
                            at,
                            write: Write { field, value },
                        },
                    );
                }
            }
            let dismissed = |n: u32| {
                tip_covered(
                    t,
                    Ref {
                        dir: *dir,
                        n,
                        hash: Hash::MAX,
                    },
                )
            };
            for (&(field, _), r) in &t.refs {
                if referenced(r.own.n) && !dismissed(r.own.n) {
                    refs.push((field, r.target, contradicted(&r.target)));
                }
            }
            for k in t.killed.values() {
                if ok(k.n) && !dismissed(k.n) {
                    own_deletes.push((*dir, k.n));
                }
            }
        }
    }
    let merged_entries: BTreeSet<Hash> = placed
        .iter()
        .copied()
        .filter(|e| !runs.contains(e) && (!spec.quarantine || !covered.contains(e)))
        .collect();
    for &e in &merged_entries {
        let entry = world.entry(e);
        let Some(write) = entry.write else { continue };
        writes.insert(
            e,
            Live {
                at: entry.at(),
                write,
            },
        );
        let exact: BTreeSet<Dir> = references(world, entry)
            .filter(|(_, r)| contradicted(r))
            .map(|(_, r)| r.dir)
            .collect();
        refs.extend(references(world, entry).map(|(field, r)| (field, r, exact.contains(&r.dir))));
        if write.value == Some(Val::Deleted) {
            own_deletes.push((entry.dir, entry.n));
        }
    }
    // A reference to a write beyond its writer's fork point was made on one
    // branch: the earlier writes it passed may have died only on that branch.
    let kills = |target: Ref, exact: bool, w: &Live| {
        let exact = exact || beyond(target.dir, target.n);
        w.at.dir == target.dir && (w.at.hash == target.hash || (!exact && w.at.n < target.n))
    };
    let live: Vec<Live> = writes
        .values()
        .filter(|w| {
            let own_later = spec.supersede
                && writes.values().any(|o| {
                    o.at.dir == w.at.dir && o.write.field == w.write.field && o.at.n > w.at.n
                });
            let own_deleted = w.write.field != Field::Exist
                && own_deletes
                    .iter()
                    .any(|&(d, n)| d == w.at.dir && n > w.at.n);
            let referenced = refs
                .iter()
                .any(|&(f, t, exact)| f == w.write.field && kills(t, exact, w));
            !(own_later || own_deleted || referenced)
        })
        .copied()
        .collect();
    let base = show(&live, BTreeSet::new());
    // Items: runs of entries, joined to the tip they follow, and gaps.
    let mut items = BTreeSet::new();
    let root = |e: Hash, set: &BTreeSet<Hash>| {
        let mut root = e;
        while let Some(p) = world.entry(root).prev.filter(|p| set.contains(p)) {
            root = p;
        }
        root
    };
    let mut groups: BTreeMap<(ItemKind, Hash), Vec<Write>> = BTreeMap::new();
    // A tip's own write the merge leaves out, above the fork point or
    // dismissed, that replaced an own write the merge does not hold: a
    // forgotten basis, so a run shows the field whatever the merged state shows,
    // and a dismissal does not hide it.
    let mut forgotten: BTreeMap<Hash, BTreeMap<Field, Option<Val>>> = BTreeMap::new();
    for (dir, list) in &dirs.tips {
        let beyond_f = |n: u32| limit_of(*dir).is_some_and(|f| n > f);
        // Where several tips part inside a segment, the fork point may lie as
        // high as just below the lowest head.
        let highest_fork = limit_of(*dir).map(|f| match list.iter().map(|t| t.head.n).min() {
            Some(low) if list.len() > 1 => f.max(low.saturating_sub(1)),
            _ => f,
        });
        let known_before = |field: &Field, n: u32| {
            writes
                .values()
                .any(|w| w.at.dir == *dir && w.write.field == *field && w.at.n < n)
        };
        for t in list {
            let forced: BTreeMap<Field, Option<Val>> = t
                .own
                .iter()
                .filter(|(field, (at, _))| {
                    let first = t.first[*field];
                    let pre_fork = highest_fork.is_none_or(|f| first <= f);
                    let replaced_own = first < at.n && (!beyond_f(at.n) || pre_fork);
                    (beyond_f(at.n) || tip_covered(t, *at))
                        && replaced_own
                        && !known_before(field, at.n)
                })
                .map(|(&field, &(_, value))| (field, value))
                .collect();
            if !beyond_f(t.head.n) && forced.is_empty() {
                continue;
            }
            let mut own: Vec<(u32, Write)> = t
                .own
                .iter()
                .filter(|(_, (at, _))| beyond_f(at.n) && !tip_covered(t, *at))
                .map(|(&field, &(at, value))| (at.n, Write { field, value }))
                .collect();
            own.sort();
            forgotten.insert(t.head.hash, forced);
            groups
                .entry((ItemKind::Run, t.head.hash))
                .or_default()
                .extend(own.into_iter().map(|(_, w)| w));
        }
    }
    for (set, kind) in [(&runs, ItemKind::Run), (&gaps, ItemKind::Gap)] {
        let mut ordered: Vec<Hash> = set.iter().copied().collect();
        ordered.sort_by_key(|&e| world.entry(e).n);
        for e in ordered {
            let start = root(e, set);
            let key = match world.entry(start).prev {
                Some(p)
                    if kind == ItemKind::Run
                        && dirs.is_head(p)
                        && groups.contains_key(&(ItemKind::Run, p)) =>
                {
                    (ItemKind::Run, p)
                }
                _ => (kind, start),
            };
            if let Some(w) = world.entry(e).write {
                groups.entry(key).or_default().push(w);
            } else {
                groups.entry(key).or_default();
            }
        }
    }
    for ((kind, key), writes) in groups {
        let dir = if kind == ItemKind::Run && dirs.is_head(key) {
            dirs.tips
                .values()
                .flatten()
                .find(|t| t.head.hash == key)
                .unwrap()
                .dir
        } else {
            world.entry(key).dir
        };
        let mut change = change(&writes, &base.values, base.exists);
        let forced = forgotten.remove(&key).unwrap_or_default();
        change.extend(&forced);
        if !change.is_empty() {
            items.insert(Item {
                dir,
                kind,
                change,
                forgotten: forced.into_keys().collect(),
            });
        }
    }
    let shown = Shown { items, ..base };
    let memory = Memory {
        tips: dirs.tips.values().flatten().cloned().collect(),
        entries: placed.union(&runs).chain(&gaps).copied().collect(),
        forks,
        limit: memory.limit.clone(),
        shown: memory.shown.clone(),
    };
    (shown, memory)
}
