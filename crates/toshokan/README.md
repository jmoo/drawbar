# toshokan

**Keep what a file can't say about itself beside the file, for every writer.**

toshokan (図書館, "library") keeps tags, provenance and relations between files
in a hidden directory inside a folder of real files. Every running instance of
an app writes its own log there, offline and without coordinating, and every
reader merges the logs to the same state, whatever order a sync client delivers
them in. Files are saved, renamed and deleted without overwriting anything in
place, so file changes can be undone.

> ⚠️ Proof of concept. The format is unstable, and the crate is not published.

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

The crate documentation describes the API, the browser driver and how to run
the browser suites. [SPEC.md](SPEC.md) specifies every file toshokan writes and
how it is merged, [RETENTION.md](RETENTION.md) how little history it keeps, and
[spec/](spec/README.md) model-checks the protocol and maps it to the tests,
the power-loss tests included. `scripts/bench.py` runs `tests/bench.rs` in a
browser profile and reports its memory.
