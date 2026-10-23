//! Histories: writers, a clone, what each has read when it writes, and when it
//! compacts; and the folder a design makes of them.

use std::collections::{BTreeMap, BTreeSet};

use super::oracle::View;
use super::world::{Body, Dir, Entry, Field, File, Hash, Ref, Val, World, Write};
use super::{Compaction, Design};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Set(Val),
    Clear,
    Add(u8),
    Remove(u8),
    Delete,
    Revive,
    Dismiss,
}

/// A writing instance: its directory, its chain, and how much of each other
/// instance's chain it has read.
#[derive(Clone, Debug)]
pub struct Inst {
    pub dir: Dir,
    pub chain: Vec<Hash>,
    pub seen: BTreeMap<usize, usize>,
    pub clone_of: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Write { inst: usize, hash: Hash },
    Compact { inst: usize },
    Clone { from: usize },
}

#[derive(Clone, Debug)]
pub struct History {
    pub world: World,
    pub insts: Vec<Inst>,
    pub events: Vec<Event>,
    pub acts: Vec<String>,
}

/// What the enumeration may do.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    /// Directories, the first of which creates the entity.
    pub writers: usize,
    /// Whether the second directory's writer may be cloned once.
    pub clone: bool,
    /// Entries written after the one creating the entity.
    pub entries: usize,
    pub acts: &'static [Act],
}

impl History {
    /// The first writer has created the entity.
    pub fn seeded() -> Self {
        let create = Entry {
            hash: 0,
            dir: 0,
            n: 0,
            prev: None,
            write: Some(Write {
                field: Field::Exist,
                value: Some(Val::Created),
            }),
            replaces: Vec::new(),
            observed: Vec::new(),
            dismisses: Vec::new(),
        };
        Self {
            world: World {
                entries: vec![create],
            },
            insts: vec![Inst {
                dir: 0,
                chain: vec![0],
                seen: BTreeMap::new(),
                clone_of: None,
            }],
            events: vec![Event::Write { inst: 0, hash: 0 }],
            acts: vec!["x creates".into()],
        }
    }

    pub fn written(&self) -> usize {
        self.world.entries.len() - 1
    }

    /// The entries instance `i` has read: its own chain and what it has seen of
    /// the others.
    fn read_by(&self, i: usize) -> BTreeSet<Hash> {
        let inst = &self.insts[i];
        let mut seen: BTreeSet<Hash> = inst.chain.iter().copied().collect();
        for (&j, &len) in &inst.seen {
            seen.extend(self.insts[j].chain.iter().take(len));
        }
        seen
    }

    /// Every next step: an instance, a new writer or a clone writes one act,
    /// having first caught up with any set of the others, then compacts or not.
    pub fn children(&self, bounds: &Bounds) -> Vec<History> {
        let mut starts: Vec<History> = vec![self.clone()];
        let dirs: BTreeSet<Dir> = self.insts.iter().map(|i| i.dir).collect();
        if dirs.len() < bounds.writers {
            let mut h = self.clone();
            h.insts.push(Inst {
                dir: dirs.len() as Dir,
                chain: Vec::new(),
                seen: BTreeMap::from([(0, 1)]),
                clone_of: None,
            });
            starts.push(h);
        }
        let cloned = self.insts.iter().any(|i| i.clone_of.is_some());
        if bounds.clone && !cloned {
            if let Some(y) = self
                .insts
                .iter()
                .position(|i| i.dir == 1 && !i.chain.is_empty())
            {
                let mut h = self.clone();
                let mut copy = h.insts[y].clone();
                copy.clone_of = Some(y);
                h.insts.push(copy);
                h.events.push(Event::Clone { from: y });
                h.acts.push("y is cloned".into());
                starts.push(h);
            }
        }
        let mut children = Vec::new();
        for start in starts {
            let candidates: Vec<usize> = if start.insts.len() > self.insts.len() {
                vec![start.insts.len() - 1]
            } else {
                (0..start.insts.len()).collect()
            };
            for i in candidates {
                let behind: Vec<usize> = (0..start.insts.len())
                    .filter(|&j| j != i)
                    .filter(|&j| {
                        start.insts[i].seen.get(&j).copied().unwrap_or(0)
                            < start.insts[j].chain.len()
                    })
                    .collect();
                for mask in 0..1usize << behind.len() {
                    let mut h = start.clone();
                    for (b, &j) in behind.iter().enumerate() {
                        if mask & (1 << b) != 0 {
                            let len = h.insts[j].chain.len();
                            h.insts[i].seen.insert(j, len);
                        }
                    }
                    for &act in bounds.acts {
                        let Some(child) = h.write(i, act, mask) else {
                            continue;
                        };
                        let mut compacted = child.clone();
                        compacted.events.push(Event::Compact { inst: i });
                        compacted.acts.last_mut().unwrap().push_str(", compacts");
                        children.push(child);
                        children.push(compacted);
                    }
                }
            }
        }
        children
    }

    /// Instance `i` writes `act` as its view allows.
    fn write(&self, i: usize, act: Act, caught_up: usize) -> Option<History> {
        let seen = self.read_by(i);
        let view = View::of(&self.world, &seen);
        let shown = &view.shown;
        let exists = shown.exists;
        let (write, replaces, observed, dismisses) = match act {
            Act::Set(v) if exists => (
                Some((Field::Reg, Some(v))),
                view.live_on(Field::Reg),
                vec![],
                vec![],
            ),
            Act::Clear if exists && shown.values.contains_key(&Field::Reg) => (
                Some((Field::Reg, None)),
                view.live_on(Field::Reg),
                vec![],
                vec![],
            ),
            Act::Add(m) if exists => (
                Some((Field::Member(m), Some(Val::In))),
                view.live_on(Field::Member(m)),
                vec![],
                vec![],
            ),
            Act::Remove(m) if exists && shown.values.contains_key(&Field::Member(m)) => (
                Some((Field::Member(m), None)),
                view.live_on(Field::Member(m)),
                vec![],
                vec![],
            ),
            Act::Delete if exists => {
                let observed: Vec<Ref> = view
                    .live
                    .iter()
                    .filter(|l| l.write.field != Field::Exist)
                    .map(|l| l.at)
                    .collect();
                (
                    Some((Field::Exist, Some(Val::Deleted))),
                    view.live_on(Field::Exist),
                    observed,
                    vec![],
                )
            }
            Act::Revive if !exists || shown.deletion_conflict => (
                Some((Field::Exist, Some(Val::Created))),
                view.live_on(Field::Exist),
                vec![],
                vec![],
            ),
            Act::Dismiss if !view.dismissals().is_empty() => {
                (None, vec![], vec![], view.dismissals())
            }
            _ => return None,
        };
        let mut h = self.clone();
        let hash = h.world.entries.len() as Hash;
        let inst = &h.insts[i];
        let entry = Entry {
            hash,
            dir: inst.dir,
            n: inst.chain.len() as u32,
            prev: inst.chain.last().copied(),
            write: write.map(|(field, value)| Write { field, value }),
            replaces,
            observed,
            dismisses,
        };
        h.world.entries.push(entry);
        h.insts[i].chain.push(hash);
        h.events.push(Event::Write { inst: i, hash });
        let who = match (h.insts[i].dir, h.insts[i].clone_of) {
            (_, Some(_)) => "y'".to_string(),
            (d, None) => ["x", "y", "z"][d as usize].to_string(),
        };
        let sync = if caught_up != 0 { " after reading" } else { "" };
        h.acts.push(format!("{who}{sync} {act:?}"));
        Some(h)
    }
}

/// What a design makes of a history: the folder, and every snapshot it wrote.
#[derive(Clone, Debug)]
pub struct Folder<S> {
    pub files: Vec<File<S>>,
}

/// One process's state while a history is replayed.
#[derive(Clone, Default)]
struct Proc {
    chain: Vec<Hash>,
    open: Option<usize>,
    last: Option<usize>,
    recorded: BTreeSet<usize>,
}

/// Replays `history` with design `d`: each process appends to its own segment,
/// a compaction seals it, writes `d`'s snapshot, and deletes every sealed
/// segment and earlier snapshot of its directory on its chain.
pub fn materialize<D: Design>(history: &History, d: &D) -> Folder<D::Snapshot> {
    let world = &history.world;
    let mut files: Vec<File<D::Snapshot>> = Vec::new();
    let mut heads: BTreeMap<usize, Hash> = BTreeMap::new();
    let mut procs: Vec<Proc> = vec![Proc::default()];
    for event in &history.events {
        match *event {
            Event::Write { inst, hash } => {
                while procs.len() <= inst {
                    procs.push(Proc::default());
                }
                let dir = history.insts[inst].dir;
                let p = &mut procs[inst];
                let id = *p.open.get_or_insert_with(|| {
                    files.push(File {
                        id: files.len(),
                        dir,
                        body: Body::Segment(Vec::new()),
                        sealed: false,
                        deleted: false,
                    });
                    files.len() - 1
                });
                if let Body::Segment(lines) = &mut files[id].body {
                    lines.push(hash);
                }
                p.chain.push(hash);
                p.recorded.insert(id);
            }
            Event::Clone { from } => {
                let mut copy = procs[from].clone();
                copy.open = None;
                procs.push(copy);
            }
            Event::Compact { inst } => {
                let dir = history.insts[inst].dir;
                let mut p = procs[inst].clone();
                if let Some(open) = p.open.take() {
                    files[open].sealed = true;
                }
                let previous = p.last.and_then(|id| match &files[id].body {
                    Body::Snapshot(s) => Some(s.clone()),
                    Body::Segment(_) => None,
                });
                let snapshot = d.compact(&Compaction {
                    world,
                    dir,
                    chain: &p.chain,
                    previous: previous.as_ref(),
                    files: &files,
                    recorded: &p.recorded,
                });
                let chain: BTreeSet<Hash> = p.chain.iter().copied().collect();
                for f in files.iter_mut().filter(|f| f.dir == dir && !f.deleted) {
                    f.deleted = match &f.body {
                        Body::Segment(lines) => f.sealed && lines.iter().all(|h| chain.contains(h)),
                        Body::Snapshot(_) => chain.contains(&heads[&f.id]),
                    };
                }
                heads.insert(files.len(), *p.chain.last().unwrap());
                files.push(File {
                    id: files.len(),
                    dir,
                    body: Body::Snapshot(snapshot),
                    sealed: true,
                    deleted: false,
                });
                p.last = Some(files.len() - 1);
                p.recorded.clear();
                procs[inst] = p;
            }
        }
    }
    Folder { files }
}

pub fn entries_of(history: &History, dir: Dir) -> Vec<&Entry> {
    history
        .world
        .entries
        .iter()
        .filter(|e| e.dir == dir)
        .collect()
}
