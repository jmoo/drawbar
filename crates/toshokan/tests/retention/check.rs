//! The properties, and the enumeration that drives them.
//!
//! P3, P5, P6, P8, P10, P12: at rest, fresh and cached readers of the design
//! show what the oracle shows. P1: along a cached reader's deliveries, a value a
//! merged write replaced never shows alone again. P4: a step by one branch of a
//! forked writer changes no merged value. P7: no reader merges what a dismissal
//! it holds names. P9: a contribution's size is bounded by fields and writers.
//! P11: after a restore or a dropped version and the grace step, the folder
//! wins. Three documented divergences are counted rather than failed: a value
//! the oracle resolves and design R shows beside it as a conflict, after a fork
//! or, after a fault, a reference that names a lost write; and a merged value
//! design R shows only in a run, as a forgotten basis or because the fork lies
//! inside a segment it cannot see.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::history::{materialize, Bounds, Folder, History};
use super::oracle::{self, Oracle};
use super::spec::{Contribution, Spec};
use super::world::{self, Body, Dir, Field, File, Hash, Item, ItemKind, Seen, Shown, World, Write};
use super::{Design, Reading};

/// What a sweep counted.
#[derive(Default, Debug)]
pub struct Tally {
    pub histories: AtomicUsize,
    pub reads: AtomicUsize,
    /// Fields where design R shows a conflict the oracle does not.
    pub conflicts: AtomicUsize,
    /// Fields where the oracle merges a value design R shows only in a run: a
    /// forgotten basis, or a fork point it cannot locate exactly.
    pub collapsed: AtomicUsize,
    /// Faulted folders after which a cached reader still holds an entry the
    /// fault took, because something it sees descends from that entry.
    pub stale: AtomicUsize,
    /// Fields that show, after a fault, a write a reference to a lost write no
    /// longer covers.
    pub contradicted: AtomicUsize,
    /// Set when a worker finds a violation, so the others stop.
    pub failed: AtomicBool,
}

impl Tally {
    fn add(&self, counter: &AtomicUsize, n: usize) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub fn report(&self) -> String {
        format!(
            "{} histories, {} reads; {} fields shown as a conflict the oracle resolves, {} fields whose merged value shows only in a run, {} faulted folders where a cached reader keeps a lost entry, {} fields after a fault where a reference to a lost write no longer covers an earlier one",
            self.histories.load(Ordering::Relaxed),
            self.reads.load(Ordering::Relaxed),
            self.conflicts.load(Ordering::Relaxed),
            self.collapsed.load(Ordering::Relaxed),
            self.stale.load(Ordering::Relaxed),
            self.contradicted.load(Ordering::Relaxed),
        )
    }
}

/// How hard to look at each history.
#[derive(Clone, Copy, Debug)]
pub struct Depth {
    /// Delivery orders per folder, at most.
    pub orders: usize,
    /// Whether to restore and drop versions.
    pub faults: bool,
    /// Whether to read every subset of files.
    pub subsets: bool,
}

fn whole<S>(f: &File<S>) -> Seen {
    let lines = match &f.body {
        Body::Segment(lines) => lines.len(),
        Body::Snapshot(_) => 1,
    };
    Seen { file: f.id, lines }
}

/// Every way a reader can have received everything: each file that remains in
/// whole, and each deleted file in whole or not at all.
fn rests<S>(files: &[File<S>]) -> Vec<Vec<Seen>> {
    let kept: Vec<Seen> = files
        .iter()
        .filter(|f| !f.deleted && !empty(f))
        .map(whole)
        .collect();
    let gone: Vec<Seen> = files
        .iter()
        .filter(|f| f.deleted && !empty(f))
        .map(whole)
        .collect();
    let gone = &gone[..gone.len().min(4)];
    (0..1usize << gone.len())
        .map(|mask| {
            let mut seen = kept.clone();
            seen.extend(
                gone.iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, s)| *s),
            );
            seen
        })
        .collect()
}

fn empty<S>(f: &File<S>) -> bool {
    matches!(&f.body, Body::Segment(lines) if lines.is_empty())
}

fn read<D: Design>(
    d: &D,
    world: &World,
    files: &[File<D::Snapshot>],
    seen: &[Seen],
    memory: &D::Memory,
) -> (Shown, D::Memory) {
    d.read(&Reading { world, files, seen }, memory)
}

/// Compares what design R shows with what the oracle shows, counting the two
/// documented divergences.
fn agree(
    world: &World,
    held: &BTreeSet<Hash>,
    tips: &BTreeSet<Hash>,
    oracle: &Shown,
    spec: &Shown,
    tally: &Tally,
    faulted: bool,
) -> Result<(), String> {
    if oracle == spec {
        return Ok(());
    }
    let existence = |shown: &Shown| -> BTreeSet<super::world::Val> {
        let writes = shown.writes.get(&Field::Exist).into_iter().flatten();
        writes
            .filter_map(|&h| world.entry(h).write.and_then(|w| w.value))
            .collect()
    };
    let with_existence = |shown: &Shown| {
        let mut values = shown.values.clone();
        values.insert(Field::Exist, existence(shown));
        values
    };
    let (ov, sv) = (with_existence(oracle), with_existence(spec));
    let forks = oracle::place(world, held, &BTreeSet::new()).forks;
    let forked: BTreeSet<Dir> = forks.iter().map(|f| f.0).collect();
    let fields: BTreeSet<Field> = ov.keys().chain(sv.keys()).copied().collect();
    let mut divergent = false;
    for field in fields {
        let o = ov.get(&field).cloned().unwrap_or_default();
        let s = sv.get(&field).cloned().unwrap_or_default();
        if o == s {
            continue;
        }
        divergent = true;
        let forgetting: BTreeSet<Dir> = spec
            .items
            .iter()
            .filter(|i| i.forgotten.contains(&field))
            .map(|i| i.dir)
            .collect();
        let forgotten = |v: &super::world::Val| {
            let writes = oracle.writes.get(&field).into_iter().flatten();
            writes
                .copied()
                .filter(|&e| world.entry(e).write.is_some_and(|w| w.value == Some(*v)))
                .all(|e| forgetting.contains(&world.entry(e).dir) && !tips.contains(&e))
        };
        // A pre-fork write a reader places above a fork point it cannot locate
        // exactly shows in its writer's run.
        let in_run = |v: &super::world::Val| {
            let writers: BTreeSet<Dir> = oracle
                .writes
                .get(&field)
                .into_iter()
                .flatten()
                .filter(|&&e| world.entry(e).write.is_some_and(|w| w.value == Some(*v)))
                .map(|&e| world.entry(e).dir)
                .collect();
            let mut runs = spec
                .items
                .iter()
                .filter(|i| i.kind == ItemKind::Run && writers.contains(&i.dir));
            runs.any(|i| i.change.get(&field) == Some(&Some(*v)))
        };
        let extra = !s.is_subset(&o);
        if !o.difference(&s).all(|v| forgotten(v) || in_run(v))
            || (extra && !faulted && forked.is_empty())
        {
            return Err(format!("{field:?}: oracle {o:?}, design {s:?}"));
        }
        if !o.is_subset(&s) {
            tally.add(&tally.collapsed, 1);
        }
        if extra && faulted {
            tally.add(&tally.contradicted, 1);
        } else if extra {
            tally.add(&tally.conflicts, 1);
        }
    }
    if divergent || oracle.items == spec.items {
        return Ok(());
    }
    let unforced: BTreeSet<Item> = spec
        .items
        .iter()
        .filter_map(|i| {
            let mut change = i.change.clone();
            change.retain(|&field, &mut value| {
                !i.forgotten.contains(&field)
                    || !world::change(&[Write { field, value }], &spec.values, spec.exists)
                        .is_empty()
            });
            (!change.is_empty()).then(|| Item {
                change,
                forgotten: BTreeSet::new(),
                ..i.clone()
            })
        })
        .collect();
    if unforced != oracle.items {
        return Err("items differ".into());
    }
    tally.add(&tally.collapsed, 1);
    Ok(())
}

/// The values a reader merged: everything but its items.
fn merged(s: &Shown) -> (bool, bool, &BTreeMap<Field, BTreeSet<super::world::Val>>) {
    (s.exists, s.deletion_conflict, &s.values)
}

/// P1 along one sequence of states, each with the entries the dismissals it
/// holds name: a write whose value gave way to another value never shows alone
/// again on its field without an item touching it, unless the user dismissed
/// every write that replaced it.
fn no_silent_return(states: &[(Shown, BTreeSet<Hash>)]) -> Result<(), String> {
    let mut replaced: BTreeMap<(Field, Hash), BTreeSet<Hash>> = BTreeMap::new();
    for pair in states.windows(2) {
        let ((before, _), (after, dismissed)) = (&pair[0], &pair[1]);
        for (field, writes) in &before.writes {
            let now = after.writes.get(field).cloned().unwrap_or_default();
            let then = before.values.get(field).cloned().unwrap_or_default();
            let values = after.values.get(field).cloned().unwrap_or_default();
            for w in writes {
                if !now.contains(w) && !now.is_empty() && values != then {
                    replaced.entry((*field, *w)).or_default().extend(&now);
                }
            }
        }
        for (field, writes) in &after.writes {
            let touched = after.items.iter().any(|i| i.change.contains_key(field));
            for w in writes {
                let by = replaced.get(&(*field, *w));
                if writes.len() == 1 && by.is_some_and(|by| !by.is_subset(dismissed)) && !touched {
                    return Err(format!("{field:?} write {w} came back alone"));
                }
            }
        }
    }
    Ok(())
}

/// Delivery orders of `n` files: every one up to `cap`, then a spread of them.
fn orders(n: usize, cap: usize) -> Vec<Vec<usize>> {
    let mut all = Vec::new();
    let mut order: Vec<usize> = (0..n).collect();
    permute(&mut order, 0, &mut all, cap);
    all
}

fn permute(order: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>, cap: usize) {
    if out.len() >= cap {
        return;
    }
    if k == order.len() {
        out.push(order.clone());
        return;
    }
    for i in k..order.len() {
        order.swap(k, i);
        permute(order, k + 1, out, cap);
        order.swap(k, i);
    }
}

pub struct Checker<'a> {
    pub history: &'a History,
    /// Earlier states of this history, oldest first, for restores.
    pub ancestors: &'a [History],
    pub oracle: Folder<oracle::Folds>,
    pub spec: Folder<Contribution>,
    pub tally: &'a Tally,
    pub depth: Depth,
    pub design: Spec,
}

impl<'a> Checker<'a> {
    pub fn new(
        history: &'a History,
        ancestors: &'a [History],
        tally: &'a Tally,
        depth: Depth,
        design: Spec,
    ) -> Self {
        Self {
            history,
            ancestors,
            oracle: materialize(history, &Oracle),
            spec: materialize(history, &design),
            tally,
            depth,
            design,
        }
    }

    fn fail(&self, what: &str, seen: &[Seen], detail: String) -> ! {
        let files: Vec<String> = seen
            .iter()
            .map(|s| match &self.spec.files[s.file].body {
                Body::Segment(lines) => format!(
                    "seg{}{:?}",
                    s.file,
                    lines
                        .iter()
                        .take(s.lines)
                        .map(|&h| oracle::describe(self.history.world.entry(h)))
                        .collect::<Vec<_>>()
                ),
                Body::Snapshot(c) => format!(
                    "snap{}@{}",
                    s.file,
                    oracle::describe(self.history.world.entry(c.head.hash))
                ),
            })
            .collect();
        panic!(
            "{what}: {detail}\n  history: {:?}\n  seen: {files:?}",
            self.history.acts
        );
    }

    fn world(&self) -> &World {
        &self.history.world
    }

    /// The own writes the tips in `seen` hold: snapshots no other one there
    /// certifies.
    fn tip_writes(files: &[File<Contribution>], seen: &[Seen]) -> BTreeSet<Hash> {
        let snapshots: Vec<&Contribution> = seen
            .iter()
            .filter_map(|s| match &files[s.file].body {
                Body::Snapshot(c) => Some(c),
                Body::Segment(_) => None,
            })
            .collect();
        snapshots
            .iter()
            .filter(|c| {
                !snapshots
                    .iter()
                    .any(|o| o.dir == c.dir && o.head != c.head && o.certifies(c.head))
            })
            .flat_map(|c| c.own.values().map(|(at, _)| at.hash))
            .collect()
    }

    /// The oracle's and design R's fresh reads of `seen`.
    fn fresh(&self, seen: &[Seen]) -> (Shown, Shown, BTreeSet<Hash>) {
        self.tally.add(&self.tally.reads, 1);
        let r = Reading {
            world: self.world(),
            files: &self.oracle.files,
            seen,
        };
        let held = Oracle.holds(&r);
        let (o, _) = Oracle.read(&r, &Default::default());
        let (s, _) = read(
            &self.design,
            self.world(),
            &self.spec.files,
            seen,
            &Default::default(),
        );
        (o, s, held)
    }

    /// Every property on this history.
    pub fn check(&self) -> Shown {
        self.tally.add(&self.tally.histories, 1);
        self.sizes();
        let mut rest_shown = None;
        for seen in rests(&self.spec.files) {
            let (o, s, held) = self.fresh(&seen);
            if let Err(e) = agree(
                self.world(),
                &held,
                &Self::tip_writes(&self.spec.files, &seen),
                &o,
                &s,
                self.tally,
                false,
            ) {
                self.fail(
                    "a fresh reader at rest differs from the oracle",
                    &seen,
                    format!("{e}\n  oracle {o:?}\n  design {s:?}"),
                );
            }
            rest_shown.get_or_insert(s);
        }
        self.copies();
        self.cached();
        if self.depth.subsets {
            self.subsets();
        }
        if self.depth.faults {
            self.faults();
        }
        rest_shown.unwrap_or_default()
    }

    /// A sync client's conflicted copy of every segment, under another name, is
    /// read like the segment.
    fn copies(&self) {
        let mut spec_files = self.spec.files.clone();
        let mut oracle_files = self.oracle.files.clone();
        copy(&mut spec_files);
        copy(&mut oracle_files);
        let seen: Vec<Seen> = spec_files
            .iter()
            .filter(|f| !f.deleted && !empty(f))
            .map(whole)
            .collect();
        let ro = Reading {
            world: self.world(),
            files: &oracle_files,
            seen: &seen,
        };
        let (o, _) = Oracle.read(&ro, &Default::default());
        let (s, _) = read(
            &self.design,
            self.world(),
            &spec_files,
            &seen,
            &Default::default(),
        );
        if let Err(e) = agree(
            self.world(),
            &Oracle.holds(&ro),
            &Self::tip_writes(&spec_files, &seen),
            &o,
            &s,
            self.tally,
            false,
        ) {
            self.fail("a conflicted copy changes what a reader shows", &seen, e);
        }
    }

    /// P9.
    fn sizes(&self) {
        let fields = 1 + 1 + 2;
        let writers = 1 + self
            .history
            .insts
            .iter()
            .map(|i| i.dir as usize)
            .max()
            .unwrap_or(0);
        for f in &self.spec.files {
            let Body::Snapshot(c) = &f.body else { continue };
            let segments = self
                .spec
                .files
                .iter()
                .filter(|g| g.dir == f.dir && matches!(g.body, Body::Segment(_)))
                .count();
            let ok = c.own.len() <= fields
                && c.refs.len() <= fields * (writers - 1)
                && c.certs.segments.len() <= segments
                && c.certs.hashes.len() <= c.dismissals.len();
            if !ok {
                panic!(
                    "P9: snapshot {} is larger than its bound: {c:?}\n  history: {:?}",
                    f.id, self.history.acts
                );
            }
        }
    }

    /// The entries the dismissals in `seen` name.
    fn dismissed(&self, seen: &[Seen]) -> BTreeSet<Hash> {
        let world = self.world();
        let mut dismissals = Vec::new();
        for s in seen {
            match &self.spec.files[s.file].body {
                Body::Segment(lines) => dismissals.extend(
                    lines
                        .iter()
                        .take(s.lines)
                        .flat_map(|&h| world.entry(h).dismisses.iter().copied()),
                ),
                Body::Snapshot(c) => dismissals.extend(c.dismissals.iter().map(|(_, d)| *d)),
            }
        }
        dismissals
            .iter()
            .flat_map(|d| oracle::covered(world, d))
            .collect()
    }

    /// P3 and P1: cached readers in many orders end where a fresh one does, and
    /// never show a replaced value alone again.
    fn cached(&self) {
        let files = &self.spec.files;
        let kept: Vec<Seen> = files
            .iter()
            .filter(|f| !f.deleted && !empty(f))
            .map(whole)
            .collect();
        let gone: Vec<Seen> = files
            .iter()
            .filter(|f| f.deleted && !empty(f))
            .map(whole)
            .collect();
        // Deleted files: never seen; seen first and gone last; or seen first and
        // gone before anything else arrives, as when a compaction's deletions
        // overtake its snapshot.
        for mode in 0..3 {
            let late = mode == 1;
            for order in orders(kept.len(), self.depth.orders) {
                let mut seen: Vec<Seen> = if mode > 0 { gone.clone() } else { Vec::new() };
                let mut memory = Default::default();
                let mut states = Vec::new();
                if mode > 0 {
                    let (s, m) = read(&self.design, self.world(), files, &seen, &memory);
                    states.push((s, self.dismissed(&seen)));
                    memory = m;
                }
                if mode == 2 {
                    seen.clear();
                    let (s, m) = read(&self.design, self.world(), files, &seen, &memory);
                    states.push((s, self.dismissed(&seen)));
                    memory = m;
                }
                for &i in &order {
                    seen.push(kept[i]);
                    let (s, m) = read(&self.design, self.world(), files, &seen, &memory);
                    states.push((s, self.dismissed(&seen)));
                    memory = m;
                }
                if late {
                    for g in &gone {
                        seen.retain(|s| s != g);
                        let (s, m) = read(&self.design, self.world(), files, &seen, &memory);
                        states.push((s, self.dismissed(&seen)));
                        memory = m;
                    }
                }
                self.tally.add(&self.tally.reads, states.len());
                if let Err(e) = no_silent_return(&states) {
                    self.fail("P1: a cached reader shows a replaced value alone", &seen, e);
                }
                let (o, fresh, held) = self.fresh(&seen);
                let cached = states.last().map(|(s, _)| s.clone()).unwrap_or_default();
                if let Err(e) = agree(
                    self.world(),
                    &held,
                    &Self::tip_writes(files, &seen),
                    &o,
                    &cached,
                    self.tally,
                    false,
                ) {
                    self.fail(
                        "P3: a cached reader at rest differs from the oracle",
                        &seen,
                        format!("{e}\n  oracle {o:?}\n  cached {cached:?}\n  fresh {fresh:?}"),
                    );
                }
                let (graced, _) = self.design.grace(
                    &Reading {
                        world: self.world(),
                        files,
                        seen: &seen,
                    },
                    &memory,
                );
                if graced != cached {
                    self.fail(
                        "the grace step changes a reader at rest",
                        &seen,
                        format!("before {cached:?}\n  after {graced:?}"),
                    );
                }
            }
        }
    }

    /// P7 at every subset of files: no merged value is one only dismissed entries wrote.
    fn subsets(&self) {
        let files: Vec<Seen> = self
            .spec
            .files
            .iter()
            .filter(|f| !empty(f))
            .map(whole)
            .collect();
        let files = &files[..files.len().min(8)];
        for mask in 0..1usize << files.len() {
            let seen: Vec<Seen> = files
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, s)| *s)
                .collect();
            let (_, s, held) = self.fresh(&seen);
            let placement = oracle::place(self.world(), &held, &BTreeSet::new());
            let mut written: BTreeMap<Field, BTreeSet<super::world::Val>> = BTreeMap::new();
            for &e in held.difference(&placement.dismissed) {
                if let Some(w) = self.world().entry(e).write {
                    if let Some(v) = w.value {
                        written.entry(w.field).or_default().insert(v);
                    }
                }
            }
            for (field, values) in &s.values {
                for v in values {
                    if !written.get(field).is_some_and(|w| w.contains(v)) {
                        self.fail(
                            "P7: a reader merges a value only dismissed entries wrote",
                            &seen,
                            format!("{field:?}={v:?}"),
                        );
                    }
                }
            }
        }
    }

    /// P11: after a restore of the folder or of one directory, or a dropped
    /// version, a cached reader that had read everything, once it has read the
    /// new folder and passed the grace step, shows what a fresh oracle shows.
    fn faults(&self) {
        let world = self.world();
        let before: Vec<Seen> = rests(&self.spec.files)
            .into_iter()
            .next()
            .unwrap_or_default();
        let (_, memory) = read(
            &self.design,
            world,
            &self.spec.files,
            &before,
            &Default::default(),
        );
        let mut variants: Vec<Fault> = Vec::new();
        for (i, f) in self.spec.files.iter().enumerate() {
            let Body::Segment(lines) = &f.body else {
                continue;
            };
            if f.deleted {
                continue;
            }
            for k in 1..lines.len() {
                let mut s = self.spec.files.clone();
                let mut o = self.oracle.files.clone();
                if let Body::Segment(l) = &mut s[i].body {
                    l.truncate(k);
                }
                if let Body::Segment(l) = &mut o[i].body {
                    l.truncate(k);
                }
                variants.push((format!("segment {i} dropped to {k} lines"), s, o));
            }
        }
        for ancestor in self.ancestors {
            let s_old = materialize(ancestor, &self.design).files;
            let o_old = materialize(ancestor, &Oracle).files;
            let dirs: BTreeSet<Dir> = self.spec.files.iter().map(|f| f.dir).collect();
            let scopes: Vec<(String, BTreeSet<Dir>)> =
                std::iter::once(("the folder".to_string(), dirs.clone()))
                    .chain(
                        dirs.iter()
                            .map(|&d| (format!("directory {d}"), BTreeSet::from([d]))),
                    )
                    .collect();
            for (scope, within) in scopes {
                let label = format!("{scope} restored to {:?}", ancestor.acts.last());
                variants.push((
                    label,
                    restore(&self.spec.files, &s_old, &within),
                    restore(&self.oracle.files, &o_old, &within),
                ));
            }
        }
        for (label, spec_files, oracle_files) in variants {
            let seen: Vec<Seen> = spec_files
                .iter()
                .filter(|f| !f.deleted && !empty(f))
                .map(whole)
                .collect();
            let r = Reading {
                world,
                files: &spec_files,
                seen: &seen,
            };
            let (_, m) = self.design.read(&r, &memory);
            let (after, kept) = self.design.grace(&r, &m);
            let ro = Reading {
                world,
                files: &oracle_files,
                seen: &seen,
            };
            let (o, _) = Oracle.read(&ro, &Default::default());
            let held = Oracle.holds(&ro);
            if kept.entries.iter().any(|e| !held.contains(e)) {
                self.tally.add(&self.tally.stale, 1);
                continue;
            }
            if let Err(e) = agree(
                world,
                &held,
                &Self::tip_writes(&spec_files, &seen),
                &o,
                &after,
                self.tally,
                true,
            ) {
                self.fail(
                    &format!("P11: after {label} and the grace step, the folder does not win"),
                    &seen,
                    format!("{e}\n  oracle {o:?}\n  design {after:?}"),
                );
            }
        }
    }
}

/// A faulted folder: what happened, and the files each design then holds.
type Fault = (String, Vec<File<Contribution>>, Vec<File<oracle::Folds>>);

/// Every segment copied under another name, as a sync client's conflicted copy.
fn copy<S: Clone>(files: &mut Vec<File<S>>) {
    let n = files.len();
    for i in 0..n {
        if matches!(files[i].body, Body::Segment(_)) && !files[i].deleted {
            let mut c = files[i].clone();
            c.id = files.len();
            files.push(c);
        }
    }
}

/// The folder `now` with the directories `within` as they were in `old`.
fn restore<S: Clone>(now: &[File<S>], old: &[File<S>], within: &BTreeSet<Dir>) -> Vec<File<S>> {
    now.iter()
        .map(|f| {
            if !within.contains(&f.dir) {
                f.clone()
            } else if let Some(o) = old.get(f.id) {
                o.clone()
            } else {
                File {
                    deleted: true,
                    ..f.clone()
                }
            }
        })
        .collect()
}

/// P4: a step by a branch of a forked writer changes no merged value.
fn branch_step(parent: &History, child: &History) -> bool {
    let Some(super::history::Event::Write { inst, .. } | super::history::Event::Compact { inst }) =
        child.events.last().copied()
    else {
        return false;
    };
    let dir = child.insts[inst].dir;
    let mut by_prev: BTreeMap<Hash, usize> = BTreeMap::new();
    for e in parent.world.entries.iter().filter(|e| e.dir == dir) {
        if let Some(p) = e.prev {
            *by_prev.entry(p).or_default() += 1;
        }
    }
    by_prev.values().any(|&n| n > 1)
}

/// P4 for one step: the merged values are unchanged, except on fields shown as
/// a forgotten basis before or after it.
fn branch_kept(before: &Shown, after: &Shown, tally: &Tally) -> bool {
    if merged(before) == merged(after) {
        return true;
    }
    let items = before.items.iter().chain(&after.items);
    let forgotten: BTreeSet<Field> = items.flat_map(|i| i.forgotten.iter().copied()).collect();
    let fields: BTreeSet<Field> = before
        .values
        .keys()
        .chain(after.values.keys())
        .copied()
        .collect();
    let existence =
        (before.exists, before.deletion_conflict) == (after.exists, after.deletion_conflict);
    let kept = (existence || forgotten.contains(&Field::Exist))
        && fields
            .iter()
            .all(|f| forgotten.contains(f) || before.values.get(f) == after.values.get(f));
    if kept {
        tally.add(&tally.collapsed, 1);
    }
    kept
}

/// Every history within `bounds`, each checked at `depth`.
pub fn sweep(bounds: Bounds, depth: Depth, design: Spec) -> Tally {
    let tally = Tally::default();
    let root = History::seeded();
    let first = root.children(&bounds);
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        while let Some(h) = first.get(next.fetch_add(1, Ordering::Relaxed)) {
                            let ancestors = vec![root.clone()];
                            let shown = Checker::new(h, &ancestors, &tally, depth, design).check();
                            let mut ancestors = ancestors;
                            explore(h, &shown, &mut ancestors, &bounds, depth, design, &tally);
                        }
                    }));
                    if let Err(payload) = work {
                        tally.failed.store(true, Ordering::Relaxed);
                        std::panic::resume_unwind(payload);
                    }
                })
            })
            .collect();
        for handle in handles {
            if let Err(payload) = handle.join() {
                std::panic::resume_unwind(payload);
            }
        }
    });
    tally
}

fn explore(
    h: &History,
    shown: &Shown,
    ancestors: &mut Vec<History>,
    bounds: &Bounds,
    depth: Depth,
    design: Spec,
    tally: &Tally,
) {
    if h.written() >= bounds.entries || tally.failed.load(Ordering::Relaxed) {
        return;
    }
    ancestors.push(h.clone());
    for child in h.children(bounds) {
        let child_shown = Checker::new(&child, ancestors, tally, depth, design).check();
        if branch_step(h, &child) && !branch_kept(shown, &child_shown, tally) {
            panic!(
                "P4: a step by a branch of a forked writer changes merged values\n  before {shown:?}\n  after {child_shown:?}\n  history: {:?}",
                child.acts
            );
        }
        explore(
            &child,
            &child_shown,
            ancestors,
            bounds,
            depth,
            design,
            tally,
        );
    }
    ancestors.pop();
}
