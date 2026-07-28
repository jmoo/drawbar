//! Real USB transport, via `nusb` (pure Rust: macOS/IOKit, Linux/usbfs, Windows/WinUSB).
//!
//! Enumeration lives here rather than in the portable core on purpose — WebUSB has no
//! programmatic device listing at all (its `requestDevice()` needs a user gesture), so
//! a cross-platform `list()` would be a lie.

use nusb::transfer::{Queue, RequestBuffer};
use nusb::{DeviceInfo, Interface};

use super::{Transport, EP_IN, EP_OUT};
use crate::error::{Error, Result};

/// Clavia DMI AB. Read off the device descriptor in a firmware-update capture.
pub const VENDOR_ID: u16 = 0x0ffc;
/// Nord Electro 5.
pub const PRODUCT_ID_ELECTRO5: u16 = 0x0027;

/// USB vendor-specific interface class. The protocol rides this; the instrument's
/// other interface is USB-MIDI (audio class), which we deliberately leave alone so
/// CoreMIDI/ALSA keep working.
const CLASS_VENDOR_SPECIFIC: u8 = 0xff;

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
        Ok(Self { interface, read_queue })
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
}
