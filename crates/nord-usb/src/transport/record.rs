//! A recording tap over a live transport: every frame in either direction is appended
//! to a script file in the format [`crate::transport::replay`] reads back.
//!
//! Frames are written whole and in wire order, because a golden replay is only worth
//! having if it is byte-exact. A bulk body read therefore contributes one line per
//! chunk, so recording a large object yields a large script.
//!
//! Writes are unbuffered: a session that wedges or is killed still leaves everything
//! that reached the wire on disk, which is the case the recording usually exists for.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::error::Result;

/// Appends directed frames to a replay script.
///
/// An I/O failure part-way through is held rather than raised: aborting a live session
/// mid-transaction leaves the instrument with an open session, which is worse than a
/// short script. Call [`Recorder::check`] once the operation is done to surface it.
pub struct Recorder {
    file: File,
    failed: Option<std::io::Error>,
}

impl Recorder {
    /// Create `path`, truncating it, and write the script header.
    ///
    /// The header says where the frames came from, which is what a reader needs to know
    /// whether they are an oracle: `source: nord` is this project's own traffic, a
    /// regression baseline rather than a match against the vendor application.
    ///
    /// Fails immediately if the path is not writable — the point at which a caller can
    /// still do something about it.
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
    /// One command opens several transactions — a move names both slots before moving
    /// anything — so this is written per transaction, not per file, and each one opens a
    /// section the replay sweep drives on its own.
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

    /// Write a free-form comment line, to mark what the following frames belong to.
    pub fn comment(&mut self, text: &str) {
        if self.failed.is_some() {
            return;
        }
        if let Err(e) = writeln!(self.file, "\n# {text}") {
            self.failed = Some(e);
        }
    }

    /// The first I/O error the recorder hit, if any. Recording stops at that point.
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
