//! Every request the power-loss workloads make after their setup, faulted once:
//! failed before it takes effect, carried out with its reply lost, carried out
//! with its reply lost and every request after it failing, or, where it carries
//! bytes, cut short. The run goes on as an app would: a step that fails
//! ends the run, and the library either writes again, which settles what the
//! failure left, or is dropped. Either way the folder is then checked as a power
//! loss leaves it, and the install opened again: each path holds its old bytes,
//! its new bytes, or nothing while a record names it; every acknowledged entry is
//! there; recovery reaches the model before or after the failed step and nothing
//! else; settling again changes nothing; and the next write succeeds.

mod power_loss;

use std::cell::RefCell;

use power_loss::{
    before, check, models, verify, workloads, Acked, Async, Fault, Faulty, Lib, Place, Runner,
    Step, Workload, NAME, PROBE,
};
use toshokan::blocking::Library;
use toshokan::error::Why;
use toshokan::io::Capabilities;
use toshokan::report::Rekey;
use toshokan::simulator::Machine;
use toshokan::{Error, Io, MemDisk, Root};

/// A folder that can neither rename nor sync, whose moves copy, as a browser's
/// picked folder: a write lands whole when its stream closes, so none is short.
const PICKED: Capabilities = Capabilities {
    append: true,
    rename_file: false,
    no_replace: false,
    rename_dir: false,
    fsync: false,
};

/// A folder that syncs and cannot rename, whose moves copy, as FAT and exFAT.
const FAT: Capabilities = Capabilities {
    fsync: true,
    ..PICKED
};

/// The folders the sweep runs in, and whether a write there can be cut short.
const FOLDERS: [(&str, Capabilities, bool); 3] = [
    ("renaming", Capabilities::ALL, true),
    ("picked", PICKED, false),
    ("fat", FAT, true),
];

/// A folder and a local root in memory.
struct Mem {
    folder: MemDisk,
    local: RefCell<MemDisk>,
}

impl Mem {
    fn new(folder: Capabilities) -> Self {
        Self {
            folder: MemDisk::with_capabilities(folder, Capabilities::ALL),
            local: RefCell::new(MemDisk::new()),
        }
    }
}

impl Place for Mem {
    type Backend = Machine;

    fn backend(&self) -> Machine {
        Machine {
            folder: self.folder.clone(),
            local: self.local.borrow().clone(),
        }
    }

    fn tree(&self) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
        let files = self.folder.files(Root::Folder).into_iter();
        Ok(files
            .map(|(path, bytes)| (path.as_str().to_owned(), bytes))
            .collect())
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let path = power_loss::rel(path);
        let parent = path.parent().expect("not the root");
        let make = Io::MakeDir {
            root: Root::Folder,
            path: parent,
        };
        let create = Io::Create {
            root: Root::Folder,
            path,
            bytes: bytes.to_vec(),
        };
        for io in [make, create] {
            self.folder.perform(io).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn lose_local(&self) -> Result<(), String> {
        *self.local.borrow_mut() = MemDisk::new();
        Ok(())
    }
}

/// How the run goes on once a step fails.
#[derive(Clone, Copy, Debug)]
enum Then {
    /// The library writes again, which settles what the failure left, with that
    /// write's request of the index given faulted as well.
    Writes(Option<(u64, Fault)>),
    /// The library is dropped, as an app that quits.
    Quits,
}

/// How many of the mutating requests that settle what a failure left are faulted
/// in turn as the library writes again.
const SETTLE_FAULTS: usize = 4;

/// One faulted run.
struct Case {
    workload: &'static str,
    folder: &'static str,
    driver: &'static str,
    request: u64,
    io: String,
    fault: Fault,
    then: Then,
}

impl std::fmt::Display for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} in a {} folder, {}: request {} ({}) {:?}, then {:?}",
            self.workload, self.folder, self.driver, self.request, self.io, self.fault, self.then
        )
    }
}

/// Runs `workload`'s setup, then its steps with `fault` armed, and returns how
/// many steps returned, the entries they committed, and whether one failed.
fn steps<L: Lib<Machine>>(
    workload: &Workload,
    place: &Mem,
    fault: Option<(u64, Fault)>,
    then: Then,
) -> Result<Ran, String> {
    before(place)?;
    let models = models(workload);
    let mut runner = Runner::<Mem, L>::new(place);
    let setup = workload.setup.len();
    for (i, &step) in workload.setup.iter().enumerate() {
        runner.perform(step, &models[i])?;
    }
    {
        let mut arms = runner.arms.borrow_mut();
        (arms.fault, arms.counted, arms.seen) = (fault, 0, Some(Vec::new()));
    }
    let mut acked = Vec::new();
    let mut settling = false;
    let mut done = 0;
    let mut failed = None;
    for (n, &step) in workload.steps.iter().enumerate() {
        let i = setup + n;
        // An interrupted step stands for a process that stops, not one whose
        // requests fail; what it leaves is faulted in the steps after it.
        let interrupted = matches!(step, Step::Interrupt { .. });
        runner.arms.borrow_mut().paused = interrupted;
        let performed = runner.perform(step, &models[i]);
        runner.arms.borrow_mut().paused = false;
        match performed {
            Ok(entry) => {
                settling = match step {
                    Step::Interrupt { .. } => true,
                    Step::Settle => false,
                    _ => settling,
                };
                verify(&runner, &models[i + 1], settling)
                    .map_err(|e| format!("after {step:?}: {e}"))?;
                if let Some(entry) = entry {
                    acked.push(Acked::find(place, entry)?);
                }
                done = n + 1;
            }
            Err(error) if fault.is_some() => {
                failed = Some(error);
                break;
            }
            Err(error) => return Err(error),
        }
    }
    let seen = runner.arms.borrow_mut().seen.take().unwrap_or_default();
    let mut settling = Vec::new();
    if let (Some(mut library), Then::Writes(second)) = (runner.library.take(), then) {
        let finishing = !library.finishing().is_empty();
        {
            let mut arms = runner.arms.borrow_mut();
            (arms.fault, arms.counted, arms.seen) = (second, 0, Some(Vec::new()));
        }
        let mut probe = || {
            library.commit("Probe", |intent| {
                let (intent, _) = intent.create(|e| {
                    e.set(NAME, PROBE.into());
                });
                intent
            })
        };
        let mut wrote = probe();
        let probed = runner.arms.borrow_mut().seen.take().unwrap_or_default();
        runner.arms.borrow_mut().fault = None;
        if finishing && second.is_none() {
            settling = probed;
        }
        let faulted = second.is_some();
        // A write that finds a failed write of its own landed writes nothing and
        // stops its writer; so does a second fault, and the write after either goes
        // on with a healthy folder.
        for _ in 0..2 {
            let again = match &wrote {
                Err(Error::Rekey {
                    why: Rekey::Landed, ..
                }) => true,
                Err(Error::ReadOnly(_)) | Ok(_) => false,
                Err(_) => faulted,
            };
            if !again {
                break;
            }
            wrote = probe();
        }
        match wrote {
            Ok(_) => {}
            // Only a settle the second fault failed may leave the session read-only.
            Err(Error::ReadOnly(Why::Unfinished { .. })) if faulted => {}
            Err(e) => return Err(format!("the next write: {e}")),
        }
        library.close().map_err(|e| format!("closing: {e}"))?;
    }
    runner.arms.borrow_mut().fault = None;
    Ok(Ran {
        done,
        acked,
        seen,
        settling,
        failed,
    })
}

/// What a run did: how many steps returned, the entries they committed, the
/// requests its steps made, those of the write after a failure that settled what
/// it left, and the error of the step that failed.
struct Ran {
    done: usize,
    acked: Vec<Acked>,
    seen: Vec<power_loss::Seen>,
    settling: Vec<power_loss::Seen>,
    failed: Option<String>,
}

/// Faults each request of `workload`'s steps in turn with each fault that can
/// befall it, and returns the cases that fail, each with what failed.
fn sweep<L: Lib<Machine>>(
    workload: &Workload,
    (name, folder, shortens): (&'static str, Capabilities, bool),
    driver: &'static str,
) -> Vec<(Case, String)> {
    let seen = steps::<L>(workload, &Mem::new(folder), None, Then::Quits)
        .unwrap_or_else(|e| panic!("{} runs unfaulted: {e}", workload.name))
        .seen;
    let mut failed = Vec::new();
    for (request, io) in seen.iter().enumerate() {
        let faults = [
            Some(Fault::Fails),
            io.mutates.then_some(Fault::Unanswered),
            io.mutates.then_some(Fault::Unplugged),
            (shortens && io.shortens).then_some(Fault::Short),
        ];
        for fault in faults.into_iter().flatten() {
            let mut thens = vec![Then::Writes(None), Then::Quits];
            while let Some(then) = thens.pop() {
                let case = Case {
                    workload: workload.name,
                    folder: name,
                    driver,
                    request: request as u64,
                    io: io.io.clone(),
                    fault,
                    then,
                };
                let place = Mem::new(folder);
                let ran = steps::<L>(workload, &place, Some((case.request, fault)), then);
                let checked = ran.and_then(|ran| {
                    let settles = ran.settling.iter().enumerate();
                    let mutating = settles.filter(|(_, io)| io.mutates).take(SETTLE_FAULTS);
                    let second =
                        mutating.map(|(at, _)| Then::Writes(Some((at as u64, Fault::Fails))));
                    thens.extend(second);
                    let step = ran.failed.unwrap_or_default();
                    let checked = check(workload, &place, ran.done, &ran.acked);
                    checked
                        .map(|_| ())
                        .map_err(|e| format!("{e}\n    after the step failed: {step}"))
                });
                if let Err(error) = checked {
                    failed.push((case, error));
                }
            }
        }
    }
    failed
}

/// Sweeps every workload in `folder` through the driver `L` names.
fn sweep_in<L: Lib<Machine>>(folder: (&'static str, Capabilities, bool), driver: &'static str) {
    let failed: Vec<(Case, String)> = workloads()
        .iter()
        .flat_map(|workload| sweep::<L>(workload, folder, driver))
        .collect();
    let shown: Vec<String> = failed
        .iter()
        .map(|(case, error)| format!("{case}\n    {error}"))
        .collect();
    assert!(
        failed.is_empty(),
        "{} faulted runs failed:\n{}",
        failed.len(),
        shown.join("\n")
    );
}

type Blocking = Library<Faulty<Machine>>;

#[test]
fn every_request_faulted_in_a_renaming_folder_leaves_what_a_power_loss_may() {
    sweep_in::<Blocking>(FOLDERS[0], "blocking");
}

#[test]
fn every_request_faulted_in_a_renaming_folder_through_async_leaves_what_a_power_loss_may() {
    sweep_in::<Async<Machine>>(FOLDERS[0], "async");
}

#[test]
fn every_request_faulted_in_a_picked_folder_leaves_what_a_power_loss_may() {
    sweep_in::<Blocking>(FOLDERS[1], "blocking");
}

#[test]
fn every_request_faulted_in_a_picked_folder_through_async_leaves_what_a_power_loss_may() {
    sweep_in::<Async<Machine>>(FOLDERS[1], "async");
}

#[test]
fn every_request_faulted_in_a_fat_folder_leaves_what_a_power_loss_may() {
    sweep_in::<Blocking>(FOLDERS[2], "blocking");
}

#[test]
fn every_request_faulted_in_a_fat_folder_through_async_leaves_what_a_power_loss_may() {
    sweep_in::<Async<Machine>>(FOLDERS[2], "async");
}
