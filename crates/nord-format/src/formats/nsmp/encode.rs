//! Building a v2 sample instrument from PCM — tier "instrument-valid".
//!
//! The inverse of [`codec`](super::codec), and honest about how far the inverse goes.
//! What this emits is a file whose container, section chain, stroke header, count
//! laws and record grammar are the format's, and whose audio is the source on the
//! field lattice quantised the way the instrument's encoder quantises. What it is
//! **not** is byte-identical to what Nord Sample Editor would produce for the same
//! input: the resampling [`kernel`](super::kernel) is an approximation, the rule the
//! editor uses to pick a quantiser shift is not known, and the encoder's own choice
//! of predictor order per record is reproduced only under [`Predictor::Minimising`].
//!
//! So three claims: a file from here **round-trips through this crate's own decoder
//! exactly** under either predictor, it obeys every structural law the format is known
//! to have, and — confirmed on hardware — **the Electro 5 loads and plays one**, under
//! either predictor, at the pitch the decoder renders.
//!
//! ```no_run
//! # use nord_format::formats::nsmp::encode;
//! let samples: Vec<i16> = vec![0; 44_100];
//! let options = encode::Options::new("Test").root_key(60);
//! let instrument = encode::instrument(&samples, &options).unwrap();
//! std::fs::write("test.nsmp", instrument.to_bytes().unwrap()).unwrap();
//! ```
//!
//! [`multi_zone`] is the same builder across a keyboard: one `stk` per zone, highest
//! zone first, each zone's record naming its stroke by the global id the caller gives
//! it. Zone counts move where a stroke's audio may start, so the allocation each stroke
//! is packed into comes from [`stroke::header_len`](super::stroke::header_len) rather
//! than from a constant.
//!
//! ⚠️ A stroke from here plays once and stops, where vendor content sustains: the
//! format's loop points are not located, so sustained material is cut short. Confirmed
//! on hardware.

use super::codec::{self, PITCH_DEN, PITCH_NUM, WRAP};
use super::kernel;
use super::section::{self, Section};
use super::stroke::PACKET_LEN;
use super::{Sample, MAX_NAME_LEN};
use crate::cbin::{Cbin, Generation, Header};
use crate::error::{Error, ParseError};

/// This writes v2 only, so the stroke header is always the narrow one.
const HEADER_LEN: usize = codec::Layout::V2.header_len();

/// Content version of the Sample Library 2.0 layout this writes.
const VERSION: u32 = 200;

/// Unexplained v2 sample-instrument `aux` value.
const AUX: u32 = 0x000f_0000;

/// Section schema versions, which do not track the content version.
const HDR_VERSION: u8 = 9;
const CAT_VERSION: u8 = 5;
const MAP_VERSION: u8 = 10;
const STK_VERSION: u8 = 9;
const STY_VERSION: u8 = 5;
const CONTAINER_VERSION: u8 = 11;

/// Fields per cell. Content records cover whole cells, which is why their counts are
/// always a multiple of it.
const CELL: usize = 24;

/// Cells one record may cover, from the 14-bit count field: `16368 / CELL`.
const MAX_CELLS: usize = 682;

/// Fields the 1:1 regime puts in one record. Warmup and resync split into chunks of
/// this with a remainder of at least 25, which the count laws guarantee.
const CHUNK: usize = 32;

/// Widest emitted field; quantisation shifts values until they fit.
const MAX_WIDTH: u8 = 13;

/// Narrowest field. Width 2 is the draft the encoder codes everything at before it
/// promotes anything, and a width-1 flag-1 record is the terminator.
const MIN_WIDTH: u8 = 2;

/// Absolute field ceiling imposed by the stream directory and minimum width.
const MAX_FIELDS: usize = MAX_STREAM_WORDS * 24 / MIN_WIDTH as usize;

/// Zones one instrument may hold, from the `map` section's single count byte.
const MAX_ZONES: usize = u8::MAX as usize;

/// The widest stroke id a zone record can name: the field is one byte, and zero is
/// not an id the editor issues.
const MAX_STROKE_ID: u32 = u8::MAX as u32;

/// Source samples the kernel is allowed to ring out past the end of the input.
const RING_OUT: usize = 160;

/// Resync position ratio: `R1 = round(63·frames/634)`.
const RHO_NUM: u64 = 63;
const RHO_DEN: u64 = 634;

/// Shortest modelled input; shorter streams use an unresolved opening.
pub const MIN_FRAMES: usize = 4096;

/// Longest input the stroke header's 16-bit word directory can address unambiguously.
const MAX_STREAM_WORDS: usize = WRAP;

/// Backward-difference coefficients for predictor orders 0 to 4.
const DIFFERENCE: [&[i32]; 5] = [
    &[1],
    &[1, -1],
    &[1, -2, 1],
    &[1, -3, 3, -1],
    &[1, -4, 6, -4, 1],
];

/// How content records code their fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Predictor {
    /// Store every content field outright at order zero.
    #[default]
    Plain,
    /// Choose the narrowest predictor per cell, breaking ties by residual sum.
    /// Smaller than plain records and exact through this crate's decoder.
    Minimising,
}

/// What to build around the audio.
#[derive(Debug, Clone)]
pub struct Options {
    name: String,
    root_key: u8,
    top_note: Option<u8>,
    predictor: Predictor,
}

impl Options {
    /// Defaults: the name given, root key C4, the editor's own top note, plain records.
    pub fn new(name: impl Into<String>) -> Options {
        Options {
            name: name.into(),
            root_key: 60,
            top_note: None,
            predictor: Predictor::Plain,
        }
    }

    /// The MIDI note the sample plays untransposed at.
    pub fn root_key(mut self, note: u8) -> Options {
        self.root_key = note;
        self
    }

    /// The highest note the zone covers. Defaults to two octaves above the root, which
    /// is the layout the editor lays down for a single zone.
    pub fn top_note(mut self, note: u8) -> Options {
        self.top_note = Some(note);
        self
    }

    pub fn predictor(mut self, predictor: Predictor) -> Options {
        self.predictor = predictor;
        self
    }

    fn resolved_top_note(&self) -> u8 {
        self.top_note
            .unwrap_or_else(|| self.root_key.saturating_add(24).min(127))
    }
}

/// Stroke landmarks derived from the source frame count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Source frames the stroke covers.
    pub frames: usize,
    /// Fields in the stream — the source plus a ring-out past its end.
    pub fields: usize,
    /// Field the resync record starts at.
    pub resync_at: usize,
    /// Fields in the opening 1:1 run.
    pub warmup: usize,
    /// Fields in the resync 1:1 run.
    pub resync: usize,
    /// Content cells between the warmup and the resync.
    pub cells_before: usize,
    /// Content cells between the resync and the terminator.
    pub cells_after: usize,
}

impl Plan {
    /// The layout for `frames` source samples.
    pub fn new(frames: usize) -> Result<Plan, Error> {
        if frames < MIN_FRAMES {
            return Err(ParseError::OutOfBounds {
                value: format!("{frames} frames"),
                bound: format!(
                    "the modelled range: at least {MIN_FRAMES} frames, below which the \
                     stream opens a way this crate has not modelled"
                ),
            }
            .into());
        }
        let frames = u64::try_from(frames).map_err(|_| size_error(frames))?;
        let fields = frames
            .checked_add(RING_OUT as u64)
            .and_then(|n| n.checked_mul(u64::from(PITCH_DEN)))
            .and_then(|n| round_ratio(n, u64::from(PITCH_NUM)))
            .ok_or_else(|| size_error(frames as usize))?;
        if fields > MAX_FIELDS {
            return Err(size_error(frames as usize).into());
        }
        let resync_at = frames
            .checked_mul(RHO_NUM)
            .and_then(|n| round_ratio(n, RHO_DEN))
            .ok_or_else(|| size_error(frames as usize))?;
        let warmup = band(resync_at);
        let resync = band(fields - warmup);
        if resync_at < warmup || fields < resync_at + resync {
            return Err(ParseError::AssertFail(format!(
                "{frames} frames put the resync at field {resync_at} of {fields}, which \
                 leaves no room for the 1:1 runs around it"
            ))
            .into());
        }
        Ok(Plan {
            frames: frames as usize,
            fields,
            resync_at,
            warmup,
            resync,
            cells_before: (resync_at - warmup) / CELL,
            cells_after: (fields - resync_at - resync) / CELL,
        })
    }
}

/// `round(num/den)`, half away from zero, on non-negative integers.
fn round_ratio(num: u64, den: u64) -> Option<usize> {
    num.checked_add(den / 2)
        .and_then(|n| usize::try_from(n / den).ok())
}

fn size_error(frames: usize) -> ParseError {
    ParseError::OutOfBounds {
        value: format!("{frames} frames"),
        bound: format!("audio whose encoded stream fits {MAX_STREAM_WORDS} words"),
    }
}

/// The 25..=96-field 1:1 run that preserves a landmark's cell phase.
fn band(r: usize) -> usize {
    let residue = (r % CELL + CELL - 1) % CELL + 1;
    residue + CELL * ((residue - 1) / 8 + 1)
}

/// Split a 1:1 run into legal 25..=32-field records.
fn chunks(mut n: usize) -> Vec<usize> {
    let mut out = Vec::new();
    while n > CHUNK {
        out.push(CHUNK);
        n -= CHUNK;
    }
    out.push(n);
    out
}

/// The source on the lattice, quantised — the stream's field values and the two
/// header statistics that describe them.
#[derive(Debug, Clone)]
struct Quantised {
    /// One stored value per field, sign-extended and inside [`MAX_WIDTH`] bits.
    values: Vec<i32>,
    /// Bits the values were shifted right by. Dequantising shifts back.
    shift: i32,
    /// Statistic B: the largest content field taken at a fixed shift of 2.
    peak: u32,
}

/// Resample and choose the smallest nonnegative shift that fits [`MAX_WIDTH`].
/// The instrument's shift-selection rule remains unknown.
fn quantise(source: &[i16], plan: &Plan) -> Quantised {
    let raw: Vec<i64> = (0..plan.fields).map(|f| kernel::field(source, f)).collect();
    let low = raw.iter().copied().min().unwrap_or(0);
    let high = raw.iter().copied().max().unwrap_or(0);

    let mut shift = 0i32;
    while width_of(low >> shift, high >> shift) > MAX_WIDTH {
        shift += 1;
    }

    // Statistic B is taken at a fixed shift of two and over content fields only, which
    // is why a value the 1:1 regime carries never sets it.
    let content =
        |f: usize| (f >= plan.warmup && f < plan.resync_at) || f >= plan.resync_at + plan.resync;
    let peak = raw
        .iter()
        .enumerate()
        .filter(|&(f, _)| content(f))
        .map(|(_, &v)| (v >> 2).unsigned_abs())
        .max()
        .unwrap_or(0)
        .min(u64::from(u32::MAX >> 8)) as u32;

    Quantised {
        values: raw.iter().map(|&v| (v >> shift) as i32).collect(),
        shift,
        peak,
    }
}

/// Bits a two's-complement field needs to hold everything in `low..=high`, floored at
/// [`MIN_WIDTH`].
fn width_of(low: i64, high: i64) -> u8 {
    let mut w = MIN_WIDTH;
    while w < 16 && (low < -(1i64 << (w - 1)) || high > (1i64 << (w - 1)) - 1) {
        w += 1;
    }
    w
}

/// One record, before it becomes words.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spec {
    one_to_one: bool,
    width: u8,
    order: u8,
    first: usize,
    count: usize,
}

impl Spec {
    /// Words this record occupies, header included.
    fn span(&self) -> usize {
        (24 + self.count * usize::from(self.width)).div_ceil(24)
    }
}

/// The Nth backward difference at `at`, across record boundaries.
fn residual(values: &[i32], at: usize, order: u8) -> i64 {
    DIFFERENCE[usize::from(order)]
        .iter()
        .enumerate()
        .map(|(j, &c)| match at.checked_sub(j) {
            Some(k) => i64::from(c) * i64::from(values[k]),
            None => 0,
        })
        .sum()
}

/// The width one cell needs at `order`, and the sum of the residuals it would store.
fn cost(values: &[i32], first: usize, order: u8) -> (u8, u64) {
    let mut low = 0i64;
    let mut high = 0i64;
    let mut total = 0u64;
    for at in first..first + CELL {
        let e = residual(values, at, order);
        low = low.min(e);
        high = high.max(e);
        total += e.unsigned_abs();
    }
    (width_of(low, high), total)
}

/// The predictor order one cell codes narrowest at, tie broken by the smallest sum of
/// residuals and then by the lowest order.
fn best_order(values: &[i32], first: usize, predictor: Predictor) -> (u8, u8) {
    let plain = cost(values, first, 0);
    if predictor == Predictor::Plain {
        return (0, plain.0);
    }
    let mut best = (plain.0, plain.1, 0u8);
    for order in 1..DIFFERENCE.len() as u8 {
        let (width, total) = cost(values, first, order);
        if (width, total) < (best.0, best.1) {
            best = (width, total, order);
        }
    }
    (best.2, best.0)
}

/// Partition 1:1 values and like-coded content cells into records.
fn records(values: &[i32], plan: &Plan, predictor: Predictor) -> Vec<Spec> {
    let mut out = Vec::new();
    let mut at = 0usize;

    let one_to_one = |out: &mut Vec<Spec>, at: &mut usize, fields: usize| {
        for count in chunks(fields) {
            let mut low = 0i64;
            let mut high = 0i64;
            for &v in &values[*at..*at + count] {
                low = low.min(i64::from(v));
                high = high.max(i64::from(v));
            }
            out.push(Spec {
                one_to_one: true,
                width: width_of(low, high),
                order: 0,
                first: *at,
                count,
            });
            *at += count;
        }
    };

    let content = |out: &mut Vec<Spec>, at: &mut usize, cells: usize| {
        let mut cell = 0usize;
        while cell < cells {
            let (order, width) = best_order(values, *at + cell * CELL, predictor);
            let mut run = 1usize;
            while run < MAX_CELLS
                && cell + run < cells
                && best_order(values, *at + (cell + run) * CELL, predictor) == (order, width)
            {
                run += 1;
            }
            out.push(Spec {
                one_to_one: false,
                width,
                order,
                first: *at + cell * CELL,
                count: run * CELL,
            });
            cell += run;
        }
        *at += cells * CELL;
    };

    one_to_one(&mut out, &mut at, plan.warmup);
    content(&mut out, &mut at, plan.cells_before);
    let resync_record = out.len();
    one_to_one(&mut out, &mut at, plan.resync);
    content(&mut out, &mut at, plan.cells_after);
    debug_assert_eq!(at, plan.fields);
    debug_assert!(resync_record < out.len());
    out
}

/// A packed stroke stream: the words, and where the header's directory points.
struct Stream {
    words: Vec<u8>,
    first_record: usize,
    resync: usize,
    terminator: usize,
}

/// Right-align records in the allocation the preamble law gives this stroke:
/// `preamble` bytes of payload, then whole packets until the chain fits.
///
/// `preamble` is [`stroke::header_len`](super::stroke::header_len), which a zone table
/// can drive below [`HEADER_LEN`] — the first packet then starts inside what would
/// otherwise be header, and the loop repays the difference.
fn pack(
    specs: &[Spec],
    values: &[i32],
    resync_record: usize,
    preamble: usize,
) -> Result<Stream, Error> {
    let chain: usize = specs.iter().map(Spec::span).sum::<usize>() + 1;
    let need = chain
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(HEADER_LEN))
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("a chain of {chain} words"),
            bound: "a stroke payload of addressable length".into(),
        })?;
    let mut payload = preamble;
    while payload < need {
        payload += PACKET_LEN;
    }
    if !(payload - HEADER_LEN).is_multiple_of(3) {
        return Err(ParseError::AssertFail(format!(
            "a {preamble}-byte preamble puts the word stream off a word boundary; the \
             sections in front of the stroke are not whole words"
        ))
        .into());
    }
    let total = (payload - HEADER_LEN) / 3;
    if total > MAX_STREAM_WORDS {
        return Err(ParseError::OutOfBounds {
            value: format!("a stream of {total} words"),
            bound: format!(
                "{MAX_STREAM_WORDS} words, the reach of the stroke header's 16-bit word \
                 directory; shorten the source or code it with {:?}, which is several \
                 times denser on anything smooth",
                Predictor::Minimising
            ),
        }
        .into());
    }

    let mut words = vec![0u8; total * 3];
    let lead = total - chain;
    let mut at = lead;
    let mut resync = lead;
    for (index, spec) in specs.iter().enumerate() {
        if index == resync_record {
            resync = at;
        }
        write_record(&mut words, at, spec, values);
        at += spec.span();
    }
    words[at * 3..at * 3 + 3].copy_from_slice(&[0x80, 0x00, CELL as u8]);
    debug_assert_eq!(at + 1, total);

    Ok(Stream {
        words,
        first_record: lead,
        resync,
        terminator: at,
    })
}

/// Writes one record: its header word, then its fields, which start at the first bit
/// after it. Any alignment tail is left zero at the end of the segment.
fn write_record(words: &mut [u8], at: usize, spec: &Spec, values: &[i32]) {
    let head = (u32::from(spec.one_to_one) << 23)
        | (u32::from(spec.width - 1) << 19)
        | (u32::from(spec.order) << 14)
        | spec.count as u32;
    words[at * 3..at * 3 + 3].copy_from_slice(&head.to_be_bytes()[1..]);

    let mut bit = at * 24 + 24;
    for k in 0..spec.count {
        let field = spec.first + k;
        let value = if spec.order == 0 {
            i64::from(values[field])
        } else {
            residual(values, field, spec.order)
        };
        let raw = (value as u64) & ((1u64 << spec.width) - 1);
        for b in (0..spec.width).rev() {
            if raw >> b & 1 != 0 {
                words[bit / 8] |= 1 << (7 - bit % 8);
            }
            bit += 1;
        }
    }
}

/// Encode `A = 2^(41+s)/peak` so its exponent carries the quantiser shift.
fn statistic_a(peak: u32, shift: i32) -> (u32, u8) {
    let peak = u64::from(peak.max(1));
    let bits = 64 - peak.leading_zeros() as i32;
    let exact_power = i32::from(peak.is_power_of_two());
    let mantissa = (1u64 << (18 + bits + (1 - exact_power))) / peak;
    (mantissa as u32, (22 + shift - bits + exact_power) as u8)
}

/// Build the fixed header and its body-relative, wrapping word directory.
fn stroke_header(id: u32, root_key: u8, q: &Quantised, stream: &Stream, body_at: usize) -> Vec<u8> {
    let mut head = vec![0u8; HEADER_LEN];
    head[0..4].copy_from_slice(&id.to_be_bytes());
    head[5] = root_key;
    // Unexplained: constant on every corpus stroke.
    head[6..9].copy_from_slice(&[0x88, 0xba, 0x01]);

    let (mantissa, exponent) = statistic_a(q.peak, q.shift);
    head[9..12].copy_from_slice(&mantissa.to_be_bytes()[1..]);
    head[12] = exponent;
    head[13..16].copy_from_slice(&q.peak.to_be_bytes()[1..]);

    let base = (body_at + HEADER_LEN) / 3 % WRAP;
    let pointer = |word: usize| ((base + word) % WRAP) as u16;
    let directory = [
        pointer(stream.first_record),
        pointer(stream.resync),
        pointer(stream.terminator),
        pointer(stream.terminator),
    ];
    for (i, p) in directory.iter().enumerate() {
        let at = 20 + 9 * i;
        head[at..at + 2].copy_from_slice(&p.to_be_bytes());
        // Unexplained: a `0x80` trails the first three pointers and not the fourth,
        // which is the last field in the header.
        if i < 3 {
            head[at + 2] = 0x80;
        }
    }
    head
}

/// Encode one zone's stroke at body offset `body_at`, packed into `preamble` bytes
/// plus whole packets.
///
/// Both placements come from the sections already sized in front of this stroke, so
/// only [`multi_zone`] can supply them: `body_at` is the base the word directory is
/// written against, and a wrong one produces a file whose directory names records
/// that are not there.
fn stroke(
    source: &[i16],
    root_key: u8,
    id: u32,
    body_at: usize,
    preamble: usize,
    predictor: Predictor,
) -> Result<Vec<u8>, Error> {
    midi_note("root key", root_key)?;
    body_at
        .checked_add(HEADER_LEN)
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("body offset {body_at}"),
            bound: "an addressable stroke header".into(),
        })?;
    let plan = Plan::new(source.len())?;
    let q = quantise(source, &plan);
    let specs = records(&q.values, &plan, predictor);
    let resync_record = specs
        .iter()
        .position(|s| s.first == plan.resync_at)
        .unwrap_or(0);
    let stream = pack(&specs, &q.values, resync_record, preamble)?;

    let mut payload = stroke_header(id, root_key, &q, &stream, body_at);
    payload.extend_from_slice(&stream.words);
    Ok(payload)
}

/// The `hdr` section: a fixed prefix, then the instrument name NUL-padded.
fn hdr(name: &str) -> Result<Section, Error> {
    if name.len() > MAX_NAME_LEN {
        return Err(ParseError::OutOfBounds {
            value: format!("{name:?} ({} bytes)", name.len()),
            bound: format!("a name of at most {MAX_NAME_LEN} bytes"),
        }
        .into());
    }
    let mut payload = vec![0u8; 111];
    // Unexplained: constant on every corpus specimen.
    payload[0..6].copy_from_slice(&[0x00, 0x01, 0xb4, 0x00, 0x06, 0x50]);
    payload[12..12 + name.len()].copy_from_slice(name.as_bytes());
    Ok(Section {
        tag: *section::HDR,
        version: HDR_VERSION,
        payload,
    })
}

/// The `cat` section: a short prefix and two length-prefixed labels.
fn cat() -> Section {
    let mut payload = vec![0x0f, 0x00, 0x00, 0x00, 0x01];
    for label in [&b"Production"[..], &b"Origin"[..]] {
        payload.push(label.len() as u8);
        payload.extend_from_slice(label);
    }
    payload.push(0);
    Section {
        tag: *section::CAT,
        version: CAT_VERSION,
        payload,
    }
}

/// Build the unexplained fixed keyboard map and zone table.
///
/// `zones` is `(stroke id, top note)` per zone, already high to low.
fn map(zones: &[(u8, u8)]) -> Section {
    let mut payload = vec![0u8; super::zone::RECORDS_AT + super::zone::RECORD_LEN * zones.len()];
    payload[0] = 0x10;
    for note in 0..128 {
        payload[15 + 6 * note] = 0x10;
    }
    payload[super::zone::COUNT_AT] = zones.len() as u8;
    // Zones are stored high to low by top note.
    for (index, &(id, top_note)) in zones.iter().enumerate() {
        let at = super::zone::RECORDS_AT + super::zone::RECORD_LEN * index;
        payload[at + 2] = id;
        // A looping zone holds 0x11 here and fills the two bytes after it; nothing
        // written from PCM loops, so the flag stays plain and they stay zero.
        payload[at + 3] = 0x10;
        payload[at + 9] = top_note;
        payload[at + 11] = 0x01;
    }
    Section {
        tag: *section::MAP,
        version: MAP_VERSION,
        payload,
    }
}

/// The `sty` section. Unexplained: nine constant bytes, never seen to vary.
fn sty() -> Section {
    Section {
        tag: *section::STY,
        version: STY_VERSION,
        payload: vec![0x00, 0x01, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00],
    }
}

/// One zone to build: its audio, where it sits on the keyboard, and the id its
/// record names its stroke by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewZone<'a> {
    /// Mono PCM at [`codec::SOURCE_RATE`], already trimmed to what the zone plays.
    pub source: &'a [i16],
    /// The note this sample plays untransposed at.
    pub root_key: u8,
    /// Highest note this zone answers to. Stored as given — the file keeps top notes,
    /// it does not derive them from the root keys.
    pub top_note: u8,
    /// The stroke's global id, 1 through [`MAX_STROKE_ID`]. Zones name their strokes
    /// by it rather than by position, so it need not run parallel to the sections.
    pub global_id: u32,
}

/// Build a one-zone v2 instrument from mono PCM at [`codec::SOURCE_RATE`].
/// Refuses unmodelled lengths, invalid metadata, and streams past the directory limit.
pub fn instrument(source: &[i16], options: &Options) -> Result<Cbin<Sample>, Error> {
    midi_note("root key", options.root_key)?;
    multi_zone(
        &[NewZone {
            source,
            root_key: options.root_key,
            top_note: options.resolved_top_note(),
            global_id: 1,
        }],
        &options.name,
        options.predictor,
    )
}

/// Build a v2 instrument that spans the keyboard: one `stk` per zone, in the order
/// given, which must be highest zone first.
///
/// Refuses an empty or overlapping zone list, a duplicate or unnameable stroke id,
/// and everything [`instrument`] refuses about one zone's audio.
pub fn multi_zone(
    zones: &[NewZone<'_>],
    name: &str,
    predictor: Predictor,
) -> Result<Cbin<Sample>, Error> {
    let table = zone_table(zones)?;
    let hdr = hdr(name)?;
    let cat = cat();
    let map = map(&table);
    // The directory a stroke carries counts words from the start of the body, and these
    // two decide where the first packet may start, so both are sized before any stream
    // is written.
    let cat_len = cat.payload.len();
    let map_len = map.payload.len();

    let mut sections = vec![
        Section {
            tag: *section::CONTAINER,
            version: CONTAINER_VERSION,
            payload: Vec::new(),
        },
        hdr,
        cat,
        map,
    ];
    let mut body_at: usize = sections.iter().map(Section::encoded_len).sum();
    for (index, zone) in zones.iter().enumerate() {
        let payload = stroke(
            zone.source,
            zone.root_key,
            zone.global_id,
            body_at + section::HEADER_LEN,
            super::stroke::header_len(index, cat_len, map_len),
            predictor,
        )?;
        body_at += section::HEADER_LEN + payload.len();
        sections.push(Section {
            tag: *section::STK,
            version: STK_VERSION,
            payload,
        });
    }
    sections.push(sty());

    Ok(Cbin {
        header: Header {
            generation: Generation::V1,
            tag: *b"nsmp",
            location: 0xFFFF_FFFF,
            aux: AUX,
            version: VERSION,
        },
        body: Sample { sections },
    })
}

/// Validate the zone list and reduce it to the `(stroke id, top note)` pairs the
/// `map` section stores.
fn zone_table(zones: &[NewZone<'_>]) -> Result<Vec<(u8, u8)>, Error> {
    if zones.is_empty() || zones.len() > MAX_ZONES {
        return Err(ParseError::OutOfBounds {
            value: format!("{} zones", zones.len()),
            bound: format!("1 through {MAX_ZONES}, the map section's own count byte"),
        }
        .into());
    }
    let mut table = Vec::with_capacity(zones.len());
    for (index, zone) in zones.iter().enumerate() {
        midi_note("root key", zone.root_key)?;
        midi_note("top note", zone.top_note)?;
        if !(1..=MAX_STROKE_ID).contains(&zone.global_id) {
            return Err(ParseError::OutOfBounds {
                value: format!("stroke id {}", zone.global_id),
                bound: format!("1 through {MAX_STROKE_ID}, what a zone record can name"),
            }
            .into());
        }
        let id = zone.global_id as u8;
        if table.iter().any(|&(seen, _)| seen == id) {
            return Err(ParseError::AssertFail(format!(
                "two zones claim stroke id {id}, and a zone record names its stroke by id"
            ))
            .into());
        }
        if index > 0 && zone.top_note >= zones[index - 1].top_note {
            return Err(ParseError::AssertFail(format!(
                "zone {index} reaches up to note {} but the zone before it stops at {}; \
                 zones are stored highest first and may not overlap",
                zone.top_note,
                zones[index - 1].top_note
            ))
            .into());
        }
        table.push((id, zone.top_note));
    }
    Ok(table)
}

fn midi_note(name: &str, note: u8) -> Result<(), Error> {
    if note <= 127 {
        return Ok(());
    }
    Err(ParseError::OutOfBounds {
        value: format!("{name} {note}"),
        bound: "a MIDI note from 0 through 127".into(),
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::super::codec;
    use super::*;

    /// 44100 Hz mono, one second, at `hz` and `amplitude`.
    fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<i16> {
        (0..frames)
            .map(|k| {
                let t = k as f64 / f64::from(codec::SOURCE_RATE);
                (amplitude * (2.0 * std::f64::consts::PI * hz * t).sin()).round() as i16
            })
            .collect()
    }

    fn encoded(source: &[i16], predictor: Predictor) -> Cbin<Sample> {
        instrument(source, &Options::new("Test").predictor(predictor)).unwrap()
    }

    #[test]
    fn the_band_lands_in_the_three_windows_the_laws_allow() {
        for r in 0..2000usize {
            let b = band(r);
            assert_eq!(b % CELL, r % CELL, "r {r}");
            assert!(
                (25..=32).contains(&b) || (57..=64).contains(&b) || (89..=96).contains(&b),
                "band({r}) = {b}"
            );
        }
    }

    /// A run splits into records of 25 to 32 fields — the counts the format shows and
    /// nothing between.
    #[test]
    fn every_one_to_one_chunk_is_a_legal_count() {
        for r in 0..2000usize {
            for c in chunks(band(r)) {
                assert!((25..=32).contains(&c), "band({r}) chunk {c}");
            }
        }
    }

    /// The plan's landmarks partition the field lattice exactly: warmup, cells, resync,
    /// cells, and nothing left over.
    #[test]
    fn the_plan_covers_every_field_exactly_once() {
        for frames in [4096, 8192, 10_000, 44_100, 100_000, 441_000] {
            let p = Plan::new(frames).unwrap();
            assert_eq!(
                p.warmup + CELL * p.cells_before + p.resync + CELL * p.cells_after,
                p.fields,
                "{frames} frames"
            );
            assert_eq!(p.warmup + CELL * p.cells_before, p.resync_at);
        }
    }

    #[test]
    fn short_input_is_refused_rather_than_guessed_at() {
        assert!(Plan::new(MIN_FRAMES - 1).is_err());
        assert!(Plan::new(MIN_FRAMES).is_ok());
        assert!(Plan::new(usize::MAX).is_err());
        assert!(instrument(&vec![0i16; 1024], &Options::new("Test")).is_err());
    }

    #[test]
    fn midi_notes_outside_the_wire_range_are_refused() {
        let source = vec![0i16; MIN_FRAMES];
        assert!(instrument(&source, &Options::new("Test").root_key(128)).is_err());
        assert!(instrument(&source, &Options::new("Test").top_note(255)).is_err());
        assert!(stroke(&source, 128, 1, 0, 165, Predictor::Plain).is_err());
    }

    /// Encoded audio begins where the preamble law puts it: a stroke payload is its
    /// own header length plus whole packets, and the chain sits at the end of it.
    #[test]
    fn the_allocation_is_whole_packets_with_the_chain_at_the_end() {
        let file = encoded(&sine(440.0, 8000.0, 44_100), Predictor::Plain);
        let map_len = section::find(&file.body.sections, section::MAP)
            .unwrap()
            .payload
            .len();
        let cat_len = section::find(&file.body.sections, section::CAT)
            .unwrap()
            .payload
            .len();
        let stroke = section::find(&file.body.sections, section::STK).unwrap();
        let head = super::super::stroke::header_len(0, cat_len, map_len);
        assert_eq!((stroke.payload.len() - head) % PACKET_LEN, 0);
        assert_eq!(&stroke.payload[stroke.payload.len() - 3..], &[0x80, 0, 24]);
    }

    #[test]
    fn every_predictor_round_trips_through_the_decoder_exactly() {
        let mut differenced = 0usize;
        for predictor in [Predictor::Plain, Predictor::Minimising] {
            for source in [
                sine(440.0, 12_000.0, 44_100),
                sine(30.0, 32_000.0, 20_000),
                vec![0i16; 8192],
                vec![9000i16; 8192],
            ] {
                let file = encoded(&source, predictor);
                let (at, stroke) = file.stroke_streams()[0];
                let plan = Plan::new(source.len()).unwrap();
                let q = quantise(&source, &plan);

                let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
                assert_eq!(audio.samples.len(), plan.fields);
                if predictor == Predictor::Plain {
                    assert_eq!(audio.differenced, 0);
                } else {
                    differenced += audio.differenced;
                }
                let gain = 1i32 << q.shift;
                for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
                    assert_eq!(i32::from(got), want * gain, "{predictor:?} field {f}");
                }
            }
        }
        assert!(differenced > 0, "minimising never chose a predictor");
    }

    /// The audio survives the trip as audio, not only as numbers: a sine comes back a
    /// sine of the same amplitude on the lattice's own rate.
    #[test]
    fn a_sine_comes_back_a_sine() {
        let source = sine(440.0, 20_000.0, 44_100);
        let file = encoded(&source, Predictor::Plain);
        let (at, stroke) = file.stroke_streams()[0];
        let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
        // Well inside the source, away from the ends the kernel rings at.
        let window = &audio.samples[10_000..20_000];
        let peak = window.iter().map(|&v| i32::from(v).abs()).max().unwrap();
        assert!((19_000..=21_000).contains(&peak), "peak {peak}");
        let zero_crossings = window.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        // 10000 fields at 35002 Hz is 0.2857 s, which holds 125.7 cycles of 440 Hz.
        assert!((124..=127).contains(&zero_crossings), "{zero_crossings}");
    }

    #[test]
    fn a_records_fields_start_right_after_its_header() {
        // 30 fields of 13 bits is 390, leaving 18 spare bits in 18 words.
        let spec = Spec {
            one_to_one: true,
            width: 13,
            order: 0,
            first: 0,
            count: 30,
        };
        let tail = spec.span() * 24 - 24 - spec.count * usize::from(spec.width);
        assert_eq!(tail, 18, "this spec is chosen to leave a tail");

        let values: Vec<i32> = (0..30).map(|k| k * 7 - 40).collect();
        let mut words = vec![0u8; spec.span() * 3];
        write_record(&mut words, 0, &spec, &values);

        // The tail is the last `tail` bits of the segment, and nothing is in it.
        let total = spec.span() * 24;
        for bit in total - tail..total {
            assert_eq!(
                words[bit / 8] >> (7 - bit % 8) & 1,
                0,
                "bit {bit} is in the alignment tail and should be clear"
            );
        }
        // And the reader agrees about where the values are.
        let mut stroke = vec![0u8; HEADER_LEN];
        stroke.extend_from_slice(&words);
        stroke.extend_from_slice(&[0x80, 0x00, 0x18]);
        let end = (HEADER_LEN / 3 + spec.span()) as u16;
        for (i, p) in [HEADER_LEN as u16 / 3, 0, end, end].iter().enumerate() {
            stroke[20 + 9 * i..22 + 9 * i].copy_from_slice(&p.to_be_bytes());
        }
        let walked = codec::walk(&stroke, 0, codec::Layout::V2).unwrap();
        assert_eq!(walked.records[0].values, values);
    }

    /// The file is a file: it parses, checksums, and round-trips as bytes.
    #[test]
    fn the_instrument_reads_back_as_one() {
        let file = instrument(
            &sine(220.0, 15_000.0, 30_000),
            &Options::new("Encoded").root_key(48).top_note(72),
        )
        .unwrap();
        let bytes = file.to_bytes().unwrap();
        let read = super::super::from_bytes(&bytes).unwrap();
        assert_eq!(read.name().unwrap(), "Encoded");
        assert_eq!(read.header.version, VERSION);
        let zones = read.zones().unwrap();
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].top_note, 72);
        assert_eq!(read.strokes().unwrap()[0].root_key, 48);
        assert_eq!(read.to_bytes().unwrap(), bytes);
    }

    /// The word directory is written against the body offset the stroke actually lands
    /// at, and the walk lands on the records it names.
    #[test]
    fn the_directory_names_the_records_the_walk_finds() {
        let file = encoded(&sine(300.0, 9000.0, 50_000), Predictor::Plain);
        let (at, stroke) = file.stroke_streams()[0];
        let stream = codec::walk(stroke, at, codec::Layout::V2).unwrap();
        let directory = codec::Directory::read(stroke).unwrap();
        assert_eq!(
            codec::Directory::resolve(directory.first_record, at, codec::Layout::V2),
            stream.first_record
        );
        assert_eq!(
            codec::Directory::resolve(directory.terminator, at, codec::Layout::V2),
            stream.terminator
        );
        let resync = codec::Directory::resolve(directory.resync, at, codec::Layout::V2);
        let record = stream.records.iter().find(|r| r.at == resync).unwrap();
        assert!(record.one_to_one);
        assert_eq!(record.first_field, Plan::new(50_000).unwrap().resync_at);
    }

    /// The shift is stated in the header, so it reads back whatever rule chose it.
    #[test]
    fn the_header_states_the_shift_it_quantised_at() {
        for amplitude in [40.0, 900.0, 8000.0, 32_000.0] {
            let source = sine(440.0, amplitude, 20_000);
            let plan = Plan::new(source.len()).unwrap();
            let q = quantise(&source, &plan);
            let file = encoded(&source, Predictor::Plain);
            let (_, stroke) = file.stroke_streams()[0];
            assert_eq!(
                codec::shift(stroke, codec::Layout::V2),
                Some(q.shift),
                "amplitude {amplitude}"
            );
            assert_eq!(
                codec::peak(stroke, codec::Layout::V2),
                i32::try_from(q.peak).ok()
            );
            assert!(q.shift >= 0);
        }
    }

    /// Loud material costs a shift; quiet material does not.
    #[test]
    fn the_shift_tracks_how_loud_the_content_is() {
        let quiet = Plan::new(20_000)
            .map(|p| quantise(&sine(440.0, 500.0, 20_000), &p).shift)
            .unwrap();
        let loud = Plan::new(20_000)
            .map(|p| quantise(&sine(440.0, 32_000.0, 20_000), &p).shift)
            .unwrap();
        assert_eq!(quiet, 0);
        assert!(loud > quiet, "loud {loud} vs quiet {quiet}");
    }

    /// Every stored field fits the width its record declares, at every predictor.
    #[test]
    fn no_field_overflows_the_width_its_record_declares() {
        for predictor in [Predictor::Plain, Predictor::Minimising] {
            let source = sine(440.0, 32_000.0, 30_000);
            let plan = Plan::new(source.len()).unwrap();
            let q = quantise(&source, &plan);
            for spec in records(&q.values, &plan, predictor) {
                let limit = 1i64 << (spec.width - 1);
                for k in 0..spec.count {
                    let v = if spec.order == 0 {
                        i64::from(q.values[spec.first + k])
                    } else {
                        residual(&q.values, spec.first + k, spec.order)
                    };
                    assert!((-limit..limit).contains(&v), "{spec:?} field {k} = {v}");
                }
                assert!(spec.width <= MAX_WIDTH || spec.order > 0);
            }
        }
    }

    /// Content records cover whole cells and the 1:1 runs sit exactly where the count
    /// laws put them.
    #[test]
    fn records_tile_the_lattice_the_way_the_laws_say() {
        let source = sine(440.0, 20_000.0, 60_000);
        let plan = Plan::new(source.len()).unwrap();
        let q = quantise(&source, &plan);
        let specs = records(&q.values, &plan, Predictor::Plain);

        let mut at = 0;
        for spec in &specs {
            assert_eq!(spec.first, at);
            if !spec.one_to_one {
                assert_eq!(spec.count % CELL, 0);
                assert!(spec.count / CELL <= MAX_CELLS);
            }
            at += spec.count;
        }
        assert_eq!(at, plan.fields);
        let one_to_one: usize = specs.iter().filter(|s| s.one_to_one).map(|s| s.count).sum();
        assert_eq!(one_to_one, plan.warmup + plan.resync);
    }

    /// The minimising predictor is the encoder's law: smooth material differences down
    /// to a narrower field than it stores at, and the stream shrinks for it.
    #[test]
    fn the_minimising_predictor_narrows_smooth_material() {
        let source = sine(60.0, 30_000.0, 60_000);
        let plan = Plan::new(source.len()).unwrap();
        let q = quantise(&source, &plan);
        let plain = records(&q.values, &plan, Predictor::Plain);
        let minimised = records(&q.values, &plan, Predictor::Minimising);

        let bits = |specs: &[Spec]| -> usize { specs.iter().map(Spec::span).sum() };
        assert!(
            bits(&minimised) < bits(&plain),
            "{} words vs {}",
            bits(&minimised),
            bits(&plain)
        );
        assert!(minimised.iter().any(|s| s.order > 0));
        // The 1:1 regime never predicts.
        assert!(minimised.iter().all(|s| !s.one_to_one || s.order == 0));
    }

    /// A residual is the difference the decoder integrates: summing an order-1 run back
    /// up returns the field values it came from.
    #[test]
    fn a_residual_integrates_back_to_the_field_it_came_from() {
        let values: Vec<i32> = (0..200).map(|k| (k * k / 7) % 501 - 250).collect();
        for order in 1..DIFFERENCE.len() as u8 {
            for at in usize::from(order)..values.len() {
                let mut v = residual(&values, at, order);
                for (j, &c) in DIFFERENCE[usize::from(order)].iter().enumerate().skip(1) {
                    v -= i64::from(c) * i64::from(values[at - j]);
                }
                assert_eq!(v, i64::from(values[at]), "order {order} at {at}");
            }
        }
    }

    /// Statistic A carries the shift, whatever the peak, and reads back through the
    /// decoder's own inverse.
    #[test]
    fn statistic_a_round_trips_the_shift() {
        for peak in [0u32, 1, 2, 255, 4095, 4096, 8191, 8192] {
            for shift in 0..6 {
                let (mantissa, exponent) = statistic_a(peak, shift);
                let mut stroke = vec![0u8; HEADER_LEN];
                stroke[12] = exponent;
                stroke[13..16].copy_from_slice(&peak.to_be_bytes()[1..]);
                assert_eq!(
                    codec::shift(&stroke, codec::Layout::V2),
                    Some(shift),
                    "peak {peak}"
                );
                assert!((1 << 19..1 << 20).contains(&mantissa) || peak == 0);
            }
        }
    }

    #[test]
    fn the_stroke_header_holds_the_fixed_bytes_where_the_format_puts_them() {
        let file = instrument(
            &sine(440.0, 9000.0, 20_000),
            &Options::new("Test").root_key(64),
        )
        .unwrap();
        let (_, head) = file.stroke_streams()[0];
        assert_eq!(head[0..5], [0, 0, 0, 1, 0]);
        assert_eq!(head[5], 64);
        assert_eq!(head[6..9], [0x88, 0xba, 0x01]);
        assert_eq!(head[16..20], [0, 0, 0, 0]);
        assert_eq!([head[22], head[31], head[40]], [0x80, 0x80, 0x80]);
        assert_eq!(head[49..51], [0, 0]);
        for gap in [23..29, 32..38, 41..47] {
            assert!(head[gap.clone()].iter().all(|&b| b == 0), "{gap:?}");
        }
    }

    fn zone(source: &[i16], root_key: u8, top_note: u8, global_id: u32) -> NewZone<'_> {
        NewZone {
            source,
            root_key,
            top_note,
            global_id,
        }
    }

    #[test]
    fn every_zone_reads_back_paired_to_its_own_stroke() {
        let high = sine(880.0, 12_000.0, 12_000);
        let mid = sine(440.0, 12_000.0, 9_000);
        let low = sine(220.0, 12_000.0, 15_000);
        let file = multi_zone(
            &[
                zone(&high, 72, 96, 7),
                zone(&mid, 60, 65, 3),
                zone(&low, 48, 53, 9),
            ],
            "Three",
            Predictor::Plain,
        )
        .unwrap();

        let read = super::super::from_bytes(&file.to_bytes().unwrap()).unwrap();
        assert_eq!(read.name().unwrap(), "Three");
        let zones = read.zones().unwrap();
        assert_eq!(
            zones.iter().map(|z| z.top_note).collect::<Vec<_>>(),
            [96, 65, 53]
        );
        assert_eq!(
            zones.iter().map(|z| z.stroke_id).collect::<Vec<_>>(),
            [7, 3, 9]
        );
        assert_eq!(
            read.strokes()
                .unwrap()
                .iter()
                .map(|s| s.root_key)
                .collect::<Vec<_>>(),
            [72, 60, 48]
        );

        for (index, source) in [&high, &mid, &low].iter().enumerate() {
            let (at, stream) = read.zone_stream(index).unwrap();
            let audio = codec::decode(stream, at, codec::Layout::V2).unwrap();
            let plan = Plan::new(source.len()).unwrap();
            let q = quantise(source, &plan);
            let gain = 1i32 << q.shift;
            assert_eq!(audio.samples.len(), plan.fields, "zone {index}");
            for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
                assert_eq!(i32::from(got), want * gain, "zone {index} field {f}");
            }
        }
    }

    #[test]
    fn a_zone_decodes_the_same_alone_as_in_a_crowd() {
        let source = sine(330.0, 18_000.0, 20_000);
        let alone = instrument(&source, &Options::new("One").root_key(60)).unwrap();
        let crowd = multi_zone(
            &[
                zone(&sine(880.0, 9000.0, 8000), 72, 96, 3),
                zone(&source, 60, 65, 2),
                zone(&sine(110.0, 9000.0, 8000), 48, 53, 1),
            ],
            "Three",
            Predictor::Plain,
        )
        .unwrap();

        let one = alone.zone_stream(0).unwrap();
        let many = crowd.zone_stream(1).unwrap();
        assert_ne!(one.1, many.1, "the streams differ; only the audio must not");
        assert_eq!(
            codec::decode(one.1, one.0, codec::Layout::V2).unwrap(),
            codec::decode(many.1, many.0, codec::Layout::V2).unwrap()
        );
    }

    #[test]
    fn every_stroke_is_its_own_header_length_plus_whole_packets() {
        let source = sine(440.0, 12_000.0, 12_000);
        for count in 1..=6usize {
            let zones: Vec<NewZone> = (0..count)
                .map(|i| zone(&source, 60, 120 - 10 * i as u8, i as u32 + 1))
                .collect();
            let file = multi_zone(&zones, "Ladder", Predictor::Plain).unwrap();
            let cat_len = section::find(&file.body.sections, section::CAT)
                .unwrap()
                .payload
                .len();
            let map_len = section::find(&file.body.sections, section::MAP)
                .unwrap()
                .payload
                .len();
            for (index, section) in file
                .body
                .sections
                .iter()
                .filter(|s| s.is(section::STK))
                .enumerate()
            {
                let head = super::super::stroke::header_len(index, cat_len, map_len);
                assert_eq!(
                    (section.payload.len() - head) % PACKET_LEN,
                    0,
                    "{count} zones, stroke {index}: {} bytes over a {head}-byte header",
                    section.payload.len()
                );
            }
        }
    }

    #[test]
    fn a_zone_list_the_format_cannot_store_is_refused() {
        let source = vec![0i16; MIN_FRAMES];
        let one =
            |root, top, id| multi_zone(&[zone(&source, root, top, id)], "x", Predictor::Plain);
        assert!(multi_zone(&[], "x", Predictor::Plain).is_err());
        assert!(one(60, 84, 0).is_err(), "id zero names no stroke");
        assert!(one(60, 84, 256).is_err(), "id past the record's one byte");
        assert!(one(60, 128, 1).is_err());
        assert!(one(128, 84, 1).is_err());
        assert!(one(60, 84, 1).is_ok());

        let pair = |tops: [u8; 2], ids: [u32; 2]| {
            multi_zone(
                &[
                    zone(&source, 60, tops[0], ids[0]),
                    zone(&source, 48, tops[1], ids[1]),
                ],
                "x",
                Predictor::Plain,
            )
        };
        assert!(pair([84, 53], [1, 1]).is_err(), "duplicate stroke id");
        assert!(pair([53, 84], [2, 1]).is_err(), "zones out of order");
        assert!(pair([84, 84], [2, 1]).is_err(), "zones overlap");
        assert!(pair([84, 53], [2, 1]).is_ok());
    }

    /// Silence is silence: nothing promotes, so every content record is the width-2
    /// draft and the stream is the smallest the allocation allows.
    #[test]
    fn silence_codes_at_the_draft_width_throughout() {
        let file = encoded(&vec![0i16; 44_100], Predictor::Plain);
        let (at, stroke) = file.stroke_streams()[0];
        let stream = codec::walk(stroke, at, codec::Layout::V2).unwrap();
        assert!(stream.records.iter().all(|r| r.width == MIN_WIDTH));
        assert!(stream
            .records
            .iter()
            .all(|r| r.values.iter().all(|&v| v == 0)));
        assert_eq!(codec::peak(stroke, codec::Layout::V2), Some(0));
        assert!(codec::decode(stroke, at, codec::Layout::V2)
            .unwrap()
            .samples
            .iter()
            .all(|&s| s == 0));
    }
}
