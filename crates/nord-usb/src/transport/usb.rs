//! Real USB transport, via `nusb` (pure Rust: macOS/IOKit, Linux/usbfs, Windows/WinUSB).
//!
//! Enumeration lives here rather than in the portable core on purpose — WebUSB has no
//! programmatic device listing at all (its `requestDevice()` needs a user gesture), so
//! a cross-platform `list()` would be a lie.

use std::time::Duration;

use nusb::transfer::{Queue, RequestBuffer};
use nusb::{DeviceInfo, Interface};

use super::{Transport, CLASS_VENDOR_SPECIFIC, EP_IN, EP_OUT};
use crate::deadline::with_timeout;
use crate::error::{Error, Result};

pub use super::{PRODUCT_ID_ELECTRO5, VENDOR_ID};

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
