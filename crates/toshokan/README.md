# toshokan

**Keep what a file can't say about itself beside the file, for every writer.**

toshokan (図書館, "library") keeps tags, provenance and relations between files
in a hidden directory inside a folder of real files. Every running instance of
an app writes its own log there, offline and without coordinating, and every
reader merges the logs to the same state, whatever order a sync client delivers
them in. Each writer keeps only what it still contributes, so a library's size
follows its facts, not its history. Files are saved, renamed and deleted without
overwriting anything in place, so file changes can be undone.

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
names or derived metadata is the app's. `changed_since` gives the entities and
files that differ from an earlier view, at a cost that follows the changes, so an
app can redraw only those.

Opening scans no library file: each file is shown where its facts put it, as
unscanned, until a scan of every file. `rescan` scans every library file, for
what changed outside the app; a rescan the app drops, to commit or to close,
goes on where it stopped at the next one. An app rescans once a library opens.
`refresh` reads what other writers logged since, and scans only the files their
entries moved, or every file until one rescan has finished. `rescan_paths` scans
the files a watcher saw change. Each reports what changed and, once every file
was scanned, the files that arrived, moved or changed outside the app.

`Layout::new(".app")` keeps the library's own files in `.app/`.
`Layout::within(".app", "library")` keeps them in `.app/library/` and leaves all
of `.app/` out of scans, for an app with other files of its own there, and
`hiding()` also leaves out every name that starts with a dot.

An intent can save, rename and trash a file nothing is said about by its path,
without making it an entity, and `Committed::paths` says where such files went,
an undo's included. A draft of an unsaved edit is kept for an entity or for such
a path.

`opened` reports what needs the user: effects another writer left unfinished,
drafts and forks. When an instance's history forks, each branch is shown as an
item of the view (`items`) with the change it would make. One intent can take
changes from several items (`Intent::take`); an item stays, with no changes once
nothing is left to review, until an intent dismisses it (`Intent::dismiss`). The
instance writes as a new writer once it sees the fork, which opening, a refresh
(`retired`) or a refused compaction reports. Facts a restore of the folder took stay shown for a
day and a full rescan, then go. Undo reaches the intents of the session. A view
never changes, so an app can hand it to other threads. A field read from a view
is a value, a conflict between writers, or unreadable, so an app cannot show
half a conflict by accident.

A save takes bytes or a source the driver streams into staging a chunk at a
time, such as a `Splice` of the file being rewritten, so a file of hundreds of
megabytes is never held whole. A commit whose file effects stop partway fails
with `Error::Partial`, which says what it logged. Any other commit whose entries
reached the folder succeeds, and `Committed::local` says what the instance keeps
beside the folder that lags it until the next commit, refresh or close. A commit
leaves the pending record of its file effects in the folder; an app calls `tidy`
when idle to remove such records, and closing removes what is left.

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

Opening, rescanning and committing to a large library do their long work a
slice at a time: reading the logs and the cached view, folding, indexing, and
gathering and binding the library's files. The page's driver gives the thread
back to the browser every few milliseconds, so the page keeps drawing and
answering input.

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
writes and syncs, consumes their results, and pauses between slices of a long
computation. The `blocking` driver runs it on
the machine's file system, and the `asynch` driver on any async backend, such as
a browser's; both run it on an in-memory disk that models what survives a crash.
A directory listing with every file's metadata is one request, and so are many
reads, so a scan of every file costs a request per directory rather than per
file. A commit or a refresh binds files to entities again only as far as its
changes reach: the entities and files that share a path, a name the volume
takes for one, or an identity with what changed.
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

## Power loss

The crash harness checks toshokan against the file system it models. Two NixOS
VM tests check the native backend against ext4, XFS, Btrfs and exFAT, on x86_64
Linux with KVM:

```sh
nix build .#toshokan-power-loss-smoke  # crashes a VM mid-workload, three times
nix build .#toshokan-power-loss        # replays every flush, on each file system
nix build .#toshokan-power-loss.ext4   # on one
```

The replay logs each workload's writes with dm-log-writes, then rebuilds the
disk as it stood at every flush and at sets of the writes between two flushes,
and checks each state after the file system's own recovery. An exFAT state that
`fsck.exfat` finds corrupt is checked but counted apart. A write is replayed
whole, never torn; the crash harness covers torn writes.
[examples/power_loss.rs](examples/power_loss.rs) holds the workloads and the
checker. Neither test covers APFS, NTFS or a browser's storage.

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
