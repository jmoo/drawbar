//! The properties `spec/Portable.tla` checks, asserted of the real reader and writer
//! under the sync adversary: bounded-exhaustive over every delivery order of a few
//! scenarios, and seeded over random interleavings of writes and sync.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{fresh, layout, Instance};
use toshokan::blocking::run;
use toshokan::derive::Derived;
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
                WriterFile::Snapshot(_) | WriterFile::Unreadable { .. } => {}
            }
        }
        written
    }

    /// The number of `writer`'s entry `hash` in its chain.
    fn number(&self, writer: WriterId, hash: EntryHash) -> u64 {
        let pairs = &self.0[&writer];
        let mut n = 0;
        let mut at = pairs[&hash];
        while at != EntryHash::ZERO {
            n += 1;
            at = pairs[&at];
        }
        n
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

/// The entries of `writer` a reader holds, by what `written` says precedes
/// what: its lines, and every entry a tip's head descends from.
fn held(derived: &Derived, written: &Written, writer: WriterId) -> BTreeSet<EntryHash> {
    let Some(placed) = derived.writers.get(&writer) else {
        return BTreeSet::new();
    };
    let lines = placed.merged.iter().chain(&placed.runs).chain(&placed.gaps);
    let mut held: BTreeSet<EntryHash> = lines.map(|entry| entry.hash()).collect();
    let pairs = written.0.get(&writer);
    for tip in &placed.tips {
        let mut at = Some(tip.head.hash);
        while let Some(hash) = at.filter(|hash| *hash != EntryHash::ZERO) {
            held.insert(hash);
            at = pairs.and_then(|pairs| pairs.get(&hash)).copied();
        }
    }
    held
}

fn forks(derived: &Derived) -> BTreeSet<(WriterId, EntryHash)> {
    let forks = derived
        .writers
        .iter()
        .flat_map(|(writer, placed)| placed.forks.iter().map(move |fork| (*writer, fork.hash)));
    forks.collect()
}

/// ChainOrder: every merged entry follows its held predecessor back to a tip or
/// a genesis. NothingIgnored: every line the folder shows is held, or waits for
/// the rest of its file.
fn check_state(folder: &Files, written: &Written, derived: &Derived, case: &str) {
    for (writer, placed) in &derived.writers {
        let held = held(derived, written, *writer);
        for entry in &placed.merged {
            let prev = entry.prev();
            assert!(
                prev == EntryHash::ZERO || held.contains(&prev),
                "{case}: {} is not chained",
                entry.hash()
            );
        }
    }
    let shown = Written::of(folder.iter().map(|(path, bytes)| (path, bytes.as_slice())));
    for (writer, pairs) in &shown.0 {
        let held = held(derived, written, *writer);
        let unheld = pairs.keys().filter(|hash| !held.contains(hash)).count();
        let waiting = derived
            .writers
            .get(writer)
            .map_or(0, |placed| placed.waiting);
        assert!(
            unheld <= waiting,
            "{case}: {unheld} entries of {writer} neither held nor waiting"
        );
    }
}

/// Monotone, but for an entry no file in `folder` holds whose number a tip of
/// its writer has passed, which the reader takes as folded into that tip; and
/// ForksKept, and each fork reported when it first appears.
fn check_step(
    folder: &Files,
    written: &Written,
    before: &Derived,
    after: &Derived,
    report: &ReadReport,
    case: &str,
) {
    let shown = Written::of(folder.iter().map(|(path, bytes)| (path, bytes.as_slice())));
    for writer in before.writers.keys() {
        let tip = after
            .writers
            .get(writer)
            .and_then(|placed| placed.tips.iter().map(|tip| tip.head.n).max());
        let lines = shown.0.get(writer);
        let passed = |hash: &EntryHash| {
            let unseen = lines.is_none_or(|lines| !lines.contains_key(hash));
            unseen && tip.is_some_and(|tip| written.number(*writer, *hash) <= tip)
        };
        let lost: Vec<_> = held(before, written, *writer)
            .difference(&held(after, written, *writer))
            .filter(|hash| !passed(hash))
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
fn check_delivered(origin: &Files, written: &Written, derived: &Derived, case: &str) {
    let fresh = read(origin, &CachedView::default());
    for (writer, pairs) in &written.0 {
        let expected: BTreeSet<EntryHash> = pairs.keys().copied().collect();
        assert_eq!(
            held(derived, written, *writer),
            expected,
            "{case}: {writer} not converged"
        );
        assert_eq!(
            held(fresh.derived(), written, *writer),
            expected,
            "{case}: a fresh reader differs"
        );
        let placed = &derived.writers[writer];
        assert!(placed.gaps.is_empty(), "{case}");
        assert_eq!(placed.waiting, 0, "{case}");
    }
    assert!(
        written.forks().is_subset(&forks(derived)),
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

/// A reader with `cached` that read a folder holding `files`.
fn read(files: &Files, cached: &CachedView) -> Reader {
    let mut reader = Reader::new(layout(), cached.clone());
    run(&mut disk_of(files), reader.read()).unwrap();
    reader
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
    assert!(
        reader.derived().folded == cold.derived().folded,
        "{case}: the merged states differ"
    );
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
        let derived = reader.derived().clone();
        if &folder == origin {
            check_delivered(origin, &written, &derived, &case);
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
            let after = reader.derived();
            check_step(&next, &written, &derived, after, &report, &case);
            check_state(&next, &written, after, &case);
            if seen.insert((next.clone(), reader.cached().encode(), spent)) {
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
            held(&now, &written, *writer),
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

// A clone branches from an entry the original, which has not seen the clone's
// segment yet, then folds and deletes.
#[test]
fn every_delivery_order_of_a_compacted_fork_reports_it() {
    let mut sim = Simulator::new(0);
    let mut original = Instance::open(sim.machine(), 1);
    original.write("a1").unwrap();
    sim.observe();
    let mut clone = Instance::open(original.machine.cloned(), 2);
    let folder = original.machine.folder.clone();
    let before = folder.files(Root::Folder);
    clone.write("b2").unwrap();
    sim.observe();
    let segment = folder
        .files(Root::Folder)
        .into_iter()
        .find(|(path, _)| !before.contains_key(path))
        .expect("the clone's segment");
    let (path, bytes) = segment;
    let io = |io: Io| folder.perform(io).map(|_| ()).unwrap();
    io(Io::Remove {
        root: Root::Folder,
        path: path.clone(),
    });
    original.write("a2").unwrap();
    sim.observe();
    original.compact().unwrap();
    io(Io::Create {
        root: Root::Folder,
        path,
        bytes,
    });
    sim.observe();
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
        let folder = disk.files(Root::Folder);
        let before = reader.derived().clone();
        let report = run(&mut disk, reader.read()).unwrap();
        let case = format!("seed {seed}, settled reader {r}");
        check_step(&folder, &written, &before, reader.derived(), &report, &case);
        let origin = sim.observe();
        check_delivered(&origin, &written, reader.derived(), &case);
    }
}

fn step_reader(sim: &mut Simulator, reader: &mut Reader, r: usize, event: &SyncEvent, case: &str) {
    let folder = sim.reader(r).files(Root::Folder);
    let written = Written::of(sim.versions().iter());
    let before = reader.derived().clone();
    let case = format!("{case}, reader {r} after {event:?}");
    let report = read_again(reader, &folder, &case);
    check_step(&folder, &written, &before, reader.derived(), &report, &case);
    check_state(&folder, &written, reader.derived(), &case);
}
