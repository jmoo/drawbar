//! Clavia / Nord device transport and vendor protocol over USB.
//!
//! Layered so the protocol is testable without hardware:
//!
//! - [`wire`] — message framing and codec. Pure, fully decoded, no I/O.
//! - [`transport`] — the byte pipe. The only part that touches a device.
//! - [`session`] — the transaction wrapper every operation runs inside.
//! - [`op`] — typed operations.
//!
//! The wire format was reverse-engineered from a corpus of NSM captures and is
//! verified against all 4,589 messages in it. See the `nord-corpus` repo and the
//! project notes for the capture methodology.
//!
//! # Portability
//!
//! Desktop (macOS/Linux/Windows) via `nusb`, browsers via WebUSB. WebUSB is the
//! binding constraint on the API shape — see [`transport::Transport`] for why there
//! are no `Send` bounds, and why enumeration is deliberately backend-specific rather
//! than part of the portable core.

pub mod error;
pub mod op;
pub mod session;
pub mod transport;
pub mod wire;

pub use error::{Error, Result};
pub use session::{ReadOnly, ReadWrite, Session};
pub use transport::Transport;
pub use wire::{Location, Message, Service};
