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
use std::time::Duration;

use crate::error::{Error, Result};
use crate::transport::Transport;
use crate::wire::{cmd, ui, Message, ObjectClass, Service};

/// Read-only capability. Cannot reach any operation that mutates the device.
#[derive(Debug)]
pub struct ReadOnly;

/// Read-write capability, reachable only through an explicit escalation.
#[derive(Debug)]
pub struct ReadWrite;

/// How many queued [`cmd::CHANGED`] notifications one response read will drain before
/// giving up. A cap, not a protocol fact: it exists so a device streaming
/// notifications cannot pin the host in the read loop forever.
pub const DRAIN_CAP: usize = 32;

pub struct Session<'t, T: Transport, C = ReadOnly> {
    // `Option` rather than a plain `&mut` so the capability escalation can move the
    // borrow out: a type implementing `Drop` cannot be destructured.
    transport: Option<&'t mut T>,
    class: ObjectClass,
    closed: bool,
    device_changed: bool,
    /// How long any single read in this session may take. `None` waits forever, which
    /// is right for the commands NSM sends — they always answer. Set it before probing
    /// a command that might not, so the closing exchanges cannot hang either.
    read_limit: Option<Duration>,
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
            device_changed: false,
            read_limit: None,
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
        // can be released first ([`Self::release`] marks it closed and says the
        // best-effort GOODBYE) — the Drop assertion is there to catch *forgotten*
        // commits, not failed connections.
        let hello = Message::new(Service::Ui, ui::SUBSYSTEM, ui::HELLO, Vec::new());
        if let Err(e) = s.notify(&hello).await {
            s.closed = true; // the write itself failed: the device never saw the HELLO
            return Err(e);
        }
        if let Err(e) = s.response_to(ui::HELLO).await {
            // The write landed, so the device may already be holding the UI session
            // even though its reply was unusable.
            s.release().await;
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
                s.release().await;
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
        let (class, closed, device_changed) = (self.class, self.closed, self.device_changed);
        let read_limit = self.read_limit;
        // The husk is about to drop and no longer owns the transaction.
        self.closed = true;
        Session {
            transport,
            class,
            closed,
            device_changed,
            read_limit,
            _capability: PhantomData,
        }
    }
}

impl<T: Transport, C> Session<'_, T, C> {
    pub fn class(&self) -> ObjectClass {
        self.class
    }

    /// Whether an unsolicited [`cmd::CHANGED`] notification arrived during this
    /// session.
    ///
    /// The device queues one on its own when its contents change outside the session —
    /// a front-panel STORE, for instance — and [`Self::request`] drains it rather than
    /// mistaking it for a reply. `true` means the instrument changed under us: state
    /// read earlier in this session may be stale.
    pub fn instrument_changed(&self) -> bool {
        self.device_changed
    }

    /// Bound every read in this session, closing exchanges included.
    ///
    /// Off by default, because the operations NSM performs always draw a reply and a
    /// spurious timeout mid-transfer would be worse than waiting. Set it before
    /// [`Self::probe`]: a command that answers nothing otherwise hangs the caller in
    /// [`Self::commit`] rather than in the probe, having already passed the probe's own
    /// timeout — which is exactly the trap that a bounded probe alone does not close.
    pub fn set_read_limit(&mut self, limit: Duration) {
        self.read_limit = Some(limit);
    }

    /// One frame from the device, honoring [`Self::set_read_limit`].
    ///
    /// `Ok(None)` means the limit passed with nothing read. The transport has already
    /// cancelled the outstanding transfer by then, so the session is still in step.
    async fn read_frame(&mut self) -> Result<Option<Message>> {
        let limit = self.read_limit;
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| Error::Transport("session has no transport".into()))?;

        let raw = match limit {
            Some(limit) => match transport.read_timeout(crate::transport::READ_BUFFER, limit).await?
            {
                Some(raw) => raw,
                None => return Ok(None),
            },
            None => transport.read(crate::transport::READ_BUFFER).await?,
        };
        Message::decode_response(&raw).map(Some)
    }

    /// Send an arbitrary command and return whatever comes back, enforcing nothing.
    ///
    /// For reverse-engineering commands that have no typed operation yet. Unlike
    /// [`Self::request`] this accepts a reply that is not `command + 1` and a non-zero
    /// status, because on an undocumented command both are results rather than faults —
    /// a device that does not implement one still answers, with a status saying so.
    /// `Ok(None)` means it said nothing within `limit`.
    ///
    /// Queued [`cmd::CHANGED`] notifications are drained as in [`Self::request`], so a
    /// front-panel STORE cannot be mistaken for the probe's answer.
    ///
    /// # Warning
    ///
    /// This sends bytes no capture has ever shown the device being sent. Unknown
    /// commands have been reported to leave instrument firmware in a state only a power
    /// cycle clears, and a write-shaped command reaching a real object destroys it.
    /// Probe read-shaped commands, on backed-up content, or not at all.
    pub async fn probe(
        &mut self,
        service: Service,
        subsystem: u32,
        command: u32,
        args: &[u8],
        limit: Duration,
    ) -> Result<Option<Message>> {
        self.set_read_limit(limit);
        let req = Message::new(service, subsystem, command, args.to_vec());
        self.notify(&req).await?;

        let mut drained = 0;
        loop {
            let Some(resp) = self.read_frame().await? else {
                return Ok(None);
            };
            if resp.command == cmd::CHANGED && resp.command != command + 1 && drained < DRAIN_CAP {
                drained += 1;
                self.device_changed = true;
                continue;
            }
            return Ok(Some(resp));
        }
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
    ///
    /// Unsolicited [`cmd::CHANGED`] notifications are drained (up to [`DRAIN_CAP`])
    /// rather than mistaken for the reply. Any other failure to produce a usable,
    /// matching reply is a desync: nothing read after it can be paired with its
    /// request, so the transaction is released before the error is reported.
    async fn response_to(&mut self, command: u32) -> Result<Message> {
        let mut drained = 0;
        loop {
            let resp = match self.read_frame().await {
                Ok(Some(resp)) => resp,
                // The limit passed with no reply. Nothing can be paired with this
                // request afterwards, so it is a desync like any other — but the
                // transfer was cancelled, so the close still has a chance of landing.
                Ok(None) => {
                    self.release().await;
                    return Err(Error::Transport(format!(
                        "no reply to command {command:#04x} within the session's read limit"
                    )));
                }
                Err(e) => {
                    self.release().await;
                    return Err(e);
                }
            };

            if resp.command != command + 1 {
                if resp.command == cmd::CHANGED && drained < DRAIN_CAP {
                    drained += 1;
                    self.device_changed = true;
                    continue;
                }
                self.release().await;
                return Err(Error::UnexpectedResponse {
                    expected: command + 1,
                    got: resp.command,
                });
            }
            return match resp.status() {
                // A refusal is not a desync: request and reply are still in step, the
                // session stays usable, and the caller still owes it a close.
                Some(0) | None => Ok(resp),
                Some(code) => Err(Error::DeviceStatus(code)),
            };
        }
    }

    /// Best-effort release after a bail: mark the transaction over and say GOODBYE
    /// once. Idempotent, so a bail inside [`Self::response_to`] and the caller's own
    /// error path can both come through here.
    ///
    /// ⚠️ The HELLO is the half that wedges the instrument (see [`Self::open`]), so
    /// every bail must reach this before its error is reported. The caller is owed
    /// the original error, and failures here are deliberately dropped; the stream may
    /// be desynced by now, so the GOODBYE's reply is read to keep it out of the next
    /// session's queue but not interpreted.
    async fn release(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let goodbye = Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, Vec::new());
        if self.notify(&goodbye).await.is_err() {
            return;
        }
        // Best effort, and bounded when the session is: this runs on the failure path,
        // where the reason for failing may well be a device that has stopped answering.
        let _ = self.read_frame().await;
    }

    /// Send a fire-and-forget message without waiting for a reply.
    ///
    /// The UI progress strings ([`ui::label`], [`ui::percent`]) are sent this way: the
    /// device never acknowledges them, so routing them through [`Self::request`] would
    /// block forever on a response that never comes.
    ///
    /// Like [`Self::request`] this does not test `closed`, and does not need to:
    /// [`Self::commit`] and [`Self::abort`] both consume `self`, so a session past
    /// its close cannot be reached again. After a mid-session bail ([`Self::release`])
    /// the flag also marks the transaction already released, which `commit` checks
    /// itself.
    pub(crate) async fn notify(&mut self, msg: &Message) -> Result<()> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| Error::Transport("session has no transport".into()))?;
        transport.write(&msg.encode()).await
    }

    /// Run the closing exchanges. Always prefer this over dropping.
    pub async fn commit(mut self) -> Result<()> {
        // Already released by a mid-session bail: the GOODBYE was said, the stream is
        // desynced, and the closing exchanges would pair with stale replies. The bail's
        // own error — which the caller already holds — is the report.
        if self.closed {
            return Ok(());
        }
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
