//! What a command is answered with where it could not run, or where its answers were
//! lost on their way back, so nothing waits on it.

use super::{Cmd, Event, Failure, Intent, Key, LibPath, Slot};

/// How to answer `cmd` where it could not run, or its answers could not be read: each
/// change to a file says it did not land, or that whether it did is unheard, so nothing
/// waits on it.
pub(super) fn refused(cmd: &Cmd) -> Refusing {
    let refusing = match cmd {
        Cmd::Open => Refusing::Open,
        Cmd::Rescan | Cmd::Refresh => Refusing::Scan,
        Cmd::Intent(intent) => Refusing::Intent(intent.label().to_string(), parts(intent)),
        Cmd::Undo | Cmd::Redo | Cmd::Settle { .. } => Refusing::Intent(
            match cmd {
                Cmd::Undo => "Undo",
                Cmd::Redo => "Redo",
                _ => "Settle",
            }
            .to_string(),
            Vec::new(),
        ),
        Cmd::Draft { of, .. } => Refusing::Draft(of.clone()),
        Cmd::Discard(_) | Cmd::KeepSlot { .. } | Cmd::DiscardSlot(_) => Refusing::Kept,
        Cmd::ReadSlot(slot) => Refusing::Slot(slot.clone()),
        Cmd::Read { files, .. } => Refusing::Read(files.iter().map(|(id, ..)| *id).collect()),
    };
    refusing
}

/// What a command that could not run is answered with.
pub(super) enum Refusing {
    Open,
    Scan,
    /// The intent's label, and the answer each change to a file in it owes.
    Intent(String, Vec<Owing>),
    Draft(Key),
    Kept,
    Slot(Slot),
    Read(Vec<u64>),
}

/// The answer one change to a file owes.
pub(super) enum Owing {
    Saved(u64, LibPath),
    Rewritten(u64, LibPath),
    Imported(u64, LibPath),
    Moved(LibPath, LibPath),
}

/// What each change to a file `intent` makes owes an answer.
fn parts(intent: &Intent) -> Vec<Owing> {
    match intent {
        Intent::Save { id, path, .. } => vec![Owing::Saved(*id, path.clone())],
        Intent::Rewrite { id, path, .. } => vec![Owing::Rewritten(*id, path.clone())],
        Intent::Import { id, path, .. } => vec![Owing::Imported(*id, path.clone())],
        Intent::Move { moves } => moves
            .iter()
            .map(|moving| Owing::Moved(moving.from.clone(), moving.to.clone()))
            .collect(),
        Intent::MoveFolder { from, to } => vec![Owing::Moved(from.clone(), to.clone())],
        Intent::Batch(parts) => parts.iter().flat_map(self::parts).collect(),
        _ => Vec::new(),
    }
}

impl Refusing {
    /// The answers for `why`, where the worker did not run the command when `heard`,
    /// and otherwise ran it and its answers were lost.
    pub(super) fn answers(self, why: &str, heard: bool) -> Vec<Event> {
        let failed = || match heard {
            true => Failure::Io(why.to_string()),
            false => Failure::Unheard,
        };
        match self {
            Refusing::Open => vec![Event::Opened(Err(why.to_string()))],
            Refusing::Scan => vec![Event::Scanned(Err(why.to_string()))],
            Refusing::Draft(of) => vec![Event::Drafted {
                of,
                kept: Err(why.to_string()),
            }],
            Refusing::Kept => vec![Event::Unkept(why.to_string())],
            Refusing::Slot(slot) => vec![Event::SlotKept {
                slot,
                kept: Err(why.to_string()),
            }],
            Refusing::Read(ids) => vec![Event::Read(
                ids.into_iter().map(|id| (id, Err(failed()))).collect(),
            )],
            Refusing::Intent(label, owing) => {
                let owed = owing.into_iter().map(|owing| match owing {
                    Owing::Saved(id, path) => Event::Saved {
                        id,
                        path,
                        result: Err(failed()),
                    },
                    Owing::Rewritten(id, path) => Event::Rewritten {
                        id,
                        path,
                        result: Err(failed()),
                    },
                    Owing::Imported(id, path) => Event::Imported {
                        id,
                        path,
                        result: Err(failed()),
                    },
                    Owing::Moved(from, to) => Event::Moved {
                        from,
                        to,
                        result: Err(failed()),
                    },
                });
                let committed = match heard {
                    true => Event::Committed {
                        label,
                        made: None,
                        result: Err(why.to_string()),
                    },
                    false => Event::Unheard { label },
                };
                owed.chain(std::iter::once(committed)).collect()
            }
        }
    }
}
