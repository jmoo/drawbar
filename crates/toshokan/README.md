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
let (import, song) = lib.intent("Import").create(|e| {
    e.save(&path, bytes, Expect::Absent)
        .set(ORIGIN, "B3 Split".into());
});
import.add(song, TAGS, "Sunday".into()).commit()?;
let view = lib.view();
let tags = view.entity(song).unwrap().members(TAGS);
lib.undo()?;
```

A view answers lookups from indexes it shares with the views before and after
it, each in a search plus a step per result: the entities whose register or set
holds a value (`find`), holds any value (`with`) or a value in a range
(`range`), each value with how many entities hold it (`values`), the entity
bound to a file (`at`), the entities whose files are under a folder (`under`),
and every conflict (`conflicts`). Values compare by the JSON toshokan writes for
them; `range` and `values` decode each distinct value of the key. Searching
names or derived metadata is the app's.

`refresh` reads what other writers logged since, and scans only the files their
entries moved. `rescan` also scans every library file, for what changed outside
the app; a rescan the app drops, to commit or to close, goes on where it stopped
at the next one. `rescan_paths` scans the files a watcher saw change.

`opened` reports what needs the user: effects another writer left unfinished,
drafts, forks, facts a restore of the folder removed, and files that arrived,
moved or changed outside the app. A view never changes, so an app can hand it to
other threads. A field read from a view is a value, a conflict between writers,
or unreadable, so an app cannot show half a conflict by accident.

A save takes bytes or a source the driver streams into staging a chunk at a
time, such as a `Splice` of the file being rewritten, so a file of hundreds of
megabytes is never held whole. A commit whose file effects stop partway fails
with `Error::Partial`, which says what it logged. Any other commit whose entries
reached the folder succeeds, and `Committed::local` says what the instance keeps
beside the folder that lags it until the next commit, refresh or close.

## In the browser

With the `web` feature, a library runs on the page through the async driver,
and a dedicated worker running the same wasm bundle performs its requests on the
origin private file system or on a folder the user picked:

```rust,ignore
use toshokan::asynch::Library;
use toshokan::web::{DateClock, CryptoRandom, Folder, Worker};

let local = RelPath::new("libraries/main/local")?;
let folder = Folder::picked(handle); // or Folder::Private(path)
let worker = Worker::start(folder, &local).await?;
let (mut lib, opened) = Library::open(worker, layout, &schema, env).await?;
```

The bundle must be built by wasm-bindgen with `--target web`. The worker loads it
from a small script toshokan makes as a `blob:` URL, so a Content Security
Policy must allow `blob:` workers. `web::Hints` says when to look again: a
commit in another tab (`refresh`), the paths Chromium's file system observer saw
change (`rescan_paths`), and focus or a period for a folder other programs write
(`rescan`). [SPEC.md](SPEC.md#browsers) lists what each browser can do.

The browser suites run headless in Chromium and Firefox with
`nix build .#toshokan-web` on Linux. Elsewhere, from `crates/` in the
development shell, with a WebDriver and its browser installed:

```sh
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
GECKODRIVER=geckodriver \
  cargo test -p toshokan --features web --target wasm32-unknown-unknown \
  --test backends --test effects --test web
```

## Design

The core does no I/O. Every operation is a state machine that asks for reads,
writes and syncs, and consumes their results. The `blocking` driver runs it on
the machine's file system, and the `asynch` driver on any async backend, such as
a browser's; both run it on an in-memory disk that models what survives a crash.
A directory listing with every file's metadata is one request, and so are many
reads, so opening a library costs a request per directory rather than per file.
Time, randomness, the app's identity function and the volume's name rules are
injected, so the crash harness and the sync simulator replay every run exactly.

Each writer appends to a hash-chained log in its own directory. Entries name each
other by hash, so writers never collide, and a reader places entries by chain
whatever their files are called. Fields are multi-value registers and
observed-remove sets, merged by a join. A file effect stages its bytes, writes a
pending record, moves displaced bytes into the writer's trash, and logs what it
did; recovery after a crash, or after losing the app's data, finishes or reports
it. Where the folder cannot rename files, each move copies and then removes its
source, under the same record.

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
   for a rename that never replaces, and wasm-bindgen, js-sys and web-sys for
   the browser.
