//! The bottom of the stack: moving bytes to and from the device.
//!
//! Everything above this trait is pure logic, so the whole protocol can be built and
//! tested against committed captures with no hardware attached.

use crate::error::Result;

#[cfg(feature = "nusb")]
pub mod usb;
#[cfg(feature = "nusb")]
pub use usb::UsbTransport;

// ⚠️ `web-sys` emits WebUSB only under the unstable cfg supplied for wasm targets.
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub mod web;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub use web::WebUsbTransport;

// Same gate as the desktop backend: it taps that transport and needs a filesystem.
#[cfg(feature = "nusb")]
pub mod record;
#[cfg(feature = "nusb")]
pub use record::Recorder;

#[cfg(feature = "replay")]
pub mod replay;
#[cfg(feature = "replay")]
pub use replay::{
    Direction, ErrKind, Expect, Header, ReplayTransport, Script, Section, Source, Step,
};

/// Clavia DMI AB. Read off the device descriptor in a firmware-update capture.
pub const VENDOR_ID: u16 = 0x0ffc;
/// Nord Electro 5.
pub const PRODUCT_ID_ELECTRO5: u16 = 0x0027;

/// USB vendor-specific interface class, which carries the protocol. The instrument's
/// other interface is USB-MIDI (audio class). Every backend must leave it alone so
/// CoreMIDI and ALSA keep working, and the browser refuses to claim it.
pub const CLASS_VENDOR_SPECIFIC: u8 = 0xff;

/// Vendor bulk IN endpoint (device → host). The same in every corpus capture.
pub const EP_IN: u8 = 0x82;
/// Vendor bulk OUT endpoint (host → device).
pub const EP_OUT: u8 = 0x03;

/// The read buffer Nord Sound Manager posts. The device answers in chunks of about
/// 32 KB. The device chooses that size, not USB: the link is Full Speed with 64-byte
/// packets.
pub const READ_BUFFER: usize = 49152;

/// Whether a frame of `written` bytes leaves the device waiting for the rest of a
/// message that has already finished.
///
/// Confirmed on hardware.
///
/// ⚠️ The firmware reads a message until a short packet ends it, so a frame that is a
/// whole number of packets is never answered and the session stays open. A `RENAME`
/// carrying a 34-character name is 64 bytes on the full-speed link and gets no reply;
/// with 33 characters the same command is answered. It repeats at 128 bytes and does
/// not depend on the command or class: `BEGIN_WRITE` at 64 bytes hangs the same way on
/// Live and Settings.
///
/// A zero-length frame needs no terminator: it is one.
#[cfg(any(feature = "nusb", all(feature = "web", target_arch = "wasm32"), test))]
pub(crate) fn needs_terminator(written: usize, packet: usize) -> bool {
    written != 0 && written.is_multiple_of(packet)
}

/// A bidirectional byte pipe to the device.
///
/// # Design
///
/// **No `Send` bounds.** WASM is single-threaded and `web-sys` types are `!Send`.
/// Requiring `Send` futures, as `#[async_trait]` does by default and `tokio::spawn`
/// demands, would rule out the WebUSB backend and spread to every generic bound above
/// this one. The `async_fn_in_trait` lint is allowed because callers cannot add a
/// `Send` bound here. Desktop callers that need `Send` should bound on their own marker.
///
/// **Separate directions.** Several operations send more than one OUT before any IN
/// (`delete` is `O36 O26 I30`), so there is no request/response primitive.
///
/// **Owned buffers.** WebUSB returns an `ArrayBuffer`, so a borrowed `&[u8]` cannot be
/// returned.
///
/// **No timeout parameter on `read` and `write`.** WebUSB has no native transfer
/// timeout, so callers wrap.
#[allow(async_fn_in_trait)]
pub trait Transport {
    /// Write one message to the OUT endpoint.
    ///
    /// ⚠️ The device reads a message until a short packet ends it, so a backend that
    /// moves real packets must terminate a frame whose length is a whole multiple of the
    /// endpoint's `wMaxPacketSize`. An unterminated frame does not fail: the device
    /// never answers, and the session stays open.
    async fn write(&mut self, buf: &[u8]) -> Result<()>;

    /// Read one complete device message from the IN endpoint.
    ///
    /// `max` is the transfer buffer size. Callers must choose it large enough for the
    /// expected message; a message is never assembled across multiple calls.
    async fn read(&mut self, max: usize) -> Result<Vec<u8>>;

    /// Read, giving up after `limit`. `Ok(None)` means nothing arrived in time.
    ///
    /// For probing commands that may not exist: a device that does not recognize one
    /// may answer with an error status or say nothing, and [`Self::read`] would wait
    /// forever in the second case. Killing the hung process instead leaves the
    /// transaction open, which wedges the instrument until it is power-cycled.
    ///
    /// The default implementation has no timeout. It defers to [`Self::read`] and can
    /// only return `Ok(Some(_))`. Honoring the limit requires canceling a transfer
    /// already submitted to the OS, which only a backend can do. A backend that cannot
    /// must not pretend to, because abandoning a submitted read pairs every later
    /// request with the wrong response.
    async fn read_timeout(
        &mut self,
        max: usize,
        _limit: std::time::Duration,
    ) -> Result<Option<Vec<u8>>> {
        self.read(max).await.map(Some)
    }

    /// Write, giving up after `limit`. `Ok(false)` means the device never accepted it.
    ///
    /// A device can stop accepting writes without stopping altogether. A frame it cannot
    /// handle has been observed to stall the bulk endpoints while the instrument still
    /// plays and answers on endpoint 0. In that state [`Self::write`] blocks forever, a
    /// read timeout is never reached, and the caller hangs with no way to report why.
    ///
    /// The default has no timeout, for the same reason as [`Self::read_timeout`].
    async fn write_timeout(&mut self, buf: &[u8], _limit: std::time::Duration) -> Result<bool> {
        self.write(buf).await.map(|()| true)
    }
}

#[cfg(test)]
mod tests {
    use super::needs_terminator;

    #[test]
    fn a_frame_that_fills_whole_packets_needs_terminating() {
        const FULL_SPEED: usize = 64;
        for answered in [1, 33, 63, 65, 127, 129] {
            assert!(!needs_terminator(answered, FULL_SPEED), "{answered}");
        }
        for stranded in [64, 128, 192, 32_768] {
            assert!(needs_terminator(stranded, FULL_SPEED), "{stranded}");
        }
    }

    #[test]
    fn the_boundary_follows_the_endpoints_packet_size() {
        assert!(needs_terminator(512, 512));
        assert!(!needs_terminator(64, 512));
        assert!(!needs_terminator(576, 512));
    }

    #[test]
    fn an_empty_write_is_its_own_terminator() {
        assert!(!needs_terminator(0, 64));
        assert!(!needs_terminator(0, 512));
    }
}
