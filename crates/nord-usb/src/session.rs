//! The transaction wrapper every operation runs inside.
//!
//! Each operation is enclosed by the same exchange sequence, independent of what the
//! operation does:
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

use std::marker::PhantomData;

use crate::error::{Error, Result};
use crate::transport::Transport;
use crate::wire::{cmd, ui, Message, ObjectClass, Service};

/// Read-only capability. Cannot reach any operation that mutates the device.
#[derive(Debug)]
pub struct ReadOnly;

/// Read-write capability, reachable only through an explicit escalation.
#[derive(Debug)]
pub struct ReadWrite;

pub struct Session<'t, T: Transport, C = ReadOnly> {
    // `Option` rather than a plain `&mut` so the capability escalation can move the
    // borrow out: a type implementing `Drop` cannot be destructured.
    transport: Option<&'t mut T>,
    class: ObjectClass,
    closed: bool,
    _capability: PhantomData<C>,
}

impl<'t, T: Transport> Session<'t, T, ReadOnly> {
    /// Open a transaction scoped to one [`ObjectClass`].
    ///
    /// The class matters: `STATUS` and the addressing operations all report on
    /// whichever class was opened, so opening the wrong one yields correct-looking
    /// numbers about the wrong thing.
    pub async fn open(transport: &'t mut T, class: ObjectClass) -> Result<Self> {
        let mut s = Self {
            transport: Some(transport),
            class,
            closed: false,
            _capability: PhantomData,
        };

        // The UI/session-service handshake, then the class-scoped open, one fallible
        // step at a time — each failure needs to know whether the `HELLO` reached the
        // device, because from that moment on it holds a UI session that only `GOODBYE`
        // releases.
        //
        // ⚠️ Left half-open the device does not hang or refuse: it keeps answering, and
        // reports device status 0x1 ("empty") for every slot in every object class. That
        // survives reopening the session and clears only on a power cycle. Confirmed on
        // hardware.
        //
        // Errors are caught rather than propagated with `?` so the half-built session
        // can be marked closed first — the Drop assertion is there to catch *forgotten*
        // commits, not failed connections. And marked closed *before* the best-effort
        // GOODBYE: that send is the transaction's one release attempt, and the caller is
        // owed the original error, so a failure there is deliberately dropped.
        let hello = Message::new(Service::Ui, ui::SUBSYSTEM, ui::HELLO, Vec::new());
        if let Err(e) = s.notify(&hello).await {
            s.closed = true; // the write itself failed: the device never saw the HELLO
            return Err(e);
        }
        if let Err(e) = s.response_to(ui::HELLO).await {
            // The write landed, so the device may already be holding the UI session
            // even though its reply was unusable.
            s.closed = true;
            let _ = s
                .request(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, &[])
                .await;
            return Err(e);
        }

        let opened = s
            .request(
                Service::Program,
                10,
                cmd::SESSION_OPEN,
                &class.to_raw().to_be_bytes(),
            )
            .await;

        match opened {
            Ok(_) => Ok(s),
            Err(e) => {
                // The HELLO landed, so the UI session is open and must be released.
                s.closed = true;
                let _ = s
                    .request(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, &[])
                    .await;
                Err(e)
            }
        }
    }

    /// Escalate to a session that can mutate the device.
    ///
    /// Deliberately verbose and deliberately not `From`/`Into`: device writes can
    /// destroy patches, so callers should back up first.
    pub fn allow_destructive_writes(mut self) -> Session<'t, T, ReadWrite> {
        let transport = self.transport.take();
        let (class, closed) = (self.class, self.closed);
        // The husk is about to drop and no longer owns the transaction.
        self.closed = true;
        Session {
            transport,
            class,
            closed,
            _capability: PhantomData,
        }
    }
}

impl<T: Transport, C> Session<'_, T, C> {
    pub fn class(&self) -> ObjectClass {
        self.class
    }

    /// Send one request and read its response, enforcing the framing invariants: the
    /// reply must be `command + 1`, and must report success.
    pub(crate) async fn request(
        &mut self,
        service: Service,
        subsystem: u32,
        command: u32,
        args: &[u8],
    ) -> Result<Message> {
        let req = Message::new(service, subsystem, command, args.to_vec());
        self.notify(&req).await?;
        self.response_to(command).await
    }

    /// Read the reply to `command`, enforcing the framing invariants: it must carry
    /// `command + 1` and must report success.
    async fn response_to(&mut self, command: u32) -> Result<Message> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| Error::Transport("session has no transport".into()))?;

        let raw = transport.read(crate::transport::READ_BUFFER).await?;
        let resp = Message::decode_response(&raw)?;

        if resp.command != command + 1 {
            return Err(Error::UnexpectedResponse {
                expected: command + 1,
                got: resp.command,
            });
        }
        match resp.status() {
            Some(0) | None => Ok(resp),
            Some(code) => Err(Error::DeviceStatus(code)),
        }
    }

    /// Send a fire-and-forget message without waiting for a reply.
    ///
    /// The UI progress strings ([`ui::label`], [`ui::percent`]) are sent this way: the
    /// device never acknowledges them, so routing them through [`Self::request`] would
    /// block forever on a response that never comes.
    ///
    /// Like [`Self::request`] this does not test `closed`, and does not need to:
    /// [`Self::commit`] and [`Self::abort`] both consume `self`, so a closed session
    /// cannot be reached again. The flag exists only for the `Drop` assertion.
    pub(crate) async fn notify(&mut self, msg: &Message) -> Result<()> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| Error::Transport("session has no transport".into()))?;
        transport.write(&msg.encode()).await
    }

    /// Run the closing exchanges. Always prefer this over dropping.
    pub async fn commit(mut self) -> Result<()> {
        // Marked closed before the exchanges, not after: a transaction gets one close
        // attempt, and a failed one must surface as the `Err` it is. Marking afterwards
        // means a failure drops `self` unclosed inside this call, and the `Drop`
        // assertion panics over the very error the caller was owed.
        self.closed = true;
        if let Err(e) = self
            .request(Service::Program, 10, cmd::SESSION_CLOSE, &[])
            .await
        {
            // ⚠️ Still say GOODBYE: the HELLO is the half that wedges the instrument
            // into answering "empty" for every slot (see `open`), so a refused close
            // must not strand it. The caller is owed the close's error, so a failure
            // here is deliberately dropped.
            let _ = self
                .request(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, &[])
                .await;
            return Err(e);
        }
        self.request(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, &[])
            .await?;
        Ok(())
    }

    /// Abandon the transaction without running the closing exchanges.
    pub fn abort(mut self) {
        self.closed = true;
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
