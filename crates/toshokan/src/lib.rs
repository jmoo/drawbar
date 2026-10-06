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

// The hidden modules are reached by the integration tests, not by apps.
pub mod asynch;
#[doc(hidden)]
pub mod binding;
pub mod blocking;
#[doc(hidden)]
pub mod compaction;
#[doc(hidden)]
pub mod crash;
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
#[doc(hidden)]
pub mod writer;

pub use disk::MemDisk;
pub use env::{Clock, Env, ExactNames, Identify, Names, Random};
pub use error::{Error, Invalid, Mismatch, Refusal, Result, Why};
pub use ids::{EntityId, EntryHash, Hlc, Identity, Nonce, SegmentName, WriterId};
pub use io::{Io, IoError, IoResult, Operation, Reply, Root, Step, Task};
pub use layout::Layout;
pub use path::RelPath;
pub use plan::{Expect, Plan};
pub use report::{Committed, Opened, Outcome, Partial};
pub use schema::{Field, Members, Raw, Register, Schema, Set, Value};
pub use trash::Policy;
pub use view::{EntityView, FileRef, FileState, View};
