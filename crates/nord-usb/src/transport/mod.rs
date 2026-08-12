//! The bottom of the stack: moving bytes to and from the device.
//!
//! Everything above this trait is pure logic, so the whole protocol can be built and
//! tested against committed captures with no hardware attached — the same property
//! that makes [`nord_format`] trustworthy.

use crate::error::Result;

#[cfg(feature = "nusb")]
pub mod usb;
#[cfg(feature = "nusb")]
pub use usb::UsbTransport;

// ⚠️ Also gated on the architecture, not just the feature: `web-sys` only emits its
// WebUSB bindings under `--cfg=web_sys_unstable_apis`, which `crates/.cargo/config.toml`
// sets for the wasm target alone. Off wasm32 the module is simply absent, so a host
// build of a workspace member that enables `web` still compiles.
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub mod web;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub use web::WebUsbTransport;

#[cfg(feature = "replay")]
pub mod replay;
#[cfg(feature = "replay")]
pub use replay::{Direction, ReplayTransport, Step};

/// Clavia DMI AB. Read off the device descriptor in a firmware-update capture.
pub const VENDOR_ID: u16 = 0x0ffc;
/// Nord Electro 5.
pub const PRODUCT_ID_ELECTRO5: u16 = 0x0027;

/// USB vendor-specific interface class. The protocol rides this; the instrument's
/// other interface is USB-MIDI (audio class), which every backend must leave alone so
/// CoreMIDI/ALSA keep working — and which the browser would refuse to claim anyway.
pub const CLASS_VENDOR_SPECIFIC: u8 = 0xff;

/// Vendor bulk IN endpoint (device → host). Settled across every corpus capture.
pub const EP_IN: u8 = 0x82;
/// Vendor bulk OUT endpoint (host → device).
pub const EP_OUT: u8 = 0x03;

/// The read buffer NSM posts. The device answers with ~32KB chunks; the size is the
/// device's choice, not a USB constraint (the link is Full Speed, 64-byte packets).
pub const READ_BUFFER: usize = 49152;

/// A bidirectional byte pipe to the device.
///
/// # Why this shape
///
/// **No `Send` bounds.** WASM is single-threaded and `web-sys` types are `!Send`, so
/// requiring `Send` futures — which `#[async_trait]` adds by default, and which
/// `tokio::spawn` demands — would make the WebUSB backend impossible, and the
/// requirement would infect every generic bound above this one. The
/// `async_fn_in_trait` lint fires precisely because callers *cannot* add a `Send`
/// bound here; that is the intent, so it is allowed deliberately. Desktop callers
/// needing `Send` should bound on a `SendTransport` marker rather than changing this.
///
/// **Separate directions, not request/response.** Several operations send multiple
/// OUTs before any IN (`delete` is `O36 O26 I30`), so a `send_and_receive()` primitive
/// would be a lie.
///
/// **Owned buffers.** WebUSB hands back an `ArrayBuffer`; a borrowed `&[u8]` return
/// cannot be honored.
///
/// **No timeout parameter.** WebUSB has no native transfer timeout — callers wrap.
#[allow(async_fn_in_trait)]
pub trait Transport {
    /// Write one message to the OUT endpoint.
    async fn write(&mut self, buf: &[u8]) -> Result<()>;

    /// Read up to `max` bytes from the IN endpoint.
    async fn read(&mut self, max: usize) -> Result<Vec<u8>>;

    /// Read, giving up after `limit`. `Ok(None)` means nothing arrived in time.
    ///
    /// For probing commands whose existence is unknown: a device that does not
    /// recognise one may answer with an error status, or may say nothing at all, and
    /// [`Self::read`] would wait forever on the second case. Killing a hung process
    /// instead leaves the transaction open, which wedges the instrument until it is
    /// power-cycled.
    ///
    /// The default implementation **has no timeout** — it defers to [`Self::read`] and
    /// can only return `Ok(Some(_))`. Honoring the limit requires cancelling a transfer
    /// already submitted to the OS, which is backend-specific; a backend that cannot do
    /// that must not pretend to, because abandoning a submitted read desynchronises
    /// every later request from its response.
    async fn read_timeout(
        &mut self,
        max: usize,
        _limit: std::time::Duration,
    ) -> Result<Option<Vec<u8>>> {
        self.read(max).await.map(Some)
    }

    /// Write, giving up after `limit`. `Ok(false)` means the device never accepted it.
    ///
    /// The other half of [`Self::read_timeout`], and not a symmetry for its own sake: a
    /// device can stop accepting writes without stopping altogether. Sending it a frame
    /// it cannot handle has been observed to stall the bulk endpoints while the
    /// instrument otherwise plays normally and still answers on endpoint 0 — and in that
    /// state [`Self::write`] blocks forever, so a read timeout is never reached and the
    /// caller hangs with no way to report why.
    ///
    /// Default is no timeout, for the same reason as [`Self::read_timeout`]: honoring one
    /// means cancelling a submitted transfer, which only a backend can do.
    async fn write_timeout(&mut self, buf: &[u8], _limit: std::time::Duration) -> Result<bool> {
        self.write(buf).await.map(|()| true)
    }
}

/// Opt-in marker for desktop callers that need to move a transport across threads.
/// Deliberately *not* a supertrait of [`Transport`] — see the note there.
pub trait SendTransport: Transport + Send {}
impl<T: Transport + Send> SendTransport for T {}
