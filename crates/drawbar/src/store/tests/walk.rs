//! Two drawbars on one synced folder, each step one user action a seed chooses, and
//! after each step the invariants that hold whatever the actions were.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs;

use super::*;
use crate::browser::Act;
use crate::folders::{Clash, Occupant};
use crate::log::Level;
use crate::store::shown::Shown;

const SEEDS: [u64; 4] = [1, 2, 3, 4];
const STEPS: usize = 30;
const TAGS: [&str; 3] = ["Live", "Studio", "Gig"];
const DRAWBARS: [&str; 2] = ["A", "B"];

/// What a log says of an undo or redo the other drawbar has changed something under.
const CHANGED_SINCE: &str = "another drawbar has changed it since";

/// What a log says only when something changed outside drawbar or went wrong.
const OUTSIDE: [&str; 5] = [
    "outside drawbar",
    "changed on disk",
    "appeared in the library folder",
    "missing from the library folder",
    "did not change as asked",
];

#[test]
fn two_drawbars_on_one_folder_show_what_is_on_disk_after_every_step_of_a_seeded_walk() {
    for seed in SEEDS {
        Walk::new(seed).run();
    }
}

/// SplitMix64: every roll follows from the seed alone.
struct Dice(u64);

impl Dice {
    fn roll(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let z = self.0;
        let z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.roll() % n as u64) as usize
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn pick<T: Clone>(&mut self, from: &[T]) -> Option<T> {
        match from.len() {
            0 => None,
            n => Some(from[self.below(n)].clone()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Make,
    Save,
    Rename,
    Move,
    Delete,
    Tag,
    Untag,
    MakeFolder,
    RenameFolder,
    RemoveFolder,
    Undo,
    Redo,
    Refresh,
    Refocus,
}

/// How often the walk tries each action.
const WEIGHTS: [(Action, usize); 14] = [
    (Action::Make, 3),
    (Action::Save, 3),
    (Action::Rename, 2),
    (Action::Move, 2),
    (Action::Delete, 1),
    (Action::Tag, 3),
    (Action::Untag, 2),
    (Action::MakeFolder, 2),
    (Action::RenameFolder, 1),
    (Action::RemoveFolder, 1),
    (Action::Undo, 3),
    (Action::Redo, 1),
    (Action::Refresh, 1),
    (Action::Refocus, 1),
];

/// What an action did to the library.
#[derive(Clone, Copy, PartialEq)]
enum Made {
    /// Nothing: it only read.
    Nothing,
    /// One change, which one undo takes back.
    One,
    /// More than one change.
    Several,
    /// An undo or a redo, whose step the history moved.
    Step,
}

/// What an action did, and what it shows once it settles.
struct Done {
    made: Made,
    /// The user went on without waiting for the library's answers.
    hurried: bool,
    shows: Vec<Shows>,
}

impl Done {
    fn one(shows: Vec<Shows>) -> Done {
        Done {
            made: Made::One,
            hurried: false,
            shows,
        }
    }

    fn nothing() -> Done {
        Done {
            made: Made::Nothing,
            hurried: false,
            shows: Vec::new(),
        }
    }
}

/// Something the drawbar that acted shows once its action settles.
#[derive(Debug)]
enum Shows {
    Files(usize),
    File(String),
    NoFile(String),
    /// The file at this path holds these bytes.
    Holding(String, Vec<u8>),
    Folder(String),
    NoFolder(String),
    Wearing(String, String),
    NotWearing(String, String),
}

/// The whole library folder outside drawbar's own data: each file's bytes, and `None`
/// for each folder.
type Disk = BTreeMap<String, Option<Vec<u8>>>;

/// What one drawbar shows: each file with its tags, the folders, the hidden files, the
/// tags, and the rows whose file is gone.
#[derive(Clone, PartialEq, Debug)]
struct Seen {
    files: BTreeMap<String, Vec<String>>,
    folders: Vec<String>,
    hidden: Vec<String>,
    tags: Vec<String>,
    lost: Vec<String>,
}

fn seen(session: &Session) -> Seen {
    let files = session.bench.workspace.listed().map(|entity| {
        let path = session.path(entity.id);
        let path = path.unwrap_or_else(|| format!("{} (no file)", entity.name));
        (path, session.worn(entity.id))
    });
    let folders = &session.bench.browser.folders;
    let mut lost: Vec<String> = folders.lost().iter().map(|l| l.path.to_string()).collect();
    lost.sort();
    Seen {
        files: files.collect(),
        folders: folders.all().iter().map(|f| f.path.to_string()).collect(),
        hidden: folders.others.iter().map(LibPath::to_string).collect(),
        tags: session.tag_names(),
        lost,
    }
}

fn disk(root: &Temp) -> Disk {
    fn walk(dir: &std::path::Path, at: &str, into: &mut Disk) {
        for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{at}: {e}")) {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            if at.is_empty() && name == ".drawbar" {
                continue;
            }
            let path = match at {
                "" => name,
                _ => format!("{at}/{name}"),
            };
            match entry.file_type().unwrap().is_dir() {
                true => {
                    into.insert(path.clone(), None);
                    walk(&entry.path(), &path, into);
                }
                false => {
                    into.insert(path, Some(fs::read(entry.path()).unwrap()));
                }
            }
        }
    }
    let mut found = Disk::new();
    walk(&root.0, "", &mut found);
    found
}

/// What `left` holds that `right` does not, and the reverse, each entry as Debug writes
/// it.
fn apart<T: Debug>(
    left: impl IntoIterator<Item = T>,
    right: impl IntoIterator<Item = T>,
) -> Option<String> {
    let written = |items: Vec<T>| -> BTreeSet<String> {
        items.into_iter().map(|t| format!("{t:?}")).collect()
    };
    let left = written(left.into_iter().collect());
    let right = written(right.into_iter().collect());
    let ours: Vec<&String> = left.difference(&right).collect();
    let theirs: Vec<&String> = right.difference(&left).collect();
    (!ours.is_empty() || !theirs.is_empty())
        .then(|| format!("only in the first: {ours:?}\n  only in the second: {theirs:?}"))
}

/// How two drawbars' [`Seen`] differ, field by field.
fn seen_apart(ours: &Seen, theirs: &Seen) -> Vec<String> {
    let fields = [
        ("files and their tags", apart(&ours.files, &theirs.files)),
        ("folders", apart(&ours.folders, &theirs.folders)),
        ("hidden files", apart(&ours.hidden, &theirs.hidden)),
        ("tags", apart(&ours.tags, &theirs.tags)),
        ("lost rows", apart(&ours.lost, &theirs.lost)),
    ];
    let differ = fields
        .into_iter()
        .filter_map(|(field, d)| Some((field, d?)));
    differ.map(|(field, d)| format!("{field}: {d}")).collect()
}

/// How two stores' views differ, field by field.
fn differences(ours: &Shown, fresh: &Shown) -> Vec<String> {
    let fields = [
        ("files", apart(&ours.files, &fresh.files)),
        ("hidden", apart(&ours.others, &fresh.others)),
        ("lost", apart(&ours.lost, &fresh.lost)),
        ("origins", apart(&ours.came, &fresh.came)),
        ("tags worn", apart(&ours.worn, &fresh.worn)),
        ("tag names", apart(&ours.names, &fresh.names)),
    ];
    let differ = fields
        .into_iter()
        .filter_map(|(field, d)| Some((field, d?)));
    differ.map(|(field, d)| format!("{field}: {d}")).collect()
}

/// The clock of each change one drawbar's Undo and Redo would reach, so the walk can tell
/// whether the other drawbar has changed the library since.
#[derive(Default)]
struct History {
    done: Vec<usize>,
    undone: Vec<usize>,
}

struct Walk {
    seed: u64,
    dice: Dice,
    root: Temp,
    _theirs: Temp,
    drawbars: [Session; 2],
    /// Whether each drawbar has read every change since it last settled.
    current: [bool; 2],
    history: [History; 2],
    /// The clock of the last change each drawbar made.
    changed: [usize; 2],
    /// Whether each drawbar has chosen an undo or redo the other drawbar may have changed
    /// something under since, whose answers have not all come.
    refusable: [bool; 2],
    /// Changes made so far.
    clock: usize,
    /// How many of each drawbar's log entries have been looked at.
    vetted: [usize; 2],
    /// The names of the files the other drawbar wrote, moved or deleted since each
    /// drawbar last came back.
    theirs: [BTreeSet<String>; 2],
    steps: Vec<String>,
    named: usize,
}

impl Walk {
    fn new(seed: u64) -> Walk {
        let root = Temp::new();
        let program = Fresh::Program.bytes().unwrap();
        fs::create_dir(root.at("Live")).unwrap();
        fs::write(root.at("Live/Grand.ne5p"), with_gain(&program, "12")).unwrap();
        fs::write(root.at("Strings.ne5p"), with_gain(&program, "36")).unwrap();
        let theirs = Temp::new();
        let here = Session::open(&root);
        let there = Session::elsewhere(&root, &theirs);
        Walk {
            seed,
            dice: Dice(seed),
            root,
            _theirs: theirs,
            drawbars: [here, there],
            current: [true; 2],
            history: Default::default(),
            changed: [0; 2],
            refusable: [false; 2],
            clock: 0,
            vetted: [0; 2],
            theirs: Default::default(),
            steps: Vec::new(),
            named: 0,
        }
    }

    fn run(mut self) {
        for _ in 0..STEPS {
            self.step();
        }
        for who in 0..2 {
            self.come_back(who);
        }
        let [here, there] = self.drawbars;
        here.close();
        there.close();
    }

    fn fail(&self, invariant: &str, details: String) -> ! {
        let steps: Vec<String> = (self.steps.iter().enumerate())
            .map(|(n, step)| format!("  {}. {step}", n + 1))
            .collect();
        panic!(
            "seed {}, step {}: {invariant}\n  {details}\nsteps:\n{}",
            self.seed,
            self.steps.len(),
            steps.join("\n")
        );
    }

    fn said(&mut self, step: String) {
        self.steps.push(step);
    }

    fn fresh_name(&mut self, stem: &str) -> String {
        self.named += 1;
        format!("{stem} {}", self.named)
    }

    fn step(&mut self) {
        let who = self.dice.below(2);
        if !self.current[who] {
            self.come_back(who);
        }
        let before = (disk(&self.root), seen(&self.drawbars[who]));
        let Done {
            made,
            hurried,
            shows,
        } = loop {
            let total: usize = WEIGHTS.iter().map(|(_, w)| w).sum();
            let mut roll = self.dice.below(total);
            let action = WEIGHTS
                .iter()
                .find(|(_, w)| match roll < *w {
                    true => true,
                    false => {
                        roll -= w;
                        false
                    }
                })
                .map(|(action, _)| *action)
                .expect("a weight covers every roll");
            if let Some(done) = self.act(who, action) {
                break done;
            }
        };
        if made != Made::Nothing {
            self.current[1 - who] = false;
        }
        match hurried {
            true => self.current[who] = false,
            false => {
                self.drawbars[who].sync();
                self.current[who] = true;
            }
        }
        self.check();
        if !hurried {
            self.refusable[who] = false;
            self.shows_its_effect(who, &shows);
        }
        if made == Made::One && !hurried && self.dice.one_in(3) {
            self.undo_and_redo(who, before.clone());
        }
        let after = disk(&self.root);
        let touched = (before.0.iter())
            .filter(|(path, bytes)| after.get(*path) != Some(*bytes))
            .chain(
                after
                    .iter()
                    .filter(|(path, _)| !before.0.contains_key(*path)),
            )
            .filter_map(|(path, _)| Some(LibPath::parse(path)?.leaf().to_string()));
        self.theirs[1 - who].extend(touched.collect::<Vec<String>>());
    }

    /// The drawbar `who` comes back to its window, or asks for a refresh, and shows the
    /// library as a fresh open does once that answers.
    fn come_back(&mut self, who: usize) {
        let refocus = self.dice.one_in(2);
        let session = &mut self.drawbars[who];
        match refocus {
            true => session.refocus(),
            false => session.refresh(),
        }
        let how = ["refreshes", "comes back to its window"][usize::from(refocus)];
        self.said(format!("{} {how}", DRAWBARS[who]));
        self.vet(who, |walk, entry| walk.theirs_arrived(who, entry));
        if refocus {
            self.show_a_fresh_open(&[who], "once its rescan answered");
        }
        self.drawbars[who].sync();
        self.vet(who, |walk, entry| walk.theirs_arrived(who, entry));
        self.theirs[who].clear();
        self.refusable[who] = false;
        self.current[who] = true;
        self.check();
    }

    /// Whether `entry` says that a file the other drawbar saved changed on disk, or that
    /// an undo or redo `who` chose while the other drawbar may have changed what it takes
    /// back is let go.
    fn theirs_arrived(&self, who: usize, entry: &crate::log::Entry) -> bool {
        let quoted = (entry.text.split_once('“'))
            .and_then(|(_, rest)| rest.split_once('”'))
            .map(|(name, _)| name);
        let saved = entry.level == Level::Info
            && entry.text.contains("changed on disk")
            && quoted.is_some_and(|name| self.theirs[who].contains(name));
        saved || (self.refusable[who] && entry.text.contains(CHANGED_SINCE))
    }

    /// Take back the change just made, then make it again: the folder and what drawbar
    /// shows are as they were before it, then as they were after it.
    fn undo_and_redo(&mut self, who: usize, before: (Disk, Seen)) {
        let after = (disk(&self.root), seen(&self.drawbars[who]));
        let name = DRAWBARS[who];
        for (redo, (disk_was, seen_was)) in [(false, before), (true, after)] {
            let (offered, when) = [("Undo", "before"), ("Redo", "after")][usize::from(redo)];
            let session = &mut self.drawbars[who];
            let label = match redo {
                false => session.store.undo_label(),
                true => session.store.redo_label(),
            };
            let Some(label) = label else {
                self.fail(
                    &format!("{name} offers no {offered}"),
                    "after the change".into(),
                );
            };
            match redo {
                false => session.undo(),
                true => session.redo(),
            }
            session.sync();
            self.said(format!("{name} chooses {label} at once"));
            self.check();
            let invariant = format!("{label} leaves the folder other than it was {when}");
            if let Some(d) = apart(&disk(&self.root), &disk_was) {
                self.fail(&invariant, format!("(now first, {when} second)\n  {d}"));
            }
            let differ = seen_apart(&seen(&self.drawbars[who]), &seen_was);
            if !differ.is_empty() {
                let invariant = format!("{label} leaves {name} showing other than {when}");
                let d = format!("(now first, {when} second)\n  {}", differ.join("\n  "));
                self.fail(&invariant, d);
            }
        }
    }

    /// Every invariant that holds after a step: for each drawbar that has read every
    /// change, what it shows is a fresh open and is on disk; and no log says anything
    /// changed outside drawbar.
    fn check(&mut self) {
        for who in 0..2 {
            self.vet(who, |walk, entry| walk.theirs_arrived(who, entry));
            self.nothing_missing(who);
        }
        let current: Vec<usize> = (0..2).filter(|who| self.current[*who]).collect();
        self.show_a_fresh_open(&current, "once settled");
        for who in current {
            self.shows_the_disk(who);
        }
    }

    /// What drawbar `who` shows has what its action did.
    fn shows_its_effect(&self, who: usize, shows: &[Shows]) {
        let session = &self.drawbars[who];
        let now = seen(session);
        let holding = |path: &str| {
            let mut listed = session.bench.workspace.listed();
            let entity = listed.find(|entity| session.path(entity.id).as_deref() == Some(path));
            entity.map(|entity| entity.bytes.to_vec())
        };
        let wears = |path: &str, tag: &str| {
            now.files
                .get(path)
                .map(|worn| worn.iter().any(|t| t == tag))
        };
        let wrong: Vec<&Shows> = (shows.iter())
            .filter(|shows| match shows {
                Shows::Files(n) => now.files.len() != *n,
                Shows::File(path) => !now.files.contains_key(path),
                Shows::NoFile(path) => now.files.contains_key(path),
                Shows::Holding(path, bytes) => holding(path).as_ref() != Some(bytes),
                Shows::Folder(path) => !now.folders.contains(path),
                Shows::NoFolder(path) => now.folders.contains(path),
                Shows::Wearing(path, tag) => wears(path, tag) != Some(true),
                Shows::NotWearing(path, tag) => wears(path, tag) != Some(false),
            })
            .collect();
        if !wrong.is_empty() {
            let invariant = format!("{} does not show what it did", DRAWBARS[who]);
            self.fail(&invariant, format!("expected {wrong:?}\n  shows {now:#?}"));
        }
    }

    /// Look at the log entries `who` wrote since the last look: none says something
    /// changed outside drawbar or went wrong, unless `accounted` accounts for it.
    fn vet(&mut self, who: usize, accounted: impl Fn(&Walk, &crate::log::Entry) -> bool) {
        let log = &self.drawbars[who].bench.log;
        assert!(
            log.len() < 500,
            "{} logged more than its log keeps",
            DRAWBARS[who]
        );
        let wrong: Vec<String> = (log.iter().skip(self.vetted[who]))
            .filter(|entry| !accounted(self, entry))
            .filter(|entry| {
                entry.level != Level::Info || OUTSIDE.iter().any(|said| entry.text.contains(said))
            })
            .map(|entry| entry.text.clone())
            .collect();
        self.vetted[who] = log.len();
        if !wrong.is_empty() {
            let name = DRAWBARS[who];
            let invariant = format!("{name} says something changed outside drawbar or failed");
            self.fail(&invariant, format!("{wrong:#?}"));
        }
    }

    fn nothing_missing(&self, who: usize) {
        let folders = &self.drawbars[who].bench.browser.folders;
        if !folders.missing.is_empty() {
            let missing = format!("{:?}", folders.missing);
            self.fail(
                &format!("{} shows a file as missing", DRAWBARS[who]),
                missing,
            );
        }
    }

    fn show_a_fresh_open(&self, drawbars: &[usize], when: &str) {
        if drawbars.is_empty() {
            return;
        }
        let local = Temp::new();
        let fresh = Session::elsewhere(&self.root, &local);
        for who in drawbars {
            self.shows(*who, &fresh, when);
        }
        fresh.close();
    }

    fn shows(&self, who: usize, fresh: &Session, when: &str) {
        let name = DRAWBARS[who];
        let differ = seen_apart(&seen(&self.drawbars[who]), &seen(fresh));
        if !differ.is_empty() {
            let invariant = format!("{name} {when} does not show what a fresh open shows");
            let first = format!("({name} first, a fresh open second)\n  ");
            self.fail(&invariant, first + &differ.join("\n  "));
        }
        let held = self.drawbars[who]
            .store
            .shown()
            .cloned()
            .unwrap_or_default();
        let opened = fresh.store.shown().cloned().unwrap_or_default();
        let differ = differences(&held, &opened);
        if !differ.is_empty() {
            let invariant = format!("{name}'s view {when} is not a fresh open's");
            self.fail(&invariant, differ.join("\n  "));
        }
    }

    fn shows_the_disk(&self, who: usize) {
        let session = &self.drawbars[who];
        let on_disk = disk(&self.root);
        let mut wrong = Vec::new();
        for entity in session.bench.workspace.listed() {
            let Some(path) = session.path(entity.id) else {
                wrong.push(format!("“{}” has no file", entity.name));
                continue;
            };
            if entity.is_unsaved() {
                wrong.push(format!("{path} is unsaved"));
            }
            match on_disk.get(&path) {
                Some(Some(bytes)) if entity.unread() || *bytes == entity.bytes.to_vec() => {}
                Some(Some(_)) => wrong.push(format!("{path} holds other bytes on disk")),
                _ => wrong.push(format!("{path} is not a file on disk")),
            }
        }
        let folders = &session.bench.browser.folders;
        let shown_dirs = folders.all().iter().map(|folder| folder.path.to_string());
        for dir in shown_dirs.clone() {
            if on_disk.get(&dir) != Some(&None) {
                wrong.push(format!("folder {dir} is not on disk"));
            }
        }
        let files: BTreeSet<String> = (session.bench.workspace.listed())
            .filter_map(|entity| session.path(entity.id))
            .chain(folders.others.iter().map(LibPath::to_string))
            .collect();
        let dirs: BTreeSet<String> = shown_dirs.collect();
        for (path, bytes) in &on_disk {
            let shown = match bytes {
                Some(_) => files.contains(path),
                None => dirs.contains(path),
            };
            if !shown {
                wrong.push(format!("{path} is on disk and not shown"));
            }
        }
        if !wrong.is_empty() {
            let invariant = format!("what {} shows is not on disk", DRAWBARS[who]);
            self.fail(&invariant, format!("{wrong:#?}"));
        }
    }

    /// Do `action` as drawbar `who`, where it has something to act on: what it made of
    /// the library, and whether the user went on without waiting for its answers.
    fn act(&mut self, who: usize, action: Action) -> Option<Done> {
        let name = DRAWBARS[who];
        let files: Vec<u64> = (self.drawbars[who].bench.workspace.listed())
            .filter(|entity| entity.path.is_some())
            .map(|entity| entity.id)
            .collect();
        let folders: Vec<u64> = (self.drawbars[who].bench.browser.folders.all().iter())
            .map(|folder| folder.id)
            .collect();
        let path = |walk: &Walk, id: u64| walk.drawbars[who].path(id).unwrap_or_default();
        let done = match action {
            Action::Make => {
                let session = &mut self.drawbars[who];
                let count = session.bench.workspace.listed().count();
                perform(session, vec![Act::New(Fresh::Program)]);
                self.said(format!("{name} makes a program"));
                Done::one(vec![Shows::Files(count + 1)])
            }
            Action::Save => {
                let id = self.dice.pick(&files)?;
                let gain = 1 + self.dice.below(99);
                let session = &mut self.drawbars[who];
                session.read(&[id]);
                let held = session.bytes(id);
                let edited = with_gain(&held, &gain.to_string());
                if edited == held {
                    return None;
                }
                let Bench { workspace, log, .. } = &mut session.bench;
                workspace.replace_bytes(id, edited.clone(), log);
                perform(session, vec![Act::SaveDoc(id)]);
                let at = path(self, id);
                self.said(format!(
                    "{name} sets the gain of {at} to {gain} and saves it"
                ));
                Done::one(vec![Shows::Holding(at, edited)])
            }
            Action::Rename => {
                let id = self.dice.pick(&files)?;
                let to = self.fresh_name("Sound");
                let at = path(self, id);
                let dir = LibPath::parse(&at)?.parent();
                if !self.free(who, &dir, &format!("{to}.ne5p"), Some(Occupant::Asset(id))) {
                    return None;
                }
                let renaming = Act::RenameLocal {
                    id,
                    name: to.clone(),
                };
                perform(&mut self.drawbars[who], vec![renaming]);
                self.said(format!("{name} renames {at} to “{to}”"));
                let renamed = dir.join(&format!("{to}.ne5p")).to_string();
                Done::one(vec![Shows::NoFile(at), Shows::File(renamed)])
            }
            Action::Move => {
                let id = self.dice.pick(&files)?;
                let folder = self.dice.pick(&folders).filter(|_| !self.dice.one_in(3));
                let at = LibPath::parse(&path(self, id))?;
                let dir = self.drawbars[who].bench.browser.folders.dir(folder)?;
                if dir == at.parent() || !self.free(who, &dir, at.leaf(), None) {
                    return None;
                }
                perform(&mut self.drawbars[who], vec![Act::File { id, folder }]);
                let into = match dir.is_root() {
                    true => "the top level".to_string(),
                    false => format!("“{dir}”"),
                };
                self.said(format!("{name} moves {at} into {into}"));
                let moved = dir.join(at.leaf()).to_string();
                Done::one(vec![Shows::NoFile(at.to_string()), Shows::File(moved)])
            }
            Action::Delete => {
                let id = self.dice.pick(&files)?;
                let at = path(self, id);
                perform(&mut self.drawbars[who], vec![Act::Remove(id)]);
                self.said(format!("{name} deletes {at}"));
                Done::one(vec![Shows::NoFile(at)])
            }
            Action::Tag => {
                let id = self.dice.pick(&files)?;
                let tag = self.dice.pick(&TAGS)?;
                if self.drawbars[who].worn(id).iter().any(|worn| worn == tag) {
                    return None;
                }
                let hurried = self.dice.one_in(2);
                self.drawbars[who].tag(&[id], tag);
                let at = path(self, id);
                self.said(format!("{name} tags {at} “{tag}”{}", hurry(hurried)));
                let shows = vec![Shows::Wearing(at, tag.to_string())];
                Done {
                    hurried,
                    ..Done::one(shows)
                }
            }
            Action::Untag => {
                let id = self.dice.pick(&files)?;
                let worn = self.drawbars[who].worn(id);
                let tag = self.dice.pick(&worn)?;
                let tags = &self.drawbars[who].bench.browser.tags;
                let tag_id = tags.all().iter().find(|t| t.name == tag)?.id;
                let hurried = self.dice.one_in(2);
                let session = &mut self.drawbars[who];
                perform(
                    session,
                    vec![Act::Untag {
                        ids: vec![id],
                        tag: tag_id,
                    }],
                );
                session.decided();
                let at = path(self, id);
                self.said(format!("{name} takes “{tag}” off {at}{}", hurry(hurried)));
                let shows = vec![Shows::NotWearing(at, tag)];
                Done {
                    hurried,
                    ..Done::one(shows)
                }
            }
            Action::MakeFolder => {
                let parent = self.dice.pick(&folders).filter(|_| self.dice.one_in(2));
                let between = self.dice.one_in(2);
                let to = self.fresh_name("Folder");
                let session = &mut self.drawbars[who];
                let before: BTreeSet<u64> = folders.iter().copied().collect();
                perform(
                    session,
                    vec![match parent {
                        None => Act::NewFolder,
                        Some(parent) => Act::NewFolderIn(parent),
                    }],
                );
                let made = (session.bench.browser.folders.all().iter())
                    .map(|folder| folder.id)
                    .find(|id| !before.contains(id))
                    .expect("a new folder is shown");
                let at = session.bench.browser.folders.path_of(made)?.clone();
                if between {
                    let Bench {
                        workspace, browser, ..
                    } = &mut session.bench;
                    session.store.sync(workspace, browser, Pass::Files);
                    session.poll();
                }
                let naming = Act::RenameFolder {
                    id: made,
                    name: to.clone(),
                };
                perform(session, vec![naming]);
                let pause = ["", ", a frame passes,"][usize::from(between)];
                self.said(format!("{name} makes {at}{pause} and names it “{to}”"));
                let named = at.parent().join(&to).to_string();
                let shows = vec![Shows::Folder(named)];
                match between {
                    true => Done {
                        made: Made::Several,
                        ..Done::one(shows)
                    },
                    false => Done::one(shows),
                }
            }
            Action::RenameFolder => {
                let id = self.dice.pick(&folders)?;
                let to = self.fresh_name("Folder");
                let at = self.drawbars[who]
                    .bench
                    .browser
                    .folders
                    .path_of(id)?
                    .clone();
                // ⚠️ Where the folder renames no folder, as in a browser, an empty folder
                // inside stays behind under the old name.
                if self.holds_an_empty_folder(who, &at) {
                    return None;
                }
                let session = &mut self.drawbars[who];
                perform(
                    session,
                    vec![Act::RenameFolder {
                        id,
                        name: to.clone(),
                    }],
                );
                self.said(format!("{name} renames folder {at} to “{to}”"));
                let renamed = at.parent().join(&to).to_string();
                Done::one(vec![
                    Shows::NoFolder(at.to_string()),
                    Shows::Folder(renamed),
                ])
            }
            Action::RemoveFolder => {
                let id = self.dice.pick(&folders)?;
                let session = &self.drawbars[who];
                let shown = &session.bench.browser.folders;
                let at = shown.path_of(id)?.clone();
                let members = shown.members(Some(id), &session.bench.workspace);
                let leaves = members.iter().filter_map(|entity| entity.path.as_ref());
                let children = shown.children(Some(id)).into_iter();
                let children: Vec<&LibPath> = children.filter_map(|c| shown.path_of(c)).collect();
                let mut names = leaves.chain(children.iter().copied()).map(LibPath::leaf);
                if !names.all(|leaf| self.free(who, &at.parent(), leaf, None)) {
                    return None;
                }
                // ⚠️ Where the folder renames no folder, a folder moved up leaves an empty
                // folder inside it behind.
                if children
                    .iter()
                    .any(|child| self.holds_an_empty_folder(who, child))
                {
                    return None;
                }
                let session = &mut self.drawbars[who];
                perform(session, vec![Act::RemoveFolder(id)]);
                self.said(format!("{name} removes folder {at}"));
                Done::one(vec![Shows::NoFolder(at.to_string())])
            }
            Action::Undo | Action::Redo => return self.step_history(who, action),
            Action::Refresh => {
                self.drawbars[who].refresh();
                self.said(format!("{name} refreshes"));
                Done::nothing()
            }
            Action::Refocus => {
                self.drawbars[who].refocus();
                self.said(format!("{name} comes back to its window"));
                Done::nothing()
            }
        };
        if done.made != Made::Nothing {
            for _ in 0..[1, 2][usize::from(done.made == Made::Several)] {
                self.clock += 1;
                self.history[who].done.push(self.clock);
            }
            self.history[who].undone.clear();
            self.changed[who] = self.clock;
        }
        Some(done)
    }

    /// Undo or redo as drawbar `who`, where its Edit menu offers it. One the other drawbar
    /// has changed something under since may be let go, so the walk then forgets what
    /// comes after it.
    fn step_history(&mut self, who: usize, action: Action) -> Option<Done> {
        let redo = matches!(action, Action::Redo);
        let History { done, undone } = &mut self.history[who];
        let (from, to) = match redo {
            false => (done, undone),
            true => (undone, done),
        };
        let made_at = *from.last()?;
        let refusable = self.changed[1 - who] > made_at;
        let session = &mut self.drawbars[who];
        let offered = match redo {
            false => session.store.undo_label()?,
            true => session.store.redo_label()?,
        };
        from.pop();
        match refusable {
            true => to.clear(),
            false => to.push(made_at),
        }
        self.refusable[who] |= refusable;
        let hurried = self.dice.one_in(2);
        match redo {
            false => session.undo(),
            true => session.redo(),
        }
        self.clock += 1;
        self.changed[who] = self.clock;
        let name = DRAWBARS[who];
        self.said(format!("{name} chooses {offered}{}", hurry(hurried)));
        Some(Done {
            made: Made::Step,
            hurried,
            shows: Vec::new(),
        })
    }

    /// Whether a folder with nothing in it is anywhere inside `dir`, as `who` shows it.
    fn holds_an_empty_folder(&self, who: usize, dir: &LibPath) -> bool {
        let shown = seen(&self.drawbars[who]);
        let inside = |path: &str, of: &str| path.starts_with(&format!("{of}/"));
        let paths: Vec<&String> = shown.files.keys().chain(&shown.folders).collect();
        (shown.folders.iter())
            .filter(|folder| inside(folder, dir.as_str()))
            .any(|folder| !paths.iter().any(|path| inside(path, folder)))
    }

    /// Whether `name` is free in `dir` for drawbar `who`, leaving `except` out.
    fn free(&self, who: usize, dir: &LibPath, name: &str, except: Option<Occupant>) -> bool {
        let session = &self.drawbars[who];
        let folders = &session.bench.browser.folders;
        folders.clash(dir, name, &session.bench.workspace, except) == Clash::Free
    }
}

/// Run `acts` as the browser does, and once more after the files they wait for are read,
/// as the next frame does.
fn perform(session: &mut Session, acts: Vec<Act>) {
    session.bench.act(acts);
    session.answer_reads();
    session.bench.act(Vec::new());
}

fn hurry(hurried: bool) -> &'static str {
    match hurried {
        true => ", and the user goes on without waiting",
        false => "",
    }
}
