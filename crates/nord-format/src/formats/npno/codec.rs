//! The stroke codec: one recorded note's blocks back into samples.
//!
//! A stroke's audio is [`Stroke::blocks`](super::Stroke::blocks) back-to-back
//! blocks of [`BLOCK_WORDS`] × 2 × channels bytes. Each opens with one big-endian
//! u16 header and then carries big-endian u16 words of packed residuals:
//!
//! ```text
//! bits 0..4   the residual field width in bits
//! bits 5..7   the backward-difference order, 0..=4
//! bits 8..15  a per-block attenuation statistic, in dB against a full scale of
//!             8192; the decoder ignores it, and it is not a gain to apply
//! ```
//!
//! ⚠️ The packing is low-bit-first. Append each big-endian word to the high end
//! of a reservoir and take fields from its low bits, so global bit `b` is bit
//! `b % 16` of word `b / 16` counting that word's least significant bit as bit 0.
//! Fields are two's complement, they cross word boundaries freely, and on a stereo
//! stroke consecutive fields alternate channels. Reading the residuals
//! most-significant-bit first is structurally plausible and reconstructs no signal.
//!
//! The recurrence is the finite-difference predictor
//! [`nsmp`](crate::formats::nsmp::codec) uses, per channel:
//!
//! ```text
//! x[n] = r[n] − Σ_{j=1..order} (−1)^j C(order, j) · x[n−j]
//! ```
//!
//! seeded from the four samples the stroke's record carries, oldest first.
//!
//! A block holds `⌊8 · (block bytes − 2) / (width · channels)⌋` frames, and its last
//! [`OVERLAP`] frames repeat as the first frames of the next block. Only the frames
//! before that repeat are emitted. ⚠️ The history carried into the next block is the
//! four samples immediately before the repeat, not the four at the physical block end.
//! The emitted frames sum to the frame count the record states, and the repeat is
//! bit-exact; [`decode`] refuses a stroke that fails either check. The last block has
//! no next block to repeat into, so its final [`OVERLAP`] frames sit past the stroke's
//! end, in [`Audio::tail`].
//!
//! The packing and the predictor: Inferred from specimens; not confirmed on hardware.
//! This module only reconstructs samples; it plays nothing.
//!
//! The frames play at [`RATE`], and a stroke owns all
//! `blocks × BLOCK_WORDS × 2 × channels` bytes the container gives it: a library whose
//! spans were moved at that size still plays. Confirmed on hardware.

use super::Stroke;
use crate::error::{Error, ParseError};
use crate::formats::predictor;

/// u16 words in one block, per channel, the header included.
pub const BLOCK_WORDS: usize = 511;

/// Frames a block repeats from the one before it. Never emitted.
pub const OVERLAP: usize = 64;

/// Frames per second per channel. Confirmed on hardware.
pub const RATE: u32 = 35_002;

/// Highest backward-difference order a block header can ask for.
pub const MAX_ORDER: usize = predictor::MAX_ORDER;

/// Narrowest residual field a block header can express.
pub const MIN_WIDTH: u8 = 1;

/// Widest residual field accepted: one 16-bit word. The header's five width bits can
/// state more, and a wider field is refused.
pub const MAX_WIDTH: u8 = 16;

/// Frames a block of `width` carries, including the [`OVERLAP`] it repeats. Wider
/// fields mean fewer frames, so choosing the width fixes the frame count.
pub fn block_frames(width: u8, block_bytes: usize, channels: usize) -> usize {
    8 * (block_bytes - 2) / (usize::from(width) * channels)
}

/// Decoded audio for one stroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audio {
    /// One vector per channel, each [`Audio::frames`] long.
    pub lanes: Vec<Vec<i16>>,
    /// The [`OVERLAP`] frames per channel the last block carries past the stroke's
    /// end. The stroke does not own them and nothing plays them, but re-encoding that
    /// block needs them.
    pub tail: Vec<Vec<i16>>,
    /// Samples the reconstruction put outside `i16`, saturated to fit. A nonzero count
    /// means the stroke is not what this codec describes. Inferred from specimens; not
    /// confirmed on hardware.
    pub clipped: usize,
    /// Repeated samples compared against the block before, all of which matched.
    pub overlap_checked: usize,
}

impl Audio {
    /// Samples per channel.
    pub fn frames(&self) -> usize {
        self.lanes.first().map_or(0, Vec::len)
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(RATE)
    }

    /// The frames interleaved by channel, as a WAV file stores them.
    pub fn interleaved(&self) -> Vec<i16> {
        let frames = self.frames();
        let mut out = Vec::with_capacity(frames * self.lanes.len());
        for frame in 0..frames {
            out.extend(self.lanes.iter().map(|c| c[frame]));
        }
        out
    }
}

/// What one block's header states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub width: u8,
    pub order: u8,
    /// Attenuation in dB against a full scale of 8192. The encoder recorded it as a
    /// statistic; it is not a gain to apply.
    pub attenuation: u8,
}

impl BlockHeader {
    fn read(word: u16) -> BlockHeader {
        BlockHeader {
            width: (word & 0x1f) as u8,
            order: ((word >> 5) & 7) as u8,
            attenuation: (word >> 8) as u8,
        }
    }

    /// Frames this block carries, the overlap included.
    fn frames(self, block_bytes: usize, channels: usize) -> usize {
        block_frames(self.width, block_bytes, channels)
    }
}

/// Residual fields, low-bit-first out of a block's big-endian u16 words.
struct Fields<'a> {
    words: &'a [u8],
    next: usize,
    reservoir: u64,
    held: u32,
}

impl<'a> Fields<'a> {
    fn new(words: &'a [u8]) -> Fields<'a> {
        Fields {
            words,
            next: 0,
            reservoir: 0,
            held: 0,
        }
    }

    fn take(&mut self, width: u8) -> Option<i32> {
        while self.held < u32::from(width) {
            let at = self.next * 2;
            let word = u16::from_be_bytes(self.words.get(at..at + 2)?.try_into().unwrap());
            self.reservoir |= u64::from(word) << self.held;
            self.held += 16;
            self.next += 1;
        }
        let value = (self.reservoir & ((1u64 << width) - 1)) as i64;
        self.reservoir >>= width;
        self.held -= u32::from(width);
        let sign = 1i64 << (width - 1);
        Some(((value ^ sign) - sign) as i32)
    }
}

/// Decode one stroke, checking the block overlap and the record's frame count.
///
/// `channels` is the library's channel count, 1 or 2; any other count is refused.
/// `stroke.audio()` must be the whole span.
pub fn decode(stroke: &Stroke<'_>, channels: u16) -> Result<Audio, Error> {
    if !(1..=2).contains(&channels) {
        return Err(ParseError::OutOfBounds {
            value: format!("{channels} channels"),
            bound: "1 or 2, which is what a library states".into(),
        }
        .into());
    }
    let channels = usize::from(channels);
    let block_bytes = BLOCK_WORDS * 2 * channels;
    let audio = stroke.audio();
    let blocks = usize::from(stroke.blocks());
    if audio.len() != blocks * block_bytes {
        return Err(ParseError::AssertFail(format!(
            "the stroke spans {} bytes where {blocks} blocks hold {}",
            audio.len(),
            blocks * block_bytes
        ))
        .into());
    }

    let frames = usize::try_from(stroke.frames()).map_err(|_| ParseError::OutOfBounds {
        value: format!("{} frames", stroke.frames()),
        bound: "a frame count that fits this platform's address space".into(),
    })?;
    // The narrowest field a header can declare is the longest block, so this is the
    // most frames the span can own whatever its headers say.
    let most = blocks * (block_frames(MIN_WIDTH, block_bytes, channels) - OVERLAP);
    if frames > most {
        return Err(ParseError::AssertFail(format!(
            "the blocks own at most {most} frames where the record states {frames}"
        ))
        .into());
    }
    let mut out: Vec<Vec<i16>> = Vec::with_capacity(channels);
    for _ in 0..channels {
        let mut channel = Vec::new();
        channel
            .try_reserve_exact(frames)
            .map_err(|_| ParseError::OutOfBounds {
                value: format!("{frames} frames"),
                bound: "an allocation that fits memory".into(),
            })?;
        out.push(channel);
    }

    let seeds = stroke.seeds();
    let mut history = [[0i64; MAX_ORDER]; 2];
    for (state, seeds) in history.iter_mut().zip(&seeds) {
        // The record states the seeds oldest first; the recurrence wants the most
        // recent sample at index 0.
        for (j, slot) in state.iter_mut().enumerate() {
            *slot = i64::from(seeds[MAX_ORDER - 1 - j]);
        }
    }

    let mut clipped = 0;
    let mut overlap_checked = 0;
    let mut tail: Vec<Vec<i32>> = Vec::new();
    let mut block = vec![vec![0i32; 0]; channels];
    for index in 0..blocks {
        let raw = &audio[index * block_bytes..(index + 1) * block_bytes];
        let header = BlockHeader::read(u16::from_be_bytes([raw[0], raw[1]]));
        if !(MIN_WIDTH..=MAX_WIDTH).contains(&header.width) || usize::from(header.order) > MAX_ORDER
        {
            return Err(ParseError::OutOfBounds {
                value: format!(
                    "block {index}: width {} order {}",
                    header.width, header.order
                ),
                bound: format!(
                    "a width of {MIN_WIDTH} to {MAX_WIDTH} and an order of at most {MAX_ORDER}"
                ),
            }
            .into());
        }
        let block_frames = header.frames(block_bytes, channels);
        // The frames before the repeat have to cover it and still leave the four
        // the next block's predictor continues from.
        if block_frames < OVERLAP + MAX_ORDER {
            return Err(ParseError::AssertFail(format!(
                "block {index} holds {block_frames} frames, too few for the {OVERLAP} it \
                 repeats from the block before plus the {MAX_ORDER} the next one seeds from"
            ))
            .into());
        }
        let owned = block_frames - OVERLAP;

        let mut fields = Fields::new(&raw[2..]);
        let order = usize::from(header.order);
        for channel in block.iter_mut() {
            channel.clear();
            channel.reserve(block_frames);
        }
        for _ in 0..block_frames {
            for (channel, state) in block.iter_mut().zip(history.iter_mut()) {
                let residual = fields.take(header.width).ok_or_else(|| {
                    ParseError::AssertFail(format!(
                        "block {index} runs out of words before its {block_frames} frames"
                    ))
                })?;
                let value = predictor::predict(state, order, i64::from(residual));
                channel.push(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
            }
        }

        if !tail.is_empty() {
            for (channel, (decoded, expected)) in block.iter().zip(&tail).enumerate() {
                if decoded[..OVERLAP] != expected[..] {
                    let at = decoded[..OVERLAP]
                        .iter()
                        .zip(expected)
                        .position(|(a, b)| a != b)
                        .unwrap_or(0);
                    return Err(ParseError::AssertFail(format!(
                        "block {index} channel {channel} repeats frame {at} as {} where the \
                         block before decoded {}",
                        decoded[at], expected[at]
                    ))
                    .into());
                }
                overlap_checked += OVERLAP;
            }
        }
        tail = block.iter().map(|c| c[owned..].to_vec()).collect();
        // The history the next block continues from sits before the repeat, not at
        // the physical end of this one.
        for (state, decoded) in history.iter_mut().zip(&block) {
            for (j, slot) in state.iter_mut().enumerate() {
                *slot = i64::from(decoded[owned - 1 - j]);
            }
        }

        for (channel, decoded) in out.iter_mut().zip(&block) {
            for &sample in &decoded[..owned] {
                let narrow = sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                if i32::from(narrow) != sample {
                    clipped += 1;
                }
                channel.push(narrow);
            }
        }
    }

    let decoded = out.first().map_or(0, Vec::len);
    if decoded != frames {
        return Err(ParseError::AssertFail(format!(
            "the blocks own {decoded} frames where the record states {frames}"
        ))
        .into());
    }

    let mut narrowed = Vec::with_capacity(channels);
    for channel in &tail {
        narrowed.push(
            channel
                .iter()
                .map(|&sample| {
                    let narrow = sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                    clipped += usize::from(i32::from(narrow) != sample);
                    narrow
                })
                .collect(),
        );
    }

    Ok(Audio {
        lanes: out,
        tail: narrowed,
        clipped,
        overlap_checked,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{RECORD, REC_BLOCKS, REC_FRAMES, REC_SEEDS};
    use super::*;

    /// Packs `frames × channels` residuals the way a block carries them: a header
    /// word, then `width`-bit two's-complement fields low-bit-first into
    /// big-endian u16 words.
    fn block(width: u8, order: u8, channels: usize, residuals: &[i32]) -> Vec<u8> {
        let block_bytes = BLOCK_WORDS * 2 * channels;
        let mut out = Vec::with_capacity(block_bytes);
        out.extend_from_slice(&(u16::from(width) | (u16::from(order) << 5)).to_be_bytes());
        let mut reservoir: u64 = 0;
        let mut held = 0u32;
        for &value in residuals {
            let masked = (value as i64 as u64) & ((1u64 << width) - 1);
            reservoir |= masked << held;
            held += u32::from(width);
            while held >= 16 {
                out.extend_from_slice(&((reservoir & 0xffff) as u16).to_be_bytes());
                reservoir >>= 16;
                held -= 16;
            }
        }
        if held > 0 {
            out.extend_from_slice(&((reservoir & 0xffff) as u16).to_be_bytes());
        }
        out.resize(block_bytes, 0);
        out
    }

    /// The record fields [`decode`] reads, over a span the caller built.
    fn stroke<'a>(audio: &'a [u8], frames: u32, blocks: u16, seeds: [i16; 4]) -> Stroke<'a> {
        let mut record = [0u8; RECORD];
        record[REC_FRAMES..REC_FRAMES + 4].copy_from_slice(&frames.to_be_bytes());
        record[REC_BLOCKS..REC_BLOCKS + 2].copy_from_slice(&blocks.to_be_bytes());
        for (i, &seed) in seeds.iter().enumerate() {
            let at = REC_SEEDS + i * 2;
            record[at..at + 2].copy_from_slice(&seed.to_be_bytes());
        }
        Stroke {
            root: 0,
            record,
            audio: std::borrow::Cow::Borrowed(audio),
        }
    }

    /// Frames one block of `width` holds, the overlap included.
    fn frames_per_block(width: u8, channels: usize) -> usize {
        block_frames(width, BLOCK_WORDS * 2 * channels, channels)
    }

    #[test]
    fn order_zero_states_the_samples_outright() {
        let frames = frames_per_block(8, 1);
        let residuals: Vec<i32> = (0..frames).map(|i| (i % 61) as i32 - 30).collect();
        let audio = block(8, 0, 1, &residuals);
        let decoded = decode(&stroke(&audio, (frames - OVERLAP) as u32, 1, [0; 4]), 1).unwrap();
        assert_eq!(decoded.frames(), frames - OVERLAP);
        assert_eq!(&decoded.lanes[0][..4], &[-30, -29, -28, -27]);
        assert_eq!(decoded.clipped, 0);
    }

    #[test]
    fn order_one_integrates_from_the_records_newest_seed() {
        let frames = frames_per_block(6, 1);
        let audio = block(6, 1, 1, &vec![3i32; frames]);
        let decoded = decode(
            &stroke(&audio, (frames - OVERLAP) as u32, 1, [0, 0, 0, 100]),
            1,
        )
        .unwrap();
        assert_eq!(&decoded.lanes[0][..4], &[103, 106, 109, 112]);
    }

    #[test]
    fn a_width_the_header_cannot_carry_is_refused() {
        let audio = vec![0u8; BLOCK_WORDS * 2];
        let error = decode(&stroke(&audio, 1, 1, [0; 4]), 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("width 0"), "{error}");
    }

    #[test]
    fn a_frame_count_the_blocks_do_not_own_is_refused() {
        let audio = block(8, 0, 1, &[0i32; 16]);
        let error = decode(&stroke(&audio, 7, 1, [0; 4]), 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("the record states 7"), "{error}");
    }

    /// The frame count comes from the file, so the stroke's blocks bound it before any
    /// memory is reserved.
    #[test]
    fn a_frame_count_larger_than_the_blocks_can_hold_is_refused_before_reserving() {
        let audio = block(8, 0, 1, &[0i32; 16]);
        let error = decode(&stroke(&audio, u32::MAX, 1, [0; 4]), 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("the blocks own at most"), "{error}");
        assert!(
            error.contains(&format!("the record states {}", u32::MAX)),
            "{error}"
        );
    }

    #[test]
    fn a_channel_count_no_library_states_is_refused() {
        let audio = block(8, 0, 1, &[0i32; 16]);
        let error = decode(&stroke(&audio, 1, 1, [0; 4]), 0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("1 or 2"), "{error}");
    }

    #[test]
    fn a_span_shorter_than_its_block_count_is_refused() {
        let audio = block(8, 0, 1, &[0i32; 16]);
        let error = decode(&stroke(&audio, 1, 2, [0; 4]), 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("2 blocks hold"), "{error}");
    }

    #[test]
    fn a_block_that_does_not_repeat_the_one_before_is_refused() {
        let frames = frames_per_block(8, 1);
        let first: Vec<i32> = (0..frames).map(|i| (i % 7) as i32).collect();
        // The next block must open with the previous block's last OVERLAP frames;
        // this one opens with zeros.
        let mut audio = block(8, 0, 1, &first);
        audio.extend(block(8, 0, 1, &vec![0i32; frames]));
        let error = decode(&stroke(&audio, 2 * (frames - OVERLAP) as u32, 2, [0; 4]), 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("repeats frame"), "{error}");
    }

    #[test]
    fn a_block_repeating_the_one_before_decodes_and_emits_it_once() {
        let frames = frames_per_block(8, 1);
        let owned = frames - OVERLAP;
        let first: Vec<i32> = (0..frames).map(|i| (i % 7) as i32).collect();
        let mut second = vec![0i32; frames];
        second[..OVERLAP].copy_from_slice(&first[owned..]);
        let mut audio = block(8, 0, 1, &first);
        audio.extend(block(8, 0, 1, &second));
        let decoded = decode(&stroke(&audio, 2 * owned as u32, 2, [0; 4]), 1).unwrap();
        assert_eq!(decoded.frames(), 2 * owned);
        assert_eq!(decoded.overlap_checked, OVERLAP);
        // The repeat is emitted once, by the block that repeats it, so the two
        // blocks' frames run on continuously.
        let repeated: Vec<i16> = first[owned..].iter().map(|&v| v as i16).collect();
        assert_eq!(&decoded.lanes[0][owned..owned + OVERLAP], &repeated[..]);
        assert_eq!(decoded.lanes[0][owned + OVERLAP], 0);
    }

    #[test]
    fn a_stereo_block_alternates_channels_field_by_field() {
        let frames = frames_per_block(8, 2);
        let residuals: Vec<i32> = (0..frames * 2)
            .map(|i| if i % 2 == 0 { 10 } else { -10 })
            .collect();
        let audio = block(8, 0, 2, &residuals);
        let decoded = decode(&stroke(&audio, (frames - OVERLAP) as u32, 1, [0; 4]), 2).unwrap();
        assert_eq!(decoded.lanes.len(), 2);
        assert!(decoded.lanes[0].iter().all(|&s| s == 10));
        assert!(decoded.lanes[1].iter().all(|&s| s == -10));
        assert_eq!(decoded.interleaved()[..4], [10, -10, 10, -10]);
    }
}
