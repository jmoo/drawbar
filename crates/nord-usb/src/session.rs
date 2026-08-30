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

/// Device status meaning "the session you are using is no longer valid".
///
/// Seen when a previous run left a session open, and after a session reset. It is
/// recoverable without touching the instrument: see [`Session::open`].
pub const STALE_SESSION: u32 = 0x12;

/// Per-frame write liveness bound.
pub const WRITE_LIMIT: Duration = Duration::from_secs(10);

/// Default per-frame read liveness bound; callers may override it per session.
pub const READ_LIMIT: Duration = Duration::from_secs(30);

pub struct Session<'t, T: Transport, C = ReadOnly> {
    // `Option` rather than a plain `&mut` so the capability escalation can move the
    // borrow out: a type implementing `Drop` cannot be destructured.
    transport: Option<&'t mut T>,
    class: ObjectClass,
    closed: bool,
    device_changed: bool,
    read_limit: Duration,
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
            read_limit: READ_LIMIT,
            _capability: PhantomData,
        };

        // ⚠️ Confirmed on hardware: an abandoned UI session makes every slot appear empty.
        s.handshake().await?;

        let opened = s.open_class(class).await;

        // ⚠️ This covers an abandoned *class* session only. An abandoned **UI** session
        // reports every slot as empty without an error; [`recover`] handles that case.
        let opened = match opened {
            Err(Error::DeviceStatus(STALE_SESSION)) => {
                if let Err(error) = s.discard_stale_session().await {
                    s.release().await;
                    return Err(error);
                }
                s.open_class(class).await
            }
            other => other,
        };

        match opened {
            Ok(_) => Ok(s),
            Err(e) => {
                // The HELLO landed, so the UI session is open and must be released.
                s.release().await;
                Err(e)
            }
        }
    }

    /// The UI half of opening: `HELLO` and its reply.
    async fn handshake(&mut self) -> Result<()> {
        let hello = Message::new(Service::Ui, ui::SUBSYSTEM, ui::HELLO, Vec::new());
        if let Err(e) = self.notify(&hello).await {
            self.closed = true; // the write itself failed: the device never saw the HELLO
            return Err(e);
        }
        if let Err(e) = self.response_to(ui::HELLO).await {
            // The write landed, so the device may already be holding the UI session
            // even though its reply was unusable.
            self.release().await;
            return Err(e);
        }
        Ok(())
    }

    async fn open_class(&mut self, class: ObjectClass) -> Result<()> {
        self.request(
            Service::Program,
            10,
            cmd::SESSION_OPEN,
            &class.to_raw().to_be_bytes(),
        )
        .await
        .map(|_| ())
    }

    /// Tell the device to drop a session it still thinks is open.
    ///
    /// Sent **bare** — no `HELLO`, no open — because the machinery that would wrap it is
    /// exactly what the device is refusing. Confirmed on hardware: an instrument that
    /// answers `0x12` to everything is well again immediately afterwards.
    async fn discard_stale_session(&mut self) -> Result<()> {
        let close = Message::new(Service::Program, 10, cmd::SESSION_CLOSE, Vec::new());
        self.notify(&close).await?;
        // Its reply is uninteresting — the point is the side effect — but it must be
        // taken off the wire, or it would be read as the answer to the next request.
        let _ = self.read_frame().await?;
        Ok(())
    }

    /// Escalate to a session that can mutate the device.
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
    /// a front-panel STORE, for instance — and `Session::request` drains it rather than
    /// mistaking it for a reply. `true` means the instrument changed under us: state
    /// read earlier in this session may be stale.
    pub fn instrument_changed(&self) -> bool {
        self.device_changed
    }

    /// Override the per-frame [`READ_LIMIT`] for this session, including its close.
    pub fn set_read_limit(&mut self, limit: Duration) {
        self.read_limit = limit;
    }

    /// One frame from the device, honoring [`Self::set_read_limit`].
    ///
    /// `Ok(None)` means the limit passed with nothing read. The transport has already
    /// cancelled the outstanding transfer by then, so the session is still in step.
    async fn read_frame(&mut self) -> Result<Option<Message>> {
        self.read_frame_with_limit(self.read_limit).await
    }

    async fn read_frame_with_limit(&mut self, limit: Duration) -> Result<Option<Message>> {
        self.read_frame_as(limit, Message::decode_response).await
    }

    async fn read_frame_as(
        &mut self,
        limit: Duration,
        decode: fn(&[u8]) -> Result<Message>,
    ) -> Result<Option<Message>> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| Error::Transport("session has no transport".into()))?;

        let raw = match transport
            .read_timeout(crate::transport::READ_BUFFER, limit)
            .await?
        {
            Some(raw) => raw,
            None => return Ok(None),
        };
        decode(&raw).map(Some)
    }

    /// Send an arbitrary command and return whatever comes back, enforcing nothing.
    ///
    /// For reverse-engineering commands that have no typed operation yet. Unlike
    /// `Session::request` this accepts a reply that is not `command + 1` and a non-zero
    /// status, because on an undocumented command both are results rather than faults —
    /// a device that does not implement one still answers, with a status saying so.
    /// `Ok(None)` means it said nothing within `limit`.
    /// Call [`Self::commit_with_read_limit`] with the same limit to bound cleanup too.
    ///
    /// Queued [`cmd::CHANGED`] notifications are drained as in `Session::request`, so a
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
        let response = command.checked_add(1).ok_or_else(|| {
            Error::InvalidArgument("command 0xffffffff has no response code".into())
        })?;
        let req = Message::new(service, subsystem, command, args.to_vec());
        self.notify(&req).await?;

        let mut drained = 0;
        loop {
            let Some(resp) = self.read_probe_frame_with_limit(limit).await? else {
                return Ok(None);
            };
            if resp.command == cmd::CHANGED && resp.command != response && drained < DRAIN_CAP {
                drained += 1;
                self.device_changed = true;
                continue;
            }
            return Ok(Some(resp));
        }
    }

    async fn read_probe_frame_with_limit(&mut self, limit: Duration) -> Result<Option<Message>> {
        self.read_frame_as(limit, Message::decode_probe).await
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
        let expected = command.checked_add(1).ok_or_else(|| {
            Error::InvalidArgument("command 0xffffffff has no response code".into())
        })?;
        let mut drained = 0;
        loop {
            let resp = match self.read_frame().await {
                Ok(Some(resp)) => resp,
                // A timed-out request desynchronizes replies, but cancellation may let close land.
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

            if resp.command != expected {
                if resp.command == cmd::CHANGED && drained < DRAIN_CAP {
                    drained += 1;
                    self.device_changed = true;
                    continue;
                }
                self.release().await;
                return Err(Error::UnexpectedResponse {
                    expected,
                    got: resp.command,
                });
            }
            return match resp.status() {
                // A refusal is not a desync: request and reply are still in step, the
                // session stays usable, and the caller still owes it a close.
                Some(0) => Ok(resp),
                Some(code) => Err(Error::DeviceStatus(code)),
                None => {
                    self.release().await;
                    Err(Error::Truncated { got: 0, need: 4 })
                }
            };
        }
    }

    /// Best-effort, idempotent release after a failed exchange.
    ///
    /// ⚠️ `HELLO` without `GOODBYE` wedges inventory reads. Release failures do not
    /// replace the operation's original error.
    async fn release(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let goodbye = Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, Vec::new());
        if self.notify(&goodbye).await.is_err() {
            return;
        }
        let _ = self.read_frame().await;
    }

    /// Send a fire-and-forget message without waiting for a reply.
    ///
    /// The UI progress strings ([`ui::label`], [`ui::percent`]) are sent this way: the
    /// device never acknowledges them, so routing them through [`Self::request`] would
    /// block forever on a response that never comes.
    ///
    pub(crate) async fn notify(&mut self, msg: &Message) -> Result<()> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| Error::Transport("session has no transport".into()))?;
        let encoded = msg.encode();
        if transport.write_timeout(&encoded, WRITE_LIMIT).await? {
            Ok(())
        } else {
            Err(Error::Transport(format!(
                "the device did not accept command {:#04x} within {}s: its bulk endpoints \
                 are stalled, and a power cycle is the only way out — `nord device recover` \
                 cannot help, because that frame cannot be delivered either",
                msg.command,
                WRITE_LIMIT.as_secs()
            )))
        }
    }

    /// Run the closing exchanges. Always prefer this over dropping.
    pub async fn commit(mut self) -> Result<()> {
        // A failed exchange already released the session and reported its error.
        if self.closed {
            return Ok(());
        }
        // Mark first so a failed close surfaces as `Err` instead of a Drop assertion.
        self.closed = true;
        if let Err(e) = self
            .request(Service::Program, 10, cmd::SESSION_CLOSE, &[])
            .await
        {
            // ⚠️ A refused close must still say GOODBYE; its failure does not replace
            // the class-close error.
            let _ = self
                .request(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, &[])
                .await;
            return Err(e);
        }
        self.request(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, &[])
            .await?;
        Ok(())
    }

    /// Commit with a bounded close for exploratory probes.
    /// The consumed session's ordinary read behavior is unchanged.
    pub async fn commit_with_read_limit(mut self, limit: Duration) -> Result<()> {
        self.read_limit = limit;
        self.commit().await
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
