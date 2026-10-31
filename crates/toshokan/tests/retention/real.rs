//! toshokan as a [`Design`]: each entry of the world becomes a toshokan entry, a
//! compaction folds through [`toshokan::compaction::snapshot`], and a reader
//! derives through [`toshokan::derive`].

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use toshokan::compaction::{snapshot, Found};
use toshokan::contribution::{FieldKey, Value};
use toshokan::derive::{self, Derived, SeenSegment};
use toshokan::log::{self, EntryKind, Forked, Logged, Observed, Op, Own, Run, Target};
use toshokan::merge::{Exists, Folded};
use toshokan::report;
use toshokan::snapshot::Snapshot;
use toshokan::{EntityId, EntryHash, Hlc, Raw, WriterId};

use super::world::{
    show, Body, Dir, Dismissal, Entry, Field, File, Hash, Item, ItemKind, Kind, Live, Ref, Seen,
    Shown, Val, World, Write,
};
use super::{Bound, Compaction, Design, Reading};

/// toshokan, under the retention design.
#[derive(Clone, Copy, Debug, Default)]
pub struct Real;

const SET: &str = "s";

fn writer(dir: Dir) -> WriterId {
    WriterId::from_u128(u128::from(dir) + 1)
}

fn dir_of(writer: WriterId) -> Dir {
    Dir::try_from(writer.to_u128() - 1).expect("a writer of the world")
}

fn entity(entity: u8) -> EntityId {
    EntityId::from_u128(u128::from(entity) + 1)
}

fn entity_of(entity: EntityId) -> u8 {
    u8::try_from(entity.to_u128() - 1).expect("an entity of the world")
}

fn register(reg: u8) -> String {
    format!("r{reg}")
}

fn member(member: u8) -> Raw {
    Raw::of(&member).unwrap()
}

fn raw(value: Val) -> Raw {
    Raw::of(&format!("{value:?}")).unwrap()
}

fn val(raw: &Raw) -> Val {
    match raw.as_str() {
        r#""A""# => Val::A,
        r#""B""# => Val::B,
        other => panic!("not a value of the world: {other}"),
    }
}

fn key(field: Field) -> FieldKey {
    let e = entity(field.entity);
    match field.kind {
        Kind::Exist => FieldKey::Exist(e),
        Kind::Reg(reg) => FieldKey::Register(e, register(reg).into()),
        Kind::Member(m) => FieldKey::Member(e, SET.into(), member(m)),
    }
}

fn field(key: &FieldKey) -> Field {
    let e = entity_of(key.entity());
    match key {
        FieldKey::Exist(_) => Field::exist(e),
        FieldKey::Register(_, name) => Field::reg(e, name[1..].parse().unwrap()),
        FieldKey::Member(_, _, m) => Field::member(e, m.as_str().parse().unwrap()),
        FieldKey::File(_) => panic!("the world writes no files"),
    }
}

fn value(value: &Value) -> Option<Val> {
    match value {
        Value::Exist(true) => Some(Val::Created),
        Value::Exist(false) => Some(Val::Deleted),
        Value::Register(value) => value.as_ref().map(val),
        Value::Member(true) => Some(Val::In),
        Value::Member(false) => None,
        Value::File { .. } => panic!("the world writes no files"),
    }
}

/// Entries made, by the world's entry and the hashes of the entries it names.
type Made = HashMap<(Entry, Vec<EntryHash>), Rc<log::Entry>>;

/// The entries `e` names, whose hashes its line holds.
fn named(e: &Entry) -> impl Iterator<Item = Hash> + '_ {
    let runs = e.dismisses.iter().chain(&e.takes);
    let ends = runs.flat_map(|d| [Some(d.first), Some(d.last), d.tip]);
    let refs = e.replaces.iter().chain(&e.observed).copied();
    refs.chain(ends.flatten()).map(|r| r.hash)
}

/// The world's entries as toshokan entries, by the world's hash, and the
/// world's hash of each.
struct Lines {
    entries: Vec<Rc<log::Entry>>,
    back: HashMap<EntryHash, Hash>,
}

impl Lines {
    fn of(world: &World) -> Rc<Self> {
        thread_local! {
            static MADE: RefCell<Made> = RefCell::new(HashMap::new());
            // A history's checks read the same world many times in a row.
            static LAST: RefCell<Option<(Vec<Entry>, Rc<Lines>)>> = const { RefCell::new(None) };
        }
        let last = LAST.with_borrow(|last| {
            let (entries, lines) = last.as_ref()?;
            (*entries == world.entries).then(|| Rc::clone(lines))
        });
        if let Some(lines) = last {
            return lines;
        }
        let lines = Rc::new(MADE.with_borrow_mut(|made| {
            if made.len() > 1 << 20 {
                made.clear();
            }
            let mut lines = Lines {
                entries: Vec::with_capacity(world.entries.len()),
                back: HashMap::new(),
            };
            for e in &world.entries {
                let prev = e
                    .prev
                    .map_or(EntryHash::ZERO, |p| lines.entries[p as usize].hash());
                let named = e.prev.into_iter().chain(named(e));
                let key = named.map(|h| lines.entries[h as usize].hash()).collect();
                let entry = made
                    .entry((e.clone(), key))
                    .or_insert_with(|| Rc::new(lines.encode(world, e, prev)));
                lines.back.insert(entry.hash(), e.hash);
                lines.entries.push(Rc::clone(entry));
            }
            lines
        }));
        LAST.set(Some((world.entries.clone(), Rc::clone(&lines))));
        lines
    }

    fn of_ref(&self, at: Ref) -> log::Ref {
        log::Ref {
            writer: writer(at.dir),
            n: u64::from(at.n),
            hash: self.entries[at.hash as usize].hash(),
        }
    }

    fn own(&self, at: Ref) -> Own {
        Own {
            n: u64::from(at.n),
            hash: self.entries[at.hash as usize].hash(),
        }
    }

    fn run(&self, d: Dismissal) -> Run {
        Run {
            writer: writer(d.dir),
            from: u64::from(d.first.n),
            first: d
                .tip
                .is_none()
                .then(|| self.entries[d.first.hash as usize].hash()),
            last: self.own(d.last),
            tip: d.tip.map(|tip| self.own(tip)),
        }
    }

    fn back(&self, dir: Dir, own: Own) -> Ref {
        Ref {
            dir,
            n: u32::try_from(own.n).unwrap(),
            hash: self.back[&own.hash],
        }
    }

    fn dismissal(&self, run: &Run) -> Dismissal {
        let dir = dir_of(run.writer);
        let first = match (run.first, run.tip) {
            (Some(first), _) => self.back[&first],
            (None, Some(tip)) => self.back[&tip.hash],
            (None, None) => panic!("a run names its first entry or its tip"),
        };
        Dismissal {
            dir,
            first: Ref {
                dir,
                n: u32::try_from(run.from).unwrap(),
                hash: first,
            },
            last: self.back(dir, run.last),
            tip: run.tip.map(|tip| self.back(dir, tip)),
        }
    }

    /// `e` as an intent after `prev`, read at the world's hash, so a clone's
    /// entry differs from its original's.
    fn encode(&self, world: &World, e: &Entry, prev: EntryHash) -> log::Entry {
        let mut ops = Vec::new();
        let take = e.takes.map(|d| self.run(d));
        if let Some(Write { field, value }) = e.write {
            let replaces: Vec<log::Ref> = e.replaces.iter().map(|r| self.of_ref(*r)).collect();
            let target = |r: &Ref| {
                let written = world.entry(r.hash).write.expect("a write").field;
                let target = match written.kind {
                    Kind::Reg(reg) => Target::Register(register(reg)),
                    Kind::Member(m) => Target::Member(SET.into(), member(m)),
                    Kind::Exist => panic!("a delete observes field writes"),
                };
                Observed {
                    target,
                    at: self.of_ref(*r),
                }
            };
            let e_id = entity(field.entity);
            ops.push(match (field.kind, value) {
                (Kind::Exist, Some(Val::Deleted)) => Op::Delete {
                    entity: e_id,
                    replaces,
                    observed: e.observed.iter().map(target).collect(),
                    take,
                },
                (Kind::Exist, _) => Op::Create {
                    entity: e_id,
                    replaces,
                    take,
                },
                (Kind::Reg(reg), value) => Op::Write {
                    entity: e_id,
                    key: register(reg),
                    value: value.map(raw),
                    replaces,
                    take,
                },
                (Kind::Member(m), Some(_)) => Op::Add {
                    entity: e_id,
                    key: SET.into(),
                    value: member(m),
                    replaces,
                    take,
                },
                (Kind::Member(m), None) => Op::Remove {
                    entity: e_id,
                    key: SET.into(),
                    value: member(m),
                    tags: replaces,
                    take,
                },
            });
        }
        if !e.dismisses.is_empty() {
            let runs = e.dismisses.iter().map(|d| self.run(*d)).collect();
            ops.push(Op::Dismiss { runs });
        }
        let forks = e.forks.iter().map(|(&dir, &at)| Forked {
            writer: writer(dir),
            at: u64::from(at),
        });
        let kind = EntryKind::Intent(Logged {
            label: String::new(),
            ops,
            displaced: Vec::new(),
            forks: forks.collect(),
            record: None,
        });
        let at = Hlc {
            wall_ms: u64::from(e.hash) + 1,
            counter: 0,
        };
        log::Entry::encode(prev, u64::from(e.n), at, kind).unwrap()
    }
}

/// What a reader derives from what `r` sees.
fn derived(r: &Reading<'_, Snapshot>, lines: &Lines) -> Derived {
    let mut segments: Vec<(Dir, Vec<Rc<log::Entry>>, bool)> = Vec::new();
    let mut snapshots: Vec<(Dir, Rc<Snapshot>)> = Vec::new();
    for seen in r.seen {
        let file = &r.files[seen.file];
        match &file.body {
            Body::Segment(hashes) => {
                let sealed = file.sealed && seen.lines >= hashes.len();
                let entries = hashes.iter().take(seen.lines);
                let entries = entries.map(|h| Rc::clone(&lines.entries[*h as usize]));
                segments.push((file.dir, entries.collect(), sealed));
            }
            Body::Snapshot(s) => snapshots.push((file.dir, Rc::new(s.clone()))),
        }
    }
    let mut seen: BTreeMap<WriterId, derive::Seen<'_>> = BTreeMap::new();
    for (dir, entries, sealed) in &segments {
        let files = seen.entry(writer(*dir)).or_default();
        files.segments.push(SeenSegment {
            entries,
            sealed: *sealed,
        });
    }
    for (dir, snapshot) in snapshots {
        seen.entry(writer(dir))
            .or_default()
            .snapshots
            .push(snapshot);
    }
    derive::derive(&seen)
}

/// What `derived` shows, in the world's terms.
fn shown(world: &World, lines: &Lines, derived: &Derived) -> Shown {
    let folded = &derived.folded;
    let mut fields: BTreeSet<Field> = world
        .entries
        .iter()
        .filter_map(|e| e.write)
        .map(|w| w.field)
        .collect();
    let entities: Vec<u8> = fields.iter().map(|f| f.entity).collect();
    fields.extend(entities.into_iter().map(Field::exist));
    let at = |by: WriterId, n: u64, entry: EntryHash| Ref {
        dir: dir_of(by),
        n: u32::try_from(n).unwrap(),
        hash: lines.back[&entry],
    };
    let mut live = Vec::new();
    for f in &fields {
        let e = entity(f.entity);
        match f.kind {
            Kind::Exist => {
                for w in folded.existence(e) {
                    let value = match w.value {
                        Exists::Created => Val::Created,
                        Exists::Deleted => Val::Deleted,
                    };
                    live.push(Live {
                        at: at(w.by, w.n, w.entry),
                        write: Write {
                            field: *f,
                            value: Some(value),
                        },
                    });
                }
            }
            Kind::Reg(reg) => {
                for w in folded.register_writes(e, &register(reg)) {
                    live.push(Live {
                        at: at(w.by, w.n, w.entry),
                        write: Write {
                            field: *f,
                            value: w.value.as_ref().map(val),
                        },
                    });
                }
            }
            Kind::Member(m) => {
                for w in folded.tags(e, SET, &member(m)) {
                    live.push(Live {
                        at: at(w.by, w.n, w.entry),
                        write: Write {
                            field: *f,
                            value: w.value.then_some(Val::In),
                        },
                    });
                }
            }
        }
    }
    let mut items = BTreeSet::new();
    for (by, list) in &derived.items {
        // The model shows no item that would change nothing.
        for item in list.iter().filter(|item| !item.change.is_empty()) {
            items.insert(Item {
                dir: dir_of(*by),
                kind: match item.kind {
                    report::ItemKind::Run => ItemKind::Run,
                    report::ItemKind::Gap => ItemKind::Gap,
                    report::ItemKind::Unplaced => ItemKind::Unplaced,
                },
                run: lines.dismissal(&item.run),
                change: item
                    .change
                    .iter()
                    .map(|(k, (_, _, v))| (field(k), value(v)))
                    .collect(),
                writes: item
                    .change
                    .iter()
                    .map(|(k, (r, _, _))| (field(k), at(r.writer, r.n, r.hash)))
                    .collect(),
                forgotten: item.forgotten.iter().map(field).collect(),
            });
        }
    }
    let mut shown = show(&live, items);
    agree_on_existence(folded, &shown);
    shown.contradicted = derived.contradicted.iter().map(field).collect();
    shown.forks = derived
        .writers
        .iter()
        .filter_map(|(by, placed)| Some((dir_of(*by), u32::try_from(placed.fork?).unwrap())))
        .collect();
    shown
}

/// The merged state's own judgment of whether each entity exists matches what
/// its live writes show.
fn agree_on_existence(folded: &Folded, shown: &Shown) {
    for (e, existence) in &shown.entities {
        let present = folded.present(entity(*e));
        assert_eq!(present, existence.exists, "entity {e}: {shown:?}");
    }
}

impl Design for Real {
    type Snapshot = Snapshot;

    fn compact(&self, c: &Compaction<'_, Snapshot>) -> Snapshot {
        let lines = Lines::of(c.world);
        let chain: Vec<Rc<log::Entry>> = c
            .chain
            .iter()
            .map(|h| Rc::clone(&lines.entries[*h as usize]))
            .collect();
        let start = match c.previous {
            Some(previous) => {
                let at = chain.iter().position(|entry| entry.own() == previous.head);
                at.expect("the previous snapshot's head is on the chain") + 1
            }
            None => 0,
        };
        let segments: Vec<Vec<Rc<log::Entry>>> = c
            .files
            .iter()
            .filter(|f| f.dir == c.dir && (!f.deleted || c.recorded.contains(&f.id)))
            .filter_map(|f| match &f.body {
                Body::Segment(hashes) => Some(
                    hashes
                        .iter()
                        .map(|h| Rc::clone(&lines.entries[*h as usize]))
                        .collect(),
                ),
                Body::Snapshot(_) => None,
            })
            .collect();
        let snapshots = c
            .files
            .iter()
            .filter(|f| f.dir == c.dir && !f.deleted)
            .filter_map(|f| match &f.body {
                Body::Snapshot(s) => Some(Found::Snapshot(s)),
                Body::Segment(_) => None,
            });
        let found = segments
            .iter()
            .map(|lines| Found::Lines(lines))
            .chain(snapshots);
        let snapshot = snapshot(
            writer(c.dir),
            chain[0].hash(),
            String::new(),
            c.previous,
            &chain[start..],
            found,
        );
        let snapshot = snapshot.expect("a compaction folds something");
        Snapshot::decode(&snapshot.encode()).expect("a snapshot reads back")
    }

    fn read(&self, r: &Reading<'_, Snapshot>) -> Shown {
        let lines = Lines::of(r.world);
        shown(r.world, &lines, &derived(r, &lines))
    }

    fn names(&self, world: &World, s: &Snapshot, entry: Hash) -> bool {
        let hash = Lines::of(world).entries[entry as usize].hash();
        s.certified.named().any(|named| named.hash == hash)
    }
}

impl Real {
    /// The last entry a snapshot folds.
    pub fn head(&self, world: &World, s: &Snapshot) -> Hash {
        Lines::of(world).back[&s.head.hash]
    }

    /// The own writes of the tips among the snapshots `seen` shows: those no
    /// other one there certifies.
    pub fn tip_writes(
        &self,
        world: &World,
        files: &[File<Snapshot>],
        seen: &[Seen],
    ) -> BTreeSet<Hash> {
        let lines = Lines::of(world);
        let snapshots: Vec<&Snapshot> = seen
            .iter()
            .filter_map(|s| match &files[s.file].body {
                Body::Snapshot(snapshot) => Some(snapshot),
                Body::Segment(_) => None,
            })
            .collect();
        let tip = |s: &&&Snapshot| {
            !snapshots
                .iter()
                .any(|o| o.writer == s.writer && o.head != s.head && o.names(s.head))
        };
        let tips = snapshots.iter().filter(tip);
        tips.flat_map(|s| s.state.own.values().map(|held| lines.back[&held.at.hash]))
            .collect()
    }

    /// The runs a snapshot's dismissals name.
    pub fn dismissals(&self, world: &World, s: &Snapshot) -> Vec<Dismissal> {
        let lines = Lines::of(world);
        s.state
            .dismissals
            .iter()
            .map(|(_, run)| lines.dismissal(run))
            .collect()
    }

    /// Whether a snapshot is within P9's bound.
    pub fn bounded(&self, s: &Snapshot, b: Bound) -> bool {
        let state = &s.state;
        state.own.len() <= b.fields
            && state.refs.len() <= 2 * b.fields * (b.writers - 1)
            && state.takes.len() <= b.fields * (b.writers - 1)
            && s.certified.segments.len() <= b.segments
            && s.certified.hashes.len() <= state.dismissals.len() + b.drops
    }
}
