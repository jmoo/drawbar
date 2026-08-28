//! The stroke codec: one zone's encoded audio back into samples.
//!
//! A stroke payload is a fixed header followed by a stream of words. The words carry
//! *fields* — the source resampled onto a uniform lattice of [`PITCH_DEN`] fields per
//! [`PITCH_NUM`] input samples — quantised by truncation and one arithmetic shift the
//! header records. Decoding is a walk, that shift, and one integration: a record may
//! store the Nth backward difference of its fields rather than the fields themselves.
//!
//! Three generations share this one codec, and every entry point takes the [`Layout`]
//! saying which. Only *units* differ — word width, cell size, header size — so a caller
//! that gets the layout right gets the same decoder in all three; the lattice, the
//! kernel, the quantiser and the grammar's bit layout do not move.
//!
//! The lattice is absolute, so field 0 is the start of the source and the stream's
//! own length gives the duration. The resampling kernel measures unity gain at every
//! phase, which is why a field is already a sample in the source's own 16-bit units
//! and dequantising is a shift and nothing more. The kernel's exact taps are not
//! known, so this reconstructs the signal; it does not claim the encoder's residues.
//!
//! ⚠️ **The slack in front of a stream can hold stale words that look like records**,
//! so where the chain begins comes from the header's [`Directory`] rather than from
//! the first non-zero word. That is why a walk needs the stroke's offset in the body.
//!
//! A record may store the Nth backward difference of its fields rather than the
//! fields themselves, so [`decode`] runs a predictor: `V(f) = e(f) − Σ(−1)^j
//! C(N,j)·V(f−j)`, over one running history carried across every record boundary
//! and through every skip. Nothing needs seeding — a stroke opens with a 1:1 ramp-in
//! that settles on the content's own field value, and the history takes it from
//! there.
//!
//! ⚠️ **A record's fields are left-anchored**: they start at the first bit after the
//! header word, and the alignment tail is at the *end* of the segment. Reading from
//! the far end instead is invisible on content records, whose field counts leave no
//! tail, and displaces every 1:1 record — the warmup and the resyncs — by a whole
//! number of field slots, or rotates the values inside their width when the tail is
//! not a multiple of it.
//!
//! Everything here is inferred from specimens; not confirmed on hardware.

use std::fmt;

/// Which generation's units a stroke stream is in.
///
/// `.nsmp3` and `.nsmp4` share one set of units, so [`Layout::V3`] covers both; the
/// only thing that separates the two extensions is that v4 sometimes quantises one bit
/// finer, and the stroke header states that shift either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `.nsmp`: 3-byte words, 24-field cells, a 51-byte stroke header.
    V2,
    /// `.nsmp3` and `.nsmp4`: 4-byte words, 32-field cells, a 68-byte stroke header.
    V3,
}

impl Layout {
    /// The layout a body's content version implies. The u32 at `0x14` runs
    /// `format × 100 + revision`, so anything from 300 up is the wide chain.
    pub fn from_version(version: u32) -> Layout {
        if version >= super::V3_FROM_VERSION {
            Layout::V3
        } else {
            Layout::V2
        }
    }

    /// Bytes per stream word. A record header is exactly one word, and the top byte
    /// of a [`Layout::V3`] word has never been anything but zero.
    pub const fn word(self) -> usize {
        match self {
            Layout::V2 => 3,
            Layout::V3 => 4,
        }
    }

    /// Bytes of fixed stroke header ahead of the word stream.
    ///
    /// The wide header is the narrow one field for field — same statistics, same
    /// directory — plus two float32s and the room to hold them.
    pub const fn header_len(self) -> usize {
        match self {
            Layout::V2 => 51,
            Layout::V3 => 68,
        }
    }

    /// Fields per cell. A content record covers whole cells, so its count is a
    /// multiple of this; the terminator's own count states it.
    ///
    /// ⚠️ A stereo [`Layout::V2`] stroke cells at 48, which is a multiple of 24, so
    /// this is the divisor to check against rather than the cell size to assume.
    pub const fn cell(self) -> usize {
        match self {
            Layout::V2 => 24,
            Layout::V3 => 32,
        }
    }

    const fn word_bits(self) -> usize {
        self.word() * 8
    }
}

/// Statistic A's exponent byte, at this offset in the stroke payload.
///
/// Statistic A is the reciprocal of the content peak, carried as a normalised
/// binary float whose mantissa sits at `+9..11`. The exponent is the only place the
/// quantiser shift is written down — see [`shift`].
const STAT_A_EXP_AT: usize = 12;

/// Statistic B, the content peak: 24 bits big-endian at this offset.
///
/// The largest coded field before the shift, with the frame-0 marker excluded.
const PEAK_AT: usize = 13;

/// Where the wide stroke header's two float32s sit. Both big-endian.
const TAIL_FLOATS_AT: [usize; 2] = [57, 62];

/// What statistic A's exponent is offset by. `A = 2^(41+s) / PEAK` normalised to a
/// 20-bit mantissa, so the exponent lands `22 − bits(PEAK) + s` above zero.
const EXPONENT_BIAS: i32 = 22;

/// Shifts beyond this are not a scale, they are a misread header.
const SHIFT_LIMIT: i32 = 32;

/// Word directory: `u16` big-endian at this offset, on a 9-byte stride.
const SEEK_AT: usize = 20;
const SEEK_STRIDE: usize = 9;

/// Where the directory's 16-bit pointers roll over, in words.
///
/// ⚠️ A stroke longer than this could not be addressed unambiguously; none is, and
/// the allocation would have to reach 192 KiB for one to be.
pub const WRAP: usize = 1 << 16;

/// Input samples per [`PITCH_DEN`] fields. Exact, not an approximation.
pub const PITCH_NUM: u32 = 349;
/// Fields per [`PITCH_NUM`] input samples.
pub const PITCH_DEN: u32 = 277;

/// Rate the editor resamples every import to before encoding. Neither the source
/// rate nor its bit depth survives anywhere in the file.
pub const SOURCE_RATE: u32 = 44_100;

/// Field rate in Hz: `44100 × 277/349` is 35002.006, rounded to the nearest integer
/// because a WAV header holds no fraction.
pub const FIELD_RATE: u32 = (SOURCE_RATE * PITCH_DEN + PITCH_NUM / 2) / PITCH_NUM;

/// Bits of a record header's field count: `[flag:1][width−1:4][00][order:3][count:14]`.
///
/// ⚠️ The count runs well past a byte — a merged content run reaches this field's own
/// ceiling of 682 whole cells. Reading it as eight bits frames short records
/// correctly and then derails on the first long one, which is what makes dense
/// material look like a stream with no records in it.
const COUNT_MASK: u32 = 0x3fff;

/// Field values the predictor keeps. The order field is three bits wide, but only
/// 0 to 4 occur and a fourth-order difference reaches no further back than this.
const MAX_ORDER: usize = 4;

/// Why a stroke's stream could not be walked.
///
/// Decode coverage is a number, not a hope: a stroke either decodes or says which of
/// these it hit, so a sweep over a library can be counted by [`reason`].
///
/// [`reason`]: Unsupported::reason
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// Shorter than the fixed header, so there is no stream to walk.
    Short,
    /// A word that is not a record header: the reserved bit or a wide word's top
    /// byte set, no fields at all, or a content run that is not a whole number of
    /// cells.
    Malformed {
        /// Word index within the stream, counting from [`HEADER_LEN`].
        word: usize,
    },
    /// A record whose fields run past the end of the stroke — some earlier record
    /// was read at the wrong size.
    Desync {
        /// Word index within the stream, counting from [`HEADER_LEN`].
        word: usize,
    },
    /// The chain reached the end of the stroke without a terminator.
    NoTerminator,
    /// A stereo stroke: two streams under one header, which this does not separate.
    ///
    /// The terminator gives the cell size away — 48 fields where a mono stroke has
    /// 24. Decoding it as one stream interleaves the channels and the predictor
    /// runs away, so it is refused instead.
    Stereo,
}

impl Unsupported {
    /// A stable label, for tallying coverage across a whole library.
    pub fn reason(self) -> &'static str {
        match self {
            Unsupported::Short => "short-stroke",
            Unsupported::Malformed { .. } => "malformed-record",
            Unsupported::Desync { .. } => "desync",
            Unsupported::NoTerminator => "no-terminator",
            Unsupported::Stereo => "stereo",
        }
    }
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unsupported::Short => write!(f, "the stroke is shorter than its own header"),
            Unsupported::Malformed { word } => {
                write!(f, "word {word} is not a record header")
            }
            Unsupported::Desync { word } => write!(
                f,
                "the record at word {word} runs past the end of the stroke"
            ),
            Unsupported::NoTerminator => {
                write!(f, "the chain ran off the end with no terminator")
            }
            Unsupported::Stereo => write!(
                f,
                "a stereo stroke carries two streams under one header, which this \
                 decoder does not separate"
            ),
        }
    }
}

impl std::error::Error for Unsupported {}

/// One record, placed on the field lattice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Word index within the stream, counting from [`HEADER_LEN`].
    pub at: usize,
    /// Lattice index of this record's first field.
    pub first_field: usize,
    /// `false` for lattice content; `true` for the 1:1 regime — the warmup and the
    /// resync. Both sit on the same lattice.
    pub one_to_one: bool,
    /// Bits per field, 1 to 16.
    pub width: u8,
    /// Differencing order, 0 to 4. A record of order N stores the Nth backward
    /// difference of its field values, `e(f) = Σ(−1)^j C(N,j)·V(f−j)`, so reading
    /// it takes N integrations. Smooth material codes at 1 to 3; every impulse and
    /// noise probe codes at 0.
    ///
    /// ⚠️ It is not layout — it changes neither the record's length nor its field
    /// base — and it is only ever set on content records.
    pub order: u8,
    /// A flag one record per stroke carries, on vendor library content only.
    ///
    /// ⚠️ **A parser must carry it rather than refuse it.** Every stroke the editor
    /// writes leaves it clear, in all three generations, so a corpus of fresh renders
    /// never shows it and reading its bit as reserved rejects the entire vendor
    /// library instead. Where it is set it is set exactly once, always on a
    /// [`Record::one_to_one`] record, and usually on the one the header directory's
    /// resync pointer names. What it means is unknown.
    pub mark: bool,
    /// Stored fields, sign-extended. What they mean depends on [`Record::order`]:
    /// at 0 they are the field values, otherwise their Nth difference. Dequantise
    /// by shifting left by [`shift`].
    ///
    /// ⚠️ A width-2 record is the noise floor the encoder decided not to code — it
    /// planted the header over its own working draft and left the data behind. The
    /// values are real, at two bits; they are not a marker to be skipped.
    pub values: Vec<i32>,
}

/// A walked record chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub records: Vec<Record>,
    /// Fields the chain covers, which is the decoded length.
    pub fields: usize,
    /// Word index the chain started at.
    pub first_record: usize,
    /// Word index where the chain stops.
    pub terminator: usize,
    /// Fields per cell, read off the terminator word: 24 for a mono [`Layout::V2`]
    /// stroke, 48 for a stereo one, 32 for [`Layout::V3`]. `None` when the stream
    /// ends at the record the header's directory names instead of at a width-1
    /// terminator, which is how some library content ends.
    pub cell: Option<usize>,
}

/// Decoded audio for one zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audio {
    /// One sample per field, at [`FIELD_RATE`].
    pub samples: Vec<i16>,
    /// Fields whose dequantised value ran past 16 bits and was clamped.
    ///
    /// Nonzero on transients: the kernel rings, so a full-scale edge overshoots what
    /// a 16-bit source could hold.
    pub clipped: usize,
    /// Fields that came through the predictor rather than being stated outright.
    /// Real instrument content is nearly all of this; sparse test material is none
    /// of it. Both kinds carry a level — this is a description, not a caveat.
    pub differenced: usize,
}

impl Audio {
    /// Duration in seconds.
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / f64::from(FIELD_RATE)
    }
}

/// The content peak the stroke header records, or `None` if it is too short.
///
/// ⚠️ **[`Layout::V3`] signs it.** The wide generations store the extreme field with
/// its sign and start the accumulator at −1, so a silent stroke reads `-1` where a
/// [`Layout::V2`] one reads `0`. The magnitude is the same quantity in both, and the
/// magnitude is what the quantiser scales against.
pub fn peak(stroke: &[u8], layout: Layout) -> Option<i32> {
    let b = stroke.get(PEAK_AT..PEAK_AT + 3)?;
    let v = u32::from_be_bytes([0, b[0], b[1], b[2]]);
    Some(match layout {
        Layout::V2 => v as i32,
        Layout::V3 if v >= 1 << 23 => v as i32 - (1 << 24),
        Layout::V3 => v as i32,
    })
}

/// The stroke's quantiser shift, read out of the header.
///
/// Fields are stored shifted right by this much, so dequantising shifts back by it.
/// It counts single bits, so odd values occur, and it is **signed** — content
/// sitting below the source's 16-bit LSB is shifted *left* by the encoder, which
/// means dequantising it shifts right.
///
/// ⚠️ Read it here, not from the field width. The width saturates at 13 and stops
/// discriminating above that; this byte does not.
///
/// ⚠️ The exponent alone is not the shift: it is the shift biased by how many bits
/// the peak statistic A was normalised against occupies, so recovering one needs the
/// other. On instruments the editor wrote in one pass that peak is [`peak`], and this
/// is exact. **On vendor library content statistic A's mantissa does not reproduce
/// from [`peak`]**, so the encoder normalised against something else there and this
/// can read low by a few bits — decoded vendor audio is right in shape and can be
/// quiet by a power of two.
pub fn shift(stroke: &[u8], layout: Layout) -> Option<i32> {
    let peak = peak(stroke, layout)?.unsigned_abs().max(1);
    let exponent = i32::from(*stroke.get(STAT_A_EXP_AT)?);
    let bits = peak.ilog2() as i32 + 1;
    let exact_power = i32::from(peak.is_power_of_two());
    Some(exponent + bits - EXPONENT_BIAS - exact_power)
}

/// The two float32s a [`Layout::V3`] stroke header carries past the directory,
/// `None` on [`Layout::V2`], which has neither.
///
/// What they mean is unknown. Every stroke the editor wrote holds `(0.0, 20.0)`;
/// vendor library strokes vary in both, so they are content-dependent rather than
/// constants. Reported, not interpreted.
pub fn tail_floats(stroke: &[u8], layout: Layout) -> Option<[f32; 2]> {
    if layout == Layout::V2 {
        return None;
    }
    let at = |o: usize| -> Option<f32> {
        let b = stroke.get(o..o + 4)?;
        Some(f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    Some([at(TAIL_FLOATS_AT[0])?, at(TAIL_FLOATS_AT[1])?])
}

/// The stroke header's word directory: four landmarks in the record chain.
///
/// The unit is stream words counted from the start of the body — so the unit itself
/// changes with the [`Layout`] — and resolving a pointer needs the stroke's own
/// offset. See [`Directory::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directory {
    /// Where the chain starts.
    pub first_record: u16,
    /// The stroke's resync record.
    pub resync: u16,
    /// The [`Record::mark`] record, on strokes that carry one; the terminator again
    /// on strokes that do not.
    ///
    /// ⚠️ **Not a second copy of the terminator**, though it looks like one: every
    /// instrument the editor writes leaves it equal to [`Directory::terminator`],
    /// and every vendor stroke that marks a record points it at that record instead.
    /// Reading it as the end of the chain therefore stops the walk partway through
    /// vendor content, in every generation, while nothing in a corpus of fresh
    /// renders would show it.
    pub mark: u16,
    /// Where the chain ends.
    pub terminator: u16,
}

impl Directory {
    /// Reads the directory out of a stroke header.
    pub fn read(stroke: &[u8]) -> Option<Directory> {
        let at = |i: usize| -> Option<u16> {
            let o = SEEK_AT + SEEK_STRIDE * i;
            let b = stroke.get(o..o + 2)?;
            Some(u16::from_be_bytes([b[0], b[1]]))
        };
        Some(Directory {
            first_record: at(0)?,
            resync: at(1)?,
            mark: at(2)?,
            terminator: at(3)?,
        })
    }

    /// A stored pointer as a word index within the stream.
    ///
    /// `stroke_at` is the stroke payload's offset from the start of the body — the
    /// same base the pointers were written against.
    ///
    /// ⚠️ The pointers are 16 bits and count from the body, not from the stroke, so
    /// they wrap on an instrument bigger than [`WRAP`] words — which sample-library
    /// pianos are. The arithmetic is modular for that reason, and the result is only
    /// meaningful if it lands inside the stroke: a caller must range-check it.
    pub fn resolve(pointer: u16, stroke_at: usize, layout: Layout) -> usize {
        let base = (stroke_at + layout.header_len()) / layout.word() % WRAP;
        (usize::from(pointer) + WRAP - base) % WRAP
    }
}

/// Walks a stroke's record chain.
///
/// The grammar has one production, and it is the same bit layout in every generation.
/// A record header word is `[flag:1][width−1:4][reserved:1][mark:1][order:3][count:14]`
/// in its low 24 bits, followed by `count × width` bits of two's-complement fields,
/// right-anchored and padded to a word boundary. A flag-1 record at width 1 is the
/// terminator and ends the stream; its count is the cell size. There is no separate
/// skip token — a run the encoder declined to code is an ordinary width-2 record
/// sitting over its own draft.
///
/// `stroke_at` is the stroke payload's offset from the start of the body;
/// [`crate::formats::nsmp::Sample`]'s stream accessors hand it over with the bytes.
pub fn walk(stroke: &[u8], stroke_at: usize, layout: Layout) -> Result<Stream, Unsupported> {
    let word_len = layout.word();
    let word_bits = layout.word_bits();
    let cell = layout.cell();
    let stream = stroke
        .get(layout.header_len()..)
        .ok_or(Unsupported::Short)?;
    let words = stream.len() / word_len;
    let word = |i: usize| -> u32 {
        stream[i * word_len..][..word_len]
            .iter()
            .fold(0u32, |v, &b| (v << 8) | u32::from(b))
    };
    let directory = Directory::read(stroke);
    let pointer = |get: fn(&Directory) -> u16| -> Option<usize> {
        directory
            .map(|d| Directory::resolve(get(&d), stroke_at, layout))
            .filter(|&at| at < words)
    };

    // The directory brackets the stream. Falling back to scanning is right on
    // anything the editor wrote in one pass, and wrong on library content: its
    // slack still holds words from whatever the allocation held before, and its
    // last record is not the width-1 terminator a scan would look for.
    let first_record = pointer(|d| d.first_record)
        .unwrap_or_else(|| (0..words).find(|&at| word(at) != 0).unwrap_or(words));
    let last = pointer(|d| d.terminator);

    let mut records = Vec::new();
    let mut fields = 0usize;
    let mut i = first_record;
    while i < words {
        let raw = word(i);
        // A wide word's top byte is not part of the record header and has never held
        // anything; a narrow word has no top byte to hold anything in.
        let over = raw >> 24;
        let v = raw & 0x00ff_ffff;
        let one_to_one = v >> 23 != 0;
        let width = (((v >> 19) & 0xf) + 1) as u8;
        let mark = (v >> 18) & 1 != 0;
        let order = ((v >> 14) & 0x7) as u8;
        let count = (v & COUNT_MASK) as usize;

        let ends_here = one_to_one && width == 1 && over == 0 && !mark;
        if Some(i) == last || ends_here {
            return Ok(Stream {
                records,
                fields,
                first_record,
                terminator: i,
                cell: ends_here.then_some(count),
            });
        }
        if over != 0
            || (v >> 17) & 1 != 0
            || count == 0
            || (!one_to_one && !count.is_multiple_of(cell))
        {
            return Err(Unsupported::Malformed { word: i });
        }

        let bits = word_bits + count * usize::from(width);
        let span = bits.div_ceil(word_bits);
        if i + span > last.unwrap_or(words) {
            return Err(Unsupported::Desync { word: i });
        }
        // Fields start at the first bit after the header word; the alignment tail
        // is at the end of the segment.
        let base = (i + 1) * word_bits;
        let values = (0..count)
            .map(|k| read_field(stream, base + k * usize::from(width), width))
            .collect();

        records.push(Record {
            at: i,
            first_field: fields,
            one_to_one,
            width,
            order,
            mark,
            values,
        });
        fields += count;
        i += span;
        while i < words && word(i) == 0 {
            i += 1;
        }
    }
    Err(Unsupported::NoTerminator)
}

/// Decodes a stroke payload into audio at [`FIELD_RATE`].
///
/// `stroke_at` is the stroke payload's offset from the start of the body.
pub fn decode(stroke: &[u8], stroke_at: usize, layout: Layout) -> Result<Audio, Unsupported> {
    let stream = walk(stroke, stroke_at, layout)?;
    if stream.cell == Some(2 * layout.cell()) {
        return Err(Unsupported::Stereo);
    }
    let shift = shift(stroke, layout)
        .ok_or(Unsupported::Short)?
        .clamp(-SHIFT_LIMIT, SHIFT_LIMIT);
    let mut samples = vec![0i16; stream.fields];
    let mut clipped = 0;
    let mut differenced = 0;
    // The predictor's whole state: the last MAX_ORDER field values, carried across
    // record boundaries and through skips. A stroke opens with a ramp-in that
    // settles on the content's own level, so there is nothing else to seed.
    let mut history = [0i64; MAX_ORDER];
    for record in &stream.records {
        // Only content records difference; the 1:1 regime always states values.
        let order = if record.one_to_one {
            0
        } else {
            usize::from(record.order).min(MAX_ORDER)
        };
        if order > 0 {
            differenced += record.values.len();
        }
        for (k, &residual) in record.values.iter().enumerate() {
            let mut value = i64::from(residual);
            for j in 1..=order {
                let term = binomial(order, j).saturating_mul(history[j - 1]);
                value = if j.is_multiple_of(2) {
                    value.saturating_sub(term)
                } else {
                    value.saturating_add(term)
                };
            }
            history.copy_within(0..MAX_ORDER - 1, 1);
            history[0] = value;

            let Some(slot) = samples.get_mut(record.first_field + k) else {
                continue;
            };
            let wide = if shift >= 0 {
                value.saturating_mul(1 << shift)
            } else {
                value >> -shift
            };
            *slot = wide.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
            if i64::from(*slot) != wide {
                clipped += 1;
            }
        }
    }
    Ok(Audio {
        samples,
        clipped,
        differenced,
    })
}

/// `C(n, k)`, for the small orders a record header can express.
fn binomial(n: usize, k: usize) -> i64 {
    let mut c = 1i64;
    for i in 0..k {
        c = c * (n - i) as i64 / (i + 1) as i64;
    }
    c
}

/// One field, `width` bits big-endian from `bit`, sign-extended.
fn read_field(stream: &[u8], bit: usize, width: u8) -> i32 {
    let mut v: u32 = 0;
    for i in bit..bit + usize::from(width) {
        v = (v << 1) | u32::from((stream[i / 8] >> (7 - i % 8)) & 1);
    }
    if v & (1 << (width - 1)) != 0 {
        v as i32 - (1i32 << width)
    } else {
        v as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Layout; 2] = [Layout::V2, Layout::V3];

    /// The terminator: flag 1, width 1, count = the layout's cell size.
    fn terminator(layout: Layout) -> Vec<u8> {
        let head = (1u32 << 23) | layout.cell() as u32;
        head.to_be_bytes()[4 - layout.word()..].to_vec()
    }

    /// Packs a record: header word then right-anchored fields.
    fn block(layout: Layout, one_to_one: bool, width: u8, order: u8, values: &[i32]) -> Vec<u8> {
        packed(layout, one_to_one, width, order, false, values)
    }

    fn packed(
        layout: Layout,
        one_to_one: bool,
        width: u8,
        order: u8,
        mark: bool,
        values: &[i32],
    ) -> Vec<u8> {
        let bits = layout.word_bits();
        let count = values.len();
        let head = (u32::from(one_to_one) << 23)
            | (u32::from(width - 1) << 19)
            | (u32::from(mark) << 18)
            | (u32::from(order) << 14)
            | count as u32;
        let span = (bits + count * usize::from(width)).div_ceil(bits);
        let mut out = vec![0u8; span * layout.word()];
        out[..layout.word()].copy_from_slice(&head.to_be_bytes()[4 - layout.word()..]);
        let mut at = bits;
        for &v in values {
            let raw = (v as u32) & ((1u32 << width) - 1);
            for b in (0..width).rev() {
                if raw >> b & 1 != 0 {
                    out[at / 8] |= 1 << (7 - at % 8);
                }
                at += 1;
            }
        }
        out
    }

    /// A stroke whose header points at `chain`, built as if the stroke sat at
    /// offset 0 in the body so a pointer is just a word index.
    ///
    /// `peak` and `exponent` are the two content statistics [`shift`] reads.
    fn stroke(layout: Layout, peak: i32, exponent: u8, lead: usize, chain: &[Vec<u8>]) -> Vec<u8> {
        let word = layout.word();
        let mut s = vec![0u8; layout.header_len()];
        s[STAT_A_EXP_AT] = exponent;
        s[PEAK_AT..PEAK_AT + 3].copy_from_slice(&(peak as u32 & 0xff_ffff).to_be_bytes()[1..]);
        let body: Vec<u8> = chain.concat();
        let base = layout.header_len() / word;
        let at = (base + lead) as u16;
        let end = (base + lead + body.len() / word) as u16;
        for (i, p) in [at, at, end, end].iter().enumerate() {
            let o = SEEK_AT + SEEK_STRIDE * i;
            s[o..o + 2].copy_from_slice(&p.to_be_bytes());
        }
        s.extend(std::iter::repeat_n(0u8, lead * word));
        s.extend_from_slice(&body);
        s.extend_from_slice(&terminator(layout));
        s
    }

    /// One cell of fields, which is the smallest a content record may be.
    fn run(layout: Layout, value: i32) -> Vec<i32> {
        let mut v = vec![0; layout.cell()];
        v[0] = value;
        v
    }

    /// The exponent byte that spells a given shift for a given peak, which is what
    /// the encoder writes: `A8 = 22 + s − bits(PEAK) + (PEAK a power of two)`.
    fn exponent_for(peak: i32, shift: i32) -> u8 {
        let peak = peak.unsigned_abs().max(1);
        let bits = peak.ilog2() as i32 + 1;
        (EXPONENT_BIAS + shift - bits + i32::from(peak.is_power_of_two())) as u8
    }

    #[test]
    fn the_shift_comes_off_the_exponent_byte_and_is_signed() {
        for layout in BOTH {
            // A negative shift only arises on quiet content, which is the only case
            // whose exponent byte stays in range.
            for (peak, want) in [(8191i32, 2i32), (1, 0), (4096, 7), (255, -8), (12345, 3)] {
                let s = stroke(layout, peak, exponent_for(peak, want), 0, &[]);
                assert_eq!(shift(&s, layout), Some(want), "{layout:?} peak {peak}");
            }
            // A peak of zero reads as one rather than dividing by nothing.
            let s = stroke(layout, 0, exponent_for(1, 5), 0, &[]);
            assert_eq!(shift(&s, layout), Some(5), "{layout:?}");
        }
    }

    /// The wide generations sign statistic B, so a stroke whose extreme field is
    /// negative stores it as such — and the shift comes off its magnitude, which is
    /// the same quantity the narrow generation stores unsigned.
    #[test]
    fn statistic_b_is_signed_in_the_wide_layout() {
        let s = stroke(Layout::V3, -8191, exponent_for(8191, 2), 0, &[]);
        assert_eq!(peak(&s, Layout::V3), Some(-8191));
        assert_eq!(shift(&s, Layout::V3), Some(2));
        // Silence: the wide accumulator starts at −1, so an empty stroke reads −1
        // where a narrow one reads 0. Both scale against a magnitude of one.
        let silent = stroke(Layout::V3, -1, exponent_for(1, 0), 0, &[]);
        assert_eq!(peak(&silent, Layout::V3), Some(-1));
        assert_eq!(shift(&silent, Layout::V3), Some(0));
        // The same bytes are a large positive peak in the narrow layout, which does
        // not sign the field.
        assert_eq!(peak(&silent, Layout::V2), Some(0xff_ffff));
    }

    /// The two floats past the wide header's directory are read and reported; the
    /// narrow header has neither.
    #[test]
    fn the_wide_header_carries_two_floats_the_narrow_one_does_not() {
        let mut s = stroke(Layout::V3, 1, 22, 0, &[]);
        s[TAIL_FLOATS_AT[0]..][..4].copy_from_slice(&0.0f32.to_be_bytes());
        s[TAIL_FLOATS_AT[1]..][..4].copy_from_slice(&20.0f32.to_be_bytes());
        assert_eq!(tail_floats(&s, Layout::V3), Some([0.0, 20.0]));
        assert_eq!(tail_floats(&s, Layout::V2), None);
    }

    #[test]
    fn fields_are_right_anchored_and_sign_extended() {
        for layout in BOTH {
            let mut values = run(layout, 0);
            values[..4].copy_from_slice(&[1, -1, 4095, -4096]);
            let s = stroke(layout, 1, 22, 0, &[block(layout, false, 13, 0, &values)]);
            let walked = walk(&s, 0, layout).unwrap();
            assert_eq!(walked.records.len(), 1, "{layout:?}");
            assert_eq!(walked.records[0].width, 13);
            assert_eq!(walked.records[0].values[..4], [1, -1, 4095, -4096]);
            assert_eq!(walked.fields, layout.cell());
            assert_eq!(walked.cell, Some(layout.cell()));
        }
    }

    /// The order bits are not layout: a record with an order set covers exactly the
    /// fields it counts, at the base the records before it left off.
    #[test]
    fn an_order_moves_neither_the_length_nor_the_field_base() {
        for layout in BOTH {
            let values = run(layout, 3);
            let plain = stroke(layout, 1, 22, 0, &[block(layout, false, 4, 0, &values)]);
            let ordered = stroke(layout, 1, 22, 0, &[block(layout, false, 4, 2, &values)]);
            let a = walk(&plain, 0, layout).unwrap();
            let b = walk(&ordered, 0, layout).unwrap();
            assert_eq!(a.fields, b.fields);
            assert_eq!(a.records[0].first_field, b.records[0].first_field);
            assert_eq!(b.records[0].order, 2);
            assert_eq!(a.terminator, b.terminator);
        }
    }

    /// The bit vendor strokes set on one record each is carried through the walk.
    /// Refusing it instead rejects every vendor instrument in every generation,
    /// while nothing the editor writes would ever show the difference.
    #[test]
    fn a_marked_record_walks_and_says_it_is_marked() {
        for layout in BOTH {
            let values = run(layout, 1);
            let s = stroke(
                layout,
                1,
                22,
                0,
                &[packed(layout, true, 4, 0, true, &values)],
            );
            let walked = walk(&s, 0, layout).unwrap();
            assert_eq!(walked.records.len(), 1, "{layout:?}");
            assert!(walked.records[0].mark, "{layout:?}");
            assert_eq!(walked.records[0].values, values);
            // The bit below it has never been seen set, and stays a refusal.
            let mut s = stroke(layout, 1, 22, 0, &[block(layout, true, 4, 0, &values)]);
            let head = layout.header_len();
            s[head + layout.word() - 3] |= 0x02;
            assert_eq!(
                walk(&s, 0, layout),
                Err(Unsupported::Malformed { word: 0 }),
                "{layout:?}"
            );
        }
    }

    /// A first-order run stores the differences of its field values, so reading it
    /// is a running sum — and the sum continues from whatever the record before it
    /// left the predictor holding, rather than restarting.
    #[test]
    fn a_differenced_run_integrates_from_the_running_history() {
        for layout in BOTH {
            // A plain record settling on 100, then a first-order run of zeros, which
            // is how sustained material is coded: nothing changes, so nothing is sent.
            let settle = vec![100i32; layout.cell()];
            let hold = vec![0i32; 2 * layout.cell()];
            let s = stroke(
                layout,
                1,
                22,
                0,
                &[
                    block(layout, true, 13, 0, &settle),
                    block(layout, false, 13, 1, &hold),
                ],
            );
            let audio = decode(&s, 0, layout).unwrap();
            assert_eq!(audio.differenced, 2 * layout.cell(), "{layout:?}");
            // The level carries: every field of the differenced run holds the value
            // the 1:1 record settled on.
            assert!(
                audio.samples[layout.cell()..].iter().all(|&v| v == 100),
                "{layout:?}"
            );
        }
    }

    /// A second-order run integrates twice, so a zero residual continues the slope
    /// the history already holds.
    #[test]
    fn a_second_order_run_carries_slope_as_well_as_level() {
        for layout in BOTH {
            let ramp: Vec<i32> = (0..layout.cell()).map(|k| 10 * k as i32).collect();
            let coast = vec![0i32; layout.cell()];
            let s = stroke(
                layout,
                1,
                22,
                0,
                &[
                    block(layout, true, 13, 0, &ramp),
                    block(layout, false, 13, 2, &coast),
                ],
            );
            let audio = decode(&s, 0, layout).unwrap();
            let last = layout.cell() - 1;
            assert_eq!(audio.samples[last], 10 * last as i16, "{layout:?}");
            assert_eq!(
                audio.samples[last + 1],
                10 * (last + 1) as i16,
                "{layout:?}"
            );
            assert_eq!(
                audio.samples[last + 2],
                10 * (last + 2) as i16,
                "{layout:?}"
            );
        }
    }

    /// The 1:1 records are the ones an anchoring mistake moves: their field counts
    /// leave an alignment tail, and reading it as a lead-in displaces every value.
    #[test]
    fn a_one_to_one_record_with_an_alignment_tail_reads_from_the_front() {
        for layout in BOTH {
            // A count that is not a whole number of words at this width, so the
            // segment carries a tail.
            let values: Vec<i32> = (0..layout.cell() as i32 + 6).map(|k| k * 7 - 40).collect();
            let spent = values.len() * 13;
            assert_ne!(spent % layout.word_bits(), 0, "{layout:?}: no tail to test");
            let s = stroke(layout, 1, 22, 0, &[block(layout, true, 13, 0, &values)]);
            let walked = walk(&s, 0, layout).unwrap();
            assert_eq!(walked.records[0].values, values, "{layout:?}");
        }
    }

    #[test]
    fn dequantising_shifts_by_the_headers_own_scale() {
        for layout in BOTH {
            let mut values = run(layout, 0);
            values[..2].copy_from_slice(&[100, -100]);
            let s = stroke(
                layout,
                8191,
                exponent_for(8191, 1),
                0,
                &[block(layout, false, 13, 0, &values)],
            );
            assert_eq!(decode(&s, 0, layout).unwrap().samples[..2], [200, -200]);
        }
    }

    /// Content below the source's 16-bit LSB is shifted left by the encoder, so
    /// dequantising it shifts back the other way.
    #[test]
    fn a_negative_shift_scales_back_down() {
        for layout in BOTH {
            let mut values = run(layout, 0);
            values[..2].copy_from_slice(&[2048, -2048]);
            let s = stroke(
                layout,
                8191,
                exponent_for(8191, -4),
                0,
                &[block(layout, false, 13, 0, &values)],
            );
            assert_eq!(decode(&s, 0, layout).unwrap().samples[..2], [128, -128]);
        }
    }

    #[test]
    fn a_transient_past_full_scale_clamps_and_says_so() {
        for layout in BOTH {
            let mut values = run(layout, 0);
            values[0] = 4095;
            let s = stroke(
                layout,
                8191,
                exponent_for(8191, 4),
                0,
                &[block(layout, false, 13, 0, &values)],
            );
            let audio = decode(&s, 0, layout).unwrap();
            assert_eq!(audio.samples[0], i16::MAX);
            assert_eq!(audio.clipped, 1);
        }
    }

    /// Dense content merges whole runs into one record, and those counts need the
    /// full width of the field — an eight-bit read frames them short and derails.
    #[test]
    fn a_merged_run_carries_a_count_past_a_byte() {
        for layout in BOTH {
            let n = 43 * layout.cell();
            let values: Vec<i32> = (0..n).map(|k| k as i32 % 7 - 3).collect();
            let s = stroke(layout, 1, 22, 0, &[block(layout, false, 4, 0, &values)]);
            let walked = walk(&s, 0, layout).unwrap();
            assert_eq!(walked.records.len(), 1, "{layout:?}");
            assert_eq!(walked.records[0].values, values);
        }
    }

    /// The slack in front of a stream can hold stale words, so the directory —
    /// not the first non-zero word — is what says where the chain begins.
    #[test]
    fn the_walk_starts_where_the_directory_says_not_at_the_first_data() {
        for layout in BOTH {
            let values = run(layout, 6);
            let mut s = stroke(layout, 1, 22, 2, &[block(layout, false, 4, 0, &values)]);
            let head = layout.header_len();
            s[head..head + 2 * layout.word()].fill(0x5a);
            let walked = walk(&s, 0, layout).unwrap();
            assert_eq!(walked.first_record, 2, "{layout:?}");
            assert_eq!(walked.records.len(), 1);
            assert_eq!(walked.records[0].values[0], 6);
        }
    }

    #[test]
    fn every_refusal_names_itself() {
        for layout in BOTH {
            assert_eq!(walk(&[0u8; 8], 0, layout), Err(Unsupported::Short));

            // A content run that is not a whole number of cells.
            let s = stroke(layout, 1, 22, 0, &[block(layout, false, 4, 0, &[1, 2, 3])]);
            assert_eq!(
                walk(&s, 0, layout),
                Err(Unsupported::Malformed { word: 0 }),
                "{layout:?}"
            );

            // A record whose fields run past the end of the stroke.
            let mut s = stroke(
                layout,
                1,
                22,
                0,
                &[block(layout, false, 13, 0, &run(layout, 1))],
            );
            s[layout.header_len() + layout.word() - 2] = 0xff;
            assert!(
                matches!(walk(&s, 0, layout), Err(Unsupported::Desync { .. })),
                "{layout:?}"
            );

            // A chain with nothing to end it.
            let mut s = stroke(
                layout,
                1,
                22,
                0,
                &[block(layout, false, 4, 0, &run(layout, 1))],
            );
            s.truncate(s.len() - layout.word());
            assert_eq!(walk(&s, 0, layout), Err(Unsupported::NoTerminator));
        }
    }

    /// A wide word's top byte is not part of the record header, and a word carrying
    /// one is not a record.
    #[test]
    fn a_wide_word_with_a_top_byte_is_not_a_record() {
        let mut s = stroke(
            Layout::V3,
            1,
            22,
            0,
            &[block(Layout::V3, false, 4, 0, &run(Layout::V3, 1))],
        );
        s[Layout::V3.header_len()] = 0x01;
        assert_eq!(
            walk(&s, 0, Layout::V3),
            Err(Unsupported::Malformed { word: 0 })
        );
    }

    #[test]
    fn the_directory_resolves_against_the_strokes_own_offset() {
        let mut s = vec![0u8; Layout::V2.header_len()];
        for (i, p) in [444u16, 483, 762, 762].iter().enumerate() {
            let at = SEEK_AT + SEEK_STRIDE * i;
            s[at..at + 2].copy_from_slice(&p.to_be_bytes());
        }
        let dir = Directory::read(&s).unwrap();
        assert_eq!(dir.first_record, 444);
        assert_eq!(dir.resync, 483);
        assert_eq!(dir.mark, 762);
        assert_eq!(dir.terminator, 762);
        // A single-zone instrument puts its one stroke 981 bytes into the body, so
        // its stream starts at word 344 and the pointers count on from there.
        assert_eq!(Directory::resolve(dir.first_record, 981, Layout::V2), 100);
        assert_eq!(Directory::resolve(dir.terminator, 981, Layout::V2), 418);
        // A pointer below the base belongs to an earlier stroke, and lands past the
        // wrap rather than before zero — which is why a caller range-checks.
        assert_eq!(Directory::resolve(1, 981, Layout::V2), WRAP - 343);
        // A stroke far enough into a big instrument has a base past the wrap, and
        // its pointers count on from there modulo it.
        let far = 3 * (344 + 3 * WRAP) - Layout::V2.header_len();
        assert_eq!(Directory::resolve(dir.first_record, far, Layout::V2), 100);
        // The unit is the layout's word, so the same pointer at the same byte offset
        // names a different word in the wide chain.
        assert_eq!(Directory::resolve(444, 4 * 100 - 68, Layout::V3), 344);
    }

    #[test]
    fn the_field_rate_is_the_lattice_rate() {
        assert_eq!(FIELD_RATE, 35_002);
    }

    #[test]
    fn the_layout_follows_the_content_version() {
        assert_eq!(Layout::from_version(8), Layout::V2);
        assert_eq!(Layout::from_version(200), Layout::V2);
        assert_eq!(Layout::from_version(300), Layout::V3);
        assert_eq!(Layout::from_version(420), Layout::V3);
    }
}
