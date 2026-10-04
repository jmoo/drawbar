//! drawbar's own send paths against an attached instrument: a library opened over a
//! temporary folder, the browser's acts, the queue, and the device worker over the USB
//! transport, with no window.
//!
//! Every test writes to the instrument, so each is ignored and skipped unless
//! `DRAWBAR_HARDWARE=1`. They run one at a time, in name order, and the last one empties
//! whatever the run left behind:
//!
//! ```sh
//! DRAWBAR_HARDWARE=1 cargo test -p drawbar --lib device::hardware -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! nord-cli reads the source objects off the instrument, reads back what drawbar wrote,
//! and deletes it. `DRAWBAR_HARDWARE_NORD` names its binary (default `nord`). The sources
//! are the slots `DRAWBAR_HARDWARE_PIANO` (default `1:6`) and `DRAWBAR_HARDWARE_OTHER_PIANO`
//! (`6:3`), two piano libraries of a few megabytes, and `DRAWBAR_HARDWARE_SAMPLE` (`1:20`),
//! a sample instrument of more than a megabyte. The bundle test reads the program at
//! `DRAWBAR_HARDWARE_PROGRAM` (`6:9`), one that plays a piano and a sample, and writes
//! nothing.
//!
//! ⚠️ A test writes only to the last vacant slots of bank 1, as drawbar's own walk found
//! them when it attached, and deletes only a slot holding a name this suite gives
//! (`drawbar-hw-…`). Each slot it writes is recorded in a file under the system's temp
//! folder before the write, and emptied when the test ends, however it ends.

use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nord_usb::{Location, ObjectClass};

use super::{Connection, DeviceCmd};
use crate::browser::{apply, Act};
use crate::ondisk::OnDisk;
use crate::queue::Diff;
use crate::store::{Backend, Store};
use crate::strings::shown;
use crate::testing::{largest_allocation_anywhere, Bench, Temp};

/// The prefix of every name this suite writes to a slot, and the only names it deletes.
const MARK: &str = "drawbar-hw-";

/// The bytes of a CBIN header ahead of the body the instrument stores.
const HEADER: usize = 44;

/// More than a write takes, the backup read, delete and restore of a replace included.
const SEND_LIMIT: Duration = Duration::from_secs(300);

/// More than attaching takes: the partition table, and a walk of every class.
const ATTACH_LIMIT: Duration = Duration::from_secs(120);

/// The most any single allocation may take while a file is sent: a few transfer chunks.
const BOUNDED: usize = 128 << 10;

/// Whether this run may write to the instrument, and one is attached.
fn attached() -> bool {
    if std::env::var("DRAWBAR_HARDWARE").as_deref() != Ok("1") {
        eprintln!("skipped: set DRAWBAR_HARDWARE=1 to write to an attached instrument");
        return false;
    }
    match nord_usb::transport::usb::list() {
        Ok(found) if !found.is_empty() => true,
        Ok(_) => {
            eprintln!("skipped: no instrument is attached");
            false
        }
        Err(e) => {
            eprintln!("skipped: USB devices could not be listed: {e}");
            false
        }
    }
}

fn slot_from(var: &str, default: &str) -> Location {
    let text = std::env::var(var).unwrap_or_else(|_| default.to_string());
    let (bank, slot) = text
        .split_once(':')
        .unwrap_or_else(|| panic!("{var}={text} is not BANK:SLOT"));
    let number = |part: &str| -> u32 {
        part.trim()
            .parse()
            .unwrap_or_else(|_| panic!("{var}={text} is not BANK:SLOT"))
    };
    Location::from_user(number(bank), number(slot))
}

fn noun(class: ObjectClass) -> &'static str {
    match class {
        ObjectClass::Piano => "piano",
        ObjectClass::Sample => "sample",
        other => panic!("this suite writes no {}", other.label()),
    }
}

/// nord-cli, for what drawbar is not under test for: reading sources, reading back and
/// deleting.
struct Cli(PathBuf);

impl Cli {
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(&self.0)
            .args(["--color", "never"])
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("{} could not run: {e}", self.0.display()))
    }

    /// What a command printed to stdout and stderr, or why it failed.
    fn said(&self, args: &[&str]) -> Result<(String, String), String> {
        let out = self.run(args);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        match out.status.success() {
            true => Ok((String::from_utf8_lossy(&out.stdout).into_owned(), stderr)),
            false => Err(format!("nord {}: {stderr}", args.join(" "))),
        }
    }

    fn ok(&self, args: &[&str]) -> String {
        self.said(args).unwrap_or_else(|why| panic!("{why}")).0
    }

    /// The name of what a slot holds, or `None` where it is empty.
    fn info(&self, class: ObjectClass, slot: Location) -> Result<Option<String>, String> {
        let stdout = match self.said(&[noun(class), "info", &shown(slot)]) {
            Ok((stdout, _)) => stdout,
            Err(why) if why.contains("is empty") => return Ok(None),
            Err(why) => return Err(why),
        };
        let name = stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix("name:"))
            .ok_or_else(|| format!("no name in {stdout}"))?;
        Ok(Some(name.trim().trim_matches('"').to_string()))
    }

    fn get(&self, class: ObjectClass, slot: Location, out: &Path) {
        self.ok(&[
            noun(class),
            "get",
            &shown(slot),
            "-o",
            &out.display().to_string(),
        ]);
    }

    fn delete(&self, class: ObjectClass, slot: Location) -> Result<(), String> {
        self.said(&[noun(class), "delete", &shown(slot), "--yes"])
            .map(|_| ())
    }

    /// Every occupied slot of the class, and its name.
    fn list(&self, class: ObjectClass) -> Result<Vec<(Location, String)>, String> {
        let (list, _) = self.said(&[noun(class), "list"])?;
        Ok(list
            .lines()
            .filter_map(|line| {
                let (slot, rest) = line.trim().split_once(char::is_whitespace)?;
                let (bank, slot) = slot.split_once(':')?;
                let (_format, rest) = rest.trim_start().split_once(char::is_whitespace)?;
                let (_bytes, name) = rest.trim_start().split_once(char::is_whitespace)?;
                let at = Location::from_user(bank.parse().ok()?, slot.parse().ok()?);
                Some((at, name.trim().to_string()))
            })
            .collect())
    }

    /// How many objects the class holds, from the total its list ends with on stderr.
    fn count(&self, class: ObjectClass) -> usize {
        let (_, list) = self
            .said(&[noun(class), "list"])
            .unwrap_or_else(|why| panic!("{why}"));
        let last = list.lines().rev().find(|line| !line.trim().is_empty());
        last.and_then(|line| line.split_whitespace().next()?.parse().ok())
            .unwrap_or_else(|| panic!("no count in {list}"))
    }

    fn status(&self) -> String {
        self.ok(&["device", "status"])
    }
}

/// What this test process shares: nord-cli, the sources it read, the counts it found at
/// the start, and the record of every slot it wrote.
struct Run {
    dir: PathBuf,
    cli: Cli,
    pianos: usize,
    samples: usize,
    /// What each class held when the run started.
    held: HashMap<u32, Vec<(Location, String)>>,
    sources: Mutex<HashMap<String, PathBuf>>,
}

static RUN: OnceLock<Run> = OnceLock::new();

/// ⚠️ The first call counts the instrument with nord-cli, so it must come before drawbar
/// claims the interface.
fn run() -> &'static Run {
    RUN.get_or_init(|| {
        let nord = std::env::var("DRAWBAR_HARDWARE_NORD").unwrap_or_else(|_| "nord".into());
        let cli = Cli(PathBuf::from(nord));
        let dir = std::env::temp_dir().join(format!("drawbar-hardware-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("the run's folder");
        let pianos = cli.count(ObjectClass::Piano);
        let samples = cli.count(ObjectClass::Sample);
        let held = [ObjectClass::Piano, ObjectClass::Sample]
            .into_iter()
            .map(|class| {
                (
                    class.to_raw(),
                    cli.list(class).unwrap_or_else(|why| panic!("{why}")),
                )
            })
            .collect();
        eprintln!("at the start: {pianos} pianos, {samples} samples; run folder {dir:?}");
        Run {
            dir,
            cli,
            pianos,
            samples,
            held,
            sources: Mutex::default(),
        }
    })
}

impl Run {
    /// The object in `slot`, read once per run with nord-cli.
    fn source(&self, class: ObjectClass, slot: Location) -> PathBuf {
        let key = format!("{}-{}", noun(class), shown(slot).replace(':', "-"));
        let mut sources = self.sources.lock().expect("unpoisoned");
        if let Some(path) = sources.get(&key) {
            return path.clone();
        }
        let path = self.dir.join(&key);
        let started = Instant::now();
        self.cli.get(class, slot, &path);
        eprintln!(
            "read {} {} with nord-cli: {} bytes in {:.1?}",
            noun(class),
            shown(slot),
            fs::metadata(&path).expect("read").len(),
            started.elapsed()
        );
        sources.insert(key, path.clone());
        path
    }

    fn ledger(&self) -> PathBuf {
        self.dir.join("written")
    }

    fn written(&self) -> Vec<(ObjectClass, Location)> {
        let text = fs::read_to_string(self.ledger()).unwrap_or_default();
        text.lines()
            .filter_map(|line| {
                let (class, slot) = line.split_once(' ')?;
                let class = match class {
                    "piano" => ObjectClass::Piano,
                    "sample" => ObjectClass::Sample,
                    _ => return None,
                };
                let (bank, slot) = slot.split_once(':')?;
                Some((
                    class,
                    Location::from_user(bank.parse().ok()?, slot.parse().ok()?),
                ))
            })
            .collect()
    }

    fn record(&self, slots: &[(ObjectClass, Location)]) {
        let text: String = slots
            .iter()
            .map(|(class, slot)| format!("{} {}\n", noun(*class), shown(*slot)))
            .collect();
        fs::write(self.ledger(), text).expect("the ledger is written");
    }

    /// Note that this run is about to write `slot`.
    fn claim(&self, class: ObjectClass, slot: Location) {
        let mut written = self.written();
        if !written.contains(&(class, slot)) {
            written.push((class, slot));
        }
        self.record(&written);
    }

    /// Slots of `class` that were empty when the run started and now hold a name this
    /// suite gives, apart from those it claimed: objects a write left somewhere it did not
    /// address.
    fn strays(&self, class: ObjectClass) -> Result<Vec<(Location, String)>, String> {
        let before = &self.held[&class.to_raw()];
        let claimed = self.written();
        let mut strays: Vec<(Location, String)> = self
            .cli
            .list(class)?
            .into_iter()
            .filter(|(slot, name)| {
                name.starts_with(MARK)
                    && !before.iter().any(|(held, _)| held == slot)
                    && !claimed.contains(&(class, *slot))
            })
            .collect();
        strays.sort_by_key(|(slot, _)| (slot.bank, slot.slot));
        Ok(strays)
    }

    /// Empty a slot this run wrote, if it holds a name this suite gives, and take it off
    /// the ledger once it is empty.
    fn empty(&self, class: ObjectClass, slot: Location) -> Result<(), String> {
        let place = format!("{} {}", noun(class), shown(slot));
        match self.cli.info(class, slot)? {
            None => {}
            Some(name) if name.starts_with(MARK) => {
                self.cli.delete(class, slot)?;
                eprintln!("deleted {place} ({name:?})");
            }
            Some(name) => {
                return Err(format!(
                    "{place} holds {name:?}, which this suite did not write; left alone"
                ))
            }
        }
        if let Some(name) = self.cli.info(class, slot)? {
            return Err(format!("{place} still holds {name:?} after a delete"));
        }
        let mut written = self.written();
        written.retain(|held| *held != (class, slot));
        self.record(&written);
        Ok(())
    }
}

/// The slots one test wrote, emptied when it ends.
///
/// ⚠️ Declared before the [`Rig`], so the rig releases the instrument first and nord-cli
/// can claim it.
#[derive(Default)]
struct Claims(Vec<(ObjectClass, Location)>);

impl Claims {
    fn take(&mut self, class: ObjectClass, slot: Location) {
        run().claim(class, slot);
        self.0.push((class, slot));
    }
}

impl Drop for Claims {
    fn drop(&mut self) {
        let mut classes: Vec<ObjectClass> = self.0.iter().map(|(class, _)| *class).collect();
        classes.dedup();
        for class in classes {
            match run().strays(class) {
                Ok(strays) => strays
                    .into_iter()
                    .for_each(|(slot, _)| self.take(class, slot)),
                Err(why) => eprintln!("cleanup: {why}"),
            }
        }
        for (class, slot) in self.0.drain(..) {
            if let Err(why) = run().empty(class, slot) {
                eprintln!("cleanup: {why}");
            }
        }
    }
}

/// Nothing this run wrote landed anywhere but the slots it addressed.
fn assert_no_strays(class: ObjectClass) {
    let strays = run().strays(class).unwrap_or_else(|why| panic!("{why}"));
    assert!(
        strays.is_empty(),
        "{} slots empty at the start now hold what this run wrote: {strays:?}",
        noun(class)
    );
}

/// A library file placed in the rig's library under a name of this suite.
fn place(library: &Temp, name: &str, from: &Path) -> PathBuf {
    let to = library.at(name);
    fs::copy(from, &to).unwrap_or_else(|e| panic!("{from:?} -> {to:?}: {e}"));
    to
}

/// Where two CBIN files' bodies first differ, if they do.
fn body_difference(here: &Path, there: &Path) -> Option<String> {
    let (here, there) = (fs::read(here).unwrap(), fs::read(there).unwrap());
    let (here, there) = (
        &here[HEADER.min(here.len())..],
        &there[HEADER.min(there.len())..],
    );
    if here == there {
        return None;
    }
    let first = here.iter().zip(there).position(|(a, b)| a != b);
    Some(format!(
        "{} and {} body bytes, first differing at {first:?}",
        here.len(),
        there.len()
    ))
}

/// What one send through the queue came to.
struct Sending {
    outcome: Result<(), String>,
    took: Duration,
    /// The largest single allocation on any thread while it ran.
    largest: usize,
    /// Every range of the asset's file the send read.
    reads: Vec<Range<u64>>,
    /// The largest occupant backup seen in the library's `.drawbar/tmp`.
    backup: u64,
}

impl Sending {
    fn oversized_reads(&self, len: u64) -> Vec<&Range<u64>> {
        self.reads
            .iter()
            .filter(|read| read.end - read.start >= len.min(BOUNDED as u64))
            .collect()
    }
}

/// The app with no window: a library over a temporary folder and the instrument it
/// attaches, run a frame at a time as [`crate::app::DrawbarApp`] runs them.
struct Rig {
    library: Temp,
    store: Store,
    bench: Bench,
}

impl Rig {
    /// A library holding `files`, each read, resting in its file and checked.
    fn open(files: &[(&str, &Path)]) -> Rig {
        let library = Temp::new();
        for (name, from) in files {
            place(&library, name, from);
        }
        let bench = Bench::new();
        let store = Store::start(Backend::start(&bench.ctx, library.0.clone()));
        let mut rig = Rig {
            library,
            store,
            bench,
        };
        let names: Vec<String> = files.iter().map(|(name, _)| name.to_string()).collect();
        rig.until(
            "the library rests and checks its files",
            ATTACH_LIMIT,
            |rig| {
                names.iter().all(|name| {
                    rig.bench
                        .workspace
                        .listed()
                        .find(|entity| entity.name == *name)
                        .is_some_and(|entity| entity.rests().is_some() && entity.sendable().is_ok())
                })
            },
        );
        rig
    }

    /// One frame of the app's update, without the panels: the library, the instrument's
    /// events, the queue, the acts, and the next command.
    fn frame(&mut self, acts: Vec<Act>) {
        let Bench {
            ctx,
            browser,
            shell,
            workspace,
            device,
            tabs,
            queue,
            log,
        } = &mut self.bench;
        log.tick(ctx);
        let _ = workspace.poll(log);
        let listed: Vec<u64> = workspace.listed().map(|entity| entity.id).collect();
        for id in listed {
            workspace.hurry(id);
        }
        let released = self.store.poll(workspace, browser, queue, log);
        device.poll(log, workspace, tabs, queue);
        crate::queue::follow(workspace, device, queue, log);
        let mut acts = acts;
        if released {
            acts.push(Act::SendAll);
        }
        // As the app holds a send until the library's files are checked for outside
        // changes.
        let sending = acts.iter().any(|act| matches!(act, Act::SendAll));
        if !released && sending && self.store.hold_send() {
            acts.retain(|act| !matches!(act, Act::SendAll));
        }
        apply(browser, shell, acts, workspace, device, tabs, queue, log);
        device.keep_occupants_in(self.store.tmp());
        device.pump();
    }

    fn until(&mut self, what: &str, limit: Duration, done: impl Fn(&Rig) -> bool) {
        let started = Instant::now();
        loop {
            self.frame(Vec::new());
            if done(self) {
                return;
            }
            assert!(
                started.elapsed() < limit,
                "{what}: nothing after {limit:?}. If the instrument stopped answering, it \
                 may be wedged: stop, and power-cycle it.\n{}",
                self.bench.log.tail(40)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Whether the instrument has nothing left to do: no command running or waiting, and
    /// no walk.
    fn idle(&self) -> bool {
        let device = &self.bench.device;
        let state = &device.state;
        state.connected()
            && !state.classes().is_empty()
            && state.in_flight.is_none()
            && device.queued().is_empty()
            && state.classes().into_iter().all(|class| {
                !state
                    .scan
                    .progress(class)
                    .is_some_and(|progress| progress.running)
            })
    }

    /// Attach the instrument as the Connect button does, and wait for the walk of every
    /// class.
    fn connect(&mut self) {
        let started = Instant::now();
        self.frame(vec![Act::Connect]);
        loop {
            self.frame(Vec::new());
            if self.idle() {
                break;
            }
            assert!(
                !matches!(self.bench.device.state.connection, Connection::Disconnected),
                "no instrument could be attached; is nord-cli or another drawbar holding \
                 it?\n{}",
                self.bench.log.tail(20)
            );
            assert!(
                started.elapsed() < ATTACH_LIMIT,
                "attaching took more than {ATTACH_LIMIT:?}\n{}",
                self.bench.log.tail(20)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("attached and walked in {:.1?}", started.elapsed());
    }

    /// Release the instrument as the Disconnect button does, and wait until the worker
    /// has let the interface go.
    fn release(&mut self) {
        if !self.bench.device.state.connected() {
            return;
        }
        self.frame(vec![Act::Disconnect]);
        let started = Instant::now();
        while !matches!(self.bench.device.state.connection, Connection::Disconnected) {
            if started.elapsed() > SEND_LIMIT {
                eprintln!("the worker did not let the instrument go");
                return;
            }
            self.frame(Vec::new());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn asset(&self, name: &str) -> u64 {
        let mut listed = self.bench.workspace.listed();
        listed.find(|entity| entity.name == name).expect(name).id
    }

    fn file(&self, id: u64) -> Arc<OnDisk> {
        let entity = self.bench.workspace.get(id).expect("listed");
        entity.rests().expect("it rests in its file").clone()
    }

    /// The last vacant slots of bank 1, as the walk on attaching found them.
    fn vacant(&self, class: ObjectClass, nth_from_last: usize) -> Location {
        let vacant: Vec<Location> = self
            .bench
            .device
            .state
            .slots_of(class, 1)
            .filter(|(_, held)| held.is_none())
            .map(|(slot, _)| slot)
            .collect();
        *vacant
            .iter()
            .rev()
            .nth(nth_from_last)
            .unwrap_or_else(|| panic!("bank 1 of {} has too few vacant slots", class.label()))
    }

    fn backups(&self) -> Vec<(String, u64)> {
        let Ok(dir) = fs::read_dir(self.library.at(crate::store::TMP)) else {
            return Vec::new();
        };
        dir.filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("nord-rescued")
            })
            .map(|entry| {
                let len = entry.metadata().map_or(0, |meta| meta.len());
                (entry.file_name().to_string_lossy().into_owned(), len)
            })
            .collect()
    }

    /// Run `acts`, which queue `id` and send it, until its write lands or fails, calling
    /// `during` with every range of `file` the send has read so far.
    fn send(
        &mut self,
        acts: Vec<Act>,
        id: u64,
        file: &OnDisk,
        mut during: impl FnMut(&[Range<u64>]),
    ) -> Sending {
        file.take_reads();
        let started = Instant::now();
        let mut reads = Vec::new();
        let mut backup = 0;
        let (outcome, largest) = largest_allocation_anywhere(|| {
            self.frame(acts);
            assert!(self.bench.queue.holds(id), "the asset was not queued");
            loop {
                self.frame(Vec::new());
                reads.extend(file.take_reads());
                during(&reads);
                for (_, len) in self.backups() {
                    backup = backup.max(len);
                }
                let queue = &self.bench.queue;
                let failure = queue.entry(id).and_then(|held| held.failure.clone());
                if self.bench.device.state.in_flight.is_none() {
                    match (queue.holds(id), failure) {
                        (false, _) => return Ok(()),
                        (true, Some(why)) => return Err(why),
                        (true, None) => {}
                    }
                }
                assert!(
                    started.elapsed() < SEND_LIMIT,
                    "the send took more than {SEND_LIMIT:?}. If the instrument stopped \
                     answering, it may be wedged: stop, and power-cycle it.\n{}",
                    self.bench.log.tail(40)
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let took = started.elapsed();
        self.until("the walk after the write", ATTACH_LIMIT, Rig::idle);
        Sending {
            outcome,
            took,
            largest,
            reads,
            backup,
        }
    }

    fn logged(&self, text: &str) -> bool {
        self.bench.log.iter().any(|entry| entry.text.contains(text))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.release();
    }
}

/// How far a send has got through its file: the end of the furthest read since the reads
/// started over at the body, once the pass that checks the file is done.
fn sent(reads: &[Range<u64>]) -> Option<u64> {
    let restart = reads
        .windows(2)
        .position(|pair| pair[1].start < pair[0].start)?;
    reads[restart + 1..].iter().map(|read| read.end).max()
}

/// Report a send for the run's output.
fn report(what: &str, sending: &Sending, len: u64) {
    eprintln!(
        "{what}: {:?} in {:.1?}; largest allocation {} bytes ({:.3}% of {len}); {} reads of \
         the file, longest {} bytes; backup file peaked at {} bytes",
        sending.outcome,
        sending.took,
        sending.largest,
        sending.largest as f64 * 100.0 / len as f64,
        sending.reads.len(),
        sending
            .reads
            .iter()
            .map(|read| read.end - read.start)
            .max()
            .unwrap_or(0),
        sending.backup,
    );
}

/// The send stayed within a few transfer chunks of memory, and read its file a chunk at a
/// time.
fn assert_streamed(sending: &Sending, len: u64) {
    assert!(
        sending.largest <= BOUNDED,
        "a single allocation of {} bytes while sending {len}",
        sending.largest
    );
    assert_eq!(
        sending.oversized_reads(len),
        Vec::<&Range<u64>>::new(),
        "the file was read in pieces too large to be a transfer chunk"
    );
}

/// The slot holds a body identical to `source`'s, under `name`.
fn assert_holds(class: ObjectClass, slot: Location, source: &Path, name: &str) {
    let back = run().dir.join(format!(
        "back-{}-{}",
        noun(class),
        shown(slot).replace(':', "-")
    ));
    let held = run().cli.info(class, slot).expect("info");
    assert_eq!(
        held.as_deref(),
        Some(name),
        "{} {} is named",
        noun(class),
        shown(slot)
    );
    run().cli.get(class, slot, &back);
    let differs = body_difference(source, &back);
    let _ = fs::remove_file(&back);
    assert_eq!(
        differs,
        None,
        "{} {} holds another body",
        noun(class),
        shown(slot)
    );
}

/// Send a resting asset into a vacant slot through the queue, as dropping it on the slot
/// and pressing Send does, and check that it landed.
fn send_into_vacant(rig: &mut Rig, id: u64, class: ObjectClass, slot: Location) -> Sending {
    let file = rig.file(id);
    let sending = rig.send(
        vec![
            Act::Send {
                id,
                class,
                at: slot,
            },
            Act::SendAll,
        ],
        id,
        &file,
        |_| {},
    );
    report("send into a vacant slot", &sending, file.len);
    assert_eq!(sending.outcome, Ok(()), "{}", rig.bench.log.tail(30));
    sending
}

/// A piano library resting in its file is sent to an empty slot through the queue and the
/// worker: it lands byte for byte, the asset stands on the slot, and the send holds no
/// more than a few transfer chunks and never the file.
#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn s1_a_resting_piano_lands_in_an_empty_slot_streamed() {
    if !attached() {
        return;
    }
    let source = run().source(
        ObjectClass::Piano,
        slot_from("DRAWBAR_HARDWARE_PIANO", "1:6"),
    );
    let mut claims = Claims::default();
    let mut rig = Rig::open(&[("drawbar-hw-piano.npno", &source)]);
    rig.connect();
    let class = ObjectClass::Piano;
    let slot = rig.vacant(class, 0);
    claims.take(class, slot);
    let id = rig.asset("drawbar-hw-piano.npno");
    let len = rig.file(id).len;

    let sending = send_into_vacant(&mut rig, id, class, slot);

    assert!(
        sending.largest < (len / 100) as usize,
        "{} bytes is not under 1% of {len}",
        sending.largest
    );
    assert_streamed(&sending, len);
    let entity = rig.bench.workspace.get(id).unwrap();
    assert_eq!(
        entity.link,
        Some((class, slot)),
        "the asset stands on the slot"
    );
    rig.release();
    assert_no_strays(class);
    assert_holds(class, slot, &source, "drawbar-hw-piano");
}

/// Sending onto a slot that holds something keeps the occupant in a file under the
/// library's `.drawbar/tmp` until the new object has landed, then removes it.
#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn s2_a_replace_keeps_the_occupant_in_a_file_until_the_write_lands() {
    if !attached() {
        return;
    }
    let source = run().source(
        ObjectClass::Piano,
        slot_from("DRAWBAR_HARDWARE_PIANO", "1:6"),
    );
    let mut claims = Claims::default();
    let mut rig = Rig::open(&[("drawbar-hw-piano.npno", &source)]);
    rig.connect();
    let class = ObjectClass::Piano;
    let slot = rig.vacant(class, 0);
    claims.take(class, slot);
    let id = rig.asset("drawbar-hw-piano.npno");
    let file = rig.file(id);
    send_into_vacant(&mut rig, id, class, slot);

    let sending = rig.send(
        vec![
            Act::Replace {
                id,
                class,
                at: slot,
            },
            Act::SendAll,
        ],
        id,
        &file,
        |_| {},
    );
    report("replace", &sending, file.len);

    assert_eq!(sending.outcome, Ok(()), "{}", rig.bench.log.tail(30));
    assert!(
        rig.logged(&format!("deleting {} to make room", shown(slot))),
        "{}",
        rig.bench.log.tail(30)
    );
    assert!(
        sending.backup >= file.len - HEADER as u64,
        "the occupant waited in .drawbar/tmp: the largest file there was {} bytes",
        sending.backup
    );
    assert_eq!(
        rig.backups(),
        [],
        "the backup is removed once the write lands"
    );
    assert!(sending.largest < (file.len / 100) as usize);
    assert_streamed(&sending, file.len);
    rig.release();
    assert_no_strays(class);
    assert_holds(class, slot, &source, "drawbar-hw-piano");
}

/// A file overwritten while it is sent onto an occupied slot fails the send before its
/// last chunk, the occupant is put back as it was, and the instrument stays attached and
/// answering.
#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn s3_a_file_changed_mid_send_restores_the_occupant() {
    if !attached() {
        return;
    }
    let piano = run().source(
        ObjectClass::Piano,
        slot_from("DRAWBAR_HARDWARE_PIANO", "1:6"),
    );
    let other = run().source(
        ObjectClass::Piano,
        slot_from("DRAWBAR_HARDWARE_OTHER_PIANO", "6:3"),
    );
    let mut claims = Claims::default();
    let mut rig = Rig::open(&[
        ("drawbar-hw-piano.npno", &piano),
        ("drawbar-hw-other.npno", &other),
    ]);
    rig.connect();
    let class = ObjectClass::Piano;
    let slot = rig.vacant(class, 0);
    claims.take(class, slot);
    let occupant = rig.asset("drawbar-hw-piano.npno");
    send_into_vacant(&mut rig, occupant, class, slot);

    let id = rig.asset("drawbar-hw-other.npno");
    let file = rig.file(id);
    let path = rig.library.at("drawbar-hw-other.npno");
    let mut sent_from: Option<Instant> = None;
    let mut changed = None;
    let sending = rig.send(
        vec![
            Act::Replace {
                id,
                class,
                at: slot,
            },
            Act::SendAll,
        ],
        id,
        &file,
        |reads| {
            let Some(sent) = sent(reads) else {
                return;
            };
            let since = *sent_from.get_or_insert_with(Instant::now);
            if changed.is_none() && sent >= file.len / 4 {
                changed = Some((since.elapsed(), sent));
                let path = path.clone();
                let tail = file.len - (64 << 10);
                std::thread::spawn(move || {
                    let mut flipped = vec![0u8; 4096];
                    let handle = fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .expect("the file opens");
                    std::os::unix::fs::FileExt::read_exact_at(&handle, &mut flipped, tail)
                        .expect("the tail reads");
                    flipped.iter_mut().for_each(|byte| *byte ^= 0xff);
                    std::os::unix::fs::FileExt::write_all_at(&handle, &flipped, tail)
                        .expect("the tail is overwritten");
                    handle.sync_all().expect("on disk");
                })
                .join()
                .expect("the overwrite ran");
            }
        },
    );
    report("replace with a file changed mid-send", &sending, file.len);
    eprintln!("the file was overwritten {changed:?} (time into the send, bytes sent)");
    eprintln!("{}", rig.bench.log.tail(25));

    assert!(changed.is_some(), "the send never got a quarter of the way");
    let why = sending.outcome.clone().expect_err("the send failed");
    assert!(
        why.contains("changed after its checksum was checked"),
        "the failure says the file changed: {why}"
    );
    assert!(
        why.contains("was restored, and is unchanged"),
        "the occupant was put back: {why}"
    );
    assert!(
        rig.bench.device.state.connected(),
        "the instrument is still attached"
    );
    assert!(!rig.logged("went away"), "{}", rig.bench.log.tail(30));
    assert_eq!(rig.backups(), [], "no backup is left once it is restored");
    assert_streamed(&sending, file.len);

    rig.bench.device.state.detail = Default::default();
    let ask = DeviceCmd::SlotInfo { class, at: slot };
    let Bench { device, log, .. } = &mut rig.bench;
    device.send(ask, log);
    rig.until("drawbar reads the slot again", ATTACH_LIMIT, |rig| {
        rig.bench.device.state.detail.info.is_some()
    });
    let info = rig.bench.device.state.detail.info.clone().flatten();
    assert_eq!(
        info.map(|info| info.name),
        Some("drawbar-hw-piano".to_string()),
        "drawbar still reads the slot, and finds the occupant"
    );

    rig.release();
    assert_no_strays(class);
    eprintln!("{}", run().cli.status());
    assert_holds(class, slot, &piano, "drawbar-hw-piano");
}

/// A file cut short while it is sent into an empty slot fails the send, and the slot is
/// left empty: no partial object, and the class holds as many objects as before.
#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn s4_a_file_cut_short_mid_send_leaves_an_empty_slot_empty() {
    if !attached() {
        return;
    }
    let source = run().source(
        ObjectClass::Piano,
        slot_from("DRAWBAR_HARDWARE_PIANO", "1:6"),
    );
    let before = run().cli.status();
    let pianos = run().cli.count(ObjectClass::Piano);
    let mut claims = Claims::default();
    let mut rig = Rig::open(&[("drawbar-hw-piano.npno", &source)]);
    rig.connect();
    let class = ObjectClass::Piano;
    let slot = rig.vacant(class, 1);
    claims.take(class, slot);
    let id = rig.asset("drawbar-hw-piano.npno");
    let file = rig.file(id);
    let path = rig.library.at("drawbar-hw-piano.npno");
    let mut sent_from: Option<Instant> = None;
    let mut cut = None;
    let sending = rig.send(
        vec![
            Act::Send {
                id,
                class,
                at: slot,
            },
            Act::SendAll,
        ],
        id,
        &file,
        |reads| {
            let Some(sent) = sent(reads) else {
                return;
            };
            let since = *sent_from.get_or_insert_with(Instant::now);
            if cut.is_none() && sent >= file.len / 4 {
                cut = Some((since.elapsed(), sent));
                let path = path.clone();
                let to = file.len / 2;
                std::thread::spawn(move || {
                    fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .and_then(|handle| handle.set_len(to))
                        .expect("the file is cut short");
                })
                .join()
                .expect("the cut ran");
            }
        },
    );
    report("send of a file cut short mid-send", &sending, file.len);
    eprintln!("the file was cut short {cut:?} (time into the send, bytes sent)");
    eprintln!("{}", rig.bench.log.tail(25));

    assert!(cut.is_some(), "the send never got a quarter of the way");
    let why = sending.outcome.clone().expect_err("the send failed");
    assert!(
        why.contains("could not be read from its file"),
        "the failure says the file stopped reading: {why}"
    );
    assert!(
        rig.bench.device.state.connected(),
        "the instrument is still attached"
    );
    assert!(!rig.logged("went away"), "{}", rig.bench.log.tail(30));
    let walked = rig.bench.device.state.slot(class, slot);
    eprintln!(
        "drawbar's walk after the failure finds {}: {walked:?}",
        shown(slot)
    );
    assert_streamed(&sending, file.len);

    rig.release();
    assert_no_strays(class);
    let after = run().cli.status();
    eprintln!("before:\n{before}\nafter:\n{after}");
    let held = run().cli.info(class, slot).expect("info");
    eprintln!("nord piano info {}: {held:?}", shown(slot));
    assert_eq!(held, None, "{} holds a partial object", shown(slot));
    assert_eq!(
        run().cli.count(class),
        pianos,
        "the class holds what it held"
    );
}

/// A sample instrument resting in its file is sent to an empty slot and then replaced with
/// itself, streamed from its file both times and kept in a file while it is replaced.
#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn s5_a_sample_round_trips_and_replaces_streamed() {
    if !attached() {
        return;
    }
    let source = run().source(
        ObjectClass::Sample,
        slot_from("DRAWBAR_HARDWARE_SAMPLE", "1:20"),
    );
    let mut claims = Claims::default();
    let mut rig = Rig::open(&[("drawbar-hw-sample.nsmp", &source)]);
    rig.connect();
    let class = ObjectClass::Sample;
    let slot = rig.vacant(class, 0);
    claims.take(class, slot);
    let id = rig.asset("drawbar-hw-sample.nsmp");
    let file = rig.file(id);

    let first = send_into_vacant(&mut rig, id, class, slot);
    assert_streamed(&first, file.len);
    assert_eq!(
        rig.bench.workspace.get(id).unwrap().link,
        Some((class, slot))
    );

    let again = rig.send(
        vec![
            Act::Replace {
                id,
                class,
                at: slot,
            },
            Act::SendAll,
        ],
        id,
        &file,
        |_| {},
    );
    report("replace", &again, file.len);
    assert_eq!(again.outcome, Ok(()), "{}", rig.bench.log.tail(30));
    assert!(
        again.backup >= file.len - HEADER as u64,
        "the occupant waited in .drawbar/tmp: the largest file there was {} bytes",
        again.backup
    );
    assert_eq!(
        rig.backups(),
        [],
        "the backup is removed once the write lands"
    );
    assert_streamed(&again, file.len);

    rig.release();
    assert_no_strays(class);
    assert_holds(class, slot, &source, "drawbar-hw-sample");
}

/// The queue compares a resting asset with the slot it would replace by the checksum of
/// the slot's body, read off the instrument: the same body is Identical, and another of
/// the same length is Checksum. Neither asset's file is read.
#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn s6_the_queue_compares_a_slot_by_checksum_without_reading_the_file() {
    if !attached() {
        return;
    }
    let source = run().source(
        ObjectClass::Sample,
        slot_from("DRAWBAR_HARDWARE_SAMPLE", "1:20"),
    );
    let edited = run().dir.join("sample-edited");
    let fields = run()
        .cli
        .ok(&["sample", "edit", &source.display().to_string(), "--fields"]);
    let root = fields
        .lines()
        .find_map(|line| line.strip_prefix("zone1.root_key"))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("a first zone with a root key");
    let to = if root == "C4" { "D4" } else { "C4" };
    run().cli.ok(&[
        "sample",
        "edit",
        &source.display().to_string(),
        "--set",
        &format!("zone1.root_key={to}"),
        "-o",
        &edited.display().to_string(),
    ]);
    assert_eq!(
        fs::metadata(&edited).unwrap().len(),
        fs::metadata(&source).unwrap().len(),
        "the edit keeps the length"
    );

    let mut claims = Claims::default();
    let mut rig = Rig::open(&[
        ("drawbar-hw-sample.nsmp", &source),
        ("drawbar-hw-edited.nsmp", &edited),
    ]);
    rig.connect();
    let class = ObjectClass::Sample;
    let slot = rig.vacant(class, 0);
    claims.take(class, slot);
    let id = rig.asset("drawbar-hw-sample.nsmp");
    let other = rig.asset("drawbar-hw-edited.nsmp");
    send_into_vacant(&mut rig, id, class, slot);
    let (file, other_file) = (rig.file(id), rig.file(other));
    file.take_reads();
    other_file.take_reads();

    let diff_of = |rig: &Rig, id: u64| match rig.bench.queue.entry(id).map(|held| &held.diff) {
        Some(Diff::Pending) | None => None,
        Some(Diff::Identical) => Some("Identical"),
        Some(Diff::Checksum) => Some("Checksum"),
        Some(Diff::Empty) => Some("Empty"),
        Some(Diff::Fields(_)) => Some("Fields"),
        Some(Diff::Bytes { .. }) => Some("Bytes"),
    };
    let started = Instant::now();
    rig.frame(vec![Act::Replace {
        id,
        class,
        at: slot,
    }]);
    rig.until("the compare read of the same body", SEND_LIMIT, |rig| {
        diff_of(rig, id).is_some() && rig.idle()
    });
    eprintln!(
        "the same body: {:?} in {:.1?}",
        diff_of(&rig, id),
        started.elapsed()
    );
    assert_eq!(diff_of(&rig, id), Some("Identical"));

    rig.frame(vec![Act::Replace {
        id: other,
        class,
        at: slot,
    }]);
    assert!(!rig.bench.queue.holds(id), "the slot takes one asset");
    let started = Instant::now();
    rig.until("the compare read for another body", SEND_LIMIT, |rig| {
        diff_of(rig, other).is_some() && rig.idle()
    });
    eprintln!(
        "another body: {:?} in {:.1?}",
        diff_of(&rig, other),
        started.elapsed()
    );
    assert_eq!(diff_of(&rig, other), Some("Checksum"));
    assert_eq!(
        file.take_reads(),
        [],
        "the compare read nothing of the file"
    );
    assert_eq!(other_file.take_reads(), [], "nor of the other");

    rig.frame(vec![Act::ClearQueue]);
    rig.release();
    assert_no_strays(class);
    assert_holds(class, slot, &source, "drawbar-hw-sample");
}

/// A program on the instrument exports as the bundle nord-cli makes of it: the program
/// and what it plays, copied in one session per class, a large piano through a file and
/// never held whole, each member byte for byte and the manifest the same. Reads only.
#[test]
#[ignore = "reads an attached instrument; see the module's documentation"]
fn s7_a_program_on_the_instrument_exports_as_nord_cli_bundles_it() {
    use nord_format::bundle::archive::{copy_member, Directory};

    if !attached() {
        return;
    }
    let run = run();
    let program = slot_from("DRAWBAR_HARDWARE_PROGRAM", "6:9");
    let theirs = run.dir.join("nord-cli.ne5pbundle");
    run.cli.ok(&[
        "bundle",
        "get",
        "program",
        &shown(program),
        "-o",
        theirs.to_str().unwrap(),
    ]);

    let mut rig = Rig::open(&[]);
    rig.connect();
    let slots = vec![(ObjectClass::Program, program)];
    let (ids, largest) = largest_allocation_anywhere(|| {
        let ids = Vec::new();
        rig.frame(vec![Act::ExportBundle { ids, slots }]);
        let started = Instant::now();
        loop {
            rig.frame(Vec::new());
            let Bench {
                workspace, device, ..
            } = &mut rig.bench;
            if let Some(slots) = device.take_gathered() {
                workspace.bundle_gathered(slots);
            }
            let arriving: Vec<Act> = device.take_fetched().into_iter().map(Act::Arrive).collect();
            if !arriving.is_empty() {
                rig.frame(arriving);
            }
            let Bench {
                workspace,
                browser,
                queue,
                ..
            } = &mut rig.bench;
            rig.store
                .sync(workspace, browser, queue, crate::store::Pass::Files);
            if let Some(ids) = rig.bench.workspace.bundle_ready() {
                return ids;
            }
            assert!(
                started.elapsed() < SEND_LIMIT,
                "the bundle's objects did not arrive\n{}",
                rig.bench.log.tail(30)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    eprintln!(
        "gathered {} objects; largest allocation {largest} bytes",
        ids.len()
    );
    rig.until("every copy is read", ATTACH_LIMIT, |rig| {
        let laid = crate::bundle::lay_out(&ids, &rig.bench.workspace, &rig.bench.device.state);
        matches!(laid, Ok(crate::bundle::Laid::Ready(_)))
    });
    let Ok(crate::bundle::Laid::Ready(export)) =
        crate::bundle::lay_out(&ids, &rig.bench.workspace, &rig.bench.device.state)
    else {
        panic!("the bundle was not laid out");
    };
    assert_eq!(
        export.plan.unmet,
        [],
        "everything the program plays is in it"
    );
    let ours = run.dir.join("drawbar.ne5pbundle");
    nord_usb::block_on(crate::bundle::write_to(&export, &ours)).unwrap();

    let members = |path: &Path| -> Vec<(String, u32, Vec<u8>)> {
        let mut file = fs::File::open(path).unwrap();
        let directory = Directory::read_from(&mut file).unwrap();
        let mut found = Vec::new();
        for member in &directory.members {
            let mut bytes = Vec::new();
            if member.entry.name.ends_with(".ne5p") || member.entry.name == "meta.xml" {
                copy_member(&mut file, member, &mut bytes).unwrap();
            }
            found.push((member.entry.name.clone(), member.entry.crc32, bytes));
        }
        found
    };
    assert_eq!(members(&ours), members(&theirs));
    let mut file = fs::File::open(&ours).unwrap();
    let directory = Directory::read_from(&mut file).unwrap();
    let biggest = directory
        .members
        .iter()
        .map(|m| m.entry.size)
        .max()
        .unwrap();
    assert!(
        (largest as u64) < u64::from(biggest),
        "an allocation of {largest} bytes held the {biggest}-byte member whole"
    );
}

/// Every slot this run wrote is empty again, and each class holds what it held when the
/// run started.
#[test]
#[ignore = "writes to an attached instrument; see the module's documentation"]
fn s9_every_slot_the_run_wrote_is_empty_again() {
    if !attached() {
        return;
    }
    let run = run();
    for (class, slot) in run.written() {
        if let Err(why) = run.empty(class, slot) {
            panic!("{why}");
        }
    }
    assert_eq!(run.written(), []);
    assert_eq!(run.cli.count(ObjectClass::Piano), run.pianos);
    assert_eq!(run.cli.count(ObjectClass::Sample), run.samples);
    eprintln!("at the end: {} pianos, {} samples", run.pianos, run.samples);
    let _ = fs::remove_dir_all(&run.dir);
}
