//! The transaction wrapper every operation runs inside.
//!
//! Every operation is enclosed by the same exchange sequence:
//!
//! ```text
//! O18 I22, O22 I26, [ operation ], O22 I42, O18 I22, O18 I22
//! ```
//!
//! The numbers are payload bytes. Captures show frame lengths 40 higher, which is the
//! capture tool's Darwin header and not part of the wire.
//!
//! Closing is explicit. `Drop` is neither async nor fallible, so a failed close there
//! would be lost while a half-open transaction may leave the device in an odd state.
//! `Drop` only complains, in debug builds.

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
/// giving up. A host limit, so a device streaming notifications cannot keep the host
/// in the read loop forever.
pub const DRAIN_CAP: usize = 32;

/// Device status meaning the session in use is not valid.
///
/// Seen when a previous run left a session open, and after a session reset. It is
/// recoverable without touching the instrument: see [`Session::open`].
pub const STALE_SESSION: u32 = 0x12;

/// Per-frame write liveness bound.
pub const WRITE_LIMIT: Duration = Duration::from_secs(10);

/// Default per-frame read liveness bound; callers may override it per session.
pub const READ_LIMIT: Duration = Duration::from_secs(30);

pub struct Session<'t, T: Transport, C = ReadOnly> {
    // An `Option` so the capability escalation can move the borrow out: a type
    // implementing `Drop` cannot be destructured.
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
    /// `STATUS` and the addressing operations report on the class that was opened, so
    /// opening the wrong one yields plausible numbers about the wrong class.
    pub async fn open(transport: &'t mut T, class: ObjectClass) -> Result<Self> {
        let mut s = Self {
            transport: Some(transport),
            class,
            closed: false,
            device_changed: false,
            read_limit: READ_LIMIT,
            _capability: PhantomData,
        };

        // ⚠️ An abandoned UI session makes every slot appear empty. Confirmed on hardware.
        s.handshake().await?;

        let opened = s.open_class(class).await;

        // ⚠️ This covers an abandoned class session only. An abandoned UI session
        // reports every slot as empty without an error; `op::recover` handles that case.
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
                Err(match e {
                    Error::DeviceStatus(status) if status != STALE_SESSION => {
                        Error::ClassRefused { class, status }
                    }
                    other => other,
                })
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
    /// Sent bare, with no `HELLO` and no open, because those are what the device is
    /// refusing. Confirmed on hardware. An instrument that answers `0x12` to everything
    /// recovers immediately afterward.
    async fn discard_stale_session(&mut self) -> Result<()> {
        let close = Message::new(Service::Program, 10, cmd::SESSION_CLOSE, Vec::new());
        self.notify(&close).await?;
        // The reply is ignored, but it must be read or it would be taken as the answer
        // to the next request.
        let _ = self.read_frame().await?;
        Ok(())
    }

    /// Escalate to a session that can mutate the device.
    pub fn allow_destructive_writes(mut self) -> Session<'t, T, ReadWrite> {
        let transport = self.transport.take();
        let (class, closed, device_changed) = (self.class, self.closed, self.device_changed);
        let read_limit = self.read_limit;
        // The old session is about to drop and does not own the transaction.
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
    /// The device queues one when its contents change outside the session, for example
    /// after a front-panel STORE, and requests drain it instead of taking it for a
    /// reply. `true` means state read earlier in this session may be stale.
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
    /// canceled the outstanding transfer by then, so the session is still in step.
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
    /// For reverse-engineering commands that have no typed operation yet. Unlike a typed
    /// request, this accepts a reply that is not `command + 1` and a non-zero status:
    /// on an undocumented command both are results, since a device that does not
    /// implement one still answers with a status saying so. `Ok(None)` means it said
    /// nothing within `limit`. Call [`Self::commit_with_read_limit`] with the same limit
    /// to bound cleanup too.
    ///
    /// Queued [`cmd::CHANGED`] notifications are drained as in a typed request, so a
    /// front-panel STORE cannot be mistaken for the probe's answer.
    ///
    /// # Warning
    ///
    /// This sends bytes no capture has shown the device receiving. Unknown commands have
    /// been reported to leave instrument firmware in a state only a power cycle clears,
    /// and a write-shaped command reaching a real object destroys it. Probe only
    /// read-shaped commands, and only on backed-up content.
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

    /// Send one request and read its response through [`Self::response_to`].
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
    /// Unsolicited [`cmd::CHANGED`] notifications are drained, up to [`DRAIN_CAP`]. Any
    /// other failure to produce a usable, matching reply is a desync: nothing read after
    /// it can be paired with its request, so the transaction is released before the
    /// error is reported.
    async fn response_to(&mut self, command: u32) -> Result<Message> {
        let expected = command.checked_add(1).ok_or_else(|| {
            Error::InvalidArgument("command 0xffffffff has no response code".into())
        })?;
        let mut drained = 0;
        loop {
            let resp = match self.read_frame().await {
                Ok(Some(resp)) => resp,
                // A timed-out request desynchronizes replies, but the canceled read
                // may still let the release land.
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
                // A refusal leaves request and reply in step: the session stays usable,
                // and the caller still owes it a close.
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
    /// The UI progress strings ([`ui::label`], [`ui::percent`]) are sent this way. The
    /// device never acknowledges them, so [`Self::request`] would wait forever.
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
                 are stalled. Only a power cycle clears them; `nord device recover` cannot, \
                 because its frames cannot be delivered either",
                msg.command,
                WRITE_LIMIT.as_secs()
            )))
        }
    }

    /// Run the closing exchanges. Call this before dropping; [`Self::abort`] skips them.
    pub async fn commit(mut self) -> Result<()> {
        self.close().await
    }

    pub(crate) async fn commit_observing_changed(mut self) -> (Result<()>, bool) {
        let result = self.close().await;
        (result, self.device_changed)
    }

    async fn close(&mut self) -> Result<()> {
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
        self.close().await
    }

    /// Abandon the transaction without running the closing exchanges.
    pub fn abort(mut self) {
        self.closed = true;
    }
}

impl<T: Transport, C> Drop for Session<'_, T, C> {
    fn drop(&mut self) {
        // ⚠️ Asserting during an unwind aborts the process and hides the original panic.
        debug_assert!(
            self.closed || std::thread::panicking(),
            "Session dropped without commit() or abort(); the device may be left \
             mid-transaction. Close it explicitly."
        );
    }
}

// With `panic = "abort"`, the panic these tests catch would end the test binary.
#[cfg(all(test, panic = "unwind"))]
mod tests {
    use super::*;

    struct Silent;

    impl Transport for Silent {
        async fn write(&mut self, _buf: &[u8]) -> Result<()> {
            Ok(())
        }

        async fn read(&mut self, _max: usize) -> Result<Vec<u8>> {
            Err(Error::Transport("the test device says nothing".into()))
        }
    }

    /// The `Drop` assertion firing during the unwind would abort the process.
    #[test]
    fn a_panic_inside_a_session_is_not_replaced_by_the_drop_assertion() {
        let mut transport = Silent;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _session: Session<'_, Silent, ReadOnly> = Session {
                transport: Some(&mut transport),
                class: ObjectClass::Program,
                closed: false,
                device_changed: false,
                read_limit: READ_LIMIT,
                _capability: PhantomData,
            };
            panic!("the operation failed");
        }))
        .expect_err("the closure panics");

        assert_eq!(
            *panic.downcast::<&str>().expect("the original payload"),
            "the operation failed"
        );
    }
}
