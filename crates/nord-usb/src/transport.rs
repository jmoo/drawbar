//! The bottom of the stack: moving bytes to and from the device.
//!
//! Everything above this trait is pure logic, so the whole protocol can be built and
//! tested against committed captures with no hardware attached — the same property
//! that makes [`nord_format`] trustworthy.

use crate::error::Result;

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
/// cannot be honoured.
///
/// **No timeout parameter.** WebUSB has no native transfer timeout — callers wrap.
#[allow(async_fn_in_trait)]
pub trait Transport {
    /// Write one message to the OUT endpoint.
    async fn write(&mut self, buf: &[u8]) -> Result<()>;

    /// Read up to `max` bytes from the IN endpoint.
    async fn read(&mut self, max: usize) -> Result<Vec<u8>>;
}

/// Opt-in marker for desktop callers that need to move a transport across threads.
/// Deliberately *not* a supertrait of [`Transport`] — see the note there.
pub trait SendTransport: Transport + Send {}
impl<T: Transport + Send> SendTransport for T {}
