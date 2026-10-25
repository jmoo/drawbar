//! What a reader makes of the files it sees: each writer's tips, the entries it
//! places after them, the entries it holds back or shows apart as runs, gaps and
//! unplaced lines, its fork points, and the merged state.
//!
//! A pure function of the files, so that readers seeing the same files show the
//! same, whatever order the files came in.
//!
//! - **Tips** are the snapshots of a writer no other of its snapshots certifies.
//! - A line a tip certifies by name, or one a segment certificate matches, is
//!   **folded**. An unsealed file shorter than a certificate is a dropped version
//!   or a diverged copy: its lines past the first are **unplaced**. A sealed
//!   one, or one that differs at the certified position, diverged: only its
//!   first line is folded.
//! - An entry whose predecessor is a tip's head or placed, numbered one past it,
//!   is **placed**; a genesis entry is placed when the writer has no tip.
//! - A **fork point** is an entry with two successors, a certified predecessor
//!   of an entry no tip folds, the predecessor of a line no tip folds at or below
//!   a tip's head, or where several tips part. Every entry of a writer numbered
//!   above its lowest fork point is a **run**, and so is every entry after a
//!   dismissed one. A held entry that may still follow a tip is a **gap**.
//! - The merge kills, per field: a writer's own earlier writes, by its later own
//!   write, and its own writes in or before its own delete of their entity;
//!   writes a reference covers; and a taken run's writer's writes below the run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use crate::contribution::{self, Contribution, FieldKey, Held, Said, Value};
use crate::ids::{EntityId, EntryHash, Hlc, WriterId};
use crate::log::{Entry, Op, Own, Ref, Run};
use crate::merge::{Beside, Folded, Live};
use crate::report::ItemKind;
use crate::snapshot::Snapshot;

/// What a reader sees of one writer's directory.
#[derive(Clone, Default)]
pub struct Seen<'a> {
    pub segments: Vec<SeenSegment<'a>>,
    pub snapshots: Vec<Rc<Snapshot>>,
}

/// The readable lines of one segment file, and whether the reader sees the seal
/// marker after the last of them.
#[derive(Clone)]
pub struct SeenSegment<'a> {
    pub entries: &'a [Rc<Entry>],
    pub sealed: bool,
}

/// One item: a run, a gap or unplaced lines, with the net change it would make,
/// which is empty for a run that would change nothing.
#[derive(Clone, PartialEq, Debug)]
pub struct Item {
    pub kind: ItemKind,
    /// Which run: what a dismissal or a take names.
    pub run: Run,
    /// As [`crate::report::Item::span`] and [`crate::report::Item::entries`].
    pub span: (Hlc, Hlc),
    pub entries: u64,
    /// Per field, the run's latest write, where the merged state shows
    /// something else, or where its basis is forgotten.
    pub change: BTreeMap<FieldKey, (Ref, Hlc, Value)>,
    /// Fields in `change` whatever the merged state shows: the run replaced an
    /// own write the reader no longer holds.
    pub forgotten: BTreeSet<FieldKey>,
}

/// How one writer's entries are placed.
#[derive(Clone, Debug, Default)]
pub struct Placed {
    pub tips: Vec<Rc<Snapshot>>,
    /// Merged, each after its predecessor.
    pub merged: Vec<Rc<Entry>>,
    /// Placed, but beyond a fork point or after a dismissed entry.
    pub runs: Vec<Rc<Entry>>,
    pub gaps: Vec<Rc<Entry>>,
    pub unplaced: Vec<Rc<Entry>>,
    /// Entries named by a dismissal, and never merged.
    pub dismissed: Vec<Rc<Entry>>,
    pub forks: BTreeSet<Own>,
    /// The lowest fork point, by number.
    pub fork: Option<u64>,
    /// Entries whose number does not follow their predecessor's.
    pub misnumbered: Vec<EntryHash>,
}

impl Placed {
    /// The last entry of the writer's history the reader merges: a placed entry
    /// with no placed successor, else a tip's head. `None` when the history
    /// forked or holds nothing merged.
    pub fn head(&self) -> Option<Own> {
        if let Some(last) = self.merged.last() {
            return Some(last.own());
        }
        match self.tips.as_slice() {
            [tip] => Some(tip.head),
            _ => None,
        }
    }

    /// Whether the writer's history reaches `hash` as far as the reader sees: a
    /// tip names it, or it is merged.
    pub fn reaches(&self, hash: EntryHash) -> bool {
        let named = self
            .tips
            .iter()
            .any(|tip| tip.head.hash == hash || tip.certified.named().any(|own| own.hash == hash));
        named || self.merged.iter().any(|entry| entry.hash() == hash)
    }

    /// Whether the reader holds `hash` of this writer: a tip names it, or it is
    /// merged, a run or a gap.
    pub fn holds(&self, hash: EntryHash) -> bool {
        let named = self
            .tips
            .iter()
            .any(|tip| tip.head.hash == hash || tip.certified.named().any(|own| own.hash == hash));
        let entries = self.merged.iter().chain(&self.runs).chain(&self.gaps);
        named || entries.into_iter().any(|entry| entry.hash() == hash)
    }
}

/// Everything a read derives.
#[derive(Clone, Default)]
pub struct Derived {
    pub writers: BTreeMap<WriterId, Placed>,
    pub folded: Folded,
    /// By writer.
    pub items: BTreeMap<WriterId, Vec<Item>>,
    /// Fields where a reference the files contradict killed only its exact
    /// target.
    pub contradicted: BTreeSet<FieldKey>,
}

/// One writer's lines and tips, classified.
struct Writer {
    writer: WriterId,
    tips: Vec<Rc<Snapshot>>,
    /// Every entry seen, by hash.
    known: HashMap<EntryHash, Rc<Entry>>,
    /// Hashes of the lines of files seen.
    seen: BTreeSet<EntryHash>,
    folded: BTreeSet<EntryHash>,
    ambiguous: BTreeSet<EntryHash>,
    named: BTreeSet<Own>,
    candidates: BTreeSet<EntryHash>,
    placed: BTreeSet<EntryHash>,
    misnumbered: BTreeSet<EntryHash>,
    forks: BTreeSet<Own>,
    lowest: Option<u64>,
}

impl Writer {
    fn tip_n(&self) -> Option<u64> {
        self.tips.iter().map(|tip| tip.head.n).max()
    }

    fn is_head(&self, hash: EntryHash) -> bool {
        self.tips.iter().any(|tip| tip.head.hash == hash)
    }

    fn certified(&self, own: Own) -> bool {
        self.named.contains(&own) || self.folded.contains(&own.hash)
    }

    fn entry(&self, hash: EntryHash) -> &Rc<Entry> {
        &self.known[&hash]
    }

    fn beyond(&self, n: u64) -> bool {
        self.lowest.is_some_and(|fork| n > fork)
    }
}

/// The tips among `snapshots`: those no other certifies, one per head.
fn tips(snapshots: Vec<Rc<Snapshot>>) -> Vec<Rc<Snapshot>> {
    let mut tips: Vec<Rc<Snapshot>> = Vec::new();
    for snapshot in &snapshots {
        let superseded = snapshots
            .iter()
            .any(|other| other.head != snapshot.head && other.names(snapshot.head));
        if !superseded && !tips.iter().any(|tip| tip.head == snapshot.head) {
            tips.push(Rc::clone(snapshot));
        }
    }
    tips.sort_by_key(|tip| tip.head);
    tips
}

/// Every own entry `tip` names by hash: in its certificates, as its head, as an
/// own write, as the own entry of a reference, as an own delete, or as a
/// dismissal. An entry with an effect left on a tip's chain is among them.
fn mentioned(tip: &Snapshot) -> impl Iterator<Item = Own> + '_ {
    let state = &tip.state;
    let own = state.own.values().map(|held| held.at);
    let made = state.refs.values().map(|reference| reference.own);
    let killed = state.killed.values().copied();
    let dismissed = state.dismissals.iter().map(|(own, _)| *own);
    tip.certified
        .named()
        .chain([tip.head])
        .chain(own)
        .chain(made)
        .chain(killed)
        .chain(dismissed)
}

/// Places one writer's entries from its tips' heads, and finds its fork points.
fn classify(writer: WriterId, seen: &Seen<'_>) -> Writer {
    let tips = tips(seen.snapshots.clone());
    let mut named = BTreeSet::new();
    for tip in &tips {
        named.insert(tip.head);
        named.extend(tip.certified.named());
    }
    let mut w = Writer {
        writer,
        tips,
        known: HashMap::default(),
        seen: BTreeSet::new(),
        folded: BTreeSet::new(),
        ambiguous: BTreeSet::new(),
        named,
        candidates: BTreeSet::new(),
        placed: BTreeSet::new(),
        misnumbered: BTreeSet::new(),
        forks: BTreeSet::new(),
        lowest: None,
    };
    for segment in &seen.segments {
        let lines = &segment.entries;
        let Some(first) = lines.first() else {
            continue;
        };
        for entry in lines.iter() {
            w.known.insert(entry.hash(), Rc::clone(entry));
            w.seen.insert(entry.hash());
        }
        let certs: Vec<_> = w
            .tips
            .iter()
            .flat_map(|tip| tip.certified.segments.iter().copied())
            .filter(|cert| cert.first.hash == first.hash())
            .collect();
        let matched = certs
            .iter()
            .filter_map(|cert| {
                let pos = usize::try_from(cert.last.n.checked_sub(cert.first.n)?).ok()?;
                (lines.len() > pos && lines[pos].hash() == cert.last.hash).then_some(pos)
            })
            .max();
        let shorter = certs.iter().any(|cert| {
            usize::try_from(cert.last.n.saturating_sub(cert.first.n))
                .is_ok_and(|pos| lines.len() <= pos)
        });
        match matched {
            Some(pos) => w
                .folded
                .extend(lines[..=pos].iter().map(|entry| entry.hash())),
            // A sealed file shorter than a certificate diverged from it. An
            // unsealed one may be a version a sync client dropped, one still
            // arriving, or a copy that diverged before its end: nothing tells
            // them apart.
            None if !segment.sealed && shorter => {
                w.folded.insert(first.hash());
                w.ambiguous
                    .extend(lines[1..].iter().map(|entry| entry.hash()));
            }
            None if !certs.is_empty() => {
                w.folded.insert(first.hash());
            }
            None => {}
        }
        for entry in lines.iter() {
            if w.certified(entry.own()) {
                w.folded.insert(entry.hash());
            }
        }
    }
    let candidates: BTreeSet<EntryHash> = w
        .seen
        .iter()
        .copied()
        .filter(|hash| !w.folded.contains(hash) && !w.ambiguous.contains(hash))
        .collect();
    let heads: BTreeMap<EntryHash, u64> = w
        .tips
        .iter()
        .map(|tip| (tip.head.hash, tip.head.n))
        .collect();
    let mut by_prev: BTreeMap<EntryHash, Vec<EntryHash>> = BTreeMap::new();
    for hash in &candidates {
        by_prev
            .entry(w.entry(*hash).prev())
            .or_default()
            .push(*hash);
    }
    let mut frontier: Vec<(EntryHash, u64)> = heads.iter().map(|(hash, n)| (*hash, *n)).collect();
    if w.tips.is_empty() {
        for hash in by_prev.get(&EntryHash::ZERO).into_iter().flatten() {
            match w.entry(*hash).n() {
                0 => {
                    w.placed.insert(*hash);
                    frontier.push((*hash, 0));
                }
                _ => {
                    w.misnumbered.insert(*hash);
                }
            }
        }
    }
    while let Some((prev, n)) = frontier.pop() {
        for hash in by_prev.get(&prev).into_iter().flatten() {
            if prev == EntryHash::ZERO || w.placed.contains(hash) {
                continue;
            }
            let next = w.entry(*hash).n();
            if Some(next) != n.checked_add(1) {
                w.misnumbered.insert(*hash);
                continue;
            }
            w.placed.insert(*hash);
            frontier.push((*hash, next));
        }
    }
    let tip_n = w.tip_n();
    w.candidates = candidates;
    let mut successors: BTreeMap<EntryHash, Vec<EntryHash>> = BTreeMap::new();
    for hash in &w.candidates {
        successors
            .entry(w.entry(*hash).prev())
            .or_default()
            .push(*hash);
    }
    let mut forks = BTreeSet::new();
    for (prev, next) in &successors {
        if *prev == EntryHash::ZERO {
            continue;
        }
        let Some(n) = w.entry(next[0]).n().checked_sub(1) else {
            continue;
        };
        let at = Own { n, hash: *prev };
        if next.len() > 1 || (w.certified(at) && !w.is_head(*prev)) {
            forks.insert(at);
        }
    }
    // Tip certificates are complete, so a line no tip folds, at or below a tip's
    // head, is off that tip's chain: its writer forked at its predecessor.
    for hash in w.candidates.difference(&w.placed) {
        let entry = w.entry(*hash);
        if entry.prev() == EntryHash::ZERO || entry.n() == Entry::UNNUMBERED {
            continue;
        }
        if tip_n.is_some_and(|tip| entry.n() <= tip) {
            if let Some(n) = entry.n().checked_sub(1) {
                forks.insert(Own {
                    n,
                    hash: entry.prev(),
                });
            }
        }
    }
    if w.tips.len() > 1 {
        let mut common: Option<BTreeSet<Own>> = None;
        for tip in &w.tips {
            let named: BTreeSet<Own> = mentioned(tip).collect();
            common = Some(match common {
                None => named,
                Some(common) => common.intersection(&named).copied().collect(),
            });
        }
        let parting = common
            .unwrap_or_default()
            .into_iter()
            .max_by_key(|own| own.n);
        forks.insert(parting.unwrap_or(Own {
            n: 0,
            hash: EntryHash::ZERO,
        }));
    }
    w.lowest = forks.iter().map(|own| own.n).min();
    w.forks = forks;
    w
}

/// The hashes of a writer's lines and snapshot heads, by number.
type Numbered = BTreeMap<u64, BTreeSet<EntryHash>>;

/// The runs a dismissal names: from its first entry to its last, on its last
/// entry's chain, of one writer.
struct Dismissals(Vec<Run>);

impl Dismissals {
    /// The candidates of `w` some dismissal names: numbered from the run's first
    /// to its last, on the last's chain or, for a run of lines, after its first.
    fn covered(&self, w: &Writer) -> BTreeSet<EntryHash> {
        let mut covered = BTreeSet::new();
        for run in self.0.iter().filter(|run| run.writer == w.writer) {
            let named = |hash: &EntryHash| (run.from..=run.last.n).contains(&w.entry(*hash).n());
            let mut at = Some(run.last.hash).filter(|hash| w.known.contains_key(hash));
            while let Some(hash) = at {
                step();
                if w.candidates.contains(&hash) && named(&hash) {
                    covered.insert(hash);
                }
                let entry = w.entry(hash);
                at = Some(entry.prev())
                    .filter(|prev| entry.n() > run.from && w.known.contains_key(prev));
            }
            let Some(first) = run.first.filter(|_| run.tip.is_none()) else {
                continue;
            };
            let mut memo = HashMap::default();
            let through = |hash: EntryHash| w.known.contains_key(&hash);
            for hash in w.candidates.iter().filter(|hash| named(hash)) {
                if reaches(w, *hash, &mut memo, &through, &|at| at == first) {
                    covered.insert(*hash);
                }
            }
        }
        covered
    }

    /// Whether some dismissal names the own entry numbered `n` in tip `tip`.
    fn cover_tip(&self, writer: WriterId, tip: &Snapshot, n: u64) -> bool {
        self.in_tip(writer, tip)
            .any(|run| (run.from..=run.last.n).contains(&n))
    }

    /// The dismissals of `writer`'s runs on the chain of tip `tip`.
    fn in_tip<'d>(&'d self, writer: WriterId, tip: &'d Snapshot) -> impl Iterator<Item = &'d Run> {
        self.0.iter().filter(move |run| {
            let names = tip.names(run.last) || run.tip.is_some_and(|head| tip.names(head));
            run.writer == writer && names
        })
    }

    /// The first number from `from` on that no dismissal names in tip `tip`;
    /// `None` when they name every one.
    fn after_in_tip(&self, writer: WriterId, tip: &Snapshot, from: u64) -> Option<u64> {
        let mut first = from;
        while let Some(run) = self
            .in_tip(writer, tip)
            .find(|run| (run.from..=run.last.n).contains(&first))
        {
            first = run.last.n.checked_add(1)?;
        }
        Some(first)
    }
}

/// Whether the chain back from `start`, through entries `through` holds for,
/// reaches one `hit` holds for. `memo` keeps the answer for each entry walked,
/// so that walks from every entry of a chain take one step per entry in all.
fn reaches(
    w: &Writer,
    start: EntryHash,
    memo: &mut HashMap<EntryHash, bool>,
    through: &dyn Fn(EntryHash) -> bool,
    hit: &dyn Fn(EntryHash) -> bool,
) -> bool {
    let mut path = Vec::new();
    let mut at = start;
    let reached = loop {
        if let Some(&known) = memo.get(&at) {
            break known;
        }
        if hit(at) {
            break true;
        }
        if !through(at) {
            break false;
        }
        step();
        path.push(at);
        at = w.entry(at).prev();
    };
    for hash in path {
        memo.insert(hash, reached);
    }
    reached
}

#[cfg(test)]
thread_local! {
    /// How many steps the walks back along chains took, for tests that bound it.
    static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The first entry of `members` on the chain back from `entry` through them,
/// remembered for each entry the walk passes.
fn root_in(
    w: &Writer,
    entry: &Rc<Entry>,
    members: &HashSet<EntryHash>,
    roots: &mut HashMap<EntryHash, Rc<Entry>>,
) -> Rc<Entry> {
    let mut passed = Vec::new();
    let mut at = Rc::clone(entry);
    let root = loop {
        step();
        if let Some(root) = roots.get(&at.hash()) {
            break Rc::clone(root);
        }
        passed.push(at.hash());
        let prev = w.known.get(&at.prev());
        match prev.filter(|prev| members.contains(&prev.hash())) {
            Some(prev) => at = Rc::clone(prev),
            None => break at,
        }
    };
    for hash in passed {
        roots.insert(hash, Rc::clone(&root));
    }
    root
}

fn step() {
    #[cfg(test)]
    STEPS.set(STEPS.get() + 1);
}

/// A write a contribution or a merged entry holds.
#[derive(Clone)]
struct Candidate {
    at: Ref,
    stamp: Hlc,
    field: FieldKey,
    value: Value,
}

/// What the merge kills by: references, own deletes and takes.
#[derive(Default)]
struct Kills {
    /// Per field: each target, and whether it kills only its exact hash.
    refs: HashMap<FieldKey, Vec<(Ref, bool)>>,
    /// Per writer and entity, the numbers of its own deletes.
    deletes: HashMap<(WriterId, EntityId), Vec<u64>>,
    /// Per field and writer, the highest number below which a take of a run of
    /// that writer settles the field.
    takes: HashMap<(FieldKey, WriterId), u64>,
    contradicted: BTreeSet<FieldKey>,
}

impl Kills {
    fn reference(&mut self, field: FieldKey, target: Ref, exact: bool) {
        if exact {
            self.contradicted.insert(field.clone());
        }
        self.refs.entry(field).or_default().push((target, exact));
    }

    fn take(&mut self, field: FieldKey, writer: WriterId, below: u64) {
        let kept = self.takes.entry((field, writer)).or_insert(below);
        *kept = (*kept).max(below);
    }
}

/// Everything the reader derives from the files it sees. A reference the files
/// contradict kills only its exact target.
pub fn derive(seen: &BTreeMap<WriterId, Seen<'_>>) -> Derived {
    let classified: BTreeMap<WriterId, Writer> = seen
        .iter()
        .map(|(writer, files)| (*writer, classify(*writer, files)))
        .collect();
    // Dismissals from what is merged so far: tips' own at or below their fork
    // point, and placed entries not beyond it.
    let mut runs_dismissed = Vec::new();
    for w in classified.values() {
        for tip in &w.tips {
            for (own, run) in &tip.state.dismissals {
                if !w.beyond(own.n) {
                    runs_dismissed.push(*run);
                }
            }
        }
        for hash in &w.placed {
            let entry = w.entry(*hash);
            if w.beyond(entry.n()) {
                continue;
            }
            for op in contribution::ops(&entry.kind()) {
                runs_dismissed.extend(contribution::said(op).dismissals);
            }
        }
    }
    let dismissals = Dismissals(runs_dismissed);
    // The lines and snapshot heads the files show, for contradicted references.
    let shown: HashMap<WriterId, (u64, Numbered)> = seen
        .iter()
        .map(|(writer, files)| {
            let mut at: BTreeMap<u64, BTreeSet<EntryHash>> = BTreeMap::new();
            let lines = files
                .segments
                .iter()
                .flat_map(|segment| segment.entries.iter())
                .map(|entry| entry.own());
            let heads = files.snapshots.iter().map(|snapshot| snapshot.head);
            for own in lines.chain(heads) {
                if own.n != Entry::UNNUMBERED {
                    at.entry(own.n).or_default().insert(own.hash);
                }
            }
            let highest = at.keys().next_back().copied().unwrap_or(0);
            (*writer, (highest, at))
        })
        .collect();
    let contradicted = |target: &Ref| {
        let Some((highest, at)) = shown.get(&target.writer) else {
            return false;
        };
        let other = at
            .get(&target.n)
            .is_some_and(|hashes| !hashes.contains(&target.hash));
        target.n > *highest || other
    };
    let mut derived = Derived::default();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut kills = Kills::default();
    let mut placed_out: BTreeMap<WriterId, Placed> = BTreeMap::new();
    let mut merged_entries: BTreeMap<WriterId, Vec<Rc<Entry>>> = BTreeMap::new();
    let mut beside = Beside::default();
    for w in classified.values() {
        let covered = dismissals.covered(w);
        let mut memo = HashMap::default();
        let through = |hash: EntryHash| w.candidates.contains(&hash) && w.known.contains_key(&hash);
        let hit = |hash: EntryHash| covered.contains(&hash);
        let after: BTreeSet<EntryHash> = w
            .candidates
            .iter()
            .copied()
            .filter(|hash| {
                !covered.contains(hash)
                    && reaches(w, w.entry(*hash).prev(), &mut memo, &through, &hit)
            })
            .collect();
        let held: BTreeSet<EntryHash> = w.candidates.difference(&w.placed).copied().collect();
        let mut runs: BTreeSet<EntryHash> = w
            .candidates
            .iter()
            .copied()
            .filter(|hash| {
                !covered.contains(hash) && (w.beyond(w.entry(*hash).n()) || after.contains(hash))
            })
            .collect();
        let tip_n = w.tip_n();
        for hash in &held {
            let n = w.entry(*hash).n();
            if !covered.contains(hash)
                && !runs.contains(hash)
                && tip_n.is_some_and(|tip| n <= tip.saturating_add(1))
            {
                runs.insert(*hash);
            }
        }
        loop {
            let before = runs.len();
            for hash in &held {
                if runs.contains(&w.entry(*hash).prev()) && !covered.contains(hash) {
                    runs.insert(*hash);
                }
            }
            if runs.len() == before {
                break;
            }
        }
        let gaps: BTreeSet<EntryHash> = held
            .iter()
            .copied()
            .filter(|hash| {
                !runs.contains(hash) && !covered.contains(hash) && !w.misnumbered.contains(hash)
            })
            .collect();
        let in_order = |set: &BTreeSet<EntryHash>| -> Vec<Rc<Entry>> {
            let mut entries: Vec<Rc<Entry>> =
                set.iter().map(|hash| Rc::clone(w.entry(*hash))).collect();
            entries.sort_by_key(|entry| (entry.n(), entry.hash()));
            entries
        };
        let merged: BTreeSet<EntryHash> = w
            .placed
            .iter()
            .copied()
            .filter(|hash| !runs.contains(hash) && !covered.contains(hash))
            .collect();
        let unplaced: BTreeSet<EntryHash> = w
            .ambiguous
            .iter()
            .copied()
            .filter(|hash| !w.folded.contains(hash))
            .collect();
        merged_entries.insert(w.writer, in_order(&merged));
        placed_out.insert(
            w.writer,
            Placed {
                tips: w.tips.clone(),
                merged: in_order(&merged),
                runs: in_order(&runs),
                gaps: in_order(&gaps),
                unplaced: in_order(&unplaced),
                dismissed: in_order(&covered),
                forks: w.forks.clone(),
                fork: w.lowest,
                misnumbered: w.misnumbered.iter().copied().collect(),
            },
        );
        let ok = |n: u64| !w.beyond(n);
        for tip in &w.tips {
            let state = &tip.state;
            let dismissed = |n: u64| dismissals.cover_tip(w.writer, tip, n);
            for (field, held) in &state.own {
                if ok(held.at.n) && !dismissed(held.at.n) {
                    candidates.push(Candidate {
                        at: held.at.of(w.writer),
                        stamp: held.stamp,
                        field: field.clone(),
                        value: held.value.clone(),
                    });
                }
            }
            for ((field, _, _), reference) in &state.refs {
                if ok(reference.own.n) && !dismissed(reference.own.n) {
                    kills.reference(
                        field.clone(),
                        reference.target,
                        contradicted(&reference.target),
                    );
                }
            }
            for ((field, target), taken) in &state.takes {
                if ok(taken.own.n) && !dismissed(taken.own.n) {
                    kills.take(field.clone(), *target, taken.below);
                }
            }
            for (field, own) in &state.killed {
                if ok(own.n) && !dismissed(own.n) {
                    kills
                        .deletes
                        .entry((w.writer, field.entity()))
                        .or_default()
                        .push(own.n);
                }
            }
            keep_beside(&mut beside, w.writer, state);
        }
    }
    for (writer, entries) in &merged_entries {
        let mut tail = Contribution::default();
        for entry in entries {
            let kind = entry.kind();
            let own = entry.own();
            let saids: Vec<Said> = contribution::ops(&kind)
                .iter()
                .map(contribution::said)
                .collect();
            let exact: BTreeSet<WriterId> = saids
                .iter()
                .flat_map(|said| said.refs.iter().map(|(_, target)| target))
                .filter(|target| contradicted(target))
                .map(|target| target.writer)
                .collect();
            for said in saids {
                for write in said.writes {
                    candidates.push(Candidate {
                        at: own.of(*writer),
                        stamp: entry.at(),
                        field: write.field,
                        value: write.value,
                    });
                }
                for (field, target) in said.refs {
                    let exact = exact.contains(&target.writer);
                    kills.reference(field, target, exact);
                }
                for (field, run) in said.takes {
                    kills.take(field, run.writer, run.from);
                }
                if let Some(entity) = said.deletes {
                    kills
                        .deletes
                        .entry((*writer, entity))
                        .or_default()
                        .push(own.n);
                }
            }
            tail.fold(*writer, entry);
        }
        keep_beside(&mut beside, *writer, &tail);
    }
    let lowest: HashMap<WriterId, u64> = classified
        .values()
        .filter_map(|w| Some((w.writer, w.lowest?)))
        .collect();
    let beyond = |writer: WriterId, n: u64| lowest.get(&writer).is_some_and(|fork| n > *fork);
    // A write is live unless its writer's later own write to its field, or its
    // own delete of its entity, a reference or a take kills it.
    let mut latest_own: HashMap<(WriterId, FieldKey), u64> = HashMap::default();
    for candidate in &candidates {
        let slot = latest_own
            .entry((candidate.at.writer, candidate.field.clone()))
            .or_insert(candidate.at.n);
        *slot = (*slot).max(candidate.at.n);
    }
    let mut seen_writes: BTreeSet<(EntryHash, FieldKey)> = BTreeSet::new();
    let live: Vec<Live> = candidates
        .into_iter()
        .filter(|candidate| seen_writes.insert((candidate.at.hash, candidate.field.clone())))
        .filter(|candidate| {
            let Candidate { at, field, .. } = candidate;
            let own_later = latest_own[&(at.writer, field.clone())] > at.n;
            let own_deleted = !field.is_exist()
                && kills
                    .deletes
                    .get(&(at.writer, field.entity()))
                    .is_some_and(|deletes| deletes.iter().any(|n| *n >= at.n));
            let referenced = kills.refs.get(field).is_some_and(|targets| {
                targets.iter().any(|(target, exact)| {
                    let exact = *exact || beyond(target.writer, target.n);
                    target.writer == at.writer
                        && (target.hash == at.hash || (!exact && at.n < target.n))
                })
            });
            let settled = kills
                .takes
                .get(&(field.clone(), at.writer))
                .is_some_and(|below| at.n < *below);
            !(own_later || own_deleted || referenced || settled)
        })
        .map(|candidate| Live {
            at: candidate.at,
            stamp: candidate.stamp,
            field: candidate.field,
            value: candidate.value,
        })
        .collect();
    derived.folded = Folded::of(live, beside);
    derived.contradicted = kills.contradicted.clone();
    for w in classified.values() {
        let placed = &placed_out[&w.writer];
        let items = items(
            w,
            placed,
            &derived.folded,
            &dismissals,
            &kills,
            &merged_entries,
        );
        if !items.is_empty() {
            derived.items.insert(w.writer, items);
        }
    }
    derived.writers = placed_out;
    derived
}

/// The runs, gaps and unplaced lines of `w`, each with the change it makes
/// against `folded`: a run with entries no dismissal names even when that
/// change is empty, a gap or unplaced lines only when it is not.
fn items(
    w: &Writer,
    placed: &Placed,
    folded: &Folded,
    dismissals: &Dismissals,
    kills: &Kills,
    merged: &BTreeMap<WriterId, Vec<Rc<Entry>>>,
) -> Vec<Item> {
    struct Group {
        kind: ItemKind,
        run: Run,
        span: Option<(Hlc, Hlc)>,
        entries: u64,
        writes: Vec<(FieldKey, Ref, Hlc, Value)>,
        forced: BTreeMap<FieldKey, (Ref, Hlc, Value)>,
    }
    let mut groups: BTreeMap<(ItemKind, EntryHash), Group> = BTreeMap::new();
    let beyond_f = |n: u64| w.beyond(n);
    // Where several tips part inside a segment, the fork point may lie as high
    // as just below the lowest head.
    let highest_fork = w
        .lowest
        .map(|fork| match w.tips.iter().map(|tip| tip.head.n).min() {
            Some(low) if w.tips.len() > 1 => fork.max(low.saturating_sub(1)),
            _ => fork,
        });
    let merged_own: BTreeSet<(FieldKey, u64)> = {
        let mut own = BTreeSet::new();
        for tip in &w.tips {
            for (field, held) in &tip.state.own {
                if !beyond_f(held.at.n) && !dismissals.cover_tip(w.writer, tip, held.at.n) {
                    own.insert((field.clone(), held.at.n));
                }
            }
        }
        for entry in merged.get(&w.writer).into_iter().flatten() {
            for op in contribution::ops(&entry.kind()) {
                for write in contribution::said(op).writes {
                    own.insert((write.field, entry.n()));
                }
            }
        }
        own
    };
    let known_before =
        |field: &FieldKey, n: u64| merged_own.iter().any(|(f, at)| f == field && *at < n);
    let taken = |field: &FieldKey| kills.takes.contains_key(&(field.clone(), w.writer));
    // A tip's own write the merge leaves out, above the fork point or dismissed,
    // that replaced an own write the merge does not hold: a forgotten basis.
    for tip in &w.tips {
        let forced: BTreeMap<FieldKey, (Ref, Hlc, Value)> = tip
            .state
            .own
            .iter()
            .filter(|(field, held)| {
                let pre_fork = highest_fork.is_none_or(|fork| held.first <= fork);
                let replaced_own = held.first < held.at.n && (!beyond_f(held.at.n) || pre_fork);
                (beyond_f(held.at.n) || dismissals.cover_tip(w.writer, tip, held.at.n))
                    && replaced_own
                    && !known_before(field, held.at.n)
                    && !taken(field)
            })
            .map(|(field, held): (&FieldKey, &Held)| {
                (
                    field.clone(),
                    (held.at.of(w.writer), held.stamp, held.value.clone()),
                )
            })
            .collect();
        if !beyond_f(tip.head.n) && forced.is_empty() {
            continue;
        }
        let mut own: Vec<(FieldKey, Ref, Hlc, Value)> = tip
            .state
            .own
            .iter()
            .filter(|(_, held)| {
                beyond_f(held.at.n) && !dismissals.cover_tip(w.writer, tip, held.at.n)
            })
            .map(|(field, held)| {
                (
                    field.clone(),
                    held.at.of(w.writer),
                    held.stamp,
                    held.value.clone(),
                )
            })
            .collect();
        own.sort_by_key(|(_, at, _, _)| at.n);
        let from = w.lowest.map_or(0, |fork| fork + 1);
        let first = dismissals.after_in_tip(w.writer, tip, from);
        let above = first.and_then(|first| tip.head.n.checked_sub(first));
        let entries = above.map_or(0, |above| above.saturating_add(1));
        let head = (entries > 0).then_some(tip.at);
        let readings = own.iter().map(|(_, _, at, _)| *at);
        let readings = readings.chain(forced.values().map(|(_, at, _)| *at));
        let span = readings.chain(head).fold(None, widen);
        groups.insert(
            (ItemKind::Run, tip.head.hash),
            Group {
                kind: ItemKind::Run,
                run: Run {
                    writer: w.writer,
                    from,
                    first: None,
                    last: tip.head,
                    tip: Some(tip.head),
                },
                span,
                entries,
                writes: own,
                forced,
            },
        );
    }
    for (list, kind) in [
        (&placed.runs, ItemKind::Run),
        (&placed.gaps, ItemKind::Gap),
        (&placed.unplaced, ItemKind::Unplaced),
    ] {
        let members: HashSet<EntryHash> = list.iter().map(|entry| entry.hash()).collect();
        let mut roots: HashMap<EntryHash, Rc<Entry>> = HashMap::default();
        for entry in list.iter() {
            let root = root_in(w, entry, &members, &mut roots);
            let key = match root.prev() {
                prev if kind == ItemKind::Run
                    && w.is_head(prev)
                    && groups.contains_key(&(ItemKind::Run, prev)) =>
                {
                    (ItemKind::Run, prev)
                }
                _ => (kind, root.hash()),
            };
            let group = groups.entry(key).or_insert_with(|| Group {
                kind,
                run: Run {
                    writer: w.writer,
                    from: root.n(),
                    first: Some(root.hash()),
                    last: entry.own(),
                    tip: None,
                },
                span: None,
                entries: 0,
                writes: Vec::new(),
                forced: BTreeMap::new(),
            });
            group.run.last = entry.own();
            group.entries += 1;
            group.span = widen(group.span, entry.at());
            for op in contribution::ops(&entry.kind()) {
                for write in contribution::said(op).writes {
                    group.writes.push((
                        write.field,
                        entry.own().of(w.writer),
                        entry.at(),
                        write.value,
                    ));
                }
            }
        }
    }
    let mut items = Vec::new();
    for (_, group) in groups {
        let mut latest: BTreeMap<FieldKey, (Ref, Hlc, Value)> = BTreeMap::new();
        for (field, at, stamp, value) in group.writes {
            latest.insert(field, (at, stamp, value));
        }
        latest.retain(|field, (_, _, value)| folded.differs(field, value));
        latest.extend(
            group
                .forced
                .iter()
                .map(|(field, write)| (field.clone(), write.clone())),
        );
        let shown_empty = group.kind == ItemKind::Run && group.entries > 0;
        if latest.is_empty() && !shown_empty {
            continue;
        }
        items.push(Item {
            kind: group.kind,
            run: group.run,
            span: group.span.unwrap_or_default(),
            entries: group.entries,
            change: latest,
            forgotten: group.forced.into_keys().collect(),
        });
    }
    items
}

/// `span` widened to hold the reading `at`.
fn widen(span: Option<(Hlc, Hlc)>, at: Hlc) -> Option<(Hlc, Hlc)> {
    Some(match span {
        Some((first, last)) => (first.min(at), last.max(at)),
        None => (at, at),
    })
}

/// Whether `placed` holds nothing beside its merged chain and tips: no fork
/// point, and no run, gap, unplaced, dismissed or misnumbered entry.
fn settled(placed: &Placed) -> bool {
    placed.fork.is_none()
        && placed.runs.is_empty()
        && placed.gaps.is_empty()
        && placed.unplaced.is_empty()
        && placed.dismissed.is_empty()
        && placed.misnumbered.is_empty()
}

/// Merges `entries`, `writer`'s next entries after the last one `derived`
/// merges of it, into `derived` as deriving afresh would. Returns false, having
/// changed nothing, where they may change more than the fields they write: the
/// writer has a fork point, a run, a gap, or an unplaced, dismissed or
/// misnumbered entry, an item is shown, or an entry dismisses a run, does not
/// follow, or references a writer in such a state or an entry numbered past
/// that writer's head, which the files may contradict.
pub fn extend(derived: &mut Derived, writer: WriterId, entries: &[Rc<Entry>]) -> bool {
    let Some(placed) = derived.writers.get(&writer) else {
        return false;
    };
    if !settled(placed) || !derived.items.is_empty() {
        return false;
    }
    let shown = |target: &Ref| {
        let placed = derived
            .writers
            .get(&target.writer)
            .filter(|placed| settled(placed));
        placed
            .and_then(Placed::head)
            .is_some_and(|head| target.n <= head.n)
    };
    let mut head = placed.head();
    for entry in entries {
        let follows = match head {
            Some(head) => entry.prev() == head.hash && head.n.checked_add(1) == Some(entry.n()),
            None => entry.prev() == EntryHash::ZERO && entry.n() == 0 && placed.tips.is_empty(),
        };
        let kind = entry.kind();
        let ops = contribution::ops(&kind);
        let dismisses = ops.iter().any(|op| matches!(op, Op::Dismiss { .. }));
        let unshown = ops.iter().any(|op| {
            let said = contribution::said(op);
            said.refs.iter().any(|(_, target)| !shown(target))
        });
        if !follows || dismisses || unshown {
            return false;
        }
        head = Some(entry.own());
    }
    let forks: HashMap<WriterId, u64> = derived
        .writers
        .iter()
        .filter_map(|(writer, placed)| Some((*writer, placed.fork?)))
        .collect();
    let folded = &mut derived.folded;
    let mut touched: BTreeMap<EntityId, Vec<Live>> = BTreeMap::new();
    let mut tail = Contribution::default();
    for entry in entries {
        let own = entry.own();
        let mut deleted = Vec::new();
        for op in contribution::ops(&entry.kind()) {
            let said = contribution::said(op);
            for (field, target) in &said.refs {
                let exact = forks
                    .get(&target.writer)
                    .is_some_and(|fork| target.n > *fork);
                let lives = touched
                    .entry(field.entity())
                    .or_insert_with(|| folded.lives(field.entity()));
                lives.retain(|live| {
                    let covered = live.at.hash == target.hash || (!exact && live.at.n < target.n);
                    !(live.field == *field && live.at.writer == target.writer && covered)
                });
            }
            for (field, run) in &said.takes {
                let lives = touched
                    .entry(field.entity())
                    .or_insert_with(|| folded.lives(field.entity()));
                lives.retain(|live| {
                    !(live.field == *field && live.at.writer == run.writer && live.at.n < run.from)
                });
            }
            for write in said.writes {
                let entity = write.field.entity();
                let lives = touched
                    .entry(entity)
                    .or_insert_with(|| folded.lives(entity));
                lives.retain(|live| !(live.field == write.field && live.at.writer == writer));
                lives.push(Live {
                    at: own.of(writer),
                    stamp: entry.at(),
                    field: write.field,
                    value: write.value,
                });
            }
            deleted.extend(said.deletes);
        }
        for entity in deleted {
            let lives = touched
                .entry(entity)
                .or_insert_with(|| folded.lives(entity));
            lives.retain(|live| {
                live.field.is_exist() || live.at.writer != writer || live.at.n > own.n
            });
        }
        tail.fold(writer, entry);
    }
    for (entity, lives) in touched {
        folded.set_lives(entity, lives);
    }
    let mut beside = Beside::default();
    keep_beside(&mut beside, writer, &tail);
    folded.keep(beside);
    let placed = derived.writers.get_mut(&writer).expect("checked above");
    placed.merged.extend(entries.iter().cloned());
    true
}

/// Adds what `writer`'s `contribution` keeps beside its writes.
fn keep_beside(beside: &mut Beside, writer: WriterId, contribution: &Contribution) {
    for (item, trashed) in &contribution.trash {
        beside.trash.insert((writer, *item), trashed.clone());
    }
    beside.settled.extend(contribution.settled.iter().copied());
    beside.unknown.extend(contribution.unknown.iter().cloned());
    beside
        .extended
        .extend(contribution.extended.iter().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::{snapshot, Found};
    use crate::log::{EntryKind, Logged, Op};
    use crate::schema::Raw;

    const W: WriterId = WriterId::from_u128(0xa);
    const X: WriterId = WriterId::from_u128(0xb);
    const E: EntityId = EntityId::from_u128(0xe);

    fn entry(prev: Option<&Entry>, ops: Vec<Op>, label: &str) -> Rc<Entry> {
        let (hash, n) = prev.map_or((EntryHash::ZERO, 0), |prev| (prev.hash(), prev.n() + 1));
        let at = Hlc {
            wall_ms: n * 10 + label.len() as u64,
            counter: 0,
        };
        let kind = EntryKind::Intent(Logged {
            label: label.into(),
            ops,
            displaced: Vec::new(),
            forks: Vec::new(),
            record: None,
        });
        Rc::new(Entry::encode(hash, n, at, kind).unwrap())
    }

    fn set(value: &str, replaces: Vec<Ref>) -> Op {
        Op::Write {
            entity: E,
            key: "k".into(),
            value: Some(Raw::of(&value).unwrap()),
            replaces,
            take: None,
        }
    }

    /// A chain of writes of `values` to one register, each replacing the last.
    fn chain(writer: WriterId, values: &[&str]) -> Vec<Rc<Entry>> {
        let mut entries: Vec<Rc<Entry>> = Vec::new();
        for value in values {
            let replaces = entries
                .last()
                .map(|e| e.own().of(writer))
                .into_iter()
                .collect();
            let next = entry(
                entries.last().map(|e| &**e),
                vec![set(value, replaces)],
                value,
            );
            entries.push(next);
        }
        entries
    }

    fn lines(entries: &[Rc<Entry>], sealed: bool) -> SeenSegment<'_> {
        SeenSegment { entries, sealed }
    }

    fn values(derived: &Derived) -> Vec<String> {
        let writes = derived.folded.register(E, "k");
        let mut values: Vec<String> = writes.iter().map(|w| w.value.decode().unwrap()).collect();
        values.sort();
        values
    }

    fn derived(seen: Vec<(WriterId, Seen<'_>)>) -> Derived {
        derive(&seen.into_iter().collect())
    }

    #[test]
    fn a_chain_numbered_against_its_order_is_one_item() {
        let numbered = |prev: &Entry, n: u64, value: &str| {
            let at = Hlc {
                wall_ms: 100 + n,
                counter: 0,
            };
            let kind = EntryKind::Intent(Logged {
                label: value.into(),
                ops: vec![set(value, Vec::new())],
                displaced: Vec::new(),
                forks: Vec::new(),
                record: None,
            });
            Rc::new(Entry::encode(prev.hash(), n, at, kind).unwrap())
        };
        let genesis = entry(None, vec![set("a", Vec::new())], "a");
        let misnumbered = numbered(&genesis, 5, "b");
        let first = numbered(&misnumbered, 2, "c");
        let second = numbered(&first, 1, "d");
        let entries = [genesis, misnumbered, first.clone(), second];
        let derived = derived(vec![(
            W,
            Seen {
                segments: vec![lines(&entries, false)],
                snapshots: vec![],
            },
        )]);
        let items = &derived.items[&W];
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].run.first, Some(first.hash()), "{items:?}");
    }

    #[test]
    fn entries_arriving_in_any_order_are_merged_in_chain_order() {
        let entries = chain(W, &["a", "b", "c"]);
        let late: Vec<Rc<Entry>> = entries[1..].to_vec();
        let early = derived(vec![(
            W,
            Seen {
                segments: vec![lines(&late, false)],
                snapshots: vec![],
            },
        )]);
        let placed = &early.writers[&W];
        assert_eq!(placed.merged, []);
        assert_eq!(
            placed.gaps.len(),
            2,
            "held back until their predecessor comes"
        );
        let reversed: Vec<Rc<Entry>> = entries.iter().rev().cloned().collect();
        let all = derived(vec![(
            W,
            Seen {
                segments: vec![lines(&reversed, false)],
                snapshots: vec![],
            },
        )]);
        assert_eq!(all.writers[&W].merged, entries);
        assert_eq!(values(&all), ["c"]);
    }

    #[test]
    fn two_successors_of_one_entry_are_a_fork_and_neither_branch_is_merged() {
        let entries = chain(W, &["a"]);
        let left = entry(
            Some(&entries[0]),
            vec![set("left", vec![entries[0].own().of(W)])],
            "l",
        );
        let right = entry(
            Some(&entries[0]),
            vec![set("right", vec![entries[0].own().of(W)])],
            "r",
        );
        let all = vec![Rc::clone(&entries[0]), left, right];
        let derived = derived(vec![(
            W,
            Seen {
                segments: vec![lines(&all, false)],
                snapshots: vec![],
            },
        )]);
        let placed = &derived.writers[&W];
        assert_eq!(placed.fork, Some(0));
        assert_eq!(placed.runs.len(), 2);
        assert_eq!(values(&derived), ["a"]);
        let items = &derived.items[&W];
        assert_eq!(items.len(), 2, "one item per branch");
    }

    /// `entries` of `W` folded into a snapshot that certifies the segment they
    /// were written to.
    fn folded(entries: &[Rc<Entry>]) -> Rc<Snapshot> {
        let found = [Found::Lines(entries)];
        let snapshot = snapshot(W, entries[0].hash(), "w".into(), None, entries, found);
        Rc::new(snapshot.unwrap())
    }

    #[test]
    fn a_tip_compacted_past_a_dismissed_line_run_is_shown_again() {
        let entries = chain(W, &["a", "b", "c", "d"]);
        let tip = folded(&entries);
        let run = |last: &Rc<Entry>| {
            Dismissals(vec![crate::log::Run {
                writer: W,
                from: entries[1].n(),
                first: Some(entries[1].hash()),
                last: last.own(),
                tip: None,
            }])
        };
        assert!(!run(&entries[2]).cover_tip(W, &tip, entries[2].n()));
        assert!(
            run(&entries[3]).cover_tip(W, &tip, entries[2].n()),
            "the tip names it"
        );
    }

    /// The steps the walks back along chains take to derive what `seen` holds.
    fn steps_to_derive(seen: BTreeMap<WriterId, Seen<'_>>) -> usize {
        STEPS.set(0);
        derive(&seen);
        STEPS.get()
    }

    /// Another writer's dismissal of `W`'s run from `first` to `last`.
    fn dismissal(first: &Entry, last: &Entry) -> Vec<Rc<Entry>> {
        let run = Run {
            writer: W,
            from: first.n(),
            first: Some(first.hash()),
            last: last.own(),
            tip: None,
        };
        vec![entry(
            None,
            vec![Op::Dismiss { runs: vec![run] }],
            "dismiss",
        )]
    }

    #[test]
    fn deriving_a_long_uncompacted_chain_takes_steps_in_proportion_to_its_entries() {
        let values: Vec<String> = (0..2_000).map(|n| n.to_string()).collect();
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        fn held(entries: &[Rc<Entry>]) -> Seen<'_> {
            Seen {
                segments: vec![lines(entries, false)],
                snapshots: vec![],
            }
        }
        for n in [500, 2_000] {
            let chain = chain(W, &values[..n]);
            let mut branch = vec![Rc::clone(&chain[0])];
            for value in &values[1..n] {
                let next = entry(branch.last().map(|e| &**e), vec![set(value, vec![])], "b");
                branch.push(next);
            }
            let tip = folded(&chain);
            let one = dismissal(&chain[1], &chain[1]);
            let all = dismissal(&chain[1], &chain[n - 1]);
            let beside = Seen {
                segments: vec![lines(&branch[1..], false)],
                snapshots: vec![tip],
            };
            let cases = [
                (
                    "a dismissal of one entry",
                    BTreeMap::from([(W, held(&chain)), (X, held(&one))]),
                ),
                (
                    "a dismissal of every entry after the first",
                    BTreeMap::from([(W, held(&chain)), (X, held(&all))]),
                ),
                (
                    "a branch beside a tip that folds the other",
                    BTreeMap::from([(W, beside)]),
                ),
                (
                    "entries held without the first",
                    BTreeMap::from([(W, held(&chain[1..]))]),
                ),
            ];
            for (case, seen) in cases {
                let steps = steps_to_derive(seen);
                assert!(steps <= 4 * n, "{case}: {steps} steps for {n} entries");
            }
        }
    }

    #[test]
    fn a_copy_of_a_certified_segment_is_folded_and_a_shorter_one_is_unplaced() {
        let entries = chain(W, &["a", "b", "c"]);
        let tip = folded(&entries);
        assert!(
            tip.certified.hashes.is_empty(),
            "the segment certificate covers every line"
        );
        let shorter: Vec<Rc<Entry>> = entries[..2].to_vec();
        let none: &[&str] = &[];
        let cases = [
            (&entries[..], false, 0, None, &["c"][..]),
            (&shorter[..], false, 1, None, &["c"][..]),
            (&shorter[..], true, 0, Some(0), none),
        ];
        for (copy, sealed, unplaced, fork, shown) in cases {
            let seen = Seen {
                segments: vec![lines(copy, sealed)],
                snapshots: vec![Rc::clone(&tip)],
            };
            let derived = derived(vec![(W, seen)]);
            let placed = &derived.writers[&W];
            let case = format!("{} lines, sealed {sealed}", copy.len());
            assert_eq!(
                placed.unplaced.len(),
                unplaced,
                "{case}: an unsealed copy's lines past the first"
            );
            assert_eq!(
                placed.fork, fork,
                "{case}: a sealed copy that ends early diverged"
            );
            assert_eq!(
                values(&derived),
                shown,
                "{case}: a forked tip is shown as an item"
            );
        }
    }

    #[test]
    fn a_dismissed_run_is_never_merged_nor_shown() {
        let base = chain(W, &["a"]);
        let left = entry(
            Some(&base[0]),
            vec![set("left", vec![base[0].own().of(W)])],
            "l",
        );
        let right = entry(
            Some(&base[0]),
            vec![set("right", vec![base[0].own().of(W)])],
            "r",
        );
        let own = vec![Rc::clone(&base[0]), Rc::clone(&left), Rc::clone(&right)];
        let run = Run {
            writer: W,
            from: 1,
            first: Some(left.hash()),
            last: left.own(),
            tip: None,
        };
        let dismiss = entry(None, vec![Op::Dismiss { runs: vec![run] }], "dismiss");
        let theirs = vec![dismiss];
        let derived = derived(vec![
            (
                W,
                Seen {
                    segments: vec![lines(&own, false)],
                    snapshots: vec![],
                },
            ),
            (
                X,
                Seen {
                    segments: vec![lines(&theirs, false)],
                    snapshots: vec![],
                },
            ),
        ]);
        let items = &derived.items[&W];
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].run.first, Some(right.hash()));
        let dismissed = derived.writers[&W]
            .dismissed
            .iter()
            .map(|entry| entry.hash());
        assert_eq!(dismissed.collect::<Vec<_>>(), [left.hash()]);
        assert_eq!(values(&derived), ["a"]);
    }

    #[test]
    fn an_own_delete_ends_the_writes_of_its_own_entry() {
        let add = Op::Add {
            entity: E,
            key: "s".into(),
            value: Raw::of(&1).unwrap(),
            replaces: Vec::new(),
            take: None,
        };
        let delete = Op::Delete {
            entity: E,
            replaces: Vec::new(),
            observed: Vec::new(),
            take: None,
        };
        let entries = vec![entry(None, vec![add, delete], "both")];
        let derived = derived(vec![(
            W,
            Seen {
                segments: vec![lines(&entries, false)],
                snapshots: vec![],
            },
        )]);
        assert!(derived.folded.members(E, "s").is_empty());
        assert!(!derived.folded.present(E));
    }

    #[test]
    fn own_entries_merged_one_by_one_merge_as_deriving_afresh_does() {
        use crate::env::{Random, SeededRandom};
        let mut random = SeededRandom::new(5);
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        let theirs = chain(X, &["x", "y"]);
        let mut extended = 0;
        for _ in 0..300 {
            let mut own: Vec<Rc<Entry>> = Vec::new();
            let seen = |own: &[Rc<Entry>]| {
                derived(vec![
                    (
                        W,
                        Seen {
                            segments: vec![lines(own, false)],
                            snapshots: vec![],
                        },
                    ),
                    (
                        X,
                        Seen {
                            segments: vec![lines(&theirs, false)],
                            snapshots: vec![],
                        },
                    ),
                ])
            };
            let mut incremental = seen(&own);
            for step in 0..6 {
                let mut targets: Vec<Ref> = theirs.iter().map(|e| e.own().of(X)).collect();
                targets.extend(own.iter().map(|e| e.own().of(W)));
                let replaces: Vec<Ref> =
                    (0..pick(3)).map(|_| targets[pick(targets.len())]).collect();
                let entity = EntityId::from_u128(0xe + pick(2) as u128);
                let op = match pick(4) {
                    0 => Op::Delete {
                        entity,
                        replaces,
                        observed: Vec::new(),
                        take: None,
                    },
                    1 => Op::Create {
                        entity,
                        replaces,
                        take: None,
                    },
                    _ => Op::Write {
                        entity,
                        key: "k".into(),
                        value: Some(Raw::of(&step).unwrap()),
                        replaces,
                        take: None,
                    },
                };
                let next = entry(own.last().map(|e| &**e), vec![op], &format!("s{step}"));
                own.push(Rc::clone(&next));
                let afresh = seen(&own);
                if extend(&mut incremental, W, &[next]) {
                    extended += 1;
                    assert_eq!(incremental.folded, afresh.folded, "step {step}");
                    assert_eq!(incremental.writers[&W].merged, afresh.writers[&W].merged);
                } else {
                    incremental = afresh;
                }
            }
        }
        assert!(extended > 1000, "{extended} merged one by one");
    }
}
