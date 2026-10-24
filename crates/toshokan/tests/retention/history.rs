//! Histories: writers, a clone, what each has read when it writes, and when it
//! compacts; and the folder a design makes of them.

use std::collections::{BTreeMap, BTreeSet};

use super::world::{
    Body, Dir, Entry, Field, File, Hash, ItemKind, Ref, Seen, Shown, Val, World, Write,
};
use super::{Compaction, Design, Reading};

/// What a writer does, to an entity's register or set member where it names
/// them: `Set(entity, register, value)`, `Add(entity, member)`, and so on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Set(u8, u8, Val),
    Clear(u8, u8),
    Add(u8, u8),
    Remove(u8, u8),
    Delete(u8),
    Revive(u8),
    /// Dismiss every run the writer shows.
    Dismiss,
    /// Take the first change of the first run the writer shows.
    Take,
}

/// A writing instance: its directory, its chain, how much of each other
/// instance's chain it has read, and how many lines its open segment holds.
#[derive(Clone, Debug)]
pub struct Inst {
    pub dir: Dir,
    pub chain: Vec<Hash>,
    pub seen: BTreeMap<usize, usize>,
    pub clone_of: Option<usize>,
    pub open: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Write {
        inst: usize,
        hash: Hash,
    },
    /// With `drop_last`, the sync client first loses the newest line of the
    /// open segment, so the snapshot names that entry by hash.
    Compact {
        inst: usize,
        drop_last: bool,
    },
    /// With `shared`, the copy keeps appending to its own copy of the open
    /// segment, as a virtual machine resumed twice does.
    Clone {
        from: usize,
        shared: bool,
    },
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
    /// Directories, the first of which creates entity 0.
    pub writers: usize,
    /// Whether the second directory's writer may be cloned once.
    pub clone: bool,
    /// Whether the clone may share the open segment.
    pub shared: bool,
    /// Whether a compaction may follow a dropped version of the open segment.
    pub drops: bool,
    /// Entries written after the one creating entity 0.
    pub entries: usize,
    pub acts: &'static [Act],
}

impl Bounds {
    pub const fn new(writers: usize, clone: bool, entries: usize, acts: &'static [Act]) -> Self {
        Self {
            writers,
            clone,
            shared: false,
            drops: false,
            entries,
            acts,
        }
    }
}

/// How a writer sees the folder before it writes.
pub type ViewOf<'a> = dyn Fn(&History, usize) -> Shown + Sync + 'a;

impl History {
    /// The first writer has created entity 0.
    pub fn seeded() -> Self {
        let create = Entry {
            hash: 0,
            dir: 0,
            n: 0,
            prev: None,
            write: Some(Write {
                field: Field::exist(0),
                value: Some(Val::Created),
            }),
            replaces: Vec::new(),
            observed: Vec::new(),
            dismisses: Vec::new(),
            takes: None,
            forks: BTreeMap::new(),
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
                open: 1,
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
    pub fn read_by(&self, i: usize) -> BTreeSet<Hash> {
        let inst = &self.insts[i];
        let mut seen: BTreeSet<Hash> = inst.chain.iter().copied().collect();
        for (&j, &len) in &inst.seen {
            seen.extend(self.insts[j].chain.iter().take(len));
        }
        seen
    }

    /// Every next step: an instance, a new writer or a clone writes one act as
    /// `view` shows it the folder, having first caught up with any set of the
    /// others, then compacts or not.
    pub fn children(&self, bounds: &Bounds, view: &ViewOf<'_>) -> Vec<History> {
        let mut starts: Vec<History> = vec![self.clone()];
        let dirs: BTreeSet<Dir> = self.insts.iter().map(|i| i.dir).collect();
        if dirs.len() < bounds.writers {
            let mut h = self.clone();
            h.insts.push(Inst {
                dir: dirs.len() as Dir,
                chain: Vec::new(),
                seen: BTreeMap::from([(0, 1)]),
                clone_of: None,
                open: 0,
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
                for shared in [false, true] {
                    if shared && (!bounds.shared || self.insts[y].open == 0) {
                        continue;
                    }
                    let mut h = self.clone();
                    let mut copy = h.insts[y].clone();
                    copy.clone_of = Some(y);
                    if !shared {
                        copy.open = 0;
                    }
                    h.insts.push(copy);
                    h.events.push(Event::Clone { from: y, shared });
                    h.acts.push(
                        if shared {
                            "y is cloned mid-segment"
                        } else {
                            "y is cloned"
                        }
                        .into(),
                    );
                    starts.push(h);
                }
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
                    let shown = view(&h, i);
                    for &act in bounds.acts {
                        let Some(child) = h.write(i, act, mask, &shown) else {
                            continue;
                        };
                        for (compact, drop_last) in [(false, false), (true, false), (true, true)] {
                            if drop_last && (!bounds.drops || child.insts[i].open < 2) {
                                continue;
                            }
                            let mut next = child.clone();
                            if compact {
                                next.events.push(Event::Compact { inst: i, drop_last });
                                next.insts[i].open = 0;
                                let what = if drop_last {
                                    ", loses its last line and compacts"
                                } else {
                                    ", compacts"
                                };
                                next.acts.last_mut().unwrap().push_str(what);
                            }
                            children.push(next);
                        }
                    }
                }
            }
        }
        children
    }

    /// Instance `i` writes `act` as `shown`, its view of the folder, allows.
    fn write(&self, i: usize, act: Act, caught_up: usize, shown: &Shown) -> Option<History> {
        let world = &self.world;
        let live_on = |field: Field| -> Vec<Ref> {
            shown
                .writes
                .get(&field)
                .into_iter()
                .flatten()
                .map(|&h| world.entry(h).at())
                .collect()
        };
        let observed = |entity: u8| -> Vec<Ref> {
            let fields = shown
                .writes
                .iter()
                .filter(|(f, _)| f.entity == entity && !f.is_exist());
            fields
                .flat_map(|(_, hs)| hs.iter().map(|&h| world.entry(h).at()))
                .collect()
        };
        let conflict = |e: u8| shown.entities.get(&e).is_some_and(|x| x.deletion_conflict);
        let runs = || shown.items.iter().filter(|it| it.kind == ItemKind::Run);
        let mut takes = None;
        let (write, replaces, observed, dismisses) = match act {
            Act::Set(e, r, v) if shown.exists(e) => (
                Some((Field::reg(e, r), Some(v))),
                live_on(Field::reg(e, r)),
                vec![],
                vec![],
            ),
            Act::Clear(e, r) if shown.values.contains_key(&Field::reg(e, r)) => (
                Some((Field::reg(e, r), None)),
                live_on(Field::reg(e, r)),
                vec![],
                vec![],
            ),
            Act::Add(e, m) if shown.exists(e) => (
                Some((Field::member(e, m), Some(Val::In))),
                live_on(Field::member(e, m)),
                vec![],
                vec![],
            ),
            Act::Remove(e, m) if shown.values.contains_key(&Field::member(e, m)) => (
                Some((Field::member(e, m), None)),
                live_on(Field::member(e, m)),
                vec![],
                vec![],
            ),
            Act::Delete(e) if shown.exists(e) => (
                Some((Field::exist(e), Some(Val::Deleted))),
                live_on(Field::exist(e)),
                observed(e),
                vec![],
            ),
            Act::Revive(e) if !shown.exists(e) || conflict(e) => (
                Some((Field::exist(e), Some(Val::Created))),
                live_on(Field::exist(e)),
                vec![],
                vec![],
            ),
            Act::Dismiss if runs().next().is_some() => {
                (None, vec![], vec![], runs().map(|it| it.run).collect())
            }
            Act::Take => {
                let item = runs().next()?;
                let (&field, &value) = item.change.iter().next()?;
                let mut replaces = live_on(field);
                replaces.extend(item.writes.get(&field));
                let observed = if value == Some(Val::Deleted) {
                    observed(field.entity)
                } else {
                    vec![]
                };
                takes = Some(item.run);
                (Some((field, value)), replaces, observed, vec![])
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
            takes,
            forks: shown.forks.clone(),
        };
        h.world.entries.push(entry);
        h.insts[i].chain.push(hash);
        h.insts[i].open += 1;
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

/// What instance `i` of `history` sees when it writes under design `d`: the
/// folder as it is, the lines of each segment that it has read, and each
/// snapshot whose head it has read.
pub fn writer_view<D: Design>(history: &History, i: usize, d: &D) -> Shown {
    let folder = materialize(history, d);
    let read = history.read_by(i);
    let mut seen = Vec::new();
    for f in folder.files.iter().filter(|f| !f.deleted) {
        let lines = match &f.body {
            Body::Segment(lines) => lines.iter().take_while(|h| read.contains(h)).count(),
            Body::Snapshot(_) => usize::from(read.contains(&folder.heads[&f.id])),
        };
        if lines > 0 {
            seen.push(Seen { file: f.id, lines });
        }
    }
    let reading = Reading {
        world: &history.world,
        files: &folder.files,
        seen: &seen,
    };
    d.read(&reading)
}

/// What a design makes of a history: the folder, and each snapshot's head.
#[derive(Clone, Debug)]
pub struct Folder<S> {
    pub files: Vec<File<S>>,
    pub heads: BTreeMap<usize, Hash>,
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
            Event::Clone { from, shared } => {
                let mut copy = procs[from].clone();
                copy.open = match copy.open.filter(|_| shared) {
                    Some(open) => {
                        let mut file = files[open].clone();
                        file.id = files.len();
                        files.push(file);
                        copy.recorded.insert(files.len() - 1);
                        Some(files.len() - 1)
                    }
                    None => None,
                };
                procs.push(copy);
            }
            Event::Compact { inst, drop_last } => {
                let dir = history.insts[inst].dir;
                let mut p = procs[inst].clone();
                if let Some(open) = p.open.take() {
                    if let (true, Body::Segment(lines)) = (drop_last, &mut files[open].body) {
                        lines.pop();
                    }
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
    Folder { files, heads }
}
