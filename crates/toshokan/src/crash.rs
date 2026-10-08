//! The crash harness: a scenario crashed at every operation under every fault.

use crate::blocking::Backend;
use crate::disk::{MemDisk, Renames, Tail};
use crate::io::{Capabilities, Io, IoError, IoResult, Reply, Root};
use crate::path::RelPath;

/// One crash to inject.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fault {
    /// Operations that complete before the crash.
    pub after: u64,
    pub tail: Tail,
    pub renames: Renames,
    /// New names are durable as soon as they are made.
    pub eager_names: bool,
}

/// What a crash leaves of unsynced appends: nothing, a byte, all of them, or
/// zeros in their place.
pub const TAILS: [Tail; 5] = [
    Tail::Lost,
    Tail::Torn(1),
    Tail::Torn(u64::MAX),
    Tail::Zeroed(1),
    Tail::Zeroed(u64::MAX),
];

/// The rename faults and early names `disk` can show: none of either in roots
/// that neither rename nor sync.
pub fn faults(disk: &MemDisk) -> Vec<(Renames, bool)> {
    let [folder, local] = [Root::Folder, Root::Local].map(|root| disk.capabilities(root));
    let renames: &[Renames] = match folder.rename_file || local.rename_file {
        true => &[Renames::Atomic, Renames::CopyThenRemove],
        false => &[Renames::Atomic],
    };
    let eager: &[bool] = match folder.fsync || local.fsync {
        true => &[false, true],
        false => &[false],
    };
    renames
        .iter()
        .flat_map(|&renames| eager.iter().map(move |&eager| (renames, eager)))
        .collect()
}

/// Runs `scenario` on a disk from `setup` once per fault it can show to count
/// its operations, then once per crash point under each fault, and hands each
/// fault and the restarted disk to `check`. `scenario` must stop at the first
/// [`crate::IoError::Crashed`]. The last crash point is after every operation.
pub fn sweep(
    setup: impl Fn() -> MemDisk,
    scenario: impl Fn(&MemDisk),
    mut check: impl FnMut(Fault, MemDisk),
) {
    for (renames, eager_names) in faults(&setup()) {
        let faulty = |tail| {
            let disk = setup();
            disk.set_tail(tail);
            disk.set_renames(renames);
            disk.set_eager_names(eager_names);
            disk
        };
        let counted = faulty(Tail::Lost);
        let start = counted.mutations();
        scenario(&counted);
        assert!(!counted.crashed(), "the scenario crashes with no fault");
        let operations = counted.mutations() - start;
        for tail in TAILS {
            for after in 0..=operations {
                let disk = faulty(tail);
                disk.crash_after(after);
                scenario(&disk);
                let fault = Fault {
                    after,
                    tail,
                    renames,
                    eager_names,
                };
                check(fault, disk.restart());
            }
        }
    }
}

/// A [`MemDisk`] whose folder lands writes as a browser's picked folder does:
/// consecutive writes to one file are held in its writable stream until a
/// request `Io::keeps_open` does not allow, and a crash before then loses them.
/// A file created with bytes gets its name first and its bytes when a stream
/// closes, so a crash between leaves it empty.
pub struct Streams {
    disk: MemDisk,
    filling: Vec<Io>,
}

impl Streams {
    pub fn new(disk: MemDisk) -> Self {
        Self {
            disk,
            filling: Vec::new(),
        }
    }

    pub fn disk(&self) -> &MemDisk {
        &self.disk
    }

    fn land(&mut self) -> Result<(), IoError> {
        for write in std::mem::take(&mut self.filling) {
            let path = write.path().clone();
            self.disk
                .perform(write)
                .map_err(|error| IoError::undelivered(&path, &error))?;
        }
        Ok(())
    }

    /// Makes the name, then writes and closes a stream, removing the name when
    /// the bytes do not land.
    fn create(&mut self, path: RelPath, bytes: Vec<u8>) -> IoResult {
        let root = Root::Folder;
        self.disk.perform(Io::Create {
            root,
            path: path.clone(),
            bytes: Vec::new(),
        })?;
        let write = Io::Write {
            root,
            path: path.clone(),
            offset: 0,
            bytes,
        };
        self.disk.perform(write).inspect_err(|_| {
            let _ = self.disk.perform(Io::Remove { root, path });
        })
    }
}

impl Backend for Streams {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.disk.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let open = self
            .filling
            .first()
            .is_some_and(|filled| io.keeps_open(filled.root(), filled.path()));
        if !open {
            self.land()?;
        }
        match io {
            Io::Write {
                root: Root::Folder, ..
            } => {
                self.filling.push(io);
                Ok(Reply::Done)
            }
            Io::Create {
                root: Root::Folder,
                path,
                bytes,
            } if !bytes.is_empty() => self.create(path, bytes),
            io => self.disk.perform(io),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crash_after_a_picked_folders_create_names_a_file_leaves_it_empty() {
        let disk = MemDisk::with_capabilities(
            Capabilities {
                rename_file: false,
                fsync: false,
                ..Capabilities::ALL
            },
            Capabilities::ALL,
        );
        let path = RelPath::new("f").unwrap();
        disk.crash_after(1);
        let created = Streams::new(disk.clone()).perform(Io::Create {
            root: Root::Folder,
            path: path.clone(),
            bytes: b"bytes".to_vec(),
        });
        assert!(matches!(created, Err(IoError::Crashed)), "{created:?}");
        let files = disk.restart().files(Root::Folder);
        assert_eq!(files.get(&path), Some(&Vec::new()));
    }
}
