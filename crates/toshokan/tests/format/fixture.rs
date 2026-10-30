//! The committed fixture trees: where they are, how a test loads one onto a disk,
//! and the app the generator and the tests share.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use toshokan::blocking::{self, Backend};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::io::Capabilities;
use toshokan::report::Opened;
use toshokan::simulator::Machine;
use toshokan::{
    Env, Identify, Identity, Io, IoError, IoResult, Layout, MemDisk, Register, RelPath, Root,
    Schema, Set,
};

pub const ORIGIN: Register<String> = Register::new("origin");
pub const NOTE: Register<Option<String>> = Register::new("note");
pub const TAGS: Set<String> = Set::new("tags");

pub fn schema() -> Schema {
    Schema::of(&[ORIGIN.key(), NOTE.key(), TAGS.key()]).unwrap()
}

pub fn layout() -> Layout {
    Layout::new(".t").unwrap()
}

pub const IDENTIFY: PrefixIdentity = PrefixIdentity { prefix: 64 * 1024 };

pub fn identity(bytes: &[u8]) -> Identity {
    IDENTIFY.of_bytes(&RelPath::ROOT, bytes)
}

pub fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

pub fn env(label: &str, seed: u64, clock: &TestClock) -> Env {
    Env {
        clock: Box::new(clock.clone()),
        random: Box::new(SeededRandom::new(seed)),
        identify: Rc::new(IDENTIFY),
        names: Box::new(ExactNames),
        label: label.into(),
    }
}

/// A folder whose moves copy, as a browser's picked folder without `move()`.
pub const COPYING: Capabilities = Capabilities {
    append: true,
    rename_file: false,
    no_replace: false,
    rename_dir: false,
    fsync: false,
};

/// Every state, each written by an earlier build.
pub const STATES: [&str; 5] = [
    "one-writer-open",
    "one-writer-compacted",
    "two-writers-fork",
    "pending",
    "local",
];

/// Beside a state's trees: the directories of each that hold nothing.
pub const EMPTY_DIRS: &str = "empty-dirs.json";

/// The directory holding every fixture state.
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// What a request does in place of reaching the disk.
pub enum Fault {
    /// Fails without effect.
    Fail,
    /// Creates the file empty, then fails: a picked folder names a new file before
    /// its stream closes, and a tab stopped between leaves it empty.
    Empty,
}

type Picks = dyn Fn(&Io) -> Option<Fault>;

/// A machine whose requests a test may fault.
#[derive(Clone)]
pub struct Faulty {
    pub machine: Machine,
    faults: Rc<RefCell<Option<Box<Picks>>>>,
}

impl Faulty {
    pub fn new(machine: &Machine) -> Self {
        Self {
            machine: Machine {
                folder: machine.folder.process(),
                local: machine.local.process(),
            },
            faults: Rc::default(),
        }
    }

    pub fn fault(&self, picks: impl Fn(&Io) -> Option<Fault> + 'static) {
        *self.faults.borrow_mut() = Some(Box::new(picks));
    }

    pub fn heal(&self) {
        *self.faults.borrow_mut() = None;
    }
}

impl Backend for Faulty {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.machine, root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let fault = self.faults.borrow().as_ref().and_then(|picks| picks(&io));
        match (fault, io) {
            (None, io) => self.machine.perform(io),
            (Some(Fault::Fail), _) => Err(IoError::Other("faulted".into())),
            (Some(Fault::Empty), Io::Create { root, path, .. }) => {
                self.machine.perform(Io::Create {
                    root,
                    path,
                    bytes: Vec::new(),
                })?;
                Err(IoError::Crashed)
            }
            (Some(Fault::Empty), io) => panic!("only a create lands empty: {io:?}"),
        }
    }
}

pub type Library = blocking::Library<Faulty>;

pub fn open(backend: &Faulty, label: &str, seed: u64, clock: &TestClock) -> (Library, Opened) {
    blocking::Library::open(
        backend.clone(),
        layout(),
        &schema(),
        env(label, seed, clock),
    )
    .unwrap()
}

/// Creates `files` in `root` of `disk`, durably, with the directories they need.
pub fn put(disk: &MemDisk, root: Root, files: &BTreeMap<RelPath, Vec<u8>>) {
    for (file, bytes) in files {
        let parent = file.parent().unwrap();
        let ok = |io: Io| {
            let shown = format!("{io:?}");
            disk.perform(io).unwrap_or_else(|e| panic!("{shown}: {e}"));
        };
        ok(Io::MakeDir {
            root,
            path: parent.clone(),
        });
        ok(Io::Create {
            root,
            path: file.clone(),
            bytes: bytes.clone(),
        });
        ok(Io::Sync {
            root,
            path: file.clone(),
        });
        let mut dir = Some(parent);
        while let Some(synced) = dir {
            ok(Io::Sync {
                root,
                path: synced.clone(),
            });
            dir = synced.parent();
        }
    }
}

/// Every file under `dir` on the machine's disk, by its path below `dir`.
pub fn read_tree(dir: &Path) -> BTreeMap<RelPath, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(at) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let full = entry.path();
            if entry.file_type().unwrap().is_dir() {
                dirs.push(full);
                continue;
            }
            let below = full.strip_prefix(dir).unwrap();
            let text = below.to_str().unwrap().replace('\\', "/");
            found.insert(path(&text), std::fs::read(&full).unwrap());
        }
    }
    found
}

/// A machine whose folder and local root hold the trees of state `state`. A state
/// without a `local/` tree loads with an empty local root, as a fresh install.
pub fn load(state: &str) -> Machine {
    load_with(state, Capabilities::ALL)
}

pub fn load_with(state: &str, folder: Capabilities) -> Machine {
    let dir = fixtures().join(state);
    assert!(dir.is_dir(), "no fixture state {state}");
    let machine = Machine {
        folder: MemDisk::with_capabilities(folder, Capabilities::ALL),
        local: MemDisk::new(),
    };
    put(
        &machine.folder,
        Root::Folder,
        &read_tree(&dir.join("folder")),
    );
    put(&machine.local, Root::Local, &read_tree(&dir.join("local")));
    let empty: BTreeMap<String, Vec<String>> =
        serde_json::from_slice(&std::fs::read(dir.join(EMPTY_DIRS)).unwrap()).unwrap();
    for (tree, dirs) in empty {
        let (disk, root) = match tree.as_str() {
            "folder" => (&machine.folder, Root::Folder),
            _ => (&machine.local, Root::Local),
        };
        for dir in dirs {
            disk.perform(Io::MakeDir {
                root,
                path: path(&dir),
            })
            .unwrap();
        }
    }
    machine
}

/// [`load`] with the local root left empty.
pub fn load_folder(state: &str) -> Machine {
    let machine = load(state);
    machine.local.lose_local();
    machine
}
