//! The properties, and the enumeration that drives them.
//!
//! P3, P5, P6, P8, P10, P12: at rest, readers of the design show what the oracle
//! shows. P1: along a reader's deliveries, a value a merged write replaced never
//! shows alone again. P4: a step by one branch of a forked writer changes no
//! merged value. P7: no reader merges what a dismissal it holds names. P9: a
//! contribution's size is bounded by fields and writers, and its certificates
//! name its head. P11: after a restore or
//! a dropped version, the folder wins. Five divergences are counted rather than
//! failed: a conflict the oracle resolves, after a fork; an extra value beside
//! the oracle's after a fault, on a field the design reports contradicted; a
//! value the oracle shows after a fault that a held line's chain replaced,
//! through an entry the fault took; a merged value the design shows only in an
//! item, as a forgotten basis, because a fork lies inside a segment, or because a
//! fault left a file it cannot place; and a faulted reading that shows such
//! unplaced lines.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::history::{materialize, writer_view, Bounds, Folder, History, ViewOf};
use super::oracle::{self, Oracle};
use super::world::{
    self, Body, Dir, Existence, Field, File, Hash, ItemKind, Seen, Shown, Val, World,
};
use super::{Bound, Design, Reading};

/// What a sweep counted.
#[derive(Default, Debug)]
pub struct Tally {
    pub histories: AtomicUsize,
    pub reads: AtomicUsize,
    /// Fields where the design shows a conflict the oracle does not.
    pub conflicts: AtomicUsize,
    /// Fields where the oracle merges a value the design shows only in a run: a
    /// forgotten basis, or a fork point it cannot locate exactly.
    pub collapsed: AtomicUsize,
    /// Fields that show, after a fault, a write a reference to a lost write no
    /// longer covers.
    pub contradicted: AtomicUsize,
    /// Fields after a fault where the oracle shows a value that a held line's
    /// chain replaced, through an entry the fault took.
    pub chained: AtomicUsize,
    /// Faulted readings that show lines of a file shorter than a certificate,
    /// which the reader cannot place.
    pub unplaced: AtomicUsize,
    /// Set when a worker finds a violation, so the others stop.
    pub failed: AtomicBool,
}

impl Tally {
    fn add(&self, counter: &AtomicUsize, n: usize) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub fn report(&self) -> String {
        let n = |c: &AtomicUsize| c.load(Ordering::Relaxed);
        format!(
            "{} histories, {} reads; {} fields shown as a conflict the oracle resolves, {} fields whose merged value shows only in an item, {} fields after a fault where a reference to a lost write no longer covers an earlier one, {} fields after a fault whose oracle value a held line's chain replaced, {} faulted readings with unplaced lines",
            n(&self.histories),
            n(&self.reads),
            n(&self.conflicts),
            n(&self.collapsed),
            n(&self.contradicted),
            n(&self.chained),
            n(&self.unplaced),
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

fn read<D: Design>(d: &D, world: &World, files: &[File<D::Snapshot>], seen: &[Seen]) -> Shown {
    d.read(&Reading { world, files, seen })
}

/// Compares what the design shows with what the oracle shows, counting the
/// documented divergences: a value missing where the design shows it in a run;
/// and an extra value beside the oracle's, after a fork, or after a fault on a
/// field the design reports contradicted, or on a field it shows as a
/// forgotten basis.
fn agree(
    world: &World,
    held: &BTreeSet<Hash>,
    tips: &BTreeSet<Hash>,
    oracle: &Shown,
    spec: &Shown,
    tally: &Tally,
    faulted: Option<&Shown>,
) -> Result<(), String> {
    if oracle == spec {
        return Ok(());
    }
    let with_existence = |shown: &Shown| {
        let mut values = shown.values.clone();
        for (field, writes) in shown.writes.iter().filter(|(f, _)| f.is_exist()) {
            let exist = writes
                .iter()
                .filter_map(|&h| world.entry(h).write.and_then(|w| w.value));
            values.insert(*field, exist.collect());
        }
        values
    };
    let (ov, sv) = (with_existence(oracle), with_existence(spec));
    let chained = faulted.map(with_existence);
    let forks = oracle::place(world, held).forks;
    let forked = !forks.is_empty();
    let fields: BTreeSet<Field> = ov.keys().chain(sv.keys()).copied().collect();
    let mut divergent: BTreeSet<Field> = BTreeSet::new();
    for field in fields {
        let o = ov.get(&field).cloned().unwrap_or_default();
        let s = sv.get(&field).cloned().unwrap_or_default();
        if o == s {
            continue;
        }
        divergent.insert(field);
        let forced = spec.items.iter().any(|i| i.forgotten.contains(&field));
        let oracle_writes = |v: &Val| -> Vec<Hash> {
            let writes = oracle.writes.get(&field).into_iter().flatten().copied();
            writes
                .filter(|&e| world.entry(e).write.is_some_and(|w| w.value == Some(*v)))
                .collect()
        };
        // A value the design shows only in an item of its writer: as the run's
        // value, where the fork lies inside a segment or a fault left lines it
        // cannot place; or as a forgotten basis, which no tip it reads holds.
        let in_run = |v: &Val| {
            let writes = oracle_writes(v);
            let writers: BTreeSet<Dir> = writes.iter().map(|&e| world.entry(e).dir).collect();
            let unheld = writes.iter().all(|e| !tips.contains(e));
            spec.items
                .iter()
                .filter(|i| writers.contains(&i.dir))
                .any(|i| {
                    i.change.get(&field) == Some(&Some(*v))
                        || (unheld && i.forgotten.contains(&field))
                })
        };
        // After a fault, a value a held line's chain replaced, though the
        // entry that replaced it is gone.
        let replaced = |v: &Val| {
            let chained = chained.as_ref().map(|c| c.get(&field));
            chained.is_some_and(|c| c.is_none_or(|values| !values.contains(v)))
        };
        let missing: Vec<&Val> = o.difference(&s).filter(|v| !in_run(v)).collect();
        if let Some(v) = missing.iter().find(|v| !replaced(v)) {
            return Err(format!(
                "{field:?}: oracle {o:?}, design {s:?}; {v:?} is shown nowhere"
            ));
        }
        if !missing.is_empty() {
            tally.add(&tally.chained, 1);
        }
        if !s.is_subset(&o) {
            // The oracle's value still shows: its live writes, or, where the
            // oracle deleted the entity, the delete in a deletion conflict.
            let beside = if field.is_exist() || oracle.exists(field.entity) {
                let ow = oracle.writes.get(&field).cloned().unwrap_or_default();
                let sw = spec.writes.get(&field).cloned().unwrap_or_default();
                !ow.is_empty() && ow.is_subset(&sw)
            } else {
                spec.entities
                    .get(&field.entity)
                    .is_some_and(|e| e.deletion_conflict)
            };
            let explained = forked || (faulted.is_some() && spec.contradicted.contains(&field));
            if !forced && !(beside && explained) {
                return Err(format!(
                    "{field:?}: oracle {o:?}, design {s:?}; an extra value with no conflict"
                ));
            }
            let counter = if forced {
                &tally.collapsed
            } else if forked {
                &tally.conflicts
            } else {
                &tally.contradicted
            };
            tally.add(counter, 1);
        }
        if !o.is_subset(&s) {
            tally.add(&tally.collapsed, 1);
        }
    }
    let unplaced = spec
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Unplaced)
        .count();
    if unplaced > 0 {
        if faulted.is_none() {
            return Err("lines shown unplaced without a fault".into());
        }
        tally.add(&tally.unplaced, 1);
    }
    // Items are compared on every field whose merged values agree, and on the
    // existence of every entity shown alike.
    let entities: BTreeSet<u8> = oracle
        .entities
        .keys()
        .chain(spec.entities.keys())
        .copied()
        .collect();
    for entity in entities {
        if oracle.entities.get(&entity) != spec.entities.get(&entity) {
            divergent.insert(Field::exist(entity));
        }
    }
    let undivergent = |views: Vec<(Dir, ItemKind, BTreeMap<Field, Option<Val>>)>| {
        let mut kept: Vec<_> = views
            .into_iter()
            .filter(|v| v.1 != ItemKind::Unplaced)
            .filter_map(|(dir, kind, mut change)| {
                change.retain(|field, _| !divergent.contains(field));
                (!change.is_empty()).then_some((dir, kind, change))
            })
            .collect();
        kept.sort();
        kept
    };
    let views = |shown: &Shown| undivergent(shown.item_views());
    if views(oracle) == views(spec) {
        return Ok(());
    }
    let unforced = spec.items.iter().map(|i| {
        let mut change = i.change.clone();
        change.retain(|&field, &mut value| {
            !i.forgotten.contains(&field) || world::differs(spec, field, value)
        });
        (i.dir, i.kind, change)
    });
    let unforced = undivergent(unforced.collect());
    if unforced != views(oracle) {
        return Err(format!(
            "items differ: oracle {:?}, design {unforced:?}",
            views(oracle)
        ));
    }
    tally.add(&tally.collapsed, 1);
    Ok(())
}

/// The values a reader merged: everything but its items.
fn merged(s: &Shown) -> (&BTreeMap<u8, Existence>, &BTreeMap<Field, BTreeSet<Val>>) {
    (&s.entities, &s.values)
}

/// P1 along one sequence of states, each with the entries the dismissals it
/// holds name: a write whose value gave way to a write that replaced it, by
/// reference or as a later write of its writer, never shows alone
/// again on its field without an item or a contradiction report touching it,
/// unless every write that replaced it was dismissed or lies beyond a fork.
fn no_silent_return(world: &World, states: &[(Shown, BTreeSet<Hash>)]) -> Result<(), String> {
    let replaces = |newer: Hash, w: Hash| {
        let (n, o) = (world.entry(newer), world.entry(w));
        n.replaces.iter().chain(&n.observed).any(|r| r.hash == w) || (n.dir == o.dir && n.n > o.n)
    };
    let mut replaced: BTreeMap<(Field, Hash), BTreeSet<Hash>> = BTreeMap::new();
    for pair in states.windows(2) {
        let ((before, _), (after, dismissed)) = (&pair[0], &pair[1]);
        for (field, writes) in &before.writes {
            let now = after.writes.get(field).cloned().unwrap_or_default();
            let then = before.values.get(field).cloned().unwrap_or_default();
            let values = after.values.get(field).cloned().unwrap_or_default();
            for w in writes {
                if !now.contains(w) && now.iter().any(|&n| replaces(n, *w)) && values != then {
                    replaced.entry((*field, *w)).or_default().extend(&now);
                }
            }
        }
        for (field, writes) in &after.writes {
            let touched = after.items.iter().any(|i| i.change.contains_key(field))
                || after.contradicted.contains(field);
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

pub struct Checker<'a, D: Design> {
    pub history: &'a History,
    /// Earlier states of this history, oldest first, for restores.
    pub ancestors: &'a [History],
    pub oracle: Folder<oracle::Folds>,
    pub spec: Folder<D::Snapshot>,
    pub tally: &'a Tally,
    pub depth: Depth,
    pub design: D,
}

impl<'a, D: Design> Checker<'a, D> {
    pub fn new(
        history: &'a History,
        ancestors: &'a [History],
        tally: &'a Tally,
        depth: Depth,
        design: D,
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
                    oracle::describe(
                        self.history
                            .world
                            .entry(self.design.head(&self.history.world, c))
                    )
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

    /// The oracle's and the design's fresh reads of `seen`.
    fn fresh(&self, seen: &[Seen]) -> (Shown, Shown, BTreeSet<Hash>) {
        self.tally.add(&self.tally.reads, 1);
        let r = Reading {
            world: self.world(),
            files: &self.oracle.files,
            seen,
        };
        let held = Oracle.holds(&r);
        let o = Oracle.read(&r);
        let s = read(&self.design, self.world(), &self.spec.files, seen);
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
                &self
                    .design
                    .tip_writes(self.world(), &self.spec.files, &seen),
                &o,
                &s,
                self.tally,
                None,
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
        self.deliveries();
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
        let o = Oracle.read(&ro);
        let s = read(&self.design, self.world(), &spec_files, &seen);
        if let Err(e) = agree(
            self.world(),
            &Oracle.holds(&ro),
            &self.design.tip_writes(self.world(), &spec_files, &seen),
            &o,
            &s,
            self.tally,
            None,
        ) {
            self.fail("a conflicted copy changes what a reader shows", &seen, e);
        }
    }

    /// P9.
    fn sizes(&self) {
        let fields = self
            .history
            .world
            .entries
            .iter()
            .filter_map(|e| e.write.map(|w| w.field))
            .collect::<BTreeSet<_>>()
            .len();
        let writers = 1 + self
            .history
            .insts
            .iter()
            .map(|i| i.dir as usize)
            .max()
            .unwrap_or(0);
        let drops = self
            .history
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    super::history::Event::Compact {
                        drop_last: true,
                        ..
                    }
                )
            })
            .count();
        for f in &self.spec.files {
            let Body::Snapshot(c) = &f.body else { continue };
            let segments = self
                .spec
                .files
                .iter()
                .filter(|g| g.dir == f.dir && matches!(g.body, Body::Segment(_)))
                .count();
            let bound = Bound {
                fields,
                writers,
                segments,
                drops,
            };
            if !self.design.names_its_head(c) {
                panic!(
                    "P9: snapshot {} does not name its head: {c:?}\n  history: {:?}",
                    f.id, self.history.acts
                );
            }
            if !self.design.bounded(c, bound) {
                panic!(
                    "P9: snapshot {} is larger than its bound: {c:?}\n  history: {:?}",
                    f.id, self.history.acts
                );
            }
        }
    }

    /// The entries a reader of `seen` may stop merging without a replaced
    /// value's return being silent: what its dismissals name, and what lies
    /// beyond a fork point the files show.
    fn excused(&self, seen: &[Seen]) -> BTreeSet<Hash> {
        let r = Reading {
            world: self.world(),
            files: &self.oracle.files,
            seen,
        };
        let placement = oracle::place(self.world(), &Oracle.holds(&r));
        let mut excused = self.dismissed(seen);
        excused.extend(placement.runs);
        excused
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
                Body::Snapshot(c) => dismissals.extend(self.design.dismissals(world, c)),
            }
        }
        dismissals
            .iter()
            .flat_map(|d| oracle::covered(world, d))
            .collect()
    }

    /// P1: along every delivery order, a reader never shows a replaced value
    /// alone again.
    fn deliveries(&self) {
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
                let mut seen: Vec<Seen> = if matches!(mode, 1 | 2) {
                    gone.clone()
                } else {
                    Vec::new()
                };
                let mut states = Vec::new();
                let mut look = |seen: &[Seen]| {
                    let s = read(&self.design, self.world(), files, seen);
                    states.push((s, self.excused(seen)));
                };
                if matches!(mode, 1 | 2) {
                    look(&seen);
                }
                if mode == 2 {
                    seen.clear();
                    look(&seen);
                }
                for &i in &order {
                    seen.push(kept[i]);
                    look(&seen);
                }
                if late {
                    for g in &gone {
                        seen.retain(|s| s != g);
                        look(&seen);
                    }
                }
                self.tally.add(&self.tally.reads, states.len());
                if let Err(e) = no_silent_return(self.world(), &states) {
                    let trail: Vec<String> = states
                        .iter()
                        .map(|(s, _)| format!("{:?} {:?}", s.writes, s.item_views()))
                        .collect();
                    self.fail(
                        "P1: a reader shows a replaced value alone",
                        &seen,
                        format!("{e} (mode {mode}, order {order:?})\n  states: {trail:#?}"),
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
            let placement = oracle::place(self.world(), &held);
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
    /// version, a reader of the new folder shows what the oracle shows of it.
    fn faults(&self) {
        let world = self.world();
        let mut variants: Vec<Fault<D::Snapshot>> = Vec::new();
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
            let after = read(&self.design, world, &spec_files, &seen);
            let ro = Reading {
                world,
                files: &oracle_files,
                seen: &seen,
            };
            let held = Oracle.holds(&ro);
            let o = Oracle.read(&ro);
            // A line implies the chain it continues: what an entry the fault
            // took replaced stays replaced, though it shows nothing itself.
            let implied: BTreeSet<Hash> = held.iter().flat_map(|&e| world.chain(e)).collect();
            let chained = oracle::shown_within(world, &oracle::place(world, &implied), Some(&held));
            if let Err(e) = agree(
                world,
                &held,
                &self.design.tip_writes(self.world(), &spec_files, &seen),
                &o,
                &after,
                self.tally,
                Some(&chained),
            ) {
                self.fail(
                    &format!("P11: after {label}, the folder does not win"),
                    &seen,
                    format!("{e}\n  oracle {o:?}\n  design {after:?}"),
                );
            }
        }
    }
}

/// A faulted folder: what happened, and the files each design then holds.
type Fault<S> = (String, Vec<File<S>>, Vec<File<oracle::Folds>>);

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
    let Some(
        super::history::Event::Write { inst, .. } | super::history::Event::Compact { inst, .. },
    ) = child.events.last().copied()
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
    let entities: BTreeSet<u8> = before
        .entities
        .keys()
        .chain(after.entities.keys())
        .copied()
        .collect();
    let kept = entities.iter().all(|e| {
        forgotten.contains(&Field::exist(*e)) || before.entities.get(e) == after.entities.get(e)
    }) && fields
        .iter()
        .all(|f| forgotten.contains(f) || before.values.get(f) == after.values.get(f));
    if kept {
        tally.add(&tally.collapsed, 1);
    }
    kept
}

/// Every history within `bounds`, each checked at `depth`.
pub fn sweep<D: Design + Copy + Sync>(bounds: Bounds, depth: Depth, design: D) -> Tally {
    let tally = Tally::default();
    let root = History::seeded();
    let view = |h: &History, i: usize| writer_view(h, i, &design);
    let run = Run {
        bounds,
        depth,
        design,
        tally: &tally,
        view: &view,
    };
    let first = root.children(&bounds, &view);
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        while let Some(h) = first.get(next.fetch_add(1, Ordering::Relaxed)) {
                            let mut ancestors = vec![root.clone()];
                            let shown = Checker::new(h, &ancestors, &tally, depth, design).check();
                            run.explore(h, &shown, &mut ancestors);
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

/// One sweep's settings.
struct Run<'a, D> {
    bounds: Bounds,
    depth: Depth,
    design: D,
    tally: &'a Tally,
    view: &'a ViewOf<'a>,
}

impl<D: Design + Copy> Run<'_, D> {
    fn explore(&self, h: &History, shown: &Shown, ancestors: &mut Vec<History>) {
        if h.written() >= self.bounds.entries || self.tally.failed.load(Ordering::Relaxed) {
            return;
        }
        ancestors.push(h.clone());
        for child in h.children(&self.bounds, self.view) {
            let child_shown =
                Checker::new(&child, ancestors, self.tally, self.depth, self.design).check();
            if branch_step(h, &child) && !branch_kept(shown, &child_shown, self.tally) {
                panic!(
                    "P4: a step by a branch of a forked writer changes merged values\n  before {shown:?}\n  after {child_shown:?}\n  history: {:?}",
                    child.acts
                );
            }
            self.explore(&child, &child_shown, ancestors);
        }
        ancestors.pop();
    }
}

/// Checks the one history whose acts are `acts`, the seed's first.
pub fn replay<D: Design + Copy + Sync>(bounds: Bounds, depth: Depth, design: D, acts: &[&str]) {
    let tally = Tally::default();
    let view = |h: &History, i: usize| writer_view(h, i, &design);
    let mut h = History::seeded();
    let mut ancestors = Vec::new();
    while h.acts.len() < acts.len() {
        ancestors.push(h.clone());
        let children = h.children(&bounds, &view);
        h = children
            .into_iter()
            .find(|c| c.acts.iter().zip(acts).all(|(a, b)| a == b))
            .unwrap_or_else(|| panic!("no child after {:?}", h.acts));
    }
    Checker::new(&h, &ancestors, &tally, depth, design).check();
}
