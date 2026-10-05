//! Keep what a file cannot say about itself beside the file, safely, for every writer.
//!
//! toshokan keeps tags, provenance, relations between files, unsaved edits and history
//! in a hidden directory beside a folder of the user's files. Any number of writers
//! change it offline without coordinating: each appends to its own log, and the logs
//! merge to the same [`State`] in any order. Every change can be undone.
//!
//! An app opens a [`Library`] as one writer and changes it by intents: entities and
//! their fields and sets, and saves, renames, moves and deletions of their files, each
//! of which undo reverses.
//!
//! The crate knows nothing about any file format. It runs over the [`Fs`] trait:
//! [`MemFs`] for tests, with injected crashes, and `native::NativeFs` off the web.
//!
//! **Status:** proof of concept. The on-disk format is unstable.

pub mod blobs;
pub mod compact;
pub mod effects;
pub mod error;
pub mod fs;
pub mod ids;
pub mod journal;
pub mod layout;
pub mod library;
pub mod log;
pub mod merge;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
pub mod scan;
pub mod undo;
pub mod value;

pub use effects::Precondition;
pub use error::{Error, Mismatch, Result};
pub use fs::{Capabilities, Fingerprint, Fs, MemFs, RelPath};
pub use ids::{EntityId, IntentId, Version, WriterId};
pub use layout::Layout;
pub use library::{Change, Library, TornSegment};
pub use log::{Entry, Kind};
pub use merge::State;
pub use value::{BlobId, Value};

/// The field binding an entity to its file: the file's [`RelPath`] as text.
pub const PATH_FIELD: &str = "path";

/// The field naming the bytes toshokan last wrote or bound for an entity's file, as a
/// blob.
pub const CONTENT_FIELD: &str = "content";
