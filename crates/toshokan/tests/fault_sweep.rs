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
    /// The library writes again, which settles what the failure left.
    Writes,
    /// The library is dropped, as an app that quits.
    Quits,
}

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
    runner.arms.borrow_mut().fault = None;
    if let (Some(mut library), Then::Writes) = (runner.library.take(), then) {
        let mut probe = || {
            library.commit("Probe", |intent| {
                let (intent, _) = intent.create(|e| {
                    e.set(NAME, PROBE.into());
                });
                intent
            })
        };
        // A write that finds a failed write of its own landed, or a change it
        // cannot finish, writes nothing; the first takes a new writer for the next.
        let wrote = match probe() {
            Err(Error::Rekey { .. }) => probe(),
            wrote => wrote,
        };
        match wrote {
            Ok(_) | Err(Error::ReadOnly(Why::Unfinished { .. })) => {}
            Err(e) => return Err(format!("the next write: {e}")),
        }
        library.close().map_err(|e| format!("closing: {e}"))?;
    }
    Ok(Ran {
        done,
        acked,
        seen,
        failed,
    })
}

/// What a run did: how many steps returned, the entries they committed, the
/// requests it made, and the error of the step that failed.
struct Ran {
    done: usize,
    acked: Vec<Acked>,
    seen: Vec<power_loss::Seen>,
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
            for then in [Then::Writes, Then::Quits] {
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

/// Sweeps every workload in `folder` through both drivers.
fn sweep_in(folder: (&'static str, Capabilities, bool)) {
    let mut failed = Vec::new();
    for workload in workloads() {
        failed.extend(sweep::<Library<Faulty<Machine>>>(
            &workload, folder, "blocking",
        ));
        failed.extend(sweep::<Async<Machine>>(&workload, folder, "async"));
    }
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

#[test]
fn every_request_faulted_in_a_renaming_folder_leaves_what_a_power_loss_may() {
    sweep_in(FOLDERS[0]);
}

#[test]
fn every_request_faulted_in_a_picked_folder_leaves_what_a_power_loss_may() {
    sweep_in(FOLDERS[1]);
}

#[test]
fn every_request_faulted_in_a_fat_folder_leaves_what_a_power_loss_may() {
    sweep_in(FOLDERS[2]);
}
