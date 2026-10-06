# toshokan

**Keep what a file can't say about itself beside the file, for every writer.**

toshokan (図書館, "library") keeps tags, provenance and relations between files
in a hidden directory inside a folder of real files. Every running instance of
an app writes its own log there, offline and without coordinating, and every
reader merges the logs to the same state, whatever order a sync client delivers
them in. Files are saved, renamed and deleted without overwriting anything in
place, so file changes can be undone.

> ⚠️ Proof of concept. The format is unstable, and the crate is not published.

## Use

An app declares its keys, opens a library as one writer, renders views and
changes the library by intents:

```rust,ignore
use std::rc::Rc;
use toshokan::blocking::{Library, Native};
use toshokan::env::{ExactNames, OsRandom, PrefixIdentity, SystemClock};
use toshokan::{Env, Expect, Layout, Register, Schema, Set};

const ORIGIN: Register<String> = Register::new("origin");
const TAGS: Set<String> = Set::new("tags");

let schema = Schema::of(&[ORIGIN.key(), TAGS.key()])?;
let env = Env {
    clock: Box::new(SystemClock),
    random: Box::new(OsRandom::new()),
    identify: Rc::new(PrefixIdentity::default()),
    names: Box::new(ExactNames),
    label: "drawbar".into(),
};
let backend = Native::new("/path/to/library", "/path/to/app/data");
let (mut lib, opened) = Library::open(backend, Layout::new(".app")?, &schema, env)?;
let mut import = lib.intent("Import");
let song = import.create(|e| {
    e.save(&path, bytes, Expect::Absent)
        .set(ORIGIN, "B3 Split".into())
        .add(TAGS, "Sunday".into());
});
import.commit()?;
let view = lib.view();
let tags = view.entity(song).unwrap().members(TAGS);
lib.undo()?;
```

`opened` reports what needs the user: effects another writer left unfinished,
drafts, forks, facts a restore of the folder removed, and files that arrived,
moved or changed outside the app. A view never changes, so an app can hand it to
other threads. A field read from a view is a value, a conflict between writers,
or unreadable, so an app cannot show half a conflict by accident.

A save takes bytes or a source the driver streams into staging a chunk at a
time, such as a `Splice` of the file being rewritten, so a file of hundreds of
megabytes is never held whole. A commit whose file effects stop partway fails
with `Error::Partial`, which says what it logged.

## Design

The core does no I/O. Every operation is a state machine that asks for reads,
writes and syncs, and consumes their results. The `blocking` driver runs it on
the machine's file system, and the `asynch` driver on any async backend, such as
a browser's; both run it on an in-memory disk that models what survives a crash.
Time, randomness, the app's identity function and the volume's name rules are
injected, so the crash harness and the sync simulator replay every run exactly.

Each writer appends to a hash-chained log in its own directory. Entries name each
other by hash, so writers never collide, and a reader places entries by chain
whatever their files are called. Fields are multi-value registers and
observed-remove sets, merged by a join. A file effect stages its bytes, writes a
pending record, moves displaced bytes into the writer's trash, and logs what it
did; recovery after a crash, or after losing the app's data, finishes or reports
it.

[SPEC.md](SPEC.md) specifies every file toshokan writes and how it is merged.
[spec/](spec/README.md) model-checks, in TLA+, the protocol between writers and
readers, and maps its properties to the tests that check the code.

## Principles

1. **The folder belongs to the user.** Files keep their names, bytes and places
   unless the app asks. Opening writes nothing there.
2. **One writer per file.** Each writer writes only in its own directory.
3. **Merge is a pure function.** The same entries give the same state, in any
   order, on any machine.
4. **Nothing is overwritten.** Displaced bytes go to the writer's trash, which
   only that writer empties.
5. **Conflicts are shown.** Concurrent edits are kept, and the app can list them.
6. **Keep what you don't understand.** What a newer writer wrote survives, even
   when an older one compacts it.
7. **Nondeterminism is injected.** Time, randomness and file identity come from
   the app, so tests replay exactly.
8. **Small.** The standard library, serde, BLAKE3 and thiserror, plus rustix on
   Linux and Apple systems, libc on Apple systems and windows-sys on Windows,
   for a rename that never replaces.
