//! Clavia / Nord device transport and vendor protocol over USB.
//!
//! > This is an unofficial community project. It is **not affiliated with, endorsed
//! > by, or supported by Clavia DMI AB**. "Nord" and the instrument names are
//! > Clavia's trademarks, used here only to identify the instruments this crate
//! > talks to.
//!
//! The layers let the protocol be tested without hardware:
//!
//! - [`wire`]: message framing and codec. Pure, fully decoded, no I/O.
//! - [`transport`]: the byte pipe. The only part that touches a device.
//! - [`session`]: the transaction wrapper every operation runs inside.
//! - [`op`]: typed operations.
//! - [`device`]: an instrument as a value, with its session brackets and geometry.
//!
//! The wire format was reverse-engineered from USB captures of Nord Sound Manager and
//! is verified against every message in them.
//!
//! # Portability
//!
//! Desktop (macOS/Linux/Windows) uses `nusb`; browsers use WebUSB. WebUSB sets the
//! shape of the API: see [`transport::Transport`] for why there are no `Send` bounds.
//! Device discovery is left to each backend.

#[cfg(feature = "nusb")]
pub mod deadline;
pub mod device;
pub mod envelope;
pub mod error;
pub mod op;
pub mod session;
pub mod sleep;
pub mod transport;
pub mod wire;

pub use device::{Device, Geometry};
pub use error::{Error, Result};
pub use session::{ReadOnly, ReadWrite, Session};
pub use transport::Transport;
pub use wire::{Location, Message, ObjectClass, Service, Status};

#[cfg(feature = "replay")]
pub use transport::ReplayTransport;

/// Block on a future without pulling in a full async runtime.
///
/// The crate does not depend on an async runtime (see [`transport::Transport`] for why
/// there are no `Send` bounds). This is for CLIs and tests that only want the result.
/// Browser callers already have an executor and should not use it.
#[cfg(feature = "blocking")]
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    pollster::block_on(fut)
}
