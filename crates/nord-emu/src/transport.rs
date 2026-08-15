//! A [`Transport`] backed by [`EmuDevice`] instead of a cable.

use std::time::Duration;

use nord_usb::error::{Error, Result};
use nord_usb::transport::Transport;

use crate::EmuDevice;

/// Which side of the wire a logged frame came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Host,
    Device,
}

/// One frame, as it crossed.
#[derive(Debug, Clone)]
pub struct Frame {
    pub side: Side,
    pub bytes: Vec<u8>,
}

/// The byte pipe to an [`EmuDevice`].
///
/// Every frame in either direction is logged, so a test can compare the whole exchange
/// against a capture — the host's messages the way [`nord_usb::ReplayTransport`] does,
/// and the device's answers too, which a replay cannot check because it supplies them.
pub struct EmuTransport {
    device: EmuDevice,
    log: Vec<Frame>,
}

impl EmuTransport {
    pub fn new(device: EmuDevice) -> Self {
        Self {
            device,
            log: Vec::new(),
        }
    }

    pub fn device(&self) -> &EmuDevice {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut EmuDevice {
        &mut self.device
    }

    pub fn into_device(self) -> EmuDevice {
        self.device
    }

    /// Every frame that crossed, in order.
    pub fn log(&self) -> &[Frame] {
        &self.log
    }

    /// What the host transmitted, in order.
    pub fn sent(&self) -> Vec<&[u8]> {
        self.side(Side::Host)
    }

    /// What the device answered, in order.
    pub fn received(&self) -> Vec<&[u8]> {
        self.side(Side::Device)
    }

    fn side(&self, side: Side) -> Vec<&[u8]> {
        self.log
            .iter()
            .filter(|f| f.side == side)
            .map(|f| f.bytes.as_slice())
            .collect()
    }
}

impl From<EmuDevice> for EmuTransport {
    fn from(device: EmuDevice) -> Self {
        Self::new(device)
    }
}

impl Default for EmuTransport {
    fn default() -> Self {
        Self::new(EmuDevice::default())
    }
}

impl Transport for EmuTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        if self.device.endpoints_stalled() {
            // ⚠️ Not an error on hardware — the write simply never completes. A caller
            // that wants that shape uses `write_timeout`, which is the half of the trait
            // built for it; a plain `write` cannot block forever in one process.
            return Err(Error::Transport(
                "the device's bulk endpoints are stalled and accept nothing".into(),
            ));
        }
        self.log.push(Frame {
            side: Side::Host,
            bytes: buf.to_vec(),
        });
        self.device.receive(buf)
    }

    async fn read(&mut self, _max: usize) -> Result<Vec<u8>> {
        match self.device.take_response() {
            Some(bytes) => {
                self.log.push(Frame {
                    side: Side::Device,
                    bytes: bytes.clone(),
                });
                Ok(bytes)
            }
            // A silent device would leave a real host waiting on the endpoint. Nothing
            // here can wait, so the caller is told rather than deadlocked — and a caller
            // that means to allow silence has `read_timeout`.
            None => Err(Error::Transport(
                "the device said nothing, and this read has no limit to give up at".into(),
            )),
        }
    }

    /// `Ok(None)` when the device has nothing to say.
    ///
    /// The limit is not waited out: the device model answers a frame the moment it
    /// receives one, so anything not already queued would never arrive.
    async fn read_timeout(&mut self, max: usize, _limit: Duration) -> Result<Option<Vec<u8>>> {
        match self.device.has_response() {
            true => self.read(max).await.map(Some),
            false => Ok(None),
        }
    }

    /// `Ok(false)` once the bulk endpoints are stalled — the state where a real device
    /// stops accepting writes while the instrument otherwise plays on.
    async fn write_timeout(&mut self, buf: &[u8], _limit: Duration) -> Result<bool> {
        match self.device.endpoints_stalled() {
            true => Ok(false),
            false => self.write(buf).await.map(|()| true),
        }
    }
}
