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
//! The response to a request is `command + 1` and inserts a `u32` status (0 = success)
//! ahead of the echoed arguments. That inserted word is the reason responses run
//! exactly 4 bytes longer than their requests.
//!
//! Requests are *usually* even, but that is a pattern and not a rule — [`cmd::SELECT`]
//! is `0x2f`, an odd request whose response is `0x30`. **Direction is the only reliable
//! discriminator**, which is why this module records it at decode time rather than
//! deriving it (see [`Message::decode_response`]).

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
/// The response is always the request `+1`, so only the request code is named. Codes are
/// what the device actually sent — not guesses. Most requests happen to be even;
/// [`SELECT`] is the counter-example, so do not treat parity as meaning anything.
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
    /// Copy/duplicate an object: `src_bank, src_slot, dst_bank, dst_slot`. The device
    /// copies internally — no body crosses the wire.
    pub const COPY: u32 = 0x16;
    /// Move a program between slots.
    pub const MOVE: u32 = 0x18;
    /// Read a program's metadata (name, format tag).
    pub const INFO: u32 = 0x1e;
    /// Rename a program; args carry a length-prefixed string.
    pub const RENAME: u32 = 0x1c;
    /// Select an object live on the instrument ("open on device" / double-click).
    /// Non-destructive: nothing stored changes, the device just loads it. This is the
    /// one request with inverted parity — odd code, even response (`0x30`) — so its
    /// direction cannot be inferred from the command number.
    pub const SELECT: u32 = 0x2f;
    /// Re-link an object's dependency table ("set slot table"). Rewrites which library
    /// pianos/samples a program points at, or which programs a set list holds. Its
    /// payload semantics (notably the per-entry flag byte) are not fully pinned down,
    /// so no typed operation is built on it yet — the code is named for completeness.
    pub const RELINK: u32 = 0x35;

    /// Begin writing an entity. Args: bank, slot, body length, format tag,
    /// timestamp, `0xFFFFFFFF`, 1, and a trailing flag byte.
    pub const BEGIN_WRITE: u32 = 0x0a;
    /// Begin reading an entity. Args: bank, slot.
    pub const BEGIN_READ: u32 = 0x0c;
    /// Finish a transfer, either direction. Args: bank, slot.
    pub const END_TRANSFER: u32 = 0x0e;
    /// Push entity bytes. Args: bank, slot, offset, length, then the body.
    pub const WRITE_DATA: u32 = 0x10;
    /// List an entity's piano/sample dependencies.
    pub const DEPENDENCIES: u32 = 0x28;
}

/// The UI/session service (service 6, subsystem 1): the transaction's outer handshake
/// and the progress strings NSM paints on the **instrument's own display** during a
/// transfer.
///
/// The progress messages ([`label`], [`percent`]) are **fire-and-forget** — the device
/// never replies. They must be sent with [`crate::Session::notify`], never `request`,
/// which would block forever waiting for a response that never comes. (An earlier
/// version stopped sending them on the belief they were malformed; that came from a bad
/// hand transcription and was wrong — every example on the wire is well-formed.)
pub mod ui {
    use super::{Message, Service};
    use crate::error::{Error, Result};

    /// Subsystem paired with [`Service::Ui`].
    pub const SUBSYSTEM: u32 = 1;
    /// Open the UI side of a transaction (the `O18 I22` that starts every operation).
    pub const HELLO: u32 = 0x00;
    /// Close the UI side of a transaction.
    pub const GOODBYE: u32 = 0x02;
    /// A text progress label, e.g. `"Downloading..."`.
    pub const LABEL: u32 = 0x06;
    /// A progress percentage, 0..=100.
    pub const PERCENT: u32 = 0x07;

    /// The longest label the one-byte length field can describe.
    pub const MAX_LABEL_LEN: usize = u8::MAX as usize;

    /// A progress label. Layout is six zero bytes, a one-byte length, then unpadded
    /// ASCII — read straight off the wire and byte-for-byte reproducible.
    ///
    /// Fails for a label longer than [`MAX_LABEL_LEN`] **bytes** rather than truncating
    /// the length into a `u8`: a 256-byte label would silently encode a length of `0`
    /// and put a malformed frame on the wire. Malformed progress frames are exactly
    /// what sent this crate down a wrong path once already, so they are refused rather
    /// than emitted. Note the bound is on UTF-8 bytes, not characters.
    pub fn label(text: &str) -> Result<Message> {
        if text.len() > MAX_LABEL_LEN {
            return Err(Error::InvalidArgument(format!(
                "progress label is {} bytes; the length field holds at most {MAX_LABEL_LEN}",
                text.len(),
            )));
        }
        let mut args = vec![0u8; 6];
        args.push(text.len() as u8);
        args.extend_from_slice(text.as_bytes());
        Ok(Message::new(Service::Ui, SUBSYSTEM, LABEL, args))
    }

    /// A progress percentage. Layout is a constant `u16` 1 then the value as a `u16`.
    ///
    /// Clamped to 100. Unlike [`label`] an out-of-range value cannot produce a
    /// malformed frame — every `u16` encodes fine — so this is a cosmetic nonsense
    /// value on the instrument's display, not a protocol error, and clamping beats
    /// making every call site handle a `Result`.
    pub fn percent(pct: u16) -> Message {
        let mut args = 1u16.to_be_bytes().to_vec();
        args.extend_from_slice(&pct.min(100).to_be_bytes());
        Message::new(Service::Ui, SUBSYSTEM, PERCENT, args)
    }
}

/// What [`cmd::INFO`] reports about one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramInfo {
    pub location: Location,
    /// Length of the entity body on the wire — 121 for an Electro 5 program.
    pub body_len: u32,
    /// Four-character CBIN format tag, e.g. `ne5p`.
    pub format: String,
    /// Schema/content version, the same field the CBIN header carries at `0x14` and
    /// the one NSM prints in its "Version" column.
    ///
    /// Per format tag, not a per-item counter: `ne5p` reports 4 and `ne5t` reports 0 or
    /// 1. For library content it is the version in the object's own *name*, ×100 —
    /// `Royal Grand 3D YaS6 XL 5.4` reports `540`.
    pub version: u32,
    /// CRC-32 of the body, as the device reports it. Lets a read be verified
    /// against the device's own checksum rather than trusting the transfer.
    ///
    /// `None` for classes the device does not checksum — pianos and samples report
    /// `0xffffffff` rather than a real value, which is normalised away here so callers
    /// cannot mistake it for a checksum to verify against.
    pub crc32: Option<u32>,
    /// Slot name as shown on the instrument. Stored nowhere in the file itself.
    pub name: String,
}

impl ProgramInfo {
    /// Fixed offsets ahead of the name: bank, slot, body_len, format, version, and the
    /// two `0xffffffff` words, then the name's own length.
    const NAME_LEN_AT: usize = 28;

    pub fn decode(msg: &Message) -> Result<Self> {
        // See `Dependency::decode_all` — `payload` only strips the status word for a
        // message marked as a response, so decoding a request here would shift every
        // offset by four.
        if !msg.is_response() {
            return Err(Error::InvalidArgument(
                "object info must be decoded from a response (use Message::decode_response)".into(),
            ));
        }
        let p = msg.payload();
        if p.len() < Self::NAME_LEN_AT + 4 {
            return Err(Error::Truncated { got: p.len(), need: Self::NAME_LEN_AT + 4 });
        }
        let word = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().unwrap());

        // The layout is fixed, confirmed across ten replies spanning all four format
        // tags (ne5p, ne5t, npno, nsmp) in the set-list bundle capture:
        //
        //   bank | slot | body_len | format[4] | version | word | word
        //        | name_len | name | pad to 8 | crc32
        //
        // The two words at 20 and 24 are `0xffffffff` for the slot-addressed classes
        // but carry content-specific values for library objects, so they are read as
        // opaque and skipped rather than asserted.
        //
        // An earlier version scanned forward for the first plausible length word
        // instead, because only one example was available. That scan was bounded at
        // `n <= 32` — and this capture contains a 54-character sample name
        // ("3 Violins SM_Chamberlin_MMaster mono small version 2.0"), which it would
        // have skipped straight past, returning the wrong name or none at all.
        let name_len = word(Self::NAME_LEN_AT) as usize;
        let name_start = Self::NAME_LEN_AT + 4;
        let name_end = name_start + name_len;
        if name_end > p.len() {
            return Err(Error::Truncated { got: p.len(), need: name_end });
        }
        let name = String::from_utf8_lossy(&p[name_start..name_end]).trim_end().to_owned();

        // Trailing word, past the padding. Absent if the reply stops at the name.
        let crc32 = match p.len() >= name_end + 4 {
            true => match word(p.len() - 4) {
                u32::MAX => None,
                crc => Some(crc),
            },
            false => None,
        };

        Ok(Self {
            location: Location { bank: word(0), slot: word(4) },
            body_len: word(8),
            format: String::from_utf8_lossy(&p[12..16]).into_owned(),
            version: word(16),
            crc32,
            name,
        })
    }
}

/// One entry from a [`cmd::DEPENDENCIES`] response: a piano or sample that a program
/// (or a program that a set list) references.
///
/// The library `id` is the same id the object carries in its own file — a
/// `PianoPanel`'s piano id, a sample's sample id — so this is the bridge between the
/// content on the wire and the bytes on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// Leading byte, per-id consistent but not yet understood — ruled out as
    /// "present", reference count, class and category. Preserved verbatim.
    pub flag: u8,
    /// What kind of object this dependency is (piano, sample, program).
    pub class: ObjectClass,
    /// Content id, matching the id in the object's own file header.
    pub id: u32,
    /// Human-readable name — which the `.ne5p`/`.ne5t` files do not themselves store.
    pub name: String,
    /// Slot address, for slot-addressed dependencies (programs). Library content
    /// (pianos, samples) is addressed by `id` and reports no location.
    pub location: Option<Location>,
}

impl Dependency {
    /// Decode a whole [`cmd::DEPENDENCIES`] response into the list it carries.
    ///
    /// Layout after the leading `bank, slot, count`, each entry is
    /// `[u8 flag][u32 reserved][u32 class][u32 id][u32 name_len][name][u32 has_location][u32 bank][u32 slot]`
    /// with no alignment padding, so an entry is `29 + name_len` bytes.
    pub fn decode_all(msg: &Message) -> Result<Vec<Self>> {
        // [`Message::payload`] strips the leading status word only when the message is
        // marked as a response. Decoding a DEPENDENCIES reply with `Message::decode`
        // instead of `decode_response` would leave that word in place and shift every
        // offset below by four — the same silent four-byte misalignment that inferring
        // direction from command parity used to cause. Refuse rather than misparse.
        if !msg.is_response() {
            return Err(Error::InvalidArgument(
                "dependency list must be decoded from a response (use Message::decode_response)"
                    .into(),
            ));
        }
        let p = msg.payload();
        if p.len() < 12 {
            return Err(Error::Truncated { got: p.len(), need: 12 });
        }
        let word = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().unwrap());
        let count = word(8) as usize;

        let mut out = Vec::with_capacity(count);
        let mut i = 12;
        for _ in 0..count {
            // flag(1) + reserved(4) + class(4) + id(4) + name_len(4) = 17 bytes.
            if i + 17 > p.len() {
                return Err(Error::Truncated { got: p.len(), need: i + 17 });
            }
            let flag = p[i];
            let class = ObjectClass::from_raw(word(i + 5));
            let id = word(i + 9);
            let name_len = word(i + 13) as usize;
            let name_start = i + 17;
            let name_end = name_start + name_len;
            // name + has_location(4) + bank(4) + slot(4).
            if name_end + 12 > p.len() {
                return Err(Error::Truncated { got: p.len(), need: name_end + 12 });
            }
            let name = String::from_utf8_lossy(&p[name_start..name_end]).into_owned();
            let has_location = word(name_end) != 0;
            let location = has_location
                .then(|| Location { bank: word(name_end + 4), slot: word(name_end + 8) });
            out.push(Self { flag, class, id, name, location });
            i = name_end + 12;
        }
        Ok(out)
    }
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
    /// Set by the decoder from the direction the bytes travelled. Not inferable from
    /// the command code — see [`Message::is_response`].
    is_response: bool,
}

impl Message {
    /// A request, to send to the device.
    pub fn new(service: Service, subsystem: u32, command: u32, args: Vec<u8>) -> Self {
        Self { service, subsystem, command, args, is_response: false }
    }

    /// Whether this message was decoded as a device response.
    ///
    /// **Direction, not parity.** An earlier version inferred this from the command
    /// being odd, which held for every op decoded at the time and is wrong in general:
    /// the "select in instrument" command is `0x2f` (odd) with response `0x30` (even),
    /// exactly inverting the guess. The `response == request + 1` rule does hold — only
    /// the parity of the request does not. Getting this backwards silently misaligns
    /// [`Self::payload`] by four bytes and hides device errors, so it is now recorded
    /// at decode time by the side that knows.
    pub fn is_response(&self) -> bool {
        self.is_response
    }

    /// The status word a response leads with. `Some(0)` is success.
    pub fn status(&self) -> Option<u32> {
        if !self.is_response || self.args.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes(self.args[..4].try_into().ok()?))
    }

    /// Arguments with the response status word stripped, so request and response
    /// argument lists line up.
    pub fn payload(&self) -> &[u8] {
        if self.is_response && self.args.len() >= 4 {
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

    /// Decode bytes received *from* the device.
    pub fn decode_response(buf: &[u8]) -> Result<Self> {
        let mut m = Self::decode(buf)?;
        m.is_response = true;
        Ok(m)
    }

    /// Decode bytes without asserting a direction; treated as a request.
    /// Prefer [`Self::decode_response`] for anything read off the wire.
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
            is_response: false,
        })
    }
}

/// What kind of object a session is about.
///
/// `SESSION_OPEN` carries one of these, and [`cmd::STATUS`] then reports on that class
/// alone — which is why the same instrument reports different totals depending on what
/// was opened. Names are inferred from item counts cross-checked against a full backup
/// (29 pianos, 139 samples, ~375 programs); the numeric codes are what the device
/// actually sends, so an unrecognised one is preserved rather than rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectClass {
    Piano,
    Sample,
    Program,
    SetList,
    Unknown(u32),
}

impl ObjectClass {
    pub fn from_raw(v: u32) -> Self {
        match v {
            1 => ObjectClass::Piano,
            3 => ObjectClass::Sample,
            4 => ObjectClass::Program,
            5 => ObjectClass::SetList,
            other => ObjectClass::Unknown(other),
        }
    }

    pub fn to_raw(self) -> u32 {
        match self {
            ObjectClass::Piano => 1,
            ObjectClass::Sample => 3,
            ObjectClass::Program => 4,
            ObjectClass::SetList => 5,
            ObjectClass::Unknown(v) => v,
        }
    }

    /// The classes worth querying for an inventory. Codes 6 and 7 also answer, but
    /// reported zero items on every instrument seen so far.
    pub const INVENTORY: [ObjectClass; 4] =
        [ObjectClass::Piano, ObjectClass::Sample, ObjectClass::Program, ObjectClass::SetList];

    pub fn label(self) -> String {
        match self {
            ObjectClass::Piano => "pianos".into(),
            ObjectClass::Sample => "samples".into(),
            ObjectClass::Program => "programs".into(),
            ObjectClass::SetList => "set lists".into(),
            ObjectClass::Unknown(v) => format!("class {v}"),
        }
    }
}

/// What [`cmd::STATUS`] reports, for whichever [`ObjectClass`] the session opened.
///
/// `free + used` is constant per class (the class's total capacity). Deleting programs
/// was observed to raise `free` and lower `used` by the same amount, which is what
/// fixes the orientation — otherwise the two are indistinguishable.
///
/// The unit is not identified. It is *not* bytes: the program class totals 56,400,
/// which is far too small. Treat the numbers as opaque blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub class: ObjectClass,
    pub count: u32,
    pub free: u32,
    pub used: u32,
}

impl Status {
    pub fn total(&self) -> u32 {
        self.free + self.used
    }

    /// Blocks per item, when every item of this class costs the same.
    ///
    /// Fixed-size classes divide exactly: on a real Electro 5, programs cost **141**
    /// blocks each (`379 × 141 = 53439`, then `380 × 141 = 53580` after adding one)
    /// and set lists cost **38**. Variable-size classes — pianos and samples, where
    /// the content really does differ per item — do not divide evenly and yield
    /// `None`.
    pub fn blocks_per_item(&self) -> Option<u32> {
        if self.count == 0 || self.used == 0 || self.used % self.count != 0 {
            return None;
        }
        let per = self.used / self.count;
        // Only trust it if the class capacity is also a whole number of items;
        // otherwise the division is a coincidence.
        (per != 0 && self.total() % per == 0).then_some(per)
    }

    /// Total item slots, for classes where items are fixed-size.
    ///
    /// Far more meaningful than raw blocks: programs report 400, which is exactly the
    /// 8 banks × 50 slots of an Electro 5.
    pub fn slots(&self) -> Option<u32> {
        self.blocks_per_item().map(|per| self.total() / per)
    }

    pub fn used_percent(&self) -> f32 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            100.0 * self.used as f32 / total as f32
        }
    }

    /// Decode a [`cmd::STATUS`] response. Arguments after the status word are
    /// `count, free, used, …`.
    pub fn decode(class: ObjectClass, msg: &Message) -> Result<Self> {
        let p = msg.payload();
        if p.len() < 12 {
            return Err(Error::Truncated { got: p.len(), need: 12 });
        }
        let word = |i: usize| u32::from_be_bytes(p[i * 4..i * 4 + 4].try_into().unwrap());
        Ok(Self { class, count: word(0), free: word(1), used: word(2) })
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
        let resp = Message::decode_response(&hex(MOVE_RESP)).unwrap();

        assert_eq!(resp.command, req.command + 1);
        assert!(resp.is_response());
        assert_eq!(resp.status(), Some(0));
        // Once the status word is stripped, the arguments are identical...
        assert_eq!(resp.payload(), req.payload());
        // ...which is exactly why responses run 4 bytes longer.
        assert_eq!(hex(MOVE_RESP).len() - hex(MOVE).len(), 4);
    }

    /// Direction cannot be inferred from the command code.
    ///
    /// "Select in instrument" is `0x2f` -> `0x30`: an **odd** request with an **even**
    /// response, inverting the parity guess that held for every other decoded op. Both
    /// messages are real, from `select_setlist_1-2` (set lists) and
    /// `open_on_device_2-12` (programs) -- the same command at two object classes.
    #[test]
    fn direction_is_not_inferable_from_command_parity() {
        // Request: cmd 0x2f, args (0, 1) -- displayed set list 1:2.
        let req = Message::decode(&hex("0000001a0000000c0000000a0000002f00000000000000017f71")).unwrap();
        assert_eq!(req.command, 0x2f);
        assert!(req.command & 1 == 1, "this request really is odd-numbered");
        assert!(!req.is_response(), "an odd command must still decode as a request");
        assert_eq!(req.status(), None);
        // A request's payload must not have four bytes eaten as a status word.
        assert_eq!(req.payload().len(), 8);

        // Response: cmd 0x30 (even), status 0, then the echoed args.
        let resp = Message::decode_response(
            &hex("0000001e0000000c0000000a0000003000000000000000000000000112c4"),
        )
        .unwrap();
        assert_eq!(resp.command, req.command + 1);
        assert!(resp.command & 1 == 0, "this response really is even-numbered");
        assert!(resp.is_response());
        assert_eq!(resp.status(), Some(0), "status must be readable despite even command");
        assert_eq!(resp.payload(), req.payload(), "args line up once status is stripped");
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

    /// The progress strings encode byte-for-byte to what NSM put on the wire — the
    /// "Deleting..." label from `delete_prog_bank7_loc50` and the 100% bar from the
    /// program read. Reproducing these exactly is the whole point of un-retracting them.
    #[test]
    fn ui_label_and_percent_match_the_wire() {
        assert_eq!(
            super::ui::label("Deleting...").unwrap().encode(),
            hex("000000240000000600000001000000060000000000000b44656c6574696e672e2e2e7394"),
        );
        assert_eq!(
            super::ui::percent(100).encode(),
            hex("0000001600000006000000010000000700010064927b"),
        );
    }

    /// Object info decodes identically across all four format tags, from real replies
    /// in the set-list bundle capture. This pins three things at once: `version` is at
    /// a fixed offset (and is the name's own version x100 for library content), the
    /// name length is read rather than guessed, and `0xffffffff` means "not
    /// checksummed" rather than being handed back as a checksum.
    #[test]
    fn object_info_decodes_every_format() {
        let cases: &[(&str, &str, u32, Option<u32>, &str)] = &[
            ("000000450000000c0000000a0000001f00000000000000050000000c000000796e65357000000004ffffffffffffffff00000003666f6f000000000000000021ab3d01a1ee",
             "ne5p", 4, Some(0x21ab_3d01), "foo"),
            ("000000460000000c0000000a0000001f000000000000000000000007000000126e65357400000001ffffffffffffffff00000004746573740000000000000000dce9a145bf84",
             "ne5t", 1, Some(0xdce9_a145), "test"),
            ("0000005c0000000c0000000a0000001f0000000000000000000000000c7db5446e706e6f0000021c5e98c95affffffff0000001a526f79616c204772616e64203344205961533620584c20352e340000000500000000ffffffffc30b",
             "npno", 540, None, "Royal Grand 3D YaS6 XL 5.4"),
            ("000000610000000c0000000a0000001f0000000000000000000000000011da986e736d70000000c8554100ec000800000000001f41636f7573746963205069616e6f20335f5f4b6f7267206d6f6e6f20322e300000000000000000ffffffff366f",
             "nsmp", 200, None, "Acoustic Piano 3__Korg mono 2.0"),
        ];
        for (raw, format, version, crc32, name) in cases {
            let info = ProgramInfo::decode(&Message::decode_response(&hex(raw)).unwrap()).unwrap();
            assert_eq!(&info.format, format);
            assert_eq!(info.version, *version, "{format}");
            assert_eq!(info.crc32, *crc32, "{format}");
            assert_eq!(&info.name, name);
        }
    }

    /// A 54-character sample name, straight off the wire. The superseded name-scanning
    /// heuristic was bounded at 32 and would have skipped this entirely.
    #[test]
    fn object_info_reads_names_longer_than_the_old_scan_bound() {
        let info = ProgramInfo::decode(
            &Message::decode_response(&hex(
                "000000780000000c0000000a0000001f00000000000000000000004b002700f66e736d70000000c8554777330009000200000036332056696f6c696e7320534d5f4368616d6265726c696e5f4d4d6173746572206d6f6e6f20736d616c6c2076657273696f6e20322e300000000000000000ffffffff062d",
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(info.name, "3 Violins SM_Chamberlin_MMaster mono small version 2.0");
        assert_eq!(info.name.len(), 54);
    }

    /// A label too long for the one-byte length field is refused, not truncated. The
    /// failure it prevents is silent: `256 as u8` is 0, so the frame would claim an
    /// empty string and carry 256 bytes of payload.
    #[test]
    fn over_long_labels_are_refused_not_truncated() {
        assert!(super::ui::label(&"x".repeat(super::ui::MAX_LABEL_LEN)).is_ok());
        assert!(super::ui::label(&"x".repeat(super::ui::MAX_LABEL_LEN + 1)).is_err());
    }

    /// Percent clamps rather than erroring — no `u16` can produce a malformed frame.
    #[test]
    fn percent_clamps_to_100() {
        assert_eq!(super::ui::percent(101).encode(), super::ui::percent(100).encode());
        assert_eq!(super::ui::percent(u16::MAX).encode(), super::ui::percent(100).encode());
    }

    /// Decoding a dependency list from a *request*-decoded message would shift every
    /// offset by the four-byte status word. That must be an error, not a misparse.
    #[test]
    fn dependencies_require_a_response() {
        let raw = hex(
            "000000820000000c0000000a0000002900000000000000060000000200000002000000000000000001d303b5f20000001a526f79616c204772616e64203344205961533620584c20352e3400000000ffffffffffffffff010000000000000003f2f5cadc0000000c6166726963615f73706c697400000000ffffffffffffffffc791",
        );
        assert!(Dependency::decode_all(&Message::decode(&raw).unwrap()).is_err());
        assert!(Dependency::decode_all(&Message::decode_response(&raw).unwrap()).is_ok());
    }

    /// Decode the dependency list a real duplicate read back: a piano and a sample,
    /// each with the content id that also appears in the file header.
    #[test]
    fn decodes_real_dependencies() {
        let resp = Message::decode_response(&hex(
            "000000820000000c0000000a0000002900000000000000060000000200000002000000000000000001d303b5f20000001a526f79616c204772616e64203344205961533620584c20352e3400000000ffffffffffffffff010000000000000003f2f5cadc0000000c6166726963615f73706c697400000000ffffffffffffffffc791",
        ))
        .unwrap();
        let deps = Dependency::decode_all(&resp).unwrap();
        assert_eq!(deps.len(), 2);

        assert_eq!(deps[0].class, ObjectClass::Piano);
        assert_eq!(deps[0].id, 0xd303_b5f2);
        assert_eq!(deps[0].name, "Royal Grand 3D YaS6 XL 5.4");
        assert_eq!(deps[0].location, None);

        assert_eq!(deps[1].class, ObjectClass::Sample);
        assert_eq!(deps[1].id, 0xf2f5_cadc);
        assert_eq!(deps[1].name, "africa_split");
        assert_eq!(deps[1].location, None);
    }
}
