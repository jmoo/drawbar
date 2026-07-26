//! Typed operations.
//!
//! Batching is a property of the operation, not of a generic helper: most ops repeat
//! their per-item unit N times, but `duplicate` is **phase-separated** — all N
//! addressing exchanges first, then all N data-copy pairs. Encoding that per-op beats
//! a generic loop that then needs an escape hatch.
//!
//! Writes trigger a device-side refresh-read, so a write is not finished when its
//! write exchange returns. The operation owns that, not the caller.

use crate::error::Result;
use crate::session::Session;
use crate::transport::Transport;
use crate::wire::Location;

pub trait Operation {
    type Output;
    #[allow(async_fn_in_trait)] // see transport::Transport
    async fn run<T: Transport, C>(&self, session: &mut Session<'_, T, C>) -> Result<Self::Output>;
}

/// Read a program. Verified shape: `O26 I30, O34 I159` inside the wrapper, where the
/// 159-byte response is a reframed entity — 36-byte header, the same 121-byte body the
/// `.ne5p` carries, 2-byte CRC.
#[derive(Debug)]
pub struct Read {
    pub at: Location,
}

/// Move a program. Verified shape: `O34 I38`.
#[derive(Debug)]
pub struct Move {
    pub from: Location,
    pub to: Location,
}

/// Delete programs. Verified shape: `O36 O26 I30`, repeated per item.
#[derive(Debug)]
pub struct Delete {
    pub items: Vec<Location>,
}

/// Rename a program. Verified shape: `O33 I30`; the request carries a
/// length-prefixed name, so its size tracks the string.
#[derive(Debug)]
pub struct Rename {
    pub at: Location,
    pub name: String,
}

/// Duplicate programs. The one confirmed **phase-separated** operation.
#[derive(Debug)]
pub struct Duplicate {
    pub pairs: Vec<(Location, Location)>,
}
