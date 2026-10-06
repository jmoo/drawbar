# toshokan

**Keep what a file can't say about itself beside the file, for every writer.**
toshokan (図書館, "library") keeps tags, provenance and relations between files
in a hidden directory inside a folder of real files. Every running instance of
an app writes its own log there, offline and without coordinating, and every
reader merges the logs to the same state. Nothing in the user's files is
overwritten in place, so file changes can be undone.

> ⚠️ Proof of concept, being rewritten. The on-disk format is unstable, and the
> crate is not published.

## Use

An app declares its keys, opens a library as one writer, reads views and
changes the library by intents:

```rust,ignore
use toshokan::blocking::{Library, Native};
use toshokan::{Expect, Layout, Register, Schema, Set};

const ORIGIN: Register<String> = Register::new("origin");
const TAGS: Set<String> = Set::new("tags");

let schema = Schema::of(&[ORIGIN.key(), TAGS.key()])?;
let backend = Native::new("/path/to/library", "/path/to/app/data");
let (mut lib, opened) = Library::open(backend, Layout::new(".app")?, &schema, env)?;
lib.intent("Import")
    .create(|e| {
        e.set(ORIGIN, "B3 Split".into()).add(TAGS, "Sunday".into());
    })
    .commit()?;
```

The core does no I/O: each operation asks for reads and writes and consumes
their results. The `blocking` driver runs it on the machine's file system, and
the `asynch` driver on any async backend, such as a browser's.

[SPEC.md](SPEC.md) specifies every file toshokan writes. [spec/](spec/README.md)
model-checks, in TLA+, the protocol between writers and readers.

## Principles

1. **The folder belongs to the user.** Files keep their names, bytes and places
   unless the app asks. Opening writes nothing there.
2. **One writer per file.** Each writer writes only in its own directory.
3. **Merge is a pure function.** The same entries give the same state, in any
   order, on any machine.
4. **Nothing is overwritten.** Displaced bytes go to the writer's trash, which
   only that writer empties.
5. **Conflicts are shown.** Concurrent edits are kept, and the app can list them.
6. **Keep what you don't understand.** What a newer writer wrote survives.
7. **Nondeterminism is injected.** Time, randomness and file identity come from
   the app, so tests replay exactly.
8. **Small.** The standard library, serde, BLAKE3 and thiserror, plus rustix on
   Linux and Apple systems for a rename that never replaces.
