//! The crash harness: a scenario crashed at every operation under every fault.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use crate::disk::{MemDisk, Renames, Tail};

/// One crash to inject.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fault {
    /// Operations that complete before the crash.
    pub after: u64,
    pub tail: Tail,
    pub renames: Renames,
}

/// Runs `scenario` on a disk from `setup` once to count its operations, then once
/// per crash point under each tail and rename fault, and hands each fault and the
/// restarted disk to `check`. `scenario` must stop at the first
/// [`crate::IoError::Crashed`].
pub fn sweep(
    setup: impl Fn() -> MemDisk,
    scenario: impl Fn(&MemDisk),
    check: impl FnMut(Fault, MemDisk),
) {
    todo!()
}
