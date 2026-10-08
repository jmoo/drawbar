//! Indexed lookups and resumable rescans, against what scanning every entity and
//! every file finds, over seeded histories of two writers, outside changes and
//! rescans dropped partway.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use common::Later;
use toshokan::asynch::{self, Fs, Library};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::io::Capabilities;
use toshokan::simulator::Machine;
use toshokan::view::Conflicted;
use toshokan::{
    EntityId, Env, Expect, FileState, Io, IoResult, Layout, MemDisk, Random, Raw, Register,
    RelPath, Root, Schema, Set, View,
};

const ORIGIN: Register<String> = Register::new("origin");
const RATING: Register<u32> = Register::new("rating");
const TAGS: Set<String> = Set::new("tags");

const ORIGINS: [&str; 3] = ["x", "y", "z"];
const TAG_NAMES: [&str; 4] = ["t0", "t1", "t2", "t3"];
const DIRS: [&str; 5] = ["", "d0", "d1", "d1/e", "d2"];

fn schema() -> Schema {
    Schema::of(&[ORIGIN.key(), RATING.key(), TAGS.key()]).unwrap()
}

fn layout() -> Layout {
    Layout::new(".t").unwrap()
}

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

/// Every library path a run uses.
fn paths() -> Vec<RelPath> {
    let names = ["f0", "f1", "f2"];
    DIRS.iter()
        .flat_map(|dir| {
            names.iter().map(move |name| match dir.is_empty() {
                true => path(name),
                false => path(&format!("{dir}/{name}")),
            })
        })
        .collect()
}

/// A machine whose every request completes on its second poll, so a rescan can
/// be dropped between any two of them.
struct Yielding(Machine);

impl Fs for Yielding {
    fn capabilities(&self, root: Root) -> Capabilities {
        Fs::capabilities(&self.0, root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        Later(false).await;
        Fs::perform(&self.0, io).await
    }
}

fn open(folder: &MemDisk, label: &str, seed: u64) -> Library<Yielding> {
    let machine = Machine {
        folder: folder.process(),
        local: MemDisk::new(),
    };
    let env = Env {
        clock: Box::new(TestClock::at(1_000)),
        random: Box::new(SeededRandom::new(seed)),
        identify: Rc::new(PrefixIdentity { prefix: 1 << 16 }),
        names: Box::new(ExactNames),
        label: label.into(),
    };
    let schema = schema();
    let opened = asynch::Library::open(Yielding(machine), layout(), &schema, env);
    pollster::block_on(opened).unwrap().0
}

/// Polls `future` at most `polls` times; its output if it finished.
fn poll_at_most<F: Future>(future: F, polls: usize) -> Option<F::Output> {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    (0..polls).find_map(|_| match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    })
}

fn pick<T: Clone>(chance: &mut SeededRandom, from: &[T]) -> T {
    from[(chance.next_u128() % from.len() as u128) as usize].clone()
}

fn below(chance: &mut SeededRandom, n: usize) -> usize {
    (chance.next_u128() % n as u128) as usize
}

fn raw<T: serde::Serialize>(value: &T) -> Raw {
    Raw::of(value).unwrap()
}

/// What the lookups of `view` must answer, found by scanning every entity.
fn check(view: &View, case: &str) {
    let folded = view.folded();
    let entities: Vec<EntityId> = view.entities().iter().map(|e| e.id()).collect();
    let holding = |key: &str, set: bool, value: &Raw| -> Vec<EntityId> {
        let holds = |entity: &EntityId| match set {
            true => folded.members(*entity, key).contains(value),
            false => folded
                .register(*entity, key)
                .iter()
                .any(|write| write.value == *value),
        };
        entities.iter().copied().filter(holds).collect()
    };
    for origin in ORIGINS {
        let expected = holding("origin", false, &raw(&origin));
        assert_eq!(
            view.find(ORIGIN, &origin.into()),
            expected,
            "{case}: find {origin}"
        );
    }
    for tag in TAG_NAMES {
        let expected = holding("tags", true, &raw(&tag));
        assert_eq!(view.find(TAGS, &tag.into()), expected, "{case}: find {tag}");
    }
    let with = |key: &str, set: bool| -> Vec<EntityId> {
        let holds = |entity: &EntityId| match set {
            true => !folded.members(*entity, key).is_empty(),
            false => !folded.register(*entity, key).is_empty(),
        };
        entities.iter().copied().filter(holds).collect()
    };
    assert_eq!(
        view.with(ORIGIN),
        with("origin", false),
        "{case}: with origin"
    );
    assert_eq!(view.with(TAGS), with("tags", true), "{case}: with tags");
    assert_eq!(
        view.with(RATING),
        with("rating", false),
        "{case}: with rating"
    );

    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for entity in &entities {
        for member in folded.members(*entity, "tags") {
            *tags.entry(member.decode().unwrap()).or_default() += 1;
        }
    }
    let counted = view.values(TAGS);
    assert_eq!(
        counted.values,
        tags.into_iter().collect::<Vec<_>>(),
        "{case}: tag counts"
    );
    assert!(counted.unreadable.is_empty(), "{case}");

    let ratings = |entity: &EntityId| -> Vec<u32> {
        let writes = folded.register(*entity, "rating");
        writes.iter().map(|w| w.value.decode().unwrap()).collect()
    };
    for (low, high) in [(0, 5), (1, 3), (2, 2), (3, 9)] {
        let expected: Vec<EntityId> = entities
            .iter()
            .copied()
            .filter(|entity| ratings(entity).iter().any(|r| (low..high).contains(r)))
            .collect();
        assert_eq!(
            view.range(RATING, low..high),
            expected,
            "{case}: range {low}..{high}"
        );
    }

    let files: Vec<(EntityId, RelPath, FileState)> = entities
        .iter()
        .filter_map(|id| {
            let file = view.entity(*id)?.file()?;
            Some((*id, file.path, file.state))
        })
        .collect();
    for id in &entities {
        let logged = folded.file(*id);
        let file = view.entity(*id).unwrap().file();
        assert_eq!(logged.is_empty(), file.is_none(), "{case}: {id} has a file");
        if let Some(file) = file.filter(|file| file.state == FileState::Missing) {
            let named = logged.iter().any(|fact| fact.value.path == file.path);
            assert!(named, "{case}: {id} is missing at a path no fact names");
        }
    }
    let mut wanted: BTreeSet<RelPath> = paths().into_iter().collect();
    wanted.extend(files.iter().map(|(_, at, _)| at.clone()));
    for at in &wanted {
        let bound: Vec<EntityId> = files
            .iter()
            .filter(|(_, file, state)| {
                file == at && matches!(state, FileState::InSync | FileState::ChangedOutside)
            })
            .map(|(id, _, _)| *id)
            .collect();
        assert!(bound.len() <= 1, "{case}: {at} bound to {bound:?}");
        assert_eq!(
            view.at(at).map(|e| e.id()),
            bound.first().copied(),
            "{case}: at {at}"
        );
    }
    for dir in DIRS.map(path) {
        let mut expected: Vec<(RelPath, EntityId)> = files
            .iter()
            .filter(|(_, file, _)| file.starts_with(&dir))
            .map(|(id, file, _)| (file.clone(), *id))
            .collect();
        expected.sort();
        let under: Vec<(RelPath, EntityId)> = view
            .under(&dir)
            .iter()
            .map(|e| (e.file().unwrap().path, e.id()))
            .collect();
        assert_eq!(under, expected, "{case}: under {dir}");
    }
    assert_eq!(
        view.conflicts(),
        scanned_conflicts(view),
        "{case}: conflicts"
    );
}

/// Every conflict, found by looking at every key of every entity.
fn scanned_conflicts(view: &View) -> Vec<Conflicted> {
    let folded = view.folded();
    let distinct = |values: Vec<String>| values.into_iter().collect::<BTreeSet<_>>().len();
    let mut conflicts = Vec::new();
    for entity in folded.entities() {
        for key in folded.registers(entity) {
            let values = folded.register(entity, key).into_iter();
            if distinct(values.map(|w| w.value.as_str().to_owned()).collect()) > 1 {
                conflicts.push(Conflicted::Field {
                    entity,
                    key: key.to_owned(),
                });
            }
        }
        if folded.deletion_conflicted(entity) {
            conflicts.push(Conflicted::Existence { entity });
        }
    }
    for (entity, files) in &folded.files() {
        let facts = files.iter().map(|w| format!("{:?}", w.value)).collect();
        if distinct(facts) > 1 {
            conflicts.push(Conflicted::File { entity: *entity });
        }
    }
    conflicts.sort();
    conflicts
}

/// What a view says of each entity: its fields and its file.
type Shown = BTreeMap<
    EntityId,
    (
        Vec<String>,
        Vec<String>,
        Vec<u32>,
        Option<(RelPath, FileState)>,
    ),
>;

fn shown(view: &View) -> Shown {
    let folded = view.folded();
    let values = |entity: EntityId, key: &str| -> Vec<String> {
        let writes = folded.register(entity, key);
        writes.iter().map(|w| w.value.as_str().to_owned()).collect()
    };
    view.entities()
        .into_iter()
        .map(|entity| {
            let id = entity.id();
            let ratings = values(id, "rating")
                .iter()
                .map(|r| r.parse().unwrap())
                .collect();
            let file = entity.file().map(|file| (file.path, file.state));
            let tags = entity.members(TAGS).values;
            (id, (values(id, "origin"), tags, ratings, file))
        })
        .collect()
}

/// A change another program makes to the library's files.
fn outside(folder: &MemDisk, chance: &mut SeededRandom) {
    let files: Vec<RelPath> = folder
        .files(Root::Folder)
        .into_keys()
        .filter(|at| !layout().owns(at))
        .collect();
    let to = pick(chance, &paths());
    let perform = |io: Io| {
        let _ = folder.perform(io);
    };
    perform(Io::MakeDir {
        root: Root::Folder,
        path: to.parent().unwrap(),
    });
    if files.is_empty() || folder.files(Root::Folder).contains_key(&to) {
        return;
    }
    let from = files[below(chance, files.len())].clone();
    match below(chance, 4) {
        0 => perform(Io::Rename {
            root: Root::Folder,
            from,
            to,
        }),
        1 => {
            let bytes = folder.files(Root::Folder)[&from].clone();
            perform(Io::Create {
                root: Root::Folder,
                path: to,
                bytes,
            });
        }
        2 => perform(Io::Remove {
            root: Root::Folder,
            path: from,
        }),
        _ => perform(Io::Create {
            root: Root::Folder,
            path: to,
            bytes: b"made outside".to_vec(),
        }),
    }
}

/// One step a writer takes.
fn write(library: &mut Library<Yielding>, chance: &mut SeededRandom, case: &str) {
    let view = library.view();
    let ids: Vec<EntityId> = view.entities().iter().map(|e| e.id()).collect();
    let entity = (!ids.is_empty()).then(|| ids[below(chance, ids.len())]);
    let origin = pick(chance, &ORIGINS).to_owned();
    let tag = pick(chance, &TAG_NAMES).to_owned();
    let at = pick(chance, &paths());
    let rating = below(chance, 5) as u32;
    let intent = library.intent(case);
    let intent = match (below(chance, 8), entity) {
        (0..=1, _) | (_, None) => {
            let saves = below(chance, 2) == 0;
            let bytes = case.as_bytes().to_vec();
            intent
                .create(|e| {
                    e.set(ORIGIN, origin).add(TAGS, tag);
                    if saves {
                        e.save(&at, bytes, Expect::Absent);
                    }
                })
                .0
        }
        (2, Some(e)) => intent.set(e, ORIGIN, origin),
        (3, Some(e)) => intent.set(e, RATING, rating),
        (4, Some(e)) => intent.add(e, TAGS, tag),
        (5, Some(e)) => intent.remove(e, TAGS, &tag),
        (6, Some(e)) => intent.delete(e),
        (_, Some(e)) => intent.rename(e, &at, Expect::Absent),
    };
    let _ = pollster::block_on(intent.commit());
}

fn run_seeded(seed: u64) {
    let mut chance = SeededRandom::new(seed);
    let folder = MemDisk::new();
    let mut libraries = [
        open(&folder, "a", seed * 10),
        open(&folder, "b", seed * 10 + 1),
    ];
    for step in 0..60 {
        let case = format!("seed {seed}, step {step}");
        let k = below(&mut chance, 2);
        let library = &mut libraries[k];
        match below(&mut chance, 10) {
            0..=3 => write(library, &mut chance, &case),
            4 => outside(&folder, &mut chance),
            5 => {
                pollster::block_on(library.refresh()).unwrap();
            }
            6 => {
                pollster::block_on(library.rescan()).unwrap();
            }
            7 => {
                let _ = pollster::block_on(library.undo());
            }
            _ => {
                let polls = 1 + below(&mut chance, 30);
                if poll_at_most(library.rescan(), polls).is_some() {
                    continue;
                }
                let meanwhile = below(&mut chance, 4);
                match meanwhile {
                    0 => write(&mut libraries[k], &mut chance, &case),
                    1 => write(&mut libraries[1 - k], &mut chance, &case),
                    2 => outside(&folder, &mut chance),
                    _ => {}
                }
                let library = &mut libraries[k];
                check(&library.view(), &format!("{case}, rescan dropped"));
                pollster::block_on(library.rescan()).unwrap();
                if meanwhile == 2 {
                    // A directory listed before the change shows it at the next rescan.
                    pollster::block_on(library.rescan()).unwrap();
                }
                let mut fresh = open(&folder, "fresh", seed * 10 + 2);
                pollster::block_on(fresh.rescan()).unwrap();
                assert_eq!(
                    shown(&library.view()),
                    shown(&fresh.view()),
                    "{case}: a resumed rescan shows what a fresh open and rescan do"
                );
            }
        }
        for (k, library) in libraries.iter().enumerate() {
            check(&library.view(), &format!("{case}, instance {k}"));
        }
    }
    for library in &mut libraries {
        pollster::block_on(library.rescan()).unwrap();
        let again = pollster::block_on(library.rescan()).unwrap();
        assert!(again.changes.is_empty(), "seed {seed}: {:?}", again.changes);
    }
    let [a, b] = &libraries;
    assert_eq!(shown(&a.view()), shown(&b.view()), "seed {seed}: converged");
    check(&a.view(), &format!("seed {seed}, converged"));
}

#[test]
fn indexed_lookups_and_resumed_rescans_match_full_scans_over_seeded_histories() {
    for seed in 0..60 {
        run_seeded(seed);
    }
}
