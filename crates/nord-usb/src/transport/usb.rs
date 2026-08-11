//! Real USB transport, via `nusb` (pure Rust: macOS/IOKit, Linux/usbfs, Windows/WinUSB).
//!
//! Enumeration lives here rather than in the portable core on purpose — WebUSB has no
//! programmatic device listing at all (its `requestDevice()` needs a user gesture), so
//! a cross-platform `list()` would be a lie.

use std::time::Duration;

use nusb::transfer::{Control, ControlType, Queue, RequestBuffer};
use nusb::{DeviceInfo, Interface};

use super::{Transport, CLASS_VENDOR_SPECIFIC, EP_IN, EP_OUT};
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

/// Every attached Clavia device.
pub fn list() -> Result<Vec<DeviceInfo>> {
    Ok(nusb::list_devices()
        .map_err(map_err("listing usb devices"))?
        .filter(|d| d.vendor_id() == VENDOR_ID)
        .collect())
}

pub struct UsbTransport {
    interface: Interface,
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

        let device = info.open().map_err(map_err("opening device"))?;
        let interface = device.claim_interface(iface_num).map_err(map_err(
            "claiming the vendor interface (on Linux this usually means a udev rule is missing)",
        ))?;

        let read_queue = interface.bulk_in_queue(EP_IN);
        Ok(Self {
            interface,
            read_queue,
        })
    }
}

/// What the device says about itself on endpoint 0, outside the bulk protocol.
///
/// Read with no session open, so it answers even when the instrument is wedged — which
/// makes it the one identification that still works when nothing else does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// Firmware version as the device reports it, in hundredths: `204` is 2.04. The
    /// same value the USB descriptor carries as `bcdDevice`, which is what pins the
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
        let completion = self.interface.bulk_out(EP_OUT, buf.to_vec()).await;
        completion.status.map_err(map_err("bulk write"))?;
        Ok(())
    }

    async fn read(&mut self, max: usize) -> Result<Vec<u8>> {
        self.read_queue.submit(RequestBuffer::new(max));
        let completion = self.read_queue.next_complete().await;
        completion.status.map_err(map_err("bulk read"))?;
        Ok(completion.data)
    }

    async fn read_timeout(&mut self, max: usize, limit: Duration) -> Result<Option<Vec<u8>>> {
        self.read_queue.submit(RequestBuffer::new(max));

        if let Some(completion) = with_timeout(self.read_queue.next_complete(), limit).await {
            completion.status.map_err(map_err("bulk read"))?;
            return Ok(Some(completion.data));
        }

        // The submitted transfer is still the OS's; dropping the future did not recall
        // it. Cancel, then collect the completion it produces — leaving it queued would
        // hand it to the next read as if it were that request's reply.
        self.read_queue.cancel_all();
        match with_timeout(self.read_queue.next_complete(), REAP_LIMIT).await {
            Some(_) => Ok(None),
            // Cancellation itself did not complete, so the queue's state is unknown and
            // no later read on this transport can be trusted to be in step.
            None => Err(Error::Transport(
                "read timed out and the transfer could not be cancelled; \
                 the connection is out of step and the instrument needs a power cycle"
                    .into(),
            )),
        }
    }
}
