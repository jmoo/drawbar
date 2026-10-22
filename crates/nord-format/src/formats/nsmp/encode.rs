//! Building a sample instrument from PCM.
//!
//! The inverse of [`codec`]. This emits what Nord Sample Editor writes
//! for the same input, byte for byte, with one exception: the resampling
//! [`kernel`] matches the instrument's to within a few `1e-8` per tap,
//! and a handful of taps the editor evaluates a ulp off the closed form leave the
//! occasional field one count from the editor's. No structural field, pitch or length
//! differs because of it. The one deliberate departure is a [`Loop`] that ends at the
//! end of its audio, below.
//!
//! [`Predictor::Minimizing`], the editor's record coding, is the default.
//! [`Predictor::Plain`] stores every content field outright: the same audio in a file
//! several times larger on smooth material, and not the editor's bytes.
//!
//! Under either predictor, a file from here decodes exactly through this crate's
//! decoder and obeys every known structural law of the format.
//!
//! For [`Layout::V2`], the Electro 5 loads and plays one under either predictor, at
//! the pitch the decoder renders. Confirmed on hardware. The wide generations
//! reproduce the editor's renders, but the Electro 5 plays only v2. Their playback:
//! Inferred from specimens; not confirmed on hardware.
//!
//! ```no_run
//! # use nord_format::formats::nsmp::encode;
//! let samples: Vec<i16> = vec![0; 44_100];
//! let options = encode::Options::new("Test").root_key(60);
//! let instrument = encode::instrument(&samples, &options).unwrap();
//! std::fs::write("test.nsmp", instrument.to_bytes().unwrap()).unwrap();
//! ```
//!
//! All three generations are written from one plan. [`Options::layout`] picks the
//! generation, which decides the container (the narrow `NWS` chain or the wide `NSMP`
//! one, and the section schemas inside) and the stream's units.
//! The lattice, the kernel, the quantizer, the count laws and the record grammar's bit
//! layout are shared by all three.
//!
//! [`multi_zone`] is the same builder across a keyboard: one `stk` per zone, highest
//! zone first, each zone's record naming its stroke by the global id the caller gives
//! it. Zone counts move where a stroke's audio may start, so each stroke's allocation
//! comes from [`stroke::header_len`](super::stroke::header_len).
//!
//! Stereo is the mono plan run once per channel and interleaved. A stereo stroke
//! carries both channels under one header at the doubled cell, and every count-law
//! landmark (the field total, the resync position, both 1:1 runs) is its mono value
//! doubled. On the plan side, stereo is only a channel count: cells and 1:1 records
//! double, the terminator states the doubled cell, and the predictor keeps a history
//! per channel. Where the two channels' bits go depends on the generation: v2 and v3
//! alternate fields in one bitstream, and v4 packs each channel's half into its own
//! words and alternates those.
//!
//! For [`Layout::V2`], a stereo encode plays with its channels in order and
//! independent. Confirmed on hardware. The wide generations: Inferred from specimens;
//! not confirmed on hardware.
//!
//! A [`Loop`] truncates the stroke at its end and opens a marked record at its start.
//! That is all the container stores about looping: the crossfade is baked into the
//! audio here, and loop detune, the decay switch and the short loop's pitch-tracking
//! flag are not stored. Wide headers carry the decay amount. The caller works out the
//! fade's frame count (a project states the long loop's in frames and the short
//! loop's as a percentage of its length) and passes it in frames, fraction included.
//!
//! The resampler reads a few frames either side of each field, so the fields before
//! the loop end see the audio after it. Where there is none, it reads the loop's own
//! opening, which is what playback plays there. The editor reads silence instead,
//! so on bright material its loop clicks once per pass. Inferred from specimens; not
//! confirmed on hardware.
//!
//! For [`Layout::V2`], the Electro 5 sustains a looped encode to note-off, and the
//! seam is clean. Confirmed on hardware. The wide generations: Inferred from
//! specimens; not confirmed on hardware.
//!
//! [`from_lattice`] enters the same plan and record coding from the other side: it
//! takes a stream's stored fields and landmarks, a [`codec::Lattice`], and lays them
//! out in any generation with no resampler and no second opening ramp. Where the
//! target's shift rule is coarser than the shift the fields were stored at, the result
//! is the stream the editor renders in the target generation. Inferred from
//! specimens; not confirmed on hardware.

use super::cat::NarrowCat;
use super::codec::{self, Head, Layout, PITCH_DEN, PITCH_NUM, WRAP};
use super::kernel;
use super::keymap::{KeyTable, Level};
use super::section::{self, Framed, Framing, Section, Section4};
use super::stroke::packet_len;
use super::zone::VelocityWindow;
use super::{Sample, SampleV3};
use crate::cbin::{Cbin, Generation, Header};
use crate::error::{Error, ParseError};
use crate::formats::nsmpproj;
use crate::formats::predictor::DIFFERENCE;
use std::borrow::Cow;
use std::ops::RangeInclusive;
use thiserror::Error as ThisError;

/// Audio too short or too long for one stroke.
#[derive(ThisError, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum LengthError {
    #[error("too short: {frames} frames, and the encoder needs at least {MIN_FRAMES}")]
    TooShort { frames: usize },
    /// Longer than a stroke holds even at the narrowest field width.
    #[error(
        "too long: {frames} frames, and a {} stroke holds at most {max} (about {:.1} s at \
         {} kHz)",
        if *.channels == 1 { "mono" } else { "stereo" },
        seconds(*.max),
        f64::from(codec::SOURCE_RATE) / 1000.0
    )]
    TooLong {
        frames: usize,
        channels: usize,
        max: usize,
    },
    /// Within that ceiling, but the encoded audio still overflows the stroke.
    #[error(
        "too long: the audio encodes to {words} words, more than the {MAX_STREAM_WORDS} a \
         stroke holds; about {fits:.1} s of it would fit"
    )]
    Stream { words: usize, fits: f64 },
}

/// Seconds of source audio in `frames`.
fn seconds(frames: usize) -> f64 {
    frames as f64 / f64::from(codec::SOURCE_RATE)
}

/// Content version this writes per generation: `format × 100 + revision`, at the
/// revision the editor emits.
pub(crate) const fn version(layout: Layout) -> u32 {
    match layout {
        Layout::V2 => 200,
        Layout::V3 => 300,
        Layout::V4 => 400,
    }
}

/// The container header's `aux` word as the editor writes it: the category in bits
/// 16..24 and the sub category in the low byte, in every generation. Early-chain
/// libraries, which have no `cat`, and files read back over USB hold all ones there
/// instead. Inferred from specimens; not confirmed on hardware.
fn aux(categories: &NarrowCat) -> u32 {
    u32::from(categories.category) << 16 | u32::from(categories.sub_category)
}

/// Largest field count a record header can state, from its 14-bit count field.
/// ⚠️ The count is in fields, and a stereo cell holds two channels' worth, so a stereo
/// record holds half as many cells.
const MAX_COUNT: usize = (1 << 14) - 1;

/// Widest field a stroke's peak may take: quantization shifts until it fits. On a
/// stereo stroke this is the entire shift rule.
const PEAK_WIDTH: u8 = 14;

/// The stream units one stroke is written in: the generation's word and cell sizes,
/// scaled by how many channels share the stroke.
///
/// The lattice, the kernel, the quantizer and the record grammar's bit layout are the
/// same in every generation, so this is all a generation changes about a stream.
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
    /// words. Otherwise they alternate fields in one bitstream.
    const fn splits(self) -> bool {
        self.channels == 2 && self.layout.splits_wide_openings()
    }

    /// Words one record occupies, header included.
    ///
    /// A split record pays for each channel's own padding; a content record tiles
    /// whole words either way, so only the 1:1 regime is ever wider for it.
    const fn span(self, count: usize, width: u8) -> usize {
        self.layout.record_span(count, width, self.splits())
    }

    /// Words in one packet of allocation.
    const fn packet_words(self) -> usize {
        packet_len(self.layout) / self.word()
    }

    /// Words of slack the allocation keeps ahead of the chain's first record.
    ///
    /// The chain is right-aligned in whole packets in both chains. The wide chain adds
    /// a packet when the lead would fall below this, so its strokes carry 7 to 38
    /// words of slack where a narrow one carries 0 to 126.
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

    /// The most source frames whose fields, ring-out included, stay within
    /// [`Units::max_fields`]. [`fields_of`] rounds half up, so `frames` fit while
    /// `frames·DEN/NUM < per_channel + ½`.
    const fn max_frames(self) -> usize {
        let per_channel = self.max_fields() / self.channels - RING_OUT;
        ((2 * per_channel + 1) * PITCH_NUM as usize - 1) / (2 * PITCH_DEN as usize)
    }
}

/// Last-record field counts, per channel, that never carry the extra quantizer bit;
/// `None` for a generation whose mono strokes never spend it.
///
/// A run's records are RMAX-sized except the remainder, so a last record takes 24..=32
/// fields at v2 and 32..=48 at v3. Every count in both ranges was read from a render,
/// and these are the ones that never spend the bit. Neither set follows from any
/// arithmetic, and the two sets do not correspond.
///
/// Inferred from specimens; not confirmed on hardware.
const fn dead_last_record(layout: Layout) -> Option<&'static [usize]> {
    match layout {
        Layout::V2 => Some(&[24, 29, 32]),
        Layout::V3 => Some(&[32, 41, 43, 45, 47, 48]),
        Layout::V4 => None,
    }
}

/// Whether a stroke spends one more quantizer bit than its peak needs, narrowing its
/// widest field a bit under [`PEAK_WIDTH`] and shrinking the stream.
///
/// `values` are the stroke's fields before any shift. Read them at the smallest shift
/// that fits the peak in [`PEAK_WIDTH`] bits. The bit is spent when a field still
/// outside the signed 13-bit range there falls inside the last record of one of the
/// stroke's 1:1 runs, and that record's field count is not one [`dead_last_record`]
/// names. A field in an earlier record of a run, or in the content cells, never
/// spends it, and no run's length is otherwise consulted.
///
/// ⚠️ Every 1:1 run counts, the loop's included. A marked record opens a run of its
/// own past the resync, and a field in its last record spends the bit just as one in
/// the opening or resync run does.
///
/// A stereo stroke never spends the bit, in any generation, and neither does a v4 mono
/// one: both quantize at the peak term alone.
///
/// Inferred from specimens; not confirmed on hardware. The Electro 5 plays v2 only.
fn spends_extra_bit(values: &[i64], plan: &Plan) -> bool {
    if plan.channels != 1 {
        return false;
    }
    let Some(dead) = dead_last_record(plan.layout) else {
        return false;
    };
    let over = 1i64 << (PEAK_WIDTH - 2);
    let shift = peak_shift(values, PEAK_WIDTH);
    [
        Some((0, plan.warmup)),
        Some((plan.resync_at, plan.resync)),
        plan.looped.map(|points| (points.at, points.warmup)),
    ]
    .into_iter()
    .flatten()
    .any(|(base, run)| {
        let Some(&last) = chunks(run, plan.chunk()).last() else {
            return false;
        };
        !dead.contains(&(last / plan.channels))
            && values[base + run - last..base + run].iter().any(|&v| {
                let v = v >> shift;
                v < -over || v >= over
            })
    })
}

/// The smallest nonnegative shift fitting every value in `width` bits.
fn peak_shift(values: &[i64], width: u8) -> i32 {
    let (low, high) = extent(values);
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

/// Channels one stroke may carry. The terminator states the cell size, which can only
/// be single or doubled.
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

/// Shortest input the editor encodes: it extends a shorter project's extent to this
/// length. The opening, the count laws and the resync hold unchanged down to it.
pub const MIN_FRAMES: usize = 92;

/// Fields per channel a looped stroke carries past its loop end, repeating the loop's
/// own opening so that playback is unchanged. The mark sits the same amount past the
/// loop start, so the loop's length is preserved.
pub(crate) const LOOP_LEAD: usize = 5;

/// Minimum fields per channel between the resync point and a loop's marked record. A
/// loop whose usual [`LOOP_LEAD`] would put the mark nearer is pushed back by
/// repeating more of itself, which lengthens the whole stream.
///
/// The minimum applies to the gap from the resync point only, not to the mark's own
/// position or to the room between the mark and the run in front of it: a resync run
/// may reach the mark record with nothing between them.
///
/// Inferred from specimens; not confirmed on hardware.
pub(crate) const fn min_resync_gap(layout: Layout) -> usize {
    match layout {
        Layout::V2 => 72,
        Layout::V3 | Layout::V4 => 64,
    }
}

/// Most stream words the stroke header's 16-bit word directory can address
/// unambiguously.
const MAX_STREAM_WORDS: usize = WRAP;

/// How content records code their fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Predictor {
    /// Store every content field outright at order zero.
    Plain,
    /// Choose the narrowest predictor per cell, the lowest order among equals, as the
    /// editor does. Smaller than plain records, and exact through this crate's
    /// decoder.
    #[default]
    Minimizing,
}

/// A sustain loop, in source frames.
///
/// The container stores two things about a loop: the stroke stops at
/// [`end`](Loop::end), and the record the loop starts at carries the mark bit. Loop
/// detune, the decay switch, and whether the editor called this a short or a long loop
/// are not stored. The wide header's decay amount is set per zone, by
/// [`NewZone::loop_decay`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loop {
    /// First frame of the loop.
    pub start: usize,
    /// One past its last frame. Audio after it is not stored, though the resampler
    /// reads the first few frames of it.
    pub end: usize,
    /// Frames of the loop's tail that fade into the frames before [`start`](Loop::start).
    /// The fade is baked into the samples, because the instrument reads it from there.
    /// It is fractional because a project can state it as a percentage of the loop,
    /// and dropping the fraction moves the fade a field.
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
    /// Defaults: the name given, root key C4, the editor's own top note, the editor's
    /// record coding, no loop, the v2 generation.
    pub fn new(name: impl Into<String>) -> Options {
        Options {
            name: name.into(),
            root_key: 60,
            top_note: None,
            predictor: Predictor::default(),
            loops: None,
            channels: 1,
            secondary_start: None,
            shift: None,
            layout: Layout::V2,
        }
    }

    /// Which generation to write: `.nsmp`, `.nsmp3` or `.nsmp4`. The audio is the same
    /// in all three; the container and the stream's units differ.
    pub fn layout(mut self, layout: Layout) -> Options {
        self.layout = layout;
        self
    }

    /// Resynchronize the stream at `frames` source frames from the first: a project's
    /// `m_startSecondary`, measured from its `m_start`. When unset, the stream
    /// resynchronizes where a new project would put it, at [`default_secondary_start`].
    pub fn secondary_start(mut self, frames: f64) -> Options {
        self.secondary_start = Some(frames);
        self
    }

    /// How many channels the PCM interleaves: 1 or 2. Anything else is refused when
    /// the instrument is built.
    pub fn channels(mut self, channels: u16) -> Options {
        self.channels = channels;
        self
    }

    /// Quantize at `bits` of shift, overriding the shift rule. Experimental: it lays
    /// the same stroke out at neighboring shifts, and the editor has no such setting.
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
    /// Fields repeated past the loop end, which is also how far `at` sits past the
    /// loop start: a fixed lead per channel, or more when the mark is pushed off the
    /// resync point as [`Plan::looped`] describes.
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
/// Every field count here is a stream count: on a stereo stroke the two channels
/// interleave, so each is twice the per-channel number the mono laws state.
/// [`Plan::cell`] and the 1:1 chunk scale with it, and that is all stereo changes about
/// the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Which generation's units the stream is written in.
    pub layout: Layout,
    /// Channels interleaved into the stream: 1 or 2.
    pub channels: usize,
    /// Fields in the stream: the source plus a ring-out past its end, or, when the
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

    /// Fields one content cell covers: the generation's cell per channel.
    pub const fn cell(&self) -> usize {
        self.units().cell()
    }

    /// Fields one 1:1 record covers at most: the generation's RMAX per channel.
    const fn chunk(&self) -> usize {
        self.units().chunk()
    }

    /// Whether field `f` falls in a content record rather than one of the 1:1 runs.
    fn is_content(&self, f: usize) -> bool {
        let opening = self.looped.map(|l| l.at..l.at + l.warmup);
        ((f >= self.warmup && f < self.resync_at) || f >= self.resync_at + self.resync)
            && !opening.is_some_and(|run| run.contains(&f))
    }
}

/// Source frames onto the field lattice.
fn fields_of(frames: usize) -> Option<usize> {
    let frames = u64::try_from(frames).ok()?;
    frames
        .checked_mul(u64::from(PITCH_DEN))
        .and_then(|n| round_ratio(n, u64::from(PITCH_NUM)))
}

/// The same lattice, for a landmark that falls between two frames, such as a fade a
/// project states as a percentage of its loop. Rounding such a value to a whole frame
/// first opens the ramp a field early.
fn fields_at(frames: f64) -> Option<usize> {
    let fields = frames * f64::from(PITCH_DEN) / f64::from(PITCH_NUM);
    (fields.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&fields))
        .then_some(fields.round() as usize)
}

impl Plan {
    /// The layout for `frames` source frames of `channels`-channel audio, no loop,
    /// resynchronizing at `secondary_start` source frames from the first: the
    /// project's `m_startSecondary` measured from its `m_start`, or
    /// [`default_secondary_start`] for audio no project describes.
    ///
    /// Refuses a secondary start the stream cannot resynchronize at: off the lattice,
    /// or too close to either end for the 1:1 runs around it.
    pub fn new(
        layout: Layout,
        frames: usize,
        channels: usize,
        secondary_start: f64,
    ) -> Result<Plan, Error> {
        Plan::modeled(frames, channels)?;
        let fields = fields_of(frames)
            .and_then(|f| f.checked_add(RING_OUT))
            .and_then(|f| f.checked_mul(channels))
            .ok_or_else(|| size_error(frames, Units { layout, channels }))?;
        let resync_at = Plan::resync_at(secondary_start, channels)?;
        Plan::lay_out(layout, channels, fields, None, resync_at, || {
            size_error(frames, Units { layout, channels })
        })
    }

    /// The layout for a stroke that loops: `frames` source samples truncated at
    /// [`Loop::end`], with the loop's own opening repeated past it, resynchronizing at
    /// `secondary_start` as [`new`](Plan::new) does.
    ///
    /// The marked record sits a fixed lead per channel past the loop start, or a
    /// generation's minimum gap past the resync point when that is later: a loop
    /// starting near the resync is pushed back, and the stream grows by the same amount.
    ///
    /// Refuses a loop the format cannot state (one outside the audio, one shorter than
    /// the run it must open with, or a crossfade with no material in front of the loop
    /// to fade from) and a secondary start past the loop start. A project's own
    /// secondary start is repaired so it never lies past the loop start.
    pub fn looped(
        layout: Layout,
        frames: usize,
        channels: usize,
        points: Loop,
        secondary_start: f64,
    ) -> Result<Plan, Error> {
        Plan::modeled(points.end, channels)?;
        if points.start >= points.end || points.end > frames {
            return Err(ParseError::OutOfBounds {
                value: format!("a loop over frames {}..{}", points.start, points.end),
                bound: format!("a non-empty region of the {frames} frames given"),
            }
            .into());
        }
        // Every landmark is placed per channel and scaled by the channel count, as the
        // encoder runs one plan interleaved.
        let units = Units { layout, channels };
        let size_error = |frames: usize| size_error(frames, units);
        let lattice = |n: usize| fields_of(n).and_then(|f| f.checked_mul(channels));
        let lattice_at = |n: f64| fields_at(n).and_then(|f| f.checked_mul(channels));
        let start = lattice(points.start).ok_or_else(|| size_error(points.start))?;
        // The loop's length must be preserved, so it goes onto the lattice as a
        // length. Rounding its two ends separately can cost a field.
        let span = points.end - points.start;
        let length = lattice(span).ok_or_else(|| size_error(points.end))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| size_error(points.end))?;
        let (cell, chunk) = (units.cell(), units.chunk());
        let resync_at = Plan::resync_at(secondary_start, channels)?;
        if resync_at > start {
            return Err(ParseError::OutOfBounds {
                value: format!("a secondary start at field {resync_at}"),
                bound: format!(
                    "field {start}, where the loop starts, or earlier; the marked \
                     record follows the resync point, so the loop cannot open ahead of it"
                ),
            }
            .into());
        }
        // The mark keeps the generation's minimum gap from the resync point, so a loop
        // that starts too near it is pushed back by repeating more of itself.
        let at = start
            .checked_add(LOOP_LEAD * channels)
            .zip(resync_at.checked_add(min_resync_gap(layout) * channels))
            .map(|(ideal, floor)| ideal.max(floor))
            .ok_or_else(|| size_error(points.start))?;
        let lead = at - start;
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
                    "the {} frames before the loop starts, since the fade compares \
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
                    "the {} frames before the loop starts, since the field lattice \
                     leaves no earlier material to compare",
                    points.start,
                ),
            }
            .into());
        }
        Plan::lay_out(
            layout,
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
            || size_error(frames),
        )
    }

    /// The layout of a stream whose fields are already on the lattice: `fields` stream
    /// fields of `channels`-channel audio, resynchronizing at field `resync_at`, and
    /// looping from field `mark` to the end when there is one.
    ///
    /// The landmarks are taken as given, so a loop's lead and crossfade are already in
    /// the fields and read zero here.
    ///
    /// Refuses landmarks this generation cannot lay out: a mark nearer the resync point
    /// than the generation's minimum gap, a loop too short for the run it opens with,
    /// and a resync point too near either end.
    pub fn on_lattice(
        layout: Layout,
        fields: usize,
        channels: usize,
        resync_at: usize,
        mark: Option<usize>,
    ) -> Result<Plan, Error> {
        Plan::channels(channels)?;
        let too_long = || ParseError::OutOfBounds {
            value: format!("a stream of {fields} fields"),
            bound: format!("a stream that fits {MAX_STREAM_WORDS} words"),
        };
        let looped = match mark {
            None => None,
            Some(at) => {
                let units = Units { layout, channels };
                let cell = units.cell();
                let floor = resync_at
                    .checked_add(min_resync_gap(layout) * channels)
                    .ok_or_else(too_long)?;
                if at < floor {
                    return Err(ParseError::OutOfBounds {
                        value: format!("a loop mark at field {at}"),
                        bound: format!(
                            "field {floor} or later, the {} generation's minimum distance \
                             past the resync point at {resync_at}",
                            layout.generation()
                        ),
                    }
                    .into());
                }
                if at >= fields {
                    return Err(ParseError::OutOfBounds {
                        value: format!("a loop mark at field {at}"),
                        bound: format!("a field before the stream ends at {fields}"),
                    }
                    .into());
                }
                let length = fields - at;
                let warmup = loop_warmup(units, length)?;
                Some(Looped {
                    at,
                    lead: 0,
                    crossfade: 0,
                    warmup,
                    cells: (length - warmup) / cell,
                })
            }
        };
        Plan::lay_out(layout, channels, fields, looped, resync_at, too_long)
    }

    /// The secondary start on the lattice: a per-channel position, doubled like every
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

    fn channels(channels: usize) -> Result<(), Error> {
        if (1..=MAX_CHANNELS).contains(&channels) {
            return Ok(());
        }
        Err(ParseError::OutOfBounds {
            value: format!("{channels} channels"),
            bound: format!(
                "1 or {MAX_CHANNELS}, since the terminator states one cell size and can \
                 only say whether it is doubled"
            ),
        }
        .into())
    }

    fn modeled(frames: usize, channels: usize) -> Result<(), Error> {
        Plan::channels(channels)?;
        if frames >= MIN_FRAMES {
            return Ok(());
        }
        Err(ParseError::from(LengthError::TooShort { frames }).into())
    }

    /// Place the warmup, the resync and the cells between them across everything ahead
    /// of the loop, or across the whole stream when there is none.
    fn lay_out(
        layout: Layout,
        channels: usize,
        fields: usize,
        looped: Option<Looped>,
        resync_at: usize,
        too_long: impl FnOnce() -> ParseError,
    ) -> Result<Plan, Error> {
        let units = Units { layout, channels };
        if fields > units.max_fields() {
            return Err(too_long().into());
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

/// The 1:1 run a `length`-field loop opens with, refusing a loop too short for that run
/// and one cell after it.
fn loop_warmup(units: Units, length: usize) -> Result<usize, Error> {
    let (cell, chunk) = (units.cell(), units.chunk());
    let warmup = band(length, cell, chunk);
    if length >= warmup.saturating_add(cell) {
        return Ok(warmup);
    }
    Err(ParseError::OutOfBounds {
        value: format!("a {length}-field loop"),
        bound: format!(
            "a loop long enough for the {warmup}-field 1:1 run it opens with and one \
             {cell}-field cell after it"
        ),
    }
    .into())
}

/// Where a fresh project would put the resync in `frames` untrimmed source frames: the
/// `m_startSecondary` [`nsmpproj::default_secondary_start`] states, repaired around
/// `loops` the way the editor repairs a project it loads.
pub fn default_secondary_start(frames: usize, loops: Option<Loop>) -> f64 {
    let stop = frames as f64;
    nsmpproj::repaired_secondary_start(
        nsmpproj::default_secondary_start(stop),
        stop,
        loops.map(|l| nsmpproj::repaired_loop_start(l.start as f64)),
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

fn size_error(frames: usize, units: Units) -> ParseError {
    LengthError::TooLong {
        frames,
        channels: units.channels,
        max: units.max_frames(),
    }
    .into()
}

/// The 1:1 run length that preserves a landmark's cell phase, at either channel count.
///
/// A run of `j` records covers between `j*cell` and `j*rmax` fields, so the reachable
/// lengths come in windows with gaps between them: 24..=32, 48..=64, 72..=96 for v2
/// mono, and doubled for stereo. `band(r)` is the smallest reachable
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
    // The windows overlap from `j = 3` at 24/32 and from `j = 2` at 32/48, so this
    // settles within a few steps; the bound is a guard, not a limit anything reaches.
    while length <= 64 * cell {
        if (1..=8).any(|j| j * cell <= length && length <= j * rmax) {
            return length;
        }
        length += cell;
    }
    length
}

/// Split a 1:1 run into records of at most `chunk` fields. [`band`] guarantees the
/// remainder is a legal record.
fn chunks(mut n: usize, chunk: usize) -> Vec<usize> {
    let mut out = Vec::new();
    while n > chunk {
        out.push(chunk);
        n -= chunk;
    }
    out.push(n);
    out
}

/// The source on the lattice, quantized: the stream's field values and the two header
/// statistics that describe them.
#[derive(Debug, Clone)]
struct Quantized {
    /// One stored value per field, sign-extended and within the stream's maximum width.
    values: Vec<i32>,
    /// Bits the values were shifted right by. Dequantizing shifts back.
    shift: i32,
    /// Statistic B: the content field of largest magnitude, taken at a fixed shift of 2.
    /// Carries the extreme's sign where the generation stores one; a magnitude at v2.
    peak: i32,
}

/// Largest magnitude statistic B's 24 bits hold once a sign is allowed for. A field
/// is the source's own 16-bit unit taken at a shift of two, so nothing reaches it.
const MAX_PEAK: i64 = (1 << 23) - 1;

/// The opening ramp: the first [`RAMP_IN`] fields of a channel rise as the cube of
/// their position, toward zero like everything else the encoder quantizes.
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
/// The ramp is linear across the crossfade, matching what the editor's crossfade
/// ladder measures.
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
    // however far past its start the mark sits.
    for k in 0..lead {
        raw[end + k] = raw[at - lead + k];
    }
}

/// What the kernel resamples: `source`, continued past its last frame by the loop's
/// own opening when a loop ends within [`kernel::REACH`] of it.
///
/// ⚠️ The editor reads silence there, so this is where an encode parts from its bytes.
fn kernel_source(source: &[i16], channels: usize, loops: Option<Loop>) -> Cow<'_, [i16]> {
    let frames = source.len() / channels;
    let Some(points) = loops.filter(|l| l.end + kernel::REACH > frames) else {
        return Cow::Borrowed(source);
    };
    let length = points.end - points.start;
    let mut continued = source.to_vec();
    for frame in frames..points.end + kernel::REACH {
        let from = points.start + (frame - points.end) % length;
        continued.extend_from_slice(&source[from * channels..(from + 1) * channels]);
    }
    Cow::Owned(continued)
}

/// Resample and choose the smallest nonnegative shift that fits the stroke's peak into
/// [`PEAK_WIDTH`] bits, plus the further bit a mono stroke spends when
/// [`spends_extra_bit`] says so. `forced` lays the stroke out at that shift instead.
///
/// Each channel is resampled on its own lattice and the results interleaved, because
/// that is what the stream carries; the shift and statistic B are one pair for the
/// stroke, taken across both.
fn quantize(source: &[i16], plan: &Plan, forced: Option<u8>) -> Quantized {
    let channels = plan.channels;
    let per = plan.fields / channels;
    let mut raw = vec![0i64; plan.fields];
    // The sums each field truncates from. Statistic B ranks fields on these, so two
    // fields that truncate alike still order.
    let mut sums = vec![0f64; plan.fields];
    let kernel = kernel::Kernel::new(PITCH_NUM, PITCH_DEN);
    let mut lane: Vec<i16> = Vec::with_capacity(source.len().div_ceil(channels));
    for channel in 0..channels {
        lane.clear();
        lane.extend(source.iter().skip(channel).step_by(channels).copied());
        let accumulated: Vec<f64> = (0..per).map(|f| kernel.accumulate(&lane, f)).collect();
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

    // Statistic B is the content field of largest magnitude at a fixed shift of two, so
    // a negative extreme rounds away from zero, and a later field takes the extreme
    // only by exceeding it. Values in the 1:1 regime never set it.
    let extreme =
        (0..plan.fields)
            .filter(|&f| plan.is_content(f))
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

    Quantized {
        values: raw.iter().map(|&v| (v >> shift) as i32).collect(),
        shift,
        peak,
    }
}

/// Fields already on the lattice, at the shift this generation's rule takes for them.
/// The rule reads the fields at their stored shift, so the result is never finer: the
/// bits that shift dropped are gone. Statistic B was taken before any shift, so it
/// carries over, signed by [`negative_extreme`] where a narrow source stored none.
fn requantize(lattice: &codec::Lattice, plan: &Plan) -> Result<Quantized, Error> {
    let values: Vec<i64> = lattice.fields.iter().map(|&v| i64::from(v)).collect();
    let rise = peak_shift(&values, PEAK_WIDTH) + i32::from(spends_extra_bit(&values, plan));
    // A stream may sit at a negative shift, which keeps fractional bits of a source
    // wider than 16 bits; the editor renders 24-bit audio that way.
    let shift = lattice.shift + rise;
    if !(-codec::SHIFT_LIMIT..=codec::SHIFT_LIMIT).contains(&shift) {
        return Err(ParseError::OutOfBounds {
            value: format!("a quantizer shift of {shift} bits"),
            bound: format!(
                "-{0} through {0} bits, what a stream can state",
                codec::SHIFT_LIMIT
            ),
        }
        .into());
    }
    let magnitude = i64::from(lattice.peak.magnitude()).min(MAX_PEAK);
    let peak = match (plan.layout.signed_peak(), lattice.peak) {
        (true, codec::Peak::Signed(peak)) => peak,
        (false, _) => magnitude as i32,
        (true, codec::Peak::Magnitude(_)) => {
            match negative_extreme(&values, lattice.shift, magnitude, plan) {
                true => -magnitude as i32,
                false => magnitude as i32,
            }
        }
    };
    Ok(Quantized {
        values: values.iter().map(|&v| (v >> rise) as i32).collect(),
        shift,
        peak,
    })
}

/// Whether statistic B was negative, given only its magnitude and the content fields
/// at their stored `shift`. B rounds down from a field at a shift of two, so a positive
/// candidate is never nearer zero than a negative one and wins, except in an exact tie
/// at `±4m`, which only a shift of zero or less can see and which reads negative. With
/// no positive candidate, B reads negative, as near-silent wide renders do.
///
/// Inferred from specimens; not confirmed on hardware.
fn negative_extreme(values: &[i64], shift: i32, magnitude: i64, plan: &Plan) -> bool {
    let holds = |peak: i64, value: i64| match shift >= 2 {
        true => value == peak >> (shift - 2),
        false => value >> (2 - shift) == peak,
    };
    let candidates = |peak: i64| {
        (0..plan.fields)
            .filter(move |&f| plan.is_content(f) && holds(peak, values[f]))
            .map(|f| values[f])
    };
    let Some(positive) = candidates(magnitude).max() else {
        return true;
    };
    let edge = match shift <= 0 {
        true => (4 * magnitude) << -shift,
        false => return false,
    };
    positive == edge && candidates(-magnitude).any(|v| v == -edge)
}

/// The least and the greatest of `values`, `(0, 0)` when there are none.
fn extent<T: Copy + Into<i64>>(values: &[T]) -> (i64, i64) {
    let values = values.iter().map(|&v| v.into());
    (values.clone().min().unwrap_or(0), values.max().unwrap_or(0))
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
/// ⚠️ `stride` is the channel count: the predictor runs per channel, so a stereo field
/// differences against the field two slots back, not the other channel's.
fn residual(values: &[i32], at: usize, order: u8, stride: usize) -> i64 {
    DIFFERENCE[usize::from(order)]
        .iter()
        .enumerate()
        .map(|(j, &c)| match at.checked_sub(j * stride) {
            Some(k) => c * i64::from(values[k]),
            None => 0,
        })
        .sum()
}

/// The width one cell needs at `order`. One cell is `stride` channels' worth, and a
/// record declares one width for both.
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

/// The width each predictor order codes one cell at, indexed by order; order 0 alone
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
        Predictor::Minimizing => DIFFERENCE.len(),
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

/// Partition 1:1 values and like-coded content cells into records, with the index of
/// the record the resync run opens at, which the header's second pointer names.
///
/// A loop appends a third regime (its own marked 1:1 run and the content after it),
/// padded to a whole number of packets by [`pad_to_packet`].
fn records(values: &[i32], plan: &Plan, predictor: Predictor) -> Result<(Vec<Spec>, usize), Error> {
    let (mut specs, resync_record, opening) = unpadded(values, plan, predictor)?;
    if let Some(opening) = opening {
        pad_to_packet(&mut specs, opening, plan.units())?;
    }
    Ok((specs, resync_record))
}

/// [`records`] before the loop region is padded, with the index of the record the loop
/// opens at.
fn unpadded(
    values: &[i32],
    plan: &Plan,
    predictor: Predictor,
) -> Result<(Vec<Spec>, usize, Option<usize>), Error> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let (cell, chunk, stride) = (plan.cell(), plan.chunk(), plan.channels);

    let one_to_one = |out: &mut Vec<Spec>, at: &mut usize, fields: usize| {
        for count in chunks(fields, chunk) {
            let (low, high) = extent(&values[*at..*at + count]);
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

    let mut loop_opening = None;
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
        loop_opening = Some(opening);
    }
    if at != plan.fields {
        return Err(ParseError::AssertFail(format!(
            "the record plan covered {at} of {} fields",
            plan.fields
        ))
        .into());
    }
    Ok((out, resync_record, loop_opening))
}

/// Pad the loop region out to whole packets: sweep its content records front to back,
/// halving each one that covers more than one cell (the smaller half first) and
/// continuing into the second half, pass after pass, until the words fit.
///
/// A region with nothing left to split is widened instead, front to back, spending
/// each content record up to [`widen_cap`] before moving on, so the last one widened
/// takes only the words still owed. Both sweeps skip 1:1 records, whatever room they
/// have, including the marked one the region opens at.
///
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

    let cap = widen_cap(units.layout);
    for spec in specs[opening..].iter_mut() {
        if pad == 0 {
            break;
        }
        if spec.one_to_one {
            continue;
        }
        let count = spec.count;
        let step = |width: u8| units.span(count, width + 1) - units.span(count, width);
        while spec.width < cap && step(spec.width) <= pad {
            pad -= step(spec.width);
            spec.width += 1;
        }
    }
    if pad > 0 {
        return Err(ParseError::OutOfBounds {
            value: format!("a loop of {} record(s)", specs.len() - opening),
            bound: format!(
                "a loop with {pad} more word(s) of room in it; the encoded loop must be \
                 whole packets long, and no record of this one may be widened past \
                 {cap}"
            ),
        }
        .into());
    }
    Ok(())
}

/// Widest the padding sweep writes a content record at, per generation. The cap is
/// fixed per generation, not derived from the region: a record one width under the cap
/// is still widened up to it, and room under the cap is always spent.
///
/// v4 stops one width above v2 and v3. Neither entry derives from the other.
///
/// Inferred from specimens; not confirmed on hardware.
const fn widen_cap(layout: Layout) -> u8 {
    match layout {
        Layout::V2 | Layout::V3 => 13,
        Layout::V4 => 14,
    }
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
/// can drive below the stroke header. The first packet then starts inside what would
/// otherwise be header, and whole packets make up the difference.
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
        let lasts = plan.fields as f64 / plan.channels as f64 / f64::from(codec::FIELD_RATE);
        return Err(ParseError::from(LengthError::Stream {
            words: total,
            fits: lasts * MAX_STREAM_WORDS as f64 / total as f64,
        })
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
    // The terminator states the cell size, and with it the channel count: at twice the
    // layout's cell, a reader de-interleaves.
    if at.checked_add(1) != Some(total) {
        return Err(ParseError::AssertFail(format!(
            "the record chain ended at word {at} of {total}"
        ))
        .into());
    }
    let terminator = Head::terminator(plan.cell()).to_word();
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
/// v2 and v3 store a stereo stroke's channels as alternating fields, the order `values`
/// is already in, so the fields are written in stream order. v4 gives each channel its
/// own word stream and alternates the words, so its halves are packed apart and then
/// interleaved. Only the residual's stride depends on the channel count.
fn write_record(words: &mut [u8], at: usize, spec: &Spec, values: &[i32], units: Units) {
    let (word, bits) = (units.word(), units.word_bits());
    let head = Head {
        one_to_one: spec.one_to_one,
        width: spec.width,
        mark: spec.mark,
        reserved: false,
        order: spec.order,
        count: spec.count,
    }
    .to_word();
    words[at * word..(at + 1) * word].copy_from_slice(&head.to_be_bytes()[4 - word..]);

    let stored = |k: usize| -> u64 {
        let value = residual(values, spec.first + k, spec.order, units.channels);
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
    let half = units.layout.half_span(spec.count, spec.width);
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
/// quantizer shift, and the mantissa is `1/peak` to 20 bits scaled by the zone's gain
/// ([`zone::GAIN_UNITY`](super::zone::GAIN_UNITY) is 1.0). The reciprocal is held as a
/// 24-bit fraction in `[½, 1)`, three bits finer than the mantissa, before the gain
/// multiplies it, and one floor follows. The mantissa may leave its normalized range in
/// either direction; the exponent does not follow it. Refuses a shift the exponent
/// byte cannot state.
///
/// ⚠️ `gain` is the decibel field's round trip, not the project's own float. The two
/// agree below `2^24` and differ above it, where the mantissa wraps within its field
/// and the file states a level far quieter than the project asked for. That is what
/// the instrument plays; a caller that wants to warn about it must do so itself.
///
/// ⚠️ `peak` is the file's, not the stroke's. Every stroke of a multi-zone instrument
/// takes the reciprocal of the largest statistic B in the file, and states its shift
/// against it; only the shift and the zone's own gain belong to the stroke. Using each
/// stroke's own peak leaves every zone but the loudest playing at the wrong level.
fn statistic_a(peak: u32, shift: i32, gain: u64) -> Result<(u32, u8), Error> {
    let peak = u64::from(peak.max(1));
    let bits = 64 - peak.leading_zeros() as i32;
    let exact_power = i32::from(peak.is_power_of_two());
    let reciprocal = (1u64 << (21 + bits + (1 - exact_power))) / peak;
    let mantissa = (reciprocal * gain) >> (super::zone::GAIN_BITS + 3);
    let exponent =
        u8::try_from(22 + shift - bits + exact_power).map_err(|_| ParseError::OutOfBounds {
            value: format!("a quantizer shift of {shift} bits against a peak of {peak}"),
            bound: "a shift statistic A's exponent byte can state".into(),
        })?;
    Ok(((mantissa % (1 << 24)) as u32, exponent))
}

/// Build the fixed header and its body-relative, wrapping word directory.
fn stroke_header(
    layout: Layout,
    zone: &Placement,
    encoded: &Encoded,
    body_at: usize,
    file_peak: u32,
) -> Result<Vec<u8>, Error> {
    let (q, stream) = (&encoded.q, &encoded.stream);
    let mut head = vec![0u8; layout.header_len()];
    head[0..4].copy_from_slice(&zone.global_id.to_be_bytes());
    head[super::stroke::ROOT_KEY] = zone.root_key;
    // Unexplained: real programs hold this, and the panel cannot produce it.
    head[6..8].copy_from_slice(&[0x88, 0xba]);
    // The channel count. The terminator's cell size states it too, and a reader uses
    // the terminator because the record sizes follow it.
    head[8] = zone.channels as u8;

    let (mantissa, exponent) =
        statistic_a(file_peak, q.shift, gain_units(gain_decibels(zone.gain)))?;
    head[codec::MANTISSA_AT..codec::MANTISSA_AT + 3].copy_from_slice(&mantissa.to_be_bytes()[1..]);
    head[codec::STAT_A_EXP_AT] = exponent;
    head[codec::PEAK_AT..codec::PEAK_AT + 3].copy_from_slice(&(q.peak as u32).to_be_bytes()[1..]);

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
        let at = codec::SEEK_AT + codec::SEEK_STRIDE * i;
        head[at..at + 2].copy_from_slice(&p.to_be_bytes());
        // Unexplained: real programs hold this, and the panel cannot produce it.
        if i < 3 {
            head[at + 2] = 0x80;
        }
    }
    // The wide header's two float32 tails; the narrow header is too short to hold them.
    let tails = [gain_decibels(zone.gain), zone.loop_decay];
    for (at, value) in codec::TAIL_FLOATS_AT.iter().zip(tails) {
        if let Some(slot) = head.get_mut(*at..at + 4) {
            slot.copy_from_slice(&value.to_be_bytes());
        }
    }
    Ok(head)
}

/// The loop decay amount a project carries until something sets one.
pub const DEFAULT_LOOP_DECAY: f32 = 20.0;

/// One zone's stream, and the quantizer statistics describing it.
///
/// A stroke header cannot be written until every zone is encoded: statistic A divides
/// by the file's peak, so the last zone's audio decides the first zone's header.
struct Encoded {
    q: Quantized,
    stream: Stream,
}

/// Lay out and pack one zone's stream, into `preamble` bytes plus whole packets.
fn encode_stroke(
    layout: Layout,
    zone: &ZoneSpec<'_>,
    preamble: usize,
    predictor: Predictor,
) -> Result<Encoded, Error> {
    let channels = usize::from(zone.placement.channels);
    let lattice = match zone.audio {
        Audio::Lattice(lattice) => lattice,
        Audio::Pcm {
            source,
            loops,
            secondary_start,
            shift,
        } => {
            let frames = frames_of(source, channels)?;
            let plan = match loops {
                Some(points) => Plan::looped(layout, frames, channels, points, secondary_start)?,
                None => Plan::new(layout, frames, channels, secondary_start)?,
            };
            if let Some(bits) = shift {
                if i32::from(bits) > codec::SHIFT_LIMIT {
                    return Err(ParseError::OutOfBounds {
                        value: format!("a quantizer shift of {bits} bits"),
                        bound: format!("0 through {} bits", codec::SHIFT_LIMIT),
                    }
                    .into());
                }
            }
            let q = quantize(&kernel_source(source, channels, loops), &plan, shift);
            return laid_out(q, &plan, preamble, predictor);
        }
    };
    if usize::from(lattice.channels) != channels {
        return Err(ParseError::AssertFail(format!(
            "a {}-channel stream placed as a {channels}-channel zone",
            lattice.channels
        ))
        .into());
    }
    // A loop too short for the generation's opening run, or to fill whole packets under
    // the editor's padding, plays the same with its period repeated, so it is laid out
    // over more periods until one fits.
    let mut repeated = Cow::Borrowed(lattice);
    let mut first = None;
    for _ in 0..MAX_LOOP_PERIODS {
        let short = match relaid(layout, &repeated, preamble, predictor)? {
            Relaid::Laid(encoded) => return Ok(encoded),
            Relaid::Short(error) => error,
        };
        first.get_or_insert(short);
        let mark = lattice.mark.expect("only a loop is short");
        let period = lattice.fields[mark..].to_vec();
        repeated.to_mut().fields.extend(period);
    }
    Err(first.expect("the loop runs at least once"))
}

/// The most periods a loop laid out from the lattice is repeated over to fit.
const MAX_LOOP_PERIODS: usize = 16;

/// A stream laid out from the lattice, or the refusal of a loop too short for its
/// generation, which more periods of the same loop cure.
enum Relaid {
    Laid(Encoded),
    Short(Error),
}

fn relaid(
    layout: Layout,
    lattice: &codec::Lattice,
    preamble: usize,
    predictor: Predictor,
) -> Result<Relaid, Error> {
    let channels = usize::from(lattice.channels);
    let units = Units { layout, channels };
    let fields = lattice.fields.len();
    if let Some(mark) = lattice.mark.filter(|&mark| mark < fields) {
        if let Err(short) = loop_warmup(units, fields - mark) {
            return Ok(Relaid::Short(short));
        }
    }
    let plan = Plan::on_lattice(layout, fields, channels, lattice.resync_at, lattice.mark)?;
    let q = checked_width(requantize(lattice, &plan)?)?;
    let (mut specs, resync_record, opening) = unpadded(&q.values, &plan, predictor)?;
    if let Some(opening) = opening {
        if let Err(short) = pad_to_packet(&mut specs, opening, units) {
            return Ok(Relaid::Short(short));
        }
    }
    let stream = pack(&specs, &q.values, resync_record, preamble, &plan)?;
    Ok(Relaid::Laid(Encoded { q, stream }))
}

/// Records and packing for quantized fields on `plan`.
fn laid_out(
    q: Quantized,
    plan: &Plan,
    preamble: usize,
    predictor: Predictor,
) -> Result<Encoded, Error> {
    let q = checked_width(q)?;
    let (specs, resync_record) = records(&q.values, plan, predictor)?;
    let stream = pack(&specs, &q.values, resync_record, preamble, plan)?;
    Ok(Encoded { q, stream })
}

/// Quantized fields, once they fit the stream's widest field.
fn checked_width(q: Quantized) -> Result<Quantized, Error> {
    let (low, high) = extent(&q.values);
    if width_of(low, high) <= MAX_STORED_WIDTH {
        return Ok(q);
    }
    Err(ParseError::OutOfBounds {
        value: format!(
            "a quantizer shift of {} bits for fields spanning {low}..={high}",
            q.shift
        ),
        bound: format!("values that fit the stream's {MAX_STORED_WIDTH}-bit fields"),
    }
    .into())
}

/// Every zone's stream in order, and the file peak every header's statistic A divides
/// by.
fn encode_strokes(
    layout: Layout,
    zones: &[ZoneSpec<'_>],
    predictor: Predictor,
    cat_len: usize,
    map_len: usize,
) -> Result<(Vec<Encoded>, u32), Error> {
    let encoded = zones
        .iter()
        .enumerate()
        .map(|(index, zone)| {
            let chain = super::Chain::written_for(layout);
            let preamble = super::stroke::header_len(layout, chain, index, cat_len, map_len);
            encode_stroke(layout, zone, preamble, predictor)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let peak = encoded
        .iter()
        .map(|e| e.q.peak.unsigned_abs())
        .max()
        .unwrap_or(1);
    Ok((encoded, peak))
}

/// Appends one `stk` section per zone behind `sections`, each payload built at the
/// body offset it lands on.
fn push_strokes<F: Framing>(
    sections: &mut Vec<Framed<F>>,
    layout: Layout,
    zones: &[ZoneSpec<'_>],
    (encoded, file_peak): (Vec<Encoded>, u32),
    tag: F::Tag,
    version: F::Version,
) -> Result<(), Error> {
    let mut body_at: usize = sections.iter().map(Framed::encoded_len).sum();
    for (zone, stroke) in zones.iter().zip(&encoded) {
        let payload = stroke_payload(
            layout,
            &zone.placement,
            stroke,
            body_at + F::HEADER,
            file_peak,
        )?;
        body_at += F::HEADER + payload.len();
        sections.push(Framed {
            tag,
            version,
            payload,
        });
    }
    Ok(())
}

/// One zone's `stk` payload at body offset `body_at`.
///
/// `body_at` comes from the sections already sized in front of this stroke, so only
/// the chain builders can supply it: it is the base the word directory is written
/// against, and a wrong one produces a file whose directory names records that are
/// not there.
fn stroke_payload(
    layout: Layout,
    zone: &Placement,
    encoded: &Encoded,
    body_at: usize,
    file_peak: u32,
) -> Result<Vec<u8>, Error> {
    midi_note("root key", zone.root_key)?;
    body_at
        .checked_add(layout.header_len())
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("body offset {body_at}"),
            bound: "an addressable stroke header".into(),
        })?;
    let mut payload = stroke_header(layout, zone, encoded, body_at, file_peak)?;
    payload.extend_from_slice(&encoded.stream.words);
    Ok(payload)
}

/// Section schema versions the narrow chain writes. They track each section's own
/// schema, not the content version.
const HDR_VERSION: u8 = 9;
const CAT_VERSION: u8 = 5;
const STK_VERSION: u8 = 9;
const STY_VERSION: u8 = 5;
const CONTAINER_VERSION: u8 = 11;

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

/// The `cat` section.
fn cat(categories: &NarrowCat) -> Result<Section, Error> {
    Ok(Section {
        tag: *section::CAT,
        version: CAT_VERSION,
        payload: categories.payload()?,
    })
}

/// Build the keyboard map and the zone table behind it.
///
/// `zones` is one record per zone, already high to low.
fn map(keys: &KeyTable, zones: &[ZoneRecord]) -> Result<Section, Error> {
    let mut payload = vec![0u8; super::zone::RECORDS_AT + super::zone::RECORD_LEN * zones.len()];
    payload[..super::zone::COUNT_AT].copy_from_slice(&keys.prefix());
    payload[super::zone::COUNT_AT] = zones.len() as u8;
    // Zones are stored high to low by top note.
    for (index, record) in zones.iter().enumerate() {
        let at = super::zone::RECORDS_AT + super::zone::RECORD_LEN * index;
        payload[at + 2] = record.id;
        // Nothing here says whether the zone loops: a zone record is byte-identical
        // either way, and the loop lives in the stroke's own word directory.
        payload[at + 3..at + 6].copy_from_slice(&record.gain.to_be_bytes()[1..]);
        payload[at + 9] = record.top_note;
        payload[at + 10..at + 12].copy_from_slice(&record.rel_strength.to_be_bytes());
    }
    Ok(Section {
        tag: *section::MAP,
        version: super::keymap::VERSION,
        payload,
    })
}

/// The narrow `sty` preset a project that sets nothing renders as.
/// Unexplained: real programs hold this, and the panel cannot produce it.
pub(crate) const STY_V2_PAYLOAD: [u8; super::sty::V2_LEN] =
    [0x00, 0x01, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00];

/// The narrow `sty` preset, including every project value its schema stores.
fn sty(preset: Preset) -> Result<Section, Error> {
    if preset.velocity_to_amplitude >= super::sty::VELOCITY_LEVELS
        || preset.velocity_to_timbre >= super::sty::VELOCITY_LEVELS
    {
        return Err(ParseError::OutOfBounds {
            value: format!(
                "velocity levels {} and {}",
                preset.velocity_to_amplitude, preset.velocity_to_timbre
            ),
            bound: format!("levels below {}", super::sty::VELOCITY_LEVELS),
        }
        .into());
    }
    let mut payload = STY_V2_PAYLOAD.to_vec();
    payload[super::sty::V2_DYNAMICS_ENABLE] = u8::from(preset.dynamics_enabled);
    payload[super::sty::V2_VELOCITY_TO_AMPLITUDE] = preset.velocity_to_amplitude;
    payload[super::sty::V2_VELOCITY_TO_TIMBRE] = preset.velocity_to_timbre;
    Ok(Section {
        tag: *section::STY,
        version: STY_VERSION,
        payload,
    })
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
    sty: u32,
    /// The preset a project that touches none renders as.
    /// Unexplained: real programs hold this, and the panel cannot produce it.
    sty_payload: &'static [u8],
    /// Where the category's dynamics curve writes into that payload, and what.
    sty_dynamics: &'static [(usize, u8)],
}

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

const STY_V3_DYNAMICS: [(usize, u8); 4] = [(4, 43), (12, 74), (14, 1), (16, 74)];
const STY_V4_DYNAMICS: [(usize, u8); 5] = [(3, 1), (4, 1), (85, 74), (86, 82), (87, 90)];

/// The schema for a wide generation, `None` for the narrow chain.
fn wide_schema(layout: Layout) -> Option<WideSchema> {
    match layout {
        Layout::V2 => None,
        Layout::V3 => Some(WideSchema {
            container: 30,
            container_payload: [0x00, 0x02, 0x00, 0x0c],
            hdr: 10,
            map: 14,
            key_stride: super::keymap::RECORD_LEN,
            map_gap: &[],
            map_tail: &[0x00],
            sty: super::sty::VERSION_V3,
            sty_payload: &STY_V3_PAYLOAD,
            sty_dynamics: &STY_V3_DYNAMICS,
        }),
        Layout::V4 => Some(WideSchema {
            container: 40,
            container_payload: [0x00, 0x02, 0x00, 0x05],
            hdr: 11,
            map: 21,
            key_stride: super::keymap::RECORD_LEN + 4,
            map_gap: &[
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
                0x02, 0x02, 0x02, 0x10, 0x00, 0x00, 0x10, 0x00, 0x00, 0x10, 0x00, 0x00, 0x10, 0x00,
                0x00, 0x00, 0x00,
            ],
            map_tail: &[0x00, 0x00, 0x00, 0x01, 0x00, 0x00],
            sty: super::sty::VERSION_V4,
            sty_payload: &STY_V4_PAYLOAD,
            sty_dynamics: &STY_V4_DYNAMICS,
        }),
    }
}

/// The wide `hdr` section: the same prefix at a wider name field, then the sub name
/// the vendor's filenames append, NUL-terminated within the section.
fn hdr4(schema: &WideSchema, name: &str, sub_name: &str) -> Result<Section4, Error> {
    let mut payload = vec![0u8; 112];
    // Unexplained: real programs hold this, and the panel cannot produce it.
    payload[4..6].copy_from_slice(&[0x06, 0x50]);
    super::StringField::NAME_V3.write(&mut payload, name)?;
    super::StringField::SUB_NAME_V3.write(&mut payload, sub_name)?;
    Ok(Section4 {
        tag: *section::HDR4,
        version: schema.hdr,
        payload,
    })
}

/// The wide `cat` section: the first two categories alone, where the narrow chain
/// also stores three more and spells out its labels.
fn cat4(categories: &NarrowCat) -> Section4 {
    Section4 {
        tag: *section::CAT4,
        version: 7,
        payload: super::cat::wide_payload(categories.category, categories.sub_category).to_vec(),
    }
}

/// The wide `map` section: a per-key table at unity gain and no detune, then the
/// zone records behind their count.
///
/// The wider schema's per-key record carries a partner quad as well as the level.
/// The editor writes the identity there whatever the zone layout (only the vendor's
/// builder fills it in), so every quad names its own key.
fn map4(
    schema: &WideSchema,
    instrument: Level,
    zones: &[WideZoneRecord],
) -> Result<Section4, Error> {
    let mut payload = Vec::with_capacity(
        super::keymap::RECORD_LEN
            + super::keymap::KEYS * schema.key_stride
            + schema.map_gap.len()
            + 1
            + super::zone::WIDE_RECORD_LEN * zones.len()
            + schema.map_tail.len(),
    );
    let mut level = [0u8; super::keymap::RECORD_LEN];
    instrument.write(&mut level);
    payload.extend_from_slice(&level);
    super::keymap::Level::NEUTRAL.write(&mut level);
    for key in 0..super::keymap::KEYS as u8 {
        payload.extend_from_slice(&level);
        payload.extend(std::iter::repeat_n(
            key,
            schema.key_stride - super::keymap::RECORD_LEN,
        ));
    }
    payload.extend_from_slice(schema.map_gap);
    payload.push(zones.len() as u8);
    for record in zones {
        payload.extend_from_slice(&record.bytes());
    }
    payload.extend_from_slice(schema.map_tail);
    Ok(Section4 {
        tag: *section::MAP4,
        version: schema.map,
        payload,
    })
}

/// The wide `sty` preset, including the dynamics group a project controls.
fn sty4(schema: &WideSchema, preset: Preset) -> Section4 {
    let mut payload = schema.sty_payload.to_vec();
    if preset.dynamics_enabled {
        for &(at, value) in schema.sty_dynamics {
            payload[at] = value;
        }
    }
    Section4 {
        tag: *section::STY4,
        version: schema.sty,
        payload,
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
    /// interleaved when it has more than one channel.
    pub source: &'a [i16],
    /// Channels [`source`](NewZone::source) interleaves: 1 or 2.
    pub channels: u16,
    /// The note this sample plays untransposed at.
    pub root_key: u8,
    /// Highest note this zone answers to. Stored as given; the file keeps top notes and
    /// does not derive them from the root keys.
    pub top_note: u8,
    /// The stroke's global id, 1 through 255: one byte, and the editor never issues
    /// zero. Zones name their strokes by it, not by position, so it need not follow
    /// section order.
    pub global_id: u32,
    /// The zone's sustain loop, which truncates its audio at [`Loop::end`].
    pub loops: Option<Loop>,
    /// Where the stream resynchronizes: the project's `m_startSecondary` in source
    /// frames from the first frame of [`source`](NewZone::source), after the repair
    /// the editor applies on load ([`nsmpproj::Stroke::encoded_secondary_start`]).
    pub secondary_start: f64,
    /// Quantizer shift that overrides the shift rule, or `None` for the rule.
    /// Experimental; see [`Options::shift`].
    pub shift: Option<u8>,
    /// The stroke's loop decay amount (a project's `m_loopDecay`) in the project's own
    /// units, [`DEFAULT_LOOP_DECAY`] until something sets one.
    ///
    /// ⚠️ A wide stroke header carries it whether or not the stroke loops and whether
    /// or not the decay is switched on; nothing in the file says which. The narrow
    /// chain drops the field altogether.
    pub loop_decay: f32,
    /// Playback gain as a linear ratio, 1.0 for unity, below [`MAX_ZONE_GAIN`]. Not
    /// applied to the audio: the instrument applies it when it plays.
    ///
    /// Where it is stored depends on the generation, and the stroke's statistic A
    /// carries it in every one. The narrow zone record holds it linearly to 20
    /// fractional bits; a wide stroke header holds `20·log10(gain)` as a float32 and
    /// no byte of a wide zone record moves with it.
    pub gain: f64,
}

/// Build a one-zone instrument from PCM at [`codec::SOURCE_RATE`], mono or stereo
/// interleaved per [`Options::channels`], in the generation [`Options::layout`] names.
/// Refuses unmodeled lengths, invalid metadata, and streams past the directory limit.
pub fn instrument(source: &[i16], options: &Options) -> Result<crate::Sample, Error> {
    midi_note("root key", options.root_key)?;
    let frames = frames_of(source, usize::from(options.channels))?;
    let secondary_start = options
        .secondary_start
        .unwrap_or_else(|| default_secondary_start(frames, options.loops));
    multi_zone(
        Instrument {
            name: &options.name,
            map_gain: 1.0,
            predictor: options.predictor,
            layout: options.layout,
            preset: Preset::default(),
        },
        &[NewZone {
            source,
            channels: options.channels,
            root_key: options.root_key,
            top_note: options.resolved_top_note(),
            global_id: 1,
            loops: options.loops,
            secondary_start,
            shift: options.shift,
            gain: 1.0,
            loop_decay: DEFAULT_LOOP_DECAY,
        }],
    )
}

/// Everything an instrument states apart from its zones.
#[derive(Debug, Clone, Copy)]
pub struct Instrument<'a> {
    /// The name the `hdr` section carries.
    pub name: &'a str,
    /// The instrument's own playing gain, a linear ratio on top of every zone's. It
    /// opens the `map` section in all three generations, and it is the only gain field
    /// that clamps, at [`MAX_MAP_GAIN_DB`].
    pub map_gain: f64,
    /// How content records code their fields.
    pub predictor: Predictor,
    /// Which generation to write: `.nsmp`, `.nsmp3` or `.nsmp4`.
    pub layout: Layout,
    /// The sound preset values a project can carry into the instrument.
    pub preset: Preset,
}

/// Project preset values with a decoded destination in at least one generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// Whether the instrument loads with its category's dynamics curve.
    pub dynamics_enabled: bool,
    /// The narrow preset's velocity-to-amplitude level.
    pub velocity_to_amplitude: u8,
    /// The narrow preset's velocity-to-timbre level.
    pub velocity_to_timbre: u8,
}

impl Default for Preset {
    fn default() -> Preset {
        Preset {
            dynamics_enabled: false,
            velocity_to_amplitude: 1,
            velocity_to_timbre: 1,
        }
    }
}

/// Build an instrument that spans the keyboard: one `stk` per zone, in the order
/// given, which must be highest zone first.
///
/// Refuses an empty or overlapping zone list, a duplicate or unnameable stroke id,
/// and everything [`instrument`] refuses about one zone's audio.
pub fn multi_zone(
    instrument: Instrument<'_>,
    zones: &[NewZone<'_>],
) -> Result<crate::Sample, Error> {
    let mut keys = KeyTable::NEUTRAL;
    keys.instrument = Level::new(map_gain_units(instrument.map_gain), 0)?;
    let zones: Vec<ZoneSpec<'_>> = zones.iter().map(ZoneSpec::from).collect();
    chain(
        &Front {
            aux: aux(&NarrowCat::editor_default()),
            name: instrument.name,
            sub_name: "",
            categories: NarrowCat::editor_default(),
            keys,
            predictor: instrument.predictor,
            layout: instrument.layout,
            preset: instrument.preset,
        },
        &zones,
    )
}

/// One zone laid out again from a stream already on the lattice: its fields and
/// landmarks, and everything the chain states about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatticeZone<'a> {
    pub audio: &'a codec::Lattice,
    /// The note this sample plays untransposed at.
    pub root_key: u8,
    /// Highest note this zone answers to.
    pub top_note: u8,
    /// Lowest note, which only the wide chain stores. `None` reaches down to one
    /// above the next zone's top, or the keyboard's floor below the last zone, and
    /// then the zones must be given highest first.
    pub low_note: Option<u8>,
    /// The stroke's global id. A narrow record names it by its low byte, so on the
    /// narrow chain no two zones' ids may share one.
    pub global_id: u32,
    /// Playback gain as a linear ratio, as [`NewZone::gain`].
    pub gain: f64,
    /// The wide header's loop decay amount, as [`NewZone::loop_decay`].
    pub loop_decay: f32,
    /// Where the playing stroke sits on the strength axis, as
    /// [`Zone::rel_strength`](super::Zone::rel_strength).
    pub rel_strength: u16,
    /// The velocities the zone answers to, which only the wide chain stores.
    pub velocity: VelocityWindow,
}

/// Everything an instrument laid out from lattice streams states apart from its zones.
#[derive(Debug, Clone, PartialEq)]
pub struct LatticeInstrument<'a> {
    /// The container header's `aux` word, which the editor derives from the first two
    /// categories and library files fill with ones. Written as given.
    pub aux: u32,
    pub name: &'a str,
    /// The wide `hdr`'s sub name. The narrow chain has no field for one, so it must be
    /// empty there.
    pub sub_name: &'a str,
    /// The `cat` section's categories and labels. The wide chain stores only the first
    /// two, so the rest must be the editor's defaults there.
    pub categories: NarrowCat,
    /// The keyboard map: the instrument's own gain and detune, written as stored in
    /// every generation, and the per-key records, which only the narrow chain writes.
    /// A wide map is written neutral, so its per-key records must be.
    pub keys: KeyTable,
    pub layout: Layout,
    pub preset: Preset,
}

/// Lay streams already on the lattice out again in the generation `instrument`
/// names, in the editor's record coding.
///
/// No field is resampled, ramped again or rewritten: each stroke keeps its fields and
/// its landmarks, and only the units, the record coding and the shift change. The
/// shift is the target generation's rule applied to the stored fields, never finer
/// than they were stored at, so a stream rendered at a finer shift comes out as the
/// editor renders the coarser generation.
///
/// A loop too short for the generation's opening run or its packet padding is laid out
/// over as many repeats of its period as fit, which plays the same.
///
/// Refuses everything [`multi_zone`] refuses about a zone list, except that any stroke
/// id is taken; landmarks the generation cannot lay out ([`Plan::on_lattice`]); and a
/// sub name, narrow-only categories or a non-neutral per-key table where the generation
/// has no field for them.
pub fn from_lattice(
    instrument: &LatticeInstrument<'_>,
    zones: &[LatticeZone<'_>],
) -> Result<crate::Sample, Error> {
    let (front, zones) = lattice_front(instrument, zones)?;
    chain(&front, &zones)
}

/// The chain inputs [`from_lattice`] builds, once what the generation cannot hold is
/// refused.
fn lattice_front<'a>(
    instrument: &LatticeInstrument<'a>,
    zones: &[LatticeZone<'a>],
) -> Result<(Front<'a>, Vec<ZoneSpec<'a>>), Error> {
    let narrow = wide_schema(instrument.layout).is_none();
    if narrow && !instrument.sub_name.is_empty() {
        return Err(ParseError::AssertFail(format!(
            "the sub name {:?}: the narrow chain has no field for one",
            instrument.sub_name
        ))
        .into());
    }
    let default_cat = NarrowCat {
        category: instrument.categories.category,
        sub_category: instrument.categories.sub_category,
        ..NarrowCat::editor_default()
    };
    if !narrow && instrument.categories != default_cat {
        return Err(ParseError::AssertFail(format!(
            "categories {:?}: the wide cat holds only the first two",
            instrument.categories
        ))
        .into());
    }
    if !narrow && instrument.keys.adjusted().next().is_some() {
        return Err(ParseError::AssertFail(
            "per-key gain or detune: the wide map is written neutral".into(),
        )
        .into());
    }
    let zones: Vec<ZoneSpec<'_>> = zones
        .iter()
        .map(|zone| ZoneSpec {
            placement: Placement {
                ids: 0..=u32::MAX,
                channels: zone.audio.channels,
                root_key: zone.root_key,
                top_note: zone.top_note,
                low_note: zone.low_note,
                global_id: zone.global_id,
                gain: zone.gain,
                loop_decay: zone.loop_decay,
                rel_strength: zone.rel_strength,
                velocity: zone.velocity,
            },
            audio: Audio::Lattice(zone.audio),
        })
        .collect();
    Ok((
        Front {
            aux: instrument.aux,
            name: instrument.name,
            sub_name: instrument.sub_name,
            categories: instrument.categories.clone(),
            keys: instrument.keys.clone(),
            predictor: Predictor::Minimizing,
            layout: instrument.layout,
            preset: instrument.preset,
        },
        zones,
    ))
}

/// The container header and the sections [`from_lattice`] writes for `instrument`,
/// without the strokes or the length the wide chain closes with: everything the
/// instrument states outside its audio.
pub(crate) fn frame(
    instrument: &LatticeInstrument<'_>,
    zones: &[LatticeZone<'_>],
) -> Result<crate::Sample, Error> {
    let (front, zones) = lattice_front(instrument, zones)?;
    Ok(match wide_schema(front.layout) {
        Some(schema) => {
            let (mut sections, _, _) = wide_head(&front, &zones, &schema)?;
            sections.push(sty4(&schema, front.preset));
            crate::Sample::V3(Cbin {
                header: container(front.layout, front.aux),
                body: SampleV3 { sections },
            })
        }
        None => {
            let (mut sections, _, _) = narrow_head(&front, &zones)?;
            sections.push(sty(front.preset)?);
            crate::Sample::V2(Cbin {
                header: container(front.layout, front.aux),
                body: Sample { sections },
            })
        }
    })
}

/// Everything a chain states about one zone apart from its audio.
#[derive(Debug, Clone, PartialEq)]
struct Placement {
    /// The stroke ids the caller may name: one byte, never zero, for a new instrument,
    /// as the editor issues them; any id for a stream laid out again.
    ids: RangeInclusive<u32>,
    channels: u16,
    root_key: u8,
    top_note: u8,
    /// `None` where the zone reaches down to one above the zone below it.
    low_note: Option<u8>,
    global_id: u32,
    gain: f64,
    loop_decay: f32,
    rel_strength: u16,
    velocity: VelocityWindow,
}

/// Where a zone's stream comes from.
#[derive(Debug, Clone, Copy)]
enum Audio<'a> {
    /// PCM at [`codec::SOURCE_RATE`], resampled onto the lattice.
    Pcm {
        source: &'a [i16],
        loops: Option<Loop>,
        secondary_start: f64,
        shift: Option<u8>,
    },
    /// Fields already on the lattice.
    Lattice(&'a codec::Lattice),
}

struct ZoneSpec<'a> {
    placement: Placement,
    audio: Audio<'a>,
}

/// A new zone holds one sample, so its stroke sits at the bottom of the strength axis,
/// and it answers every velocity.
impl<'a> From<&NewZone<'a>> for ZoneSpec<'a> {
    fn from(zone: &NewZone<'a>) -> ZoneSpec<'a> {
        ZoneSpec {
            placement: Placement {
                ids: 1..=MAX_STROKE_ID,
                channels: zone.channels,
                root_key: zone.root_key,
                top_note: zone.top_note,
                low_note: None,
                global_id: zone.global_id,
                gain: zone.gain,
                loop_decay: zone.loop_decay,
                rel_strength: super::zone::REL_STRENGTH_DEFAULT,
                velocity: VelocityWindow::FULL,
            },
            audio: Audio::Pcm {
                source: zone.source,
                loops: zone.loops,
                secondary_start: zone.secondary_start,
                shift: zone.shift,
            },
        }
    }
}

/// Everything a chain states apart from its zones.
struct Front<'a> {
    aux: u32,
    name: &'a str,
    sub_name: &'a str,
    categories: NarrowCat,
    keys: KeyTable,
    predictor: Predictor,
    layout: Layout,
    preset: Preset,
}

fn chain(front: &Front<'_>, zones: &[ZoneSpec<'_>]) -> Result<crate::Sample, Error> {
    match wide_schema(front.layout) {
        Some(schema) => wide_chain(front, zones, &schema).map(crate::Sample::V3),
        None => narrow_chain(front, zones).map(crate::Sample::V2),
    }
}

/// The CBIN header every generation writes, at its own content version.
fn container(layout: Layout, aux: u32) -> Header {
    Header {
        generation: Generation::V1,
        tag: *b"nsmp",
        location: 0xFFFF_FFFF,
        aux,
        version: version(layout),
    }
}

/// The narrow chain's sections ahead of the strokes, with the `cat` and `map`
/// payload lengths that decide where the first stroke's audio may start.
fn narrow_head(
    front: &Front<'_>,
    zones: &[ZoneSpec<'_>],
) -> Result<(Vec<Section>, usize, usize), Error> {
    let table = zone_table(zones)?;
    let hdr = hdr(front.name)?;
    let cat = cat(&front.categories)?;
    let map = map(&front.keys, &table)?;
    let (cat_len, map_len) = (cat.payload.len(), map.payload.len());
    let sections = vec![
        Section {
            tag: *section::CONTAINER,
            version: CONTAINER_VERSION,
            payload: Vec::new(),
        },
        hdr,
        cat,
        map,
    ];
    Ok((sections, cat_len, map_len))
}

fn narrow_chain(front: &Front<'_>, zones: &[ZoneSpec<'_>]) -> Result<Cbin<Sample>, Error> {
    // The directory a stroke carries counts words from the start of the body, and the
    // head decides where the first packet may start, so it is sized before any stream
    // is written.
    let (mut sections, cat_len, map_len) = narrow_head(front, zones)?;
    let strokes = encode_strokes(Layout::V2, zones, front.predictor, cat_len, map_len)?;
    push_strokes(
        &mut sections,
        Layout::V2,
        zones,
        strokes,
        *section::STK,
        STK_VERSION,
    )?;
    sections.push(sty(front.preset)?);

    Ok(Cbin {
        header: container(Layout::V2, front.aux),
        body: Sample { sections },
    })
}

/// The `stk` schema version both wide generations carry.
const STK4_VERSION: u32 = 11;

/// The wide chain's sections ahead of the strokes, as [`narrow_head`].
fn wide_head(
    front: &Front<'_>,
    zones: &[ZoneSpec<'_>],
    schema: &WideSchema,
) -> Result<(Vec<Section4>, usize, usize), Error> {
    let table = wide_zone_table(zones)?;
    let hdr = hdr4(schema, front.name, front.sub_name)?;
    let cat = cat4(&front.categories);
    let map = map4(schema, front.keys.instrument, &table)?;
    let (cat_len, map_len) = (cat.payload.len(), map.payload.len());
    let sections = vec![
        Section4 {
            tag: *section::CONTAINER4,
            version: schema.container,
            payload: schema.container_payload.to_vec(),
        },
        hdr,
        cat,
        map,
    ];
    Ok((sections, cat_len, map_len))
}

fn wide_chain(
    front: &Front<'_>,
    zones: &[ZoneSpec<'_>],
    schema: &WideSchema,
) -> Result<Cbin<SampleV3>, Error> {
    let layout = front.layout;
    let (mut sections, cat_len, map_len) = wide_head(front, zones, schema)?;
    let strokes = encode_strokes(layout, zones, front.predictor, cat_len, map_len)?;
    push_strokes(
        &mut sections,
        layout,
        zones,
        strokes,
        *section::STK4,
        STK4_VERSION,
    )?;
    sections.push(sty4(schema, front.preset));
    let chain_len: usize = sections.iter().map(Framed::encoded_len).sum();
    sections.push(meta4(chain_len));

    Ok(Cbin {
        header: container(layout, front.aux),
        body: SampleV3 { sections },
    })
}

/// What a wide `map` section stores per zone.
struct WideZoneRecord {
    root_key: u8,
    top_note: u8,
    low_note: u8,
    global_id: u32,
    rel_strength: u16,
    velocity: VelocityWindow,
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
        r[12..14].copy_from_slice(&self.rel_strength.to_be_bytes());
        r[14] = self.velocity.low;
        r[15] = self.velocity.high;
        r
    }
}

/// Validate the zone list and reduce it to the records a wide `map` stores.
///
/// A wide zone states its own bottom as well as its top. A zone given no bottom
/// reaches down to one above the zone below it, and the lowest reaches the keyboard's
/// floor; zones given their bottoms may come in either order, as the format allows.
fn wide_zone_table(zones: &[ZoneSpec<'_>]) -> Result<Vec<WideZoneRecord>, Error> {
    let tiled = zones.iter().any(|z| z.placement.low_note.is_none());
    checked(zones, tiled, false)?;
    zones
        .iter()
        .enumerate()
        .map(|(index, zone)| {
            let zone = &zone.placement;
            let low_note = match zone.low_note {
                Some(low) => low,
                None => match zones.get(index + 1) {
                    Some(below) => below.placement.top_note.saturating_add(1),
                    None => super::zone::KEY_FLOOR,
                },
            };
            midi_note("low note", low_note)?;
            Ok(WideZoneRecord {
                root_key: zone.root_key,
                top_note: zone.top_note,
                low_note,
                global_id: zone.global_id,
                rel_strength: zone.rel_strength,
                velocity: zone.velocity,
            })
        })
        .collect()
}

/// What the `map` section stores per zone.
struct ZoneRecord {
    id: u8,
    top_note: u8,
    gain: u32,
    rel_strength: u16,
}

/// Validate the zone list and reduce it to the records the `map` section stores.
fn zone_table(zones: &[ZoneSpec<'_>]) -> Result<Vec<ZoneRecord>, Error> {
    checked(zones, true, true)?;
    Ok(zones
        .iter()
        .map(|zone| ZoneRecord {
            id: zone.placement.global_id as u8,
            top_note: zone.placement.top_note,
            gain: zone_record_gain(zone.placement.gain),
            rel_strength: zone.placement.rel_strength,
        })
        .collect())
}

/// Refuse a zone list no chain can state: an empty or oversized one, notes outside
/// MIDI, a duplicate or unnameable stroke id, a gain past [`MAX_ZONE_GAIN`], and, where
/// the zones `tile`, one not given highest first.
///
/// A narrow zone record names its stroke by the id's low byte, so ids pair modulo 256
/// there and must differ in that byte.
fn checked(zones: &[ZoneSpec<'_>], tile: bool, narrow: bool) -> Result<(), Error> {
    if zones.is_empty() || zones.len() > MAX_ZONES {
        return Err(ParseError::OutOfBounds {
            value: format!("{} zones", zones.len()),
            bound: format!("1 through {MAX_ZONES}, the map section's own count byte"),
        }
        .into());
    }
    let mut ids = Vec::with_capacity(zones.len());
    for (index, zone) in zones.iter().enumerate() {
        let zone = &zone.placement;
        midi_note("root key", zone.root_key)?;
        midi_note("top note", zone.top_note)?;
        if !zone.ids.contains(&zone.global_id) {
            return Err(ParseError::OutOfBounds {
                value: format!("stroke id {}", zone.global_id),
                bound: format!(
                    "{} through {}, what a zone record can name",
                    zone.ids.start(),
                    zone.ids.end()
                ),
            }
            .into());
        }
        if !zone.gain.is_finite() || zone.gain > MAX_ZONE_GAIN {
            return Err(ParseError::OutOfBounds {
                value: format!("zone {index} gain {}", zone.gain),
                bound: format!("a finite gain up to {MAX_ZONE_GAIN}"),
            }
            .into());
        }
        let id = match narrow {
            true => zone.global_id & 0xff,
            false => zone.global_id,
        };
        if ids.contains(&id) {
            return Err(ParseError::AssertFail(format!(
                "two zones claim stroke id {}, and a zone record names its stroke by id",
                zone.global_id
            ))
            .into());
        }
        ids.push(id);
        let above = index.checked_sub(1).map(|i| zones[i].placement.top_note);
        if let Some(above) = above.filter(|&above| tile && zone.top_note >= above) {
            return Err(ParseError::AssertFail(format!(
                "zone {index} reaches up to note {} but the zone before it stops at \
                 {above}; zones are stored highest first and may not overlap",
                zone.top_note,
            ))
            .into());
        }
    }
    Ok(())
}

/// The ceiling the `map`'s own gain clamps at, in decibels. A project asking for more
/// renders at this and is not repaired.
pub const MAX_MAP_GAIN_DB: f64 = 9.0;

/// Largest zone gain whose stored values this reproduces. Past it the u24s' wrap count
/// is unmeasured; below it the wrap is the format's behavior, not a mistake.
pub const MAX_ZONE_GAIN: f64 = 1000.0;

/// The zone's playing gain in decibels: the value a wide stroke header stores, and the
/// value every other gain field is derived from.
///
/// The logarithm is evaluated wider than the field and rounded once; computing it in
/// float32 throughout changes the last byte at powers of two. It is neither clamped nor
/// rounded to a grid: silence is `-inf`, and a negative gain gives the default quiet
/// NaN, which then fails the map gain's ceiling comparison.
fn gain_decibels(gain: f64) -> f32 {
    let decibels = 20.0 * gain.log10();
    match decibels.is_nan() {
        true => f32::from_bits(0x7fc0_0000),
        false => decibels as f32,
    }
}

/// The gain back from its decibel, linear with
/// [`zone::GAIN_BITS`](super::zone::GAIN_BITS) fractional bits, exponentiated wider
/// than the decibel and rounded once.
///
/// ⚠️ This does not return the linear gain the decibel came from. Below `2^24` the
/// decibel's precision costs less than half a step and the two agree; above it they
/// differ by tens of steps, and statistic A is built from this value, not the
/// project's.
fn gain_units(decibels: f32) -> u64 {
    let units = 10f64.powf(f64::from(decibels) / 20.0) * f64::from(super::zone::GAIN_UNITY);
    units.round() as u64
}

/// The `map`'s own gain as the section's opening u24. The only gain field that clamps.
fn map_gain_units(gain: f64) -> u32 {
    let ceiling = MAX_MAP_GAIN_DB as f32;
    let decibels = gain_decibels(gain);
    // The ceiling is a comparison, not a min: a NaN decibel, from a negative gain,
    // fails it and takes the ceiling, not the floor.
    let clamped = if decibels < ceiling {
        decibels
    } else {
        ceiling
    };
    gain_units(clamped) as u32
}

/// A zone gain as the narrow zone record stores it: the project's own float, wrapping
/// mod `2^24`, with a negative gain converting to zero, not masked.
///
/// ⚠️ The record and statistic A diverge here. The record takes the project's float and
/// the mantissa takes the decibel round trip, so past a gain of 16 the two u24s in one
/// file disagree, and the record's reads back as a plausible quieter gain.
fn zone_record_gain(gain: f64) -> u32 {
    let units = (gain * f64::from(super::zone::GAIN_UNITY)).round() as u64;
    (units % (1 << 24)) as u32
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
        multi_zone(made(name, predictor, Layout::V2), zones).map(narrow)
    }

    /// An instrument at unity map gain, which is what all but one test wants.
    fn made(name: &str, predictor: Predictor, layout: Layout) -> Instrument<'_> {
        Instrument {
            name,
            map_gain: 1.0,
            predictor,
            layout,
            preset: Preset::default(),
        }
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

    // Landmarks read from Nord Sample Editor renders of self-generated audio whose
    // projects state the new-project default, `m_startSecondary = m_stop / 8`, from
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
        let looped = |at| Plan::looped(Layout::V2, 44_100, 1, Loop::new(8_192, 40_000), at);
        assert!(looped(8_193.0).is_err(), "past the loop start");
        assert!(looped(8_192.0).is_ok(), "at the loop start, mark pushed");
        assert!(looped(4_096.0).is_ok());
    }

    /// The mark's two anchors, on a loop far enough past the resync point and on ones
    /// that are not, at both channel counts.
    #[test]
    fn a_loop_mark_keeps_the_generations_minimum_gap_from_the_resync_point() {
        // Loop start and secondary start in frames, then the field the mark lands on
        // at V2, V3 and V4.
        for (start, secondary, channels, marks) in [
            (92, 92.0, 1, [145, 137, 137]),
            (200, 150.0, 1, [191, 183, 183]),
            (600, 500.0, 1, [481, 481, 481]),
            (92, 92.0, 2, [290, 274, 274]),
        ] {
            let points = Loop::new(start, start + 16_384);
            for (layout, mark) in [Layout::V2, Layout::V3, Layout::V4].into_iter().zip(marks) {
                let plan = Plan::looped(layout, 88_200, channels, points, secondary).unwrap();
                let looped = plan.looped.unwrap();
                assert_eq!(
                    looped.at, mark,
                    "{layout:?} {channels}ch: a loop at frame {start} resyncing at {secondary}"
                );
                assert_eq!(looped.lead, mark - fields_of(start).unwrap() * channels);
                assert_eq!(plan.fields, mark + fields_of(16_384).unwrap() * channels);
            }
        }
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
            widths_at(&values, 8, Predictor::Minimizing, CELL, 1)[3..],
            [MIN_WIDTH, MIN_WIDTH]
        );
        assert_eq!(widths_at(&values, 8, Predictor::Plain, CELL, 1).len(), 1);
    }

    fn length_error(result: Result<impl std::fmt::Debug, Error>) -> LengthError {
        match result {
            Err(Error::Parse(ParseError::Length(why))) => why,
            other => panic!("not a length refusal: {other:?}"),
        }
    }

    #[test]
    fn short_input_is_refused() {
        assert_eq!(
            length_error(plan(MIN_FRAMES - 1, 1)),
            LengthError::TooShort {
                frames: MIN_FRAMES - 1
            }
        );
        assert!(plan(MIN_FRAMES, 1).is_ok());
        assert!(instrument(&[0i16; MIN_FRAMES - 1], &Options::new("Test")).is_err());
        assert!(instrument(&[0i16; MIN_FRAMES], &Options::new("Test")).is_ok());
        assert_eq!(
            LengthError::TooShort { frames: 10 }.to_string(),
            "too short: 10 frames, and the encoder needs at least 92"
        );
    }

    /// The ceiling a refusal states is the last length the field count admits.
    #[test]
    fn a_refused_length_states_the_most_a_stroke_holds() {
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            for channels in [1, 2] {
                let max = Units { layout, channels }.max_frames();
                let plan = |frames| {
                    Plan::new(
                        layout,
                        frames,
                        channels,
                        default_secondary_start(frames, None),
                    )
                };
                assert!(plan(max).is_ok(), "{layout:?} x{channels} at {max}");
                assert_eq!(
                    length_error(plan(max + 1)),
                    LengthError::TooLong {
                        frames: max + 1,
                        channels,
                        max
                    },
                    "{layout:?} x{channels}"
                );
            }
        }
        assert!(matches!(
            length_error(plan(usize::MAX, 1)),
            LengthError::TooLong { .. }
        ));
        let mono = Units {
            layout: Layout::V2,
            channels: 1,
        }
        .max_frames();
        assert_eq!(
            length_error(plan(mono + 1, 1)).to_string(),
            format!(
                "too long: {} frames, and a mono stroke holds at most {mono} (about 22.5 s \
                 at 44.1 kHz)",
                mono + 1
            )
        );
    }

    /// Noise needs wide fields, so it overflows the stroke well under the ceiling, and
    /// the refusal estimates how much of it fits.
    #[test]
    fn audio_that_encodes_past_the_stroke_is_refused_with_what_fits() {
        let mut state = 1u32;
        let noise: Vec<i16> = (0..4 * codec::SOURCE_RATE as usize)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 16) as i16
            })
            .collect();
        let refused = length_error(instrument(&noise, &Options::new("Noise")));
        let LengthError::Stream { words, fits } = refused else {
            panic!("refused for the wrong reason: {refused:?}");
        };
        assert!(words > MAX_STREAM_WORDS, "{words} words");
        assert!((0.5..4.0).contains(&fits), "{fits} s would fit");
    }

    #[test]
    fn forced_shifts_that_cannot_be_encoded_are_refused() {
        let mut step = vec![i16::MIN; MIN_FRAMES];
        step[MIN_FRAMES / 2..].fill(i16::MAX);
        assert!(instrument(&step, &Options::new("Test").shift(0)).is_err());
        assert!(instrument(
            &[0i16; MIN_FRAMES],
            &Options::new("Test").shift(codec::SHIFT_LIMIT as u8 + 1)
        )
        .is_err());
    }

    #[test]
    fn midi_notes_outside_the_wire_range_are_refused() {
        let source = vec![0i16; MIN_FRAMES];
        assert!(instrument(&source, &Options::new("Test").root_key(128)).is_err());
        assert!(instrument(&source, &Options::new("Test").top_note(255)).is_err());
        let bad_root = NewZone {
            root_key: 128,
            ..zone(&source, 60, 127, 1)
        };
        let bad_root = ZoneSpec::from(&bad_root);
        let encoded = encode_stroke(Layout::V2, &bad_root, 165, Predictor::Plain).unwrap();
        assert!(stroke_payload(Layout::V2, &bad_root.placement, &encoded, 0, 1).is_err());
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
        let head = super::super::stroke::header_len(
            Layout::V2,
            super::super::Chain::Library2,
            0,
            cat_len,
            map_len,
        );
        assert_eq!((stroke.payload.len() - head) % PACKET_LEN, 0);
        assert_eq!(&stroke.payload[stroke.payload.len() - 3..], &[0x80, 0, 24]);
    }

    #[test]
    fn every_predictor_round_trips_through_the_decoder_exactly() {
        let mut differenced = 0usize;
        for predictor in [Predictor::Plain, Predictor::Minimizing] {
            for source in [
                sine(440.0, 12_000.0, 44_100),
                sine(30.0, 32_000.0, 20_000),
                vec![0i16; 8192],
                vec![9000i16; 8192],
            ] {
                let file = encoded(&source, predictor);
                let (at, stroke) = file.stroke_streams()[0];
                let plan = plan(source.len(), 1).unwrap();
                let q = quantize(&source, &plan, None);

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
        assert!(differenced > 0, "minimizing never chose a predictor");
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
            let at = codec::SEEK_AT + codec::SEEK_STRIDE * i;
            stroke[at..at + 2].copy_from_slice(&p.to_be_bytes());
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
            codec::Directory::resolve(directory.terminator, at, codec::Layout::V2),
            stream.terminator
        );
        let resync = codec::Directory::resolve(directory.resync, at, codec::Layout::V2);
        let record = stream.records.iter().find(|r| r.at == resync).unwrap();
        assert!(record.one_to_one);
        assert_eq!(record.first_field, plan(50_000, 1).unwrap().resync_at);
    }

    #[test]
    fn the_header_states_the_shift_it_quantized_at() {
        for amplitude in [40.0, 900.0, 8000.0, 32_000.0] {
            let source = sine(440.0, amplitude, 20_000);
            let plan = plan(source.len(), 1).unwrap();
            let file = encoded(&source, Predictor::Plain);
            let (_, stroke) = file.stroke_streams()[0];
            let q = quantize(&source, &plan, None);
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
            .map(|p| quantize(&sine(440.0, 500.0, 20_000), &p, None).shift)
            .unwrap();
        let loud = plan(20_000, 1)
            .map(|p| quantize(&sine(440.0, 32_000.0, 20_000), &p, None).shift)
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
        let mono = quantize(&left, &plan(frames, 1).unwrap(), None);
        let stereo = quantize(&both, &plan(frames, 2).unwrap(), None);
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

    /// A stroke whose only loud field sits at `field`, resynchronizing at `resync`.
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
    fn only_a_run_s_last_record_spends_the_extra_bit() {
        // The resync run at 5464 is 89 fields: [0, 32), [32, 64), [64, 89).
        let (plan, values) = probe(Layout::V2, 5464, 5464 + 76);
        assert!(spends_extra_bit(&values, &plan));
        for offset in [12, 61, 89, 95] {
            let (plan, values) = probe(Layout::V2, 5464, 5464 + offset);
            assert!(!spends_extra_bit(&values, &plan), "run offset {offset}");
        }
    }

    /// A mono stroke that is one 1:1 run ending in a `last`-field record, with `value`
    /// in that record's final field and nothing anywhere else.
    fn opening_run(layout: Layout, last: usize, value: i64) -> (Plan, Vec<i64>) {
        let chunk = layout.rmax();
        let warmup = if last == chunk { chunk } else { chunk + last };
        let plan = Plan {
            layout,
            channels: 1,
            fields: warmup,
            resync_at: warmup,
            warmup,
            resync: 0,
            cells_before: 0,
            cells_after: 0,
            looped: None,
        };
        let mut values = vec![0; warmup];
        values[warmup - 1] = value;
        (plan, values)
    }

    #[test]
    fn each_last_record_count_obeys_the_measured_rule() {
        for (last, spends) in [
            (24, false),
            (25, true),
            (26, true),
            (27, true),
            (28, true),
            (29, false),
            (30, true),
            (31, true),
            (32, false),
        ] {
            let (plan, values) = opening_run(Layout::V2, last, 1 << (PEAK_WIDTH - 2));
            assert_eq!(
                spends_extra_bit(&values, &plan),
                spends,
                "last record {last}"
            );
        }
    }

    #[test]
    fn the_extra_bit_uses_signed_thirteen_bit_bounds() {
        for (value, spends) in [(-4097, true), (-4096, false), (4095, false), (4096, true)] {
            let (plan, values) = opening_run(Layout::V2, 25, value);
            assert_eq!(spends_extra_bit(&values, &plan), spends, "value {value}");
        }
    }

    /// The last record of a v3 run is 32..=48 fields, and these eleven of the
    /// seventeen spend the bit.
    const V3_LIVE: [usize; 11] = [33, 34, 35, 36, 37, 38, 39, 40, 42, 44, 46];

    #[test]
    fn six_of_the_seventeen_v3_last_record_counts_never_spend_it() {
        for last in 32..=48 {
            let (plan, values) = opening_run(Layout::V3, last, 1 << (PEAK_WIDTH - 2));
            assert_eq!(
                spends_extra_bit(&values, &plan),
                V3_LIVE.contains(&last),
                "last record {last}"
            );
        }
    }

    #[test]
    fn a_v4_mono_stroke_never_spends_the_extra_bit() {
        for last in 32..=48 {
            let (plan, values) = opening_run(Layout::V4, last, 1 << (PEAK_WIDTH - 2));
            assert!(!spends_extra_bit(&values, &plan), "last record {last}");
        }
    }

    /// A looped mono stroke whose only loud field sits in the last record of the run
    /// the mark opens. The opening run is a full RMAX record, a count both generations
    /// list as dead, so only the loop's run can spend the bit.
    fn loop_run(layout: Layout, last: usize) -> (Plan, Vec<i64>) {
        let at = layout.rmax();
        let fields = at + last;
        let plan = Plan {
            layout,
            channels: 1,
            fields,
            resync_at: at,
            warmup: at,
            resync: 0,
            cells_before: 0,
            cells_after: 0,
            looped: Some(Looped {
                at,
                lead: 0,
                crossfade: 0,
                warmup: last,
                cells: 0,
            }),
        };
        let mut values = vec![0; fields];
        values[fields - 1] = 1 << (PEAK_WIDTH - 2);
        (plan, values)
    }

    #[test]
    fn the_run_a_loop_mark_opens_spends_the_extra_bit() {
        for (layout, live, dead) in [(Layout::V2, 25, 29), (Layout::V3, 33, 41)] {
            let (plan, values) = loop_run(layout, live);
            assert!(spends_extra_bit(&values, &plan), "{layout:?} live");

            let unmarked = Plan {
                looped: None,
                ..plan
            };
            assert!(!spends_extra_bit(&values, &unmarked), "{layout:?} unlooped");

            let (plan, values) = loop_run(layout, dead);
            assert!(!spends_extra_bit(&values, &plan), "{layout:?} dead");
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
            .predictor(Predictor::Minimizing);
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
        let positive = quantize(&up, &plan(frames, 1).unwrap(), None).peak;
        let negative = quantize(&down, &plan(frames, 1).unwrap(), None).peak;
        assert_eq!(positive, 2);
        assert_eq!(negative, 3);
        let opposed: Vec<i16> = up.iter().zip(&down).flat_map(|(&l, &r)| [l, r]).collect();
        let stereo = quantize(&opposed, &plan(frames, 2).unwrap(), None).peak;
        assert_eq!(stereo, positive);
    }

    #[test]
    fn no_field_overflows_the_width_its_record_declares() {
        for predictor in [Predictor::Plain, Predictor::Minimizing] {
            let source = sine(440.0, 32_000.0, 30_000);
            let plan = plan(source.len(), 1).unwrap();
            let q = quantize(&source, &plan, None);
            let (specs, _) = records(&q.values, &plan, predictor).unwrap();
            for spec in specs {
                let limit = 1i64 << (spec.width - 1);
                for k in 0..spec.count {
                    let v = residual(&q.values, spec.first + k, spec.order, 1);
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
        let q = quantize(&source, &plan, None);
        let (specs, _) = records(&q.values, &plan, Predictor::Plain).unwrap();

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
    fn the_minimizing_predictor_narrows_smooth_material() {
        let source = sine(60.0, 30_000.0, 60_000);
        let plan = plan(source.len(), 1).unwrap();
        let q = quantize(&source, &plan, None);
        let (plain, _) = records(&q.values, &plan, Predictor::Plain).unwrap();
        let (minimized, _) = records(&q.values, &plan, Predictor::Minimizing).unwrap();

        let bits = |specs: &[Spec]| -> usize { specs.iter().map(|s| s.span(MONO)).sum() };
        assert!(
            bits(&minimized) < bits(&plain),
            "{} words vs {}",
            bits(&minimized),
            bits(&plain)
        );
        assert!(minimized.iter().any(|s| s.order > 0));
        // The 1:1 regime never predicts.
        assert!(minimized.iter().all(|s| !s.one_to_one || s.order == 0));
    }

    #[test]
    fn statistic_a_round_trips_the_shift() {
        for peak in [0u32, 1, 2, 255, 4095, 4096, 8191, 8192] {
            for shift in 0..6 {
                let (mantissa, exponent) = statistic_a(peak, shift, u64::from(GAIN_UNITY)).unwrap();
                let mut stroke = vec![0u8; HEADER_LEN];
                stroke[codec::STAT_A_EXP_AT] = exponent;
                stroke[codec::PEAK_AT..codec::PEAK_AT + 3]
                    .copy_from_slice(&peak.to_be_bytes()[1..]);
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
            gain: 1.0,
            loop_decay: DEFAULT_LOOP_DECAY,
        }
    }

    #[test]
    fn statistic_a_scales_a_24_bit_reciprocal_by_the_gain() {
        let a = |peak, shift, gain| statistic_a(peak, shift, gain).unwrap();
        assert_eq!(a(4096, 2, u64::from(GAIN_UNITY)), (524_288, 12));
        assert_eq!(a(4096, 2, u64::from(GAIN_UNITY / 2)), (262_144, 12));
        assert_eq!(a(4096, 2, 2 * u64::from(GAIN_UNITY)), (1_048_576, 12));
        assert_eq!(a(1225, 0, 1_436_549), (1_200_837, 11));
        assert_eq!(a(4195, 2, 8_378_122), (8_180_401, 11));
        assert_eq!(a(1225, 0, 5_557_453), (4_645_576, 11));
    }

    #[test]
    fn statistic_a_reciprocates_the_loudest_zone_in_the_file() {
        let loud = sine(440.0, 12_000.0, 20_000);
        let quiet = sine(440.0, 3_000.0, 20_000);
        let file = built(
            &[zone(&loud, 72, 127, 1), zone(&quiet, 48, 71, 2)],
            "Two",
            Predictor::Plain,
        )
        .unwrap();
        let field = |s: &[u8], at: usize| u32::from_be_bytes([0, s[at], s[at + 1], s[at + 2]]);
        let streams = file.stroke_streams();
        let (mantissa, peak) = (|s| field(s, 9), |s| field(s, 13));
        let (first, second) = (streams[0].1, streams[1].1);
        assert!(peak(first) > peak(second));
        assert_eq!(mantissa(first), mantissa(second));
        assert_eq!(
            mantissa(second),
            statistic_a(peak(first), 0, u64::from(GAIN_UNITY))
                .unwrap()
                .0,
            "the quiet zone uses the loud zone's peak"
        );
        assert_ne!(
            mantissa(second),
            statistic_a(peak(second), 0, u64::from(GAIN_UNITY))
                .unwrap()
                .0
        );
    }

    #[test]
    fn a_zone_gain_scales_statistic_a_and_touches_nothing_else() {
        let source = sine(440.0, 12_000.0, 20_000);
        let unity = built(&[zone(&source, 60, 127, 1)], "Gain", Predictor::Plain).unwrap();
        let half = NewZone {
            gain: 0.5,
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
            let q = quantize(source, &plan, None);
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
            Predictor::default(),
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
                let head = super::super::stroke::header_len(
                    Layout::V2,
                    super::super::Chain::Library2,
                    index,
                    cat_len,
                    map_len,
                );
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
            (88_200, 92, 16_476),
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
            (43_981, 60_365),
            (4_096, 20_480),
            (65_536, 81_920),
        ] {
            let file = instrument(
                &source,
                &Options::new("Looped").loops(Loop::new(start, end)),
            )
            .unwrap_or_else(|e| panic!("loop {start}..{end}: {e}"));
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
            for predictor in [Predictor::Plain, Predictor::Minimizing] {
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
        let values = quantize(&source, &plan, None).values;
        assert_eq!(
            values[plan.fields - points.lead..],
            values[points.at - points.lead..points.at]
        );
    }

    #[test]
    fn a_loop_to_the_end_of_the_audio_plays_through_its_seam() {
        // 22,050 frames is a whole 17,501 fields, so the lattice closes on itself.
        let (start, length) = (4_410, 22_050);
        let frames = start + length;
        // Harmonics of 110 Hz to 3.3 kHz: 55 whole cycles of the fundamental per loop.
        let source: Vec<i16> = (0..frames)
            .map(|i| {
                let t = i as f64 / f64::from(codec::SOURCE_RATE);
                let sum: f64 = (1..=30)
                    .map(|n| {
                        let phase = std::f64::consts::PI * f64::from(n * n) / 30.0;
                        (std::f64::consts::TAU * 110.0 * f64::from(n) * t + phase).sin()
                            / f64::from(n)
                    })
                    .sum();
                (4_000.0 * sum) as i16
            })
            .collect();
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let options = Options::new("Seam")
                .layout(layout)
                .loops(Loop::new(start, frames));
            let file = instrument(&source, &options).unwrap();
            let (at, stroke) = file.stroke_streams()[0];
            let walk = codec::walk(stroke, at, layout).unwrap();
            let mark = walk.records.iter().find(|r| r.mark).unwrap().first_field;
            let decoded = codec::decode(stroke, at, layout).unwrap().samples;
            let end = decoded.len();
            // Two passes of the loop, as the instrument plays them.
            let played: Vec<i64> = decoded[mark..]
                .iter()
                .chain(&decoded[mark..])
                .map(|&v| i64::from(v))
                .collect();
            let bend = |i: usize| (played[i + 1] - 2 * played[i] + played[i - 1]).abs();
            let seam = end - mark;
            let body = (64..seam - 64).map(bend).max().unwrap();
            let (worst, place) = (seam - 64..seam + 64).map(|i| (bend(i), i)).max().unwrap();
            assert!(
                worst <= body + body / 8,
                "{layout:?}: second difference {worst} at {} fields from the loop end, \
                 against at most {body} across the loop",
                place as i64 - seam as i64,
            );
        }
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
            quantize(&source, &plan, None).values,
            quantize(&source, &faded, None).values,
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
        // Below the shortest stroke the editor encodes, whatever the loop says.
        assert!(looped(MIN_FRAMES - 1, 1, Loop::new(10, 60)).is_err());
    }

    #[test]
    fn a_looped_stroke_round_trips_through_the_decoder_exactly() {
        let source = sine(180.0, 16_000.0, 60_000);
        for predictor in [Predictor::Plain, Predictor::Minimizing] {
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
                let q = quantize(&source, &plan, None);
                let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
                assert_eq!(audio.samples.len(), plan.fields);
                let gain = 1i32 << q.shift;
                for (f, (&want, &got)) in q.values.iter().zip(&audio.samples).enumerate() {
                    assert_eq!(i32::from(got), want * gain, "{predictor:?} field {f}");
                }
            }
        }
    }

    /// Widening from the back would finish in fewer, wider records, which the editor
    /// does not write. The region's 1:1 run here spans two records, the marked one and
    /// a second.
    ///
    /// v3 and v4 lay the same mono region out in the same words, so their widths differ
    /// only by the cap: v3 stops one width below v4, and the remainder runs on into the
    /// next record.
    #[test]
    fn the_widen_fallback_walks_past_the_regions_alignment_records() {
        for (layout, widths) in [
            (Layout::V3, [1, 1, 13, 9, 1, 1]),
            (Layout::V4, [1, 1, 14, 8, 1, 1]),
        ] {
            let units = Units {
                layout,
                channels: 1,
            };
            let record = Spec {
                one_to_one: false,
                width: 1,
                order: 0,
                mark: false,
                first: 0,
                count: units.cell(),
            };
            let opening = Spec {
                one_to_one: true,
                ..record
            };
            let mut specs = vec![
                Spec {
                    mark: true,
                    ..opening
                },
                opening,
                record,
                record,
                record,
                record,
            ];
            pad_to_packet(&mut specs, 0, units).unwrap();
            assert_eq!(
                specs.iter().map(|s| s.width).collect::<Vec<_>>(),
                widths,
                "{layout:?}"
            );
            let words: usize = specs.iter().map(|s| s.span(units)).sum();
            assert_eq!(words % units.packet_words(), 0, "{layout:?}");
        }
    }

    /// Each region here opens with its alignment run and is two words short of a whole
    /// packet. v2 and v3 spend those two words one per record; v4 spends both on the
    /// first.
    #[test]
    fn the_widen_cap_is_the_generations_constant() {
        for (layout, alignment, content, spent) in [
            (Layout::V2, 2usize, 9usize, [13u8, 13]),
            (Layout::V3, 1, 2, [13, 13]),
            (Layout::V4, 1, 2, [14, 12]),
        ] {
            let units = Units {
                layout,
                channels: 1,
            };
            // Cell-sized, so the region has nothing left to split and must be widened.
            let record = Spec {
                one_to_one: false,
                width: 12,
                order: 0,
                mark: false,
                first: 0,
                count: units.cell(),
            };
            let mut specs: Vec<Spec> = (0..alignment)
                .map(|i| Spec {
                    one_to_one: true,
                    width: 3,
                    mark: i == 0,
                    ..record
                })
                .chain(std::iter::repeat_n(record, content))
                .collect();
            pad_to_packet(&mut specs, 0, units).unwrap();

            let mut want = vec![3u8; alignment];
            want.extend(spent);
            want.resize(alignment + content, record.width);
            assert_eq!(
                specs.iter().map(|s| s.width).collect::<Vec<_>>(),
                want,
                "{layout:?}"
            );
            let words: usize = specs.iter().map(|s| s.span(units)).sum();
            assert_eq!(words % units.packet_words(), 0, "{layout:?}");
        }
    }

    #[test]
    fn a_loop_that_needs_width_past_the_measured_cap_is_refused() {
        let units = Units {
            layout: Layout::V3,
            channels: 1,
        };
        let record = Spec {
            one_to_one: false,
            width: widen_cap(Layout::V3),
            order: 0,
            mark: false,
            first: 0,
            count: units.cell(),
        };
        let mut specs = vec![
            Spec {
                one_to_one: true,
                width: 1,
                mark: true,
                ..record
            },
            record,
            record,
        ];
        let before = specs.clone();
        assert!(pad_to_packet(&mut specs, 0, units).is_err());
        assert_eq!(specs, before);
    }

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

        // Full-scale broadband material can exhaust the three spare bits per field
        // before a short loop reaches the next packet boundary.
        let mut placed = 0usize;
        let mut refused = 0usize;
        for start in (4_096..48_000).step_by(7_919) {
            for length in [900, 1_500, 4_096, 11_000] {
                for predictor in [Predictor::Plain, Predictor::Minimizing] {
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
        assert!(placed > 0, "no loop was placed");
        assert!(refused > 0, "no loop was refused");
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
                mono.warmup + CELL * mono.cells_before,
                mono.resync_at,
                "{frames} frames: the resync does not follow the cells before it"
            );
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
        for predictor in [Predictor::Plain, Predictor::Minimizing] {
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
            let q = quantize(&source, &plan, None);
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
                .predictor(Predictor::Minimizing),
        )
        .unwrap();
        let (at, stroke) = file.stroke_streams()[0];
        let audio = codec::decode(stroke, at, codec::Layout::V2).unwrap();
        assert!(audio.differenced > 0, "nothing chose a predictor");

        let plan = plan(frames, 2).unwrap();
        let q = quantize(&source, &plan, None);
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
        let q = quantize(&source, &plan, None);
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

    /// Every generation, mono and stereo. v4 stereo is the case that packs each
    /// channel's half into its own words.
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
                        .predictor(Predictor::Minimizing),
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
                let q = quantize(&source, &plan, None);
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
            assert_eq!(read.layout().unwrap(), layout, "{layout:?}");
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
    /// the keyboard's floor. Records are stored high to low in every generation.
    #[test]
    fn a_wide_zone_states_its_own_bottom() {
        let high = sine(880.0, 12_000.0, 12_000);
        let low = sine(220.0, 12_000.0, 15_000);
        let floor = super::super::zone::KEY_FLOOR;
        let stored = [(96, 66), (65, floor)];
        for layout in [Layout::V3, Layout::V4] {
            let file = multi_zone(
                made("Two", Predictor::Plain, layout),
                &[zone(&high, 72, 96, 2), zone(&low, 48, 65, 1)],
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

    /// Decibel words read from editor renders of one project at sixteen stroke gains,
    /// four of them predicted before the render and matched by it.
    #[test]
    fn a_zone_gain_in_decibels_is_the_word_the_editor_writes() {
        for (gain, word) in [
            (-1.0, 0x7fc0_0000u32),
            (0.0, 0xff80_0000),
            (0.01, 0xc220_0000),
            (0.1, 0xc1a0_0000),
            (0.5, 0xc0c0_a8c1),
            (1.0, 0x0000_0000),
            (1.1, 0x3f53_ee38),
            (1.5, 0x4061_6595),
            (2.0, 0x40c0_a8c1),
            (4.0, 0x4140_a8c1),
            (8.0, 0x4190_7e91),
            (16.0, 0x41c0_a8c1),
            (20.5, 0x41d1_e170),
            (63.75, 0x4210_5bc1),
            (333.33, 0x4249_d478),
            (1000.0, 0x4270_0000),
        ] {
            assert_eq!(gain_decibels(gain).to_bits(), word, "a gain of {gain}");
        }
    }

    /// Statistic A's mantissa is built from the decibel, not the project's float. The
    /// two differ only past `2^24`, and these deltas are what the editor writes there.
    #[test]
    fn the_gain_statistic_a_uses_is_the_decibels_round_trip() {
        for (gain, delta) in [
            (0.01, 0i64),
            (1.1, 0),
            (15.99, 0),
            (16.0, -1),
            (20.5, -1),
            (24.0, 1),
            (33.0, 2),
            (48.0, -1),
            (63.75, -3),
            (100.0, 0),
            (333.33, 39),
            (1000.0, 0),
        ] {
            let plain = (gain * f64::from(GAIN_UNITY)).round() as i64;
            let round_trip = gain_units(gain_decibels(gain)) as i64;
            assert_eq!(round_trip - plain, delta, "a gain of {gain}");
        }
    }

    /// Fixed-point words read from editor renders of one project at a range of map
    /// gains. The ceiling is a clamp on the decibel: the knee sits at +9.000 dB, not at
    /// a round linear number, and a negative gain, whose decibel is NaN, fails the
    /// comparison and takes the ceiling, not the floor.
    #[test]
    fn a_map_gain_is_the_word_the_editor_writes_and_clamps_at_the_ceiling() {
        for (gain, units) in [
            (-1.0, 0x2d_18_19_u32),
            (0.0, 0x00_00_00),
            (0.0001, 0x00_00_69),
            (0.01, 0x00_28_f6),
            (0.5, 0x08_00_00),
            (1.0, 0x10_00_00),
            (1.1, 0x11_99_9a),
            (2.0, 0x20_00_00),
            (2.8125, 0x2d_00_00),
            (2.828125, 0x2d_18_19),
            (4.0, 0x2d_18_19),
            (16.0, 0x2d_18_19),
        ] {
            assert_eq!(map_gain_units(gain), units, "a map gain of {gain}");
        }
    }

    /// A zone gain past 16 overflows both u24 stores, by different rules: the record
    /// takes the project's float and wraps, and the mantissa takes the decibel's round
    /// trip and truncates into its field. Words read from editor renders.
    #[test]
    fn a_zone_gain_past_sixteen_wraps_in_both_stores() {
        for (gain, record) in [
            (-1.0, 0x00_00_00_u32),
            (0.0, 0x00_00_00),
            (15.99, 0xff_d7_0a),
            (16.0, 0x00_00_00),
            (33.0, 0x10_00_00),
            (333.33, 0xd5_47_ae),
            (1000.0, 0x80_00_00),
        ] {
            assert_eq!(zone_record_gain(gain), record, "a gain of {gain}");
        }
        // `WG-base`'s peak is 4096, so the reciprocal is 2^22 and every step below is
        // exact in integers.
        for (gain, mantissa) in [
            (-1.0, 0x00_00_00_u32),
            (0.0, 0x00_00_00),
            (15.99, 0x7f_eb_85),
            (16.0, 0x7f_ff_ff),
            (33.0, 0x08_00_01),
            (333.33, 0x6a_a3_ea),
            (1000.0, 0x40_00_00),
        ] {
            let (got, _) = statistic_a(4096, 0, gain_units(gain_decibels(gain))).unwrap();
            assert_eq!(got, mantissa, "a gain of {gain}");
        }
    }

    /// The map gain opens the `map` section and reaches nothing else: not the zone
    /// records, statistic A, or any stream byte.
    #[test]
    fn a_map_gain_moves_the_map_section_alone() {
        let source = sine(440.0, 12_000.0, 20_000);
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let unity = made("Map", Predictor::Plain, layout);
            let quiet = Instrument {
                map_gain: 0.5,
                ..unity
            };
            let one = [zone(&source, 60, 127, 1)];
            let before = multi_zone(unity, &one).unwrap().to_bytes().unwrap();
            let after = multi_zone(quiet, &one).unwrap().to_bytes().unwrap();
            assert_eq!(before.len(), after.len(), "{layout:?}");
            let moved: Vec<_> = (0..before.len())
                .filter(|&i| before[i] != after[i])
                .collect();
            // The gain's own top byte (0x10 against 0x08) and the container checksum.
            assert!(moved.len() <= 1 + 4, "{layout:?}: {moved:?}");
        }
    }

    #[test]
    fn a_project_preset_reaches_each_generation_in_its_own_schema() {
        let source = sine(440.0, 12_000.0, 20_000);
        let preset = Preset {
            dynamics_enabled: true,
            velocity_to_amplitude: 2,
            velocity_to_timbre: 0,
        };
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let instrument = Instrument {
                preset,
                ..made("Preset", Predictor::Plain, layout)
            };
            let sample = multi_zone(instrument, &[zone(&source, 60, 127, 1)]).unwrap();
            match sample {
                crate::Sample::V2(file) => {
                    let sty = section::find(&file.body.sections, section::STY).unwrap();
                    assert_eq!(sty.payload, [0, 1, 0, 1, 2, 0, 0, 0, 0]);
                }
                crate::Sample::V3(file) => {
                    let sty = section::find(&file.body.sections, section::STY4).unwrap();
                    match layout {
                        Layout::V3 => {
                            assert_eq!((sty.payload[4], sty.payload[12]), (43, 74));
                            assert_eq!((sty.payload[14], sty.payload[16]), (1, 74));
                        }
                        Layout::V4 => {
                            assert_eq!((sty.payload[3], sty.payload[4]), (1, 1));
                            assert_eq!(sty.payload[85..88], [74, 82, 90]);
                        }
                        Layout::V2 => unreachable!(),
                    }
                }
            }
        }
    }

    /// A wide zone gain reaches the stroke header's decibel field and statistic A, and
    /// nothing else: no byte of the 16-byte zone record moves with it.
    #[test]
    fn a_wide_zone_gain_lands_in_the_stroke_header() {
        let source = sine(440.0, 12_000.0, 20_000);
        for layout in [Layout::V3, Layout::V4] {
            let one = zone(&source, 60, 127, 1);
            let made = made("Gain", Predictor::Plain, layout);
            let unity = multi_zone(made, &[one]).unwrap();
            let halved = multi_zone(made, &[NewZone { gain: 0.5, ..one }]).unwrap();
            let (_, a) = unity.stroke_streams()[0];
            let (_, b) = halved.stroke_streams()[0];
            let mantissa = |s: &[u8]| u32::from_be_bytes([0, s[9], s[10], s[11]]);
            assert_eq!(mantissa(b), mantissa(a) / 2, "{layout:?}");
            let gain_at = codec::TAIL_FLOATS_AT[0];
            assert_eq!(a[..9], b[..9], "{layout:?}");
            assert_eq!(a[12..gain_at], b[12..gain_at], "{layout:?}");
            assert_eq!(a[gain_at + 4..], b[gain_at + 4..], "{layout:?}");
            assert_eq!(
                codec::zone_gain_db(b, layout),
                Some(gain_decibels(0.5)),
                "{layout:?}"
            );
            // A zone gain moves only statistic A's mantissa, the decibel word and the
            // container checksum; the zone record does not change.
            let (before, after) = (unity.to_bytes().unwrap(), halved.to_bytes().unwrap());
            let differing = before.iter().zip(&after).filter(|(x, y)| x != y).count();
            assert_eq!(before.len(), after.len(), "{layout:?}");
            assert!(differing <= 3 + 4 + 4, "{layout:?}: {differing} bytes");
        }
    }

    #[test]
    fn a_loop_decay_lands_in_the_wide_header_and_nowhere_narrow() {
        let source = sine(440.0, 12_000.0, 20_000);
        let at = codec::TAIL_FLOATS_AT[1];
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let one = zone(&source, 60, 127, 1);
            let made = made("Decay", Predictor::Plain, layout);
            let base = multi_zone(made, &[one]).unwrap();
            let slower = multi_zone(
                made,
                &[NewZone {
                    loop_decay: 60.0,
                    ..one
                }],
            )
            .unwrap();
            let (_, a) = base.stroke_streams()[0];
            let (_, b) = slower.stroke_streams()[0];
            let wide = layout != Layout::V2;
            assert_eq!(
                codec::loop_decay(a, layout),
                wide.then_some(DEFAULT_LOOP_DECAY),
                "{layout:?}"
            );
            assert_eq!(
                codec::loop_decay(b, layout),
                wide.then_some(60.0),
                "{layout:?}"
            );
            match wide {
                false => assert_eq!(a, b),
                true => {
                    assert_eq!(a[..at], b[..at], "{layout:?}");
                    assert_eq!(a[at + 4..], b[at + 4..], "{layout:?}");
                }
            }
        }
    }

    #[test]
    fn a_zone_gain_past_the_measured_range_is_refused() {
        let source = sine(440.0, 12_000.0, 20_000);
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            for gain in [MAX_ZONE_GAIN * 2.0, f64::NAN, f64::INFINITY] {
                let loud = NewZone {
                    gain,
                    ..zone(&source, 60, 127, 1)
                };
                assert!(
                    multi_zone(made("Gain", Predictor::Plain, layout), &[loud]).is_err(),
                    "{layout:?} at {gain}"
                );
            }
            let wrapping = NewZone {
                gain: MAX_ZONE_GAIN,
                ..zone(&source, 60, 127, 1)
            };
            assert!(multi_zone(made("Gain", Predictor::Plain, layout), &[wrapping]).is_ok());
        }
    }

    /// The wide terminator states 32 fields per channel where the narrow one states 24,
    /// and a 1:1 record reaches 48 fields per channel, not 32.
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

    /// A wide stroke stores statistic B as a signed extreme, so it reads back through
    /// the codec's sign rule, not as a 24-bit magnitude.
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

    /// The lattice view of a one-zone encode, and the zone placement it was built
    /// with.
    fn relaid(file: &crate::Sample, layout: Layout) -> Result<crate::Sample, Error> {
        let (at, stroke) = file.stroke_streams()[0];
        let peak = codec::peak(stroke, file.layout().unwrap())
            .unwrap()
            .unsigned_abs();
        let lattice = codec::lattice(stroke, at, file.layout().unwrap(), peak).unwrap();
        let zone = &file.zones().unwrap()[0];
        from_lattice(
            &lattice_instrument(layout),
            &[lattice_zone(&lattice, zone.root_key, zone.top_note)],
        )
    }

    fn lattice_instrument(layout: Layout) -> LatticeInstrument<'static> {
        let mut keys = KeyTable::NEUTRAL;
        keys.instrument = Level::new(map_gain_units(1.0), 0).unwrap();
        LatticeInstrument {
            aux: aux(&NarrowCat::editor_default()),
            name: "Test",
            sub_name: "",
            categories: NarrowCat::editor_default(),
            keys,
            layout,
            preset: Preset::default(),
        }
    }

    fn lattice_zone(audio: &codec::Lattice, root_key: u8, top_note: u8) -> LatticeZone<'_> {
        LatticeZone {
            audio,
            root_key,
            top_note,
            low_note: None,
            global_id: 1,
            gain: 1.0,
            loop_decay: DEFAULT_LOOP_DECAY,
            rel_strength: super::super::zone::REL_STRENGTH_DEFAULT,
            velocity: VelocityWindow::FULL,
        }
    }

    /// Sources whose encodes differ in shape: mono, stereo, quiet, loud enough to
    /// spend the extra bit, and looped well clear of the resync point.
    fn shapes() -> Vec<(&'static str, Options, Vec<i16>)> {
        let stereo: Vec<i16> = sine(330.0, 9_000.0, 30_000)
            .into_iter()
            .zip(sine(550.0, 4_000.0, 30_000))
            .flat_map(|(l, r)| [l, r])
            .collect();
        vec![
            ("quiet", Options::new("Test"), sine(440.0, 3_000.0, 20_000)),
            ("loud", Options::new("Test"), sine(440.0, 16_000.0, 20_000)),
            ("stereo", Options::new("Test").channels(2), stereo),
            (
                "looped",
                Options::new("Test").loops(Loop::new(12_000, 28_000).crossfade(800.0)),
                sine(220.0, 12_000.0, 30_000),
            ),
        ]
    }

    #[test]
    fn a_stream_laid_out_again_in_its_own_generation_is_the_same_file() {
        for layout in Layout::ALL {
            for (shape, options, source) in shapes() {
                let file = instrument(&source, &options.clone().layout(layout)).unwrap();
                assert_eq!(
                    relaid(&file, layout).unwrap().to_bytes().unwrap(),
                    file.to_bytes().unwrap(),
                    "{shape} at {layout:?}"
                );
            }
        }
    }

    /// The twin law on this encoder's own renders: every generation quantizes the
    /// same fields, so the finer render laid out in the coarser generation is that
    /// generation's render.
    #[test]
    fn a_finer_render_laid_out_in_a_coarser_generation_is_that_generations_render() {
        for (shape, options, source) in shapes() {
            let render = |layout| instrument(&source, &options.clone().layout(layout)).unwrap();
            let shift = |file: &crate::Sample| {
                let (_, stroke) = file.stroke_streams()[0];
                codec::shift(stroke, file.layout().unwrap()).unwrap()
            };
            for from in Layout::ALL {
                for to in Layout::ALL {
                    let (finer, coarser) = (render(from), render(to));
                    if shift(&finer) > shift(&coarser) {
                        continue;
                    }
                    assert_eq!(
                        relaid(&finer, to).unwrap().to_bytes().unwrap(),
                        coarser.to_bytes().unwrap(),
                        "{shape}: {from:?} laid out as {to:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_coarser_render_never_gains_back_the_bit_it_dropped() {
        let loud = sine(440.0, 16_000.0, 20_000);
        let v2 = instrument(&loud, &Options::new("Test")).unwrap();
        let v4 = instrument(&loud, &Options::new("Test").layout(Layout::V4)).unwrap();
        let shift = |file: &crate::Sample| {
            let (_, stroke) = file.stroke_streams()[0];
            codec::shift(stroke, file.layout().unwrap()).unwrap()
        };
        assert!(
            shift(&v2) > shift(&v4),
            "the loud tone spends the v2 extra bit"
        );
        assert_eq!(shift(&relaid(&v2, Layout::V4).unwrap()), shift(&v2));
    }

    #[test]
    fn a_loop_mark_inside_the_generations_minimum_gap_is_refused() {
        for (layout, gap) in [(Layout::V2, 72), (Layout::V3, 64), (Layout::V4, 64)] {
            let fields = 4_000;
            let resync_at = 400;
            assert!(Plan::on_lattice(layout, fields, 1, resync_at, Some(resync_at + gap)).is_ok());
            let near = Plan::on_lattice(layout, fields, 1, resync_at, Some(resync_at + gap - 1));
            assert!(near.is_err(), "{layout:?}");
            assert!(Plan::on_lattice(layout, fields, 2, resync_at, Some(resync_at + gap)).is_err());
            for mark in [fields, fields + 1] {
                let past = Plan::on_lattice(layout, fields, 1, resync_at, Some(mark));
                let error = past.unwrap_err().to_string();
                assert!(
                    error.contains("before the stream ends"),
                    "{layout:?}: {error}"
                );
            }
        }
    }

    /// A loop too short for the editor's padding plays the same over more periods.
    #[test]
    fn a_loop_too_short_to_fill_whole_packets_is_laid_out_over_more_periods() {
        let fields: Vec<i32> = (0..3_000).map(|f| (f * 37) % 401 - 200).collect();
        let mark = 2_880;
        let lattice = codec::Lattice {
            fields: fields.clone(),
            channels: 1,
            shift: 0,
            peak: codec::Peak::Signed(-50),
            resync_at: 400,
            mark: Some(mark),
        };
        for layout in Layout::ALL {
            let file = from_lattice(
                &lattice_instrument(layout),
                &[lattice_zone(&lattice, 60, 84)],
            )
            .unwrap();
            let (at, stroke) = file.stroke_streams()[0];
            let out = codec::lattice(stroke, at, layout, 50).unwrap();
            assert_eq!(out.mark, Some(mark), "{layout:?}");
            assert_eq!(out.fields[..fields.len()], fields[..], "{layout:?}");
            let period = fields.len() - mark;
            assert!(
                out.fields.len() > fields.len(),
                "{layout:?} needs more periods"
            );
            assert_eq!((out.fields.len() - mark) % period, 0, "{layout:?}");
            for (f, &value) in out.fields.iter().enumerate().skip(fields.len()) {
                assert_eq!(value, out.fields[f - period], "{layout:?} field {f}");
            }
        }
    }

    #[test]
    fn a_negative_shift_carries_over() {
        let lattice = codec::Lattice {
            fields: (0..3_000).map(|f| (f * 13) % 301 - 150).collect(),
            channels: 1,
            shift: -8,
            peak: codec::Peak::Signed(1),
            resync_at: 400,
            mark: None,
        };
        let file = from_lattice(
            &lattice_instrument(Layout::V3),
            &[lattice_zone(&lattice, 60, 84)],
        )
        .unwrap();
        let (at, stroke) = file.stroke_streams()[0];
        let out = codec::lattice(stroke, at, Layout::V3, 1).unwrap();
        assert_eq!((out.shift, out.fields), (lattice.shift, lattice.fields));
    }

    #[test]
    fn a_lattice_instrument_refuses_what_its_generation_cannot_hold() {
        let lattice = codec::lattice(
            instrument(&sine(440.0, 3_000.0, 20_000), &Options::new("Test"))
                .unwrap()
                .stroke_streams()[0]
                .1,
            0,
            Layout::V2,
            0,
        );
        assert!(
            lattice.is_err(),
            "a stroke read at the wrong offset does not decode"
        );
        let file = instrument(&sine(440.0, 3_000.0, 20_000), &Options::new("Test")).unwrap();
        let (at, stroke) = file.stroke_streams()[0];
        let peak = codec::peak(stroke, Layout::V2).unwrap().unsigned_abs();
        let lattice = codec::lattice(stroke, at, Layout::V2, peak).unwrap();
        let zones = [lattice_zone(&lattice, 60, 84)];
        let named = LatticeInstrument {
            sub_name: "Sub",
            ..lattice_instrument(Layout::V2)
        };
        assert!(from_lattice(&named, &zones).is_err());
        let mut keys = KeyTable::NEUTRAL;
        keys.set_key(60, Level::new(GAIN_UNITY / 2, 0).unwrap())
            .unwrap();
        let keyed = LatticeInstrument {
            keys,
            ..lattice_instrument(Layout::V4)
        };
        assert!(from_lattice(&keyed, &zones).is_err());
        let filed = LatticeInstrument {
            categories: NarrowCat {
                timbre: 3,
                ..NarrowCat::editor_default()
            },
            ..lattice_instrument(Layout::V3)
        };
        assert!(from_lattice(&filed, &zones).is_err());
        assert!(from_lattice(&named, &zones).is_err());
        assert!(from_lattice(&lattice_instrument(Layout::V2), &zones).is_ok());
    }
}
