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
///
/// Sized past that chunk so a bulk IN ends on the device's short packet rather than on a
/// full buffer, which is what makes one read one whole frame — see [`Transport::read`].
/// It is a whole number of 64-byte packets, so it is never itself a terminator.
pub const READ_BUFFER: usize = 49152;

/// Whether a frame of `written` bytes leaves the device waiting for the rest of a
/// message that has already finished.
///
/// Confirmed on hardware.
///
/// ⚠️ The firmware reads a message until a **short** packet ends it, so a frame that is
/// a whole number of packets is never answered and the session stays open. A `RENAME`
/// carrying a 34-character name is exactly 64 bytes on the full-speed link and gets no
/// reply; at 33 characters, one byte shorter, the same command answers normally. It
/// repeats at 128, and it is not particular to a command or a class — `BEGIN_WRITE` at
/// 64 bytes hangs the same way on Live and Settings.
///
/// A zero-length frame needs no terminator: it is one.
pub(crate) fn needs_terminator(written: usize, packet: usize) -> bool {
    written != 0 && written.is_multiple_of(packet)
}

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
    ///
    /// ⚠️ The device reads a message until a **short** packet ends it, so a backend
    /// that moves real packets must terminate a frame whose length is a whole multiple
    /// of the endpoint's `wMaxPacketSize`. Leaving one unterminated does not fail: the
    /// device simply never answers, and the session stays open.
    async fn write(&mut self, buf: &[u8]) -> Result<()>;

    /// Read **one whole device message** from the IN endpoint, at most `max` bytes long.
    ///
    /// `max` is a ceiling on the frame, not a slice length: one call answers with exactly
    /// one frame — never a piece of one, never two run together — and an implementor that
    /// cannot promise that must return an error rather than a fragment. Callers hold no
    /// residual buffer and never read twice for one frame, so there is nowhere for a
    /// leftover byte to go. [`crate::Message::decode`] compares the frame's own length
    /// word against the buffer it was handed, which makes a split frame and a coalesced
    /// pair both [`crate::Error::LengthMismatch`] — a hard failure, never a signal to
    /// read again.
    ///
    /// What holds this up on a real bus is the short-packet rule [`Self::write`] states
    /// for the other direction. The device ends a message with a short packet, so a
    /// transfer of at least the message's length ends where the message does; [`READ_BUFFER`]
    /// is sized past the largest chunk the device sends for exactly that reason. Turn-taking
    /// supplies the other half: the device answers a request and then waits, so there is
    /// never a second frame queued behind the first.
    ///
    /// ⚠️ A message whose length is a whole number of packets would end in no short
    /// packet, and the transfer would sit waiting for a terminator the device never
    /// sends. That is a hang, not a half-read frame — the contract holds or nothing
    /// comes back — but it is the one shape that would break it, and no frame this
    /// crate has seen from the device is shaped that way.
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
