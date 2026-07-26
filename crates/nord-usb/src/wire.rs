//! The vendor wire protocol.
//!
//! Every message on the vendor bulk endpoints is a length-prefixed, CRC-trailered
//! frame of **big-endian** `u32`s. Note big-endian — the *file* formats
//! ([`nord_format`]) are little-endian, and mixing them up is an easy afternoon lost.
//!
//! ```text
//! ┌────────┬─────────┬───────────┬─────────┬───────────────┬───────┐
//! │ length │ service │ subsystem │ command │ args…         │ crc16 │
//! │  u32   │   u32   │    u32    │   u32   │               │  u16  │
//! └────────┴─────────┴───────────┴─────────┴───────────────┴───────┘
//!   total inc. crc                          responses lead   over all
//!                                           with u32 status  preceding bytes
//! ```
//!
//! Verified against **every** message in the specimen corpus: 4,589 messages,
//! 100% CRC match and 100% length-field match.
//!
//! Requests carry an even `command`; the matching response is `command + 1` and
//! inserts a `u32` status (0 = success) ahead of the echoed arguments. That inserted
//! word is the reason responses run exactly 4 bytes longer than their requests.

use crate::error::{Error, Result};

/// Bytes ahead of the argument region: length, service, subsystem, command.
pub const HEADER_LEN: usize = 16;
/// Trailing CRC-16.
pub const CRC_LEN: usize = 2;

/// Functional area the message is addressed to.
///
/// Only two are observed so far. `Ui` carries the human-readable progress strings
/// NSM displays (`"Deleting..."`, `"Uploading..."`); `Program` is where the actual
/// work happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Session control and UI progress strings. Pairs with subsystem `1`.
    Ui,
    /// Program/slot operations. Pairs with subsystem `10`.
    Program,
    Unknown(u32),
}

impl Service {
    pub fn from_raw(v: u32) -> Self {
        match v {
            6 => Service::Ui,
            12 => Service::Program,
            other => Service::Unknown(other),
        }
    }

    pub fn to_raw(self) -> u32 {
        match self {
            Service::Ui => 6,
            Service::Program => 12,
            Service::Unknown(v) => v,
        }
    }
}

/// Command codes observed on [`Service::Program`] (subsystem 10).
///
/// Requests are even and the response is always `+1`, so only the request code is
/// named. Codes are what the device actually sent — not guesses.
pub mod cmd {
    /// Open the transaction (the `O22 I26` that starts every operation).
    pub const SESSION_OPEN: u32 = 0x04;
    /// Close the transaction.
    pub const SESSION_CLOSE: u32 = 0x06;
    /// Device/memory status; the response carries several counters.
    pub const STATUS: u32 = 0x08;
    /// Delete a program.
    pub const DELETE: u32 = 0x14;
    /// Read a program's data. Response body is a reframed entity.
    pub const READ: u32 = 0x12;
    /// Move a program between slots.
    pub const MOVE: u32 = 0x18;
    /// Read a program's metadata (name, format tag).
    pub const INFO: u32 = 0x1e;
    /// Rename a program; args carry a length-prefixed string.
    pub const RENAME: u32 = 0x1c;
}

/// CRC-16/CCITT-FALSE — poly `0x1021`, init `0xFFFF`, no reflection, no xorout.
///
/// Identified by brute-forcing the standard CRC-16 parameter space against known
/// message/trailer pairs, then confirmed against all 4,589 corpus messages.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// One protocol message, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub service: Service,
    pub subsystem: u32,
    pub command: u32,
    /// Everything between the command word and the CRC. For a response this still
    /// includes the leading status word — use [`Message::status`] to read it.
    pub args: Vec<u8>,
}

impl Message {
    pub fn new(service: Service, subsystem: u32, command: u32, args: Vec<u8>) -> Self {
        Self { service, subsystem, command, args }
    }

    /// Responses carry an odd command code, one above the request they answer.
    pub fn is_response(&self) -> bool {
        self.command & 1 == 1
    }

    /// The status word a response leads with. `Some(0)` is success.
    pub fn status(&self) -> Option<u32> {
        if !self.is_response() || self.args.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes(self.args[..4].try_into().ok()?))
    }

    /// Arguments with the response status word stripped, so request and response
    /// argument lists line up.
    pub fn payload(&self) -> &[u8] {
        if self.is_response() && self.args.len() >= 4 {
            &self.args[4..]
        } else {
            &self.args
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let len = (HEADER_LEN + self.args.len() + CRC_LEN) as u32;
        let mut out = Vec::with_capacity(len as usize);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&self.service.to_raw().to_be_bytes());
        out.extend_from_slice(&self.subsystem.to_be_bytes());
        out.extend_from_slice(&self.command.to_be_bytes());
        out.extend_from_slice(&self.args);
        out.extend_from_slice(&crc16(&out).to_be_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_LEN + CRC_LEN {
            return Err(Error::Truncated { got: buf.len(), need: HEADER_LEN + CRC_LEN });
        }
        let declared = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
        if declared != buf.len() {
            return Err(Error::LengthMismatch { declared, actual: buf.len() });
        }

        let split = buf.len() - CRC_LEN;
        let expected = u16::from_be_bytes(buf[split..].try_into().unwrap());
        let actual = crc16(&buf[..split]);
        if expected != actual {
            return Err(Error::BadCrc { expected, actual });
        }

        Ok(Self {
            service: Service::from_raw(u32::from_be_bytes(buf[4..8].try_into().unwrap())),
            subsystem: u32::from_be_bytes(buf[8..12].try_into().unwrap()),
            command: u32::from_be_bytes(buf[12..16].try_into().unwrap()),
            args: buf[HEADER_LEN..split].to_vec(),
        })
    }
}

/// A bank/slot address. **Zero-indexed on the wire**, one-indexed in the UI and in
/// every capture directory name — `move_prog_8-13_to_7-16` puts `7, 12, 6, 15` on
/// the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub bank: u32,
    pub slot: u32,
}

impl Location {
    /// From the one-indexed numbering used by the UI and capture names.
    pub fn from_user(bank: u32, slot: u32) -> Self {
        Self { bank: bank - 1, slot: slot - 1 }
    }

    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.bank.to_be_bytes());
        out.extend_from_slice(&self.slot.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The middle exchange of `move_prog_8-13_to_7-16`, byte-for-byte off the wire.
    const MOVE: &str = "000000220000000c0000000a00000018000000070000000c000000060000000f4a55";
    /// Its response: command +1, status word inserted, arguments echoed.
    const MOVE_RESP: &str =
        "000000260000000c0000000a0000001900000000000000070000000c000000060000000f7197";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn decodes_a_real_move() {
        let m = Message::decode(&hex(MOVE)).unwrap();
        assert_eq!(m.service, Service::Program);
        assert_eq!(m.subsystem, 10);
        assert_eq!(m.command, cmd::MOVE);
        assert!(!m.is_response());

        // 8-13 -> 7-16, zero-indexed on the wire.
        let mut want = Vec::new();
        Location::from_user(8, 13).write_to(&mut want);
        Location::from_user(7, 16).write_to(&mut want);
        assert_eq!(m.payload(), want.as_slice());
    }

    #[test]
    fn response_is_request_plus_one_plus_status() {
        let req = Message::decode(&hex(MOVE)).unwrap();
        let resp = Message::decode(&hex(MOVE_RESP)).unwrap();

        assert_eq!(resp.command, req.command + 1);
        assert!(resp.is_response());
        assert_eq!(resp.status(), Some(0));
        // Once the status word is stripped, the arguments are identical...
        assert_eq!(resp.payload(), req.payload());
        // ...which is exactly why responses run 4 bytes longer.
        assert_eq!(hex(MOVE_RESP).len() - hex(MOVE).len(), 4);
    }

    #[test]
    fn round_trips_byte_exact() {
        for raw in [MOVE, MOVE_RESP] {
            let bytes = hex(raw);
            assert_eq!(Message::decode(&bytes).unwrap().encode(), bytes);
        }
    }

    #[test]
    fn rejects_a_corrupted_crc() {
        let mut bytes = hex(MOVE);
        *bytes.last_mut().unwrap() ^= 0xFF;
        assert!(matches!(Message::decode(&bytes), Err(Error::BadCrc { .. })));
    }

    #[test]
    fn crc_matches_known_messages() {
        // Session open/close and the UI hello, straight from the corpus.
        for raw in [
            "0000001200000006000000010000000006a1",
            "000000160000000c0000000a0000000400000004a218",
            "000000120000000c0000000a000000066500",
        ] {
            assert!(Message::decode(&hex(raw)).is_ok(), "{raw}");
        }
    }
}
