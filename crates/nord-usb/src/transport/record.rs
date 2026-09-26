//! A recording tap over a live transport: every frame in either direction is appended
//! to a script file in the format [`crate::transport::replay`] reads back.
//!
//! Frames are written in wire order, byte for byte. Each bulk read adds one line.
//!
//! Writes are unbuffered, so a session that wedges or is killed still leaves everything
//! that reached the wire on disk. That is usually the case a recording is for.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::error::Result;

/// Appends directed frames to a replay script.
///
/// An I/O failure partway through is held, not raised, because aborting mid-transaction
/// leaves the instrument with an open session, which is worse than a short script.
/// [`Recorder::check`], reached through
/// [`UsbTransport::finish_recording`](super::UsbTransport::finish_recording), reports it
/// once the operation is done.
pub struct Recorder {
    file: File,
    failed: Option<std::io::Error>,
}

impl Recorder {
    /// Create `path`, truncating it, and write the script header.
    ///
    /// The header says where the frames came from, so a reader knows whether they are
    /// an oracle: `source: nord` is this project's own traffic, a regression baseline
    /// and not evidence of what the vendor application sends.
    ///
    /// Fails immediately if the path is not writable, while the caller can still act on
    /// it.
    pub fn create(path: &Path, device: Option<&str>) -> Result<Self> {
        let mut file = File::create(path)?;
        writeln!(
            file,
            "# nord-usb replay script, recorded from hardware.\n\
             # Format: '<O|I> <hex>' -- O = host->device, I = device->host.\n\
             # source: nord"
        )?;
        if let Some(device) = device {
            writeln!(file, "# device: {device}")?;
        }
        Ok(Self { file, failed: None })
    }

    /// Declare what the frames that follow are doing: `<class> <verb> <args…>`, in the
    /// CLI's own spellings.
    ///
    /// One command can open several transactions (a move describes both slots before
    /// moving anything), so this is written per transaction. Each one opens a section
    /// the replay sweep drives on its own.
    pub fn intent(&mut self, intent: &str) {
        if self.failed.is_some() {
            return;
        }
        if let Err(e) = writeln!(self.file, "\n# intent: {intent}") {
            self.failed = Some(e);
        }
    }

    /// Record a frame the host sent.
    pub fn out(&mut self, bytes: &[u8]) {
        self.line('O', bytes);
    }

    /// Record a frame the device sent.
    pub fn r#in(&mut self, bytes: &[u8]) {
        self.line('I', bytes);
    }

    /// Declare that the transaction just recorded failed, and how.
    ///
    /// Written after its frames, because the outcome is only known once the operation is
    /// over. A script that says nothing claims the operation succeeded.
    pub fn expect(&mut self, e: &crate::error::Error) {
        if self.failed.is_some() {
            return;
        }
        if let Err(io) = writeln!(self.file, "# expect: err {}", e.expect_kind()) {
            self.failed = Some(io);
        }
    }

    /// The first I/O error the recorder hit, if any. Recording pauses from that error
    /// until this is called.
    pub fn check(&mut self) -> Result<()> {
        match self.failed.take() {
            Some(e) => Err(e.into()),
            None => Ok(()),
        }
    }

    fn line(&mut self, tag: char, bytes: &[u8]) {
        if self.failed.is_some() {
            return;
        }
        let mut hex = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            hex.push_str(&format!("{b:02x}"));
        }
        if let Err(e) = writeln!(self.file, "{tag} {hex}") {
            self.failed = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script missing frames replays as a different exchange.
    #[test]
    fn a_frame_that_could_not_be_written_is_reported_by_the_check() {
        let path = std::env::temp_dir().join(format!("nord-record-{}.script", std::process::id()));
        File::create(&path).expect("the script path is writable");
        let unwritable = File::open(&path).expect("reopening it read-only");
        let mut recorder = Recorder {
            file: unwritable,
            failed: None,
        };

        recorder.out(&[0x00, 0x11]);
        let err = recorder
            .check()
            .expect_err("the frame never reached the script");

        assert!(matches!(err, crate::error::Error::Io(_)), "{err}");
        assert!(
            recorder.check().is_ok(),
            "a reported failure is not reported twice"
        );
        std::fs::remove_file(&path).ok();
    }
}
