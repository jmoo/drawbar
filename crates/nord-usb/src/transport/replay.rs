//! A [`Transport`] that replays a recorded exchange instead of touching hardware.
//!
//! The whole protocol layer can then be exercised anywhere — including under Wine,
//! qemu and wasm — with no device attached. It also makes every operation assertable:
//! [`ReplayTransport::sent`] hands back exactly what the operation put on the wire, so
//! a test can compare that against a real capture.
//!
//! The script is a flat list of directed messages, in the order they occurred.
//! `Out` entries are what the *host* sent, and are checked against what the code under
//! test actually sends; `In` entries are fed back as device responses.

use super::Transport;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Host → device.
    Out,
    /// Device → host.
    In,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub direction: Direction,
    pub bytes: Vec<u8>,
}

/// How strictly to police what the code under test transmits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// Every `Out` must match the script byte-for-byte. Use in tests.
    Exact,
    /// Ignore what is sent and just serve the next `In`. Useful for demos against a
    /// capture whose addressing differs from what is being asked for.
    Lenient,
}

pub struct ReplayTransport {
    script: Vec<Step>,
    pos: usize,
    sent: Vec<Vec<u8>>,
    strictness: Strictness,
}

impl ReplayTransport {
    pub fn new(script: Vec<Step>) -> Self {
        Self { script, pos: 0, sent: Vec::new(), strictness: Strictness::Exact }
    }

    pub fn lenient(mut self) -> Self {
        self.strictness = Strictness::Lenient;
        self
    }

    /// Everything the code under test transmitted, in order.
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }

    /// Whether the whole script was consumed. A test that leaves steps unread has
    /// usually stopped short of the behaviour it meant to check.
    pub fn is_exhausted(&self) -> bool {
        self.pos >= self.script.len()
    }

    /// Parse the simple text format written by `corpus capture-messages`:
    /// one `O <hex>` or `I <hex>` per line; `#` comments and blanks ignored.
    pub fn from_script(text: &str) -> Result<Self> {
        let mut script = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (tag, hex) = line
                .split_once(char::is_whitespace)
                .ok_or_else(|| Error::Transport(format!("line {}: expected '<O|I> <hex>'", n + 1)))?;
            let direction = match tag {
                "O" | "o" => Direction::Out,
                "I" | "i" => Direction::In,
                other => {
                    return Err(Error::Transport(format!(
                        "line {}: unknown direction {other:?}, want O or I",
                        n + 1
                    )))
                }
            };
            let hex = hex.trim();
            if hex.len() % 2 != 0 {
                return Err(Error::Transport(format!("line {}: odd-length hex", n + 1)));
            }
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<std::result::Result<Vec<u8>, _>>()
                .map_err(|e| Error::Transport(format!("line {}: {e}", n + 1)))?;
            script.push(Step { direction, bytes });
        }
        Ok(Self::new(script))
    }
}

impl Transport for ReplayTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.sent.push(buf.to_vec());

        let step = self.script.get(self.pos).ok_or_else(|| {
            Error::Transport(format!("script exhausted; host sent an extra {} bytes", buf.len()))
        })?;
        if step.direction != Direction::Out {
            return Err(Error::Transport(
                "host wrote, but the script expects the device to speak next".into(),
            ));
        }
        if self.strictness == Strictness::Exact && step.bytes != buf {
            return Err(Error::Transport(format!(
                "sent bytes differ from the script at step {}\n  expected {}\n  got      {}",
                self.pos,
                hex(&step.bytes),
                hex(buf),
            )));
        }
        self.pos += 1;
        Ok(())
    }

    async fn read(&mut self, _max: usize) -> Result<Vec<u8>> {
        let step = self
            .script
            .get(self.pos)
            .ok_or_else(|| Error::Transport("script exhausted; host expected a response".into()))?;
        if step.direction != Direction::In {
            return Err(Error::Transport(
                "host read, but the script expects the host to speak next".into(),
            ));
        }
        self.pos += 1;
        Ok(step.bytes.clone())
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
