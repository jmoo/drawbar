//! Keep what a file cannot say about itself beside the file, for every writer.
//!
//! toshokan keeps facts about a folder of the user's files (tags, provenance,
//! relations) in a hidden directory inside it. Every running instance of an app is
//! a writer with its own hash-chained log there; writers never coordinate, and
//! every reader merges the logs to the same state whatever order a sync client
//! delivered them in. Concurrent edits are kept and shown as conflicts. Files are
//! saved, renamed and deleted without overwriting anything in place: displaced
//! bytes go to the writer's own trash, which also makes file undo durable.
//!
//! The core is sans-IO: every operation is a state machine that emits [`Io`]
//! requests and consumes their results ([`Operation`]). [`blocking`] runs it on the
//! machine's file system or on a [`MemDisk`]; [`asynch`] runs it on any async
//! [`asynch::Fs`]. Time, randomness and the app's identity function come in
//! through [`Env`], so tests replay exactly.
//!
//! **Status:** proof of concept. The on-disk format is unstable.
//!
//! # Using a library
//!
//! A view answers lookups from the merged state: the entities whose register or
//! set holds a value (`find`) or any value (`with`), and each value with how many
//! entities hold it (`values`), each by a visit of every entity shown; and the
//! entity bound to a file (`at`), by a search. Values compare by the JSON toshokan
//! writes for them; `values` decodes each distinct value of the key. Searching
//! names or derived metadata is the app's. `changed_since` gives the entities and
//! files that differ from an earlier view, so an app can redraw only those.
//!
//! Opening scans no library file: each file is shown where its facts put it, as
//! unscanned, until a scan of every file. `rescan` scans every library file, for
//! what changed outside the app; a rescan the app drops, to commit or to close,
//! goes on where it stopped at the next one. An app rescans once a library opens.
//! `refresh` reads what other writers logged since, and scans only the files their
//! entries moved, or every file until one rescan has finished. `rescan_paths` scans
//! the files a watcher saw change. Each reports what changed and, once every file
//! was scanned, the files that arrived, moved or changed outside the app.
//!
//! `Layout::new(".app")` keeps the library's own files in `.app/`.
//! `Layout::within(".app", "library")` keeps them in `.app/library/` and leaves all
//! of `.app/` out of scans, for an app with other files of its own there, and
//! `hiding()` also leaves out every name that starts with a dot.
//!
//! An intent can save, rename and trash a file nothing is said about by its path,
//! without making it an entity, and `Committed::paths` says where such files went,
//! an undo's included. A draft of an unsaved edit is kept for an entity or for such
//! a path.
//!
//! `opened` reports what needs the user: effects another writer left unfinished,
//! drafts and forks. When an instance's history forks, each branch is shown as an
//! item of the view (`items`) with the change it would make. One intent can take
//! changes from several items (`Intent::take`); an item stays, with no changes once
//! nothing is left to review, until an intent dismisses it (`Intent::dismiss`). The
//! instance writes as a new writer once it sees the fork, which opening, a refresh
//! (`retired`) or a refused compaction reports. A reader keeps nothing between
//! opens: what it shows is what the files in the folder say, so facts a restore of
//! the folder took go at the next read. Undo reaches the intents of the session but
//! those that only dismiss, and `Committed::undoable` says which a commit is. A
//! view never changes, so an app can hand it to other threads. A field read from a
//! view is a value, a conflict between writers, or unreadable, so an app cannot
//! show half a conflict by accident.
//!
//! A save takes bytes or a source the driver streams into staging a chunk at a
//! time, such as a `Splice` of the file being rewritten, so a file of hundreds of
//! megabytes is never held whole. A commit whose file effects stop partway fails
//! with `Error::Partial`, which says what it logged. Any other commit whose entries
//! reached the folder succeeds, and `Committed::local` says what the instance keeps
//! beside the folder that lags it until the next commit, refresh or close. A commit
//! leaves the pending record of its file effects in the folder; an app calls `tidy`
//! when idle to remove such records, and closing removes what is left.
//!
//! # In the browser
//!
//! With the `web` feature, a library runs on the page through the async driver,
//! and a dedicated worker running the same wasm bundle performs its requests on the
//! origin private file system or on a folder the user picked:
//!
//! ```rust,ignore
//! use toshokan::asynch::Library;
//! use toshokan::web::{DateClock, CryptoRandom, Folder, Worker};
//!
//! let local = RelPath::new("libraries/main/local")?;
//! let folder = Folder::picked(handle); // or Folder::Private(path)
//! let worker = Worker::start(folder, &local).await?;
//! let (mut lib, opened) = Library::open(worker, layout, &schema, env).await?;
//! ```
//!
//! Each stage of an operation runs in one piece on the thread that drives it:
//! parsing the logs, deriving the merged state, and gathering and binding the
//! library's files. Each takes longer the larger the library, so a
//! library on a page that must stay responsive belongs in a dedicated worker.
//!
//! The bundle must be built by wasm-bindgen with `--target web`. The worker loads it
//! from a small script toshokan makes as a `blob:` URL, so a Content Security
//! Policy must allow `blob:` workers. `web::Hints` says when to look again: a
//! commit in another tab (`refresh`), the paths Chromium's file system observer saw
//! change (`rescan_paths`), and focus or a period for a folder other programs write
//! (`rescan`). The Browsers section of `SPEC.md` lists what each browser can do.
//!
//! The browser suites run headless with `nix build .#toshokan-web` on Linux.
//! Elsewhere, from `crates/` in the development shell, with a WebDriver and its
//! browser installed:
//!
//! ```sh
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//! GECKODRIVER=geckodriver \
//!   cargo test -p toshokan --features web --target wasm32-unknown-unknown \
//!   --test backends --test effects --test web
//! ```

// The hidden modules are reached by the integration tests, not by apps.
pub mod asynch;
#[doc(hidden)]
pub mod binding;
pub mod blocking;
#[doc(hidden)]
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
pub mod drafts;
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
mod pack;
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
pub use drafts::Drafted;
pub use env::{Clock, Env, ExactNames, Identify, Names, Random};
pub use error::{Error, Invalid, Mismatch, Refusal, Result, Unbindable, Why};
pub use ids::{EntityId, EntryHash, Hlc, Identity, Nonce, SegmentName, WriterId};
pub use io::{Io, IoError, IoResult, Operation, Reply, Root, Step, Task};
pub use layout::Layout;
pub use path::RelPath;
pub use plan::{Expect, Plan};
pub use report::{Committed, Lag, Local, Opened, Outcome, Partial};
pub use schema::{Field, Keyed, Members, Raw, Register, Schema, Set, Value};
pub use trash::Policy;
pub use view::{Changed, Counts, EntityView, FileRef, FileState, Listed, View};
