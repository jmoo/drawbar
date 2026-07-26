//! The transaction wrapper every operation runs inside.
//!
//! Each operation is enclosed by the same exchange sequence, independent of what the
//! operation actually does:
//!
//! ```text
//! O18 I22, O22 I26, [ operation ], O22 I42, O18 I22, O18 I22
//! ```
//!
//! (Payload bytes. Captures quote frame lengths, which are 40 higher — that is the
//! sniffer's Darwin header, not anything on the wire.)
//!
//! # Why `commit()` and not `Drop`
//!
//! This is an RAII shape, and closing in `Drop` is still wrong: `Drop` can be neither
//! async nor fallible, so a failed close would be silently swallowed — unacceptable
//! where a half-open transaction may leave the device in an odd state. Closing is
//! explicit; `Drop` only *complains* in debug builds.
//!
//! `open_on_device` omits the closing `O22 I42` entirely, so the wrapper is a property
//! each operation declares rather than something applied unconditionally.

use crate::error::Result;
use crate::transport::Transport;

/// Read-only capability. Cannot reach any operation that mutates the device.
#[derive(Debug)]
pub struct ReadOnly;

/// Read-write capability, reachable only through an explicit escalation. Device
/// writes can destroy patches, so "did I gate this?" is a compile error, not a
/// review question.
#[derive(Debug)]
pub struct ReadWrite;

pub struct Session<'t, T: Transport, C = ReadOnly> {
    // `Option` rather than a plain `&mut` so the capability escalation can move the
    // borrow out. A type implementing `Drop` cannot be destructured, so `take()` is
    // the way to hand the transport onward without upsetting the borrow checker.
    #[allow(dead_code)]
    transport: Option<&'t mut T>,
    closed: bool,
    _capability: std::marker::PhantomData<C>,
}

impl<'t, T: Transport> Session<'t, T, ReadOnly> {
    /// Run the opening exchanges and hand back a read-only session.
    pub async fn open(transport: &'t mut T) -> Result<Self> {
        // TODO: emit SESSION_OPEN once ops land; the codes are in wire::cmd.
        Ok(Self {
            transport: Some(transport),
            closed: false,
            _capability: std::marker::PhantomData,
        })
    }

    /// Escalate to a session that can mutate the device.
    ///
    /// Deliberately verbose and deliberately not `From`/`Into`: per the architecture's
    /// safety-first rule, callers should back up before writing.
    pub fn allow_destructive_writes(mut self) -> Session<'t, T, ReadWrite> {
        let transport = self.transport.take();
        let closed = self.closed;
        // The husk is about to drop; it no longer owns the transaction, so silence
        // its debug assertion. The returned session inherits the real state.
        self.closed = true;
        Session { transport, closed, _capability: std::marker::PhantomData }
    }
}

impl<T: Transport, C> Session<'_, T, C> {
    /// Run the closing exchanges. Always prefer this over dropping.
    pub async fn commit(mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }

    /// Abandon the transaction without committing.
    pub async fn abort(mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

impl<T: Transport, C> Drop for Session<'_, T, C> {
    fn drop(&mut self) {
        debug_assert!(
            self.closed,
            "Session dropped without commit()/abort() — the device may be left \
             mid-transaction. Close it explicitly."
        );
    }
}
