//! Drawbars on one library, each step one user action or one step of sync a seed
//! chooses, and after each step the invariants that hold whatever the steps were.

mod sync;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs;
use std::ops::RangeInclusive;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use toshokan::env::SeededRandom;
use toshokan::log::{EntryKind, Op};
use toshokan::simulator::SyncEvent;

use super::*;
use crate::browser::Act;
use crate::folders::{Clash, Occupant};
use crate::log::Level;
use crate::store::shown::{Class, Row, Shown};
use sync::{leaf, tree, Entry, Sync, Tree};

const TAGS: [&str; 3] = ["Live", "Studio", "Gig"];

/// What a log says of an undo or redo the library refused and let go of: its reason
/// follows.
const LET_GO: [&str; 2] = ["cannot be undone: ", "cannot be made again: "];

/// The reasons for letting go of an undo or redo that another drawbar's change gives, as
/// far as this drawbar can tell them while sync lags.
const CONCURRENT: [&str; 6] = [
    "changed it since",
    "not as drawbar last saw it",
    "no longer in the library",
    "another asset's file",
    "is a folder",
    "holds files",
];

/// What a log says of an undo or redo the library refused and kept, as one that may go
/// once what the folder holds in its way moves: its reason follows.
const KEPT: [&str; 3] = ["Undo: ", "Redo: ", "The library did not change as asked: "];

/// The reason `text` gives for an undo or redo the library refused and kept.
fn kept_for(text: &str) -> Option<&str> {
    let why = KEPT.iter().find_map(|said| text.strip_prefix(said))?;
    Some(why.trim_end_matches('.'))
}

/// What a kept refusal says of a file the folder holds in the way, which moving it lets
/// the undo or redo by.
const IN_THE_WAY: [&str; 4] = [
    "is a folder",
    "holds files",
    "is another asset's file",
    "is taken",
];

/// What a kept refusal says of a file not as the undo or redo's step left it, which only
/// that file coming back as it was lets by.
const NOT_AS_LEFT: &str = "is not as drawbar last saw it";

/// What a kept refusal says of an entity whose file has not arrived.
const NO_FILE: &str = "it has no file";

/// Whether `text` says an undo or redo was let go.
fn let_go(text: &str) -> bool {
    LET_GO.iter().any(|said| text.contains(said))
}

/// Whether `text` says an undo or redo was let go for what another drawbar may have done.
fn let_go_for_another(text: &str) -> bool {
    let_go(text) && CONCURRENT.iter().any(|said| text.contains(said))
}

/// What a log may say of a file two drawbars changed before either saw the other's
/// change: a true conflict.
const CONTESTED: [&str; 5] = [
    "changed on disk while drawbar held an unsaved edit",
    "changed on disk, and drawbar now shows it as it is there",
    "did not change as asked",
    "Undo: ",
    "Redo: ",
];

/// How many times sync delivers everything and every drawbar rescans before the walk
/// says sync does not settle.
const ROUNDS: usize = 4;

#[test]
fn two_drawbars_on_one_folder_show_what_is_on_disk_after_every_step_of_a_seeded_walk() {
    walks(&[World::Shared], &[2], 1..=4);
}

#[test]
fn drawbars_on_synced_copies_show_their_own_copy_and_agree_once_sync_settles() {
    let worlds = [
        World::Synced { chaos: false },
        World::Synced { chaos: true },
    ];
    walks(&worlds, &[2, 3], 1..=1);
}

/// Where the drawbars of a walk keep the library.
#[derive(Clone, Copy, Debug)]
enum World {
    /// One folder, which every drawbar reads and writes.
    Shared,
    /// A copy of the folder for each drawbar, which sync keeps in step, with the
    /// simulator's chaos or without it.
    Synced { chaos: bool },
}

impl World {
    /// How many steps a walk takes: in a synced world, about half of them sync.
    fn steps(self) -> usize {
        match self {
            World::Shared => 30,
            World::Synced { .. } => 60,
        }
    }
}

/// Walk each of `worlds` with each of `counts` drawbars, from each seed of
/// `DRAWBAR_WALK_SEEDS`, a range such as `1..=500`, or of `seeds` where it is unset, and
/// say every walk that failed.
fn walks(worlds: &[World], counts: &[usize], seeds: RangeInclusive<u64>) {
    let seeds = match std::env::var("DRAWBAR_WALK_SEEDS") {
        Ok(range) => seed_range(&range),
        Err(_) => seeds,
    };
    let mut failed = Vec::new();
    let mut walked = 0;
    for seed in seeds {
        for world in worlds {
            for count in counts {
                walked += 1;
                let walk = || Walk::new(*world, *count, seed).run();
                if let Err(panic) = panic::catch_unwind(AssertUnwindSafe(walk)) {
                    let said = panicked(panic);
                    let walk = format!("{world:?} with {count} drawbars, seed {seed}");
                    match said.starts_with(&walk) {
                        true => failed.push(said),
                        false => failed.push(format!("{walk}: {said}")),
                    }
                }
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {walked} walks failed:\n\n{}",
        failed.len(),
        failed.join("\n\n")
    );
}

fn seed_range(range: &str) -> RangeInclusive<u64> {
    let seed = |text: &str| text.trim().parse::<u64>().ok();
    let parsed = range
        .split_once("..=")
        .and_then(|(from, to)| Some(seed(from)?..=seed(to)?));
    parsed.unwrap_or_else(|| panic!("DRAWBAR_WALK_SEEDS is a range such as 1..=500, not {range:?}"))
}

fn panicked(panic: Box<dyn std::any::Any + Send>) -> String {
    match panic.downcast::<String>() {
        Ok(said) => *said,
        Err(panic) => match panic.downcast::<&str>() {
            Ok(said) => said.to_string(),
            Err(_) => "a panic that says nothing".to_string(),
        },
    }
}

/// Drawbar `who`'s name in what the walk says.
fn name(who: usize) -> char {
    char::from(b'A' + u8::try_from(who).expect("fewer than 26 drawbars"))
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

/// One in how many files put in place within [`COARSE`] of the file put at its path
/// before it takes that file's time, as on a volume whose times are coarser than its
/// writes.
const TIES: usize = 4;

/// How coarse the times of a volume [`TIES`] models are, in nanoseconds.
const COARSE: u64 = 2_000_000_000;

/// The walk's time, which every drawbar's clock reads and which dates every file written,
/// and the seeds of the ids each drawbar draws: all from the walk's seed, with dice of
/// their own so that they leave the walk's choices as they were.
#[derive(Clone)]
struct Times(Arc<Mutex<Ticks>>);

struct Ticks {
    ms: u64,
    /// The time of the last file written, in nanoseconds since the Unix epoch.
    ns: u64,
    /// The time each path was last given.
    given: BTreeMap<std::path::PathBuf, SystemTime>,
    dice: Dice,
}

impl Times {
    fn new(seed: u64) -> Times {
        let ms = 1_800_000_000_000;
        let ticks = Ticks {
            ms,
            ns: ms * 1_000_000,
            given: BTreeMap::new(),
            dice: Dice(seed ^ 0x7469_6d65_7469_6d65),
        };
        Times(Arc::new(Mutex::new(ticks)))
    }

    fn ticks(&self) -> std::sync::MutexGuard<'_, Ticks> {
        self.0.lock().expect("never poisoned")
    }

    /// Up to two seconds pass, as between two steps of the user's.
    fn pass(&self) {
        let mut ticks = self.ticks();
        ticks.ms += 1 + ticks.dice.below(2_000) as u64;
    }

    /// The time of the file just written at `at`, or, where it was staged with the time
    /// `staged` and put in place there, that time or now and then the time the file put
    /// there just before it was given.
    fn stamp(&self, at: &std::path::Path, staged: Option<SystemTime>) -> SystemTime {
        let mut ticks = self.ticks();
        let now = ticks.ms * 1_000_000;
        let recent = |time: &SystemTime| {
            let ns = time
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos());
            ns.is_ok_and(|ns| ns + u128::from(COARSE) >= u128::from(now))
        };
        let before = ticks.given.get(at).copied().filter(recent);
        let tied = before.filter(|_| staged.is_some() && ticks.dice.one_in(TIES));
        let time = tied.or(staged).unwrap_or_else(|| {
            let after = ticks.ns.max(ticks.ms * 1_000_000);
            ticks.ns = after + 1_000 * (1 + ticks.dice.below(1_000) as u64);
            SystemTime::UNIX_EPOCH + Duration::from_nanos(ticks.ns)
        });
        ticks.given.insert(at.to_path_buf(), time);
        time
    }

    /// Give the file at `path` the time of a file written now.
    fn date(&self, path: &std::path::Path) {
        let file = fs::File::options().write(true).open(path);
        file.and_then(|file| file.set_modified(self.stamp(path, None)))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    /// What a drawbar opened next takes from outside: this time, and ids of its own.
    fn ambient(&self) -> Ambient {
        let random = SeededRandom::new(self.ticks().dice.roll());
        let stamping = self.clone();
        Ambient {
            clock: Box::new(self.clone()),
            random: Box::new(random),
            stamp: Some(Box::new(move |at, staged| stamping.stamp(at, staged))),
        }
    }
}

impl toshokan::Clock for Times {
    fn now_ms(&mut self) -> u64 {
        self.ticks().ms
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Make,
    Edit,
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
const WEIGHTS: [(Action, usize); 15] = [
    (Action::Make, 3),
    (Action::Edit, 2),
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

/// What a failure broke.
#[derive(Clone, Copy)]
enum Broke {
    /// Each drawbar shows its own folder, as a fresh open of it does.
    OwnFolder,
    /// Once sync settles and every drawbar rescans, all agree with each other and with
    /// every folder.
    Convergence,
    /// Only another drawbar's save, a conflict copy or a chaos event makes a drawbar say
    /// something changed outside drawbar or went wrong.
    Silence,
    /// An undo or redo another drawbar changed something under is refused once, and
    /// the next goes.
    Undo,
    /// An asset with an unsaved edit stays in the list with its edit.
    Unsaved,
    /// The drawbar that acted shows what it did.
    Effect,
    /// The sync model's premise: only a log's writer changes it.
    Model,
}

impl Broke {
    fn says(self) -> &'static str {
        match self {
            Broke::OwnFolder => "each drawbar shows its own folder",
            Broke::Convergence => "drawbars converge once sync settles",
            Broke::Silence => "sync is silent",
            Broke::Undo => "undo is never stuck",
            Broke::Unsaved => "no unsaved edit is lost",
            Broke::Effect => "a drawbar shows what it did",
            Broke::Model => "only its writer changes a log",
        }
    }
}

/// What an action did to the library.
#[derive(Clone, Copy, PartialEq)]
enum Made {
    /// Nothing: it only read, or edited without saving.
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
    /// The asset whose bytes it edited, with the gain it set.
    edited: Option<(u64, usize)>,
    /// The asset whose file it made or saved.
    wrote: Option<u64>,
}

impl Done {
    fn one(shows: Vec<Shows>) -> Done {
        Done {
            made: Made::One,
            hurried: false,
            shows,
            edited: None,
            wrote: None,
        }
    }

    fn nothing() -> Done {
        Done {
            made: Made::Nothing,
            ..Done::one(Vec::new())
        }
    }
}

/// Something the drawbar that acted shows once its action settles.
#[derive(Debug)]
enum Shows {
    Files(usize),
    File(String),
    NoFile(String),
    /// The asset with this id holds these bytes.
    Holding(u64, Vec<u8>),
    Folder(String),
    NoFolder(String),
    Wearing(String, String),
    NotWearing(String, String),
}

/// The library files and folders of a folder, outside drawbar's own data.
type Disk = BTreeMap<String, Entry>;

/// Whether `path` of a folder is in drawbar's own data.
fn own(path: &str) -> bool {
    path == ".drawbar" || path.starts_with(".drawbar/")
}

/// The hash of every entry of every writer's log in `root`'s copy.
fn logged(root: &Temp) -> BTreeSet<toshokan::EntryHash> {
    logged_entries(root)
        .iter()
        .map(|entry| entry.hash())
        .collect()
}

/// Every entry of every writer's log in `root`'s copy.
fn logged_entries(root: &Temp) -> Vec<toshokan::log::Entry> {
    let writers = fs::read_dir(root.at(".drawbar/library/writers.2"));
    let dirs = writers.into_iter().flatten().flatten();
    let files = dirs.flat_map(|dir| fs::read_dir(dir.path()).into_iter().flatten().flatten());
    let logs = files.filter(|file| file.path().extension().is_some_and(|ext| ext == "txt"));
    let held = logs.map(|file| fs::read(file.path()).unwrap_or_default());
    let held: Vec<Vec<u8>> = held.collect();
    let lines = held
        .iter()
        .flat_map(|log| log.split_inclusive(|byte| *byte == b'\n'));
    lines
        .filter_map(|line| toshokan::log::Entry::parse(line).ok())
        .collect()
}

/// Each file fact the logs in `root`'s copy give, by path and identity.
fn logged_facts(root: &Temp) -> BTreeSet<(String, toshokan::Identity)> {
    let entries = logged_entries(root);
    let ops = entries.iter().flat_map(|entry| match entry.kind() {
        EntryKind::Intent(logged) => logged.ops.clone(),
        EntryKind::Bind(bound) => bound.ops.clone(),
        _ => Vec::new(),
    });
    let facts = ops.filter_map(|op| match op {
        Op::File { file, .. } | Op::Path { file, .. } => file,
        _ => None,
    });
    facts
        .map(|fact| (fact.path.to_string(), fact.identity))
        .collect()
}

/// The leaves of the paths whose entries differ from `before` to `after`: every one,
/// those of files whose bytes changed, and those no longer there.
fn compare(before: &Disk, after: &Disk) -> [BTreeSet<String>; 3] {
    let mut sets: [BTreeSet<String>; 3] = Default::default();
    let paths: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    for path in paths {
        let (was, is) = (before.get(path), after.get(path));
        if was == is {
            continue;
        }
        let name = leaf(path).to_string();
        if matches!((was, is), (Some(Entry::File(_)), Some(Entry::File(_)))) {
            sets[1].insert(name.clone());
        }
        if is.is_none() {
            sets[2].insert(name.clone());
        }
        sets[0].insert(name);
    }
    sets
}

fn disk(root: &Temp) -> Disk {
    library(&tree(&root.0))
}

/// The tags `session` shows on each entity.
fn entities_worn(session: &Session) -> BTreeMap<toshokan::EntityId, BTreeSet<String>> {
    let rows = shown(session).rows.into_values();
    let worn = rows.filter_map(|row| Some((row.entity?, row.worn)));
    worn.collect()
}

/// The library files and folders of `tree`.
fn library(tree: &Tree) -> Disk {
    let library = tree.iter().filter(|(path, _)| !own(path));
    library
        .map(|(path, item)| (path.clone(), item.entry.clone()))
        .collect()
}

/// What one drawbar shows present: each file with its tags, the folders, the hidden
/// files, the tags, and the tagged rows whose file is gone, a row listed as missing among
/// them.
#[derive(Clone, PartialEq, Debug)]
struct Seen {
    files: BTreeMap<String, Vec<String>>,
    folders: Vec<String>,
    hidden: Vec<String>,
    tags: Vec<String>,
    lost: Vec<String>,
}

fn seen(session: &Session) -> Seen {
    let folders = &session.bench.browser.folders;
    let listed = session.bench.workspace.listed();
    let files = (listed.filter(|entity| !missing(session, entity.id))).map(|entity| {
        let path = session.path(entity.id);
        let path = path.unwrap_or_else(|| format!("{} (no file)", entity.name));
        (path, session.worn(entity.id))
    });
    let missing = (session.bench.workspace.listed())
        .filter(|entity| missing(session, entity.id) && !session.worn(entity.id).is_empty())
        .filter_map(|entity| session.path(entity.id));
    let lost = folders.lost().iter().map(|l| l.path.to_string());
    let lost: BTreeSet<String> = lost.chain(missing).collect();
    Seen {
        files: files.collect(),
        folders: folders.all().iter().map(|f| f.path.to_string()).collect(),
        hidden: folders.others.iter().map(LibPath::to_string).collect(),
        tags: session.tag_names(),
        lost: lost.into_iter().collect(),
    }
}

/// Whether `id` is listed as missing: a row whose file went, which is not shown present.
fn missing(session: &Session, id: u64) -> bool {
    session.bench.browser.folders.missing.contains(&id)
}

/// The names a log line or a question is about: each one quoted, and what comes before
/// a colon that starts it.
fn names(text: &str) -> Vec<&str> {
    let quoted = text
        .split('“')
        .skip(1)
        .filter_map(|rest| rest.split_once('”'));
    let mut names: Vec<&str> = quoted.map(|(name, _)| name).collect();
    if let Some((first, _)) = text
        .split_once(": ")
        .filter(|(first, _)| !first.contains('“'))
    {
        names.push(first);
    }
    names
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

/// How two stores' views differ, field by field. Each view numbers its rows itself, so
/// rows compare by what they show.
fn differences(ours: &Shown, fresh: &Shown) -> Vec<String> {
    let fields = [
        (
            "files",
            apart(rows(ours, Class::File), rows(fresh, Class::File)),
        ),
        (
            "hidden",
            apart(rows(ours, Class::Other), rows(fresh, Class::Other)),
        ),
        (
            "lost",
            apart(rows(ours, Class::Lost), rows(fresh, Class::Lost)),
        ),
        (
            "absent",
            apart(rows(ours, Class::Absent), rows(fresh, Class::Absent)),
        ),
        ("folders", apart(&ours.dirs, &fresh.dirs)),
        ("tag names", apart(&ours.names, &fresh.names)),
    ];
    let differ = fields
        .into_iter()
        .filter_map(|(field, d)| Some((field, d?)));
    differ.map(|(field, d)| format!("{field}: {d}")).collect()
}

/// The rows `shown` shows as `class`, each by what it shows.
fn rows(shown: &Shown, class: Class) -> impl Iterator<Item = &Row> {
    shown.class(class).map(|(_, row)| row)
}

fn shown(session: &Session) -> Shown {
    session.store.shown().cloned().unwrap_or_default()
}

/// Where the walk's library is.
enum Folders {
    Shared(Temp),
    Synced { sync: Sync, chaos: bool },
}

impl Folders {
    /// The folder drawbar `who` opened.
    fn root(&self, who: usize) -> &Temp {
        match self {
            Folders::Shared(root) => root,
            Folders::Synced { sync, .. } => sync.root(who),
        }
    }

    /// Which folder drawbar `who` opened, in the order of [`Folders::root`].
    fn of(&self, who: usize) -> usize {
        match self {
            Folders::Shared(_) => 0,
            Folders::Synced { .. } => who,
        }
    }
}

/// A fresh open of a folder, with new local state, and the folder as it opened it.
struct Opened {
    folder: Tree,
    session: Session,
    _local: Temp,
}

/// The files drawbars changed since sync last settled, or since a drawbar made an unsaved
/// edit of it still held: each by every name the walk saw it under, with the drawbars that
/// changed it.
#[derive(Default)]
struct Contested(Vec<(BTreeSet<String>, BTreeSet<usize>)>);

impl Contested {
    /// Drawbar `who` changed the file it knew as `names`.
    fn changed(&mut self, who: usize, names: BTreeSet<String>) {
        self.join(names, BTreeSet::from([who]));
    }

    /// A drawbar shows a file it showed as `was` as `is` now: one file, by either name.
    fn moved(&mut self, was: String, is: String) {
        let names = BTreeSet::from([was, is]);
        if self.0.iter().any(|(known, _)| !known.is_disjoint(&names)) {
            self.join(names, BTreeSet::new());
        }
    }

    fn join(&mut self, mut names: BTreeSet<String>, mut by: BTreeSet<usize>) {
        self.0.retain(|(known, changers)| {
            let same = !known.is_disjoint(&names);
            if same {
                names.extend(known.iter().cloned());
                by.extend(changers);
            }
            !same
        });
        self.0.push((names, by));
    }

    /// Sync settled: what stays is each file a drawbar still holds an unsaved edit of, by
    /// `held`, which another drawbar's change meets as a true conflict until it is saved.
    fn settled(&mut self, held: Vec<(usize, String)>) {
        self.0
            .retain(|(known, _)| held.iter().any(|(_, at)| known.contains(at)));
        for (who, at) in held {
            self.changed(who, BTreeSet::from([at]));
        }
    }

    /// Whether two or more drawbars changed a file named `name`, by its path or its leaf.
    fn names(&self, name: &str) -> bool {
        self.0
            .iter()
            .any(|(known, by)| by.len() > 1 && named(known, name))
    }
}

/// Whether `name` is a path of `known` or the leaf of one.
fn named(known: &BTreeSet<String>, name: &str) -> bool {
    known.iter().any(|path| path == name || leaf(path) == name)
}

/// A file sync put at a path after the drawbar read the entry naming it: its bytes, and
/// the bytes the path held before.
type Late = (Vec<u8>, Vec<u8>);

/// What a drawbar holds: each asset's path, tags and bytes, and each folder's path.
type Holdings = BTreeMap<(bool, u64), (Option<String>, Vec<String>, Vec<u8>)>;

fn holdings(session: &Session) -> Holdings {
    let assets = session.bench.workspace.listed().map(|entity| {
        let held = (
            session.path(entity.id),
            session.worn(entity.id),
            entity.bytes.to_vec(),
        );
        ((false, entity.id), held)
    });
    let folders = session.bench.browser.folders.all().iter();
    let folders = folders.map(|f| {
        (
            (true, f.id),
            (Some(f.path.to_string()), Vec::new(), Vec::new()),
        )
    });
    assets.chain(folders).collect()
}

/// The clock of each change one drawbar's Undo and Redo would reach, so the walk can tell
/// whether another drawbar has changed the library since.
#[derive(Default)]
struct History {
    done: Vec<usize>,
    undone: Vec<usize>,
}

/// An undo or redo the library may refuse, chosen when the log was `log` entries long.
struct Chosen {
    redo: bool,
    made_at: usize,
    log: usize,
}

/// What undos or redos one drawbar chose take back, where another drawbar may have
/// changed it since: the clock up to which the earliest of their changes knew every other
/// drawbar's, and the paths and tags they touched.
#[derive(Clone)]
struct Exposed {
    known: usize,
    touched: BTreeSet<String>,
}

/// A file a drawbar made or saved: where, what it holds, and the entity it was given.
#[derive(Clone)]
struct Wrote {
    path: String,
    bytes: Vec<u8>,
    entity: Option<toshokan::EntityId>,
}

/// An undo or redo of the change at `made_at` the library refused and kept, as `why` says.
struct Kept {
    redo: bool,
    made_at: usize,
    why: String,
}

/// An edit the walk made and has not saved: its gain and the bytes it left.
struct Edit {
    gain: usize,
    bytes: Vec<u8>,
}

struct Walk {
    world: World,
    seed: u64,
    dice: Dice,
    times: Times,
    folders: Folders,
    _locals: Vec<Temp>,
    drawbars: Vec<Session>,
    /// The last fresh open of each folder.
    fresh: BTreeMap<usize, Opened>,
    /// Whether each drawbar has read every change since it last settled.
    current: Vec<bool>,
    /// Whether sync changed a library file in each drawbar's copy since it last
    /// rescanned that no log entry it has yet to read names, which a refresh does not
    /// see.
    unlooked: Vec<bool>,
    /// Whether sync changed a library file in each drawbar's copy that a log entry it
    /// has yet to read names, which the refresh that reads it looks at.
    awaiting: Vec<bool>,
    history: Vec<History>,
    /// Each drawbar's undo or redo, until it settles.
    chosen: Vec<Option<Chosen>>,
    /// The clock of the last change each drawbar made.
    changed: Vec<usize>,
    /// Where each drawbar has chosen an undo or redo another drawbar may have changed
    /// something under since, whose answers have not all come.
    refusable: Vec<Option<Exposed>>,
    /// Each change a drawbar made: its clock, the drawbar, and the paths and tags it
    /// touched.
    changes: Vec<(usize, usize, BTreeSet<String>)>,
    /// The paths the action under way gave a folder before the name it ends with.
    passed: BTreeSet<String>,
    /// Changes made so far.
    clock: usize,
    /// The clock at each settle of sync.
    settled: Vec<usize>,
    /// How many of each drawbar's log entries have been looked at.
    vetted: Vec<usize>,
    /// The names of the files other drawbars wrote, moved or deleted, or sync brought,
    /// since each drawbar last came back.
    theirs: Vec<BTreeSet<String>>,
    /// Of those, the names of the files whose bytes another drawbar or sync changed in
    /// each drawbar's folder.
    rewritten: Vec<BTreeSet<String>>,
    /// Of those, the names of the files another drawbar or sync took from each drawbar's
    /// folder, deleted or moved.
    removed: Vec<BTreeSet<String>>,
    /// The names of the files another drawbar deleted from its own folder since sync
    /// last settled.
    trashed: Vec<BTreeSet<String>>,
    /// The names of the files whose tags another drawbar changed since sync last
    /// settled.
    retagged: Vec<BTreeSet<String>>,
    /// The paths at which sync put a file of other bytes but the length and time of the
    /// one it replaced in each drawbar's copy, which the drawbar sees only through the log
    /// entry that names it.
    unseen: Vec<BTreeSet<String>>,
    /// Of those, the ones sync put there after the drawbar read the log entry naming
    /// them, with their bytes and the bytes before: the drawbar may show the bytes before
    /// until the file next changes or a save of it is tried, which is refused and asks.
    late: Vec<BTreeMap<String, Late>>,
    /// The file facts the logs in each drawbar's copy gave when it last looked, by path
    /// and identity.
    read_facts: Vec<BTreeSet<(String, toshokan::Identity)>>,
    /// Each name a drawbar gave what it showed under another, with that name.
    renames: BTreeSet<(String, String)>,
    /// The drawbars whose action left an asset holding each run of bytes.
    held: BTreeMap<Vec<u8>, BTreeSet<usize>>,
    /// Every name each drawbar has shown each asset and folder under.
    known: Vec<BTreeMap<(bool, u64), BTreeSet<String>>>,
    /// What each file whose tags another drawbar changed since sync last settled held.
    retagged_bytes: Vec<BTreeSet<Vec<u8>>>,
    /// The tags each drawbar showed on each entity when its log was last vetted.
    worn: Vec<BTreeMap<toshokan::EntityId, BTreeSet<String>>>,
    /// The paths of the tagged files each drawbar showed when its log was last vetted.
    tagged: Vec<BTreeSet<String>>,
    /// Each drawbar's undo or redo the library refused and kept, until the walk tries it
    /// again.
    kept: Vec<Option<Kept>>,
    /// The names of what the walk moved out of the way of a kept undo or redo since sync
    /// last settled.
    aside: BTreeSet<String>,
    /// Each drawbar's unsaved edits, by asset.
    unsaved: Vec<BTreeMap<u64, Edit>>,
    /// Each file a drawbar made or saved, by the drawbar and the clock of the change.
    wrote: BTreeMap<(usize, usize), Wrote>,
    /// The files each drawbar's undos or redos take back made or saved, until their
    /// answers have all come.
    reversing: Vec<Vec<Wrote>>,
    contested: Contested,
    /// Where each drawbar last showed each asset and folder.
    at: Vec<BTreeMap<(bool, u64), String>>,
    /// Whether a chaos event or a conflict touched each drawbar's copy since it last came
    /// back.
    odd: Vec<bool>,
    steps: Vec<String>,
    named: usize,
}

impl Walk {
    fn new(world: World, count: usize, seed: u64) -> Walk {
        let times = Times::new(seed);
        let first = Temp::new();
        let program = Fresh::Program.bytes().unwrap();
        fs::create_dir(first.at("Live")).unwrap();
        for (path, gain) in [("Live/Grand.ne5p", "12"), ("Strings.ne5p", "36")] {
            fs::write(first.at(path), with_gain(&program, gain)).unwrap();
            times.date(&first.at(path));
        }
        let folders = match world {
            World::Shared => Folders::Shared(first),
            World::Synced { chaos } => Folders::Synced {
                sync: Sync::new(first, count, times.clone()),
                chaos,
            },
        };
        let locals: Vec<Temp> = (1..count).map(|_| Temp::new()).collect();
        let root = folders.root(0);
        let mut drawbars = vec![Session::ambient(root, &root.local(), times.ambient())];
        drawbars[0].read_all();
        let others = (1..count)
            .map(|who| Session::ambient(folders.root(who), &locals[who - 1].0, times.ambient()));
        drawbars.extend(others);
        Walk {
            world,
            seed,
            dice: Dice(seed),
            times,
            folders,
            _locals: locals,
            drawbars,
            fresh: BTreeMap::new(),
            current: vec![true; count],
            unlooked: vec![false; count],
            awaiting: vec![false; count],
            history: (0..count).map(|_| History::default()).collect(),
            chosen: (0..count).map(|_| None).collect(),
            changed: vec![0; count],
            refusable: vec![None; count],
            changes: Vec::new(),
            passed: BTreeSet::new(),
            clock: 0,
            settled: Vec::new(),
            vetted: vec![0; count],
            theirs: vec![BTreeSet::new(); count],
            rewritten: vec![BTreeSet::new(); count],
            removed: vec![BTreeSet::new(); count],
            trashed: vec![BTreeSet::new(); count],
            retagged: vec![BTreeSet::new(); count],
            known: vec![BTreeMap::new(); count],
            held: BTreeMap::new(),
            renames: BTreeSet::new(),
            unseen: vec![BTreeSet::new(); count],
            late: vec![BTreeMap::new(); count],
            read_facts: vec![BTreeSet::new(); count],
            tagged: vec![BTreeSet::new(); count],
            retagged_bytes: vec![BTreeSet::new(); count],
            worn: vec![BTreeMap::new(); count],
            unsaved: (0..count).map(|_| BTreeMap::new()).collect(),
            wrote: BTreeMap::new(),
            reversing: vec![Vec::new(); count],
            odd: vec![false; count],
            kept: (0..count).map(|_| None).collect(),
            aside: BTreeSet::new(),
            contested: Contested::default(),
            at: vec![BTreeMap::new(); count],
            steps: Vec::new(),
            named: 0,
        }
    }

    fn run(mut self) {
        for _ in 0..self.world.steps() {
            self.step();
        }
        match self.folders {
            Folders::Shared(_) => {
                for who in 0..self.drawbars.len() {
                    let refocus = self.dice.one_in(2);
                    self.come_back(who, refocus);
                }
            }
            Folders::Synced { .. } => self.settle(),
        }
        for fresh in std::mem::take(&mut self.fresh).into_values() {
            fresh.session.close();
        }
        for session in self.drawbars {
            session.close();
        }
    }

    fn fail(&self, broke: Broke, what: &str, details: String) -> ! {
        let steps: Vec<String> = (self.steps.iter().enumerate())
            .map(|(n, step)| format!("  {}. {step}", n + 1))
            .collect();
        panic!(
            "{:?} with {} drawbars, seed {}, step {}: {}: {what}\n  {details}\nsteps:\n{}",
            self.world,
            self.drawbars.len(),
            self.seed,
            self.steps.len(),
            broke.says(),
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

    fn others(&self, who: usize) -> impl Iterator<Item = usize> {
        (0..self.drawbars.len()).filter(move |other| *other != who)
    }

    fn step(&mut self) {
        self.times.pass();
        let syncs = matches!(self.folders, Folders::Synced { .. });
        if syncs && self.dice.one_in(2) && self.sync_step() {
            return;
        }
        let who = self.dice.below(self.drawbars.len());
        if !self.current[who] {
            let refocus = self.dice.one_in(2);
            self.come_back(who, refocus);
        }
        let root = self.folders.root(who);
        let before = (disk(root), seen(&self.drawbars[who]));
        let (shown_before, logged) = (shown(&self.drawbars[who]), logged(root));
        let held = holdings(&self.drawbars[who]);
        let log = self.drawbars[who].bench.log.len();
        let late = self.late_shown(who);
        let Done {
            made,
            hurried,
            shows,
            edited,
            wrote,
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
        let shared = matches!(self.folders, Folders::Shared(_));
        if shared && made != Made::Nothing {
            for other in self.others(who).collect::<Vec<_>>() {
                self.current[other] = false;
            }
        }
        match hurried {
            true => self.current[who] = false,
            false => {
                self.drawbars[who].sync();
                self.kept_late(who, late, log);
                self.learn_whether_refused(who);
                self.current[who] = self.looked(who);
            }
        }
        if let Some((id, gain)) = edited {
            self.track_edit(who, id, gain);
        }
        if let Some(id) = wrote.filter(|_| made == Made::One && !hurried) {
            self.note_written(who, id);
        }
        self.changed(who, &held);
        self.check();
        let clock = self.clock;
        // A change refused over what sync changed unseen shows nothing of itself.
        let refused = !hurried && {
            let mut since = self.drawbars[who].bench.log.iter().skip(log);
            since.any(|entry| {
                entry.text.contains("did not change as asked") && self.accounted(who, entry)
            })
        };
        if !hurried {
            self.refusable[who] = None;
            self.reversing[who].clear();
            if !refused {
                self.shows_its_effect(who, &shows);
            }
            self.answer_questions(who);
            self.clear_the_way(who);
        }
        // An answer that made a change of its own, as Keep both, is what Undo takes back.
        let last = self.clock == clock;
        if made == Made::One && !hurried && !refused && self.dice.one_in(3) && last {
            self.undo_and_redo(who, before.clone(), &shown_before, &logged);
        }
        self.trashes(who, &before.0);
        match shared {
            true => {
                let [touched, rewritten, removed] =
                    compare(&before.0, &disk(self.folders.root(who)));
                for other in self.others(who).collect::<Vec<_>>() {
                    self.theirs[other].extend(touched.iter().cloned());
                    self.rewritten[other].extend(rewritten.iter().cloned());
                    self.removed[other].extend(removed.iter().cloned());
                }
            }
            false => self.record(),
        }
    }

    /// One step of sync where some copy is behind: one path delivered to one copy, a
    /// chaos event where the walk has chaos, or now and then every delivery at once.
    /// Whether there was anything to do.
    fn sync_step(&mut self) -> bool {
        let Folders::Synced { sync, chaos } = &self.folders else {
            return false;
        };
        let chaotic = *chaos && self.dice.one_in(5);
        let offered = (0..self.drawbars.len()).map(|copy| match chaotic {
            true => (copy, sync.chaos(copy)),
            false => (copy, sync.deliveries(copy)),
        });
        let offered: Vec<(usize, Vec<SyncEvent>)> =
            offered.filter(|(_, events)| !events.is_empty()).collect();
        let Some((copy, events)) = self.dice.pick(&offered) else {
            return false;
        };
        if !chaotic && self.dice.one_in(8) {
            self.settle();
            return true;
        }
        let event = self.dice.pick(&events).expect("a copy offered is behind");
        let Folders::Synced { sync, .. } = &mut self.folders else {
            unreachable!("the walk syncs");
        };
        let before = tree(&sync.root(copy).0);
        sync.apply(copy, &event);
        self.arrived(copy, &event, chaotic);
        self.brought(copy, &before);
        self.said(format!("in {}'s copy, {}", name(copy), described(&event)));
        self.check();
        true
    }

    /// Whether log files sync has yet to deliver to drawbar `copy`'s copy.
    fn behind_on_logs(&self, copy: usize) -> bool {
        let Folders::Synced { sync, .. } = &self.folders else {
            return false;
        };
        sync.deliveries(copy).iter().any(|event| match event {
            SyncEvent::Deliver { path } | SyncEvent::Delete { path } => own(path.as_str()),
            _ => false,
        })
    }

    /// Note what drawbar `who` sees of its copy once it rescans, or refreshes: a refresh
    /// reads every log entry delivered, and looks again at the paths they name.
    fn looking(&mut self, who: usize, rescans: bool) {
        self.unlooked[who] &= !rescans;
        self.awaiting[who] &= !rescans && self.behind_on_logs(who);
    }

    /// Whether drawbar `who` has looked at every change sync made to its copy.
    fn looked(&self, who: usize) -> bool {
        !self.unlooked[who] && !self.awaiting[who]
    }

    /// What drawbar `copy` may say once it reads what `event` did to its copy: that a file
    /// another drawbar saved changed on disk, and, for chaos or a conflict, anything.
    fn arrived(&mut self, copy: usize, event: &SyncEvent, chaotic: bool) {
        let Folders::Synced { sync, .. } = &self.folders else {
            return;
        };
        self.current[copy] = false;
        let (SyncEvent::Deliver { path } | SyncEvent::Delete { path }) = event else {
            self.odd[copy] |= chaotic;
            self.unlooked[copy] = true;
            return;
        };
        if !own(path.as_str()) {
            let since = self.settled.last().copied().unwrap_or(0);
            let logged = (self.changes.iter())
                .filter(|(clock, by, _)| *clock > since && *by != copy)
                .any(|(.., touched)| touched.contains(path.as_str()));
            match logged && self.behind_on_logs(copy) {
                true => self.awaiting[copy] = true,
                false => self.unlooked[copy] = true,
            }
        }
        self.odd[copy] |= chaotic || sync.strange(path.as_str());
        if matches!(event, SyncEvent::Deliver { .. }) && !own(path.as_str()) {
            self.theirs[copy].insert(leaf(path.as_str()).to_string());
        }
    }

    /// What each asset of drawbar `who` holding an unsaved edit was saved as.
    fn bases(&self, who: usize) -> Vec<(u64, Vec<u8>)> {
        let workspace = &self.drawbars[who].bench.workspace;
        let held = self.unsaved[who].keys().filter_map(|id| workspace.get(*id));
        held.map(|entity| (entity.id, entity.saved.bytes().to_vec()))
            .collect()
    }

    /// Drawbar `who` asks what to do with each unsaved edit whose file another drawbar
    /// rewrote, once what it was saved as moves to that drawbar's bytes: it asked, by the
    /// asset's name, whether `bases` say the edit was over other bytes before, since its
    /// log was `log` entries long.
    fn asked_over_edits(&self, who: usize, bases: Vec<(u64, Vec<u8>)>, log: usize) {
        let session = &self.drawbars[who];
        let said = session
            .bench
            .log
            .iter()
            .skip(log)
            .map(|entry| entry.text.as_str());
        let said = said.filter(|text| text.contains("changed on disk while"));
        let asked: Vec<&str> = session.bench.browser.questions().chain(said).collect();
        for (id, was) in bases {
            let Some(entity) = session.bench.workspace.get(id).filter(|e| e.is_unsaved()) else {
                continue;
            };
            let moved = entity.saved.bytes().to_vec() != was;
            let named = [entity.name.as_str()];
            let theirs = self.holds_theirs(who, &named)
                && self
                    .known_as(who, named[0])
                    .iter()
                    .any(|n| self.rewritten[who].contains(n));
            let asked = asked.iter().any(|text| names(text).contains(&named[0]));
            if moved && theirs && !asked {
                let what = format!(
                    "{} took another drawbar's save under the edit of {}",
                    name(who),
                    entity.name
                );
                self.fail(Broke::Unsaved, &what, "nothing asked whose to keep".into());
            }
        }
    }

    /// Note each file drawbar `who` deleted from its own folder, which held `before`: a
    /// path gone whose bytes fewer files hold now, as a move leaves them somewhere.
    fn trashes(&mut self, who: usize, before: &Disk) {
        let after = disk(self.folders.root(who));
        let count = |disk: &Disk, bytes: &Vec<u8>| {
            let files = disk
                .values()
                .filter(|entry| matches!(entry, Entry::File(b) if b == bytes));
            files.count()
        };
        let gone = before.iter().filter(|(path, entry)| match entry {
            Entry::File(bytes) => {
                !after.contains_key(*path) && count(&after, bytes) < count(before, bytes)
            }
            Entry::Folder => false,
        });
        let gone: Vec<String> = gone.map(|(path, _)| leaf(path).to_string()).collect();
        for other in self.others(who).collect::<Vec<_>>() {
            self.trashed[other].extend(gone.iter().cloned());
        }
    }

    /// Note the files whose bytes sync changed in drawbar `copy`'s copy, which held
    /// `before`, and those it took away.
    fn brought(&mut self, copy: usize, before: &Tree) {
        let now = tree(&self.folders.root(copy).0);
        let [_, rewritten, removed] = compare(&library(before), &library(&now));
        self.rewritten[copy].extend(rewritten);
        self.removed[copy].extend(removed);
        let alike = |path: &String, item: &sync::Item| {
            let was = before.get(path).filter(|was| was.modified == item.modified);
            match (was.map(|was| &was.entry), &item.entry) {
                (Some(Entry::File(old)), Entry::File(new)) => old != new && old.len() == new.len(),
                _ => false,
            }
        };
        let unseen: Vec<(&String, &sync::Item)> = now
            .iter()
            .filter(|(path, item)| !own(path) && alike(path, item))
            .collect();
        self.unseen[copy].extend(unseen.iter().map(|(path, _)| (*path).clone()));
        let file = |tree: &Tree, path: &String| match tree.get(path).map(|item| &item.entry) {
            Some(Entry::File(bytes)) => Some(bytes.clone()),
            _ => None,
        };
        self.late[copy].retain(|path, (bytes, _)| file(&now, path).as_ref() == Some(bytes));
        for (path, _) in unseen {
            let (Some(bytes), Some(was)) = (file(&now, path), file(before, path)) else {
                continue;
            };
            let at = LibPath::parse(path);
            let identity = at.and_then(|at| crate::store::keys::identity_of_bytes(&at, &bytes));
            let read =
                identity.is_some_and(|id| self.read_facts[copy].contains(&(path.clone(), id)));
            if read {
                self.late[copy].insert(path.clone(), (bytes, was));
            }
        }
    }

    /// What the asset drawbar `who` shows at `path` was saved as, where it read it.
    fn saved_at(&self, who: usize, path: &str) -> Option<Vec<u8>> {
        let session = &self.drawbars[who];
        let mut listed = session.bench.workspace.listed();
        let at = listed.find(|entity| session.path(entity.id).as_deref() == Some(path));
        let saved = at
            .filter(|entity| !entity.unread())
            .map(|e| e.saved.bytes().to_vec());
        saved.filter(|saved| !saved.is_empty())
    }

    /// Each path [`Walk::late`] holds for drawbar `who`, with the bytes there and what
    /// the asset `who` shows there was saved as.
    fn late_shown(&self, who: usize) -> Vec<(String, Vec<u8>, Option<Vec<u8>>)> {
        let late = self.late[who].iter();
        late.map(|(path, (bytes, _))| (path.clone(), bytes.clone(), self.saved_at(who, path)))
            .collect()
    }

    /// Fail where drawbar `who` saved an edit over a file sync rewrote after it read the
    /// log entry naming the rewrite, where `shown` says it showed other bytes there, or
    /// where such a save, refused since its log was `said` entries long, asked nothing.
    /// A refused save ends the drawbar's showing the bytes before.
    fn kept_late(
        &mut self,
        who: usize,
        shown: Vec<(String, Vec<u8>, Option<Vec<u8>>)>,
        said: usize,
    ) {
        let on_disk = disk(self.folders.root(who));
        for (path, bytes, before) in shown {
            let Some(Entry::File(held)) = on_disk.get(&path) else {
                continue;
            };
            let wrote = *held != bytes && self.saved_at(who, &path).as_ref() == Some(held);
            if wrote && before.is_some_and(|before| before != bytes) {
                let what = format!("{} saved over {path}", name(who));
                let details = "another drawbar's rewrite it never showed".to_string();
                self.fail(Broke::Unsaved, &what, details);
            }
            let session = &self.drawbars[who];
            let about = |text: &str| names(text).contains(&leaf(&path));
            let since = session.bench.log.iter().skip(said);
            let refused = since
                .filter(|entry| {
                    entry
                        .text
                        .contains("was not saved, because it changed on disk")
                })
                .any(|entry| about(&entry.text));
            if !refused {
                continue;
            }
            if !session.bench.browser.questions().any(about) {
                let what = format!("{} was refused a save over {path}", name(who));
                self.fail(Broke::Unsaved, &what, "and asked nothing".to_string());
            }
            self.late[who].remove(&path);
        }
    }

    /// Note each file, folder and tag drawbar `who` changed since it held `before`: what
    /// other drawbars may change too before sync settles, and what an undo of another
    /// drawbar's may be refused over.
    fn changed(&mut self, who: usize, before: &Holdings) {
        self.retags(who, before);
        let after = holdings(&self.drawbars[who]);
        for (key, (.., bytes)) in &after {
            if before.get(key).is_none_or(|(.., was)| was != bytes) {
                self.held.entry(bytes.clone()).or_default().insert(who);
            }
        }
        let keys: BTreeSet<&(bool, u64)> = before.keys().chain(after.keys()).collect();
        let mut touched = BTreeSet::new();
        for key in keys {
            let (was, is) = (before.get(key), after.get(key));
            if was == is {
                continue;
            }
            let held = [was, is].into_iter().flatten();
            let names: BTreeSet<String> =
                held.clone().filter_map(|(path, ..)| path.clone()).collect();
            let worn = held.map(|(_, worn, _)| worn.iter().cloned().collect::<BTreeSet<_>>());
            let worn: Vec<BTreeSet<String>> = worn.collect();
            let retagged = match worn.as_slice() {
                [was, is] => was.symmetric_difference(is).cloned().collect(),
                [one] => one.clone(),
                _ => BTreeSet::new(),
            };
            touched.extend(names.iter().cloned().chain(retagged));
            if let (Some((Some(was), ..)), Some((Some(is), ..))) = (was, is) {
                self.renames
                    .insert((leaf(was).to_string(), leaf(is).to_string()));
            }
            if matches!(self.folders, Folders::Synced { .. }) {
                self.contested.changed(who, names);
            }
        }
        touched.append(&mut self.passed);
        if !touched.is_empty() {
            self.changes.push((self.clock, who, touched));
        }
    }

    /// The paths and tags the change drawbar `who` made at the clock `made_at` touched.
    fn touched_by(&self, who: usize, made_at: usize) -> BTreeSet<String> {
        let mut made =
            (self.changes.iter()).filter(|(clock, by, _)| *by == who && *clock >= made_at);
        made.next()
            .map(|(.., touched)| touched.clone())
            .unwrap_or_default()
    }

    /// Whether a drawbar other than `who` touched what `name` names, what is inside it or
    /// a folder holding it, after the clock `known`, under that name or one a change of
    /// `who`'s since gave it.
    fn touched_since(&self, known: usize, who: usize, name: &str) -> bool {
        let since: Vec<&(usize, usize, BTreeSet<String>)> = self
            .changes
            .iter()
            .filter(|(clock, ..)| *clock > known)
            .collect();
        let meets = |touched: &BTreeSet<String>, names: &BTreeSet<String>| {
            let within = |inner: &str, outer: &str| inner.starts_with(&format!("{outer}/"));
            names.iter().any(|name| {
                named(touched, name) || touched.iter().any(|t| within(t, name) || within(name, t))
            })
        };
        let mut names = BTreeSet::from([name.to_string()]);
        loop {
            let had = names.len();
            for (_, by, touched) in &since {
                if *by == who && meets(touched, &names) {
                    names.extend(touched.iter().cloned());
                }
            }
            if names.len() == had {
                break;
            }
        }
        since
            .iter()
            .any(|(_, by, touched)| *by != who && meets(touched, &names))
    }

    /// Take in what each drawbar wrote to its copy, as sync's next scan does.
    fn record(&mut self) {
        let Folders::Synced { sync, .. } = &mut self.folders else {
            return;
        };
        let copies = self.drawbars.len();
        let recorded: Result<Vec<Vec<String>>, String> =
            (0..copies).map(|copy| sync.record(copy)).collect();
        match recorded {
            Ok(notes) => self.steps.extend(notes.into_iter().flatten()),
            Err(broken) => self.fail(Broke::Model, "a copy changed another's log", broken),
        }
    }

    /// Deliver everything everywhere, with no chaos, and have every drawbar come back to
    /// its window, until no drawbar changes its copy: then every drawbar shows what every
    /// other does and what every copy holds.
    fn settle(&mut self) {
        for round in 0.. {
            self.record();
            let Folders::Synced { sync, .. } = &self.folders else {
                return;
            };
            let behind: Vec<usize> = (0..self.drawbars.len())
                .filter(|copy| !sync.deliveries(*copy).is_empty())
                .collect();
            if round > 0 && behind.is_empty() {
                break;
            }
            if round == ROUNDS {
                let names: Vec<char> = behind.iter().map(|copy| name(*copy)).collect();
                let details = format!("after {ROUNDS} rounds, {names:?} still lag");
                self.fail(Broke::Convergence, "sync does not settle", details);
            }
            let Folders::Synced { sync, .. } = &mut self.folders else {
                return;
            };
            let delivered: Vec<(usize, Tree, Vec<SyncEvent>)> = (behind.into_iter())
                .map(|copy| (copy, tree(&sync.root(copy).0), sync.settle(copy)))
                .collect();
            for (copy, before, events) in delivered {
                for event in events {
                    self.arrived(copy, &event, false);
                }
                self.brought(copy, &before);
            }
            self.settled.push(self.clock);
            self.said("sync delivers everything everywhere".to_string());
            for who in 0..self.drawbars.len() {
                self.come_back(who, true);
            }
        }
        if let Folders::Synced { sync, .. } = &mut self.folders {
            sync.settled();
        }
        // Another drawbar's tags reach this one in its log alone, once it has sent them.
        self.retagged.iter_mut().for_each(BTreeSet::clear);
        self.retagged_bytes.iter_mut().for_each(BTreeSet::clear);
        self.trashed.iter_mut().for_each(BTreeSet::clear);
        self.agree();
        for who in 0..self.drawbars.len() {
            if let Some(kept) = self.kept[who].take() {
                self.again(who, kept, false);
            }
        }
        self.aside.clear();
        let held = (self.unsaved.iter().enumerate()).flat_map(|(who, edits)| {
            let session = &self.drawbars[who];
            let paths = edits.keys().filter_map(|id| session.path(*id));
            paths.map(move |path| (who, path))
        });
        self.contested.settled(held.collect());
    }

    /// Every drawbar shows what drawbar A shows and nothing missing but for an unsaved
    /// edit, and every copy holds what A's does.
    fn agree(&self) {
        for who in 0..self.drawbars.len() {
            self.nothing_missing(who, Broke::Convergence);
        }
        let first = &self.drawbars[0];
        let on_disk = disk(self.folders.root(0));
        for who in 1..self.drawbars.len() {
            let session = &self.drawbars[who];
            let what = format!("A and {} show different libraries", name(who));
            let differ = seen_apart(&seen(first), &seen(session));
            if !differ.is_empty() {
                let d = format!("(A first)\n  {}", differ.join("\n  "));
                self.fail(Broke::Convergence, &what, d);
            }
            let differ = differences(&shown(first), &shown(session));
            if !differ.is_empty() {
                let d = format!("(A's view first)\n  {}", differ.join("\n  "));
                self.fail(Broke::Convergence, &what, d);
            }
            if let Some(d) = apart(&on_disk, &disk(self.folders.root(who))) {
                let what = format!("A's and {}'s copies differ", name(who));
                self.fail(Broke::Convergence, &what, format!("(A's first)\n  {d}"));
            }
        }
    }

    /// The drawbar `who` comes back to its window, or asks for a refresh, and shows the
    /// library as a fresh open does once that answers.
    fn come_back(&mut self, who: usize, refocus: bool) {
        let held = disk(self.folders.root(who));
        let late = self.late_shown(who);
        let bases = self.bases(who);
        let log = self.drawbars[who].bench.log.len();
        let session = &mut self.drawbars[who];
        match refocus {
            true => session.refocus(),
            false => session.refresh(),
        }
        self.looking(who, refocus);
        self.read_facts[who] = logged_facts(self.folders.root(who));
        let how = ["refreshes", "comes back to its window"][usize::from(refocus)];
        self.said(format!("{} {how}", name(who)));
        self.vet(who);
        if refocus {
            self.show_a_fresh_open(self.folders.of(who), &[who], "once its rescan answered");
        }
        self.drawbars[who].sync();
        self.kept_late(who, late, log);
        self.trashes(who, &held);
        self.learn_whether_refused(who);
        self.vet(who);
        self.asked_over_edits(who, bases, log);
        self.answer_questions(who);
        // What sync changed that the drawbar has not looked at may still surprise it.
        if self.looked(who) {
            self.theirs[who].clear();
            self.rewritten[who].clear();
            self.removed[who].clear();
            self.odd[who] = false;
        }
        if self.looked(who) && !self.behind_on_logs(who) {
            self.unseen[who].clear();
        }
        self.refusable[who] = None;
        self.reversing[who].clear();
        self.current[who] = self.looked(who);
        self.check();
    }

    /// Whether `entry` says that a file another drawbar saved changed on disk, that an
    /// undo or redo `who` chose while another drawbar may have changed what it takes back
    /// is let go for what another drawbar did to a file it names, that `who` keeps an
    /// unsaved edit whose file went, or names a file a chaos event or a conflict touched:
    /// a line that names no file, where one touched `who`'s copy since it last came back.
    /// A line [`CONTESTED`] allows, or such a let go, may name a file two drawbars
    /// changed since sync last settled.
    fn accounted(&self, who: usize, entry: &crate::log::Entry) -> bool {
        let text = entry.text.as_str();
        let names = names(text);
        let named = |set: &BTreeSet<String>| {
            let known = names.iter().flat_map(|name| self.known_as(who, name));
            known.into_iter().any(|name| set.contains(&name))
        };
        let saved = text.contains("changed on disk")
            && (named(&self.rewritten[who]) || self.holds_theirs(who, &names));
        let unseen = text.contains("changed on disk")
            && (self.unseen[who].iter().chain(self.late[who].keys()))
                .any(|path| text.contains(path.as_str()));
        let deleted = text.starts_with("Another drawbar deleted") && named(&self.trashed[who]);
        let moved = text.starts_with("Another drawbar moved") && self.moved_away(who, &names);
        let gone = self.tagged_gone(who, text);
        let retagged = text.contains("changed the tags of")
            && (named(&self.retagged[who]) || self.retagged_like(who, &names));
        let refused = self.refusable[who].as_ref().is_some_and(|exposed| {
            let theirs = &self.theirs[who];
            let another = |name: &str| {
                theirs.contains(name)
                    || theirs.contains(leaf(name))
                    || self.touched_since(exposed.known, who, name)
            };
            let touched = exposed.touched.iter().map(String::as_str);
            let waits = kept_for(&entry.text).is_some_and(|why| why.ends_with(NOT_AS_LEFT));
            (let_go_for_another(&entry.text) || waits)
                && names.iter().copied().chain(touched).any(another)
        });
        let session = &self.drawbars[who];
        let edited = (self.unsaved[who].keys()).filter(|id| missing(session, **id));
        let edited = edited.filter_map(|id| session.bench.workspace.get(*id));
        let edited: BTreeSet<String> = edited.map(|entity| entity.name.clone()).collect();
        let kept = text.contains("drawbar still holds your changes") && named(&edited);
        let strange = match &self.folders {
            Folders::Shared(_) => false,
            Folders::Synced { sync, .. } => match names.is_empty() {
                true => self.odd[who],
                false => names.iter().any(|name| {
                    sync.names_strange(name) || self.holds_what_chaos_wrote(who, sync, name)
                }),
            },
        };
        let held_back = kept_for(&entry.text).is_some_and(|why| self.in_the_way(who, why));
        let no_file = kept_for(&entry.text) == Some(NO_FILE);
        let aside = names.iter().any(|name| self::named(&self.aside, name));
        let found = let_go_for_another(&entry.text)
            && names.iter().any(|name| self.found_in_like_bytes(who, name));
        let apart = let_go_for_another(&entry.text)
            && names.iter().any(|name| self.adopted_apart(who, name));
        let taken = (let_go_for_another(&entry.text) || no_file)
            && (self.reversing[who].iter())
                .filter(|wrote| no_file || names.contains(&leaf(&wrote.path)))
                .any(|wrote| self.taken_for_like_bytes(who, wrote));
        let contested = CONTESTED.iter().any(|said| entry.text.contains(said));
        let conflict = (contested || let_go_for_another(&entry.text))
            && names.iter().any(|name| self.contested.names(name));
        saved
            || unseen
            || deleted
            || moved
            || gone
            || retagged
            || refused
            || kept
            || strange
            || conflict
            || held_back
            || aside
            || found
            || taken
            || apart
    }

    /// Whether `names`, of a line saying another drawbar moved a file elsewhere and it is
    /// not there yet, name a file sync took from `who`'s copy or another drawbar changed
    /// since sync last settled, and a place in that copy that holds no such file.
    fn moved_away(&self, who: usize, names: &[&str]) -> bool {
        let (Some(from), to) = (names.first(), names.get(1)) else {
            return false;
        };
        let on_disk = disk(self.folders.root(who));
        let file = |path: &str| matches!(on_disk.get(path), Some(Entry::File(_)));
        let there = match to {
            Some(to) => file(to) || file(&format!("{to}/{from}")),
            None => file(from),
        };
        let known = self.known_as(who, from);
        let since = self.before_settles();
        let moved = known.iter().any(|from| self.removed[who].contains(from))
            || self.touched_since(since, who, from);
        moved && !there
    }

    /// Whether `text` counts tagged files no longer in the folder, and `who`'s copy holds
    /// no file at the paths of at least that many of the tagged files it showed at its
    /// last vet and the tagged rows it shows lost.
    fn tagged_gone(&self, who: usize, text: &str) -> bool {
        let Some(count) = text.split(' ').next().and_then(|n| n.parse::<usize>().ok()) else {
            return false;
        };
        let on_disk = disk(self.folders.root(who));
        let lost = seen(&self.drawbars[who]).lost;
        let paths: BTreeSet<&String> = self.tagged[who].iter().chain(&lost).collect();
        let gone = paths
            .into_iter()
            .filter(|path| !on_disk.contains_key(*path));
        text.contains("no longer in the library folder") && gone.count() >= count
    }

    /// Whether the file `who`'s copy holds at `name`, or at a path whose leaf it is, holds
    /// what a chaos event wrote elsewhere too: binding, which knows a file by its contents,
    /// may take either for the other.
    fn holds_what_chaos_wrote(&self, who: usize, sync: &Sync, name: &str) -> bool {
        let on_disk = disk(self.folders.root(who));
        let mut named = on_disk
            .iter()
            .filter(|(path, _)| *path == name || leaf(path) == name);
        named.any(|(_, entry)| matches!(entry, Entry::File(bytes) if sync.chaos_wrote(bytes)))
    }

    /// Whether drawbar `who` shows at `name` an entity with no file that another drawbar
    /// shows in the file there: each adopted the file before seeing the other's entity,
    /// and an edit since leaves the file holding only one's.
    fn adopted_apart(&self, who: usize, name: &str) -> bool {
        let at = |row: &Row| row.path.as_str() == name || leaf(row.path.as_str()) == name;
        let missing = |row: &Row| row.state == toshokan::RowState::Missing;
        let rows = shown(&self.drawbars[who]).rows.into_values();
        let mut gone = rows
            .filter(|row| at(row) && missing(row))
            .filter_map(|row| row.entity);
        gone.any(|entity| {
            self.others(who).any(|other| {
                let mut rows = shown(&self.drawbars[other]).rows.into_values();
                rows.any(|row| row.entity == Some(entity) && at(&row) && !missing(&row))
            })
        })
    }

    /// Whether the entity of the file drawbar `by` `wrote` and another's were taken for
    /// one: two drawbars wrote files at one path, and a drawbar shows the other's entity
    /// there; or, as an identity is a file's contents, another file a drawbar made or
    /// saved held those bytes too, and a drawbar shows, in a file of those bytes, an
    /// entity at its path where the other was written too, or the other's entity given
    /// there, or its own entity elsewhere, in a file saved as that entity's since or
    /// holding those bytes.
    fn taken_for_like_bytes(&self, by: usize, wrote: &Wrote) -> bool {
        let theirs = (self.wrote.iter()).filter(|((who, _), other)| {
            *who != by && other.path == wrote.path && other.entity != wrote.entity
        });
        let theirs: BTreeSet<_> = theirs.filter_map(|(_, other)| other.entity).collect();
        let like: Vec<&Wrote> = (self.wrote.values())
            .filter(|other| other.bytes == wrote.bytes)
            .collect();
        let many = like.len() > 1;
        let there: Vec<&&Wrote> = like.iter().filter(|o| o.path == wrote.path).collect();
        let joined = there.len() > 1;
        let there: BTreeSet<_> = there.iter().map(|other| other.entity).collect();
        let saved_as = many
            && (self.wrote.values())
                .any(|other| other.entity == wrote.entity && other.path != wrote.path);
        (0..self.drawbars.len()).any(|who| {
            let on_disk = disk(self.folders.root(who));
            let holds = |path: &str| {
                many && matches!(on_disk.get(path), Some(Entry::File(b)) if *b == wrote.bytes)
            };
            let rows = shown(&self.drawbars[who]).rows.into_values();
            rows.into_iter().any(|row| {
                let Some(entity) = row.entity else {
                    return false;
                };
                let held = holds(row.path.as_str());
                match row.path.as_str() == wrote.path {
                    true => theirs.contains(&entity) || joined && held,
                    false if row.entity == wrote.entity => saved_as || held,
                    false => held && there.contains(&row.entity),
                }
            })
        })
    }

    /// Note the file of asset `id` that drawbar `who`'s change just made or saved.
    fn note_written(&mut self, who: usize, id: u64) {
        let session = &self.drawbars[who];
        let Some(path) = session.path(id) else {
            return;
        };
        let wrote = Wrote {
            path,
            bytes: session.bytes(id),
            entity: session.store.entity(id),
        };
        self.wrote.insert((who, self.clock), wrote);
    }

    /// Whether drawbar `who` shows the file at `name` as the file of an entity another
    /// drawbar shows missing, or elsewhere in a file of the same bytes: its file gone
    /// here, the entity was found in this file by its contents, which cannot tell its
    /// file from another saved with the same bytes.
    fn found_in_like_bytes(&self, who: usize, name: &str) -> bool {
        let file = |who: usize, path: &str| match disk(self.folders.root(who)).remove(path) {
            Some(Entry::File(bytes)) => Some(bytes),
            _ => None,
        };
        let session = &self.drawbars[who];
        let mut listed = session.bench.workspace.listed().map(|entity| entity.id);
        let here = listed.find_map(|id| {
            let path = session
                .path(id)
                .filter(|path| path == name || leaf(path) == name)?;
            Some((session.store.entity(id)?, file(who, &path)?, path))
        });
        let Some((entity, bytes, path)) = here else {
            return false;
        };
        self.others(who).any(|other| {
            let rows = shown(&self.drawbars[other]).rows.into_values();
            let mut rows = rows.filter(|row| row.entity == Some(entity));
            rows.any(|row| match row.state {
                toshokan::RowState::Missing => true,
                _ => {
                    row.path.as_str() != path
                        && file(other, row.path.as_str()) == Some(bytes.clone())
                }
            })
        })
    }

    /// Whether what `why` says is in the way of the undo or redo `who` chose is so in its
    /// copy: a folder or a file at the path it names, a file other than the one `who`
    /// holds there, or, while sync lags, no file at any path the change touched.
    fn in_the_way(&self, who: usize, why: &str) -> bool {
        let chosen = self.chosen[who].as_ref().map(|chosen| chosen.made_at);
        let Some(made_at) = chosen.or(self.kept[who].as_ref().map(|kept| kept.made_at)) else {
            return false;
        };
        let on_disk = disk(self.folders.root(who));
        if why == NO_FILE {
            let Folders::Synced { sync, .. } = &self.folders else {
                return false;
            };
            let behind = (0..self.drawbars.len()).any(|copy| !sync.deliveries(copy).is_empty());
            let touched = self.touched_by(who, made_at);
            let file = |path: &String| matches!(on_disk.get(path), Some(Entry::File(_)));
            return behind && !touched.iter().any(file);
        }
        let Some(at) = names(why).first().map(|at| at.to_string()) else {
            return false;
        };
        let inside = |path: &String| path.starts_with(&format!("{at}/"));
        let session = &self.drawbars[who];
        let held = |bytes: &[u8]| {
            let mut listed = session.bench.workspace.listed();
            listed.any(|entity| {
                session.path(entity.id).as_deref() == Some(at.as_str()) && entity.saved.holds(bytes)
            })
        };
        if why.ends_with(NOT_AS_LEFT) {
            return match on_disk.get(&at) {
                Some(Entry::File(bytes)) => !held(bytes),
                Some(Entry::Folder) | None => true,
            };
        }
        match (
            IN_THE_WAY.iter().position(|said| why.ends_with(said)),
            on_disk.get(&at),
        ) {
            (Some(0), Some(Entry::Folder)) => true,
            (Some(1), Some(Entry::Folder)) => on_disk.keys().any(inside),
            (Some(2 | 3), Some(Entry::File(_))) => true,
            _ => false,
        }
    }

    /// Whether what `why` says is not as an undo or redo of `who`'s left it names a file
    /// sync's chaos or a conflict touched, which only that file coming back lets by.
    fn strangely(&self, who: usize, why: &str) -> bool {
        let Folders::Synced { sync, .. } = &self.folders else {
            return false;
        };
        let at = names(why)
            .first()
            .copied()
            .filter(|_| why.ends_with(NOT_AS_LEFT));
        at.is_some_and(|at| sync.strange(at) || self.holds_what_chaos_wrote(who, sync, at))
    }

    /// Where the undo or redo `who` chose was refused and kept for something in its way,
    /// now and then move that out of the way, as the user may, and choose it again: it
    /// goes.
    fn clear_the_way(&mut self, who: usize) {
        let obstacle = |kept: &Kept| IN_THE_WAY.iter().any(|said| kept.why.ends_with(said));
        let History { done, undone } = &self.history[who];
        let offered =
            |kept: &Kept| [done, undone][usize::from(kept.redo)].last() == Some(&kept.made_at);
        let Some(kept) = self.kept[who].take_if(|kept| obstacle(kept) && offered(kept)) else {
            return;
        };
        let at = names(&kept.why).first().map(|at| at.to_string());
        let Some(at) = at.filter(|_| self.dice.one_in(2)) else {
            return;
        };
        let Some(path) = LibPath::parse(&at) else {
            return;
        };
        let Ok(meta) = fs::symlink_metadata(self.folders.root(who).at(&at)) else {
            return;
        };
        let stem = self.fresh_name("Aside");
        let root = self.folders.root(who);
        let leaf = match (meta.is_dir(), path.leaf().rsplit_once('.')) {
            (false, Some((_, extension))) => format!("{stem}.{extension}"),
            _ => stem,
        };
        let to = path.parent().join(&leaf).to_string();
        fs::rename(root.at(&at), root.at(&to)).unwrap_or_else(|e| panic!("{at}: {e}"));
        self.said(format!(
            "in {}'s folder, {at} is moved aside to {to}",
            name(who)
        ));
        self.aside.extend([at, to]);
        for other in 0..self.drawbars.len() {
            if self.folders.of(other) == self.folders.of(who) {
                self.current[other] = false;
            }
        }
        self.come_back(who, true);
        self.again(who, kept, true);
    }

    /// Choose again the undo or redo `kept` is of, where it is still the step `who`'s
    /// Edit menu offers, once what was in its way is `cleared` or sync settled: it goes,
    /// or is let go for good only for what another drawbar changed since. Once cleared it
    /// is never kept again; once sync settles, only for a folder or file still in its
    /// way, or a file sync's chaos or a conflict left other than the step left it.
    fn again(&mut self, who: usize, kept: Kept, cleared: bool) {
        let Kept { redo, made_at, why } = kept;
        let History { done, undone } = &self.history[who];
        let from = [done, undone][usize::from(redo)];
        if from.last() != Some(&made_at) {
            return;
        }
        let session = &mut self.drawbars[who];
        let label = match redo {
            false => session.store.undo_label(),
            true => session.store.redo_label(),
        };
        let Some(label) = label else {
            return;
        };
        let held = holdings(session);
        let log = session.bench.log.len();
        match redo {
            false => session.undo(),
            true => session.redo(),
        }
        session.sync();
        self.said(format!("{} chooses {label} again", name(who)));
        self.expose(who, made_at);
        self.changed(who, &held);
        self.chosen[who] = Some(Chosen { redo, made_at, log });
        self.check();
        self.chosen[who] = None;
        self.refusable[who] = None;
        self.reversing[who].clear();
        let since = self.drawbars[who].bench.log.iter().skip(log);
        let said: Vec<String> = since.map(|entry| entry.text.clone()).collect();
        let kept_again = said
            .iter()
            .find_map(|text| kept_for(text))
            .map(str::to_string);
        if let Some(again) = kept_again {
            self.kept[who] = Some(Kept {
                redo,
                made_at,
                why: again.clone(),
            });
            let stays = !cleared && (self.in_the_way(who, &again) || self.strangely(who, &again));
            if !stays {
                let after = ["sync settles", "that is out of the way"][usize::from(cleared)];
                let what = format!("{label}, kept as {why}, is kept again once {after}");
                self.fail(Broke::Undo, &what, format!("{said:#?}"));
            }
            return;
        }
        let refused = said.iter().any(|text| let_go(text));
        let History { done, undone } = &mut self.history[who];
        let (from, to) = match redo {
            false => (done, undone),
            true => (undone, done),
        };
        from.pop();
        if !refused {
            to.push(made_at);
        }
    }

    /// Note that the undo or redo `who` chooses of the change at `made_at` may be refused
    /// where another drawbar changed something since that change knew every other's.
    fn expose(&mut self, who: usize, made_at: usize) {
        let wrote = self.wrote.get(&(who, made_at)).cloned();
        self.reversing[who].extend(wrote);
        let known = self.known_at(made_at);
        if !self.others(who).any(|other| self.changed[other] > known) {
            return;
        }
        let mut touched = self.touched_by(who, made_at);
        let exposed = match self.refusable[who].take() {
            Some(was) => {
                touched.extend(was.touched);
                Exposed {
                    known: was.known.min(known),
                    touched,
                }
            }
            None => Exposed { known, touched },
        };
        self.refusable[who] = Some(exposed);
    }

    /// Follow through [`Contested`] each file drawbar `who` shows under another name since
    /// it was last looked at.
    fn follow(&mut self, who: usize) {
        let held = holdings(&self.drawbars[who]).into_iter();
        let now = held
            .filter_map(|(key, (path, ..))| Some((key, path?)))
            .collect();
        let was = std::mem::replace(&mut self.at[who], now);
        for (key, path) in &self.at[who] {
            let known = self.known[who].entry(*key).or_default();
            known.insert(leaf(path).to_string());
        }
        for (key, path) in was {
            if let Some(is) = self.at[who].get(&key).filter(|is| **is != path) {
                self.contested.moved(path, is.clone());
            }
        }
    }

    /// Whether drawbar `who` shows a file by one of `names` holding what a file another
    /// drawbar changed the tags of held, which binding, knowing an entity by its file's
    /// contents, may take for it; or shows the tags of an entity at one of those names,
    /// or of any entity where it shows none by them, changed since its last vet.
    fn retagged_like(&self, who: usize, names: &[&str]) -> bool {
        let session = &self.drawbars[who];
        let known: BTreeSet<String> = names.iter().flat_map(|n| self.known_as(who, n)).collect();
        let named = (session.bench.workspace.listed()).filter(|e| known.contains(&e.name));
        let paths: Vec<String> = named.filter_map(|e| session.path(e.id)).collect();
        let on_disk = disk(self.folders.root(who));
        let theirs = |path: &String| match on_disk.get(path) {
            Some(Entry::File(bytes)) => self.retagged_bytes[who].contains(bytes),
            _ => false,
        };
        let now = entities_worn(session);
        let rows = shown(session).rows.into_values();
        let mut changed = rows.filter(|row| {
            let entity = row.entity.filter(|e| now.get(e) != self.worn[who].get(e));
            entity.is_some() && (paths.is_empty() || known.contains(leaf(row.path.as_str())))
        });
        paths.iter().any(theirs) || changed.next().is_some()
    }

    /// The clock of the settle of sync before the last, as a drawbar reads what another
    /// drawbar changed before the last settle once it comes back after it.
    fn before_settles(&self) -> usize {
        self.settled.iter().rev().nth(1).copied().unwrap_or(0)
    }

    /// Whether drawbar `who` shows a file by one of `names` that holds what another
    /// drawbar's action left an asset holding.
    fn holds_theirs(&self, who: usize, names: &[&str]) -> bool {
        let session = &self.drawbars[who];
        let known: BTreeSet<String> = names.iter().flat_map(|n| self.known_as(who, n)).collect();
        let mut listed = session.bench.workspace.listed();
        listed.any(|entity| {
            let theirs = self
                .held
                .get(&entity.saved.bytes().to_vec())
                .into_iter()
                .flatten();
            known.contains(&entity.name) && theirs.into_iter().any(|by| *by != who)
        })
    }

    /// `name`, every other name drawbar `who` has shown what it shows as `name` under,
    /// and every name a drawbar renamed one of those from or to.
    fn known_as(&self, who: usize, name: &str) -> BTreeSet<String> {
        let known = self.known[who]
            .values()
            .filter(|known| known.contains(name));
        let mut names: BTreeSet<String> = known.flatten().cloned().collect();
        names.insert(name.to_string());
        loop {
            let had = names.len();
            for (was, is) in &self.renames {
                if names.contains(was) || names.contains(is) {
                    names.extend([was.clone(), is.clone()]);
                }
            }
            if names.len() == had {
                return names;
            }
        }
    }

    /// Note, of drawbar `who`, which held `before`, each file whose tags it changed, as
    /// one another drawbar may say it changed the tags of.
    fn retags(&mut self, who: usize, before: &Holdings) {
        let after = holdings(&self.drawbars[who]);
        let worn = |held: &Holdings, key| held.get(key).map(|(_, worn, _)| worn.clone());
        let keys: BTreeSet<&(bool, u64)> = before.keys().chain(after.keys()).collect();
        let keys = keys.into_iter().filter(|key| {
            worn(before, key).unwrap_or_default() != worn(&after, key).unwrap_or_default()
        });
        let held = keys.flat_map(|key| [before.get(key), after.get(key)].into_iter().flatten());
        let held: Vec<_> = held.collect();
        let paths = held
            .iter()
            .filter_map(|(path, ..)| Some(leaf(path.as_deref()?).to_string()));
        let retagged: BTreeSet<String> = paths.collect();
        let on_disk = disk(self.folders.root(who));
        let files = held
            .iter()
            .filter_map(|(path, ..)| on_disk.get(path.as_deref()?));
        let bytes = files.filter_map(|entry| match entry {
            Entry::File(bytes) => Some(bytes.clone()),
            Entry::Folder => None,
        });
        let bytes: BTreeSet<Vec<u8>> = bytes.collect();
        for other in self.others(who).collect::<Vec<_>>() {
            self.retagged[other].extend(retagged.iter().cloned());
            self.retagged_bytes[other].extend(bytes.iter().cloned());
        }
    }

    /// Take back the change just made, then make it again: the folder and what drawbar
    /// shows are as they were before it, then as they were after it.
    fn undo_and_redo(
        &mut self,
        who: usize,
        before: (Disk, Seen),
        shown_before: &Shown,
        logged: &BTreeSet<toshokan::EntryHash>,
    ) {
        let after = (disk(self.folders.root(who)), seen(&self.drawbars[who]));
        let name = name(who);
        for (redo, (disk_was, seen_was)) in [(false, before), (true, after)] {
            let (offered, when) = [("Undo", "before"), ("Redo", "after")][usize::from(redo)];
            let session = &mut self.drawbars[who];
            let label = match redo {
                false => session.store.undo_label(),
                true => session.store.redo_label(),
            };
            let Some(label) = label else {
                let what = format!("{name} offers no {offered}");
                self.fail(Broke::Effect, &what, "after the change".into());
            };
            let held = holdings(session);
            let log = session.bench.log.len();
            match redo {
                false => session.undo(),
                true => session.redo(),
            }
            session.sync();
            if let Some(made_at) = self.history[who].done.last().copied() {
                self.expose(who, made_at);
            }
            self.said(format!("{name} chooses {label} at once"));
            self.changed(who, &held);
            self.check();
            let since = self.drawbars[who].bench.log.iter().skip(log);
            if since.into_iter().any(|entry| let_go(&entry.text)) {
                self.history[who].done.pop();
                return;
            }
            let what = format!("{label} leaves the folder other than it was {when}");
            if let Some(d) = apart(&disk(self.folders.root(who)), &disk_was) {
                self.fail(
                    Broke::Effect,
                    &what,
                    format!("(now first, {when} second)\n  {d}"),
                );
            }
            let pinned = match redo {
                false => {
                    let mut pinned = self.pinned_away(who, logged, shown_before);
                    pinned.extend(self.tagged_apart(who, &seen_was));
                    pinned
                }
                true => BTreeSet::new(),
            };
            let unpinned = |seen: &Seen| Seen {
                files: (seen.files.iter())
                    .filter(|(path, _)| !pinned.contains(*path))
                    .map(|(path, tags)| (path.clone(), tags.clone()))
                    .collect(),
                lost: (seen.lost.iter())
                    .filter(|path| !pinned.contains(*path))
                    .cloned()
                    .collect(),
                ..seen.clone()
            };
            let differ = seen_apart(&unpinned(&seen(&self.drawbars[who])), &unpinned(&seen_was));
            if !differ.is_empty() {
                let what = format!("{label} leaves {name} showing other than {when}");
                let d = format!("(now first, {when} second)\n  {}", differ.join("\n  "));
                self.fail(Broke::Effect, &what, d);
            }
        }
    }

    /// The paths of the rows drawbar `who` shows lost now and did not as `before`, of
    /// entities whose latest file fact at the path of another entity's row gives another
    /// identity than that entity's latest there: two drawbars tagged two versions of one
    /// file, and once an undo leaves the file holding the other entity's fact, this one's
    /// file is nowhere.
    fn tagged_apart(&self, who: usize, before: &Seen) -> BTreeSet<String> {
        type Latest = ((toshokan::Hlc, toshokan::EntryHash), toshokan::Identity);
        let mut latest: BTreeMap<(toshokan::EntityId, String), Latest> = BTreeMap::new();
        for entry in logged_entries(self.folders.root(who)) {
            let ops = match entry.kind() {
                EntryKind::Intent(logged) => logged.ops.clone(),
                EntryKind::Bind(bound) => bound.ops.clone(),
                _ => Vec::new(),
            };
            for op in ops {
                if let Op::File {
                    entity,
                    file: Some(file),
                    ..
                }
                | Op::Pin { entity, file, .. } = op
                {
                    let fact = ((entry.at(), entry.own().hash), file.identity);
                    let key = (entity, file.path.to_string());
                    if latest.get(&key).is_none_or(|had| had.0 < fact.0) {
                        latest.insert(key, fact);
                    }
                }
            }
        }
        let at = |entity: toshokan::EntityId, path: &str| {
            let fact = latest.get(&(entity, path.to_string()));
            fact.map(|(_, identity)| *identity)
        };
        let now = shown(&self.drawbars[who]);
        let present: Vec<&Row> = now.class(Class::File).map(|(_, row)| row).collect();
        let lost = now.class(Class::Lost).map(|(_, row)| row);
        let lost = lost.filter(|row| !before.lost.contains(&row.path.to_string()));
        let apart = lost.filter(|row| {
            let (Some(entity), path) = (row.entity, row.path.as_str()) else {
                return false;
            };
            let Some(ours) = at(entity, path) else {
                return false;
            };
            present.iter().any(|other| {
                let theirs = other
                    .entity
                    .filter(|e| *e != entity && other.path == row.path);
                theirs
                    .and_then(|e| at(e, path))
                    .is_some_and(|theirs| theirs != ours)
            })
        });
        apart.map(|row| row.path.to_string()).collect()
    }

    /// The paths of the rows of each entity `who` showed, as `before` or now, at a path
    /// another entity's file fact named until a pin drawbar `who` logged since its log held
    /// `logged` replaced it: the pin settled which entity that path is, and pins are never
    /// undone, so an undo leaves those entities bound as the pin left them.
    fn pinned_away(
        &self,
        who: usize,
        logged: &BTreeSet<toshokan::EntryHash>,
        before: &Shown,
    ) -> BTreeSet<String> {
        let all = self::logged_entries(self.folders.root(who));
        let ops = |kind: &EntryKind| match kind {
            EntryKind::Intent(logged) => logged.ops.clone(),
            EntryKind::Bind(bound) => bound.ops.clone(),
            _ => Vec::new(),
        };
        let by_hash: BTreeMap<_, _> = all.iter().map(|entry| (entry.hash(), entry)).collect();
        let mut stale = BTreeSet::new();
        for entry in all.iter().filter(|entry| !logged.contains(&entry.hash())) {
            let EntryKind::Bind(bound) = entry.kind() else {
                continue;
            };
            for op in &bound.ops {
                let Op::Pin {
                    entity, replaces, ..
                } = op
                else {
                    continue;
                };
                let replaced = replaces.iter().filter_map(|at| by_hash.get(&at.hash));
                for old in replaced.flat_map(|entry| ops(entry.kind())) {
                    let path = match old {
                        Op::File {
                            entity: of,
                            file: Some(file),
                            ..
                        }
                        | Op::Pin {
                            entity: of, file, ..
                        } if of == *entity => file.path.to_string(),
                        _ => continue,
                    };
                    stale.insert((*entity, path));
                }
            }
        }
        let now = shown(&self.drawbars[who]);
        let rows: Vec<&Row> = before.rows.values().chain(now.rows.values()).collect();
        let named = |row: &&&Row| {
            let entity = row.entity;
            stale.iter().any(|(pinned, path)| {
                entity.is_some_and(|e| e != *pinned) && row.path.as_str() == path
            })
        };
        let moved: BTreeSet<_> = rows
            .iter()
            .filter(named)
            .filter_map(|row| row.entity)
            .collect();
        let rows = rows
            .iter()
            .filter(|row| row.entity.is_some_and(|e| moved.contains(&e)));
        rows.map(|row| row.path.to_string()).collect()
    }

    /// Every invariant that holds after a step: for each drawbar that has read every
    /// change, what it shows is a fresh open and is on disk; no log says anything changed
    /// outside drawbar; Undo is not stuck; every unsaved edit is kept; and, on one shared
    /// folder, no file is missing. While sync lags, a file in transit may show missing.
    fn check(&mut self) {
        let lags = matches!(self.folders, Folders::Synced { .. });
        for who in 0..self.drawbars.len() {
            self.vet(who);
            if !lags {
                self.nothing_missing(who, Broke::OwnFolder);
            }
            self.not_stuck(who);
        }
        let current: Vec<usize> = (0..self.drawbars.len())
            .filter(|who| self.current[*who])
            .collect();
        let mut by_folder: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for who in &current {
            by_folder
                .entry(self.folders.of(*who))
                .or_default()
                .push(*who);
        }
        for (folder, drawbars) in by_folder {
            self.show_a_fresh_open(folder, &drawbars, "once settled");
        }
        for who in current {
            self.shows_the_disk(who);
        }
        self.edits_kept();
    }

    /// What drawbar `who` shows has what its action did.
    fn shows_its_effect(&self, who: usize, shows: &[Shows]) {
        let session = &self.drawbars[who];
        let now = seen(session);
        let holding = |id: u64, bytes: &[u8]| {
            let entity = session.bench.workspace.get(id);
            entity.is_some_and(|entity| *entity.bytes == *bytes)
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
                Shows::Holding(id, bytes) => !holding(*id, bytes),
                Shows::Folder(path) => !now.folders.contains(path),
                Shows::NoFolder(path) => now.folders.contains(path),
                Shows::Wearing(path, tag) => wears(path, tag) != Some(true),
                Shows::NotWearing(path, tag) => wears(path, tag) != Some(false),
            })
            .collect();
        if !wrong.is_empty() {
            let what = format!("{} does not show what it did", name(who));
            self.fail(
                Broke::Effect,
                &what,
                format!("expected {wrong:?}\n  shows {now:#?}"),
            );
        }
    }

    /// Look at the log entries `who` wrote since the last look: none says what another
    /// drawbar or program changed, or that something went wrong, unless
    /// [`Walk::accounted`] accounts for it.
    fn vet(&mut self, who: usize) {
        self.follow(who);
        let log = &self.drawbars[who].bench.log;
        assert!(
            log.len() < 500,
            "{} logged more than its log keeps",
            name(who)
        );
        let wrong: Vec<String> = (log.iter().skip(self.vetted[who]))
            .filter(|entry| !self.accounted(who, entry))
            .filter(|entry| entry.level != Level::Info || entry.theirs)
            .map(|entry| entry.text.clone())
            .collect();
        self.vetted[who] = log.len();
        let files = seen(&self.drawbars[who]).files.into_iter();
        let tagged = files.filter(|(_, worn)| !worn.is_empty());
        self.tagged[who] = tagged.map(|(path, _)| path).collect();
        self.worn[who] = entities_worn(&self.drawbars[who]);
        if wrong.is_empty() {
            return;
        }
        let undoing = |text: &String| text.contains("cannot be");
        let (broke, what) = match wrong.iter().any(undoing) {
            true => (Broke::Undo, "says an undo or redo failed"),
            false => (
                Broke::Silence,
                "says something changed outside drawbar or failed",
            ),
        };
        self.fail(
            broke,
            &format!("{} {what}", name(who)),
            format!("{wrong:#?}"),
        );
    }

    /// Undo goes on from a step the walk's history holds: a drawbar whose history the
    /// walk has emptied, with no refusal still to learn, offers no Undo.
    fn not_stuck(&self, who: usize) {
        let known = self.chosen[who].is_none() && self.history[who].done.is_empty();
        let offered = self.drawbars[who].store.undo_label();
        if let Some(label) = offered.filter(|_| known) {
            let what = format!("{} offers {label} after every step was undone", name(who));
            self.fail(Broke::Undo, &what, "the walk's history is empty".into());
        }
    }

    fn nothing_missing(&self, who: usize, broke: Broke) {
        let folders = &self.drawbars[who].bench.browser.folders;
        let unsaved = &self.unsaved[who];
        let missing: Vec<&u64> = (folders.missing.iter())
            .filter(|id| !unsaved.contains_key(id))
            .collect();
        if !missing.is_empty() {
            let what = format!("{} shows a file as missing", name(who));
            self.fail(broke, &what, format!("{missing:?}"));
        }
    }

    /// Each drawbar in `drawbars`, all on `folder`, shows what a fresh open of it shows.
    fn show_a_fresh_open(&mut self, folder: usize, drawbars: &[usize], when: &str) {
        let root = self.folders.root(drawbars[0]);
        let now = tree(&root.0);
        if self
            .fresh
            .get(&folder)
            .is_none_or(|fresh| fresh.folder != now)
        {
            let local = Temp::new();
            let session = Session::ambient(root, &local.0, self.times.ambient());
            let fresh = Opened {
                folder: now,
                session,
                _local: local,
            };
            if let Some(stale) = self.fresh.insert(folder, fresh) {
                stale.session.close();
            }
        }
        for who in drawbars {
            self.shows(*who, &self.fresh[&folder].session, when);
        }
    }

    fn shows(&self, who: usize, fresh: &Session, when: &str) {
        let name = name(who);
        let differ = seen_apart(&seen(&self.drawbars[who]), &seen(fresh));
        if !differ.is_empty() {
            let what = format!("{name} {when} does not show what a fresh open shows");
            let first = format!("({name} first, a fresh open second)\n  ");
            self.fail(Broke::OwnFolder, &what, first + &differ.join("\n  "));
        }
        let differ = differences(&shown(&self.drawbars[who]), &shown(fresh));
        if !differ.is_empty() {
            let what = format!("{name}'s view {when} is not a fresh open's");
            self.fail(Broke::OwnFolder, &what, differ.join("\n  "));
        }
    }

    fn shows_the_disk(&self, who: usize) {
        let session = &self.drawbars[who];
        let on_disk = disk(self.folders.root(who));
        let mut wrong = Vec::new();
        let listed = session.bench.workspace.listed();
        for entity in listed.filter(|entity| !missing(session, entity.id)) {
            let Some(path) = session.path(entity.id) else {
                wrong.push(format!("“{}” has no file", entity.name));
                continue;
            };
            let saved = match entity.is_unsaved() {
                true => entity.saved.bytes().to_vec(),
                false => entity.bytes.to_vec(),
            };
            match on_disk.get(&path) {
                Some(Entry::File(bytes)) if entity.unread() || saved.is_empty() => {}
                Some(Entry::File(bytes)) if *bytes == saved => {}
                Some(Entry::File(_)) if self.unseen[who].contains(&path) => {}
                Some(Entry::File(bytes))
                    if self.late[who].get(&path) == Some(&(bytes.clone(), saved.clone())) => {}
                Some(Entry::File(_)) => wrong.push(format!("{path} holds other bytes on disk")),
                _ => wrong.push(format!("{path} is not a file on disk")),
            }
        }
        let folders = &session.bench.browser.folders;
        let shown_dirs = folders.all().iter().map(|folder| folder.path.to_string());
        for dir in shown_dirs.clone() {
            if on_disk.get(&dir) != Some(&Entry::Folder) {
                wrong.push(format!("folder {dir} is not on disk"));
            }
        }
        let files: BTreeSet<String> = (session.bench.workspace.listed())
            .filter_map(|entity| session.path(entity.id))
            .chain(folders.others.iter().map(LibPath::to_string))
            .collect();
        let dirs: BTreeSet<String> = shown_dirs.collect();
        for (path, entry) in &on_disk {
            let shown = match entry {
                Entry::File(_) => files.contains(path),
                Entry::Folder => dirs.contains(path),
            };
            if !shown {
                wrong.push(format!("{path} is on disk and not shown"));
            }
        }
        if !wrong.is_empty() {
            let what = format!("what {} shows is not on disk", name(who));
            self.fail(Broke::OwnFolder, &what, format!("{wrong:#?}"));
        }
    }

    /// Follow the unsaved edit an action left on `id`, or forget the one it saved.
    fn track_edit(&mut self, who: usize, id: u64, gain: usize) {
        let entity = self.drawbars[who].bench.workspace.get(id);
        match entity.filter(|entity| entity.is_unsaved()) {
            Some(entity) => {
                let bytes = entity.bytes.to_vec();
                self.unsaved[who].insert(id, Edit { gain, bytes });
            }
            None => {
                self.unsaved[who].remove(&id);
            }
        }
    }

    /// Every unsaved edit the walk made is held by its asset, which its drawbar lists, but
    /// for one its asset's file came to hold, which is no longer unsaved: an undo put the
    /// file back as the edit has it, or another drawbar saved the same bytes.
    fn edits_kept(&mut self) {
        for (who, edits) in self.unsaved.iter_mut().enumerate() {
            let workspace = &self.drawbars[who].bench.workspace;
            let saved = |id: &u64, edit: &Edit| {
                workspace
                    .get(*id)
                    .is_some_and(|entity| entity.saved.holds(&edit.bytes))
            };
            edits.retain(|id, edit| !saved(id, edit));
        }
        for (who, edits) in self.unsaved.iter().enumerate() {
            let session = &self.drawbars[who];
            for (id, edit) in edits {
                let mut listed = session.bench.workspace.listed();
                if listed.any(|entity| entity.id == *id && entity.bytes.to_vec() == edit.bytes) {
                    continue;
                }
                let entity = session.bench.workspace.get(*id);
                let path = entity.and_then(|_| session.path(*id)).unwrap_or_default();
                let what = format!(
                    "{}'s edit setting the gain of {path} to {} is gone",
                    name(who),
                    edit.gain
                );
                let listed = session.bench.workspace.listed().any(|e| e.id == *id);
                let details = match (listed, entity) {
                    (true, Some(entity)) => {
                        let saved = ["", " and saved"][usize::from(!entity.is_unsaved())];
                        format!("the asset is listed{saved}, holding other bytes")
                    }
                    _ => "the asset is no longer listed, and no asset holds the edit".into(),
                };
                self.fail(Broke::Unsaved, &what, details);
            }
        }
    }

    /// Answer each question `who` asks, as the seed chooses.
    fn answer_questions(&mut self, who: usize) {
        for _ in 0..8 {
            let Some((title, answers)) = self.drawbars[who].bench.browser.asking() else {
                return;
            };
            let answer = self.dice.pick(&answers).expect("a question has answers");
            let session = &mut self.drawbars[who];
            let held = holdings(session);
            let acts = session.bench.browser.answer(answer);
            perform(session, acts);
            session.sync();
            self.said(format!("{} answers “{title}” with {answer}", name(who)));
            self.changed(who, &held);
            if matches!(self.folders, Folders::Shared(_)) {
                for other in self.others(who).collect::<Vec<_>>() {
                    self.current[other] = false;
                }
            }
            if answer == "Keep both" || answer == "Take theirs" {
                let named = names(&title);
                let session = &self.drawbars[who];
                let workspace = &session.bench.workspace;
                // The answer settles the edit of the asset asked about, which another
                // asset of the same name may not share; Keep both saves it as a file.
                let about = |id: &u64| {
                    let entity = workspace.get(*id);
                    entity.is_some_and(|e| named.contains(&e.name.as_str()) && !e.is_unsaved())
                };
                let filed = |edit: &Edit| workspace.listed().any(|e| e.saved.holds(&edit.bytes));
                let both = answer == "Keep both";
                (self.unsaved[who]).retain(|id, edit| !about(id) || both && !filed(edit));
            }
            if answer == "Keep both" {
                self.edits_kept();
                self.clock += 1;
                self.history[who].done.push(self.clock);
                self.history[who].undone.clear();
                self.changed[who] = self.clock;
            }
            self.check();
        }
        self.fail(
            Broke::Effect,
            &format!("{} asks on and on", name(who)),
            String::new(),
        );
    }

    /// Do `action` as drawbar `who`, where it has something to act on: what it made of
    /// the library, and whether the user went on without waiting for its answers.
    fn act(&mut self, who: usize, action: Action) -> Option<Done> {
        let name = name(who);
        let session = &self.drawbars[who];
        let edits = matches!(action, Action::Edit | Action::Save);
        let files: Vec<u64> = (session.bench.workspace.listed())
            .filter(|entity| entity.path.is_some())
            .filter(|entity| edits || !missing(session, entity.id))
            .map(|entity| entity.id)
            .collect();
        let folders: Vec<u64> = (self.drawbars[who].bench.browser.folders.all().iter())
            .map(|folder| folder.id)
            .collect();
        let path = |walk: &Walk, id: u64| walk.drawbars[who].path(id).unwrap_or_default();
        let done = match action {
            Action::Make => {
                let count = seen(&self.drawbars[who]).files.len();
                let listed = |session: &Session| -> BTreeSet<u64> {
                    session.bench.workspace.listed().map(|e| e.id).collect()
                };
                let before = listed(&self.drawbars[who]);
                perform(&mut self.drawbars[who], vec![Act::New(Fresh::Program)]);
                let made = listed(&self.drawbars[who])
                    .difference(&before)
                    .next()
                    .copied();
                self.said(format!("{name} makes a program"));
                Done {
                    wrote: made,
                    ..Done::one(vec![Shows::Files(count + 1)])
                }
            }
            Action::Edit | Action::Save => {
                let saves = matches!(action, Action::Save);
                let id = self.dice.pick(&files)?;
                let gain = 1 + self.dice.below(99);
                let session = &mut self.drawbars[who];
                session.read(&[id]);
                let held = session.bytes(id);
                let gained =
                    crate::fields::apply(&held, &[("center_panel.gain".into(), gain.to_string())]);
                let edited = gained.ok()?.1;
                if edited == held {
                    return None;
                }
                // An edit back to what the file holds leaves nothing to write, but where
                // that file is missing the save writes one.
                let writes = missing(session, id);
                let Bench { workspace, log, .. } = &mut session.bench;
                let saved = workspace.get(id)?.saved.bytes().to_vec();
                workspace.replace_bytes(id, edited.clone(), log);
                if saves {
                    perform(session, vec![Act::SaveDoc(id)]);
                }
                let at = path(self, id);
                let then = ["leaves it unsaved", "saves it"][usize::from(saves)];
                self.said(format!("{name} sets the gain of {at} to {gain} and {then}"));
                // A missing file's asset edited back to what it was saved as holds no
                // work, and may go.
                let goes = writes && !saves && edited == saved;
                Done {
                    made: [Made::Nothing, Made::One]
                        [usize::from(saves && (writes || edited != saved))],
                    edited: Some((id, gain)),
                    wrote: saves.then_some(id),
                    ..Done::one(match goes {
                        true => Vec::new(),
                        false => vec![Shows::Holding(id, edited)],
                    })
                }
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
                self.unsaved[who].remove(&id);
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
                self.passed.insert(at.to_string());
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
                self.looking(who, false);
                self.said(format!("{name} refreshes"));
                Done::nothing()
            }
            Action::Refocus => {
                self.drawbars[who].refocus();
                self.looking(who, true);
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

    /// Undo or redo as drawbar `who`, where its Edit menu offers it. One another drawbar
    /// has changed something under since may be let go: the walk learns which once it
    /// settles.
    fn step_history(&mut self, who: usize, action: Action) -> Option<Done> {
        let redo = matches!(action, Action::Redo);
        let made_at = *match redo {
            false => &self.history[who].done,
            true => &self.history[who].undone,
        }
        .last()?;
        self.expose(who, made_at);
        self.kept[who] = None;
        // Its answers may land only once another drawbar looks.
        let touched = self.touched_by(who, made_at);
        for other in self.others(who).collect::<Vec<_>>() {
            let leaves = touched.iter().map(|path| leaf(path).to_string());
            self.retagged[other].extend(leaves);
        }
        let History { done, undone } = &mut self.history[who];
        let from = match redo {
            false => done,
            true => undone,
        };
        let session = &mut self.drawbars[who];
        let offered = match redo {
            false => session.store.undo_label()?,
            true => session.store.redo_label()?,
        };
        from.pop();
        let log = session.bench.log.len();
        self.chosen[who] = Some(Chosen { redo, made_at, log });
        let hurried = self.dice.one_in(2);
        match redo {
            false => session.undo(),
            true => session.redo(),
        }
        self.clock += 1;
        self.changed[who] = self.clock;
        self.said(format!("{} chooses {offered}{}", name(who), hurry(hurried)));
        Some(Done {
            made: Made::Step,
            hurried,
            ..Done::nothing()
        })
    }

    /// The clock up to which every drawbar had every other's changes when the change at
    /// `made_at` was made: on one folder, that clock; while sync lags, the last settle's
    /// before it, since a change made before it may arrive after. A settle at the clock of
    /// the change came after it.
    fn known_at(&self, made_at: usize) -> usize {
        match self.folders {
            Folders::Shared(_) => made_at,
            Folders::Synced { .. } => {
                let settled = self.settled.iter().rev().find(|clock| **clock < made_at);
                settled.copied().unwrap_or(0)
            }
        }
    }

    /// Take the step a settled undo or redo of `who` moved into the history, unless the
    /// library refused it as changed since.
    fn learn_whether_refused(&mut self, who: usize) {
        let Some(Chosen { redo, made_at, log }) = self.chosen[who].take() else {
            return;
        };
        let since: Vec<&crate::log::Entry> =
            self.drawbars[who].bench.log.iter().skip(log).collect();
        if since.iter().any(|entry| let_go(&entry.text)) {
            return;
        }
        let why = since.iter().find_map(|entry| kept_for(&entry.text));
        let History { done, undone } = &mut self.history[who];
        match (redo, why) {
            (false, None) | (true, Some(_)) => undone.push(made_at),
            (true, None) | (false, Some(_)) => done.push(made_at),
        }
        let why = why.map(str::to_string);
        self.kept[who] = why.map(|why| Kept { redo, made_at, why });
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

/// What a sync event does to a copy, as a step says it.
fn described(event: &SyncEvent) -> String {
    match event {
        SyncEvent::Deliver { path } => format!("{path} arrives"),
        SyncEvent::Delete { path } => format!("the deletion of {path} arrives"),
        SyncEvent::Shorten { path, len } => format!("only the first {len} bytes of {path} arrive"),
        SyncEvent::Hide { path } => format!("{path} disappears for a while"),
        SyncEvent::Resurrect { path, version } => {
            format!("version {version} of {path}, which was deleted, comes back")
        }
        SyncEvent::ConflictedCopy { path, name } => format!("{path} arrives as {name}"),
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
