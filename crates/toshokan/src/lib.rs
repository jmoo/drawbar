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

pub mod asynch;
pub mod binding;
pub mod blocking;
pub mod compaction;
pub mod crash;
pub mod disk;
pub mod drafts;
pub mod effects;
pub mod env;
pub mod error;
pub mod ids;
pub mod intent;
pub mod io;
pub mod layout;
pub mod library;
pub mod line;
pub mod log;
pub mod merge;
pub mod path;
pub mod pending;
pub mod plan;
pub mod reader;
pub mod recovery;
pub mod report;
pub mod schema;
pub mod simulator;
pub mod snapshot;
pub mod trash;
pub mod undo;
pub mod view;
pub mod writer;

pub use disk::MemDisk;
pub use env::{Clock, Env, Identify, Random};
pub use error::{Error, Invalid, Mismatch, Refusal, Result, Why};
pub use ids::{EntityId, EntryHash, Hlc, Identity, Nonce, SegmentName, WriterId};
pub use io::{Io, IoError, IoResult, Operation, Reply, Root, Step, Task};
pub use layout::Layout;
pub use path::RelPath;
pub use plan::{Expect, Plan};
pub use report::{Committed, Opened, Outcome};
pub use schema::{Field, Members, Raw, Register, Schema, Set, Value};
pub use trash::Policy;
pub use view::{EntityView, FileRef, FileState, View};
