//! Building a sample instrument from PCM — tier "instrument-valid".
//!
//! The inverse of [`codec`](super::codec), and honest about how far the inverse goes.
//! What this emits is a file whose container, section chain, stroke header, count
//! laws and record grammar are the format's, and whose audio is the source on the
//! field lattice quantised the way the instrument's encoder quantises. What it is
//! **not** is byte-identical to what Nord Sample Editor would produce for the same
//! input: the resampling [`kernel`](super::kernel) is the instrument's to within a few
//! `1e-8` per tap, which leaves a field in a few thousand one count off, a mono
//! stroke's quantiser shift is picked by a rule with one known gap
//! ([`spends_extra_bit`]), and the encoder's own choice of predictor order per record
//! is reproduced only under [`Predictor::Minimising`].
//!
//! So three claims: a file from here **round-trips through this crate's own decoder
//! exactly** under either predictor, it obeys every structural law the format is known
//! to have, and **the Electro 5 loads and plays one** under either predictor, at the pitch
//! the decoder renders.
//!
//! Confirmed on hardware for [`Layout::V2`], which is what the Electro 5 plays. The
//! wide generations are inferred from specimens; not confirmed on hardware.
//!
//! ```no_run
//! # use nord_format::formats::nsmp::encode;
//! let samples: Vec<i16> = vec![0; 44_100];
//! let options = encode::Options::new("Test").root_key(60);
//! let instrument = encode::instrument(&samples, &options).unwrap();
//! std::fs::write("test.nsmp", instrument.to_bytes().unwrap()).unwrap();
//! ```
//!
//! **All three generations write from the one plan.** [`Options::layout`] picks the
//! generation; what moves with it is the container — the narrow `NWS` chain against
//! the wide `NSMP` one, and the section schemas inside — and the stream's units, which
//! [`Units`] holds. The lattice, the kernel, the quantiser, the count laws and the
//! record grammar's bit layout are the same object in all three.
//!
//! [`multi_zone`] is the same builder across a keyboard: one `stk` per zone, highest
//! zone first, each zone's record naming its stroke by the global id the caller gives
//! it. Zone counts move where a stroke's audio may start, so the allocation each stroke
//! is packed into comes from [`stroke::header_len`](super::stroke::header_len) rather
//! than from a constant.
//!
//! **Stereo is the mono plan run once per channel and interleaved.** A stereo stroke
//! carries both channels under one header at the doubled cell, and every count-law
//! landmark — the field total, the resync position, both 1:1 runs — is exactly its mono
//! value doubled. So the whole of stereo, on the plan side, is a channel count: cells
//! and 1:1 records double, the terminator states the doubled cell, and the predictor
//! keeps a history per channel. Where the two channels' *bits* go does move with the
//! generation: v2 and v3 alternate fields in one bitstream, v4 packs each channel's
//! half into its own words and alternates those.
//!
//! Inferred from specimens; not confirmed on hardware.
//!
//! A [`Loop`] truncates the stroke at its end and opens a marked record at its start,
//! which is the whole of what the container stores about looping: the crossfade is
//! baked into the audio here, and loop detune, loop decay and the short loop's
//! pitch-tracking flag reach the file nowhere at all. The fade's frame count is the
//! caller's to work out — a project states the long loop's in frames and the short
//! loop's as a percentage of its length — and it arrives here already in frames,
//! fraction and all.
//!
//! Inferred from specimens; not confirmed on hardware.

use super::codec::{self, Layout, PITCH_DEN, PITCH_NUM, WRAP};
use super::kernel;
use super::section::{self, Section, Section4};
use super::stroke::packet_len;
use super::{Sample, SampleV3};
use crate::cbin::{Cbin, Generation, Header};
use crate::error::{Error, ParseError};
use crate::formats::nsmpproj;

/// Content version this writes per generation: `format × 100 + revision`, at the
/// revision the editor emits.
const fn version(layout: Layout) -> u32 {
    match layout {
        Layout::V2 => 200,
        Layout::V3 => 300,
        Layout::V4 => 400,
    }
}

/// The sample-instrument `aux` value, the same in every generation.
/// Unexplained: real programs hold this, and the panel cannot produce it.
const AUX: u32 = 0x000f_0000;

/// Largest field count a record header can state, from its 14-bit count field.
/// ⚠️ A record covers whole cells, so how many *cells* that is halves on a stereo
/// stroke — the count is a field count, and a stereo cell holds two channels' worth.
const MAX_COUNT: usize = (1 << 14) - 1;

/// Widest field a stroke's peak may take: quantisation shifts until it fits. On a
/// stereo stroke this is the whole of the shift rule.
const PEAK_WIDTH: u8 = 14;

/// The stream units one stroke is written in: the generation's word and cell sizes,
/// scaled by how many channels share the stroke.
///
/// Everything else about the encoder is generation-independent — the lattice, the
/// kernel, the quantiser and the record grammar's bit layout do not move — so this is
/// the whole of what a generation changes about a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Units {
    layout: Layout,
    /// 1 or 2.
    channels: usize,
}

impl Units {
    const fn word(self) -> usize {
        self.layout.word()
    }

    const fn word_bits(self) -> usize {
        self.layout.word() * 8
    }

    /// Fields one content cell covers: the generation's cell per channel.
    const fn cell(self) -> usize {
        self.layout.cell() * self.channels
    }

    /// Fields one 1:1 record covers at most: the generation's RMAX per channel.
    const fn chunk(self) -> usize {
        self.layout.rmax() * self.channels
    }

    /// Whether a record's two channels occupy alternating, independently padded
    /// words rather than alternating fields in one bitstream.
    const fn splits(self) -> bool {
        self.channels == 2 && self.layout.splits_wide_openings()
    }

    /// Words one channel's half of a split record occupies.
    const fn half(self, count: usize, width: u8) -> usize {
        (count / 2 * width as usize).div_ceil(self.word_bits())
    }

    /// Words one record occupies, header included.
    ///
    /// A split record pays for each channel's own padding; a content record tiles
    /// whole words either way, so only the 1:1 regime is ever wider for it.
    const fn span(self, count: usize, width: u8) -> usize {
        if self.splits() {
            1 + 2 * self.half(count, width)
        } else {
            (self.word_bits() + count * width as usize).div_ceil(self.word_bits())
        }
    }

    /// Words in one packet of allocation.
    const fn packet_words(self) -> usize {
        packet_len(self.layout) / self.word()
    }

    /// Words of slack the allocation keeps ahead of the chain's first record.
    ///
    /// The chain is right-aligned in whole packets either way; the wide chain buys a
    /// further packet rather than let the lead fall below this, so its strokes carry
    /// 7 to 38 words of slack where a narrow one carries 0 to 126.
    ///
    /// Inferred from specimens; not confirmed on hardware.
    const fn min_lead(self) -> usize {
        match self.layout {
            Layout::V2 => 0,
            Layout::V3 | Layout::V4 => 7,
        }
    }

    /// Absolute field ceiling imposed by the stream directory and minimum width.
    const fn max_fields(self) -> usize {
        MAX_STREAM_WORDS * self.word_bits() / MIN_WIDTH as usize
    }
}

/// Last-record field counts, per channel, that do not carry the extra quantiser bit —
/// `None` for a generation whose mono strokes never spend it.
///
/// A run's records are RMAX-sized until a remainder, so the range a last record can
/// take is the generation's: 24..=32 fields at v2 and 32..=48 at v3. Every width in
/// both ranges has been read off a render, and these are the ones that never buy the
/// bit. There is no arithmetic behind either set and no correspondence between them.
///
/// Inferred from specimens; not confirmed on hardware.
const fn dead_last_record(layout: Layout) -> Option<&'static [usize]> {
    match layout {
        Layout::V2 => Some(&[24, 29, 32]),
        Layout::V3 => Some(&[32, 41, 43, 45, 47, 48]),
        Layout::V4 => None,
    }
}

/// Whether a stroke spends one more quantiser bit than its peak needs, narrowing its
/// widest field a bit under [`PEAK_WIDTH`] and shrinking the stream.
///
/// `values` are the stroke's fields before any shift. Read them at the smallest shift
/// that fits the peak in [`PEAK_WIDTH`] bits: the bit is spent when a field still of
/// magnitude `2^13` or more there falls inside the **last record of one of the two 1:1
/// runs** — the opening run at field 0, or the resync run — and that record's field
/// count is not one [`dead_last_record`] names. A field in an earlier record of a run,
/// or out in the content cells, never buys it, and neither run's length is otherwise
/// consulted.
///
/// A stereo stroke never spends the bit, in any generation, and neither does a v4 mono
/// one: both quantise at the peak term alone.
///
/// ⚠️ A stroke the clause fires on can still refuse when taking the bit would barely
/// shrink the stream. Nothing here models that, so such a stroke quantises one bit
/// coarser than the editor's; the threshold is not pinned.
///
/// Inferred from specimens; not confirmed on hardware, the Electro 5 playing v2 only.
fn spends_extra_bit(values: &[i64], plan: &Plan) -> bool {
    if plan.channels != 1 {
        return false;
    }
    let Some(dead) = dead_last_record(plan.layout) else {
        return false;
    };
    let over = 1i64 << (PEAK_WIDTH - 2);
    let shift = peak_shift(values, PEAK_WIDTH);
    [(0, plan.warmup), (plan.resync_at, plan.resync)]
        .into_iter()
        .any(|(base, run)| {
            let Some(&last) = chunks(run, plan.chunk()).last() else {
                return false;
            };
            !dead.contains(&(last / plan.channels))
                && values[base + run - last..base + run]
                    .iter()
                    .any(|&v| (v >> shift).abs() >= over)
        })
}

/// The smallest nonnegative shift fitting every value in `width` bits.
fn peak_shift(values: &[i64], width: u8) -> i32 {
    let low = values.iter().copied().min().unwrap_or(0);
    let high = values.iter().copied().max().unwrap_or(0);
    let mut shift = 0i32;
    while width_of(low >> shift, high >> shift) > width {
        shift += 1;
    }
    shift
}

/// Widest field a record header can declare, from its four-bit width. Padding stores
/// values wider than they need, which sign-extend back to themselves.
const MAX_STORED_WIDTH: u8 = 16;

/// Narrowest field. Width 2 is the draft the encoder codes everything at before it
/// promotes anything, and a width-1 flag-1 record is the terminator.
const MIN_WIDTH: u8 = 2;

/// Channels one stroke may carry. The terminator states the cell size, and one bit of
/// doubling is all it can say.
const MAX_CHANNELS: usize = 2;

/// Zones one instrument may hold, from the `map` section's single count byte.
const MAX_ZONES: usize = u8::MAX as usize;

/// The widest stroke id a zone record can name: the field is one byte, and zero is
/// not an id the editor issues.
const MAX_STROKE_ID: u32 = u8::MAX as u32;

/// Fields an unlooped stroke carries past the end of its source, every one of which
/// stores zero: the kernel's ring past the last sample is cut, not coded.
const RING_OUT: usize = 127;

/// Fields the stream's opening ramp lasts, per channel: field `f` of each channel is
/// scaled by `(f / RAMP_IN)³`, truncated, until the ramp reaches 1.
/// Inferred from specimens; not confirmed on hardware.
const RAMP_IN: usize = 35;

/// Shortest modelled input; shorter streams use an unresolved opening.
pub const MIN_FRAMES: usize = 4096;

/// Fields a looped stroke carries past its loop end, repeating the loop's own opening
/// so that playback is unchanged. The mark clears the loop start by the same amount,
/// which is why the loop's length survives it.
const LOOP_LEAD: usize = 5;

/// Fields a loop's pre-roll needs before the mark: an opening 1:1 run and a resync run,
/// both of which reach [`band`]'s widest.
const MIN_PRE_LOOP: usize = 192;

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
    /// Choose the narrowest predictor per cell, the lowest order among equals — the
    /// editor's own choice. Smaller than plain records and exact through this crate's
    /// decoder.
    Minimising,
}

/// A sustain loop, in source frames.
///
/// The container stores a loop as two things and nothing else: the stroke stops at
/// [`end`](Loop::end), and the record the loop starts at carries the mark bit. Loop
/// detune, loop decay, and whether the editor called this a short loop or a long one
/// are not stored anywhere, so a caller that needs them cannot have them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loop {
    /// First frame of the loop.
    pub start: usize,
    /// One past its last frame. Audio after it is not encoded.
    pub end: usize,
    /// Frames of the loop's tail that fade into the frames before [`start`](Loop::start).
    /// The fade is applied to the samples here, because that is where the instrument
    /// reads it from. Fractional, because a project can state it as a percentage of the
    /// loop rather than a frame count, and dropping the fraction moves the fade a field.
    /// Inferred from specimens; not confirmed on hardware.
    pub crossfade: f64,
}

impl Loop {
    /// A loop over `start..end` with no crossfade.
    pub fn new(start: usize, end: usize) -> Loop {
        Loop {
            start,
            end,
            crossfade: 0.0,
        }
    }

    pub fn crossfade(mut self, frames: f64) -> Loop {
        self.crossfade = frames;
        self
    }
}

/// What to build around the audio.
#[derive(Debug, Clone)]
pub struct Options {
    name: String,
    root_key: u8,
    top_note: Option<u8>,
    predictor: Predictor,
    loops: Option<Loop>,
    channels: u16,
    secondary_start: Option<f64>,
    shift: Option<u8>,
    layout: Layout,
}

impl Options {
    /// Defaults: the name given, root key C4, the editor's own top note, plain records,
    /// no loop, the v2 generation.
    pub fn new(name: impl Into<String>) -> Options {
        Options {
            name: name.into(),
            root_key: 60,
            top_note: None,
            predictor: Predictor::Plain,
            loops: None,
            channels: 1,
            secondary_start: None,
            shift: None,
            layout: Layout::V2,
        }
    }

    /// Which generation to write: `.nsmp`, `.nsmp3` or `.nsmp4`. The audio is the same
    /// object in all three — what moves is the container and the stream's units.
    pub fn layout(mut self, layout: Layout) -> Options {
        self.layout = layout;
        self
    }

    /// Resynchronise the stream at `frames` source frames from the first one — a
    /// project's `m_startSecondary`, measured from its `m_start`. Unset, the stream
    /// resynchronises where a fresh project would put it: [`default_secondary_start`].
    pub fn secondary_start(mut self, frames: f64) -> Options {
        self.secondary_start = Some(frames);
        self
    }

    /// How many channels the PCM interleaves — 1 or 2. Anything else is refused when
    /// the instrument is built.
    pub fn channels(mut self, channels: u16) -> Options {
        self.channels = channels;
        self
    }

    /// Quantise at `bits` of shift instead of what the shift rule picks. Experimental: a
    /// lever for laying the same stroke out at neighbouring shifts, not a setting the
    /// editor exposes.
    pub fn shift(mut self, bits: u8) -> Options {
        self.shift = Some(bits);
        self
    }

    /// Loop the stroke, which also truncates it at [`Loop::end`].
    pub fn loops(mut self, points: Loop) -> Options {
        self.loops = Some(points);
        self
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

/// Where a loop lands on the field lattice. Every count is in stream fields, so on a
/// stereo stroke each is twice what one channel sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Looped {
    /// Field the marked record opens at.
    pub at: usize,
    /// Fields repeated past the loop end, which is also how far `at` clears the loop
    /// start. See [`LOOP_LEAD`].
    pub lead: usize,
    /// Fields of the loop's tail the crossfade rewrites.
    pub crossfade: usize,
    /// Fields in the 1:1 run the loop opens with.
    pub warmup: usize,
    /// Content cells between that run and the terminator.
    pub cells: usize,
}

/// Stroke landmarks derived from the source frame count.
///
/// Every field count here is a **stream** count: on a stereo stroke the two channels
/// interleave, so each is twice the per-channel number the mono laws state. [`cell`] and
/// [`chunk`] scale with it, which is the whole of what stereo changes about the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Which generation's units the stream is written in.
    pub layout: Layout,
    /// Source frames the stroke covers — frames, not samples: a stereo frame is two.
    pub frames: usize,
    /// Channels interleaved into the stream: 1 or 2.
    pub channels: usize,
    /// Fields in the stream — the source plus a ring-out past its end, or, when the
    /// stroke loops, the source up to the loop end plus the repeated lead.
    pub fields: usize,
    /// Field the resync record starts at.
    pub resync_at: usize,
    /// Fields in the opening 1:1 run.
    pub warmup: usize,
    /// Fields in the resync 1:1 run.
    pub resync: usize,
    /// Content cells between the warmup and the resync.
    pub cells_before: usize,
    /// Content cells between the resync and the loop start, or the terminator.
    pub cells_after: usize,
    /// The loop, once it is on the lattice.
    pub looped: Option<Looped>,
}

impl Plan {
    const fn units(&self) -> Units {
        Units {
            layout: self.layout,
            channels: self.channels,
        }
    }

    /// Fields one content cell covers — the generation's cell per channel.
    pub const fn cell(&self) -> usize {
        self.units().cell()
    }

    /// Fields one 1:1 record covers at most — the generation's RMAX per channel.
    const fn chunk(&self) -> usize {
        self.units().chunk()
    }
}

/// Source frames onto the field lattice.
fn fields_of(frames: usize) -> Option<usize> {
    let frames = u64::try_from(frames).ok()?;
    frames
        .checked_mul(u64::from(PITCH_DEN))
        .and_then(|n| round_ratio(n, u64::from(PITCH_NUM)))
}

/// The same lattice, for a landmark that falls between two frames — a fade a project
/// states as a percentage of its loop rather than as a frame count. Rounding such a
/// value to a whole frame before it reaches the lattice opens the ramp a field early.
fn fields_at(frames: f64) -> Option<usize> {
    let fields = frames * f64::from(PITCH_DEN) / f64::from(PITCH_NUM);
    (fields.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&fields))
        .then_some(fields.round() as usize)
}

impl Plan {
    /// The layout for `frames` source frames of `channels`-channel audio, no loop,
    /// resynchronising at `secondary_start` source frames from the first — the
    /// project's `m_startSecondary` measured from its `m_start`, or
    /// [`default_secondary_start`] for audio no project describes.
    ///
    /// Refuses a secondary start the stream cannot resynchronise at: off the lattice,
    /// or too close to either end for the 1:1 runs around it.
    pub fn new(
        layout: Layout,
        frames: usize,
        channels: usize,
        secondary_start: f64,
    ) -> Result<Plan, Error> {
        Plan::modelled(frames, channels)?;
        let fields = fields_of(frames)
            .and_then(|f| f.checked_add(RING_OUT))
            .and_then(|f| f.checked_mul(channels))
            .ok_or_else(|| size_error(frames))?;
        let resync_at = Plan::resync_at(secondary_start, channels)?;
        Plan::lay_out(layout, frames, channels, fields, None, resync_at)
    }

    /// The layout for a stroke that loops: `frames` source samples truncated at
    /// [`Loop::end`], with the loop's own opening repeated past it, resynchronising at
    /// `secondary_start` as [`new`](Plan::new) does.
    ///
    /// Refuses a loop the format cannot state — one outside the audio, one shorter than
    /// the run it has to open with, or a crossfade with no material in front of the loop
    /// to fade from — and a secondary start that leaves no room ahead of the loop.
    pub fn looped(
        layout: Layout,
        frames: usize,
        channels: usize,
        points: Loop,
        secondary_start: f64,
    ) -> Result<Plan, Error> {
        Plan::modelled(points.end, channels)?;
        if points.start >= points.end || points.end > frames {
            return Err(ParseError::OutOfBounds {
                value: format!("a loop over frames {}..{}", points.start, points.end),
                bound: format!("a non-empty region of the {frames} frames given"),
            }
            .into());
        }
        // Everything below is laid out per channel and scaled at the end, because that
        // is what the encoder does: one plan, interleaved.
        let lattice = |n: usize| fields_of(n).and_then(|f| f.checked_mul(channels));
        let lattice_at = |n: f64| fields_at(n).and_then(|f| f.checked_mul(channels));
        let start = lattice(points.start).ok_or_else(|| size_error(points.start))?;
        // The loop's length is what has to survive, so it is put on the lattice as a
        // length. Rounding its two ends separately can cost it a field.
        let span = points.end - points.start;
        let length = lattice(span).ok_or_else(|| size_error(points.end))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| size_error(points.end))?;
        // Ahead of the mark the stream still has to open and resync, so a loop that
        // starts too early is pushed off the front by repeating more of itself.
        let units = Units { layout, channels };
        let (cell, chunk) = (units.cell(), units.chunk());
        let lead = (LOOP_LEAD * channels).max((MIN_PRE_LOOP * channels).saturating_sub(start));
        let at = start
            .checked_add(lead)
            .ok_or_else(|| size_error(points.start))?;
        let fields = end
            .checked_add(lead)
            .ok_or_else(|| size_error(points.end))?;
        let warmup = band(length, cell, chunk);
        if length < warmup.saturating_add(cell) {
            return Err(ParseError::OutOfBounds {
                value: format!("a {length}-field loop"),
                bound: format!(
                    "a loop long enough for the {warmup}-field 1:1 run it opens with and \
                     one {cell}-field cell after it"
                ),
            }
            .into());
        }
        if !(0.0..=points.start as f64).contains(&points.crossfade) {
            return Err(ParseError::OutOfBounds {
                value: format!("a {} frame crossfade", points.crossfade),
                bound: format!(
                    "the {} frames before the loop starts — the fade compares \
                     each frame with the material one loop length behind it",
                    points.start,
                ),
            }
            .into());
        }
        // Put the fade's opening on the loop-relative lattice. Above 100% it begins
        // before the loop start, so its distance is added to the loop length.
        let crossfade = if points.crossfade <= span as f64 {
            let opens =
                lattice_at(span as f64 - points.crossfade).ok_or_else(|| size_error(span))?;
            length.checked_sub(opens).ok_or_else(|| size_error(span))?
        } else {
            let before = lattice_at(points.crossfade - span as f64)
                .ok_or_else(|| size_error(points.start))?;
            length
                .checked_add(before)
                .ok_or_else(|| size_error(points.end))?
        };
        if crossfade > start {
            return Err(ParseError::OutOfBounds {
                value: format!("a {} frame crossfade", points.crossfade),
                bound: format!(
                    "the {} frames before the loop starts — the field lattice \
                     leaves no earlier material to compare",
                    points.start,
                ),
            }
            .into());
        }
        let resync_at = Plan::resync_at(secondary_start, channels)?;
        Plan::lay_out(
            layout,
            frames,
            channels,
            fields,
            Some(Looped {
                at,
                lead,
                crossfade,
                warmup,
                cells: (length - warmup) / cell,
            }),
            resync_at,
        )
    }

    /// The secondary start on the lattice — a per-channel position, doubled like every
    /// other landmark when the two channels interleave.
    fn resync_at(secondary_start: f64, channels: usize) -> Result<usize, Error> {
        fields_at(secondary_start)
            .and_then(|f| f.checked_mul(channels))
            .ok_or_else(|| {
                ParseError::OutOfBounds {
                    value: format!("a secondary start at frame {secondary_start}"),
                    bound: "a position on the field lattice".into(),
                }
                .into()
            })
    }

    fn modelled(frames: usize, channels: usize) -> Result<(), Error> {
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(ParseError::OutOfBounds {
                value: format!("{channels} channels"),
                bound: format!(
                    "1 or {MAX_CHANNELS} — the terminator states one cell size, and all \
                     it can say is whether the cell is doubled"
                ),
            }
            .into());
        }
        if frames >= MIN_FRAMES {
            return Ok(());
        }
        Err(ParseError::OutOfBounds {
            value: format!("{frames} frames"),
            bound: format!(
                "the modelled range: at least {MIN_FRAMES} frames, below which the \
                 stream opens a way this crate has not modelled"
            ),
        }
        .into())
    }

    /// Place the warmup, the resync and the cells between them across everything ahead
    /// of the loop — or across the whole stream when there is none.
    fn lay_out(
        layout: Layout,
        frames: usize,
        channels: usize,
        fields: usize,
        looped: Option<Looped>,
        resync_at: usize,
    ) -> Result<Plan, Error> {
        let units = Units { layout, channels };
        if fields > units.max_fields() {
            return Err(size_error(frames).into());
        }
        let (cell, chunk) = (units.cell(), units.chunk());
        let band = |r: usize| band(r, cell, chunk);
        let head = looped.map_or(fields, |l| l.at);
        let warmup = band(resync_at);
        let fits = resync_at >= warmup
            && head
                .checked_sub(warmup)
                .and_then(|rest| resync_at.checked_add(band(rest)))
                .is_some_and(|end| head >= end);
        if !fits {
            return Err(ParseError::OutOfBounds {
                value: format!("a secondary start at field {resync_at}"),
                bound: format!(
                    "the {head} fields ahead of the {}, less the 1:1 run at each end",
                    if looped.is_some() {
                        "loop"
                    } else {
                        "terminator"
                    }
                ),
            }
            .into());
        }
        let resync = band(head - warmup);
        Ok(Plan {
            layout,
            frames,
            channels,
            fields,
            resync_at,
            warmup,
            resync,
            cells_before: (resync_at - warmup) / cell,
            cells_after: (head - resync_at - resync) / cell,
            looped,
        })
    }
}

/// Where a fresh project would put the resync in `frames` untrimmed source frames: the
/// `m_startSecondary` [`nsmpproj::default_secondary_start`] states, repaired around
/// `loops` the way the editor repairs a project it loads.
pub fn default_secondary_start(frames: usize, loops: Option<Loop>) -> f64 {
    let stop = frames as f64;
    nsmpproj::repaired_secondary_start(
        nsmpproj::default_secondary_start(stop),
        stop,
        loops.map(|l| l.start as f64),
    )
}

/// `round(num/den)`, half away from zero, on non-negative integers.
fn round_ratio(num: u64, den: u64) -> Option<usize> {
    num.checked_add(den / 2)
        .and_then(|n| usize::try_from(n / den).ok())
}

/// Frames in interleaved PCM, refusing a buffer that is not whole frames.
fn frames_of(source: &[i16], channels: usize) -> Result<usize, Error> {
    if channels == 0 || !source.len().is_multiple_of(channels) {
        return Err(ParseError::AssertFail(format!(
            "{} sample(s) is not a whole number of {channels}-channel frames",
            source.len()
        ))
        .into());
    }
    Ok(source.len() / channels)
}

fn size_error(frames: usize) -> ParseError {
    ParseError::OutOfBounds {
        value: format!("{frames} frames"),
        bound: format!("audio whose encoded stream fits {MAX_STREAM_WORDS} words"),
    }
}

/// The 1:1 run that preserves a landmark's cell phase — constructive, and the same
/// statement at either channel count.
///
/// A run of `j` records covers between `j*cell` and `j*rmax` fields, so the reachable
/// lengths come in windows with gaps between them: 24..=32, 48..=64, 72..=96 at the mono
/// pair, and everything doubled at the stereo one. `band(r)` is the smallest reachable
/// length at or above `cell` that is congruent to `r`, which at `r ≡ 0` is `cell` itself.
fn band(r: usize, cell: usize, rmax: usize) -> usize {
    let residue = if r.is_multiple_of(cell) {
        cell
    } else {
        r % cell
    };
    let mut length = if residue == cell {
        cell
    } else {
        residue + cell
    };
    // The windows overlap from `j = 7` on, so this settles within a few steps; the bound
    // is a guard, not a limit anything reaches.
    while length <= 64 * cell {
        if (1..=8).any(|j| j * cell <= length && length <= j * rmax) {
            return length;
        }
        length += cell;
    }
    length
}

/// Split a 1:1 run into records of at most `chunk` fields. [`band`] is what guarantees
/// the remainder is a legal record rather than a stub.
fn chunks(mut n: usize, chunk: usize) -> Vec<usize> {
    let mut out = Vec::new();
    while n > chunk {
        out.push(chunk);
        n -= chunk;
    }
    out.push(n);
    out
}

/// The source on the lattice, quantised — the stream's field values and the two
/// header statistics that describe them.
#[derive(Debug, Clone)]
struct Quantised {
    /// One stored value per field, sign-extended and within the stream's maximum width.
    values: Vec<i32>,
    /// Bits the values were shifted right by. Dequantising shifts back.
    shift: i32,
    /// Statistic B: the content field of largest magnitude, taken at a fixed shift of 2.
    /// Carries the extreme's sign where the generation stores one; a magnitude at v2.
    peak: i32,
}

/// Largest magnitude statistic B's 24 bits hold once a sign is allowed for. A field
/// is the source's own 16-bit unit taken at a shift of two, so nothing reaches it.
const MAX_PEAK: i64 = (1 << 23) - 1;

/// The opening ramp: the first [`RAMP_IN`] fields of a channel rise as the cube of
/// their position, toward zero like everything else the encoder quantises.
fn ramp_in(fields: &mut [i64]) {
    let cube = |n: usize| (n * n * n) as i64;
    for (f, value) in fields.iter_mut().enumerate().take(RAMP_IN) {
        *value = *value * cube(f) / cube(RAMP_IN);
    }
}

/// Ramp the loop's tail into the material one loop length behind it, then repeat the
/// loop's opening past its end.
///
/// One channel at a time, so every count here is a per-channel one.
///
/// The ramp is linear across the crossfade, which is what the editor's own crossfade
/// ladder measures out.
///
/// Inferred from specimens; not confirmed on hardware.
fn bake_loop(raw: &mut [i64], at: usize, lead: usize, crossfade: usize) {
    let fields = raw.len();
    let end = fields - lead;
    let length = fields - at;
    let span = crossfade as i64;
    for k in 0..crossfade {
        let f = end - crossfade + k;
        let (near, far) = (raw[f], raw[f - length]);
        let step = (far - near) * k as i64;
        raw[f] = near + (2 * step + span * step.signum()) / (2 * span);
    }
    // The repeated fields are the loop's own opening, so the loop plays the same region
    // however far the mark clears its start.
    for k in 0..lead {
        raw[end + k] = raw[at - lead + k];
    }
}

/// Resample and choose the smallest nonnegative shift that fits the stroke's peak into
/// [`PEAK_WIDTH`] bits, plus the further bit a mono stroke spends when
/// [`spends_extra_bit`] says so. `forced` lays the stroke out at that shift instead.
///
/// Each channel is resampled on its own lattice and the results interleaved, because
/// that is what the stream carries; the shift and statistic B are one pair for the
/// stroke, taken across both.
fn quantise(source: &[i16], plan: &Plan, forced: Option<u8>) -> Quantised {
    let channels = plan.channels;
    let per = plan.fields / channels;
    let mut raw = vec![0i64; plan.fields];
    // The sums each field truncates from. Statistic B ranks fields on these, so two
    // fields that truncate alike still order.
    let mut sums = vec![0f64; plan.fields];
    let mut lane: Vec<i16> = Vec::with_capacity(source.len().div_ceil(channels));
    for channel in 0..channels {
        lane.clear();
        lane.extend(source.iter().skip(channel).step_by(channels).copied());
        let accumulated: Vec<f64> = (0..per).map(|f| kernel::accumulate(&lane, f)).collect();
        let mut fields: Vec<i64> = accumulated.iter().map(|sum| sum.trunc() as i64).collect();
        ramp_in(&mut fields);
        match &plan.looped {
            Some(points) => bake_loop(
                &mut fields,
                points.at / channels,
                points.lead / channels,
                points.crossfade / channels,
            ),
            None => fields[per - RING_OUT..].fill(0),
        }
        for (f, (value, sum)) in fields.into_iter().zip(accumulated).enumerate() {
            let at = f * channels + channel;
            raw[at] = value;
            // A field the ramp, the loop or the ring-out rewrote ranks by what it holds.
            sums[at] = if value == sum.trunc() as i64 {
                sum
            } else {
                value as f64
            };
        }
    }
    let mut shift = peak_shift(&raw, PEAK_WIDTH);
    if spends_extra_bit(&raw, plan) {
        shift += 1;
    }
    if let Some(bits) = forced {
        shift = i32::from(bits);
    }

    // Statistic B is the content field of largest magnitude at a fixed shift of two —
    // a negative extreme therefore rounds away from zero — and a later field takes the
    // extreme only by exceeding it. Content only, which is why a value the 1:1 regime
    // carries never sets it.
    let opening = plan.looped.map(|l| l.at..l.at + l.warmup);
    let content = |f: usize| {
        ((f >= plan.warmup && f < plan.resync_at) || f >= plan.resync_at + plan.resync)
            && !opening.as_ref().is_some_and(|run| run.contains(&f))
    };
    let extreme = (0..plan.fields)
        .filter(|&f| content(f))
        .fold(None, |best: Option<usize>, f| match best {
            Some(b) if sums[f].abs() <= sums[b].abs() => Some(b),
            _ => Some(f),
        });
    let signed = extreme
        .map_or(0, |f| raw[f] >> 2)
        .clamp(-MAX_PEAK - 1, MAX_PEAK) as i32;
    let peak = match plan.layout.signed_peak() {
        true => signed,
        false => signed.abs(),
    };

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
    while i128::from(low) < -(1i128 << (w - 1)) || i128::from(high) > (1i128 << (w - 1)) - 1 {
        w += 1;
    }
    w
}

/// One record, before it becomes words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Spec {
    one_to_one: bool,
    width: u8,
    order: u8,
    /// Set on the record a loop starts at, and on no other.
    mark: bool,
    first: usize,
    count: usize,
}

impl Spec {
    /// Words this record occupies, header included.
    fn span(&self, units: Units) -> usize {
        units.span(self.count, self.width)
    }
}

/// The Nth backward difference at `at`, across record boundaries.
///
/// ⚠️ **`stride` is the channel count**: the predictor runs per channel, so a stereo
/// field differences against the field two slots back, not the other channel's.
fn residual(values: &[i32], at: usize, order: u8, stride: usize) -> i64 {
    DIFFERENCE[usize::from(order)]
        .iter()
        .enumerate()
        .map(|(j, &c)| match at.checked_sub(j * stride) {
            Some(k) => i64::from(c) * i64::from(values[k]),
            None => 0,
        })
        .sum()
}

/// The width one cell needs at `order`, and the sum of the residuals it would store.
/// One cell is `stride` channels' worth, and a record declares one width for both.
fn width_at(values: &[i32], first: usize, order: u8, cell: usize, stride: usize) -> u8 {
    let mut low = 0i64;
    let mut high = 0i64;
    for at in first..first + cell {
        let e = residual(values, at, order, stride);
        low = low.min(e);
        high = high.max(e);
    }
    width_of(low, high)
}

/// The width each predictor order codes one cell at, indexed by order — order 0 alone
/// under [`Predictor::Plain`].
fn widths_at(
    values: &[i32],
    first: usize,
    predictor: Predictor,
    cell: usize,
    stride: usize,
) -> Vec<u8> {
    let orders = match predictor {
        Predictor::Plain => 1,
        Predictor::Minimising => DIFFERENCE.len(),
    };
    (0..orders as u8)
        .map(|order| width_at(values, first, order, cell, stride))
        .collect()
}

/// The order and width a cell is coded at, given the widths each order needs: the
/// record being extended, `(order, width)`, keeps its order while the cell's narrowest
/// width is still the record's and that order still reaches it; otherwise the lowest
/// order that reaches the narrowest width.
fn choose_order(widths: &[u8], extending: Option<(u8, u8)>) -> (u8, u8) {
    let narrowest = *widths.iter().min().unwrap_or(&MIN_WIDTH);
    let reaches = |order: u8| widths.get(usize::from(order)) == Some(&narrowest);
    let order = extending
        .filter(|&(order, width)| width == narrowest && reaches(order))
        .map_or_else(
            || (0..widths.len() as u8).find(|&o| reaches(o)).unwrap_or(0),
            |(order, _)| order,
        );
    (order, narrowest)
}

/// Partition 1:1 values and like-coded content cells into records.
///
/// A loop appends a third regime — its own 1:1 run, marked, and the content after it —
/// grown to a whole number of packets by [`pad_to_packet`].
fn records(values: &[i32], plan: &Plan, predictor: Predictor) -> Result<Vec<Spec>, Error> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let (cell, chunk, stride) = (plan.cell(), plan.chunk(), plan.channels);

    let one_to_one = |out: &mut Vec<Spec>, at: &mut usize, fields: usize| {
        for count in chunks(fields, chunk) {
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
                mark: false,
                first: *at,
                count,
            });
            *at += count;
        }
    };

    // A record runs on while each cell's narrowest width is still the record's and the
    // record's own order still reaches it; the first cell that breaks either opens a new
    // record at the lowest order that reaches its width.
    let content = |out: &mut Vec<Spec>, at: &mut usize, cells: usize| {
        let mut run: Option<Spec> = None;
        for index in 0..cells {
            let first = *at + index * cell;
            let widths = widths_at(values, first, predictor, cell, stride);
            let (order, width) = choose_order(&widths, run.map(|r| (r.order, r.width)));
            match run {
                Some(ref mut record)
                    if (record.order, record.width) == (order, width)
                        && record.count + cell <= MAX_COUNT =>
                {
                    record.count += cell;
                }
                _ => {
                    out.extend(run.take());
                    run = Some(Spec {
                        one_to_one: false,
                        width,
                        order,
                        mark: false,
                        first,
                        count: cell,
                    });
                }
            }
        }
        out.extend(run);
        *at += cells * cell;
    };

    one_to_one(&mut out, &mut at, plan.warmup);
    content(&mut out, &mut at, plan.cells_before);
    let resync_record = out.len();
    one_to_one(&mut out, &mut at, plan.resync);
    content(&mut out, &mut at, plan.cells_after);
    if let Some(points) = &plan.looped {
        let opening = out.len();
        one_to_one(&mut out, &mut at, points.warmup);
        out[opening].mark = true;
        content(&mut out, &mut at, points.cells);
        pad_to_packet(&mut out, opening, plan.units())?;
    }
    if at != plan.fields {
        return Err(ParseError::AssertFail(format!(
            "the record plan covered {at} of {} fields",
            plan.fields
        ))
        .into());
    }
    if resync_record >= out.len() {
        return Err(
            ParseError::AssertFail("the record plan produced no resync record".into()).into(),
        );
    }
    Ok(out)
}

/// Pad the loop region out to whole packets the way the editor does: sweep its content
/// records front to back, halving each one that covers more than one cell — the smaller
/// half first — and carrying on into the second half, pass after pass, until the words
/// fit. A region with nothing left to split is widened instead, from its last record
/// back, which is this crate's own choice: no render has shown what the editor does then.
/// Inferred from specimens; not confirmed on hardware.
fn pad_to_packet(specs: &mut Vec<Spec>, opening: usize, units: Units) -> Result<(), Error> {
    let cell = units.cell();
    let packet = units.packet_words();
    let words = |specs: &[Spec]| specs.iter().map(|s| s.span(units)).sum::<usize>();
    let mut pad = (packet - words(&specs[opening..]) % packet) % packet;

    let splittable = |spec: &Spec| !spec.one_to_one && spec.count > cell;
    while pad > 0 && specs[opening..].iter().any(splittable) {
        let mut at = opening;
        while pad > 0 && at < specs.len() {
            let spec = specs[at];
            if splittable(&spec) {
                let head = spec.count / cell / 2 * cell;
                specs[at].count = head;
                specs.insert(
                    at + 1,
                    Spec {
                        first: spec.first + head,
                        count: spec.count - head,
                        ..spec
                    },
                );
                pad -= 1;
            }
            at += 1;
        }
    }

    let mut at = specs.len() - 1;
    while pad > 0 {
        let spec = specs[at];
        let wider = Spec {
            width: spec.width + 1,
            ..spec
        };
        if spec.width < MAX_STORED_WIDTH && wider.span(units) - spec.span(units) <= pad {
            pad -= wider.span(units) - spec.span(units);
            specs[at].width += 1;
        } else if at > opening {
            at -= 1;
        } else {
            return Err(ParseError::OutOfBounds {
                value: format!("a loop of {} record(s)", specs.len() - opening),
                bound: format!(
                    "a loop with {pad} more word(s) of room in it — the encoded loop has \
                     to be whole packets long, and this one cannot be widened that far; \
                     loop over more of the audio"
                ),
            }
            .into());
        }
    }
    Ok(())
}

/// A packed stroke stream: the words, and where the header's directory points.
struct Stream {
    words: Vec<u8>,
    first_record: usize,
    resync: usize,
    /// The marked record a loop starts at, when the stroke loops.
    mark: Option<usize>,
    terminator: usize,
}

/// Right-align records in the allocation the preamble law gives this stroke:
/// `preamble` bytes of payload, then whole packets until the chain fits.
///
/// `preamble` is [`stroke::header_len`](super::stroke::header_len), which a zone table
/// can drive below the stroke header — the first packet then starts inside what would
/// otherwise be header, and the loop repays the difference.
fn pack(
    specs: &[Spec],
    values: &[i32],
    resync_record: usize,
    preamble: usize,
    plan: &Plan,
) -> Result<Stream, Error> {
    let units = plan.units();
    let (word, header) = (units.word(), plan.layout.header_len());
    let chain: usize = specs.iter().map(|s| s.span(units)).sum::<usize>() + 1;
    let need = (chain + units.min_lead())
        .checked_mul(word)
        .and_then(|bytes| bytes.checked_add(header))
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("a chain of {chain} words"),
            bound: "a stroke payload of addressable length".into(),
        })?;
    let mut payload = preamble;
    while payload < need {
        payload += packet_len(plan.layout);
    }
    if !(payload - header).is_multiple_of(word) {
        return Err(ParseError::AssertFail(format!(
            "a {preamble}-byte preamble puts the word stream off a word boundary; the \
             sections in front of the stroke are not whole words"
        ))
        .into());
    }
    let total = (payload - header) / word;
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

    let mut words = vec![0u8; total * word];
    let lead = total - chain;
    let mut at = lead;
    let mut resync = lead;
    let mut mark = None;
    for (index, spec) in specs.iter().enumerate() {
        if index == resync_record {
            resync = at;
        }
        if spec.mark {
            mark = Some(at);
        }
        write_record(&mut words, at, spec, values, units);
        at += spec.span(units);
    }
    // The terminator states the cell size, which is what says how many channels the
    // stroke carries: twice the layout's cell and a reader de-interleaves.
    if at.checked_add(1) != Some(total) {
        return Err(ParseError::AssertFail(format!(
            "the record chain ended at word {at} of {total}"
        ))
        .into());
    }
    let terminator = (1u32 << 23) | plan.cell() as u32;
    words[at * word..(at + 1) * word].copy_from_slice(&terminator.to_be_bytes()[4 - word..]);

    Ok(Stream {
        words,
        first_record: lead,
        resync,
        mark,
        terminator: at,
    })
}

/// Writes one record: its header word, then its fields, which start at the first bit
/// after it. Any alignment tail is left zero at the end of the segment.
///
/// v2 and v3 store a stereo stroke's channels as **alternating fields**, which is the
/// order `values` is already in, so the fields go down in stream order; v4 gives each
/// channel its own word stream and alternates the words, so its halves are packed
/// apart and then interleaved. Only the residual's reach moves with the channel count.
fn write_record(words: &mut [u8], at: usize, spec: &Spec, values: &[i32], units: Units) {
    let (word, bits) = (units.word(), units.word_bits());
    let head = (u32::from(spec.one_to_one) << 23)
        | (u32::from(spec.width - 1) << 19)
        | (u32::from(spec.mark) << 18)
        | (u32::from(spec.order) << 14)
        | spec.count as u32;
    words[at * word..(at + 1) * word].copy_from_slice(&head.to_be_bytes()[4 - word..]);

    let stored = |k: usize| -> u64 {
        let field = spec.first + k;
        let value = if spec.order == 0 {
            i64::from(values[field])
        } else {
            residual(values, field, spec.order, units.channels)
        };
        (value as u64) & ((1u64 << spec.width) - 1)
    };
    let put = |words: &mut [u8], mut bit: usize, raw: u64| {
        for b in (0..spec.width).rev() {
            if raw >> b & 1 != 0 {
                words[bit / 8] |= 1 << (7 - bit % 8);
            }
            bit += 1;
        }
    };

    if !units.splits() {
        for k in 0..spec.count {
            put(
                words,
                (at + 1) * bits + k * usize::from(spec.width),
                stored(k),
            );
        }
        return;
    }
    // Each channel is packed into its own contiguous words first, because the two
    // halves are padded apart; the words then alternate from the header on.
    let per = spec.count / 2;
    let half = units.half(spec.count, spec.width);
    let mut packed = vec![0u8; half * word];
    for channel in 0..2 {
        packed.fill(0);
        for k in 0..per {
            put(
                &mut packed,
                k * usize::from(spec.width),
                stored(2 * k + channel),
            );
        }
        for w in 0..half {
            let to = (at + 1 + 2 * w + channel) * word;
            words[to..to + word].copy_from_slice(&packed[w * word..(w + 1) * word]);
        }
    }
}

/// Encode `A = gain · 2^(41+s)/peak` as `(mantissa, exponent)`: the exponent carries the
/// quantiser shift, the mantissa is `1/peak` to 20 bits scaled by the zone's gain
/// ([`zone::GAIN_UNITY`](super::zone::GAIN_UNITY) is 1.0). The reciprocal is held as a
/// 24-bit fraction in `[½, 1)` — three bits finer than the mantissa — before the gain
/// multiplies it, and one floor follows; the mantissa leaves its normalised range
/// freely in either direction, and the exponent never moves with it.
fn statistic_a(peak: u32, shift: i32, gain: u32) -> (u32, u8) {
    let peak = u64::from(peak.max(1));
    let bits = 64 - peak.leading_zeros() as i32;
    let exact_power = i32::from(peak.is_power_of_two());
    let reciprocal = (1u64 << (21 + bits + (1 - exact_power))) / peak;
    let mantissa = (reciprocal * u64::from(gain)) >> (super::zone::GAIN_BITS + 3);
    (mantissa as u32, (22 + shift - bits + exact_power) as u8)
}

/// Build the fixed header and its body-relative, wrapping word directory.
#[allow(clippy::too_many_arguments)]
fn stroke_header(
    layout: Layout,
    id: u32,
    root_key: u8,
    q: &Quantised,
    stream: &Stream,
    body_at: usize,
    channels: usize,
    gain: u32,
) -> Vec<u8> {
    let mut head = vec![0u8; layout.header_len()];
    head[0..4].copy_from_slice(&id.to_be_bytes());
    head[5] = root_key;
    // Unexplained: real programs hold this, and the panel cannot produce it.
    head[6..8].copy_from_slice(&[0x88, 0xba]);
    // The channel count, stated a second time — the terminator's cell size says it too,
    // and a reader takes the terminator because that is what the record sizes follow.
    head[8] = channels as u8;

    let (mantissa, exponent) = statistic_a(q.peak.unsigned_abs(), q.shift, gain);
    head[9..12].copy_from_slice(&mantissa.to_be_bytes()[1..]);
    head[12] = exponent;
    head[13..16].copy_from_slice(&(q.peak as u32).to_be_bytes()[1..]);

    let base = (body_at + layout.header_len()) / layout.word() % WRAP;
    let pointer = |word: usize| ((base + word) % WRAP) as u16;
    // The third pointer names the loop's marked record; aimed at the terminator it says
    // the stroke does not loop.
    let directory = [
        pointer(stream.first_record),
        pointer(stream.resync),
        pointer(stream.mark.unwrap_or(stream.terminator)),
        pointer(stream.terminator),
    ];
    for (i, p) in directory.iter().enumerate() {
        let at = 20 + 9 * i;
        head[at..at + 2].copy_from_slice(&p.to_be_bytes());
        // Unexplained: real programs hold this, and the panel cannot produce it.
        if i < 3 {
            head[at + 2] = 0x80;
        }
    }
    // The wide header's two float32 tails. The editor writes this pair on every
    // stroke it renders; what they scale is unexplained.
    for (at, value) in codec::TAIL_FLOATS_AT.iter().zip(WIDE_TAIL_FLOATS) {
        if let Some(slot) = head.get_mut(*at..at + 4) {
            slot.copy_from_slice(&value.to_be_bytes());
        }
    }
    head
}

/// The two float32s a wide stroke header carries at [`codec::TAIL_FLOATS_AT`].
/// Unexplained: real programs hold this, and the panel cannot produce it.
const WIDE_TAIL_FLOATS: [f32; 2] = [0.0, 20.0];

/// Encode one zone's stroke at body offset `body_at`, packed into `preamble` bytes
/// plus whole packets.
///
/// Both placements come from the sections already sized in front of this stroke, so
/// only [`multi_zone`] can supply them: `body_at` is the base the word directory is
/// written against, and a wrong one produces a file whose directory names records
/// that are not there.
#[allow(clippy::too_many_arguments)]
fn stroke(
    layout: Layout,
    source: &[i16],
    channels: usize,
    root_key: u8,
    id: u32,
    body_at: usize,
    preamble: usize,
    predictor: Predictor,
    loops: Option<Loop>,
    secondary_start: f64,
    shift: Option<u8>,
    gain: u32,
) -> Result<Vec<u8>, Error> {
    midi_note("root key", root_key)?;
    let frames = frames_of(source, channels)?;
    body_at
        .checked_add(layout.header_len())
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("body offset {body_at}"),
            bound: "an addressable stroke header".into(),
        })?;
    let plan = match loops {
        Some(points) => Plan::looped(layout, frames, channels, points, secondary_start)?,
        None => Plan::new(layout, frames, channels, secondary_start)?,
    };
    if let Some(bits) = shift {
        if i32::from(bits) > codec::SHIFT_LIMIT {
            return Err(ParseError::OutOfBounds {
                value: format!("a quantiser shift of {bits} bits"),
                bound: format!("0 through {} bits", codec::SHIFT_LIMIT),
            }
            .into());
        }
    }
    let q = quantise(source, &plan, shift);
    let low = q.values.iter().copied().min().unwrap_or(0);
    let high = q.values.iter().copied().max().unwrap_or(0);
    if width_of(i64::from(low), i64::from(high)) > MAX_STORED_WIDTH {
        return Err(ParseError::OutOfBounds {
            value: format!(
                "a quantiser shift of {} bits for fields spanning {low}..={high}",
                q.shift
            ),
            bound: format!("values that fit the stream's {MAX_STORED_WIDTH}-bit fields"),
        }
        .into());
    }
    let specs = records(&q.values, &plan, predictor)?;
    let resync_record = specs
        .iter()
        .position(|s| s.first == plan.resync_at)
        .ok_or_else(|| {
            ParseError::AssertFail(format!(
                "no record begins at the planned resync field {}",
                plan.resync_at
            ))
        })?;
    let stream = pack(&specs, &q.values, resync_record, preamble, &plan)?;

    let mut payload = stroke_header(layout, id, root_key, &q, &stream, body_at, channels, gain);
    payload.extend_from_slice(&stream.words);
    Ok(payload)
}

/// Section schema versions the narrow chain writes. They track the section's own
/// schema rather than the content version.
const HDR_VERSION: u8 = 9;
const CAT_VERSION: u8 = 5;
const STK_VERSION: u8 = 9;
const STY_VERSION: u8 = 5;
const CONTAINER_VERSION: u8 = 11;

/// The category every chain's `cat` section opens with.
/// Unexplained: real programs hold this, and the panel cannot produce it.
const CATEGORY: u8 = 0x0f;

/// The `hdr` section: a fixed prefix, then the instrument name NUL-padded.
fn hdr(name: &str) -> Result<Section, Error> {
    let mut payload = vec![0u8; 111];
    // Unexplained: real programs hold this, and the panel cannot produce it.
    payload[0..6].copy_from_slice(&[0x00, 0x01, 0xb4, 0x00, 0x06, 0x50]);
    super::StringField::NAME.write(&mut payload, name)?;
    Ok(Section {
        tag: *section::HDR,
        version: HDR_VERSION,
        payload,
    })
}

/// The `cat` section: a short prefix and two length-prefixed labels.
fn cat() -> Section {
    let mut payload = vec![CATEGORY, 0x00, 0x00, 0x00, 0x01];
    for label in [&b"Production"[..], &b"Origin"[..]] {
        payload.push(label.len() as u8);
        payload.extend_from_slice(label);
    }
    // Every section payload is a whole number of 24-bit words; the labels are
    // padded out to one.
    while !payload.len().is_multiple_of(3) {
        payload.push(0);
    }
    Section {
        tag: *section::CAT,
        version: CAT_VERSION,
        payload,
    }
}

/// Build a neutral keyboard map — unity gain and no detune at every key —
/// and the zone table behind it.
///
/// `zones` is one record per zone, already high to low.
fn map(zones: &[ZoneRecord]) -> Section {
    let mut payload = vec![0u8; super::zone::RECORDS_AT + super::zone::RECORD_LEN * zones.len()];
    payload[..super::zone::COUNT_AT].copy_from_slice(&super::keymap::KeyTable::NEUTRAL.prefix());
    payload[super::zone::COUNT_AT] = zones.len() as u8;
    // Zones are stored high to low by top note.
    for (index, record) in zones.iter().enumerate() {
        let at = super::zone::RECORDS_AT + super::zone::RECORD_LEN * index;
        payload[at + 2] = record.id;
        // Nothing here says whether the zone loops: a zone record is byte-identical
        // either way, and the loop lives in the stroke's own word directory.
        payload[at + 3..at + 6].copy_from_slice(&record.gain.to_be_bytes()[1..]);
        payload[at + 9] = record.top_note;
        // One sample in the zone, so the playing stroke sits at the bottom of the
        // strength axis. A stack positions its enabled stroke higher; nothing the
        // builder produces has one.
        payload[at + 10..at + 12].copy_from_slice(&super::zone::REL_STRENGTH_DEFAULT.to_be_bytes());
    }
    Section {
        tag: *section::MAP,
        version: super::keymap::VERSION,
        payload,
    }
}

/// The `sty` section: nine constant bytes.
/// Unexplained: real programs hold this, and the panel cannot produce it.
fn sty() -> Section {
    Section {
        tag: *section::STY,
        version: STY_VERSION,
        payload: vec![0x00, 0x01, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00],
    }
}

/// Everything a `.nsmp3` chain and a `.nsmp4` chain do not share.
///
/// The section versions track their own schemas, so they move independently of the
/// content version and of each other. The payloads named here are constant across
/// every render of a project that does not reach them.
///
/// Inferred from specimens; not confirmed on hardware.
struct WideSchema {
    container: u32,
    /// The `NSMP` payload. Constant per generation and unrelated to the stroke count.
    /// Unexplained: real programs hold this, and the panel cannot produce it.
    container_payload: [u8; 4],
    hdr: u32,
    map: u32,
    /// Bytes one per-key record takes: the level alone, or the level and the partner
    /// quad the wider schema puts behind it.
    key_stride: usize,
    /// The unexplained run between the per-key table and the zone count.
    map_gap: &'static [u8],
    /// The unexplained run behind the last zone record.
    map_tail: &'static [u8],
    /// Whether the zone records are stored low to high. Every other layout stores
    /// them high to low.
    zones_ascend: bool,
    sty: u32,
    /// The preset a project that touches none renders as.
    /// Unexplained: real programs hold this, and the panel cannot produce it.
    sty_payload: &'static [u8],
}

/// The `map`'s gain-and-detune unit: unity gain, no detune. It opens the section as
/// the instrument's own level and then repeats once per key.
const LEVEL: [u8; 6] = [0x10, 0x00, 0x00, 0x00, 0x00, 0x00];

const STY_V3_PAYLOAD: [u8; super::sty::V3_LEN] = [
    0x00, 0x00, 0x7f, 0x1e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x7f, 0x00, 0x02, 0x00,
    0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00,
];

const STY_V4_PAYLOAD: [u8; super::sty::V4_LEN_LONG] = [
    0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1e,
    0x1e, 0x1e, 0x00, 0x00, 0x00, 0x7f, 0x7f, 0x7f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// The schema for a wide generation, `None` for the narrow chain.
fn wide_schema(layout: Layout) -> Option<WideSchema> {
    match layout {
        Layout::V2 => None,
        Layout::V3 => Some(WideSchema {
            container: 30,
            container_payload: [0x00, 0x02, 0x00, 0x0c],
            hdr: 10,
            map: 14,
            key_stride: LEVEL.len(),
            map_gap: &[],
            map_tail: &[0x00],
            zones_ascend: true,
            sty: super::sty::VERSION_V3,
            sty_payload: &STY_V3_PAYLOAD,
        }),
        Layout::V4 => Some(WideSchema {
            container: 40,
            container_payload: [0x00, 0x02, 0x00, 0x05],
            hdr: 11,
            map: 21,
            key_stride: LEVEL.len() + 4,
            map_gap: &[
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
                0x02, 0x02, 0x02, 0x10, 0x00, 0x00, 0x10, 0x00, 0x00, 0x10, 0x00, 0x00, 0x10, 0x00,
                0x00, 0x00, 0x00,
            ],
            map_tail: &[0x00, 0x00, 0x00, 0x01, 0x00, 0x00],
            zones_ascend: false,
            sty: super::sty::VERSION_V4,
            sty_payload: &STY_V4_PAYLOAD,
        }),
    }
}

/// Keys the wide `map`'s per-key table describes.
const KEYS: usize = 128;

/// The wide `hdr` section: the same prefix at a wider name field, with the sub-name
/// the vendor's filenames append left empty.
fn hdr4(schema: &WideSchema, name: &str) -> Result<Section4, Error> {
    let mut payload = vec![0u8; 112];
    // Unexplained: real programs hold this, and the panel cannot produce it.
    payload[4..6].copy_from_slice(&[0x06, 0x50]);
    super::StringField::NAME_V3.write(&mut payload, name)?;
    Ok(Section4 {
        tag: *section::HDR4,
        version: schema.hdr,
        payload,
    })
}

/// The wide `cat` section: the category alone, where the narrow chain also spells
/// out its labels.
fn cat4() -> Section4 {
    let mut payload = vec![0u8; 8];
    payload[0] = CATEGORY;
    Section4 {
        tag: *section::CAT4,
        version: 7,
        payload,
    }
}

/// The wide `map` section: a per-key table at unity gain and no detune, then the
/// zone records behind their count.
///
/// The wider schema's per-key record carries a partner quad as well as the level.
/// The editor writes the identity there whatever the zone layout — only the vendor's
/// own builder fills it in — so every quad names its own key.
fn map4(schema: &WideSchema, zones: &[WideZoneRecord]) -> Section4 {
    let mut payload = Vec::with_capacity(
        LEVEL.len()
            + KEYS * schema.key_stride
            + schema.map_gap.len()
            + 1
            + super::zone::WIDE_RECORD_LEN * zones.len()
            + schema.map_tail.len(),
    );
    payload.extend_from_slice(&LEVEL);
    for key in 0..KEYS as u8 {
        payload.extend_from_slice(&LEVEL);
        payload.extend(std::iter::repeat_n(key, schema.key_stride - LEVEL.len()));
    }
    payload.extend_from_slice(schema.map_gap);
    payload.push(zones.len() as u8);
    let ordered: Vec<&WideZoneRecord> = match schema.zones_ascend {
        true => zones.iter().rev().collect(),
        false => zones.iter().collect(),
    };
    for record in ordered {
        payload.extend_from_slice(&record.bytes());
    }
    payload.extend_from_slice(schema.map_tail);
    Section4 {
        tag: *section::MAP4,
        version: schema.map,
        payload,
    }
}

/// The wide `sty` section.
fn sty4(schema: &WideSchema) -> Section4 {
    Section4 {
        tag: *section::STY4,
        version: schema.sty,
        payload: schema.sty_payload.to_vec(),
    }
}

/// The `meta` section: the length of everything ahead of it, which is the only place
/// a wide file states its own size.
fn meta4(chain_len: usize) -> Section4 {
    let mut payload = vec![0u8; super::meta::LEN];
    payload[0..2].copy_from_slice(&2u16.to_be_bytes());
    payload[2..6].copy_from_slice(&(chain_len as u32).to_be_bytes());
    Section4 {
        tag: *section::META4,
        version: super::meta::VERSION,
        payload,
    }
}

/// One zone to build: its audio, where it sits on the keyboard, and the id its
/// record names its stroke by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NewZone<'a> {
    /// PCM at [`codec::SOURCE_RATE`], already trimmed to what the zone plays and
    /// **interleaved** when it has more than one channel.
    pub source: &'a [i16],
    /// Channels [`source`](NewZone::source) interleaves: 1 or 2.
    pub channels: u16,
    /// The note this sample plays untransposed at.
    pub root_key: u8,
    /// Highest note this zone answers to. Stored as given — the file keeps top notes,
    /// it does not derive them from the root keys.
    pub top_note: u8,
    /// The stroke's global id, 1 through [`MAX_STROKE_ID`]. Zones name their strokes
    /// by it rather than by position, so it need not run parallel to the sections.
    pub global_id: u32,
    /// The zone's sustain loop, which truncates its audio at [`Loop::end`].
    pub loops: Option<Loop>,
    /// Where the stream resynchronises: the project's `m_startSecondary` in source
    /// frames from the first frame of [`source`](NewZone::source), after the repair
    /// the editor applies on load ([`nsmpproj::Stroke::encoded_secondary_start`]).
    pub secondary_start: f64,
    /// Quantiser shift to lay the stroke out at instead of the rule's choice, or `None`
    /// for the rule. Experimental — see [`Options::shift`].
    pub shift: Option<u8>,
    /// Playback gain with [`zone::GAIN_BITS`](super::zone::GAIN_BITS) fractional bits —
    /// [`zone::GAIN_UNITY`](super::zone::GAIN_UNITY) is 1.0 — below `1 << 24`. Not
    /// applied to the audio: it goes into the zone record and the stroke's statistic A,
    /// and the instrument applies it when it plays.
    ///
    /// ⚠️ The narrow chain alone stores it. No byte of a wide zone record is known to
    /// hold a gain, so a wide build refuses anything but unity rather than write one
    /// the instrument would not apply.
    pub gain: u32,
}

/// Build a one-zone instrument from PCM at [`codec::SOURCE_RATE`], mono or stereo
/// interleaved per [`Options::channels`], in the generation [`Options::layout`] names.
/// Refuses unmodelled lengths, invalid metadata, and streams past the directory limit.
pub fn instrument(source: &[i16], options: &Options) -> Result<crate::Sample, Error> {
    midi_note("root key", options.root_key)?;
    let frames = frames_of(source, usize::from(options.channels))?;
    let secondary_start = options
        .secondary_start
        .unwrap_or_else(|| default_secondary_start(frames, options.loops));
    multi_zone(
        &[NewZone {
            source,
            channels: options.channels,
            root_key: options.root_key,
            top_note: options.resolved_top_note(),
            global_id: 1,
            loops: options.loops,
            secondary_start,
            shift: options.shift,
            gain: super::zone::GAIN_UNITY,
        }],
        &options.name,
        options.predictor,
        options.layout,
    )
}

/// Build an instrument that spans the keyboard: one `stk` per zone, in the order
/// given, which must be highest zone first.
///
/// Refuses an empty or overlapping zone list, a duplicate or unnameable stroke id,
/// and everything [`instrument`] refuses about one zone's audio.
pub fn multi_zone(
    zones: &[NewZone<'_>],
    name: &str,
    predictor: Predictor,
    layout: Layout,
) -> Result<crate::Sample, Error> {
    match wide_schema(layout) {
        Some(schema) => wide_chain(zones, name, predictor, layout, &schema).map(crate::Sample::V3),
        None => narrow_chain(zones, name, predictor).map(crate::Sample::V2),
    }
}

/// The CBIN header every generation writes, at its own content version.
fn container(layout: Layout) -> Header {
    Header {
        generation: Generation::V1,
        tag: *b"nsmp",
        location: 0xFFFF_FFFF,
        aux: AUX,
        version: version(layout),
    }
}

fn narrow_chain(
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
            Layout::V2,
            zone.source,
            usize::from(zone.channels),
            zone.root_key,
            zone.global_id,
            body_at + section::HEADER_LEN,
            super::stroke::header_len(Layout::V2, index, cat_len, map_len),
            predictor,
            zone.loops,
            zone.secondary_start,
            zone.shift,
            zone.gain,
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
        header: container(Layout::V2),
        body: Sample { sections },
    })
}

/// The `stk` schema version both wide generations carry.
const STK4_VERSION: u32 = 11;

fn wide_chain(
    zones: &[NewZone<'_>],
    name: &str,
    predictor: Predictor,
    layout: Layout,
    schema: &WideSchema,
) -> Result<Cbin<SampleV3>, Error> {
    let table = wide_zone_table(zones)?;
    let hdr = hdr4(schema, name)?;
    let cat = cat4();
    let map = map4(schema, &table);
    let cat_len = cat.payload.len();
    let map_len = map.payload.len();

    let mut sections = vec![
        Section4 {
            tag: *section::CONTAINER4,
            version: schema.container,
            payload: schema.container_payload.to_vec(),
        },
        hdr,
        cat,
        map,
    ];
    let mut body_at: usize = sections.iter().map(Section4::encoded_len).sum();
    for (index, zone) in zones.iter().enumerate() {
        let payload = stroke(
            layout,
            zone.source,
            usize::from(zone.channels),
            zone.root_key,
            zone.global_id,
            body_at + section::HEADER4_LEN,
            super::stroke::header_len(layout, index, cat_len, map_len),
            predictor,
            zone.loops,
            zone.secondary_start,
            zone.shift,
            zone.gain,
        )?;
        body_at += section::HEADER4_LEN + payload.len();
        sections.push(Section4 {
            tag: *section::STK4,
            version: STK4_VERSION,
            payload,
        });
    }
    sections.push(sty4(schema));
    let chain_len: usize = sections.iter().map(Section4::encoded_len).sum();
    sections.push(meta4(chain_len));

    Ok(Cbin {
        header: container(layout),
        body: SampleV3 { sections },
    })
}

/// What a wide `map` section stores per zone.
struct WideZoneRecord {
    root_key: u8,
    top_note: u8,
    low_note: u8,
    global_id: u32,
}

impl WideZoneRecord {
    fn bytes(&self) -> [u8; super::zone::WIDE_RECORD_LEN] {
        let mut r = [0u8; super::zone::WIDE_RECORD_LEN];
        r[0] = self.root_key;
        r[1] = self.top_note;
        r[2] = self.low_note;
        // Unexplained: real programs hold this, and the panel cannot produce it.
        r[7] = 1;
        r[8..12].copy_from_slice(&self.global_id.to_be_bytes());
        // One sample in the zone, so the playing stroke sits at the bottom of the
        // strength axis.
        r[12..14].copy_from_slice(&super::zone::REL_STRENGTH_DEFAULT.to_be_bytes());
        let full = super::zone::VelocityWindow::FULL;
        r[14] = full.low;
        r[15] = full.high;
        r
    }
}

/// Validate the zone list and reduce it to the records a wide `map` stores.
///
/// A wide zone states its own bottom as well as its top, and zones tile: each reaches
/// down to one above the zone below it, and the lowest reaches the keyboard's floor.
fn wide_zone_table(zones: &[NewZone<'_>]) -> Result<Vec<WideZoneRecord>, Error> {
    let table = zone_table(zones)?;
    for (index, zone) in zones.iter().enumerate() {
        if zone.gain != super::zone::GAIN_UNITY {
            return Err(ParseError::AssertFail(format!(
                "zone {index} sets a gain, and no byte of a wide zone record is known \
                 to hold one"
            ))
            .into());
        }
    }
    Ok(table
        .iter()
        .enumerate()
        .map(|(index, record)| WideZoneRecord {
            root_key: zones[index].root_key,
            top_note: record.top_note,
            low_note: match table.get(index + 1) {
                Some(below) => below.top_note.saturating_add(1),
                None => super::zone::KEY_FLOOR,
            },
            global_id: zones[index].global_id,
        })
        .collect())
}

/// What the `map` section stores per zone.
struct ZoneRecord {
    id: u8,
    top_note: u8,
    gain: u32,
}

/// Validate the zone list and reduce it to the records the `map` section stores.
fn zone_table(zones: &[NewZone<'_>]) -> Result<Vec<ZoneRecord>, Error> {
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
        if zone.gain >> 24 != 0 {
            return Err(ParseError::OutOfBounds {
                value: format!(
                    "zone {index} gain {}/{}",
                    zone.gain,
                    super::zone::GAIN_UNITY
                ),
                bound: "below 16.0, the most a zone record's 24-bit gain holds".into(),
            }
            .into());
        }
        let id = zone.global_id as u8;
        if table.iter().any(|seen: &ZoneRecord| seen.id == id) {
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
        table.push(ZoneRecord {
            id,
            top_note: zone.top_note,
            gain: zone.gain,
        });
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
    use super::super::zone::GAIN_UNITY;
    use super::*;

    /// The narrow chain's own units, which most of these tests are written against.
    const CELL: usize = Layout::V2.cell();
    const CHUNK: usize = Layout::V2.rmax();
    const HEADER_LEN: usize = Layout::V2.header_len();
    const PACKET_LEN: usize = packet_len(Layout::V2);
    const VERSION: u32 = version(Layout::V2);
    const MONO: Units = Units {
        layout: Layout::V2,
        channels: 1,
    };
    const PACKET_WORDS: usize = MONO.packet_words();

    /// The narrow body an encode produced. These tests build no wide one.
    fn narrow(sample: crate::Sample) -> Cbin<Sample> {
        match sample {
            crate::Sample::V2(file) => file,
            crate::Sample::V3(_) => panic!("the narrow chain was asked for"),
        }
    }

    fn built(
        zones: &[NewZone<'_>],
        name: &str,
        predictor: Predictor,
    ) -> Result<Cbin<Sample>, Error> {
        multi_zone(zones, name, predictor, Layout::V2).map(narrow)
    }

    fn plan(frames: usize, channels: usize) -> Result<Plan, Error> {
        Plan::new(
            Layout::V2,
            frames,
            channels,
            default_secondary_start(frames, None),
        )
    }

    fn looped(frames: usize, channels: usize, points: Loop) -> Result<Plan, Error> {
        Plan::looped(
            Layout::V2,
            frames,
            channels,
            points,
            default_secondary_start(frames, Some(points)),
        )
    }

    fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<i16> {
        (0..frames)
            .map(|k| {
                let t = k as f64 / f64::from(codec::SOURCE_RATE);
                (amplitude * (2.0 * std::f64::consts::PI * hz * t).sin()).round() as i16
            })
            .collect()
    }

    fn encoded(source: &[i16], predictor: Predictor) -> Cbin<Sample> {
        narrow(instrument(source, &Options::new("Test").predictor(predictor)).unwrap())
    }

    #[test]
    fn the_band_is_the_shortest_run_a_whole_number_of_records_can_cover() {
        for channels in [1usize, 2] {
            let (cell, rmax) = (CELL * channels, CHUNK * channels);
            for r in 0..2000usize {
                let b = band(r, cell, rmax);
                assert_eq!(b % cell, r % cell, "{channels}ch r {r}");
                assert!(b >= cell, "band({r}) = {b}");
                let records = (1..=8).find(|j| j * cell <= b && b <= j * rmax);
                assert!(records.is_some(), "{channels}ch band({r}) = {b}");
                for shorter in (cell..b).filter(|s| s % cell == b % cell) {
                    assert!(
                        !(1..=8).any(|j| j * cell <= shorter && shorter <= j * rmax),
                        "{channels}ch band({r}) = {b}, but {shorter} is reachable"
                    );
                }
            }
            assert_eq!(band(0, cell, rmax), cell);
            assert_eq!(band(cell, cell, rmax), cell);
        }
    }

    #[test]
    fn every_one_to_one_chunk_is_a_legal_count() {
        for channels in [1usize, 2] {
            let (cell, rmax) = (CELL * channels, CHUNK * channels);
            for r in 0..2000usize {
                let run = band(r, cell, rmax);
                let split = chunks(run, rmax);
                assert_eq!(split.iter().sum::<usize>(), run, "band({r})");
                for c in split {
                    assert!((cell..=rmax).contains(&c), "band({r}) chunk {c}");
                }
            }
        }
    }

    #[test]
    fn the_plan_covers_every_field_exactly_once() {
        for frames in [4096, 8192, 10_000, 44_100, 100_000, 441_000] {
            let p = plan(frames, 1).unwrap();
            assert_eq!(
                p.warmup + CELL * p.cells_before + p.resync + CELL * p.cells_after,
                p.fields,
                "{frames} frames"
            );
            assert_eq!(p.warmup + CELL * p.cells_before, p.resync_at);
        }
    }

    // Landmarks read off Nord Sample Editor renders of self-generated audio whose
    // projects state the fresh default, `m_startSecondary = m_stop / 8`, from
    // `m_start = 1`: a 44 100-frame mono sine and a 30 870-frame stereo pair.
    #[test]
    fn the_resync_lands_where_the_projects_secondary_start_says() {
        let mono = Plan::new(Layout::V2, 44_099, 1, 5_512.5 - 1.0).unwrap();
        assert_eq!(
            (mono.fields, mono.warmup, mono.resync_at, mono.resync),
            (35_128, 30, 4_374, 58)
        );
        let both = Plan::new(Layout::V2, 30_869, 2, 3_858.75 - 1.0).unwrap();
        assert_eq!(
            (both.fields, both.warmup, both.resync_at, both.resync),
            (49_256, 124, 6_124, 124)
        );
        // Half-up on the lattice: 11 025 frames land on exactly 8 750.5 fields.
        assert_eq!(
            Plan::new(Layout::V2, 88_200, 1, 11_025.0)
                .unwrap()
                .resync_at,
            8_751
        );
    }

    #[test]
    fn a_secondary_start_the_stream_cannot_resync_at_is_refused() {
        for at in [0.0, 20.0, 50_000.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                Plan::new(Layout::V2, 44_100, 1, at).is_err(),
                "secondary start {at}"
            );
        }
        assert!(Plan::looped(Layout::V2, 44_100, 1, Loop::new(8_192, 40_000), 8_192.0).is_err());
        assert!(Plan::looped(Layout::V2, 44_100, 1, Loop::new(8_192, 40_000), 4_096.0).is_ok());
    }

    #[test]
    fn audio_without_a_project_resyncs_where_a_fresh_project_would() {
        assert_eq!(default_secondary_start(44_100, None), 5_512.5);
        assert_eq!(
            default_secondary_start(44_100, Some(Loop::new(1_000, 40_000))),
            500.0
        );
        assert_eq!(
            default_secondary_start(44_100, Some(Loop::new(0, 40_000))),
            nsmpproj::MIN_SECONDARY_START
        );
        let stated = instrument(
            &vec![0i16; 44_100],
            &Options::new("Stated").secondary_start(5_521.281862),
        )
        .unwrap();
        let fresh = instrument(&vec![0i16; 44_100], &Options::new("Stated")).unwrap();
        assert_ne!(stated.stroke_streams()[0].1, fresh.stroke_streams()[0].1);
    }

    #[test]
    fn the_stream_opens_on_a_cubic_ramp() {
        let mut fields = vec![-4_000i64; 40];
        ramp_in(&mut fields);
        assert_eq!(fields[0], 0);
        assert_eq!(fields[7], -4_000 * 343 / 42_875);
        assert_eq!(fields[34], -4_000 * 39_304 / 42_875);
        assert!(fields[..RAMP_IN].windows(2).all(|w| w[0] >= w[1]));
        assert!(fields[RAMP_IN..].iter().all(|&v| v == -4_000));
    }

    #[test]
    fn a_width_tie_goes_to_the_lowest_order_unless_a_record_already_holds_it() {
        let widths = [13, 10, 7, 4, 4];
        assert_eq!(choose_order(&widths, None), (3, 4));
        assert_eq!(choose_order(&widths, Some((4, 4))), (4, 4));
        assert_eq!(choose_order(&widths, Some((4, 3))), (3, 4), "width changed");
        assert_eq!(choose_order(&widths, Some((2, 4))), (3, 4));
        assert_eq!(choose_order(&[9], Some((3, 9))), (0, 9));
        // C(k, 3): the third difference is 1 everywhere and the fourth is 0, so orders
        // 3 and 4 both fit width 2.
        let values: Vec<i32> = (0..48).map(|k| k * (k - 1) * (k - 2) / 6).collect();
        assert_eq!(
            widths_at(&values, 8, Predictor::Minimising, CELL, 1)[3..],
            [MIN_WIDTH, MIN_WIDTH]
        );
        assert_eq!(widths_at(&values, 8, Predictor::Plain, CELL, 1).len(), 1);
    }

    #[test]
    fn short_input_is_refused_rather_than_guessed_at() {
        assert!(plan(MIN_FRAMES - 1, 1).is_err());
        assert!(plan(MIN_FRAMES, 1).is_ok());
        assert!(plan(usize::MAX, 1).is_err());
        assert!(instrument(&vec![0i16; 1024], &Options::new("Test")).is_err());
    }

    #[test]
    fn forced_shifts_that_cannot_be_encoded_are_refused() {
        let mut step = vec![i16::MIN; MIN_FRAMES];
        step[MIN_FRAMES / 2..].fill(i16::MAX);
        assert!(instrument(&step, &Options::new("Test").shift(0)).is_err());
        assert!(instrument(
            &vec![0i16; MIN_FRAMES],
            &Options::new("Test").shift(codec::SHIFT_LIMIT as u8 + 1)
        )
        .is_err());
    }

    #[test]
    fn midi_notes_outside_the_wire_range_are_refused() {
        let source = vec![0i16; MIN_FRAMES];
        assert!(instrument(&source, &Options::new("Test").root_key(128)).is_err());
        assert!(instrument(&source, &Options::new("Test").top_note(255)).is_err());
        assert!(stroke(
            Layout::V2,
            &source,
            1,
            128,
            1,
            0,
            165,
            Predictor::Plain,
            None,
            default_secondary_start(source.len(), None),
            None,
            GAIN_UNITY
        )
        .is_err());
    }

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
        let head = super::super::stroke::header_len(Layout::V2, 0, cat_len, map_len);
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
                let plan = plan(source.len(), 1).unwrap();
                let q = quantise(&source, &plan, None);

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
            mark: false,
            first: 0,
            count: 30,
        };
        let tail = spec.span(MONO) * 24 - 24 - spec.count * usize::from(spec.width);
        assert_eq!(tail, 18, "this spec is chosen to leave a tail");

        let values: Vec<i32> = (0..30).map(|k| k * 7 - 40).collect();
        let mut words = vec![0u8; spec.span(MONO) * 3];
        write_record(&mut words, 0, &spec, &values, MONO);

        // The tail is the last `tail` bits of the segment, and nothing is in it.
        let total = spec.span(MONO) * 24;
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
        let end = (HEADER_LEN / 3 + spec.span(MONO)) as u16;
        for (i, p) in [HEADER_LEN as u16 / 3, 0, end, end].iter().enumerate() {
            stroke[20 + 9 * i..22 + 9 * i].copy_from_slice(&p.to_be_bytes());
        }
        let walked = codec::walk(&stroke, 0, codec::Layout::V2).unwrap();
        assert_eq!(walked.records[0].values, values);
    }

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
        assert_eq!(record.first_field, plan(50_000, 1).unwrap().resync_at);
    }

    #[test]
    fn the_header_states_the_shift_it_quantised_at() {
        for amplitude in [40.0, 900.0, 8000.0, 32_000.0] {
            let source = sine(440.0, amplitude, 20_000);
            let plan = plan(source.len(), 1).unwrap();
            let file = encoded(&source, Predictor::Plain);
            let (_, stroke) = file.stroke_streams()[0];
            let q = quantise(&source, &plan, None);
            assert_eq!(
                codec::shift(stroke, codec::Layout::V2),
                Some(q.shift),
                "amplitude {amplitude}"
            );
            assert_eq!(codec::peak(stroke, codec::Layout::V2), Some(q.peak));
            assert!(q.shift >= 0);
        }
    }

    #[test]
    fn the_shift_tracks_how_loud_the_content_is() {
        let quiet = plan(20_000, 1)
            .map(|p| quantise(&sine(440.0, 500.0, 20_000), &p, None).shift)
            .unwrap();
        let loud = plan(20_000, 1)
            .map(|p| quantise(&sine(440.0, 32_000.0, 20_000), &p, None).shift)
            .unwrap();
        assert_eq!(quiet, 0);
        assert!(loud > quiet, "loud {loud} vs quiet {quiet}");
    }

    #[test]
    fn a_stereo_stroke_stops_shifting_where_its_peak_fits() {
        let frames = 30_000;
        let left = sine(220.0, 12_000.0, frames);
        let right = sine(330.0, 12_000.0, frames);
        let both: Vec<i16> = left
            .iter()
            .zip(&right)
            .flat_map(|(&l, &r)| [l, r])
            .collect();
        let mono = quantise(&left, &plan(frames, 1).unwrap(), None);
        let stereo = quantise(&both, &plan(frames, 2).unwrap(), None);
        assert_eq!(stereo.shift, 1);
        let widest = stereo
            .values
            .iter()
            .map(|v| width_of(i64::from(*v), i64::from(*v)))
            .max()
            .unwrap();
        assert_eq!(widest, PEAK_WIDTH);
        // The mono stroke sees the same peak and may spend one further bit on top.
        assert!((stereo.shift..=stereo.shift + 1).contains(&mono.shift));
    }

    /// A stroke whose only loud field sits at `field`, resynchronising at `resync`.
    fn probe(layout: Layout, resync: usize, field: usize) -> (Plan, Vec<i64>) {
        let frames = 100_000;
        let secondary = resync as f64 * f64::from(PITCH_NUM) / f64::from(PITCH_DEN);
        let plan = Plan::new(layout, frames, 1, secondary).unwrap();
        assert_eq!(plan.resync_at, resync);
        let mut values = vec![0i64; plan.fields];
        values[field] = 1 << (PEAK_WIDTH - 2);
        (plan, values)
    }

    #[test]
    fn only_a_run_s_last_record_buys_the_extra_bit() {
        // The resync run at 5464 is 89 fields: [0, 32), [32, 64), [64, 89).
        let (plan, values) = probe(Layout::V2, 5464, 5464 + 76);
        assert!(spends_extra_bit(&values, &plan));
        for offset in [12, 61, 89, 95] {
            let (plan, values) = probe(Layout::V2, 5464, 5464 + offset);
            assert!(!spends_extra_bit(&values, &plan), "run offset {offset}");
        }
    }

    #[test]
    fn three_last_record_widths_never_buy_it() {
        // 5464's opening run is 64 fields — [0, 32), [32, 64) — and a 32-field last
        // record is dead, where 4762's 58-field run ends in a live 26-field one.
        let (dead, values) = probe(Layout::V2, 5464, 40);
        assert_eq!(dead.warmup, 64);
        assert!(!spends_extra_bit(&values, &dead));
        let (live, values) = probe(Layout::V2, 4762, 40);
        assert_eq!(live.warmup, 58);
        assert!(spends_extra_bit(&values, &live));
    }

    /// A v3 opening run of `width` fields is one record, and `resync mod 32` picks the
    /// width, so this walks every last record a v3 run can end in.
    fn v3_opening_run(width: usize) -> (Plan, Vec<i64>) {
        let (plan, values) = probe(Layout::V3, 4096 + width - 32, width - 1);
        assert_eq!(plan.warmup, width);
        (plan, values)
    }

    #[test]
    fn six_of_the_seventeen_v3_last_record_widths_never_buy_it() {
        const LIVE: [usize; 11] = [33, 34, 35, 36, 37, 38, 39, 40, 42, 44, 46];
        for width in 32..=48 {
            let (plan, values) = v3_opening_run(width);
            assert_eq!(
                spends_extra_bit(&values, &plan),
                LIVE.contains(&width),
                "opening run of {width} fields"
            );
        }
    }

    #[test]
    fn a_v4_mono_stroke_never_buys_the_extra_bit() {
        for width in 32..=48 {
            let (plan, values) = probe(Layout::V4, 4096 + width - 32, width - 1);
            assert_eq!(plan.warmup, width);
            assert!(
                !spends_extra_bit(&values, &plan),
                "opening run of {width} fields"
            );
        }
    }

    #[test]
    fn the_extra_bit_narrows_the_stroke_the_header_declares() {
        let loud = header_shift(&sine(440.0, 12_000.0, 44_100), 1);
        let quiet = header_shift(&sine(440.0, 3_000.0, 44_100), 1);
        assert_eq!(loud - quiet, 2);
    }

    fn header_shift(source: &[i16], channels: u16) -> i32 {
        let options = Options::new("Shift")
            .channels(channels)
            .predictor(Predictor::Minimising);
        let file = instrument(source, &options).unwrap();
        let (_, stroke) = file.stroke_streams()[0];
        codec::shift(stroke, codec::Layout::V2).unwrap()
    }

    #[test]
    fn statistic_b_takes_the_sign_of_the_extreme_field() {
        let frames = 20_000;
        let mut up = vec![0i16; frames];
        up[10_000] = 13;
        let down: Vec<i16> = up.iter().map(|v| -v).collect();
        let positive = quantise(&up, &plan(frames, 1).unwrap(), None).peak;
        let negative = quantise(&down, &plan(frames, 1).unwrap(), None).peak;
        assert_eq!(positive, 2);
        assert_eq!(negative, 3);
        let opposed: Vec<i16> = up.iter().zip(&down).flat_map(|(&l, &r)| [l, r]).collect();
        let stereo = quantise(&opposed, &plan(frames, 2).unwrap(), None).peak;
        assert_eq!(stereo, positive);
    }

    #[test]
    fn no_field_overflows_the_width_its_record_declares() {
        for predictor in [Predictor::Plain, Predictor::Minimising] {
            let source = sine(440.0, 32_000.0, 30_000);
            let plan = plan(source.len(), 1).unwrap();
            let q = quantise(&source, &plan, None);
            for spec in records(&q.values, &plan, predictor).unwrap() {
                let limit = 1i64 << (spec.width - 1);
                for k in 0..spec.count {
                    let v = if spec.order == 0 {
                        i64::from(q.values[spec.first + k])
                    } else {
                        residual(&q.values, spec.first + k, spec.order, 1)
                    };
                    assert!((-limit..limit).contains(&v), "{spec:?} field {k} = {v}");
                }
                assert!(spec.width <= PEAK_WIDTH || spec.order > 0);
            }
        }
    }

    #[test]
    fn records_tile_the_lattice_the_way_the_laws_say() {
        let source = sine(440.0, 20_000.0, 60_000);
        let plan = plan(source.len(), 1).unwrap();
        let q = quantise(&source, &plan, None);
        let specs = records(&q.values, &plan, Predictor::Plain).unwrap();

        let mut at = 0;
        for spec in &specs {
            assert_eq!(spec.first, at);
            if !spec.one_to_one {
                assert_eq!(spec.count % CELL, 0);
                assert!(spec.count <= MAX_COUNT);
            }
            at += spec.count;
        }
        assert_eq!(at, plan.fields);
        let one_to_one: usize = specs.iter().filter(|s| s.one_to_one).map(|s| s.count).sum();
        assert_eq!(one_to_one, plan.warmup + plan.resync);
    }

    #[test]
    fn the_minimising_predictor_narrows_smooth_material() {
        let source = sine(60.0, 30_000.0, 60_000);
        let plan = plan(source.len(), 1).unwrap();
        let q = quantise(&source, &plan, None);
        let plain = records(&q.values, &plan, Predictor::Plain).unwrap();
        let minimised = records(&q.values, &plan, Predictor::Minimising).unwrap();

        let bits = |specs: &[Spec]| -> usize { specs.iter().map(|s| s.span(MONO)).sum() };
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

    #[test]
    fn a_residual_integrates_back_to_the_field_it_came_from() {
        let values: Vec<i32> = (0..200).map(|k| (k * k / 7) % 501 - 250).collect();
        for order in 1..DIFFERENCE.len() as u8 {
            for at in usize::from(order)..values.len() {
                let mut v = residual(&values, at, order, 1);
                for (j, &c) in DIFFERENCE[usize::from(order)].iter().enumerate().skip(1) {
                    v -= i64::from(c) * i64::from(values[at - j]);
                }
                assert_eq!(v, i64::from(values[at]), "order {order} at {at}");
            }
        }
    }

    #[test]
    fn statistic_a_round_trips_the_shift() {
        for peak in [0u32, 1, 2, 255, 4095, 4096, 8191, 8192] {
            for shift in 0..6 {
                let (mantissa, exponent) = statistic_a(peak, shift, GAIN_UNITY);
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
        let stereo = instrument(
            &vec![0i16; 2 * MIN_FRAMES],
            &Options::new("Test").channels(2),
        )
        .unwrap();
        assert_eq!(stereo.stroke_streams()[0].1[6..9], [0x88, 0xba, 0x02]);
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
            channels: 1,
            root_key,
            top_note,
            global_id,
            loops: None,
            secondary_start: default_secondary_start(source.len(), None),
            shift: None,
            gain: GAIN_UNITY,
        }
    }

    #[test]
    fn statistic_a_scales_a_24_bit_reciprocal_by_the_gain() {
        assert_eq!(statistic_a(4096, 2, GAIN_UNITY), (524_288, 12));
        assert_eq!(statistic_a(4096, 2, GAIN_UNITY / 2), (262_144, 12));
        assert_eq!(statistic_a(4096, 2, 2 * GAIN_UNITY), (1_048_576, 12));
        assert_eq!(statistic_a(1225, 0, 1_436_549), (1_200_837, 11));
        assert_eq!(statistic_a(4195, 2, 8_378_122), (8_180_401, 11));
        assert_eq!(statistic_a(1225, 0, 5_557_453), (4_645_576, 11));
    }

    #[test]
    fn a_zone_gain_scales_statistic_a_and_touches_nothing_else() {
        let source = sine(440.0, 12_000.0, 20_000);
        let unity = built(&[zone(&source, 60, 127, 1)], "Gain", Predictor::Plain).unwrap();
        let half = NewZone {
            gain: GAIN_UNITY / 2,
            ..zone(&source, 60, 127, 1)
        };
        let halved = built(&[half], "Gain", Predictor::Plain).unwrap();
        let (_, a) = unity.stroke_streams()[0];
        let (_, b) = halved.stroke_streams()[0];
        assert_eq!(a[..9], b[..9]);
        assert_eq!(a[12..], b[12..]);
        let mantissa = |s: &[u8]| u32::from_be_bytes([0, s[9], s[10], s[11]]);
        assert_eq!(mantissa(b), mantissa(a) / 2);
        assert_eq!(unity.zones().unwrap()[0].gain, GAIN_UNITY);
        assert_eq!(halved.zones().unwrap()[0].gain, GAIN_UNITY / 2);

        let over = NewZone {
            gain: 1 << 24,
            ..zone(&source, 60, 127, 1)
        };
        assert!(built(&[over], "Gain", Predictor::Plain).is_err());
    }

    #[test]
    fn every_zone_reads_back_paired_to_its_own_stroke() {
        let high = sine(880.0, 12_000.0, 12_000);
        let mid = sine(440.0, 12_000.0, 9_000);
        let low = sine(220.0, 12_000.0, 15_000);
        let file = built(
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
            let plan = plan(source.len(), 1).unwrap();
            let q = quantise(source, &plan, None);
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
        let alone = narrow(instrument(&source, &Options::new("One").root_key(60)).unwrap());
        let crowd = built(
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
            let file = built(&zones, "Ladder", Predictor::Plain).unwrap();
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
                let head = super::super::stroke::header_len(Layout::V2, index, cat_len, map_len);
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
        let one = |root, top, id| built(&[zone(&source, root, top, id)], "x", Predictor::Plain);
        assert!(built(&[], "x", Predictor::Plain).is_err());
        assert!(one(60, 84, 0).is_err(), "id zero names no stroke");
        assert!(one(60, 84, 256).is_err(), "id past the record's one byte");
        assert!(one(60, 128, 1).is_err());
        assert!(one(128, 84, 1).is_err());
        assert!(one(60, 84, 1).is_ok());

        let pair = |tops: [u8; 2], ids: [u32; 2]| {
            built(
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

    #[test]
    fn a_looped_plan_covers_every_field_exactly_once() {
        for (frames, start, end) in [
            (88_200, 16_384, 32_768),
            (88_200, 4_096, 20_480),
            (88_200, 0, 16_384),
            (88_200, 43_981, 60_365),
            (44_100, 20_000, 44_100),
        ] {
            let plan = looped(frames, 1, Loop::new(start, end)).unwrap();
            let points = plan.looped.unwrap();
            assert_eq!(
                plan.warmup + CELL * plan.cells_before + plan.resync + CELL * plan.cells_after,
                points.at,
                "{start}..{end}: the pre-roll does not reach the loop"
            );
            assert_eq!(
                points.at + points.warmup + CELL * points.cells,
                plan.fields,
                "{start}..{end}: the loop does not reach the terminator"
            );
            assert_eq!(points.at - fields_of(start).unwrap(), points.lead);
        }
    }

    #[test]
    fn a_loop_comes_back_the_length_it_asked_for() {
        let source = sine(220.0, 18_000.0, 88_200);
        for (start, end) in [
            (16_384, 32_768),
            (16_384, 17_408),
            (43_981, 60_365),
            (4_096, 20_480),
            (65_536, 81_920),
        ] {
            let file = instrument(
                &source,
                &Options::new("Looped").loops(Loop::new(start, end)),
            )
            .unwrap();
            let (at, stroke) = file.stroke_streams()[0];
            let walk = codec::walk(stroke, at, codec::Layout::V2).unwrap();
            let mark = walk.records.iter().find(|r| r.mark).unwrap();
            let frames = (walk.fields - mark.first_field) as f64 * f64::from(codec::SOURCE_RATE)
                / f64::from(codec::FIELD_RATE);
            assert!(
                (frames - (end - start) as f64).abs() < 1.0,
                "loop {start}..{end} came back {frames} frames long"
            );
        }
    }

    #[test]
    fn the_loop_starts_a_packet_and_the_directory_says_so() {
        let source = sine(330.0, 14_000.0, 60_000);
        for (start, end) in [(8_192, 24_576), (20_000, 40_000), (4_096, 59_000)] {
            for predictor in [Predictor::Plain, Predictor::Minimising] {
                let file = instrument(
                    &source,
                    &Options::new("Looped")
                        .predictor(predictor)
                        .loops(Loop::new(start, end)),
                )
                .unwrap();
                let (at, stroke) = file.stroke_streams()[0];
                let walk = codec::walk(stroke, at, codec::Layout::V2).unwrap();
                let directory = codec::Directory::read(stroke).unwrap();
                let marked: Vec<_> = walk.records.iter().filter(|r| r.mark).collect();
                assert_eq!(marked.len(), 1, "{start}..{end} {predictor:?}");
                assert_eq!(
                    codec::Directory::resolve(directory.mark, at, codec::Layout::V2),
                    marked[0].at
                );
                assert_ne!(directory.mark, directory.terminator);
                assert_eq!(
                    (walk.terminator - marked[0].at) % PACKET_WORDS,
                    0,
                    "{start}..{end} {predictor:?}: {} words",
                    walk.terminator - marked[0].at
                );
            }
        }
    }

    #[test]
    fn an_unlooped_stroke_marks_nothing() {
        let file = encoded(&sine(440.0, 9_000.0, 44_100), Predictor::Plain);
        let (at, stroke) = file.stroke_streams()[0];
        let directory = codec::Directory::read(stroke).unwrap();
        assert_eq!(directory.mark, directory.terminator);
        assert!(codec::walk(stroke, at, codec::Layout::V2)
            .unwrap()
            .records
            .iter()
            .all(|r| !r.mark));
    }

    #[test]
    fn the_tail_repeats_the_loops_opening() {
        let source = sine(200.0, 20_000.0, 88_200);
        let plan = looped(source.len(), 1, Loop::new(16_384, 32_768)).unwrap();
        let points = plan.looped.unwrap();
        let values = quantise(&source, &plan, None).values;
        assert_eq!(
            values[plan.fields - points.lead..],
            values[points.at - points.lead..points.at]
        );
    }

    // (loop length, crossfade frames, fields the ramp covers).
    // Inferred from specimens; not confirmed on hardware.
    const MEASURED_FADES: &[(usize, f64, usize)] = &[
        (8_192, 81.92, 65),
        (8_192, 163.84, 130),
        (8_192, 409.6, 325),
        (8_192, 819.2, 650),
        (8_192, 1_638.4, 1_300),
        (8_192, 2_048.0, 1_626),
        (8_192, 3_276.8, 2_601),
        (8_192, 4_096.0, 3_251),
        (8_192, 6_144.0, 4_877),
        (8_192, 8_192.0, 6_502),
        (2_048, 512.0, 406),
        (4_096, 1_024.0, 813),
        (16_384, 4_096.0, 3_251),
        (32_768, 8_192.0, 6_502),
        (7_000, 700.0, 556),
        (10_000, 1_000.0, 794),
        (4_096, 409.6, 325),
        (1_024, 409.6, 325),
        (16_384, 256.0, 203),
        (16_384, 1_024.0, 813),
        (16_384, 8_192.0, 6_502),
    ];

    #[test]
    fn the_fade_opens_where_the_editors_own_renders_open_it() {
        for &(length, crossfade, want) in MEASURED_FADES {
            let points = Loop::new(16_384, 16_384 + length).crossfade(crossfade);
            let plan = looped(88_200, 1, points).unwrap();
            assert_eq!(
                plan.looped.unwrap().crossfade,
                want,
                "a {crossfade} frame fade in a {length} frame loop"
            );
        }
    }

    #[test]
    fn the_crossfade_ramps_linearly_into_the_material_before_the_loop() {
        let source = sine(150.0, 22_000.0, 88_200);
        let points = Loop::new(16_384, 32_768);
        let plan = looped(source.len(), 1, points).unwrap();
        let faded = looped(source.len(), 1, points.crossfade(4_096.0)).unwrap();
        let (plain, mixed) = (
            quantise(&source, &plan, None).values,
            quantise(&source, &faded, None).values,
        );
        assert_eq!(plain.len(), mixed.len());

        let loop_at = faded.looped.unwrap();
        let end = faded.fields - loop_at.lead;
        let length = faded.fields - loop_at.at;
        let span = loop_at.crossfade;
        assert!(span > 3_000, "the fade is {span} fields");
        // Untouched in front of the fade, and the fade itself is the ramp.
        assert_eq!(plain[..end - span], mixed[..end - span]);
        for k in 0..span {
            let f = end - span + k;
            let (near, far) = (f64::from(plain[f]), f64::from(plain[f - length]));
            let u = k as f64 / span as f64;
            let want = near + (far - near) * u;
            assert!(
                (f64::from(mixed[f]) - want).abs() <= 1.0,
                "field {f}: {} against {want}",
                mixed[f]
            );
        }
    }

    #[test]
    fn a_crossfade_may_begin_before_the_loop_start() {
        let source = sine(150.0, 22_000.0, 60_000);
        let points = Loop::new(16_384, 24_576).crossfade(16_384.0);
        let plan = looped(source.len(), 1, points).unwrap();
        let looped = plan.looped.unwrap();

        assert!(looped.crossfade > fields_of(points.end - points.start).unwrap());
        assert!(looped.crossfade <= fields_of(points.start).unwrap());
        let file = instrument(&source, &Options::new("Long fade").loops(points)).unwrap();
        let (at, stroke) = file.stroke_streams()[0];
        assert!(codec::decode(stroke, at, codec::Layout::V2).is_ok());
    }

    #[test]
    fn a_loop_the_format_cannot_state_is_refused() {
        let frames = 44_100;
        let stated = |points| looped(frames, 1, points);
        assert!(stated(Loop::new(8_192, 40_000)).is_ok());
        assert!(stated(Loop::new(8_192, 8_192)).is_err(), "empty loop");
        assert!(stated(Loop::new(40_000, 8_192)).is_err(), "loop runs back");
        assert!(stated(Loop::new(8_192, 44_101)).is_err(), "past the audio");
        assert!(
            stated(Loop::new(8_192, 8_250)).is_err(),
            "shorter than a run"
        );
        assert!(
            stated(Loop::new(1_024, 40_000).crossfade(4_096.0)).is_err(),
            "nothing in front of the loop to fade from"
        );
        assert!(
            stated(Loop::new(8_192, 40_000).crossfade(40_000.0)).is_err(),
            "not enough material before the fade"
        );
        // Below the modelled opening, whatever the loop says.
        assert!(looped(4_000, 1, Loop::new(100, 3_000)).is_err());
    }

    #[test]
    fn a_looped_stroke_round_trips_through_the_decoder_exactly() {
        let source = sine(180.0, 16_000.0, 60_000);
        for predictor in [Predictor::Plain, Predictor::Minimising] {
            for points in [
                Loop::new(8_192, 40_960),
                Loop::new(8_192, 40_960).crossfade(4_096.0),
            ] {
                let file = instrument(
                    &source,
                    &Options::new("Looped").predictor(predictor).loops(points),
                )
                .unwrap();
                let (at, stroke) = file.stroke_streams()[0];
                let plan = looped(source.len(), 1, points).unwrap();
                let q = quantise(&source, &plan, None);
                let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
                assert_eq!(audio.samples.len(), plan.fields);
                let gain = 1i32 << q.shift;
                for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
                    assert_eq!(i32::from(got), want * gain, "{predictor:?} field {f}");
                }
            }
        }
    }

    // Full-scale broadband material can exhaust the three spare bits per field before
    // a short loop reaches the next packet boundary.
    #[test]
    fn a_loop_lands_on_a_packet_boundary_or_is_refused() {
        let mut source = Vec::with_capacity(60_000);
        let mut state = 12_345u64;
        for k in 0..60_000u64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let noise = ((state >> 40) as i32 - 8_192) / 4;
            let tone = (20_000.0 * (k as f64 * 0.031).sin()) as i32;
            source.push((tone + noise).clamp(-32_768, 32_767) as i16);
        }

        let mut placed = 0usize;
        let mut refused = 0usize;
        for start in (4_096..48_000).step_by(7_919) {
            for length in [900, 1_500, 4_096, 11_000] {
                for predictor in [Predictor::Plain, Predictor::Minimising] {
                    let points =
                        Loop::new(start, start + length).crossfade((length / 4).min(start) as f64);
                    let options = Options::new("Sweep").predictor(predictor).loops(points);
                    let Ok(file) = instrument(&source, &options) else {
                        refused += 1;
                        continue;
                    };
                    let (at, stroke) = file.stroke_streams()[0];
                    let walk = codec::walk(stroke, at, codec::Layout::V2).unwrap();
                    let mark = walk.records.iter().find(|r| r.mark).unwrap();
                    assert_eq!(
                        (walk.terminator - mark.at) % PACKET_WORDS,
                        0,
                        "loop {start}..{} under {predictor:?} covers {} words",
                        start + length,
                        walk.terminator - mark.at
                    );
                    placed += 1;
                }
            }
        }
        assert!(placed > 40, "{placed} placed, {refused} refused");
    }

    fn stereo(hz: f64, ratio: f64, amplitude: f64, frames: usize) -> Vec<i16> {
        let left = sine(hz, amplitude, frames);
        let right = sine(hz * ratio, amplitude * 0.6, frames);
        left.iter()
            .zip(&right)
            .flat_map(|(&l, &r)| [l, r])
            .collect()
    }

    #[test]
    fn a_stereo_plan_is_the_mono_plan_doubled() {
        for frames in [4096, 4409, 8192, 10_000, 44_100, 100_000, 441_000] {
            let mono = plan(frames, 1).unwrap();
            let both = plan(frames, 2).unwrap();
            assert_eq!(both.fields, 2 * mono.fields, "{frames} frames: T");
            assert_eq!(both.resync_at, 2 * mono.resync_at, "{frames} frames: R1");
            assert_eq!(both.warmup, 2 * mono.warmup, "{frames} frames: W");
            assert_eq!(both.resync, 2 * mono.resync, "{frames} frames: R");
            assert_eq!(both.cells_before, mono.cells_before, "{frames} frames");
            assert_eq!(both.cells_after, mono.cells_after, "{frames} frames");
            assert_eq!(
                both.warmup
                    + both.cell() * both.cells_before
                    + both.resync
                    + both.cell() * both.cells_after,
                both.fields,
                "{frames} frames: the plan does not tile the lattice"
            );
        }
    }

    #[test]
    fn a_stereo_stroke_round_trips_through_the_decoder_exactly() {
        for predictor in [Predictor::Plain, Predictor::Minimising] {
            let source = stereo(220.0, 1.5, 14_000.0, 30_000);
            let file = instrument(
                &source,
                &Options::new("Stereo").channels(2).predictor(predictor),
            )
            .unwrap();
            let (at, stroke) = file.stroke_streams()[0];

            let stream = codec::walk(stroke, at, codec::Layout::V2).unwrap();
            assert_eq!(stream.channels, 2, "{predictor:?}");
            assert_eq!(stream.cell, Some(2 * CELL), "{predictor:?}");
            assert_eq!(&stroke[stroke.len() - 3..], &[0x80, 0, 48]);

            let plan = plan(30_000, 2).unwrap();
            let q = quantise(&source, &plan, None);
            let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
            assert_eq!(audio.channels, 2);
            assert_eq!(audio.samples.len(), plan.fields);
            let gain = 1i32 << q.shift;
            for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
                assert_eq!(i32::from(got), want * gain, "{predictor:?} field {f}");
            }
        }
    }

    #[test]
    fn each_channel_predicts_against_its_own_history() {
        let frames = 20_000;
        let source: Vec<i16> = (0..frames)
            .flat_map(|k| {
                let up = (k as i32 % 2048) - 1024;
                [up as i16, -(up as i16)]
            })
            .collect();
        let file = instrument(
            &source,
            &Options::new("Ramps")
                .channels(2)
                .predictor(Predictor::Minimising),
        )
        .unwrap();
        let (at, stroke) = file.stroke_streams()[0];
        let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
        assert!(audio.differenced > 0, "nothing chose a predictor");

        let plan = plan(frames, 2).unwrap();
        let q = quantise(&source, &plan, None);
        let gain = 1i32 << q.shift;
        for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
            assert_eq!(i32::from(got), want * gain, "field {f}");
        }
    }

    #[test]
    fn the_channels_are_resampled_apart() {
        let frames = 12_000;
        let source: Vec<i16> = sine(300.0, 20_000.0, frames)
            .into_iter()
            .flat_map(|l| [l, 0])
            .collect();
        let file = instrument(&source, &Options::new("Panned").channels(2)).unwrap();
        let (at, stroke) = file.stroke_streams()[0];
        let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
        assert!(audio.samples.iter().step_by(2).any(|&v| v.abs() > 10_000));
        assert!(audio.samples[1..].iter().step_by(2).all(|&v| v == 0));
    }

    #[test]
    fn a_stereo_stroke_loops_the_way_a_mono_one_does() {
        let source = stereo(180.0, 1.25, 16_000.0, 60_000);
        let points = Loop::new(8_192, 40_960).crossfade(2_048.0);
        let file = instrument(&source, &Options::new("Looped").channels(2).loops(points)).unwrap();
        let (at, stroke) = file.stroke_streams()[0];
        let walk = codec::walk(stroke, at, codec::Layout::V2).unwrap();
        assert_eq!(walk.channels, 2);
        let mark = walk.records.iter().find(|r| r.mark).unwrap();
        assert_eq!((walk.terminator - mark.at) % PACKET_WORDS, 0);
        let frames = (walk.fields - mark.first_field) as f64 / 2.0 * f64::from(codec::SOURCE_RATE)
            / f64::from(codec::FIELD_RATE);
        assert!(
            (frames - 32_768.0).abs() < 1.0,
            "loop came back {frames} frames"
        );

        let plan = looped(60_000, 2, points).unwrap();
        let q = quantise(&source, &plan, None);
        let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
        let gain = 1i32 << q.shift;
        for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
            assert_eq!(i32::from(got), want * gain, "field {f}");
        }
    }

    #[test]
    fn a_channel_count_the_terminator_cannot_state_is_refused() {
        let source = vec![0i16; 3 * MIN_FRAMES];
        assert!(plan(MIN_FRAMES, 0).is_err());
        assert!(plan(MIN_FRAMES, 3).is_err());
        assert!(instrument(&source, &Options::new("x").channels(3)).is_err());
        assert!(instrument(
            &vec![0i16; 2 * MIN_FRAMES + 1],
            &Options::new("x").channels(2)
        )
        .is_err());
        assert!(instrument(&vec![0i16; 2 * MIN_FRAMES], &Options::new("x").channels(2)).is_ok());
        let short = vec![0i16; MIN_FRAMES];
        assert!(instrument(&short, &Options::new("x")).is_ok());
        assert!(instrument(&short, &Options::new("x").channels(2)).is_err());
    }

    /// Every generation, mono and stereo, through this crate's own decoder. v4 stereo
    /// is the one that packs each channel's half into its own words, so it is the one
    /// this would catch.
    #[test]
    fn every_generation_round_trips_through_the_decoder_exactly() {
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            for channels in [1u16, 2] {
                let frames = 30_000;
                let source: Vec<i16> = match channels {
                    1 => sine(220.0, 14_000.0, frames),
                    _ => stereo(220.0, 1.5, 14_000.0, frames),
                };
                let file = instrument(
                    &source,
                    &Options::new("Round trip")
                        .layout(layout)
                        .channels(channels)
                        .predictor(Predictor::Minimising),
                )
                .unwrap();
                let (at, stroke) = file.stroke_streams()[0];
                let plan = Plan::new(
                    layout,
                    frames,
                    usize::from(channels),
                    default_secondary_start(frames, None),
                )
                .unwrap();
                let q = quantise(&source, &plan, None);
                let audio = codec::decode(stroke, at, layout)
                    .unwrap_or_else(|e| panic!("{layout:?} {channels}ch: {e}"));
                assert_eq!(audio.channels, channels, "{layout:?} {channels}ch");
                assert_eq!(audio.samples.len(), plan.fields, "{layout:?} {channels}ch");
                let gain = 1i32 << q.shift;
                for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
                    assert_eq!(
                        i32::from(got),
                        want * gain,
                        "{layout:?} {channels}ch field {f}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_wide_instrument_reads_back_as_one() {
        for (layout, version) in [(Layout::V3, 300u32), (Layout::V4, 400)] {
            let file = instrument(
                &sine(220.0, 15_000.0, 30_000),
                &Options::new("Encoded")
                    .layout(layout)
                    .root_key(48)
                    .top_note(72),
            )
            .unwrap();
            let bytes = file.to_bytes().unwrap();
            let read = crate::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
            let crate::Entity::Sample(read) = read else {
                panic!("{layout:?} did not read back as a sample");
            };
            assert_eq!(read.name().unwrap(), "Encoded", "{layout:?}");
            assert_eq!(read.layout(), layout, "{layout:?}");
            assert_eq!(read.to_bytes().unwrap(), bytes, "{layout:?}");
            let crate::Sample::V3(read) = &read else {
                panic!("{layout:?} did not read back on the wide chain");
            };
            assert_eq!(read.header.version, version);
            let zones = read.zones().unwrap();
            assert_eq!(zones.len(), 1);
            assert_eq!(zones[0].root_key, 48);
            assert_eq!(zones[0].top_note, 72);
            assert_eq!(zones[0].low_note, Some(super::super::zone::KEY_FLOOR));
            assert_eq!(
                read.meta().unwrap().chain_len as usize,
                read.chain_len_before_meta()
            );
        }
    }

    /// Zones tile: each reaches down to one above the one below it, and the lowest to
    /// the keyboard's floor. ⚠️ The v3 `map` stores its records low to high and the v4
    /// one high to low, which is why the stored order is asserted per generation.
    #[test]
    fn a_wide_zone_states_its_own_bottom() {
        let high = sine(880.0, 12_000.0, 12_000);
        let low = sine(220.0, 12_000.0, 15_000);
        let floor = super::super::zone::KEY_FLOOR;
        for (layout, stored) in [
            (Layout::V3, [(65, floor), (96, 66)]),
            (Layout::V4, [(96, 66), (65, floor)]),
        ] {
            let file = multi_zone(
                &[zone(&high, 72, 96, 2), zone(&low, 48, 65, 1)],
                "Two",
                Predictor::Plain,
                layout,
            )
            .unwrap();
            let zones = file.zones().unwrap();
            assert_eq!(
                zones
                    .iter()
                    .map(|z| (z.top_note, z.low_note.unwrap()))
                    .collect::<Vec<_>>(),
                stored,
                "{layout:?}"
            );
        }
    }

    #[test]
    fn a_wide_zone_refuses_a_gain_it_has_nowhere_to_put() {
        let source = sine(440.0, 12_000.0, 20_000);
        let quiet = NewZone {
            gain: GAIN_UNITY / 2,
            ..zone(&source, 60, 127, 1)
        };
        for layout in [Layout::V3, Layout::V4] {
            assert!(
                multi_zone(&[quiet], "Gain", Predictor::Plain, layout).is_err(),
                "{layout:?}"
            );
            assert!(multi_zone(
                &[zone(&source, 60, 127, 1)],
                "Gain",
                Predictor::Plain,
                layout
            )
            .is_ok());
        }
    }

    /// The wide terminator states 32 fields per channel where the narrow one states 24,
    /// and a 1:1 run reaches 48 fields per channel rather than 32.
    #[test]
    fn the_wide_plan_tiles_the_lattice_in_its_own_units() {
        for frames in [4096, 10_000, 44_100, 100_000] {
            for layout in [Layout::V3, Layout::V4] {
                let p =
                    Plan::new(layout, frames, 1, default_secondary_start(frames, None)).unwrap();
                assert_eq!(p.cell(), 32, "{layout:?} {frames} frames");
                assert_eq!(
                    p.warmup + p.cell() * p.cells_before + p.resync + p.cell() * p.cells_after,
                    p.fields,
                    "{layout:?} {frames} frames"
                );
                for run in chunks(p.warmup, p.chunk())
                    .into_iter()
                    .chain(chunks(p.resync, p.chunk()))
                {
                    assert!(
                        (32..=48).contains(&run),
                        "{layout:?} {frames} frames: {run}"
                    );
                }
            }
        }
    }

    /// A silent wide stroke stores statistic B as a signed extreme, so it reads back
    /// through the codec's own sign rule rather than as a 24-bit magnitude.
    #[test]
    fn a_wide_statistic_b_carries_the_extremes_sign() {
        let frames = 20_000;
        let mut down = vec![0i16; frames];
        down[10_000] = -13;
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let file = instrument(&down, &Options::new("Peak").layout(layout)).unwrap();
            let (_, stroke) = file.stroke_streams()[0];
            let want = if layout.signed_peak() { -3 } else { 3 };
            assert_eq!(codec::peak(stroke, layout), Some(want), "{layout:?}");
        }
    }

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
