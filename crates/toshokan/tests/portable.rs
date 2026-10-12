//! The properties `spec/Portable.tla` checks, asserted of the real reader and writer
//! under the sync adversary: bounded-exhaustive over every delivery order of a few
//! scenarios, and seeded over random interleavings of writes and sync.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{fresh, layout, Instance};
use toshokan::blocking::run;
use toshokan::env::SeededRandom;
use toshokan::reader::{CachedView, ReadReport, Reader, WriterFile};
use toshokan::simulator::{apply, events, Files, Simulator, SyncEvent, Versions};
use toshokan::{EntryHash, Io, MemDisk, Random, RelPath, Root, WriterId};

/// Every entry each writer has written, with its predecessor, as the folder ever
/// held them.
#[derive(Default)]
struct Written(BTreeMap<WriterId, BTreeMap<EntryHash, EntryHash>>);

impl Written {
    fn of<'a>(files: impl IntoIterator<Item = (&'a RelPath, &'a [u8])>) -> Self {
        let mut written = Self::default();
        for (path, bytes) in files {
            let Some(writer) = writer_of(path) else {
                continue;
            };
            let pairs = written.0.entry(writer).or_default();
            match WriterFile::parse(bytes) {
                WriterFile::Segment(segment) => {
                    let lines = segment.entries.iter();
                    pairs.extend(lines.map(|entry| (entry.hash(), entry.prev())));
                }
                WriterFile::Snapshot(snapshot) => pairs.extend(snapshot.pairs()),
                WriterFile::Unreadable => {}
            }
        }
        written
    }

    /// The predecessors with two or more successors, per writer.
    fn forks(&self) -> BTreeSet<(WriterId, EntryHash)> {
        let mut after: BTreeMap<(WriterId, EntryHash), usize> = BTreeMap::new();
        for (writer, pairs) in &self.0 {
            for prev in pairs.values() {
                *after.entry((*writer, *prev)).or_default() += 1;
            }
        }
        after
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(fork, _)| fork)
            .collect()
    }
}

fn writer_of(path: &RelPath) -> Option<WriterId> {
    let writers = layout().writers();
    let dir = path.parent()?;
    (dir.parent()? == writers).then(|| dir.name()?.parse().ok())?
}

fn held(view: &CachedView, writer: WriterId) -> BTreeSet<EntryHash> {
    view.writers()
        .get(&writer)
        .map_or_else(BTreeSet::new, |log| {
            let mut held: BTreeSet<EntryHash> = log.entries().iter().map(|e| e.hash()).collect();
            held.extend(
                log.snapshots()
                    .iter()
                    .flat_map(|s| s.folded.iter().copied()),
            );
            held
        })
}

fn forks(view: &CachedView) -> BTreeSet<(WriterId, EntryHash)> {
    view.writers()
        .values()
        .flat_map(|log| log.forks().iter().map(|fork| (fork.writer, fork.prev)))
        .collect()
}

/// ChainOrder: every held entry follows its held predecessor back to a genesis.
/// NothingIgnored: every line the folder shows is held or counted as a gap.
fn check_state(folder: &Files, view: &CachedView, case: &str) {
    for log in view.writers().values() {
        for head in log.heads() {
            assert!(
                log.chain_to(head).is_some(),
                "{case}: {head} is not chained"
            );
        }
    }
    let shown = Written::of(folder.iter().map(|(path, bytes)| (path, bytes.as_slice())));
    for (writer, pairs) in &shown.0 {
        let held = held(view, *writer);
        let waiting = pairs.keys().filter(|hash| !held.contains(hash)).count();
        let gaps: usize = view.writers()[writer]
            .gaps()
            .iter()
            .map(|gap| gap.held)
            .sum();
        assert_eq!(
            waiting, gaps,
            "{case}: entries of {writer} neither held nor reported"
        );
    }
}

/// Monotone and ForksKept, and each fork reported when it first appears.
fn check_step(before: &CachedView, after: &CachedView, report: &ReadReport, case: &str) {
    for writer in before.writers().keys() {
        let lost: Vec<_> = held(before, *writer)
            .difference(&held(after, *writer))
            .copied()
            .collect();
        assert!(lost.is_empty(), "{case}: {writer} lost {lost:?}");
    }
    let (old, new) = (forks(before), forks(after));
    assert!(old.is_subset(&new), "{case}: a fork was retracted");
    for fork in &report.forks {
        assert!(
            !old.contains(&(fork.writer, fork.prev)),
            "{case}: {fork:?} reported twice"
        );
    }
    let found: BTreeSet<_> = report
        .forks
        .iter()
        .map(|fork| (fork.writer, fork.prev))
        .collect();
    assert_eq!(
        found,
        &new - &old,
        "{case}: forks found and reported differ"
    );
}

/// DeliveredConverges: once a reader's folder equals the origin's, it holds every
/// entry written, holds none back, reports every fork, and a reader without a
/// cached view holds the same.
fn check_delivered(origin: &Files, written: &Written, view: &CachedView, case: &str) {
    let fresh_view = read(origin, &CachedView::default()).0;
    for (writer, pairs) in &written.0 {
        let expected: BTreeSet<EntryHash> = pairs.keys().copied().collect();
        assert_eq!(
            held(view, *writer),
            expected,
            "{case}: {writer} not converged"
        );
        assert_eq!(
            held(&fresh_view, *writer),
            expected,
            "{case}: a fresh reader differs"
        );
        assert_eq!(view.writers()[writer].gaps(), [], "{case}");
    }
    assert!(
        written.forks().is_subset(&forks(view)),
        "{case}: a fork is unreported"
    );
}

fn disk_of(files: &Files) -> MemDisk {
    let disk = MemDisk::new();
    for (path, bytes) in files {
        let dir = path.parent().unwrap();
        disk.perform(Io::MakeDir {
            root: Root::Folder,
            path: dir,
        })
        .unwrap();
        disk.perform(Io::Create {
            root: Root::Folder,
            path: path.clone(),
            bytes: bytes.clone(),
        })
        .unwrap();
    }
    disk
}

/// What a reader with `cached` places from a folder holding `files`.
fn read(files: &Files, cached: &CachedView) -> (CachedView, ReadReport) {
    let mut reader = Reader::new(layout(), cached.clone());
    let report = run(&mut disk_of(files), reader.read()).unwrap();
    (reader.cached().clone(), report)
}

/// A reader that read the folder before reads it again: it reads only the files
/// that changed, and places what a reader with its cached view placing every file
/// does.
fn read_again(reader: &mut Reader, files: &Files, case: &str) -> ReadReport {
    let mut cold = Reader::new(layout(), reader.cached().clone());
    let report = run(&mut disk_of(files), reader.read()).unwrap();
    let cold_report = run(&mut disk_of(files), cold.read()).unwrap();
    assert_eq!(
        reader.cached(),
        cold.cached(),
        "{case}: the cached views differ"
    );
    assert_eq!(reader.removed(), cold.removed(), "{case}");
    let placed = |report: &ReadReport| -> BTreeSet<EntryHash> {
        report.placed.values().flatten().copied().collect()
    };
    assert_eq!(placed(&report), placed(&cold_report), "{case}");
    assert_eq!(report.forks, cold_report.forks, "{case}");
    assert_eq!(report.gaps, cold_report.gaps, "{case}");
    assert_eq!(report.unreadable, cold_report.unreadable, "{case}");
    report
}

/// Every state one reader reaches from an empty folder by any sequence of sync
/// events spending at most `chaos` of the spec's chaos budget, each state checked.
fn explore(origin: &Files, versions: &Versions, chaos: u32) {
    let written = Written::of(versions.iter());
    let mut delivered = 0;
    let mut seen = BTreeSet::new();
    let start = Reader::new(layout(), CachedView::default());
    let mut stack = vec![(Files::new(), start, 0)];
    while let Some((folder, reader, spent)) = stack.pop() {
        let case = format!("{} files, chaos {spent}", folder.len());
        let view = reader.cached().clone();
        if &folder == origin {
            check_delivered(origin, &written, &view, &case);
            delivered += 1;
        }
        for event in events(origin, versions, &folder) {
            let spent = spent + u32::from(event.is_chaos(versions, &folder));
            if spent > chaos {
                continue;
            }
            let mut next = folder.clone();
            apply(origin, versions, &mut next, &event);
            let case = format!("{case}, then {event:?}");
            let mut reader = reader.clone();
            let report = read_again(&mut reader, &next, &case);
            let after = reader.cached();
            check_step(&view, after, &report, &case);
            check_state(&next, after, &case);
            if seen.insert((next.clone(), after.encode(), spent)) {
                stack.push((next, reader, spent));
            }
        }
    }
    assert!(
        delivered > 1,
        "{delivered} delivered states of {}",
        seen.len()
    );
}

/// Every entry the origin holds now is every entry ever written: nothing a
/// writer deleted was not folded first.
fn check_retained(sim: &mut Simulator) {
    let origin: Vec<RelPath> = sim.observe().into_keys().collect();
    let written = Written::of(sim.versions().iter());
    let now = fresh(sim.origin());
    for (writer, pairs) in &written.0 {
        let expected: BTreeSet<EntryHash> = pairs.keys().copied().collect();
        assert_eq!(
            held(&now, *writer),
            expected,
            "retained of {writer}: {origin:?}"
        );
    }
}

#[test]
fn every_delivery_order_after_crashes_and_compaction_converges() {
    let mut sim = Simulator::new(0);
    let mut first = Instance::open(sim.machine(), 1);
    first.write("a1").unwrap();
    sim.observe();
    let mut second = Instance::open(first.crash(), 2);
    second.write("a2").unwrap();
    sim.observe();
    second.compact().unwrap();
    sim.observe();
    second.write("a3").unwrap();
    check_retained(&mut sim);
    let origin = sim.observe();
    explore(&origin, sim.versions(), 1);
}

// The spec's configs that fail without folded hash lists and self-sealed deletion:
// a clone branches from an entry the original then folds and deletes.
#[test]
fn every_delivery_order_of_a_compacted_fork_reports_it() {
    let mut sim = Simulator::new(0);
    let mut original = Instance::open(sim.machine(), 1);
    original.write("a1").unwrap();
    sim.observe();
    let mut clone = Instance::open(original.machine.cloned(), 2);
    clone.write("b2").unwrap();
    sim.observe();
    original.write("a2").unwrap();
    sim.observe();
    original.compact().unwrap();
    check_retained(&mut sim);
    let origin = sim.observe();
    assert_eq!(Written::of(sim.versions().iter()).forks().len(), 1);
    explore(&origin, sim.versions(), 1);
}

/// One step of a seeded run.
fn random(random: &mut SeededRandom, n: usize) -> usize {
    (random.next_u128() % n as u128) as usize
}

#[test]
fn seeded_interleavings_of_writers_and_sync_converge() {
    for seed in 0..40 {
        run_seeded(seed);
    }
}

fn run_seeded(seed: u64) {
    let mut chance = SeededRandom::new(seed);
    let mut sim = Simulator::new(2);
    let mut instances = vec![Instance::open(sim.machine(), seed * 100)];
    let mut readers: Vec<Reader> = (0..2)
        .map(|_| Reader::new(layout(), CachedView::default()))
        .collect();
    let mut opened = 1;
    let mut chaos = 0;
    for step in 0..80 {
        let case = format!("seed {seed}, step {step}");
        match random(&mut chance, 10) {
            0..=2 => {
                let k = random(&mut chance, instances.len());
                if instances[k].write(&case).is_err() {
                    instances[k].writer = None;
                }
            }
            3 => {
                let k = random(&mut chance, instances.len());
                if instances[k].writer.is_some() {
                    let _ = instances[k].compact();
                }
            }
            4 => {
                let k = random(&mut chance, instances.len());
                let action = random(&mut chance, 4);
                let instance = instances.remove(k);
                let machine = match action {
                    0 => instance.crash(),
                    1 => {
                        let mut machine = instance.close();
                        machine.lose_local();
                        machine
                    }
                    2 if instances.len() < 2 => {
                        let copy = instance.machine.cloned();
                        instances.push(instance);
                        copy
                    }
                    _ => instance.close(),
                };
                opened += 1;
                instances.push(Instance::open(machine, seed * 100 + opened));
            }
            _ => {
                let r = random(&mut chance, readers.len());
                let offered = sim.events(r);
                if offered.is_empty() {
                    continue;
                }
                let event = &offered[random(&mut chance, offered.len())];
                let folder = sim.reader(r).files(Root::Folder);
                let costs = event.is_chaos(sim.versions(), &folder);
                if costs && chaos == 3 {
                    continue;
                }
                chaos += u32::from(costs);
                sim.apply(r, event);
                step_reader(&mut sim, &mut readers[r], r, event, &case);
            }
        }
        check_retained(&mut sim);
    }
    let written = Written::of(sim.versions().iter());
    for (r, reader) in readers.iter_mut().enumerate() {
        sim.settle(r);
        let mut disk = sim.reader(r).clone();
        let before = reader.cached().clone();
        let report = run(&mut disk, reader.read()).unwrap();
        let case = format!("seed {seed}, settled reader {r}");
        check_step(&before, reader.cached(), &report, &case);
        let origin = sim.observe();
        check_delivered(&origin, &written, reader.cached(), &case);
    }
}

fn step_reader(sim: &mut Simulator, reader: &mut Reader, r: usize, event: &SyncEvent, case: &str) {
    let folder = sim.reader(r).files(Root::Folder);
    let before = reader.cached().clone();
    let case = format!("{case}, reader {r} after {event:?}");
    let report = read_again(reader, &folder, &case);
    check_step(&before, reader.cached(), &report, &case);
    check_state(&folder, reader.cached(), &case);
}
