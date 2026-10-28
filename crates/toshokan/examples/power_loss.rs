//! Drives the native backend through short scripted workloads and checks what a
//! power loss leaves of one. The NixOS tests in `power-loss/` run it on real file
//! systems, crash them and replay their writes.
//!
//! ```text
//! power_loss list                  # the workloads, and how many steps each takes
//! power_loss run WORKLOAD DIR MARK # MARK is dm:<device> or file:<path>
//! power_loss check WORKLOAD DIR N [ENTRY…]
//! ```
//!
//! `run` performs the workload's setup in `DIR` and marks `setup`, then marks
//! `step-<i>` once step `i` returns, as a dm-log-writes mark or a synced line,
//! and prints each mark. A step that commits an entry marks
//! `step-<i>:<writer>/<n>/<hash>`.
//! `check` opens what a crash left in `DIR` as the same install, once `N` steps
//! returned, committing the `ENTRY`s their marks name, with the workload's
//! hand-written model as the oracle of what each step changed.

// Empty on wasm, which has no native backend.
#![cfg_attr(target_arch = "wasm32", no_main)]
#![cfg(not(target_arch = "wasm32"))]

#[path = "../tests/power_loss/mod.rs"]
mod power_loss;

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use power_loss::{check, run, workload, workloads, Acked, Faulty, Place};
use toshokan::blocking::{Library, Native};
use toshokan::report::Start;

/// A folder and a local root in the machine's file system.
struct Dirs {
    folder: PathBuf,
    local: PathBuf,
}

impl Dirs {
    fn new(dir: &Path) -> Result<Self, String> {
        let dirs = Self {
            folder: dir.join("folder"),
            local: dir.join("local"),
        };
        make_dirs(&dirs.local)?;
        Ok(dirs)
    }
}

impl Place for Dirs {
    type Backend = Native;

    fn backend(&self) -> Native {
        Native::new(&self.folder, &self.local)
    }

    fn tree(&self) -> Result<BTreeMap<String, Vec<u8>>, String> {
        tree(&self.folder)
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let full = self.folder.join(path);
        make_dirs(full.parent().unwrap())?;
        let mut file = File::create_new(&full).map_err(|e| format!("{path}: {e}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("{path}: {e}"))?;
        sync_dir(full.parent().unwrap())
    }

    fn lose_local(&self) -> Result<(), String> {
        fs::remove_dir_all(&self.local).map_err(|e| format!("{}: {e}", self.local.display()))?;
        make_dirs(&self.local)
    }
}

enum Marks {
    Device(String),
    File(File),
}

impl Marks {
    fn parse(spec: &str) -> Result<Self, String> {
        if let Some(device) = spec.strip_prefix("dm:") {
            return Ok(Self::Device(device.into()));
        }
        let Some(path) = spec.strip_prefix("file:") else {
            return Err(format!("{spec} is neither dm:<device> nor file:<path>"));
        };
        let file = OpenOptions::new().create(true).append(true).open(path);
        file.map(Self::File).map_err(|e| format!("{path}: {e}"))
    }

    fn mark(&mut self, label: &str) -> Result<(), String> {
        match self {
            Self::Device(device) => {
                let status = Command::new("dmsetup")
                    .args(["message", device, "0", "mark", label])
                    .status()
                    .map_err(|e| format!("dmsetup: {e}"))?;
                match status.success() {
                    true => Ok(()),
                    false => Err(format!("dmsetup could not mark {label}: {status}")),
                }
            }
            Self::File(file) => writeln!(file, "{label}")
                .and_then(|()| file.sync_all())
                .map_err(|e| format!("marking {label}: {e}")),
        }
    }
}

fn sync_dir(dir: &Path) -> Result<(), String> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("syncing {}: {e}", dir.display()))
}

/// Creates `dir` and every directory above it that is missing, durably.
fn make_dirs(dir: &Path) -> Result<(), String> {
    if dir.is_dir() {
        return Ok(());
    }
    let parent = dir.parent().expect("not the root");
    make_dirs(parent)?;
    fs::create_dir(dir).map_err(|e| format!("making {}: {e}", dir.display()))?;
    sync_dir(parent)
}

/// Every file under `root`, by path relative to it with `/` between names.
fn tree(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().into_string().expect("UTF-8 names");
            let path = format!("{prefix}{name}");
            match entry.file_type()?.is_dir() {
                true => walk(&entry.path(), &format!("{path}/"), out)?,
                false => drop(out.insert(path, fs::read(entry.path())?)),
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out).map_err(|e| format!("reading {}: {e}", root.display()))?;
    Ok(out)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let done = match args[..] {
        ["list"] => {
            for workload in workloads() {
                println!("{} {}", workload.name, workload.steps.len());
            }
            Ok(())
        }
        ["run", name, dir, marks] => workload(name).and_then(|workload| {
            let mut marks = Marks::parse(marks)?;
            let dirs = Dirs::new(Path::new(dir))?;
            run::<_, Library<Faulty<Native>>>(&workload, &dirs, |label| {
                marks.mark(label)?;
                // replay.py counts the labels printed as the steps that returned.
                if label != "setup" {
                    println!("{label}");
                }
                Ok(())
            })
        }),
        ["check", name, dir, acked, ref entries @ ..] => workload(name).and_then(|workload| {
            let acked: usize = acked.parse().map_err(|e| format!("{acked}: {e}"))?;
            let entries = entries
                .iter()
                .map(|entry| entry.parse())
                .collect::<Result<Vec<Acked>, String>>()?;
            let checked = check(&workload, &Dirs::new(Path::new(dir))?, acked, &entries)?;
            let start = match checked.start {
                Start::Resumed(_) => "resumed",
                Start::New => "new",
                Start::Rekeyed { .. } => "rekeyed",
            };
            println!(
                "ok {name} acked={acked} model={} start={start} settled={} orphans={} unreadable={}",
                checked.model, checked.settled, checked.orphans, checked.unreadable
            );
            Ok(())
        }),
        _ => Err(
            "usage: power_loss list | run WORKLOAD DIR MARK | check WORKLOAD DIR N [ENTRY…]".into(),
        ),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("power_loss: {error}");
            ExitCode::FAILURE
        }
    }
}
