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
//! Derived from captured traffic and confirmed on hardware: this framing carries every
//! operation the crate performs, and two platforms emit byte-identical request frames
//! for the same verb. What an individual command *means* is a separate question, and
//! several below are still open.
//!
//! A response to a request is `command + 1` and inserts a `u32` status (0 = success)
//! ahead of the echoed arguments. The unsolicited [`cmd::CHANGED`] notification is
//! status-less.
//!
//! Requests are *usually* even, but that is a pattern and not a rule — [`cmd::SELECT`]
//! is `0x2f`, an odd request whose response is `0x30`. **Direction is the only reliable
//! discriminator**, which is why this module records it at decode time rather than
//! deriving it (see [`Message::decode_response`]).

use crate::error::{Error, Result};
use nord_format::fields::Library;

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
/// [`cmd::SELECT`] is the counter-example, so do not treat parity as meaning anything.
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

    /// Begin writing an entity. Args: bank, slot, body length, format tag, timestamp,
    /// `0xFFFFFFFF`, then the slot's **name**, length-prefixed — a placeholder name
    /// becomes the slot's name.
    pub const BEGIN_WRITE: u32 = 0x0a;

    /// Reclaim library storage; the argument is a block count (256KiB blocks, what
    /// `STATUS` counts). A library `BEGIN_WRITE` short on free blocks is refused
    /// `0x16` until this has run in the same session. Destroys nothing.
    pub const WRITE_PREPARE: u32 = 0x22;
    /// Query the cleaning pass: three reply words `[requested, done, running]`, ready
    /// when `running` is 0. Only meaningful after `0x22` in the same session.
    pub const WRITE_PREPARE_2: u32 = 0x26;
    /// Begin reading an entity. Args: bank, slot.
    pub const BEGIN_READ: u32 = 0x0c;
    /// Finish a transfer, either direction. Args: bank, slot.
    pub const END_TRANSFER: u32 = 0x0e;
    /// Push entity bytes. Args: bank, slot, offset, length, then the body.
    pub const WRITE_DATA: u32 = 0x10;
    /// List an entity's piano/sample dependencies.
    pub const DEPENDENCIES: u32 = 0x28;

    /// List the device's storage partitions. No arguments.
    ///
    /// **The partition index is the object class code.** The classes this crate names
    /// are positions in this table, which is why the numbering has gaps: 0 and 2 are
    /// `(Native)` variants of the piano and sample libraries, holding the same objects in
    /// a different order.
    pub const PARTITIONS: u32 = 0x00;

    /// List one partition's banks and their slot capacity. Args: partition index.
    ///
    /// The only source of a class's geometry. Piano "banks" are the panel's categories
    /// (`Grand`, `Upright`, …), so a piano address is category:position.
    pub const BANKS: u32 = 0x02;

    /// The object the panel currently has loaded, for the session's class. No arguments;
    /// the reply is a bank/slot pair. The read half of [`SELECT`].
    ///
    /// Class-dependent: status `0x1` when nothing of the session's class is loaded, and
    /// status `0x15` from the library classes, which have no focus at all.
    pub const FOCUS: u32 = 0x31;

    /// Adjacent occupied slot: `bank, slot, direction` (`0` forward, `1` backward);
    /// slot `0xffff_ffff` walks from the bank's boundary. Status `1` is the
    /// end-of-walk signal, not a fault; an empty bank and a missing bank answer
    /// identically, so bank existence needs [`INFO`]. ⚠️ Omitting the direction word
    /// is refused `0x11` after any write since power-up.
    pub const NEXT_SLOT: u32 = 0x20;

    /// Erases an entire partition.
    ///
    /// Reported by independent interop projects as erase-all-in-partition; **not
    /// confirmed on hardware, deliberately.** A session is class-scoped, so the session
    /// is what aims this: opened on a library class it takes the whole piano or sample
    /// store, which is hundreds of megabytes and a long restore from a backup. Named
    /// here so it can be recognised and refused, not so it can be sent.
    pub const ERASE_ALL: u32 = 0x24;

    /// Highest command the instrument has ever been seen to answer.
    ///
    /// Above this is unexplored space, and it is not empty: at least one code up there
    /// reaches a destructive path, paints its own progress label, never replies, and
    /// needs a power cycle. Distance from the known range is not evidence that a code is
    /// unimplemented.
    pub const HIGHEST_ANSWERING: u32 = 0x3d;

    /// Wedges the instrument: no reply, the session's close goes unanswered, and the
    /// bulk endpoints stall until a power cycle. Reported elsewhere as the read half of
    /// [`NOTIFY_ENABLE`], which is not what it does here.
    pub const NOTIFY_READ_WEDGE: u32 = 0x2a;

    /// Unsolicited device → host notification — no request pairs with it, so it
    /// arrives in place of whatever reply the host reads for next. Observed on
    /// hardware, queued by a front-panel STORE while a cable session was possible;
    /// absent from the capture corpus, so NSM presumably drains it silently.
    /// Hypothesis, not confirmed: "an object changed".
    pub const CHANGED: u32 = 0x2c;

    /// Enable/disable change notifications for a class: `class, on`. The reported
    /// read half is [`NOTIFY_READ_WEDGE`].
    pub const NOTIFY_ENABLE: u32 = 0x2d;
}

/// The UI/session service (service 6, subsystem 1): the transaction's outer handshake
/// and the progress strings NSM paints on the **instrument's own display** during a
/// transfer.
///
/// The progress messages ([`ui::label`], [`ui::percent`]) are **fire-and-forget** — the
/// device never replies. They must be sent with `Session::notify`, never `request`,
/// which would block forever waiting for a response that never comes.
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
    /// Per format tag, not a per-item counter: `ne5p` reports 4 and `ne5t` reports 0
    /// or 1. For library content it is the version in the object's own *name*, ×100 —
    /// `Royal Grand 3D YaS6 XL 5.4` reports `540`.
    pub version: u32,
    /// CRC-32 of the body, as the device reports it. Lets a read be verified
    /// against the device's own checksum rather than trusting the transfer.
    ///
    /// `None` for classes the device does not checksum — pianos and samples report
    /// `0xffffffff` rather than a real value, which is normalized away here so callers
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
        // A request-decoded message retains the status position and shifts every field.
        if !msg.is_response() {
            return Err(Error::InvalidArgument(
                "object info must be decoded from a response (use Message::decode_response)".into(),
            ));
        }
        let p = msg.payload();
        if p.len() < Self::NAME_LEN_AT + 4 {
            return Err(Error::Truncated {
                got: p.len(),
                need: Self::NAME_LEN_AT + 4,
            });
        }
        let word = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().unwrap());

        // Words 20 and 24 vary for libraries, so preserve their position without asserting them.
        let name_len = word(Self::NAME_LEN_AT) as usize;
        let name_start = Self::NAME_LEN_AT + 4;
        let name_end = checked_end(p, name_start, name_len)?;
        let name = String::from_utf8_lossy(&p[name_start..name_end])
            .trim_end()
            .to_owned();

        // Trailing word, past the padding. Absent if the reply stops at the name.
        let crc32 = match p.len().saturating_sub(name_end) >= 4 {
            true => match word(p.len() - 4) {
                u32::MAX => None,
                crc => Some(crc),
            },
            false => None,
        };

        Ok(Self {
            location: Location {
                bank: word(0),
                slot: word(4),
            },
            body_len: word(8),
            format: String::from_utf8_lossy(&p[12..16]).into_owned(),
            version: word(16),
            crc32,
            name,
        })
    }
}

/// Fixed-size field block trailing each partition record.
const PARTITION_FIELDS: usize = 29;

fn read_u32(buf: &[u8], at: usize) -> Result<u32> {
    let end = at.checked_add(4).ok_or(Error::Truncated {
        got: buf.len(),
        need: usize::MAX,
    })?;
    buf.get(at..end)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or(Error::Truncated {
            got: buf.len(),
            need: end,
        })
}

fn checked_end(buf: &[u8], start: usize, len: usize) -> Result<usize> {
    let end = start.checked_add(len).ok_or(Error::Truncated {
        got: buf.len(),
        need: usize::MAX,
    })?;
    if end > buf.len() {
        return Err(Error::Truncated {
            got: buf.len(),
            need: end,
        });
    }
    Ok(end)
}

/// One of the device's storage partitions, from [`cmd::PARTITIONS`].
///
/// **The index in the reply is the object class code** — `ObjectClass::from_raw` numbers
/// positions in this table, gaps included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// Position in the table, and therefore the class code.
    pub index: u32,
    /// The device's own name: `Piano`, `Samp Lib`, `Program`, `Set List`, …
    pub name: String,
    /// Whether this is the `(Native)` view of a library. Native and user partitions
    /// describe **one** pool — identical capacity fields — ordered differently.
    pub native: bool,
    /// The 29 trailing bytes, verbatim: four big-endian words and then 13 one-byte
    /// flags. Only the words this type exposes an accessor for are decoded; the rest are
    /// carried so a caller can look at them without another read.
    ///
    /// Static configuration, not state — every value is unchanged by storing or deleting
    /// content.
    pub fields: Vec<u8>,
}

/// One bank within a partition, from [`cmd::BANKS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bank {
    /// Zero-based position, as addresses use it. The panel shows this plus one.
    pub index: u32,
    /// The device's name for it. For pianos these are the panel's **categories**
    /// (`Grand`, `Upright`, `EPiano1`, …), not numbers.
    pub name: String,
    /// How many slots the bank holds. `0xfffe` appears for the `(Native)` partitions and
    /// is a sentinel, not a capacity.
    pub slots: u32,
}

impl Partition {
    /// Decode a [`cmd::PARTITIONS`] reply: `[u8 count]` then that many
    /// `[u32 name_len][name][29 bytes]` records.
    ///
    /// ⚠️ The length prefix is a **`u32`**. Read as a `u16` the first record still parses
    /// and every one after it lands mid-field, which looks like corruption rather than a
    /// framing mistake.
    pub fn decode_all(msg: &Message) -> Result<Vec<Self>> {
        if !msg.is_response() {
            return Err(Error::InvalidArgument(
                "partitions must be decoded from a response".into(),
            ));
        }
        let p = msg.payload();
        let count = *p.first().ok_or(Error::Truncated { got: 0, need: 1 })? as usize;
        let mut out = Vec::with_capacity(count);
        let mut at = 1;
        for index in 0..count {
            let len = read_u32(p, at)? as usize;
            let name_start = checked_end(p, at, 4)?;
            let end = checked_end(p, name_start, len)?;
            let fields_end = checked_end(p, end, PARTITION_FIELDS)?;
            let name = String::from_utf8_lossy(&p[name_start..end])
                .trim_end()
                .to_string();
            out.push(Partition {
                index: index as u32,
                native: name.contains("(Native)"),
                name,
                fields: p[end..fields_end].to_vec(),
            });
            at = fields_end;
        }
        Ok(out)
    }

    /// The partition's allocation granularity, in **net** bytes — the payload one unit of
    /// whatever [`Status`] counts here holds.
    ///
    /// Library partitions report a storage block minus its own overhead; an Electro 5
    /// reports `261632` (256 KiB − 512) for pianos and `131064` (128 KiB − 8) for
    /// samples. Slot-addressed partitions report `1`, which is what says their counters
    /// are byte-granular.
    ///
    /// ⚠️ Net, not gross. Sizing a write off the enclosing power-of-two block instead
    /// differs only for a body within the overhead of an exact block boundary, but it
    /// differs by a whole block when it does.
    ///
    /// Confirmed on hardware.
    pub fn allocation_unit(&self) -> Option<u32> {
        self.fields
            .get(..4)
            .map(|w| u32::from_be_bytes(w.try_into().expect("four bytes")))
    }
}

impl Bank {
    /// Decode a [`cmd::BANKS`] reply: the echoed partition, a count, then
    /// `[u32 name_len][name][u32 slots]` records.
    pub fn decode_all(msg: &Message) -> Result<Vec<Self>> {
        if !msg.is_response() {
            return Err(Error::InvalidArgument(
                "banks must be decoded from a response".into(),
            ));
        }
        let p = msg.payload();
        let count = *p.get(4).ok_or(Error::Truncated {
            got: p.len(),
            need: 5,
        })? as usize;
        let mut out = Vec::with_capacity(count);
        let mut at = 5;
        for index in 0..count {
            let len = read_u32(p, at)? as usize;
            let name_start = checked_end(p, at, 4)?;
            let end = checked_end(p, name_start, len)?;
            let name = String::from_utf8_lossy(&p[name_start..end])
                .trim_end()
                .to_string();
            out.push(Bank {
                index: index as u32,
                name,
                slots: read_u32(p, end)?,
            });
            at = end + 4;
        }
        Ok(out)
    }

    /// The sentinel the `(Native)` partitions report instead of a real capacity.
    pub const UNBOUNDED: u32 = 0xfffe;

    /// Whether [`Self::slots`] is a real capacity rather than the sentinel.
    pub fn is_bounded(&self) -> bool {
        self.slots != Self::UNBOUNDED
    }
}

/// One entry from a [`cmd::DEPENDENCIES`] response: a piano or sample that a program
/// (or a program that a set list) references.
///
/// The library `id` is the same id the object carries in its own file — a
/// `PianoPanel`'s piano id, a sample's sample id — so this is the bridge between the
/// content on the wire and the bytes on disk.
pub struct Dependency {
    /// Whether this reference is **live**: `1` when the section owning it (piano or
    /// sample) is routed to a keyboard part in that program, `0` otherwise.
    ///
    /// ⚠️ Not a presence flag. The device resolves an unrouted section's model index to
    /// a library object anyway, so a `0` row can name a piano the program's own body
    /// records as `none` — and the same object reads `1` from one program and `0` from
    /// another. **Filter on this before treating a row as a dependency**, or a bundle
    /// walk collects objects nothing plays.
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
    /// Whether this row is a dependency the object actually has.
    ///
    /// A row addresses its object one of two ways: library content (pianos, samples)
    /// by [`Self::id`], slot-addressed content (a set list's programs) by
    /// [`Self::location`] — with `id` always `0`. Confirmed on hardware. Requiredness
    /// therefore asks whether the row addresses *anything*, by either field; an
    /// id-only filter silently classifies every set-list dependency as unassigned and
    /// a set-list bundle walk collects nothing.
    ///
    /// Two kinds of row are reported but are **not** dependencies, and both look like one
    /// at a glance:
    ///
    /// - The section owning it is not routed to a keyboard part ([`Self::flag`] `0`). The
    ///   device resolves the section's model index to a library object regardless, so the
    ///   row can name a piano the object's own body records as `none`.
    /// - The section *is* routed but nothing is assigned to it, giving a live flag with a
    ///   null [`Self::id`] and no location.
    ///
    /// Anything collecting an object's real requirements — a bundle walk above all —
    /// wants this rather than the raw list, or it goes looking for objects that either
    /// are not played or do not exist.
    pub fn is_required(&self) -> bool {
        self.flag == 1 && (self.id != 0 || self.location.is_some())
    }

    /// Decode a whole [`cmd::DEPENDENCIES`] response into the list it carries.
    ///
    /// Layout after the leading `bank, slot, count`, each entry is
    /// `[u8 flag][u32 reserved][u32 class][u32 id][u32 name_len][name][u32 has_location][u32 bank][u32 slot]`
    /// with no alignment padding, so an entry is `29 + name_len` bytes.
    pub fn decode_all(msg: &Message) -> Result<Vec<Self>> {
        // Request decoding leaves the status position in place and shifts every entry.
        if !msg.is_response() {
            return Err(Error::InvalidArgument(
                "dependency list must be decoded from a response (use Message::decode_response)"
                    .into(),
            ));
        }
        let p = msg.payload();
        if p.len() < 12 {
            return Err(Error::Truncated {
                got: p.len(),
                need: 12,
            });
        }
        let word = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().unwrap());
        let count = word(8) as usize;

        // Bound allocation by the payload's minimum possible entry count.
        let mut out = Vec::with_capacity(count.min((p.len() - 12) / 29));
        let mut i = 12;
        for _ in 0..count {
            // flag(1) + reserved(4) + class(4) + id(4) + name_len(4) = 17 bytes.
            let name_start = checked_end(p, i, 17)?;
            let flag = p[i];
            let class = ObjectClass::from_raw(word(i + 5));
            let id = word(i + 9);
            let name_len = word(i + 13) as usize;
            let name_end = checked_end(p, name_start, name_len)?;
            let record_end = checked_end(p, name_end, 12)?;
            let name = String::from_utf8_lossy(&p[name_start..name_end]).into_owned();
            let has_location = word(name_end) != 0;
            let location = has_location.then(|| Location {
                bank: word(name_end + 4),
                slot: word(name_end + 8),
            });
            out.push(Self {
                flag,
                class,
                id,
                name,
                location,
            });
            i = record_end;
        }
        Ok(out)
    }
}

/// CRC-16/CCITT-FALSE — poly `0x1021`, init `0xFFFF`, no reflection, no xorout.
///
/// Identified from known message/trailer pairs and checked across the capture corpus.
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
    /// Everything between the command word and the CRC. Ordinary responses include
    /// their leading status word; [`cmd::CHANGED`] does not.
    pub args: Vec<u8>,
    /// Set by the decoder from the direction the bytes traveled. Not inferable from
    /// the command code — see [`Message::is_response`].
    is_response: bool,
}

/// The protocol version to put on [`Service::Program`] frames, when something other than
/// the caller's is wanted.
///
/// The device treats 8, 9 and 10 as synonyms and drops anything newer; **values below 8
/// stall the bulk endpoints and need a power cycle**, so this exists to compare the
/// accepted window, not to sweep.
#[cfg(feature = "fault-injection")]
fn protocol_version_override() -> Option<u32> {
    std::env::var("NORD_PROTOCOL_VERSION")
        .ok()
        .and_then(|v| v.parse().ok())
}

#[cfg(not(feature = "fault-injection"))]
fn protocol_version_override() -> Option<u32> {
    None
}

impl Message {
    /// A request, to send to the device.
    pub fn new(service: Service, subsystem: u32, command: u32, args: Vec<u8>) -> Self {
        // Only the program service carries a version here; the UI service's `1` is a real
        // subsystem selector and overriding it would be a different frame entirely.
        let subsystem = match service {
            Service::Program => protocol_version_override().unwrap_or(subsystem),
            _ => subsystem,
        };
        Self {
            service,
            subsystem,
            command,
            args,
            is_response: false,
        }
    }

    /// Whether this message was decoded as a device response.
    ///
    /// **Direction, not parity.** Parity invites the guess and does not support it: the
    /// "select in instrument" command is `0x2f` (odd) with response `0x30` (even),
    /// exactly inverting it. The `response == request + 1` rule does hold — only the
    /// parity of the request does not. Getting this backwards silently misaligns
    /// [`Self::payload`] by four bytes and hides device errors, so it is recorded at
    /// decode time by the side that knows.
    pub fn is_response(&self) -> bool {
        self.is_response
    }

    /// The status word an ordinary response leads with. `Some(0)` is success.
    pub fn status(&self) -> Option<u32> {
        if !self.is_response || self.command == cmd::CHANGED || self.args.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes(self.args[..4].try_into().ok()?))
    }

    /// Arguments with an ordinary response's status stripped. Notifications are unchanged.
    pub fn payload(&self) -> &[u8] {
        if self.is_response && self.command != cmd::CHANGED && self.args.len() >= 4 {
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
        // CHANGED is an unsolicited notification, not a command response, and carries no status.
        if m.command != cmd::CHANGED && buf.len() < HEADER_LEN + 4 + CRC_LEN {
            return Err(Error::Truncated {
                got: buf.len(),
                need: HEADER_LEN + 4 + CRC_LEN,
            });
        }
        m.is_response = true;
        Ok(m)
    }

    /// Decode an exploratory reply without requiring the ordinary status word.
    /// A short frame is an observation, not a typed operation failure.
    pub fn decode_probe(buf: &[u8]) -> Result<Self> {
        let mut m = Self::decode(buf)?;
        m.is_response = true;
        Ok(m)
    }

    /// Decode bytes without asserting a direction; treated as a request.
    /// Prefer [`Self::decode_response`] for anything read off the wire.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_LEN + CRC_LEN {
            return Err(Error::Truncated {
                got: buf.len(),
                need: HEADER_LEN + CRC_LEN,
            });
        }
        let declared = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
        if declared != buf.len() {
            return Err(Error::LengthMismatch {
                declared,
                actual: buf.len(),
            });
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
/// alone. Confirmed on hardware: **the class code is the device's partition index**,
/// and the partition table names each one. An unrecognized numeric class is preserved.
///
/// The gaps at `0` and `2` are the `Piano (Native)` and `Samp Lib (Native)` partitions
/// — a second view of the same objects in storage order rather than by category. Both
/// are readable and neither is modelled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectClass {
    Piano,
    Sample,
    Program,
    SetList,
    /// The three Live slots. Wire-addressed `0:0..0:2`; bodies are `ne5p`-shaped.
    Live,
    /// The global settings singleton, wire-addressed `0:0`. Reports no body checksum
    /// (`0xffffffff`), like the library classes.
    Settings,
    Unknown(u32),
}

/// The four library classes are the libraries a decoded body can refer into, and their
/// codes are [`Library::code`]'s — one table for a caller holding both.
impl From<Library> for ObjectClass {
    fn from(library: Library) -> Self {
        match library {
            Library::Piano => ObjectClass::Piano,
            Library::Sample => ObjectClass::Sample,
            Library::Program => ObjectClass::Program,
            Library::SetList => ObjectClass::SetList,
        }
    }
}

impl ObjectClass {
    pub fn from_raw(v: u32) -> Self {
        if let Some(library) = u8::try_from(v).ok().and_then(Library::from_code) {
            return library.into();
        }
        match v {
            6 => ObjectClass::Live,
            7 => ObjectClass::Settings,
            other => ObjectClass::Unknown(other),
        }
    }

    pub fn to_raw(self) -> u32 {
        match self {
            ObjectClass::Piano => Library::Piano.code().into(),
            ObjectClass::Sample => Library::Sample.code().into(),
            ObjectClass::Program => Library::Program.code().into(),
            ObjectClass::SetList => Library::SetList.code().into(),
            ObjectClass::Live => 6,
            ObjectClass::Settings => 7,
            ObjectClass::Unknown(v) => v,
        }
    }

    /// The classes worth querying for an inventory. Live and Settings also answer, but
    /// report zero items — they are singletons, not slot-counted storage.
    pub const INVENTORY: [ObjectClass; 4] = [
        ObjectClass::Piano,
        ObjectClass::Sample,
        ObjectClass::Program,
        ObjectClass::SetList,
    ];

    pub fn label(self) -> String {
        match self {
            ObjectClass::Piano => "pianos".into(),
            ObjectClass::Sample => "samples".into(),
            ObjectClass::Program => "programs".into(),
            ObjectClass::SetList => "set lists".into(),
            ObjectClass::Live => "live slots".into(),
            ObjectClass::Settings => "settings".into(),
            ObjectClass::Unknown(v) => format!("class {v}"),
        }
    }

    /// Whether this class is one of the content libraries, whose objects vary in size
    /// and whose [`Status`] counters are storage blocks rather than bytes.
    pub fn is_library(self) -> bool {
        matches!(self, ObjectClass::Piano | ObjectClass::Sample)
    }

    /// Whether a write into an *occupied* slot of this class lands without deleting it
    /// first.
    ///
    /// Confirmed on hardware.
    ///
    /// Live and Settings accept the ordinary `BEGIN_WRITE` → `WRITE_DATA` →
    /// `END_TRANSFER` sequence at their occupied slots and the body
    /// reads back as what was sent, where every other class answers status `0x4` until
    /// the slot is empty. Their delete has never been attempted, so composing a write
    /// out of delete-then-write there is both unnecessary and untested.
    pub fn overwrites_in_place(self) -> bool {
        matches!(self, ObjectClass::Live | ObjectClass::Settings)
    }

    /// Whether the device stores a name for the objects of this class.
    ///
    /// Confirmed on hardware.
    ///
    /// Live and Settings hold fixed names (`Live 1`, `Settings`) — they answer
    /// `0x1c` rename with success and change nothing, and they carry `BEGIN_WRITE`'s
    /// name argument and discard it. Partition record word 3, the slot-family name
    /// length, is `0` for both.
    pub fn names_its_slots(self) -> bool {
        !matches!(self, ObjectClass::Live | ObjectClass::Settings)
    }
}

/// What [`cmd::STATUS`] reports, for whichever [`ObjectClass`] the session opened.
///
/// **The unit differs by class family.** The slot-addressed classes (program, set list,
/// live, settings) count **bytes**: a program costs 141 = 121 body + 16 name + 4 CRC, a
/// set list 38 = 18 + 16 + 4. The library classes (piano, sample) count **storage
/// blocks** of [`Partition::allocation_unit`] net payload bytes each — just under 256
/// KiB for pianos and 128 KiB for samples on an Electro 5.
///
/// ⚠️ **`free + used` is not the capacity.** A delete moves its space into
/// [`Self::dirty`], not into `free`, so a report built from those two words shrinks
/// every time something is deleted. [`Self::total`] sums all four storage words, which
/// *is* constant, and [`Self::available`] is the space a write can actually reach —
/// `free` now, plus whatever the cleaning pass can reclaim out of `dirty`.
///
/// `dirty` and `spare` read `0` outside the library partitions.
///
/// Confirmed on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub class: ObjectClass,
    pub count: u32,
    /// Space a write may use immediately, without a cleaning pass.
    pub free: u32,
    /// Space live objects occupy. Deleting lowers this and raises `dirty`.
    pub used: u32,
    /// Space held by deleted objects, reclaimable by [`cmd::WRITE_PREPARE`]. Survives a
    /// power cycle, where `free`'s prepared state does not reliably.
    pub dirty: u32,
    /// The fifth word: a small per-partition constant (1 and 2 observed), never seen to
    /// move. Meaning unknown, and counted in [`Self::total`] only because the four
    /// storage words together sum to a value that does not change.
    pub spare: u32,
}

impl Status {
    /// The partition's capacity — constant per class, in the class's own unit.
    pub fn total(&self) -> u64 {
        u64::from(self.free) + u64::from(self.used) + u64::from(self.dirty) + u64::from(self.spare)
    }

    /// Space a write can reach: what is free now plus what cleaning can reclaim.
    ///
    /// A partition reporting `free` 0 with a large `dirty` pool is entirely writable —
    /// [`crate::op::write`] reclaims the shortfall before it begins.
    pub fn available(&self) -> u64 {
        u64::from(self.free) + u64::from(self.dirty)
    }

    /// Bytes per item, when every item of this class costs the same.
    ///
    /// Only the slot-addressed classes resolve: their `STATUS` unit is bytes and every
    /// item is one fixed record. The library classes count blocks of genuinely
    /// variable-size content, and a class this crate cannot name has no known unit, so
    /// both yield `None` whatever their counters happen to divide into.
    pub fn bytes_per_item(&self) -> Option<u32> {
        if self.class.is_library() || matches!(self.class, ObjectClass::Unknown(_)) {
            return None;
        }
        if self.count == 0 || self.used == 0 || !self.used.is_multiple_of(self.count) {
            return None;
        }
        let per = self.used / self.count;
        // Only trust it if the class capacity is also a whole number of items;
        // otherwise the division is a coincidence.
        (per != 0 && self.total().is_multiple_of(u64::from(per))).then_some(per)
    }

    /// Total item slots, for classes where items are fixed-size.
    ///
    /// Far more meaningful than a byte count: programs report 400, which is exactly the
    /// 8 banks × 50 slots of an Electro 5.
    pub fn slots(&self) -> Option<u32> {
        self.bytes_per_item()
            .and_then(|per| u32::try_from(self.total() / u64::from(per)).ok())
    }

    pub fn used_percent(&self) -> f32 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            100.0 * self.used as f32 / total as f32
        }
    }

    /// Decode a [`cmd::STATUS`] response: `count, free, used, dirty, spare`.
    ///
    /// The five-word shape is what an Electro 5 answers. Confirmed on hardware. Only the
    /// first three words are required; a shorter reply decodes with the missing words as
    /// zero.
    pub fn decode(class: ObjectClass, msg: &Message) -> Result<Self> {
        // Request decoding would leave the status position and shift every counter.
        if !msg.is_response() {
            return Err(Error::InvalidArgument(
                "status must be decoded from a response (use Message::decode_response)".into(),
            ));
        }
        let p = msg.payload();
        if p.len() < 12 {
            return Err(Error::Truncated {
                got: p.len(),
                need: 12,
            });
        }
        if !p.len().is_multiple_of(4) {
            return Err(Error::Truncated {
                got: p.len(),
                need: (p.len() / 4 + 1) * 4,
            });
        }
        let word = |i: usize| u32::from_be_bytes(p[i * 4..i * 4 + 4].try_into().unwrap());
        Ok(Self {
            class,
            count: word(0),
            free: word(1),
            used: word(2),
            dirty: if p.len() >= 16 { word(3) } else { 0 },
            spare: if p.len() >= 20 { word(4) } else { 0 },
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
    ///
    /// Panics if either number is zero.
    pub fn from_user(bank: u32, slot: u32) -> Self {
        assert!(
            bank >= 1 && slot >= 1,
            "from_user takes the panel's one-indexed numbering; got {bank}:{slot}"
        );
        Self {
            bank: bank - 1,
            slot: slot - 1,
        }
    }

    /// The one-indexed bank number shown by the instrument.
    pub fn user_bank(self) -> u64 {
        u64::from(self.bank) + 1
    }

    /// The one-indexed slot number shown by the instrument.
    pub fn user_slot(self) -> u64 {
        u64::from(self.slot) + 1
    }

    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.bank.to_be_bytes());
        out.extend_from_slice(&self.slot.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    /// A library reference in a decoded body and a session on the wire name the same
    /// catalogue by the same code, in both directions.
    #[test]
    fn a_library_class_code_is_the_librarys_own() {
        use super::ObjectClass;
        use nord_format::fields::Library;
        for library in [
            Library::Piano,
            Library::Sample,
            Library::Program,
            Library::SetList,
        ] {
            let class = ObjectClass::from(library);
            assert_eq!(class.to_raw(), u32::from(library.code()), "{library:?}");
            assert_eq!(ObjectClass::from_raw(library.code().into()), class);
        }
        assert_eq!(ObjectClass::from_raw(6), ObjectClass::Live);
        assert_eq!(ObjectClass::from_raw(0), ObjectClass::Unknown(0));
    }

    use super::*;

    /// The middle exchange of `move_prog_8-13_to_7-16`, byte-for-byte off the wire.
    const MOVE: &str = "000000220000000c0000000a00000018000000070000000c000000060000000f4a55";
    /// Its response: command +1, status word inserted, arguments echoed.
    const MOVE_RESP: &str =
        "000000260000000c0000000a0000001900000000000000070000000c000000060000000f7197";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn only_the_buffer_classes_overwrite_in_place_and_hold_no_name() {
        for class in [ObjectClass::Live, ObjectClass::Settings] {
            assert!(class.overwrites_in_place(), "{}", class.label());
            assert!(!class.names_its_slots(), "{}", class.label());
        }
        let storage = [
            ObjectClass::Piano,
            ObjectClass::Sample,
            ObjectClass::Program,
            ObjectClass::SetList,
            ObjectClass::Unknown(9),
        ];
        for class in storage {
            assert!(!class.overwrites_in_place(), "{}", class.label());
            assert!(class.names_its_slots(), "{}", class.label());
        }
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
        let req =
            Message::decode(&hex("0000001a0000000c0000000a0000002f00000000000000017f71")).unwrap();
        assert_eq!(req.command, 0x2f);
        assert!(req.command & 1 == 1, "this request really is odd-numbered");
        assert!(
            !req.is_response(),
            "an odd command must still decode as a request"
        );
        assert_eq!(req.status(), None);
        // A request's payload must not have four bytes eaten as a status word.
        assert_eq!(req.payload().len(), 8);

        // Response: cmd 0x30 (even), status 0, then the echoed args.
        let resp = Message::decode_response(&hex(
            "0000001e0000000c0000000a0000003000000000000000000000000112c4",
        ))
        .unwrap();
        assert_eq!(resp.command, req.command + 1);
        assert!(
            resp.command & 1 == 0,
            "this response really is even-numbered"
        );
        assert!(resp.is_response());
        assert_eq!(
            resp.status(),
            Some(0),
            "status must be readable despite even command"
        );
        assert_eq!(
            resp.payload(),
            req.payload(),
            "args line up once status is stripped"
        );
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
    fn a_response_without_a_status_word_is_truncated() {
        let bytes = Message::new(Service::Program, 10, cmd::STATUS + 1, Vec::new()).encode();
        assert!(matches!(
            Message::decode_response(&bytes),
            Err(Error::Truncated { need: 22, .. })
        ));
    }

    #[test]
    fn changed_is_a_statusless_notification() {
        let bytes = Message::new(Service::Program, 10, cmd::CHANGED, vec![1, 2, 3, 4]).encode();
        let message = Message::decode_response(&bytes).unwrap();
        assert_eq!(message.status(), None);
        assert_eq!(message.payload(), [1, 2, 3, 4]);
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

    /// Object info uses its declared name length and treats `0xffffffff` as no checksum.
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
        assert_eq!(
            info.name,
            "3 Violins SM_Chamberlin_MMaster mono small version 2.0"
        );
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
        assert_eq!(
            super::ui::percent(101).encode(),
            super::ui::percent(100).encode()
        );
        assert_eq!(
            super::ui::percent(u16::MAX).encode(),
            super::ui::percent(100).encode()
        );
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

        // The piano row reads flag 0 — reported, but its section is not routed.
        assert!(!deps[0].is_required());
        assert!(deps[1].is_required());
    }

    /// A live flag addressing nothing — routed section, nothing assigned — is the one
    /// row shape [`Dependency::is_required`] must reject that liveness alone accepts.
    #[test]
    fn a_live_row_addressing_nothing_is_not_required() {
        let d = Dependency {
            flag: 1,
            class: ObjectClass::Piano,
            id: 0,
            name: String::new(),
            location: None,
        };
        assert!(!d.is_required());
    }

    /// A set list's dependencies are programs: slot-addressed, [`Dependency::id`]
    /// always `0`, the address in the location words. Confirmed on hardware — a real
    /// set list read back four such rows, all live. A required-filter keyed on id
    /// alone classifies every one as "routed but nothing assigned".
    ///
    /// The frame is constructed to the confirmed shape — echoed bank/slot, count,
    /// then four 29-byte id-0 rows (empty name) with locations — not a byte capture.
    #[test]
    fn set_list_dependencies_are_required_by_location_not_id() {
        let mut args = Vec::new();
        // Status, then the echoed set-list address (panel 1:43, 0-indexed on the
        // wire) and row count.
        for w in [0u32, 0, 42, 4] {
            args.extend_from_slice(&w.to_be_bytes());
        }
        // Slots A–D held panel 1:7, 1:3, 1:39, 1:41.
        for slot in [6u32, 2, 38, 40] {
            args.push(1); // flag: the slot is live
                          // missing, class (program), id, name_len, has_location, bank, slot.
            for w in [0u32, 4, 0, 0, 1, 0, slot] {
                args.extend_from_slice(&w.to_be_bytes());
            }
        }
        assert_eq!(args.len() - 4, 128);

        let raw = Message::new(Service::Program, 10, cmd::DEPENDENCIES + 1, args).encode();
        let deps = Dependency::decode_all(&Message::decode_response(&raw).unwrap()).unwrap();

        assert_eq!(deps.len(), 4);
        let slots: Vec<u32> = deps.iter().map(|d| d.location.unwrap().slot).collect();
        assert_eq!(slots, [6, 2, 38, 40]);
        for d in &deps {
            assert_eq!(d.class, ObjectClass::Program);
            assert_eq!(d.id, 0);
            assert!(d.name.is_empty());
            assert!(d.is_required(), "a slot-addressed dependency is required");
        }
    }
}
