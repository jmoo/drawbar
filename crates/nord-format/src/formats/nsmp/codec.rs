//! The stroke codec: one zone's encoded audio back into samples.
//!
//! A stroke payload is a fixed header followed by a stream of words. The words carry
//! *fields* — the source resampled onto a uniform lattice of [`PITCH_DEN`] fields per
//! [`PITCH_NUM`] input samples — quantised by truncation and one arithmetic shift the
//! header records. Decoding is a walk, that shift, and one integration: a record may
//! store the Nth backward difference of its fields rather than the fields themselves.
//!
//! Three generations share this one codec, and every entry point takes the [`Layout`]
//! saying which. Mostly what differs is *units* — word width, cell size, header size —
//! and the lattice, the kernel, the quantiser and the grammar's bit layout do not move
//! at all. The one behavioural difference is how a **stereo** stroke carries its two
//! channels: v2 and v3 alternate fields, v4 alternates words.
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
//! C(N,j)·V(f−j)`, over a running history carried across every record boundary and
//! through every skip. Nothing needs seeding — a stroke opens with a 1:1 ramp-in that
//! settles on the content's own field value, and the history takes it from there.
//! **A stereo stroke keeps one history per channel**; they are two signals sharing a
//! header, and predicting one against the other's samples diverges.
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
/// `.nsmp3` and `.nsmp4` share every unit — word size, cell size, header length — and
/// differ only in behaviour: v4 sometimes quantises one bit finer, which the stroke
/// header states either way, and v4 alone gives a stereo stroke's two channels a word
/// stream each ([`Layout::splits_wide_openings`]) where v2 and v3 alternate fields.
/// That second difference changes how long a record is, so the two cannot share one
/// variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `.nsmp`: 3-byte words, 24-field cells, a 51-byte stroke header.
    V2,
    /// `.nsmp3`: 4-byte words, 32-field cells, a 68-byte stroke header.
    V3,
    /// `.nsmp4`: [`Layout::V3`]'s units, and a word stream per channel on stereo.
    V4,
}

impl Layout {
    /// The layout a body's content version implies. The u32 at `0x14` runs
    /// `format × 100 + revision`, so anything from 300 up is the wide chain and
    /// anything from 400 up is v4.
    pub fn from_version(version: u32) -> Layout {
        match version {
            v if v >= super::V4_FROM_VERSION => Layout::V4,
            v if v >= super::V3_FROM_VERSION => Layout::V3,
            _ => Layout::V2,
        }
    }

    /// Whether a stereo stroke gives each channel its own run of words, the two
    /// interleaved word by word, rather than interleaving the channels field by field.
    ///
    /// ⚠️ **True on [`Layout::V4`] only.** v2 and v3 alternate whole fields, so their
    /// records are one bit-run and size like any other. Mono never splits anywhere.
    ///
    /// It is a sizing question as well as a de-interleaving one: each channel's half of
    /// a record is padded to a whole word, so the payload is
    /// `2 × ceil((count/2 × width) / word_bits)` words. That exceeds the unsplit
    /// `ceil((word_bits + count × width) / word_bits)` by one whenever the halves do
    /// not tile — which content records never do, their counts being multiples of the
    /// stereo cell, and 1:1 records regularly do.
    pub const fn splits_wide_openings(self) -> bool {
        matches!(self, Layout::V4)
    }

    /// Bytes per stream word. A record header is exactly one word, and the top byte
    /// of a wide word is never part of it.
    ///
    /// ⚠️ The top byte is not always zero — vendor *payload* words fill it. It is only
    /// the header's own reserved space, so the check belongs on words being read as
    /// headers, not on the stream at large.
    pub const fn word(self) -> usize {
        match self {
            Layout::V2 => 3,
            Layout::V3 | Layout::V4 => 4,
        }
    }

    /// Bytes of fixed stroke header ahead of the word stream.
    ///
    /// The wide header is the narrow one field for field — same statistics, same
    /// directory — plus two float32s and the room to hold them.
    pub const fn header_len(self) -> usize {
        match self {
            Layout::V2 => 51,
            Layout::V3 | Layout::V4 => 68,
        }
    }

    /// Fields per cell. A content record covers whole cells, so its count is a
    /// multiple of this; the terminator's own count states it.
    ///
    /// ⚠️ A stereo stroke cells at twice this, which is still a multiple of it, so
    /// this is the divisor to check against rather than the cell size to assume.
    pub const fn cell(self) -> usize {
        match self {
            Layout::V2 => 24,
            Layout::V3 | Layout::V4 => 32,
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
/// ⚠️ **Strokes do run longer than this**, so a resolved pointer names a whole family
/// of words `WRAP` apart and the reader has to pick. Which alias is right depends on
/// what the pointer names: the terminator sits at the end of the stream, so it takes
/// the highest alias that still lands inside it, while `ptr[0]` opens the stream and
/// takes the lowest. Raising both the same way puts the opening record past the
/// content. Vendor strokes reach 216 KiB — 72,000-odd words — where the editor's own
/// renders never leave the first period.
pub const WRAP: usize = 1 << 16;

/// Input samples per [`PITCH_DEN`] fields.
///
/// ⚠️ **An approximation, by 1.6e-7.** The pitch is `22050/17501` — the ratio that
/// puts the field rate on a round 35,002 Hz — and `349/277` is its continued-fraction
/// convergent. The error is a timing drift of about one field per 17,501, invisible
/// on anything short and not audible on anything, but it is a drift rather than an
/// offset, so a long stroke's late fields land on the wrong side of a quantiser edge.
///
/// It stays because [`super::kernel::PHASES`] is `PITCH_DEN`: the exact ratio wants a
/// 17,501-phase bank where this wants 277, and the instrument's own kernel is neither
/// — it is a 512-entry table read by truncating the phase to 9 bits. Moving to the
/// exact pitch means replacing the analytic bank, not editing two numbers.
pub const PITCH_NUM: u32 = 349;
/// Fields per [`PITCH_NUM`] input samples.
pub const PITCH_DEN: u32 = 277;

/// Rate the editor resamples every import to before encoding. Neither the source
/// rate nor its bit depth survives anywhere in the file.
pub const SOURCE_RATE: u32 = 44_100;

/// Field rate in Hz. Exactly 35,002 — `44100 × 17501/22050` is a whole number, and the
/// rounding here only absorbs what [`PITCH_NUM`] approximates.
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
}

impl Unsupported {
    /// A stable label, for tallying coverage across a whole library.
    pub fn reason(self) -> &'static str {
        match self {
            Unsupported::Short => "short-stroke",
            Unsupported::Malformed { .. } => "malformed-record",
            Unsupported::Desync { .. } => "desync",
            Unsupported::NoTerminator => "no-terminator",
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
    /// ⚠️ **Channel-major on a stereo stroke** — the first half is one channel's
    /// fields and the second half the other's, whichever way the stream stored them.
    /// The walk undoes the interleave so that a reader never has to know which
    /// generation it came from; what it must know is that the two halves are
    /// *separate signals*, each predicted against its own history.
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
    /// Channels the stroke carries: 2 when the terminator states twice the layout's
    /// cell, 1 otherwise. Half of the vendor library is stereo.
    pub channels: usize,
}

/// Decoded audio for one zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audio {
    /// One sample per field, at [`FIELD_RATE`], **interleaved by channel** — so on a
    /// stereo zone this is `L R L R` and holds two samples per frame.
    pub samples: Vec<i16>,
    /// 1 or 2.
    pub channels: u16,
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
    /// Frames — samples per channel, which is what the duration is measured in.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels).max(1)
    }

    /// Duration in seconds.
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(FIELD_RATE)
    }
}

/// The content peak the stroke header records, or `None` if it is too short.
///
/// ⚠️ **The wide generations sign it.** They store the extreme field with its sign and
/// start the accumulator at −1, so a silent stroke reads `-1` where a [`Layout::V2`]
/// one reads `0`. The magnitude is the same quantity in both, and the magnitude is what
/// the quantiser scales against.
pub fn peak(stroke: &[u8], layout: Layout) -> Option<i32> {
    let b = stroke.get(PEAK_AT..PEAK_AT + 3)?;
    let v = u32::from_be_bytes([0, b[0], b[1], b[2]]);
    Some(match layout {
        Layout::V2 => v as i32,
        _ if v >= 1 << 23 => v as i32 - (1 << 24),
        _ => v as i32,
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

    /// A stored pointer resolved against a stream of `words`, taking the **last** of
    /// its aliases that still lands inside it.
    ///
    /// This is the reading for [`Directory::terminator`], which by definition sits at
    /// the end of the stream. On a stroke shorter than [`WRAP`] words it is exactly
    /// [`Directory::resolve`]; past that it is the only reading that finds the end
    /// rather than a word one period short of it.
    pub fn resolve_end(pointer: u16, stroke_at: usize, layout: Layout, words: usize) -> usize {
        let mut at = Directory::resolve(pointer, stroke_at, layout);
        while at + WRAP < words {
            at += WRAP;
        }
        at
    }
}

/// The cell size a word states if it is the stream's terminator, or `None` if it is
/// not one.
///
/// A terminator is a width-1 flag-1 word whose count is the cell size — `cell` on a
/// mono stroke, twice it on a stereo one.
///
/// ⚠️ **The count is the test, not a detail read off afterwards.** Payload words land
/// on that bit pattern with other counts, and taking the first of them for the end
/// stops the walk in the middle of the stream: one library stroke carries a count-0
/// match at word 94 of 11,244, with its directory naming the last word and ten
/// thousand more words of content in between.
fn terminator_cell(raw: u32, cell: usize) -> Option<usize> {
    let v = raw & 0x00ff_ffff;
    let one_to_one = v >> 23 != 0;
    let width = ((v >> 19) & 0xf) + 1;
    let mark = (v >> 18) & 1 != 0;
    let count = (v & COUNT_MASK) as usize;
    let ok =
        one_to_one && width == 1 && raw >> 24 == 0 && !mark && (count == cell || count == 2 * cell);
    ok.then_some(count)
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

    // The directory brackets the stream. Falling back to scanning is right on
    // anything the editor wrote in one pass, and wrong on library content: its
    // slack still holds words from whatever the allocation held before, and its
    // last record is not the width-1 terminator a scan would look for.
    let first_record = directory
        .map(|d| Directory::resolve(d.first_record, stroke_at, layout))
        .filter(|&at| at < words)
        .unwrap_or_else(|| (0..words).find(|&at| word(at) != 0).unwrap_or(words));
    let last = directory
        .map(|d| Directory::resolve_end(d.terminator, stroke_at, layout, words))
        .filter(|&at| at < words);

    // Whether the stroke is stereo is stated by the terminator, and a stereo stroke
    // needs that answer before it can read — on [`Layout::V4`], size — its first
    // record. Read it off the word the directory names rather than discovering it at
    // the end.
    let stereo = last.is_some_and(|at| terminator_cell(word(at), cell) == Some(2 * cell));
    let wide_openings = stereo && layout.splits_wide_openings();

    let mut records = Vec::new();
    // Reused across records: one channel's words, gathered out of the stream.
    let mut gathered: Vec<u8> = Vec::new();
    let mut fields = 0usize;
    let mut i = first_record;
    while i < words {
        let raw = word(i);
        // A wide word's top byte is not part of the record header, and no header has
        // ever set it; a narrow word has no top byte to set.
        let over = raw >> 24;
        let v = raw & 0x00ff_ffff;
        let one_to_one = v >> 23 != 0;
        let width = (((v >> 19) & 0xf) + 1) as u8;
        let mark = (v >> 18) & 1 != 0;
        let order = ((v >> 14) & 0x7) as u8;
        let count = (v & COUNT_MASK) as usize;

        let ends_here = terminator_cell(raw, cell).is_some();
        if Some(i) == last || ends_here {
            return Ok(Stream {
                records,
                fields,
                first_record,
                terminator: i,
                cell: ends_here.then_some(count),
                channels: if stereo { 2 } else { 1 },
            });
        }
        if over != 0
            || (v >> 17) & 1 != 0
            || count == 0
            || (!one_to_one && !count.is_multiple_of(cell))
        {
            return Err(Unsupported::Malformed { word: i });
        }

        let span = if wide_openings && one_to_one && count.is_multiple_of(2) {
            // Each channel's share is padded to a whole word, so the record is a word
            // longer than one run of the same fields whenever the halves do not tile.
            // Content counts are multiples of the stereo cell and always tile; the 1:1
            // records are where it shows.
            1 + 2 * (count / 2 * usize::from(width)).div_ceil(word_bits)
        } else {
            (word_bits + count * usize::from(width)).div_ceil(word_bits)
        };
        if i + span > last.unwrap_or(words) {
            return Err(Unsupported::Desync { word: i });
        }
        // Fields start at the first bit after the header word; the alignment tail
        // is at the end of the segment.
        let base = (i + 1) * word_bits;
        let values = if !stereo {
            (0..count)
                .map(|k| read_field(stream, base + k * usize::from(width), width))
                .collect()
        } else if wide_openings {
            // Each channel owns alternate words. Gather one channel's words into a
            // run of its own and the fields fall out of it at the usual offsets.
            let per = count / 2;
            let channel_words = (per * usize::from(width)).div_ceil(word_bits);
            let mut values = Vec::with_capacity(count);
            for channel in 0..2 {
                gathered.clear();
                for k in 0..channel_words {
                    let at = (i + 1 + 2 * k + channel) * word_len;
                    gathered.extend_from_slice(&stream[at..at + word_len]);
                }
                values
                    .extend((0..per).map(|k| read_field(&gathered, k * usize::from(width), width)));
            }
            values
        } else {
            // One run of fields, the channels taking turns within it.
            let field = |k: usize| read_field(stream, base + k * usize::from(width), width);
            (0..count)
                .map(|k| field(2 * (k % (count / 2)) + k / (count / 2)))
                .collect()
        };

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
    let channels = stream.channels;
    let shift = shift(stroke, layout)
        .ok_or(Unsupported::Short)?
        .clamp(-SHIFT_LIMIT, SHIFT_LIMIT);
    let mut samples = vec![0i16; stream.fields];
    let mut clipped = 0;
    let mut differenced = 0;
    // The predictor's whole state: the last MAX_ORDER field values, carried across
    // record boundaries and through skips. A stroke opens with a ramp-in that
    // settles on the content's own level, so there is nothing else to seed.
    //
    // ⚠️ **One history per channel.** A stereo stroke's two channels are two signals
    // that happen to share a header; predicting one against the other's samples makes
    // the integration diverge rather than merely sound wrong.
    let mut history = [[0i64; MAX_ORDER]; 2];
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
        // Values arrive channel-major, so the halves index their own channel and the
        // output interleaves them back together.
        let per = record.values.len() / channels;
        for (k, &residual) in record.values.iter().enumerate() {
            let (channel, k) = if channels == 2 {
                (k / per, k % per)
            } else {
                (0, k)
            };
            let history = &mut history[channel];
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

            let at = record.first_field + k * channels + channel;
            let Some(slot) = samples.get_mut(at) else {
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
        channels: channels as u16,
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

    /// A stereo stroke carrying one content record of `l` and `r`, laid out the way
    /// `layout` lays a stereo stroke out: alternating fields on v2 and v3, a word
    /// stream each on v4.
    fn stereo_stroke(layout: Layout, width: u8, l: &[i32], r: &[i32]) -> Vec<u8> {
        assert_eq!(l.len(), r.len());
        let word = layout.word();
        let bits = layout.word_bits();
        let count = l.len() * 2;
        let head = (u32::from(width - 1) << 19) | count as u32;

        let mut body = head.to_be_bytes()[4 - word..].to_vec();
        if layout.splits_wide_openings() {
            // Pack each channel on its own, then take one word from each in turn.
            let per = |v: &[i32]| {
                let mut out = vec![0u8; (v.len() * usize::from(width)).div_ceil(bits) * word];
                let mut at = 0;
                for &x in v {
                    for b in (0..width).rev() {
                        if (x as u32) >> b & 1 != 0 {
                            out[at / 8] |= 1 << (7 - at % 8);
                        }
                        at += 1;
                    }
                }
                out
            };
            let (a, b) = (per(l), per(r));
            for k in 0..a.len() / word {
                body.extend_from_slice(&a[k * word..][..word]);
                body.extend_from_slice(&b[k * word..][..word]);
            }
        } else {
            let woven: Vec<i32> = l.iter().zip(r).flat_map(|(&a, &b)| [a, b]).collect();
            body = block(layout, false, width, 0, &woven);
        }

        let mut s = vec![0u8; layout.header_len()];
        s[STAT_A_EXP_AT] = exponent_for(1, 0);
        s[PEAK_AT..PEAK_AT + 3].copy_from_slice(&1u32.to_be_bytes()[1..]);
        let base = (layout.header_len() / word) as u16;
        let end = base + (body.len() / word) as u16;
        for (i, p) in [base, base, end, end].iter().enumerate() {
            let o = SEEK_AT + SEEK_STRIDE * i;
            s[o..o + 2].copy_from_slice(&p.to_be_bytes());
        }
        s.extend_from_slice(&body);
        let term = (1u32 << 23) | (2 * layout.cell()) as u32;
        s.extend_from_slice(&term.to_be_bytes()[4 - word..]);
        s
    }

    #[test]
    fn a_stereo_stroke_decodes_to_two_channels() {
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let per = layout.cell(); // one stereo cell is `cell` fields per channel
            let l: Vec<i32> = (0..per as i32).map(|k| 100 + k).collect();
            let r: Vec<i32> = (0..per as i32).map(|k| -100 - k).collect();
            let s = stereo_stroke(layout, 11, &l, &r);

            let stream = walk(&s, 0, layout).expect("the stereo stroke walks");
            assert_eq!(stream.channels, 2, "{layout:?}");
            assert_eq!(stream.cell, Some(2 * layout.cell()), "{layout:?}");

            let audio = decode(&s, 0, layout).expect("the stereo stroke decodes");
            assert_eq!(audio.channels, 2, "{layout:?}");
            assert_eq!(audio.frames(), per, "{layout:?}");
            let got_l: Vec<i32> = audio
                .samples
                .iter()
                .step_by(2)
                .map(|&v| i32::from(v))
                .collect();
            let got_r: Vec<i32> = audio.samples[1..]
                .iter()
                .step_by(2)
                .map(|&v| i32::from(v))
                .collect();
            assert_eq!(got_l, l, "{layout:?}: left channel");
            assert_eq!(got_r, r, "{layout:?}: right channel");
        }
    }

    #[test]
    fn a_stereo_stroke_predicts_each_channel_against_its_own_history() {
        // Two ramps of opposite slope. Order 1 stores first differences, so reading
        // them against one shared history integrates the wrong signal and the two
        // channels come back as a runaway rather than as ramps.
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let per = layout.cell();
            let l: Vec<i32> = (0..per as i32).map(|k| 10 * k).collect();
            let r: Vec<i32> = (0..per as i32).map(|k| -7 * k).collect();
            let diff = |v: &[i32]| -> Vec<i32> {
                v.iter()
                    .enumerate()
                    .map(|(i, &x)| if i == 0 { x } else { x - v[i - 1] })
                    .collect()
            };
            let s = stereo_stroke_ordered(layout, 11, &diff(&l), &diff(&r));
            let audio = decode(&s, 0, layout).expect("decodes");
            let got_l: Vec<i32> = audio
                .samples
                .iter()
                .step_by(2)
                .map(|&v| i32::from(v))
                .collect();
            let got_r: Vec<i32> = audio.samples[1..]
                .iter()
                .step_by(2)
                .map(|&v| i32::from(v))
                .collect();
            assert_eq!(got_l, l, "{layout:?}: left ramp");
            assert_eq!(got_r, r, "{layout:?}: right ramp");
        }
    }

    /// [`stereo_stroke`] with the record's order set to 1.
    fn stereo_stroke_ordered(layout: Layout, width: u8, l: &[i32], r: &[i32]) -> Vec<u8> {
        let mut s = stereo_stroke(layout, width, l, r);
        let at = layout.header_len();
        let word = layout.word();
        let mut head = 0u32;
        for &b in &s[at..at + word] {
            head = (head << 8) | u32::from(b);
        }
        head |= 1 << 14;
        s[at..at + word].copy_from_slice(&head.to_be_bytes()[4 - word..]);
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
        assert_eq!(Layout::from_version(310), Layout::V3);
        assert_eq!(Layout::from_version(400), Layout::V4);
        assert_eq!(Layout::from_version(420), Layout::V4);
    }

    #[test]
    fn only_v4_splits_a_stereo_stroke_s_openings() {
        assert!(Layout::V4.splits_wide_openings());
        assert!(!Layout::V3.splits_wide_openings());
        assert!(!Layout::V2.splits_wide_openings());
    }

    #[test]
    fn a_terminator_is_a_width_one_word_stating_the_cell_size() {
        // Mono and stereo terminators, in both word sizes.
        assert_eq!(terminator_cell(0x0080_0018, 24), Some(24));
        assert_eq!(terminator_cell(0x0080_0030, 24), Some(48));
        assert_eq!(terminator_cell(0x0080_0020, 32), Some(32));
        assert_eq!(terminator_cell(0x0080_0040, 32), Some(64));
        // The shape without the count is payload, not the end of the stream.
        assert_eq!(terminator_cell(0x0080_0000, 32), None);
        assert_eq!(terminator_cell(0x0080_0018, 32), None);
        // A marked record is a record, and a wide word's top byte is reserved here.
        assert_eq!(terminator_cell(0x00c4_0020, 32), None);
        assert_eq!(terminator_cell(0x6580_0020, 32), None);
    }

    #[test]
    fn the_terminator_pointer_rises_past_the_period_and_the_opening_does_not() {
        let first = Directory::resolve(5_999, 0, Layout::V2);
        // A stroke inside one period has one alias, so both readings agree.
        assert_eq!(
            Directory::resolve_end(5_999, 0, Layout::V2, first + 1),
            first
        );
        // Past it the terminator takes the last alias that still lands inside the
        // stream, while resolve — what the opening pointer uses — stays at the first.
        for periods in 1..4 {
            let words = first + periods * WRAP + 1;
            assert_eq!(
                Directory::resolve_end(5_999, 0, Layout::V2, words),
                first + periods * WRAP
            );
        }
    }
}
