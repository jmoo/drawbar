//! Keep tags and edit history beside the files they describe, in a folder that syncs.
//!
//! toshokan keeps facts about a folder of the user's files (tags, provenance,
//! relations) in a hidden directory inside it, so the folder can be synced by
//! Dropbox, OneDrive, iCloud or a network share. Sync goes wrong in ordinary ways
//! even for someone who runs one app at a time: another computer has stale state,
//! files arrive out of order, a backup restores part of the folder, or a
//! conflicted copy appears. toshokan is built so that:
//!
//! - normal syncing needs no intervention;
//! - once a sync settles, every reader shows the same state;
//! - an interrupted or conflicting change leaves every file recoverable;
//! - when recovery is needed, the app can say what happened without the user
//!   knowing how the logs work.
//!
//! Several writers can work in one folder at once without error. An app that
//! wants live collaboration is better served by a CRDT library such as Automerge.
//! The on-disk format may change before 1.0.
//!
//! # Design
//!
//! - **The folder belongs to the user.** Files keep their names, bytes and places
//!   unless the app asks. Opening writes nothing there.
//! - **One writer per file.** Every running instance of an app is a writer that
//!   appends only to a hash-chained log in its own directory, so a sync client
//!   never has two versions of one file to reconcile.
//! - **Merge is a pure function.** Every reader merges the logs to the same
//!   state, whatever order they arrived in, on any machine.
//! - **Conflicts are shown.** Concurrent edits by several writers are kept, and a
//!   view lists them.
//! - **Nothing is overwritten.** Files are saved, renamed and deleted without
//!   overwriting anything in place. Displaced bytes go to the writer's own trash,
//!   which only that writer empties, so file undo is durable.
//! - **Keep what you don't understand.** What a newer writer wrote survives, even
//!   when an older one compacts it.
//! - **The core does no I/O.** Every operation is a state machine that emits
//!   [`Io`] requests and consumes their results ([`Operation`]). [`blocking`] runs
//!   it on the machine's file system and [`asynch`] on any async [`asynch::Fs`],
//!   such as a browser's. Both run it on a [`MemDisk`], which models what survives
//!   a crash. A directory listing with every file's metadata is one request, and
//!   so are many reads, so a scan costs a request per directory.
//! - **Nondeterminism is injected.** Time, randomness, the app's identity function
//!   and the volume's name rules come in through [`Env`], so tests replay exactly.
//! - **Small.** toshokan depends on serde, serde_json, BLAKE3 and thiserror; on
//!   rustix, libc or windows-sys for a rename that never replaces; and on
//!   wasm-bindgen, wasm-bindgen-futures, js-sys and web-sys in the browser.
//!
//! [format.md](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/format.md)
//! specifies every file toshokan writes, and
//! [protocol.md](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md)
//! how writers and readers use them.
//!
//! # Using a library
//!
//! An app declares its keys in a [`Schema`], opens a library as one writer with
//! [`blocking::Library::open`] or [`asynch::Library::open`], renders [`View`]s, and
//! changes the library by [intents](intent), each one commit.
//! [`Layout::new(".app")`](Layout::new) keeps the library's own files in `.app/`.
//! [`Layout::within(".app", "library")`](Layout::within) keeps them in
//! `.app/library/` and leaves all of `.app/` out of scans, for an app with other
//! files of its own there, and [`Layout::hiding`] also leaves out every name that
//! starts with a dot.
//!
//! ## Views
//!
//! A view never changes, so an app can hand it to other threads. A field read from
//! a view is a value, a conflict between writers, or unreadable, so an app cannot
//! show half a conflict by accident.
//!
//! A view answers lookups from the merged state: the entities whose register or
//! set holds a value ([`View::find`]) or any value ([`View::with`]), and each value
//! with how many entities hold it ([`View::values`]), each by a visit of every
//! entity shown; and the entity bound to a file ([`View::at`]), by a search. Values
//! compare by the JSON toshokan writes for them. Searching names or derived
//! metadata is the app's. [`View::changed_since`] gives the entities and files that
//! differ from an earlier view, so an app can redraw only those.
//!
//! ## Scans
//!
//! Opening scans no library file: each file is shown where its facts put it, as
//! unscanned, until a scan of every file. An app rescans once a library opens.
//! `rescan` scans every library file, for what changed outside the app; a rescan
//! the app drops, to commit or to close, goes on where it stopped at the next one.
//! `refresh` reads what other writers logged since, and scans only the files their
//! entries moved, or every file until one rescan has finished. `rescan_paths` scans
//! the files a watcher saw change. Each reports what changed and, once every file
//! was scanned, the files that arrived, moved or changed outside the app.
//!
//! Two writers can each make an entity of one file before either sees the other's.
//! The file is then the lowest entity's, and [`View::joined`] lists each other one,
//! shown without a file. An app commits [`Intent::join`](intent::Intent::join) to
//! give the lowest what each holds and delete the rest.
//!
//! ## Commits and undo
//!
//! An intent can also save, rename and trash a file nothing is said about, by its
//! path, without making it an entity, and [`Committed::paths`] says where such
//! files went, an undo's included. A save takes bytes or a source the driver
//! streams into staging a chunk at a time, such as a [`Splice`](plan::Splice) of
//! the file being rewritten, so a file of hundreds of megabytes is never held
//! whole.
//!
//! A commit whose file effects fail logs nothing and fails. The next write, or the
//! next open, finishes them first, and where it cannot, the library is read-only
//! until it is opened again. Where what they move is gone, the writer gives them up
//! as effects left unfinished, for the app to ask the user about, and goes on as a
//! new writer. Any other commit whose entries reached the folder succeeds, and
//! [`Committed::local`] says what the instance keeps beside the folder that lags
//! it until the next commit, refresh or close.
//!
//! Undo reaches the intents of this session, and ends at close. An intent that
//! only dismisses runs, only joins entities or settles another writer's record is
//! no step of it, and [`Committed::undoable`] says which a commit is. An undo is
//! refused with [`Refusal::ChangedSince`] where another writer changed the same
//! thing since.
//!
//! ## What needs the user
//!
//! [`Opened::orphaned`] lists effects another writer left unfinished, to settle
//! with the user's consent. When an instance's history forks, [`View::forks`]
//! lists the fork and each branch is an item of the view ([`View::items`]) with
//! the change it would make. One intent can take changes from several items
//! ([`Intent::take`](intent::Intent::take)); an item stays, with no changes once
//! nothing is left to review, until an intent dismisses it
//! ([`Intent::dismiss`](intent::Intent::dismiss)). The instance writes as a new
//! writer once it sees the fork, which opening ([`Opened::start`]), a refresh
//! ([`Refreshed::retired`](report::Refreshed::retired)) or a refused compaction
//! reports.
//!
//! A reader keeps nothing between opens: what it shows is what the files in the
//! folder say, so facts a restore of the folder took go at the next read.
//!
//! # In the browser
//!
//! With the `web` feature, a library runs through the async driver in a dedicated
//! worker the app starts, on an `Executor` that performs its requests on the origin
//! private file system or on a folder the user picked:
//!
//! ```rust,ignore
//! use toshokan::asynch::Library;
//! use toshokan::web::{Executor, Folder};
//!
//! let local = RelPath::new("libraries/main/local")?;
//! let folder = Folder::picked(handle); // or Folder::Private(path)
//! let executor = Executor::new(folder, &local).await?;
//! let (mut lib, opened) = Library::open(executor, layout, &schema, env).await?;
//! ```
//!
//! `web::Hints` says when to look again: a commit in another tab (`refresh`), the
//! paths Chromium's file system observer saw change (`rescan_paths`), and focus,
//! being shown, or a period while shown, for a folder other programs write
//! (`rescan`).
//! [Browsers](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#browsers)
//! lists what each browser can do.

// The hidden modules are reached by the integration tests, not by apps.
pub mod asynch;
#[doc(hidden)]
pub mod binding;
pub mod blocking;
#[doc(hidden)]
pub mod compaction;
#[doc(hidden)]
pub mod contribution;
#[doc(hidden)]
pub mod crash;
#[doc(hidden)]
pub mod derive;
pub mod disk;
#[doc(hidden)]
pub mod effects;
pub mod env;
pub mod error;
mod flow;
pub mod ids;
pub mod intent;
pub mod io;
pub mod layout;
#[doc(hidden)]
pub mod library;
#[doc(hidden)]
pub mod line;
pub mod log;
#[doc(hidden)]
pub mod merge;
pub mod path;
#[doc(hidden)]
pub mod pending;
pub mod plan;
#[doc(hidden)]
pub mod reader;
#[doc(hidden)]
pub mod recovery;
pub mod report;
pub mod schema;
#[doc(hidden)]
pub mod simulator;
#[doc(hidden)]
pub mod snapshot;
pub mod trash;
#[doc(hidden)]
pub mod undo;
pub mod view;
#[cfg(feature = "web")]
pub mod web;
#[doc(hidden)]
pub mod writer;

pub use disk::MemDisk;
pub use env::{Clock, Env, ExactNames, Identify, Names, Random};
pub use error::{Error, Invalid, Mismatch, Refusal, Result, Unbindable, Why};
pub use ids::{EntityId, EntryHash, Hlc, Identity, Nonce, SegmentName, WriterId};
pub use io::{Io, IoError, IoResult, Operation, Reply, Root, Step, Task};
pub use layout::Layout;
pub use path::RelPath;
pub use plan::{Expect, Plan};
pub use report::{Committed, Lag, Local, Opened, Outcome};
pub use schema::{Field, Keyed, Members, Raw, Register, Schema, Set, Value};
pub use view::{Changed, Counts, EntityView, FileRef, FileState, Listed, View};
