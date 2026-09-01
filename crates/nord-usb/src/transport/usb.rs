//! Real USB transport, via `nusb` (pure Rust: macOS/IOKit, Linux/usbfs, Windows/WinUSB).
//!
//! Enumeration lives here rather than in the portable core on purpose — WebUSB has no
//! programmatic device listing at all (its `requestDevice()` needs a user gesture), so
//! a cross-platform `list()` would be a lie.

use std::time::Duration;

use nusb::transfer::{Control, ControlType, Queue, RequestBuffer};
use nusb::{DeviceInfo, Interface};

use super::record::Recorder;
use super::{needs_terminator, Transport, CLASS_VENDOR_SPECIFIC, EP_IN, EP_OUT};
use crate::deadline::with_timeout;
use crate::error::{Error, Result};

pub use super::{PRODUCT_ID_ELECTRO5, VENDOR_ID};
// Re-exported so callers can name a control recipient without depending on `nusb`.
pub use nusb::transfer::Recipient;

/// How long a cancelled transfer is given to come back before the transport is declared
/// out of step. Cancellation is local to the host controller, so this covers a stall,
/// not a device round trip.
const REAP_LIMIT: Duration = Duration::from_secs(2);

fn map_err<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error::Transport(format!("{what}: {e}"))
}

/// Why the device would not open.
///
/// A missing udev rule fails *here* rather than at the interface claim: the usbfs node
/// is not writable, so the device never opens and the claim is never reached. Naming the
/// rule is the difference between a one-line fix and hunting a hardware fault, so the
/// hint has to ride on this call to be seen at all.
fn open_error(e: std::io::Error) -> Error {
    let hint = if cfg!(target_os = "linux") && e.kind() == std::io::ErrorKind::PermissionDenied {
        format!(
            " — no write access to the device node; what is usually missing is a udev rule \
             granting it for vendor {VENDOR_ID:04x}"
        )
    } else {
        String::new()
    };
    Error::Transport(format!("opening device: {e}{hint}"))
}

/// Every attached Clavia device.
pub fn list() -> Result<Vec<DeviceInfo>> {
    Ok(nusb::list_devices()
        .map_err(map_err("listing usb devices"))?
        .filter(|d| d.vendor_id() == VENDOR_ID)
        .collect())
}

pub struct UsbTransport {
    interface: Interface,
    /// `wMaxPacketSize` for [`EP_OUT`], read from the interface descriptor rather than
    /// assumed: it decides which frames need a terminating zero-length packet, and it
    /// is 64 only because this link is full speed.
    out_packet: usize,
    /// What the device descriptor calls itself, kept for the recorder's header.
    product: Option<String>,
    /// Set to mirror every frame into a replay script. `None` is the normal case.
    record: Option<Recorder>,
    // A persistent IN queue: submitting a fresh buffer per read is simpler to reason
    // about than juggling completions, and the protocol is strictly turn-taking.
    read_queue: Queue<RequestBuffer>,
}

impl UsbTransport {
    /// Open the first attached Clavia device.
    pub fn open_first() -> Result<Self> {
        let info = list()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Transport("no Clavia device found".into()))?;
        Self::open(&info)
    }

    pub fn open(info: &DeviceInfo) -> Result<Self> {
        // Claim the vendor-specific interface, discovered by class rather than
        // hard-coded: the audio/MIDI interface must be left to the OS driver.
        let iface_num = info
            .interfaces()
            .find(|i| i.class() == CLASS_VENDOR_SPECIFIC)
            .map(|i| i.interface_number())
            .ok_or_else(|| {
                Error::Transport(
                    "device exposes no vendor-specific interface; is this a Nord?".into(),
                )
            })?;

        let device = info.open().map_err(open_error)?;
        let interface = device.claim_interface(iface_num).map_err(map_err(
            "claiming the vendor interface (another application holding it — Nord Sound \
             Manager, or a WebUSB page — will block this)",
        ))?;

        let out_packet = interface
            .descriptors()
            .find_map(|alt| {
                alt.endpoints()
                    .find(|ep| ep.address() == EP_OUT)
                    .map(|ep| ep.max_packet_size())
            })
            .filter(|size| *size > 0)
            .ok_or_else(|| {
                Error::Transport(format!(
                    "the vendor interface reports no bulk OUT endpoint at {EP_OUT:#04x}"
                ))
            })?;

        let read_queue = interface.bulk_in_queue(EP_IN);
        Ok(Self {
            interface,
            out_packet,
            product: info.product_string().map(str::to_owned),
            record: None,
            read_queue,
        })
    }

    /// End a frame the device would otherwise still be reading — see
    /// [`needs_terminator`].
    async fn terminate(&mut self, written: usize) -> Result<()> {
        if !needs_terminator(written, self.out_packet) {
            return Ok(());
        }
        let completion = self.interface.bulk_out(EP_OUT, Vec::new()).await;
        completion.status.map_err(map_err("bulk write terminator"))
    }

    async fn write_frame(&mut self, buf: &[u8]) -> Result<()> {
        let completion = self.interface.bulk_out(EP_OUT, buf.to_vec()).await;
        completion.status.map_err(map_err("bulk write"))?;
        self.terminate(buf.len()).await
    }

    /// Mirror every frame this transport carries into a replay script at `path`.
    ///
    /// The script is written as it goes, so it stays useful even if the run ends badly.
    /// Recording an operation that moves a body writes that body out in full.
    pub fn recording_to(mut self, path: &std::path::Path) -> Result<Self> {
        self.record = Some(Recorder::create(path, self.describe().as_deref())?);
        Ok(self)
    }

    /// Model and firmware for the script header, best effort: the model comes from the
    /// descriptor, the rest from endpoint 0, and an instrument that will not answer
    /// there is still worth recording.
    fn describe(&self) -> Option<String> {
        let product = self.product.as_deref()?;
        Some(match self.identity() {
            Ok(id) => format!(
                "{product}, firmware v{}.{:02} build {}",
                id.firmware / 100,
                id.firmware % 100,
                id.build
            ),
            Err(_) => product.to_string(),
        })
    }

    /// Declare what the frames that follow belong to. No-op when not recording.
    pub fn mark_intent(&mut self, intent: &str) {
        if let Some(r) = self.record.as_mut() {
            r.intent(intent);
        }
    }

    /// Label the frames that follow in the script. No-op when not recording.
    pub fn mark(&mut self, what: &str) {
        if let Some(r) = self.record.as_mut() {
            r.comment(what);
        }
    }

    /// Record that the transaction just performed failed. No-op when not recording.
    pub fn mark_expect(&mut self, e: &Error) {
        if let Some(r) = self.record.as_mut() {
            r.expect(e);
        }
    }

    /// Surface the first error the recorder hit, if it is recording. Call after the
    /// operation completes: a failed write is deliberately not allowed to abort a live
    /// session part-way.
    pub fn recording_result(&mut self) -> Result<()> {
        match self.record.as_mut() {
            Some(r) => r.check(),
            None => Ok(()),
        }
    }
}

/// What the device says about itself on endpoint 0, outside the bulk protocol.
///
/// Read with no session open, so it answers even when the instrument is wedged — which
/// makes it the one identification that still works when nothing else does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// Firmware version as the device reports it, in hundredths: `204` is 2.04. The
    /// same value the USB descriptor carries as `bcdDevice`, which confirms the
    /// scaling.
    pub firmware: u16,
    /// Largest transfer the device will accept or produce, in bytes, framing included.
    ///
    /// [`crate::op`]'s read chunk is this minus the frame header and CRC — a bound
    /// derived from captures long before the device was asked for it, and the two agree
    /// exactly.
    pub max_transfer: u32,
    /// Reported at request `0x00`. Reads as a small constant; its meaning is not pinned
    /// down, so it is carried verbatim rather than named something it might not be.
    pub kind: u16,
    /// Reported at request `0x05`. Plausibly a build number, unconfirmed.
    pub build: u16,
}

impl UsbTransport {
    /// Ask the device to identify itself over endpoint 0. Read-only.
    ///
    /// Opens no transaction, so unlike everything in [`crate::op`] this is safe on an
    /// instrument in an unknown state.
    pub fn identity(&self) -> Result<Identity> {
        let limit = Duration::from_millis(500);
        let word = |request: u8| -> Result<u16> {
            let b = self.vendor_control_in(Recipient::Device, request, 0, 0, 2, limit)?;
            if b.len() < 2 {
                return Err(Error::Transport(format!(
                    "vendor request {request:#04x} returned {} bytes, expected 2",
                    b.len()
                )));
            }
            Ok(u16::from_le_bytes([b[0], b[1]]))
        };

        let max = self.vendor_control_in(Recipient::Device, 0x08, 0, 0, 4, limit)?;
        if max.len() < 4 {
            return Err(Error::Transport(format!(
                "vendor request 0x08 returned {} bytes, expected 4",
                max.len()
            )));
        }

        Ok(Identity {
            kind: word(0x00)?,
            firmware: word(0x04)?,
            build: word(0x05)?,
            max_transfer: u32::from_le_bytes([max[0], max[1], max[2], max[3]]),
        })
    }

    /// One read on the interrupt endpoint (`0x81`), or `None` on timeout. Nothing is
    /// known to arrive here outside the firmware-update handshake.
    pub async fn interrupt_read(
        &mut self,
        len: usize,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>> {
        use crate::deadline::with_timeout;
        let buf = nusb::transfer::RequestBuffer::new(len);
        match with_timeout(self.interface.interrupt_in(0x81, buf), timeout).await {
            Some(completion) => {
                completion.status.map_err(map_err("interrupt read"))?;
                Ok(Some(completion.data))
            }
            None => Ok(None),
        }
    }

    /// One vendor control read on endpoint 0, outside the bulk protocol entirely.
    ///
    /// Separate from [`Transport`] on purpose: WebUSB can issue control transfers, but
    /// nothing portable is built on this yet, and putting it in the trait would oblige
    /// the replay backend to fake a channel no capture covers.
    ///
    /// Returns the bytes the device sent, truncated to what it actually produced — a
    /// device that recognises the request but has less to say than `len` is normal, and
    /// an unrecognised request stalls the endpoint, which surfaces as an error rather
    /// than as empty data.
    ///
    /// The timeout is the driver's own, so this cannot hang the way a bulk read can.
    pub fn vendor_control_in(
        &self,
        recipient: Recipient,
        request: u8,
        value: u16,
        index: u16,
        len: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let control = Control {
            control_type: ControlType::Vendor,
            recipient,
            request,
            value,
            index,
        };
        let n = self
            .interface
            .control_in_blocking(control, &mut buf, timeout)
            .map_err(map_err("vendor control read"))?;
        buf.truncate(n);
        Ok(buf)
    }
}

impl Transport for UsbTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.write_frame(buf).await?;
        // The terminator is a packet, not a frame: recording it would put a length word
        // in the script that no message has.
        if let Some(r) = self.record.as_mut() {
            r.out(buf);
        }
        Ok(())
    }

    async fn read(&mut self, max: usize) -> Result<Vec<u8>> {
        self.read_queue.submit(RequestBuffer::new(max));
        let completion = self.read_queue.next_complete().await;
        completion.status.map_err(map_err("bulk read"))?;
        if let Some(r) = self.record.as_mut() {
            r.r#in(&completion.data);
        }
        Ok(completion.data)
    }

    async fn write_timeout(&mut self, buf: &[u8], limit: Duration) -> Result<bool> {
        // Each `bulk_out` owns its transfer, so dropping the future cancels it.
        match with_timeout(self.write_frame(buf), limit).await {
            Some(result) => {
                result?;
                if let Some(r) = self.record.as_mut() {
                    r.out(buf);
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn read_timeout(&mut self, max: usize, limit: Duration) -> Result<Option<Vec<u8>>> {
        self.read_queue.submit(RequestBuffer::new(max));

        if let Some(completion) = with_timeout(self.read_queue.next_complete(), limit).await {
            completion.status.map_err(map_err("bulk read"))?;
            if let Some(r) = self.record.as_mut() {
                r.r#in(&completion.data);
            }
            return Ok(Some(completion.data));
        }

        // Cancel and reap the OS-owned transfer before any later read can consume it.
        self.read_queue.cancel_all();
        match with_timeout(self.read_queue.next_complete(), REAP_LIMIT).await {
            Some(_) => Ok(None),
            // An unreaped cancellation leaves the response queue out of step.
            None => Err(Error::Transport(
                "read timed out and the transfer could not be cancelled; \
                 the connection is out of step and the instrument needs a power cycle"
                    .into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error as IoError, ErrorKind};

    /// The udev hint is the whole point of this branch, and the failure it explains is
    /// the one every first run on Linux hits.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "the hint is Linux-only")]
    fn permission_denied_names_the_udev_rule() {
        let msg = open_error(IoError::from(ErrorKind::PermissionDenied)).to_string();
        assert!(msg.contains("udev"), "{msg}");
        assert!(msg.contains("0ffc"), "{msg}");
    }

    /// Every other failure is something else entirely — a rule would not fix a device
    /// that has been unplugged, and saying so would send the reader the wrong way.
    #[test]
    fn other_failures_do_not_mention_udev() {
        for kind in [ErrorKind::NotFound, ErrorKind::ResourceBusy] {
            let msg = open_error(IoError::from(kind)).to_string();
            assert!(!msg.contains("udev"), "{kind:?}: {msg}");
        }
    }
}
