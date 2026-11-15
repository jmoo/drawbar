//! drawbar's links between library files and program slots, against an attached Electro
//! 5, with no window. Every test writes to the instrument, so each is ignored and skipped
//! unless `DRAWBAR_HARDWARE=1`. `docs/src/reference/testing.md` gives the command.
//!
//! ⚠️ A test writes only to the programs [`SPARES`], which must be empty when the run
//! starts, to the programs of [`SCRATCH`], which the run reads first with nord-cli and puts
//! back, and to one sample slot under a name of this suite. Each test empties the spares,
//! puts the scratch programs back where they differ and deletes the suite's samples when
//! it ends, however it ends.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use nord_format::components::library_id;
use nord_usb::{Location, ObjectClass};

use super::hardware::{attached, place, Cli, Rig, ATTACH_LIMIT, HEADER, MARK, SEND_LIMIT};
use super::{link_offers, standing_of, Linked};
use crate::bring::{Change, Row, Way};
use crate::browser::Act;
use crate::link::{Standing, There};
use crate::store::{Pass, Store};
use crate::strings::shown;
use crate::testing::{Bench, Temp};
use crate::workspace::{print_of, Fresh, Origin};

const PROGRAM: ObjectClass = ObjectClass::Program;

const PIANO_CATEGORY: &str = "piano_panel.category";
const PIANO_MODEL: &str = "piano_panel.piano_model";
const PIANO_ID: &str = "piano_panel.id";

/// Empty program slots the suite sends to.
const SPARES: [(u32, u32); 2] = [(7, 6), (7, 44)];

/// The bank and slots of the programs the suite copies, edits, overwrites and moves.
const SCRATCH: (u32, std::ops::RangeInclusive<u32>) = (8, 1..=12);

/// A slot the instrument this suite was written on holds a New program's bytes in.
const FIRST: (u32, u32) = (5, 46);

fn at((bank, slot): (u32, u32)) -> Location {
    Location::from_user(bank, slot)
}

fn spare() -> Location {
    at(SPARES[0])
}

fn other_spare() -> Location {
    at(SPARES[1])
}

fn scratch(slot: u32) -> Location {
    assert!(SCRATCH.1.contains(&slot), "8:{slot} is not a scratch slot");
    at((SCRATCH.0, slot))
}

/// A program slot as nord-cli reports it: its name and its body's CRC-32.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Occupant {
    name: String,
    crc: u32,
}

/// A scratch program as the run found it, and the file it was read into.
struct Kept {
    at: Location,
    held: Occupant,
    file: PathBuf,
}

/// nord-cli, the run's folder, and the scratch programs as the run found them.
struct Bank {
    cli: Cli,
    dir: PathBuf,
    kept: Vec<Kept>,
}

static BANK: OnceLock<Bank> = OnceLock::new();

/// ⚠️ The first call reads the instrument with nord-cli, so it must come before drawbar
/// claims the interface.
fn bank() -> &'static Bank {
    BANK.get_or_init(|| {
        let nord = std::env::var("DRAWBAR_HARDWARE_NORD").unwrap_or_else(|_| "nord".into());
        let cli = Cli(PathBuf::from(nord));
        let dir = std::env::temp_dir().join(format!("drawbar-links-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("the run's folder");
        for slot in SPARES.map(at) {
            if let Some(held) = info(&cli, slot).unwrap_or_else(|why| panic!("{why}")) {
                panic!(
                    "program {} holds {held:?}; this suite writes only to empty spares",
                    shown(slot)
                );
            }
        }
        let kept = SCRATCH
            .1
            .map(|slot| {
                let at = scratch(slot);
                let held = info(&cli, at)
                    .unwrap_or_else(|why| panic!("{why}"))
                    .unwrap_or_else(|| panic!("scratch program {} is empty", shown(at)));
                let folder = dir.join(format!("kept-{slot}"));
                fs::create_dir_all(&folder).expect("the backup's folder");
                let file = folder.join(format!("{}.ne5p", held.name));
                cli.ok(&["program", "get", &shown(at), "-o", path_arg(&file)]);
                Kept { at, held, file }
            })
            .collect();
        eprintln!("scratch programs read into {dir:?}");
        Bank { cli, dir, kept }
    })
}

fn path_arg(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// What a program slot holds, or `None` where it is empty.
fn info(cli: &Cli, at: Location) -> Result<Option<Occupant>, String> {
    let stdout = match cli.said(&["program", "info", &shown(at)]) {
        Ok((stdout, _)) => stdout,
        Err(why) if why.contains("is empty") => return Ok(None),
        Err(why) => return Err(why),
    };
    let field = |key: &str| {
        stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix(key))
            .map(|value| value.trim().trim_matches('"').to_string())
            .ok_or_else(|| format!("no {key} in {stdout}"))
    };
    let crc = field("crc32:")?;
    let crc = u32::from_str_radix(crc.trim_start_matches("0x"), 16)
        .map_err(|e| format!("crc32 {crc}: {e}"))?;
    Ok(Some(Occupant {
        name: field("name:")?,
        crc,
    }))
}

impl Bank {
    fn held(&self, at: Location) -> Option<Occupant> {
        info(&self.cli, at).unwrap_or_else(|why| panic!("{why}"))
    }

    fn kept(&self, slot: u32) -> &Kept {
        let at = scratch(slot);
        self.kept.iter().find(|kept| kept.at == at).expect("read")
    }

    /// The body a program slot holds.
    fn body(&self, at: Location) -> Vec<u8> {
        let file = self
            .dir
            .join(format!("back-{}", shown(at).replace(':', "-")));
        self.cli
            .ok(&["program", "get", &shown(at), "-o", path_arg(&file)]);
        let bytes = fs::read(&file).expect("read back");
        let _ = fs::remove_file(&file);
        bytes[HEADER..].to_vec()
    }

    fn deps(&self, at: Location) -> String {
        let said = self.cli.said(&["program", "deps", &shown(at)]);
        let (stdout, stderr) = said.unwrap_or_else(|why| panic!("{why}"));
        format!("{stdout}{stderr}")
    }

    /// Put every slot this suite may write back as the run found it, and say what could
    /// not be.
    fn restore(&self) -> Vec<String> {
        let mut trouble = Vec::new();
        for slot in SPARES.map(at) {
            let place = shown(slot);
            let emptied = info(&self.cli, slot).and_then(|held| match held {
                Some(held) => self
                    .cli
                    .said(&["program", "delete", &place, "--yes"])
                    .map(|_| eprintln!("deleted program {place} ({held:?})")),
                None => Ok(()),
            });
            trouble.extend(emptied.err());
        }
        for kept in &self.kept {
            let place = shown(kept.at);
            let put = info(&self.cli, kept.at).and_then(|held| match held {
                Some(held) if held == kept.held => Ok(()),
                _ => self
                    .cli
                    .said(&["program", "put", path_arg(&kept.file), &place, "--yes"])
                    .map(|_| eprintln!("put program {place} back")),
            });
            trouble.extend(put.err());
            match info(&self.cli, kept.at) {
                Ok(Some(held)) if held == kept.held => {}
                other => trouble.push(format!("{place} holds {other:?}, not {:?}", kept.held)),
            }
        }
        let samples = self.cli.list(ObjectClass::Sample).map(|samples| {
            samples
                .into_iter()
                .filter(|(_, name)| name.starts_with(MARK))
                .collect::<Vec<_>>()
        });
        match samples {
            Ok(ours) => {
                for (slot, name) in ours {
                    let deleted = self.cli.delete(ObjectClass::Sample, slot);
                    eprintln!("deleted sample {} ({name:?})", shown(slot));
                    trouble.extend(deleted.err());
                }
            }
            Err(why) => trouble.push(why),
        }
        trouble
    }
}

/// Puts back what a test wrote when it ends.
///
/// ⚠️ Declared before the [`Rig`], so the rig releases the instrument first and nord-cli
/// can claim it.
struct Restores {
    checked: bool,
}

impl Restores {
    fn begin() -> Restores {
        bank();
        Restores { checked: false }
    }

    /// Put everything back, and fail where something could not be.
    fn finish(mut self) {
        self.checked = true;
        let trouble = bank().restore();
        assert_eq!(
            trouble,
            Vec::<String>::new(),
            "the instrument was not restored"
        );
    }
}

impl Drop for Restores {
    fn drop(&mut self) {
        if self.checked {
            return;
        }
        for why in bank().restore() {
            eprintln!("cleanup: {why}");
        }
    }
}

/// A library holding `files`, each read and its checksum taken.
fn open(files: &[(&str, &Path)]) -> Rig {
    let library = Temp::new();
    for (name, from) in files {
        place(&library, name, from);
    }
    let bench = Bench::new();
    let store = Store::start(crate::testing::backend(&bench.ctx, &library));
    let mut rig = Rig {
        library,
        store,
        bench,
        syncing: false,
    };
    let names: Vec<String> = files.iter().map(|(name, _)| name.to_string()).collect();
    wait(
        &mut rig,
        "the library reads its files",
        ATTACH_LIMIT,
        |rig| names.iter().all(|name| read(rig, name).is_some()),
    );
    rig
}

/// The id of the file named `name`, once its checksum is taken.
fn read(rig: &Rig, name: &str) -> Option<u64> {
    rig.bench
        .workspace
        .listed()
        .find(|entity| entity.name == name && entity.saved_print().is_some())
        .map(|entity| entity.id)
}

/// One frame of the app, with what a read brought in filed as the app files it and the
/// library's files written.
fn tick(rig: &mut Rig, acts: Vec<Act>) {
    rig.frame(acts);
    let fetched = rig.bench.device.take_fetched();
    if !fetched.is_empty() {
        rig.frame(fetched.into_iter().map(Act::Arrive).collect());
    }
    let Bench {
        workspace, browser, ..
    } = &mut rig.bench;
    rig.store.sync(workspace, browser, Pass::Files);
}

fn wait(rig: &mut Rig, what: &str, limit: Duration, done: impl Fn(&Rig) -> bool) {
    let started = Instant::now();
    loop {
        tick(rig, Vec::new());
        if done(rig) {
            return;
        }
        assert!(
            started.elapsed() < limit,
            "{what}: nothing after {limit:?}. If the instrument stopped answering, it may be \
             wedged: stop, and power-cycle it.\n{}",
            rig.bench.log.tail(40)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn settle(rig: &mut Rig) {
    wait(rig, "the instrument settles", SEND_LIMIT, Rig::idle);
}

/// Every write and read of the library committed, as before the app quits.
fn committed(rig: &mut Rig) {
    wait(rig, "the library commits", ATTACH_LIMIT, |rig| {
        !rig.store.deciding()
    });
}

fn act(rig: &mut Rig, acts: Vec<Act>) {
    tick(rig, acts);
    settle(rig);
}

fn ids(rig: &Rig) -> Vec<u64> {
    rig.bench
        .workspace
        .listed()
        .map(|entity| entity.id)
        .collect()
}

/// The file that arrived since `before` was listed.
fn arrived(rig: &Rig, before: &[u64]) -> Option<u64> {
    let mut new = rig
        .bench
        .workspace
        .listed()
        .filter(|entity| !before.contains(&entity.id) && entity.saved_print().is_some());
    new.next().map(|entity| entity.id)
}

fn new_program(rig: &mut Rig) -> u64 {
    let before = ids(rig);
    tick(rig, vec![Act::New(Fresh::Program)]);
    wait(rig, "a New program", ATTACH_LIMIT, |rig| {
        arrived(rig, &before).is_some()
    });
    arrived(rig, &before).expect("made")
}

/// Copy a program slot to this computer, as dragging it onto the library does.
fn copy(rig: &mut Rig, from: Location) -> u64 {
    let before = ids(rig);
    tick(
        rig,
        vec![Act::Copy {
            class: PROGRAM,
            at: from,
        }],
    );
    wait(rig, "the copy arrives", SEND_LIMIT, |rig| {
        rig.idle() && arrived(rig, &before).is_some()
    });
    let id = arrived(rig, &before).expect("arrived");
    let origin = &entity(rig, id).origin;
    assert!(
        matches!(origin, Origin::Device { class: PROGRAM, at } if *at == from),
        "the copy came off {}: {}",
        shown(from),
        origin.label()
    );
    id
}

fn duplicate(rig: &mut Rig, id: u64) -> u64 {
    let before = ids(rig);
    tick(rig, vec![Act::DuplicateLocal(id)]);
    wait(rig, "the duplicate", ATTACH_LIMIT, |rig| {
        arrived(rig, &before).is_some()
    });
    arrived(rig, &before).expect("made")
}

/// Press Sync once, and wait for its sheet to be confirmed and for the walk after the
/// writes.
fn sync(rig: &mut Rig) {
    tick(rig, vec![Act::Sync(None)]);
    synced(rig, |_| {});
}

/// Once the open sheet's read lands, turn `id`'s row to `way` and confirm the sheet as a
/// click on its Sync does, then wait as [`sync`] does.
fn sync_with(rig: &mut Rig, id: u64, way: Way) {
    wait(rig, "the sheet's read", ATTACH_LIMIT, |rig| {
        let device = &rig.bench.device;
        let sheet = device.asks.sheet.as_ref();
        sheet.is_some_and(|sheet| sheet.read(&device.state))
    });
    let bench = &mut rig.bench;
    let all = rows(bench);
    let row = all.iter().find(|row| row.id == id).cloned();
    let row = row.unwrap_or_else(|| panic!("{id} has no row: {all:?}"));
    let sheet = bench.device.asks.sheet.as_mut().expect("open");
    for _ in 0..3 {
        if sheet.way(&row, &all, &bench.workspace) != Some(way) {
            sheet.flip(&row, &all, &bench.workspace);
        }
    }
    assert_eq!(
        sheet.way(&row, &all, &bench.workspace),
        Some(way),
        "{row:?}"
    );
    rig.syncing = true;
    synced(rig, |_| {});
}

/// Wait, a frame at a time and at most [`SEND_LIMIT`], until the pressed sheet has closed
/// and what Sync asked of the keyboard is done, telling `each` after every frame.
fn synced(rig: &mut Rig, mut each: impl FnMut(&Rig)) {
    let started = Instant::now();
    loop {
        tick(rig, Vec::new());
        each(rig);
        let failures: Vec<String> = rig
            .bench
            .queue
            .entries()
            .iter()
            .filter_map(|held| held.failure.clone())
            .collect();
        assert_eq!(
            failures,
            Vec::<String>::new(),
            "the send failed\n{}",
            rig.bench.log.tail(30)
        );
        if !rig.syncing && rig.idle() && rig.bench.queue.is_empty() {
            return;
        }
        assert!(
            started.elapsed() < SEND_LIMIT,
            "the send took more than {SEND_LIMIT:?}. If the instrument stopped answering, it \
             may be wedged: stop, and power-cycle it.\n{}",
            rig.bench.log.tail(40)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn send(rig: &mut Rig, id: u64, to: Location) {
    tick(
        rig,
        vec![Act::Send {
            id,
            class: PROGRAM,
            at: to,
        }],
    );
    sync(rig);
}

fn entity(rig: &Rig, id: u64) -> &crate::workspace::LocalEntity {
    rig.bench.workspace.get(id).expect("listed")
}

fn linked(rig: &Rig, id: u64) -> Option<Linked> {
    standing_of(&rig.bench.device.state, entity(rig, id))
}

/// Where a file is linked on the attached keyboard, and how it stands there.
fn standing(rig: &Rig, id: u64) -> Option<(Location, Standing)> {
    let linked = linked(rig, id)?;
    Some((
        linked.at.expect("a program slot has an address"),
        linked.standing,
    ))
}

/// The slots Link to offers a file.
fn also_at(rig: &Rig, id: u64) -> Vec<Location> {
    let state = &rig.bench.device.state;
    let offers = link_offers(state, &rig.bench.workspace, entity(rig, id));
    offers.into_iter().map(|(_, at)| at).collect()
}

fn queued(rig: &Rig) -> Vec<(u64, ObjectClass, Location)> {
    let entries = rig.bench.queue.entries().iter();
    entries.map(|held| (held.id, held.class, held.at)).collect()
}

fn rows(bench: &Bench) -> Vec<Row> {
    crate::bring::rows(&bench.device.state, &bench.workspace, &bench.queue)
}

fn changes(rig: &Rig) -> Vec<(u64, Change)> {
    let rows = rows(&rig.bench).into_iter();
    rows.map(|row| (row.id, row.change)).collect()
}

/// An edit of `id`'s current bytes through the field registry, held unsaved as a typed
/// edit is.
fn edit(rig: &mut Rig, id: u64, sets: &[(String, String)]) -> Vec<u8> {
    let bytes = entity(rig, id).bytes.to_vec();
    let (_, edited) = crate::fields::apply(&bytes, sets).unwrap_or_else(|why| panic!("{why}"));
    assert_ne!(edited, bytes, "{sets:?} changes the program");
    tick(
        rig,
        vec![Act::Restore {
            id,
            bytes: edited.clone(),
        }],
    );
    edited
}

fn field(bytes: &[u8], path: &str) -> nord_format::fields::Field {
    let (fields, _) = crate::fields::apply(bytes, &[]).unwrap_or_else(|why| panic!("{why}"));
    fields
        .into_iter()
        .find(|field| field.path == path)
        .unwrap_or_else(|| panic!("no {path}"))
}

fn set(path: &str, value: impl ToString) -> (String, String) {
    (path.to_string(), value.to_string())
}

fn log_lines(rig: &Rig, text: &str) -> usize {
    let entries = rig.bench.log.iter();
    entries.filter(|entry| entry.text.contains(text)).count()
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l01_a_new_program_is_only_a_hint_at_the_slot_holding_its_bytes_and_saving_queues_nothing() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let fresh = print_of(&Fresh::Program.bytes().unwrap()).expect("a print");
    let first = at(FIRST);
    let premise = bank().held(first);
    eprintln!(
        "program {} holds {premise:?}; a New program's crc32 is {:#010x}",
        shown(first),
        fresh.crc
    );
    let mut rig = open(&[]);
    rig.connect();
    let id = new_program(&mut rig);

    assert_eq!(linked(&rig, id), None, "a New program is linked nowhere");
    let also = also_at(&rig, id);
    act(&mut rig, vec![Act::SaveDoc(id)]);
    assert_eq!(queued(&rig), [], "saving a New program queues nothing");
    assert_eq!(changes(&rig), [], "nothing to sync");
    assert_eq!(linked(&rig, id), None, "saving links nothing");
    drop(rig);

    eprintln!("drawbar's walk finds its bytes at {also:?}");
    assert!(
        !also.is_empty(),
        "the walk finds no slot holding a New program"
    );
    for slot in &also {
        let held = bank().held(*slot);
        assert_eq!(
            held.map(|held| held.crc),
            Some(fresh.crc),
            "nord-cli reads {} as holding a New program",
            shown(*slot)
        );
    }
    if premise.is_some_and(|held| held.crc == fresh.crc) {
        assert!(also.contains(&first), "the hint names {}", shown(first));
    }
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l02_a_new_program_sent_to_a_slot_stays_linked_to_it_through_a_reopen() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let mut rig = open(&[]);
    rig.connect();
    let id = new_program(&mut rig);
    send(&mut rig, id, spare());

    assert_eq!(standing(&rig, id), Some((spare(), Standing::Same)));
    let name = entity(&rig, id).name.clone();
    committed(&mut rig);
    let mut rig = rig.reopen();
    rig.connect();
    wait(
        &mut rig,
        "the reopened library reads it",
        ATTACH_LIMIT,
        |rig| read(rig, &name).is_some(),
    );
    let id = read(&rig, &name).expect("read");
    assert_eq!(
        standing(&rig, id),
        Some((spare(), Standing::Same)),
        "“{name}” after a reopen"
    );
    drop(rig);
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l03_a_copy_is_linked_to_its_slot_and_a_duplicate_or_second_copy_is_not() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let mut rig = open(&[]);
    rig.connect();
    let from = scratch(1);
    let original = copy(&mut rig, from);
    assert_eq!(standing(&rig, original), Some((from, Standing::Same)));

    let twin = duplicate(&mut rig, original);
    assert_eq!(linked(&rig, twin), None, "a Duplicate has no link");
    send(&mut rig, twin, other_spare());
    assert_eq!(
        standing(&rig, twin),
        Some((other_spare(), Standing::Same)),
        "the duplicate sent"
    );
    assert_eq!(
        standing(&rig, original),
        Some((from, Standing::Same)),
        "the original stays"
    );

    let again = copy(&mut rig, from);
    assert_eq!(linked(&rig, again), None, "a second copy of a linked slot");
    assert_eq!(standing(&rig, original), Some((from, Standing::Same)));
    drop(rig);
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l04_an_edited_copy_saved_is_newer_here_and_sync_lands_the_edit() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let mut rig = open(&[]);
    rig.connect();
    let from = scratch(2);
    let id = copy(&mut rig, from);
    let attack = field(&entity(&rig, id).bytes, "sample_panel.attack").value;
    let to = if attack == "0" { "10" } else { "0" };
    let edited = edit(&mut rig, id, &[set("sample_panel.attack", to)]);
    assert_eq!(queued(&rig), [], "an edit queues nothing");

    act(&mut rig, vec![Act::SaveDoc(id)]);
    assert_eq!(standing(&rig, id), Some((from, Standing::Ahead)));
    assert_eq!(queued(&rig), [], "the save queues nothing");
    assert_eq!(changes(&rig), [(id, Change::Ahead)]);
    sync(&mut rig);
    assert_eq!(standing(&rig, id), Some((from, Standing::Same)));
    drop(rig);

    assert_eq!(
        bank().body(from),
        edited[HEADER..],
        "{} holds the edit",
        shown(from)
    );
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l05_a_slot_overwritten_behind_drawbars_back_is_newer_there_and_saving_queues_nothing() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let mut rig = open(&[]);
    rig.connect();
    let from = scratch(3);
    let id = copy(&mut rig, from);
    rig.release();
    let other = bank().kept(4);
    bank().cli.ok(&[
        "program",
        "put",
        path_arg(&other.file),
        &shown(from),
        "--yes",
    ]);
    rig.connect();

    assert_eq!(standing(&rig, id), Some((from, Standing::Theirs)));
    let by = |change: &Change| matches!(change, Change::Theirs | Change::Replaced { .. });
    let said = changes(&rig);
    assert!(
        matches!(said.as_slice(), [(row, change)] if *row == id && by(change)),
        "the keyboard's side changed: {said:?}"
    );
    act(&mut rig, vec![Act::SaveDoc(id)]);
    assert_eq!(queued(&rig), [], "saving an unchanged file queues nothing");
    assert_eq!(standing(&rig, id), Some((from, Standing::Theirs)));
    drop(rig);
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l06_a_slot_emptied_behind_drawbars_back_is_removed_and_an_explicit_send_puts_it_back() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let mut rig = open(&[]);
    rig.connect();
    let from = scratch(5);
    let id = copy(&mut rig, from);
    rig.release();
    bank().cli.ok(&["program", "delete", &shown(from), "--yes"]);
    rig.connect();

    let linked = linked(&rig, id).expect("still linked");
    assert_eq!(
        (linked.at, linked.there, linked.standing),
        (Some(from), There::Vacant, Standing::Gone)
    );
    assert_eq!(
        changes(&rig),
        [(id, Change::Removed)],
        "no way until one is picked"
    );
    tick(
        &mut rig,
        vec![Act::Send {
            id,
            class: PROGRAM,
            at: from,
        }],
    );
    assert_eq!(queued(&rig), [], "its own slot is Sync's to put back");
    let sheet = rig.bench.device.asks.sheet.as_ref();
    assert_eq!(
        sheet.and_then(|sheet| sheet.picked),
        Some(id),
        "the sheet opens on it"
    );
    sync_with(&mut rig, id, Way::Up);
    assert_eq!(standing(&rig, id), Some((from, Standing::Same)));
    drop(rig);

    let kept = bank().kept(5);
    assert_eq!(bank().held(from), Some(kept.held.clone()));
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l07_a_file_whose_header_names_the_slot_holding_it_links_on_its_own_and_a_copy_does_not() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let slot = scratch(6);
    let kept = bank().kept(6);
    let said = bank().cli.ok(&["program", "info", path_arg(&kept.file)]);
    assert!(
        said.contains("bank 8 slot 6"),
        "the backup's header names its slot: {said}"
    );
    let name = format!("{MARK}8-6.ne5p");
    let mut rig = open(&[(&name, &kept.file)]);
    let id = read(&rig, &name).expect("read");
    assert_eq!(linked(&rig, id), None, "nothing is attached");
    rig.connect();

    assert_eq!(standing(&rig, id), Some((slot, Standing::Same)));
    assert_eq!(
        log_lines(&rig, "linked to the slot"),
        1,
        "{}",
        rig.bench.log.tail(20)
    );
    assert_eq!(
        log_lines(&rig, "1 file linked to the slot that holds it."),
        1
    );

    let copy = format!("{MARK}8-6 copy.ne5p");
    let before = ids(&rig);
    act(
        &mut rig,
        vec![Act::Import {
            name: copy.clone(),
            bytes: fs::read(&kept.file).unwrap(),
        }],
    );
    wait(&mut rig, "the copy is read", ATTACH_LIMIT, |rig| {
        arrived(rig, &before).is_some()
    });
    let twin = arrived(&rig, &before).expect("imported");
    assert_eq!(linked(&rig, twin), None, "one file per slot");
    assert!(
        also_at(&rig, twin).is_empty(),
        "the slot is the linked file's, so the copy is offered nothing"
    );
    assert_eq!(standing(&rig, id), Some((slot, Standing::Same)));
    assert_eq!(log_lines(&rig, "linked to the slot"), 1);
    drop(rig);
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l07_two_files_naming_one_slot_at_once_link_neither() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let slot = scratch(6);
    let kept = bank().kept(6);
    let names = [format!("{MARK}8-6.ne5p"), format!("{MARK}8-6 copy.ne5p")];
    let mut rig = open(&[(&names[0], &kept.file), (&names[1], &kept.file)]);
    rig.connect();

    for name in &names {
        let id = read(&rig, name).expect("read");
        assert_eq!(linked(&rig, id), None, "“{name}” is linked");
        let also = also_at(&rig, id);
        assert_eq!(also, [slot], "“{name}” is hinted at its slot");
    }
    assert_eq!(log_lines(&rig, "linked to the slot"), 0);
    drop(rig);
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l08_a_link_follows_a_program_drawbar_moves_and_moves_back() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let mut rig = open(&[]);
    rig.connect();
    let from = scratch(7);
    let id = copy(&mut rig, from);
    let rearrange = |from, to| Act::Rearrange {
        class: PROGRAM,
        from,
        to,
    };

    act(&mut rig, vec![rearrange(from, spare())]);
    assert_eq!(standing(&rig, id), Some((spare(), Standing::Same)), "moved");
    act(&mut rig, vec![rearrange(spare(), from)]);
    assert_eq!(
        standing(&rig, id),
        Some((from, Standing::Same)),
        "moved back"
    );
    drop(rig);

    assert_eq!(bank().held(spare()), None);
    assert_eq!(bank().held(from), Some(bank().kept(7).held.clone()));
    restores.finish();
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l09_turning_the_model_dial_names_the_piano_there_by_the_hash_of_its_name() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let mut rig = open(&[]);
    rig.connect();
    let id = copy(&mut rig, scratch(8));
    let bytes = entity(&rig, id).bytes.to_vec();
    let category = field(&bytes, PIANO_CATEGORY);
    let stored = (category.spec.legal)()
        .iter()
        .position(|legal| *legal == category.value)
        .and_then(|raw| u8::try_from(raw).ok())
        .expect("a stored category");
    let current = field(&bytes, PIANO_MODEL).value;
    let state = &rig.bench.device.state;
    let pianos = state.pianos_of(stored).expect("the Type's bank was read");
    let (model, name) = pianos
        .iter()
        .enumerate()
        .filter(|(model, _)| model.to_string() != current)
        .find_map(|(model, piano)| Some((model, piano.as_ref()?.name.trim().to_string())))
        .expect("another piano of the program's Type");
    let mut sets = vec![set(PIANO_MODEL, model)];
    let follows = crate::document::piano_follows(&sets, &field_list(&bytes), &rig.bench.device);
    let expected = library_id(&name);
    assert_eq!(
        follows,
        Some(set(PIANO_ID, expected)),
        "Model {model} of {} names “{name}”",
        category.value
    );
    sets.extend(follows);
    edit(&mut rig, id, &sets);
    send(&mut rig, id, spare());
    assert_eq!(standing(&rig, id), Some((spare(), Standing::Same)));
    drop(rig);

    let deps = bank().deps(spare());
    eprintln!("nord program deps {}:\n{deps}", shown(spare()));
    let row = deps
        .lines()
        .find(|line| line.trim_start().starts_with("pianos"))
        .unwrap_or_else(|| panic!("no piano row in {deps}"));
    assert!(
        row.contains(&format!("{expected:#010x}")) && row.trim_end().ends_with(&name),
        "the instrument reports “{name}” ({expected:#010x}): {row}"
    );
    restores.finish();
}

fn field_list(bytes: &[u8]) -> Vec<nord_format::fields::Field> {
    crate::fields::apply(bytes, &[]).expect("fields").0
}

#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn l10_a_program_placed_on_a_slot_lists_the_sample_it_plays_and_sync_writes_it_first() {
    if !attached() {
        return;
    }
    let restores = Restores::begin();
    let source = std::env::var("DRAWBAR_HARDWARE_SMALL_SAMPLE").map_or_else(
        |_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../nord-format/tests/fixtures/demo/drawbar-pad.nsmp")
        },
        PathBuf::from,
    );
    let sample = format!("{MARK}probe");
    let fresh = Fresh::Program.bytes().unwrap();
    let plays = vec![set("sample_panel.id", library_id(&sample))];
    let (_, program) = crate::fields::apply(&fresh, &plays).expect("set");
    let program_file = bank().dir.join("plays.ne5p");
    fs::write(&program_file, &program).unwrap();
    let sample_name = format!("{sample}.nsmp");
    let program_name = format!("{MARK}plays.ne5p");
    let mut rig = open(&[(&sample_name, &source), (&program_name, &program_file)]);
    rig.connect();
    let probe = read(&rig, &sample_name).expect("read");
    let id = read(&rig, &program_name).expect("read");

    act(
        &mut rig,
        vec![Act::Send {
            id,
            class: PROGRAM,
            at: spare(),
        }],
    );
    assert_eq!(
        queued(&rig),
        [(id, PROGRAM, spare())],
        "only what was placed"
    );
    let played = changes(&rig);
    assert!(
        played.contains(&(probe, Change::Plays { by: id })),
        "the sample is a row of its own: {played:?}\n{}",
        rig.bench.log.tail(20)
    );

    let mut order = Vec::new();
    let mut seen = Vec::new();
    tick(&mut rig, vec![Act::Sync(None)]);
    synced(&mut rig, |rig| {
        for id in [probe, id] {
            let held = rig.bench.queue.holds(id);
            if held && !seen.contains(&id) {
                seen.push(id);
            }
            if !held && seen.contains(&id) && !order.contains(&id) {
                order.push(id);
            }
        }
    });
    settle(&mut rig);
    assert_eq!(order, [probe, id], "the sample is written first");
    drop(rig);

    let deps = bank().deps(spare());
    eprintln!("nord program deps {}:\n{deps}", shown(spare()));
    let id = format!("{:#010x}", library_id(&sample));
    let names = |line: &str| {
        let line = line.trim();
        line.starts_with("samples") && line.contains(&id) && line.ends_with(&sample)
    };
    assert!(
        deps.lines().any(names),
        "the instrument finds “{sample}” ({id}): {deps}"
    );
    restores.finish();
}
