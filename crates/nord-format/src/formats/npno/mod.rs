//! Piano libraries (`.npno`).
//!
//! The body is a `CNSP` stream: a metadata prefix carrying the name, a 128-entry
//! key map and ten per-note tables; then a directory of **strokes** — one
//! recorded note each — and the encoded audio those strokes own. [`Piano`] is the
//! file, body verbatim and checksum verified; [`Library`] is the container
//! parsed, a view whose writer re-lays the directory and the audio from the model
//! it holds. [`codec`] turns one stroke's audio back into samples.
//!
//! Offsets below are relative to the body's first byte, and the stream's own
//! integers are big-endian where the CBIN header's are little-endian.
//!
//! | body offset | field |
//! |---|---|
//! | `0x00` | `"CNSP"` |
//! | `0x04` | u16 stream version — `0x450` or `0x464` |
//! | `0x06` | u32, unique per file; meaning open |
//! | `0x1c` | `Name#Variant`, NUL-padded to 32 bytes |
//! | `0x3c` | the bare name, and at `0x5c` the variant — `0x464` streams only |
//! | `0x8c` | 128-entry key map: the root note that plays each key, `0xFF` uncovered |
//! | `0x18c` | 128-entry per-key fine tune, one of ten per-note tables from `0x10c` |
//! | `0x61c` | u16 stream version, echoed |
//! | `0x61e` | u16 channel count, 1 or 2 |
//! | `0x620` | u16 stroke count `N` |
//! | `0x622` | 128 × u16 strokes per root note, summing to `N` |
//! | `0x732` | `N` × 118-byte stroke records, grouped in ascending root order |
//!
//! The layout is inferred from specimens; not confirmed on hardware. That the key
//! map's value is the recording's root note, that a stroke's [`Bank`] is what it is
//! played for, and that [`Stroke::layer`] indexes softness are confirmed on
//! hardware.
//!
//! Audio follows the directory, one span per record in the directory's own order.
//! The first span starts at the next `1022 × channels` boundary offset by
//! [`AUDIO_ALIGN_BIAS`] (the bias is unexplained), the gap in front of it is zero,
//! each span abuts the one before, and the last ends at the body's end. Because a
//! stroke carries its own predictor seeds and its blocks overlap only each other, a
//! span is self-contained and moves verbatim — which is what makes the transforms
//! on [`Library`] no more than a re-lay.
//!
//! ⚠️ Real libraries are tens of megabytes and reading one allocates the body —
//! [`crate::cbin::inspect`] answers container questions in O(1) instead.
//!
//! ⚠️ The header's `location` and `aux` are unchecked here on purpose: this is a
//! library format, where those words hold something other than a bank/slot pair, and
//! no local specimen says what. Gating on them would refuse real files.

pub mod codec;

use crate::cbin::{self, Cbin, Header, RawBody};
use crate::error::{try_vec, Error, ParseError};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Seek, Write};
use std::ops::RangeInclusive;

pub const FORMAT: &str = "npno";

/// The body's stream magic.
pub const CNSP_MAGIC: &[u8; 4] = b"CNSP";

/// MIDI notes the key map, the count table and each per-note table cover.
pub const NOTES: usize = 128;

/// A key map entry for a note the library does not cover.
pub const UNCOVERED: u8 = 0xff;

/// The stream versions the prefix offsets are validated against. A body with
/// another version still reads and writes verbatim; its fields are refused rather
/// than read from offsets that may not hold them.
pub const KNOWN_VERSIONS: &[u16] = &[0x450, 0x464];
/// [`KNOWN_VERSIONS`] as the gate spells them.
const KNOWN_VERSIONS_U32: &[u32] = &[0x450, 0x464];

/// The stream version that also writes the name and variant as separate fields.
const VERSION_SPLIT_NAME: u16 = 0x464;

const KEY_MAP_AT: usize = 0x8c;
const FINE_TUNE_AT: usize = 0x18c;
const VERSION_AT: usize = 0x04;
const VERSION_ECHO_AT: usize = 0x61c;
const CHANNELS_AT: usize = 0x61e;
const STROKE_COUNT_AT: usize = 0x620;
const ROOT_COUNTS_AT: usize = 0x622;

/// First byte of the stroke directory, and so the length of the prefix.
const DIRECTORY_AT: usize = 0x732;

/// Bytes per stroke record.
const RECORD: usize = 118;

const REC_START: usize = 0x00;
const REC_BANK: usize = 0x04;
const REC_LAYER: usize = 0x05;
const REC_FRAMES: usize = 0x06;
const REC_BLOCKS: usize = 0x0a;
const REC_SEEDS: usize = 0x0c;
const REC_ID: usize = 0x6e;

/// Predictor seeds a record carries per channel.
const SEEDS: usize = 4;

/// The audio grid's offset from a whole number of blocks. Unexplained.
pub const AUDIO_ALIGN_BIAS: usize = 192;

/// Cents one unit of [`Library::fine_tune`] is worth. Measured between 0.6 and
/// 0.8 cents per unit; this is the midpoint. Confirmed on hardware.
pub const FINE_TUNE_CENTS_PER_UNIT: f32 = 0.7;

/// What a stroke is played for, from the record's `+0x04`.
///
/// Confirmed on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bank {
    /// Played at note-on. Every library has these.
    Attack,
    /// Played from the note-on while the sustain pedal is down, which the panel's
    /// acoustics bit 0 enables. Only the larger libraries carry them.
    Resonance,
    /// Played at note-off.
    Release,
}

impl Bank {
    pub const ALL: [Bank; 3] = [Bank::Attack, Bank::Resonance, Bank::Release];

    pub fn from_code(code: u8) -> Option<Bank> {
        match code {
            0 => Some(Bank::Attack),
            1 => Some(Bank::Resonance),
            2 => Some(Bank::Release),
            _ => None,
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Bank::Attack => 0,
            Bank::Resonance => 1,
            Bank::Release => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Bank::Attack => "attack",
            Bank::Resonance => "resonance",
            Bank::Release => "release",
        }
    }
}

impl fmt::Display for Bank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Which velocity layers of a root to keep.
///
/// A root's layers are counted within one [`Bank`], since each bank indexes its
/// own set. Nothing is renumbered: the layer values that survive keep the values
/// they had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layers {
    /// The loudest `n` of each root and bank — the `n` lowest layer values.
    Loudest(usize),
    /// Exactly these layer values, wherever they occur.
    Only(BTreeSet<u8>),
}

/// What a transform removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Change {
    pub strokes_removed: usize,
    pub roots_removed: usize,
    pub keys_uncovered: usize,
}

/// A fixed-width, NUL-padded text field in the prefix.
#[derive(Clone, Copy)]
struct TextField {
    at: usize,
    len: usize,
}

impl TextField {
    /// `Name#Variant`, on every stream version.
    const COMBINED: TextField = TextField {
        at: 0x1c,
        len: 0x20,
    };
    /// The bare name, written only on [`VERSION_SPLIT_NAME`] streams.
    const NAME: TextField = TextField {
        at: 0x3c,
        len: 0x20,
    };
    /// The variant alone, written only on [`VERSION_SPLIT_NAME`] streams.
    const VARIANT: TextField = TextField {
        at: 0x5c,
        len: 0x20,
    };

    /// Longest string the field holds, the terminator excluded.
    const fn capacity(self) -> usize {
        self.len - 1
    }

    fn read(self, prefix: &[u8]) -> String {
        let field = &prefix[self.at..self.at + self.len];
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        String::from_utf8_lossy(&field[..end]).into_owned()
    }

    fn write(self, prefix: &mut [u8], text: &str) -> Result<(), Error> {
        if text.len() > self.capacity() {
            return Err(ParseError::OutOfBounds {
                value: format!("{text:?} ({} bytes)", text.len()),
                bound: format!("at most {} bytes", self.capacity()),
            }
            .into());
        }
        let field = &mut prefix[self.at..self.at + self.len];
        field.fill(0);
        field[..text.len()].copy_from_slice(text.as_bytes());
        Ok(())
    }
}

/// A piano library (`npno`): the CBIN container with the `CNSP` body verbatim.
///
/// Reads and writes byte-exactly, checksum verified. [`Piano::library`] parses the
/// body into the model the transforms and the writer work on.
pub struct Piano {
    pub file: Cbin<RawBody>,
}

impl Piano {
    pub fn new() -> Piano {
        Piano {
            file: Cbin {
                header: Header::new(FORMAT, (0, 0), 0),
                body: RawBody(Vec::new()),
            },
        }
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Piano, Error> {
        Ok(Piano {
            file: cbin::read(reader, FORMAT)?,
        })
    }

    pub fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<(), Error> {
        self.file.write_to(writer)
    }

    /// The body bytes, after checking they open with the `CNSP` magic.
    fn cnsp(&self) -> Result<&[u8], Error> {
        let body = &self.file.body.0;
        if body.get(..4) != Some(CNSP_MAGIC.as_slice()) {
            return Err(ParseError::AssertFail(format!(
                "body opens {:02x?}, not the CNSP stream",
                body.get(..4).unwrap_or_default()
            ))
            .into());
        }
        Ok(body)
    }

    /// The body bytes, after checking the magic and that the stream version is one
    /// the prefix offsets are pinned to.
    fn mapped(&self) -> Result<&[u8], Error> {
        let version = self.stream_version()?;
        crate::formats::known_version(FORMAT, u32::from(version), KNOWN_VERSIONS_U32)?;
        self.cnsp()
    }

    /// The stream version at body `0x04`.
    pub fn stream_version(&self) -> Result<u16, Error> {
        let body = self.cnsp()?;
        let bytes = body.get(VERSION_AT..VERSION_AT + 2).ok_or_else(|| {
            ParseError::AssertFail("body ends inside the CNSP header".to_string())
        })?;
        Ok(u16::from_be_bytes(bytes.try_into().unwrap()))
    }

    /// The `(name, variant)` pair from the `Name#Variant` field — for
    /// *Electric Grand 1 CP80*, `("Electric Grand 1", "CP80")`. The variant is
    /// empty when the field carries none.
    pub fn name(&self) -> Result<(String, String), Error> {
        let body = self.mapped()?;
        if body.len() < DIRECTORY_AT {
            return Err(short("the prefix"));
        }
        Ok(split_name(&TextField::COMBINED.read(body)))
    }

    /// The 128-entry key map: for each MIDI note, the root note whose strokes play
    /// it, or [`UNCOVERED`].
    pub fn key_map(&self) -> Result<&[u8], Error> {
        self.mapped()?
            .get(KEY_MAP_AT..KEY_MAP_AT + NOTES)
            .ok_or_else(|| short("the key map"))
    }

    /// The container parsed: the prefix, the stroke directory and each stroke's
    /// audio span.
    pub fn library(&self) -> Result<Library<'_>, Error> {
        Library::parse(self)
    }
}

impl Default for Piano {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Piano {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("npno::Piano")
            .field("header", &self.file.header)
            .field("body_len", &self.file.body.0.len())
            .finish()
    }
}

/// `Name#Variant` split on its separator, each half trimmed of the padding the
/// vendor lays either side of it.
fn split_name(field: &str) -> (String, String) {
    let (name, variant) = field.split_once('#').unwrap_or((field, ""));
    (name.trim().to_owned(), variant.trim().to_owned())
}

fn short(what: &str) -> Error {
    ParseError::AssertFail(format!("the body ends inside {what}")).into()
}

fn overflow(what: &str) -> Error {
    ParseError::OutOfBounds {
        value: what.to_string(),
        bound: "an offset that fits this platform's address space".into(),
    }
    .into()
}

fn be16(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Where the first audio span starts, given the directory's end and the block size.
///
/// The grid is whole blocks offset by [`AUDIO_ALIGN_BIAS`]; the bytes between the
/// directory and it are zero.
fn first_audio_offset(directory_end: usize, block: usize) -> Result<usize, Error> {
    directory_end
        .checked_add(AUDIO_ALIGN_BIAS)
        .map(|biased| biased.div_ceil(block))
        .and_then(|blocks| blocks.checked_mul(block))
        .and_then(|at| at.checked_sub(AUDIO_ALIGN_BIAS))
        .ok_or_else(|| overflow("the first audio offset"))
}

/// One recorded note: the directory record, and the audio bytes it owns.
///
/// The record is carried verbatim apart from its audio offset, which is a
/// placement and is recomputed every time a library is written.
#[derive(Clone)]
pub struct Stroke<'a> {
    /// The note the recording was made at. It comes from the record's position in
    /// the count table rather than from a field of the record itself. Confirmed on
    /// hardware.
    pub root: u8,
    record: [u8; RECORD],
    audio: &'a [u8],
}

impl<'a> Stroke<'a> {
    /// The `+0x04` bank byte. Specimens hold only the codes [`Bank`] names, but an
    /// unnamed one is carried rather than refused.
    pub fn bank_code(&self) -> u8 {
        self.record[REC_BANK]
    }

    pub fn bank(&self) -> Option<Bank> {
        Bank::from_code(self.bank_code())
    }

    /// Softness index within the root's bank; 0 is the loudest recording, and a
    /// bank's values need be neither dense nor start at zero. Confirmed on
    /// hardware.
    pub fn layer(&self) -> u8 {
        self.record[REC_LAYER]
    }

    /// Frames the stroke owns, which is what [`codec::decode`] emits: the block
    /// overlap is excluded.
    pub fn frames(&self) -> u32 {
        be32(&self.record, REC_FRAMES)
    }

    pub fn blocks(&self) -> u16 {
        be16(&self.record, REC_BLOCKS)
    }

    /// The identifier at `+0x6e`. Distinguishes a recording across libraries;
    /// what else it means is open.
    pub fn id(&self) -> u32 {
        be32(&self.record, REC_ID)
    }

    /// The predictor's four seed samples for `channel`, oldest first. A mono
    /// stroke's second group is unused.
    pub fn seeds(&self, channel: usize) -> [i16; SEEDS] {
        let mut out = [0i16; SEEDS];
        let base = REC_SEEDS + channel * SEEDS * 2;
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = be16(&self.record, base + i * 2) as i16;
        }
        out
    }

    /// The encoded audio, `blocks × 1022 × channels` bytes.
    pub fn audio(&self) -> &'a [u8] {
        self.audio
    }

    /// The record as stored, its audio offset excluded from any meaning: the
    /// writer replaces it.
    pub fn record(&self) -> &[u8; RECORD] {
        &self.record
    }
}

impl fmt::Debug for Stroke<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stroke")
            .field("root", &self.root)
            .field("bank", &self.bank_code())
            .field("layer", &self.layer())
            .field("frames", &self.frames())
            .field("blocks", &self.blocks())
            .finish()
    }
}

/// A piano library parsed: the prefix, and every stroke with its audio.
///
/// Strokes borrow their audio from the [`Piano`] they were parsed from, so a
/// transform that drops strokes copies nothing. The fields the container derives —
/// the stroke count, the per-root counts and every audio offset — are not stored in
/// the model at all; [`Library::to_body`] computes them from the stroke list, which
/// is what makes an unmodified library rebuild to the bytes it was read from.
#[derive(Clone)]
pub struct Library<'a> {
    /// The container header, carried so a transform yields a whole file.
    pub header: Header,
    /// Body bytes before the directory. The setters edit it; the writer rewrites
    /// the counts within it.
    prefix: Vec<u8>,
    channels: u16,
    strokes: Vec<Stroke<'a>>,
}

impl<'a> Library<'a> {
    fn parse(piano: &'a Piano) -> Result<Library<'a>, Error> {
        let body = piano.mapped()?;
        let prefix = body
            .get(..DIRECTORY_AT)
            .ok_or_else(|| short("the prefix"))?;

        let version = be16(prefix, VERSION_AT);
        let echo = be16(prefix, VERSION_ECHO_AT);
        if echo != version {
            return Err(ParseError::AssertFail(format!(
                "the stream version {version:#06x} is echoed as {echo:#06x}"
            ))
            .into());
        }

        let channels = be16(prefix, CHANNELS_AT);
        if !(1..=2).contains(&channels) {
            return Err(ParseError::OutOfBounds {
                value: format!("{channels} channels"),
                bound: "1 or 2".into(),
            }
            .into());
        }
        let block = block_bytes(channels);

        let count = usize::from(be16(prefix, STROKE_COUNT_AT));
        let counts: Vec<u16> = (0..NOTES)
            .map(|n| be16(prefix, ROOT_COUNTS_AT + n * 2))
            .collect();
        let summed: usize = counts.iter().map(|&c| usize::from(c)).sum();
        if summed != count {
            return Err(ParseError::AssertFail(format!(
                "the per-root counts sum to {summed} where the stroke count is {count}"
            ))
            .into());
        }

        let directory_end = RECORD
            .checked_mul(count)
            .and_then(|len| DIRECTORY_AT.checked_add(len))
            .ok_or_else(|| overflow("the stroke directory"))?;
        let records = body
            .get(DIRECTORY_AT..directory_end)
            .ok_or_else(|| short("the stroke directory"))?;

        let first = first_audio_offset(directory_end, block)?;
        let pad = body
            .get(directory_end..first)
            .ok_or_else(|| short("the alignment gap before the audio"))?;
        if pad.iter().any(|&b| b != 0) {
            return Err(ParseError::AssertFail(
                "the alignment gap before the audio is not zero".into(),
            )
            .into());
        }

        let mut strokes = Vec::new();
        strokes
            .try_reserve_exact(count)
            .map_err(|_| overflow("the stroke list"))?;
        let mut at = first;
        let mut roots = counts
            .iter()
            .enumerate()
            .flat_map(|(note, &n)| std::iter::repeat_n(note as u8, usize::from(n)));
        for i in 0..count {
            let mut record = [0u8; RECORD];
            record.copy_from_slice(&records[i * RECORD..(i + 1) * RECORD]);
            let root = roots.next().expect("the counts sum to the stroke count");
            let start = be32(&record, REC_START);
            if usize::try_from(start) != Ok(at) {
                return Err(ParseError::AssertFail(format!(
                    "stroke {i} starts at {start:#x} where the spans before it end at {at:#x}"
                ))
                .into());
            }
            let span = usize::from(be16(&record, REC_BLOCKS))
                .checked_mul(block)
                .ok_or_else(|| overflow("a stroke's audio span"))?;
            let end = at.checked_add(span).ok_or_else(|| overflow("the audio"))?;
            let audio = body
                .get(at..end)
                .ok_or_else(|| short("a stroke's audio span"))?;
            strokes.push(Stroke {
                root,
                record,
                audio,
            });
            at = end;
        }
        if at != body.len() {
            return Err(ParseError::AssertFail(format!(
                "the audio ends at {at:#x} where the body ends at {:#x}",
                body.len()
            ))
            .into());
        }

        let library = Library {
            header: piano.file.header.clone(),
            prefix: prefix.to_vec(),
            channels,
            strokes,
        };
        library.check_key_map()?;
        Ok(library)
    }

    /// Every key map entry names a root the directory holds.
    fn check_key_map(&self) -> Result<(), Error> {
        let roots = self.roots();
        for (key, &root) in self.key_map().iter().enumerate() {
            if root != UNCOVERED && !roots.contains(&root) {
                return Err(ParseError::AssertFail(format!(
                    "key {key} plays root {root}, which no stroke records"
                ))
                .into());
            }
        }
        Ok(())
    }

    pub fn stream_version(&self) -> u16 {
        be16(&self.prefix, VERSION_AT)
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Bytes in one encoded block, `1022 × channels`.
    pub fn block_bytes(&self) -> usize {
        block_bytes(self.channels)
    }

    pub fn strokes(&self) -> &[Stroke<'a>] {
        &self.strokes
    }

    /// The `(name, variant)` pair, from the same field [`Piano::name`] reads.
    pub fn name(&self) -> (String, String) {
        split_name(&TextField::COMBINED.read(&self.prefix))
    }

    /// The 128-entry key map: the root note that plays each key, or [`UNCOVERED`].
    pub fn key_map(&self) -> &[u8] {
        &self.prefix[KEY_MAP_AT..KEY_MAP_AT + NOTES]
    }

    fn key_map_mut(&mut self) -> &mut [u8] {
        &mut self.prefix[KEY_MAP_AT..KEY_MAP_AT + NOTES]
    }

    /// The root notes the directory records, ascending.
    pub fn roots(&self) -> BTreeSet<u8> {
        self.strokes.iter().map(|s| s.root).collect()
    }

    /// The keys the map routes to `root`, ascending.
    pub fn keys_for(&self, root: u8) -> Vec<u8> {
        self.key_map()
            .iter()
            .enumerate()
            .filter(|&(_, &r)| r == root)
            .map(|(key, _)| key as u8)
            .collect()
    }

    /// The per-key fine tune at `0x18c + key`, in units worth
    /// [`FINE_TUNE_CENTS_PER_UNIT`] each. Confirmed on hardware.
    pub fn fine_tune(&self, key: u8) -> i8 {
        self.prefix[FINE_TUNE_AT + usize::from(key)] as i8
    }

    pub fn set_fine_tune(&mut self, key: u8, units: i8) {
        self.prefix[FINE_TUNE_AT + usize::from(key)] = units as u8;
    }

    /// Rename the library, leaving the variant alone.
    pub fn set_name(&mut self, name: &str) -> Result<(), Error> {
        let (_, variant) = self.name();
        self.write_name(name, &variant)
    }

    /// Replace the variant — the text after the `#`, which is where the vendor
    /// records the voicing and the library's size — leaving the name alone.
    pub fn set_variant(&mut self, variant: &str) -> Result<(), Error> {
        let (name, _) = self.name();
        self.write_name(&name, variant)
    }

    fn write_name(&mut self, name: &str, variant: &str) -> Result<(), Error> {
        TextField::COMBINED.write(&mut self.prefix, &format!("{name}#{variant}"))?;
        if self.stream_version() == VERSION_SPLIT_NAME {
            TextField::NAME.write(&mut self.prefix, name)?;
            TextField::VARIANT.write(&mut self.prefix, variant)?;
        }
        Ok(())
    }

    /// Route `key` to `root`, or to nothing when `root` is `None`.
    ///
    /// A root the directory does not record is refused: the instrument would have
    /// no stroke to play.
    pub fn set_key_root(&mut self, key: u8, root: Option<u8>) -> Result<(), Error> {
        if let Some(root) = root {
            if !self.roots().contains(&root) {
                return Err(ParseError::OutOfBounds {
                    value: format!("root {root}"),
                    bound: "a root the directory records".into(),
                }
                .into());
            }
        }
        self.key_map_mut()[usize::from(key)] = root.unwrap_or(UNCOVERED);
        Ok(())
    }

    /// Drop every stroke of one bank — the resonance set turns a large library into
    /// a small one, the release set silences the note-off sample.
    pub fn drop_bank(&mut self, bank: Bank) -> Change {
        let code = bank.code();
        self.retain(|s| s.bank_code() != code)
    }

    /// Keep only the layers `keep` selects, per root and bank.
    pub fn keep_layers(&mut self, keep: &Layers) -> Change {
        match keep {
            Layers::Only(layers) => {
                let layers = layers.clone();
                self.retain(|s| layers.contains(&s.layer()))
            }
            Layers::Loudest(n) => {
                let mut groups: BTreeMap<(u8, u8), BTreeSet<u8>> = BTreeMap::new();
                for stroke in &self.strokes {
                    groups
                        .entry((stroke.root, stroke.bank_code()))
                        .or_default()
                        .insert(stroke.layer());
                }
                let kept: BTreeSet<(u8, u8, u8)> = groups
                    .into_iter()
                    .flat_map(|((root, bank), layers)| {
                        layers.into_iter().take(*n).map(move |l| (root, bank, l))
                    })
                    .collect();
                self.retain(|s| kept.contains(&(s.root, s.bank_code(), s.layer())))
            }
        }
    }

    /// Uncover every key outside `range`, then drop the roots nothing plays any
    /// more. Keys inside the range keep the roots they had.
    pub fn cut_range(&mut self, range: RangeInclusive<u8>) -> Change {
        self.restrict(|key| range.contains(&key))
    }

    /// Two libraries, one covering the keys below `key` and one covering `key` and
    /// above, each cut the way [`Library::cut_range`] cuts.
    pub fn split_at(&self, key: u8) -> (Library<'a>, Library<'a>) {
        let mut low = self.clone();
        let mut high = self.clone();
        low.restrict(|k| k < key);
        high.restrict(|k| k >= key);
        (low, high)
    }

    /// Uncover every key `keep` rejects, then drop the roots nothing plays.
    fn restrict(&mut self, keep: impl Fn(u8) -> bool) -> Change {
        let mut uncovered = 0;
        for (key, slot) in self.key_map_mut().iter_mut().enumerate() {
            if !keep(key as u8) && *slot != UNCOVERED {
                *slot = UNCOVERED;
                uncovered += 1;
            }
        }
        let live: BTreeSet<u8> = self.key_map().iter().copied().collect();
        let mut change = self.retain(|s| live.contains(&s.root));
        change.keys_uncovered += uncovered;
        change
    }

    /// Drop the strokes `keep` rejects, then uncover the keys whose root has gone.
    fn retain(&mut self, mut keep: impl FnMut(&Stroke<'a>) -> bool) -> Change {
        let strokes_before = self.strokes.len();
        let roots_before = self.roots().len();
        self.strokes.retain(|s| keep(s));
        let roots = self.roots();
        let mut keys_uncovered = 0;
        for slot in self.key_map_mut() {
            if *slot != UNCOVERED && !roots.contains(slot) {
                *slot = UNCOVERED;
                keys_uncovered += 1;
            }
        }
        Change {
            strokes_removed: strokes_before - self.strokes.len(),
            roots_removed: roots_before - roots.len(),
            keys_uncovered,
        }
    }

    /// Bytes the body would occupy.
    pub fn body_len(&self) -> Result<usize, Error> {
        let (_, len) = self.extent()?;
        Ok(len)
    }

    /// The first audio offset and the body length the current stroke list implies.
    fn extent(&self) -> Result<(usize, usize), Error> {
        let directory_end = RECORD
            .checked_mul(self.strokes.len())
            .and_then(|len| DIRECTORY_AT.checked_add(len))
            .ok_or_else(|| overflow("the stroke directory"))?;
        let first = first_audio_offset(directory_end, self.block_bytes())?;
        let mut len = first;
        for stroke in &self.strokes {
            len = len
                .checked_add(stroke.audio.len())
                .ok_or_else(|| overflow("the audio"))?;
        }
        Ok((first, len))
    }

    /// Lay the body out: the prefix with its counts rewritten, the directory with
    /// every audio offset recomputed, the zero gap, then the audio spans in
    /// directory order.
    pub fn to_body(&self) -> Result<Vec<u8>, Error> {
        let count = u16::try_from(self.strokes.len()).map_err(|_| ParseError::OutOfBounds {
            value: format!("{} strokes", self.strokes.len()),
            bound: "the u16 stroke count the directory holds".into(),
        })?;
        if self.strokes.windows(2).any(|w| w[0].root > w[1].root) {
            return Err(ParseError::AssertFail(
                "the strokes are not in ascending root order, which is what the per-root \
                 counts index them by"
                    .into(),
            )
            .into());
        }

        let (first, len) = self.extent()?;
        let mut out = try_vec(len)?;
        out[..DIRECTORY_AT].copy_from_slice(&self.prefix);
        out[CHANNELS_AT..CHANNELS_AT + 2].copy_from_slice(&self.channels.to_be_bytes());
        out[STROKE_COUNT_AT..STROKE_COUNT_AT + 2].copy_from_slice(&count.to_be_bytes());
        for note in 0..NOTES {
            let n = self
                .strokes
                .iter()
                .filter(|s| usize::from(s.root) == note)
                .count();
            let n = u16::try_from(n).expect("a per-root count is at most the stroke count");
            let at = ROOT_COUNTS_AT + note * 2;
            out[at..at + 2].copy_from_slice(&n.to_be_bytes());
        }

        let mut at = first;
        for (i, stroke) in self.strokes.iter().enumerate() {
            let start = u32::try_from(at).map_err(|_| ParseError::OutOfBounds {
                value: format!("audio offset {at:#x}"),
                bound: "the u32 offset a stroke record holds".into(),
            })?;
            let record = DIRECTORY_AT + i * RECORD;
            out[record..record + RECORD].copy_from_slice(&stroke.record);
            out[record + REC_START..record + REC_START + 4].copy_from_slice(&start.to_be_bytes());
            out[at..at + stroke.audio.len()].copy_from_slice(stroke.audio);
            at += stroke.audio.len();
        }
        Ok(out)
    }

    /// The library as a file, ready to write. The container recomputes its own
    /// checksum.
    ///
    /// The u32 at body `0x06` is unique per file and is not a checksum, a size or a
    /// hash of anything in it; with nothing to recompute it from, an edit carries
    /// it over rather than inventing a value.
    pub fn to_piano(&self) -> Result<Piano, Error> {
        Ok(Piano {
            file: Cbin {
                header: self.header.clone(),
                body: RawBody(self.to_body()?),
            },
        })
    }
}

impl fmt::Debug for Library<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, variant) = self.name();
        f.debug_struct("npno::Library")
            .field("name", &name)
            .field("variant", &variant)
            .field(
                "stream_version",
                &format_args!("{:#06x}", self.stream_version()),
            )
            .field("channels", &self.channels)
            .field("strokes", &self.strokes.len())
            .field("roots", &self.roots().len())
            .finish()
    }
}

fn block_bytes(channels: u16) -> usize {
    codec::BLOCK_WORDS * 2 * usize::from(channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body shaped like a real one: prefix, directory and audio spans laid out
    /// by the same law the reader checks, with each stroke's audio filled with a
    /// byte naming it so a re-lay is visible.
    struct Build {
        version: u16,
        channels: u16,
        /// `(root, bank, layer, blocks)`, in ascending root order.
        strokes: Vec<(u8, u8, u8, u16)>,
        /// `(key, root)` routes.
        map: Vec<(u8, u8)>,
    }

    impl Build {
        fn new() -> Build {
            Build {
                version: 0x450,
                channels: 1,
                strokes: vec![(60, 0, 0, 1), (60, 2, 3, 1), (72, 0, 0, 2)],
                map: vec![(60, 60), (61, 60), (72, 72)],
            }
        }

        fn body(&self) -> Vec<u8> {
            let block = block_bytes(self.channels);
            let count = self.strokes.len();
            let directory_end = DIRECTORY_AT + count * RECORD;
            let first = first_audio_offset(directory_end, block).unwrap();
            let audio: usize = self
                .strokes
                .iter()
                .map(|&(_, _, _, blocks)| usize::from(blocks) * block)
                .sum();
            let mut body = vec![0u8; first + audio];
            body[..4].copy_from_slice(CNSP_MAGIC);
            body[VERSION_AT..VERSION_AT + 2].copy_from_slice(&self.version.to_be_bytes());
            body[VERSION_ECHO_AT..VERSION_ECHO_AT + 2].copy_from_slice(&self.version.to_be_bytes());
            body[CHANNELS_AT..CHANNELS_AT + 2].copy_from_slice(&self.channels.to_be_bytes());
            let name = b"Test Piano#Variant";
            body[0x1c..0x1c + name.len()].copy_from_slice(name);
            body[KEY_MAP_AT..KEY_MAP_AT + NOTES].fill(UNCOVERED);
            for &(key, root) in &self.map {
                body[KEY_MAP_AT + usize::from(key)] = root;
            }
            body[STROKE_COUNT_AT..STROKE_COUNT_AT + 2]
                .copy_from_slice(&(count as u16).to_be_bytes());
            for note in 0..NOTES {
                let n = self
                    .strokes
                    .iter()
                    .filter(|&&(root, ..)| usize::from(root) == note)
                    .count() as u16;
                let at = ROOT_COUNTS_AT + note * 2;
                body[at..at + 2].copy_from_slice(&n.to_be_bytes());
            }
            let mut at = first;
            for (i, &(_, bank, layer, blocks)) in self.strokes.iter().enumerate() {
                let rec = DIRECTORY_AT + i * RECORD;
                body[rec..rec + 4].copy_from_slice(&(at as u32).to_be_bytes());
                body[rec + REC_BANK] = bank;
                body[rec + REC_LAYER] = layer;
                body[rec + REC_BLOCKS..rec + REC_BLOCKS + 2].copy_from_slice(&blocks.to_be_bytes());
                body[rec + REC_ID..rec + REC_ID + 4].copy_from_slice(&(i as u32).to_be_bytes());
                let span = usize::from(blocks) * block;
                body[at..at + span].fill(0x40 + i as u8);
                at += span;
            }
            body
        }

        fn piano(&self) -> Piano {
            let body = self.body();
            Piano {
                file: Cbin {
                    header: Header::new(FORMAT, (0, 0), 530),
                    body: RawBody(body),
                },
            }
        }
    }

    #[test]
    fn the_name_field_splits_on_the_separator() {
        let piano = Build::new().piano();
        assert_eq!(piano.stream_version().unwrap(), 0x450);
        assert_eq!(
            piano.name().unwrap(),
            ("Test Piano".to_string(), "Variant".to_string())
        );
    }

    #[test]
    fn an_unknown_stream_version_still_round_trips_but_does_not_decode() {
        let mut build = Build::new();
        build.version = 0x500;
        let piano = build.piano();
        assert_eq!(piano.stream_version().unwrap(), 0x500);
        assert!(
            piano.name().is_err(),
            "the name offset is only pinned on known versions"
        );
        assert!(piano.key_map().is_err());
        assert!(piano.library().is_err());
    }

    #[test]
    fn a_body_without_the_magic_is_refused() {
        let mut piano = Build::new().piano();
        piano.file.body.0[0] = b'Q';
        assert!(piano.name().is_err(), "a non-CNSP body has no name to read");
    }

    #[test]
    fn a_library_rebuilds_to_the_bytes_it_was_read_from() {
        let piano = Build::new().piano();
        let rebuilt = piano.library().unwrap().to_body().unwrap();
        assert_eq!(rebuilt, piano.file.body.0);
    }

    #[test]
    fn the_directory_reports_each_strokes_root_bank_and_layer() {
        let piano = Build::new().piano();
        let library = piano.library().unwrap();
        let seen: Vec<(u8, Option<Bank>, u8)> = library
            .strokes()
            .iter()
            .map(|s| (s.root, s.bank(), s.layer()))
            .collect();
        assert_eq!(
            seen,
            [
                (60, Some(Bank::Attack), 0),
                (60, Some(Bank::Release), 3),
                (72, Some(Bank::Attack), 0),
            ]
        );
        assert_eq!(library.keys_for(60), [60, 61]);
    }

    #[test]
    fn a_stroke_whose_start_does_not_abut_the_one_before_is_refused() {
        let mut piano = Build::new().piano();
        let second = DIRECTORY_AT + RECORD;
        let start = be32(&piano.file.body.0, second + REC_START);
        piano.file.body.0[second..second + 4].copy_from_slice(&(start + 2).to_be_bytes());
        let error = piano.library().unwrap_err().to_string();
        assert!(error.contains("stroke 1 starts at"), "{error}");
    }

    #[test]
    fn a_key_routed_to_a_root_no_stroke_records_is_refused() {
        let mut build = Build::new();
        build.map.push((80, 80));
        let error = build.piano().library().unwrap_err().to_string();
        assert!(error.contains("key 80 plays root 80"), "{error}");
    }

    #[test]
    fn a_count_table_that_does_not_sum_to_the_stroke_count_is_refused() {
        let mut piano = Build::new().piano();
        let at = ROOT_COUNTS_AT + 60 * 2;
        piano.file.body.0[at..at + 2].copy_from_slice(&5u16.to_be_bytes());
        let error = piano.library().unwrap_err().to_string();
        assert!(error.contains("per-root counts sum to"), "{error}");
    }

    #[test]
    fn dropping_a_bank_relays_the_audio_and_leaves_the_rest_verbatim() {
        let piano = Build::new().piano();
        let before = piano.library().unwrap();
        let mut after = piano.library().unwrap();
        let change = after.drop_bank(Bank::Release);
        assert_eq!(
            change,
            Change {
                strokes_removed: 1,
                roots_removed: 0,
                keys_uncovered: 0
            }
        );

        let body = after.to_body().unwrap();
        let trimmed = Piano {
            file: Cbin {
                header: after.header.clone(),
                body: RawBody(body),
            },
        };
        let reparsed = trimmed.library().unwrap();
        assert_eq!(reparsed.strokes().len(), 2);
        for (kept, moved) in before
            .strokes()
            .iter()
            .filter(|s| s.bank() != Some(Bank::Release))
            .zip(reparsed.strokes())
        {
            assert_eq!(kept.audio(), moved.audio(), "a span moved verbatim");
            assert_eq!(kept.id(), moved.id());
            assert_eq!(&kept.record()[REC_BANK..], &moved.record()[REC_BANK..]);
        }
    }

    #[test]
    fn dropping_every_stroke_of_a_root_uncovers_the_keys_it_played() {
        let mut library_owner = Build::new();
        library_owner.strokes = vec![(60, 0, 0, 1), (72, 1, 0, 1)];
        let piano = library_owner.piano();
        let mut library = piano.library().unwrap();
        let change = library.drop_bank(Bank::Resonance);
        assert_eq!(change.strokes_removed, 1);
        assert_eq!(change.roots_removed, 1);
        assert_eq!(change.keys_uncovered, 1);
        assert_eq!(library.key_map()[72], UNCOVERED);
        library.to_body().unwrap();
    }

    #[test]
    fn keeping_the_loudest_layer_keeps_one_per_root_and_bank() {
        let mut build = Build::new();
        build.strokes = vec![
            (60, 0, 0, 1),
            (60, 0, 5, 1),
            (60, 2, 26, 1),
            (60, 2, 30, 1),
            (72, 0, 1, 1),
        ];
        let piano = build.piano();
        let mut library = piano.library().unwrap();
        library.keep_layers(&Layers::Loudest(1));
        let kept: Vec<(u8, u8, u8)> = library
            .strokes()
            .iter()
            .map(|s| (s.root, s.bank_code(), s.layer()))
            .collect();
        assert_eq!(kept, [(60, 0, 0), (60, 2, 26), (72, 0, 1)]);
    }

    #[test]
    fn keeping_named_layers_takes_them_wherever_they_occur() {
        let mut build = Build::new();
        build.strokes = vec![(60, 0, 0, 1), (60, 0, 5, 1), (72, 0, 5, 1)];
        let piano = build.piano();
        let mut library = piano.library().unwrap();
        library.keep_layers(&Layers::Only([5].into_iter().collect()));
        let kept: Vec<(u8, u8)> = library
            .strokes()
            .iter()
            .map(|s| (s.root, s.layer()))
            .collect();
        assert_eq!(kept, [(60, 5), (72, 5)]);
    }

    #[test]
    fn cutting_the_range_drops_the_roots_nothing_plays_any_more() {
        let piano = Build::new().piano();
        let mut library = piano.library().unwrap();
        let change = library.cut_range(0..=70);
        assert_eq!(change.keys_uncovered, 1);
        assert_eq!(change.roots_removed, 1);
        assert_eq!(library.roots(), [60].into_iter().collect());
        assert_eq!(library.key_map()[72], UNCOVERED);
        assert_eq!(library.key_map()[60], 60);
    }

    #[test]
    fn a_split_gives_each_half_the_roots_its_keys_play() {
        let piano = Build::new().piano();
        let (low, high) = piano.library().unwrap().split_at(70);
        assert_eq!(low.roots(), [60].into_iter().collect());
        assert_eq!(high.roots(), [72].into_iter().collect());
        assert_eq!(low.keys_for(60), [60, 61]);
        assert_eq!(high.keys_for(72), [72]);
        let audio: usize = piano
            .library()
            .unwrap()
            .strokes()
            .iter()
            .map(|s| s.audio().len())
            .sum();
        let halves: usize = [&low, &high]
            .iter()
            .flat_map(|l| l.strokes())
            .map(|s| s.audio().len())
            .sum();
        assert_eq!(
            halves, audio,
            "a split shares every stroke out exactly once"
        );
    }

    #[test]
    fn a_rename_leaves_the_variant_and_writes_both_fields_on_the_split_version() {
        let mut build = Build::new();
        build.version = VERSION_SPLIT_NAME;
        let piano = build.piano();
        let mut library = piano.library().unwrap();
        library.set_name("Renamed").unwrap();
        library.set_variant("Sml").unwrap();
        assert_eq!(library.name(), ("Renamed".into(), "Sml".into()));
        assert_eq!(TextField::NAME.read(&library.prefix), "Renamed");
        assert_eq!(TextField::VARIANT.read(&library.prefix), "Sml");
    }

    #[test]
    fn a_name_past_the_field_is_refused_without_changing_it() {
        let piano = Build::new().piano();
        let mut library = piano.library().unwrap();
        let too_long = "x".repeat(TextField::COMBINED.capacity());
        assert!(library.set_name(&too_long).is_err());
        assert_eq!(library.name().0, "Test Piano");
    }

    #[test]
    fn a_remap_to_a_root_the_directory_does_not_record_is_refused() {
        let piano = Build::new().piano();
        let mut library = piano.library().unwrap();
        assert!(library.set_key_root(64, Some(61)).is_err());
        library.set_key_root(64, Some(72)).unwrap();
        assert_eq!(library.keys_for(72), [64, 72]);
        library.set_key_root(64, None).unwrap();
        assert_eq!(library.keys_for(72), [72]);
    }

    #[test]
    fn fine_tune_reads_and_writes_the_per_key_byte() {
        let piano = Build::new().piano();
        let mut library = piano.library().unwrap();
        assert_eq!(library.fine_tune(60), 0);
        library.set_fine_tune(60, -4);
        assert_eq!(library.fine_tune(60), -4);
        assert_eq!(library.to_body().unwrap()[FINE_TUNE_AT + 60], 0xfc);
    }

    #[test]
    fn the_first_audio_offset_sits_on_the_block_grid_less_the_bias() {
        for block in [1022, 2044] {
            for count in [0usize, 1, 38, 2196] {
                let end = DIRECTORY_AT + count * RECORD;
                let at = first_audio_offset(end, block).unwrap();
                assert!(at >= end, "the audio never overlaps the directory");
                assert_eq!((at + AUDIO_ALIGN_BIAS) % block, 0);
                assert!(at - end < block, "no whole spare block in the gap");
            }
        }
    }
}
