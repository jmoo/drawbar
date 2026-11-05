# toshokan

**Keep what a file cannot say about itself beside the file, in a folder that syncs.**

toshokan (図書館, "library") keeps tags, provenance and relations between files in
a hidden directory inside a folder of real files, so the folder can live in
Dropbox, OneDrive, iCloud or on a network share.

- A stale computer, files that arrive out of order, a partial restore or a
  conflicted copy need no intervention.
- Once a sync settles, every reader shows the same state.
- Nothing is overwritten in place, so an interrupted or conflicting change leaves
  every file recoverable.
- Concurrent edits are kept and shown as conflicts.
- Files keep their names, bytes and places unless the app asks, and opening a
  library writes nothing in the folder.
- It runs natively and in the browser, on the origin private file system or a
  folder the user picked.

It is not for live collaboration, which a CRDT library such as Automerge serves.
The on-disk format may change before 1.0.

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
let (import, song) = lib.intent("Import").create(|e| {
    e.save(&path, bytes, Expect::Absent)
        .set(ORIGIN, "B3 Split".into());
});
import.add(song, TAGS, "Sunday".into()).commit()?;
let view = lib.view();
let tags = view.entity(song).unwrap().members(TAGS);
lib.undo()?;
```

The [crate documentation](https://docs.rs/toshokan) describes the API.
[spec/format.md](spec/format.md) specifies every file toshokan writes,
[spec/protocol.md](spec/protocol.md) how writers and readers use them, and
[spec/](spec/README.md) model-checks the protocol.
