//! Browser transport, via WebUSB (`navigator.usb`). Chrome/Edge only — Firefox and
//! Safari have declined the spec.
//!
//! **Confirmed on hardware** (Chrome on macOS, a real Electro 5) for the read-only
//! path: open, claim-by-class, both transfer directions across whole transactions
//! (session open, STATUS, INFO, close), the masked endpoint numbering, and
//! release/close handing the interface back to other hosts. **Still not confirmed:**
//! the `select_configuration` branch (it only runs when the OS left the device
//! unconfigured, which macOS does not), multi-chunk bulk reads, any write, and the
//! zero-length-packet edge on `read` — each is marked where it lives.
//!
//! # Enumeration is not here
//!
//! A page can only obtain a device from `navigator.usb.requestDevice()`, and that call
//! requires transient user activation — a click, in the page, that no library function
//! can manufacture. So device *selection* belongs to the page (`crates/drawbar`
//! shows the whole ceremony) and this module starts from a [`UsbDevice`] the page
//! already has. That is the same split [`super::usb`] documents, arrived at from the
//! other direction: there, enumeration is host-specific; here, it is gesture-bound.

use js_sys::{Reflect, Uint8Array};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{UsbDevice, UsbTransferStatus};

use super::{Transport, CLASS_VENDOR_SPECIFIC, EP_IN, EP_OUT};
use crate::error::{Error, Result};

/// WebUSB addresses an endpoint by its *number*, without the direction bit that a
/// descriptor address carries: [`EP_IN`] `0x82` is endpoint 2, [`EP_OUT`] `0x03` is
/// endpoint 3. Passing the raw address instead yields `NotFoundError`.
const fn endpoint_number(address: u8) -> u8 {
    address & 0x0f
}

/// Rejected promises carry a `DOMException`, which is not a `js_sys::Error`, so its
/// text has to be read off the object rather than downcast to one.
fn describe(err: &JsValue) -> String {
    let field = |k: &str| {
        Reflect::get(err, &JsValue::from_str(k))
            .ok()
            .and_then(|v| v.as_string())
    };
    match (field("name"), field("message")) {
        (Some(name), Some(msg)) => format!("{name}: {msg}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => err.as_string().unwrap_or_else(|| format!("{err:?}")),
    }
}

fn map_err(what: &str) -> impl FnOnce(JsValue) -> Error + '_ {
    move |e| Error::Transport(format!("{what}: {}", describe(&e)))
}

fn check_status(status: UsbTransferStatus, what: &str) -> Result<()> {
    match status {
        UsbTransferStatus::Ok => Ok(()),
        // A mid-transaction stall is a desync; clearing it would preserve bad framing.
        UsbTransferStatus::Stall => Err(Error::Transport(format!(
            "{what}: endpoint stalled; the device must be reconnected or clearHalt() called"
        ))),
        UsbTransferStatus::Babble => Err(Error::Transport(format!(
            "{what}: device sent more than the transfer asked for"
        ))),
        other => Err(Error::Transport(format!(
            "{what}: unexpected transfer status {other:?}"
        ))),
    }
}

/// A [`Transport`] over an opened, claimed WebUSB device.
///
/// ⚠️ The device must be **opened and its vendor interface claimed** before any
/// transfer. [`WebUsbTransport::open`] does that; [`WebUsbTransport::new`] assumes the
/// caller already has.
pub struct WebUsbTransport {
    device: UsbDevice,
    /// `None` when the caller claimed the interface itself, in which case
    /// [`Self::close`] cannot release it on their behalf.
    interface_number: Option<u8>,
}

impl WebUsbTransport {
    /// Wrap a device that is already open with the vendor interface claimed.
    ///
    /// ⚠️ [`Self::close`] will not release that interface — it does not know which one
    /// to release. Release it yourself, or use [`Self::open`].
    pub fn new(device: UsbDevice) -> Self {
        Self {
            device,
            interface_number: None,
        }
    }

    /// Run the browser ceremony: open the device, configure it if the platform left it
    /// unconfigured, find the vendor-specific interface **by class**, and claim it.
    ///
    /// The interface number is discovered rather than hard-coded for the same reason as
    /// on the desktop backend: the instrument's other interface is USB-MIDI and must be
    /// left to the OS. (Chrome refuses to claim an audio-class interface at all, so
    /// getting this wrong surfaces as a `SecurityError` rather than silent MIDI loss.)
    pub async fn open(device: UsbDevice) -> Result<Self> {
        JsFuture::from(device.open())
            .await
            .map_err(map_err("opening the device"))?;

        // Inferred, not confirmed on hardware: an unconfigured device uses configuration 1.
        if device.configuration().is_none() {
            JsFuture::from(device.select_configuration(1))
                .await
                .map_err(map_err("selecting configuration 1"))?;
        }

        let configuration = device.configuration().ok_or_else(|| {
            Error::Transport("device reports no active configuration after selecting one".into())
        })?;

        let interface_number = configuration
            .interfaces()
            .iter()
            .find(|iface| iface.alternate().interface_class() == CLASS_VENDOR_SPECIFIC)
            .map(|iface| iface.interface_number())
            .ok_or_else(|| {
                Error::Transport(
                    "device exposes no vendor-specific interface; is this a Nord?".into(),
                )
            })?;

        JsFuture::from(device.claim_interface(interface_number))
            .await
            .map_err(map_err(
                "claiming the vendor interface (another application holding it — \
                 Nord Sound Manager — will block this)",
            ))?;

        Ok(Self {
            device,
            interface_number: Some(interface_number),
        })
    }

    /// The wrapped device, for pages that want to read its descriptors.
    pub fn device(&self) -> &UsbDevice {
        &self.device
    }

    /// Release the interface and close the device.
    ///
    /// ⚠️ A claim is held for as long as the page holds the device handle, so a page
    /// that never releases keeps Nord Sound Manager (and the desktop backend) locked
    /// out until the tab closes.
    pub async fn close(self) -> Result<()> {
        if let Some(number) = self.interface_number {
            JsFuture::from(self.device.release_interface(number))
                .await
                .map_err(map_err("releasing the vendor interface"))?;
        }
        JsFuture::from(self.device.close())
            .await
            .map_err(map_err("closing the device"))?;
        Ok(())
    }
}

impl Transport for WebUsbTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        // ⚠️ WebUSB reads asynchronously; a view into growable wasm memory can be invalidated.
        let data = Uint8Array::from(buf);
        let transfer = self
            .device
            .transfer_out_with_u8_array(endpoint_number(EP_OUT), &data)
            .map_err(map_err("bulk write"))?;
        let result = JsFuture::from(transfer)
            .await
            .map_err(map_err("bulk write"))?;

        check_status(result.status(), "bulk write")?;
        // A short write truncates a frame and desynchronizes the next response.
        let written = result.bytes_written() as usize;
        if written != buf.len() {
            return Err(Error::Transport(format!(
                "bulk write sent {written} of {} bytes",
                buf.len()
            )));
        }
        Ok(())
    }

    async fn read(&mut self, max: usize) -> Result<Vec<u8>> {
        // ⚠️ WebUSB has no timeout, so this relies on each message ending with a
        // short packet. Confirmed on hardware; `READ_BUFFER` is packet-aligned.
        let result = JsFuture::from(self.device.transfer_in(endpoint_number(EP_IN), max as u32))
            .await
            .map_err(map_err("bulk read"))?;

        check_status(result.status(), "bulk read")?;

        // A zero-length transfer has no decodable protocol message.
        let view = result
            .data()
            .ok_or_else(|| Error::Transport("bulk read returned no data".into()))?;

        // Copy the DataView window, not its entire backing buffer.
        Ok(Uint8Array::new_with_byte_offset_and_length(
            &view.buffer(),
            view.byte_offset() as u32,
            view.byte_length() as u32,
        )
        .to_vec())
    }
}
