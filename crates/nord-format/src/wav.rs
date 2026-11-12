//! RIFF/WAVE as a take arrives: integer PCM of 8 to 32 bits or IEEE float, plain or
//! `WAVE_FORMAT_EXTENSIBLE`, read to `f32`.

mod marks;
mod pcm16;

use std::ops::Range;

pub use marks::{read_marks, with_marks, Loop, Marks, Play, Region};
pub use pcm16::{
    dithered, mono_pcm16, pcm16, pcm16_layout, quantize, read_pcm16, Pcm16, Pcm16Layout,
};

use crate::error::{malformed, out_of_bounds, Error};

/// How each sample is stored. Integer samples of 8 bits are unsigned, wider ones signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Int { bits: u16 },
    Float { bits: u16 },
}

impl Encoding {
    fn bytes(self) -> usize {
        match self {
            Encoding::Int { bits } | Encoding::Float { bits } => usize::from(bits / 8),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub channels: u16,
    pub rate: u32,
    pub encoding: Encoding,
}

impl Format {
    fn block(&self) -> usize {
        usize::from(self.channels) * self.encoding.bytes()
    }
}

/// Where a file keeps its samples, found without copying them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub format: Format,
    /// The data chunk's body: frames interleaved by channel.
    pub data: Range<usize>,
}

impl Layout {
    pub fn frames(&self) -> usize {
        self.data.len() / self.format.block()
    }
}

/// Samples in -1.0..1.0, frames interleaved by channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Audio {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels).max(1)
    }
}

/// A top-level chunk: its id and where its body sits in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chunk {
    pub id: [u8; 4],
    pub body: Range<usize>,
}

impl Chunk {
    /// The chunk with its header and any pad byte, as it sits in the file.
    pub fn whole(&self) -> Range<usize> {
        self.body.start - 8..self.body.end + self.body.len() % 2
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The top-level chunks of a RIFF/WAVE file, in file order.
pub(crate) fn chunks(bytes: &[u8]) -> Result<Vec<Chunk>, Error> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return malformed("not a RIFF/WAVE file");
    }
    let declared = u32_at(bytes, 4) as usize;
    if declared.checked_add(8) != Some(bytes.len()) {
        return malformed(format!(
            "RIFF declares {declared} payload bytes but the file is {} bytes",
            bytes.len()
        ));
    }
    let mut found = Vec::new();
    let mut at = 12;
    while bytes.len() - at >= 8 {
        let id: [u8; 4] = std::array::from_fn(|i| bytes[at + i]);
        let size = u32_at(bytes, at + 4) as usize;
        let body = at + 8;
        let Some(end) = body.checked_add(size).filter(|&e| e <= bytes.len()) else {
            return malformed(format!(
                "chunk {} claims {size} bytes but the file ends first",
                String::from_utf8_lossy(&id)
            ));
        };
        found.push(Chunk {
            id,
            body: body..end,
        });
        at = end + size % 2;
        if at > bytes.len() {
            return malformed("an odd-sized chunk has no pad byte");
        }
    }
    if at != bytes.len() {
        return malformed(format!(
            "{} trailing byte(s) do not form a chunk",
            bytes.len() - at
        ));
    }
    Ok(found)
}

/// The one chunk of `id`, if there is one; a second is refused.
pub(crate) fn only<'a>(chunks: &'a [Chunk], id: &[u8; 4]) -> Result<Option<&'a Chunk>, Error> {
    let mut of_id = chunks.iter().filter(|c| &c.id == id);
    let first = of_id.next();
    if of_id.next().is_some() {
        return malformed(format!(
            "more than one {} chunk",
            String::from_utf8_lossy(id).trim_end()
        ));
    }
    Ok(first)
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// The length of the extensible extension: valid bits, channel mask and sub-format GUID.
const EXTENSIBLE_CB_SIZE: u16 = 22;
/// The tail every `KSDATAFORMAT_SUBTYPE_*` GUID shares; its first two bytes are the
/// format tag the sub-format stands for.
const SUBTYPE_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

/// The format tag a fmt chunk stands for, looking through an extensible one to its
/// sub-format.
fn format_tag(fmt: &[u8], bits: u16) -> Result<u16, Error> {
    let tag = u16_at(fmt, 0);
    if tag != WAVE_FORMAT_EXTENSIBLE {
        return Ok(tag);
    }
    let Some(cb_size) = fmt.get(16..18).map(|b| u16::from_le_bytes([b[0], b[1]])) else {
        return malformed(format!(
            "the extensible fmt chunk is {} bytes, too short to declare its extension",
            fmt.len()
        ));
    };
    if cb_size != EXTENSIBLE_CB_SIZE {
        return malformed(format!(
            "the extensible fmt chunk declares a {cb_size}-byte extension; \
             the extension is {EXTENSIBLE_CB_SIZE} bytes"
        ));
    }
    let Some(extension) = fmt.get(18..18 + usize::from(EXTENSIBLE_CB_SIZE)) else {
        return malformed(format!(
            "the extensible fmt chunk is {} bytes, too short for its {EXTENSIBLE_CB_SIZE}-byte \
             extension",
            fmt.len()
        ));
    };
    let valid_bits = u16_at(extension, 0);
    if valid_bits == 0 || valid_bits > bits {
        return malformed(format!(
            "the extensible fmt chunk declares {valid_bits} valid bits in a {bits}-bit sample"
        ));
    }
    let sub_format = &extension[6..22];
    if sub_format[2..] != SUBTYPE_TAIL {
        return malformed("the extensible sub-format is not a KSDATAFORMAT_SUBTYPE GUID");
    }
    Ok(u16_at(sub_format, 0))
}

fn read_format(fmt: &[u8]) -> Result<Format, Error> {
    if fmt.len() < 16 {
        return malformed(format!(
            "fmt chunk is {} bytes; it needs at least 16",
            fmt.len()
        ));
    }
    let channels = u16_at(fmt, 2);
    let rate = u32_at(fmt, 4);
    let byte_rate = u32_at(fmt, 8);
    let block = u16_at(fmt, 12);
    let bits = u16_at(fmt, 14);
    let encoding = match (format_tag(fmt, bits)?, bits) {
        (WAVE_FORMAT_PCM, 8 | 16 | 24 | 32) => Encoding::Int { bits },
        (WAVE_FORMAT_IEEE_FLOAT, 32 | 64) => Encoding::Float { bits },
        (tag @ (WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT), bits) => {
            return malformed(format!(
                "{bits}-bit samples in format {tag}; PCM is read at 8, 16, 24 or 32 bits, \
                 float at 32 or 64"
            ))
        }
        (tag, _) => {
            return malformed(format!(
                "format {tag} is neither PCM (1) nor IEEE float (3)"
            ))
        }
    };
    if channels == 0 {
        return malformed("the fmt chunk declares no channels");
    }
    if rate == 0 {
        return malformed("the fmt chunk declares a zero sample rate");
    }
    let format = Format {
        channels,
        rate,
        encoding,
    };
    let expected_block = format.block();
    let expected_rate = u64::from(rate) * expected_block as u64;
    if usize::from(block) != expected_block || u64::from(byte_rate) != expected_rate {
        return malformed(format!(
            "fmt declares {bits}-bit samples in {channels} channel(s) at byte rate \
             {byte_rate} and block size {block}; they take {expected_rate} and {expected_block}"
        ));
    }
    Ok(format)
}

/// Check a file as [`read`] does, and say where its samples are.
pub fn layout(bytes: &[u8]) -> Result<Layout, Error> {
    let chunks = chunks(bytes)?;
    let Some(fmt) = only(&chunks, b"fmt ")? else {
        return malformed("no fmt chunk");
    };
    let format = read_format(&bytes[fmt.body.clone()])?;
    let Some(data) = only(&chunks, b"data")? else {
        return malformed("no data chunk");
    };
    if data.body.len() % format.block() != 0 {
        return malformed(format!(
            "data chunk is {} bytes, not a whole {}-byte frame",
            data.body.len(),
            format.block()
        ));
    }
    Ok(Layout {
        format,
        data: data.body.clone(),
    })
}

/// Read every sample, whatever its encoding, at its stored rate and channel count.
pub fn read(bytes: &[u8]) -> Result<Audio, Error> {
    let layout = layout(bytes)?;
    let data = &bytes[layout.data.clone()];
    let width = layout.format.encoding.bytes();
    let decode: fn(&[u8]) -> f32 = match layout.format.encoding {
        Encoding::Int { bits: 8 } => |s: &[u8]| (f32::from(s[0]) - 128.0) / 128.0,
        Encoding::Int { bits: 16 } => {
            |s: &[u8]| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32_768.0
        }
        Encoding::Int { bits: 24 } => {
            |s: &[u8]| (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0
        }
        Encoding::Int { .. } => |s: &[u8]| {
            (f64::from(i32::from_le_bytes([s[0], s[1], s[2], s[3]])) / 2_147_483_648.0) as f32
        },
        Encoding::Float { bits: 32 } => |s: &[u8]| f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        Encoding::Float { .. } => {
            |s: &[u8]| f64::from_le_bytes(std::array::from_fn(|i| s[i])) as f32
        }
    };
    let samples = data.chunks_exact(width).map(decode).collect();
    Ok(Audio {
        rate: layout.format.rate,
        channels: layout.format.channels,
        samples,
    })
}

/// `x` on the integer scale whose full range is `full` each side, rounded and clipped.
fn to_int(x: f32, full: f64) -> i64 {
    (f64::from(x) * full).round().clamp(-full, full - 1.0) as i64
}

/// One sample stored as `encoding`. Integer encodings round and clip to their range;
/// float stores the value as it is.
fn put(out: &mut Vec<u8>, x: f32, encoding: Encoding) {
    let int = |full: f64| to_int(x, full);
    match encoding {
        Encoding::Int { bits: 8 } => out.push((int(128.0) + 128) as u8),
        Encoding::Int { bits: 16 } => out.extend_from_slice(&(int(32_768.0) as i16).to_le_bytes()),
        Encoding::Int { bits: 24 } => {
            out.extend_from_slice(&(int(8_388_608.0) as i32).to_le_bytes()[..3]);
        }
        Encoding::Int { .. } => {
            out.extend_from_slice(&(int(2_147_483_648.0) as i32).to_le_bytes());
        }
        Encoding::Float { bits: 32 } => out.extend_from_slice(&x.to_le_bytes()),
        Encoding::Float { .. } => out.extend_from_slice(&f64::from(x).to_le_bytes()),
    }
}

/// Write `audio` as a WAV of `encoding`: a fmt chunk, a `fact` chunk for float, and the
/// data.
pub fn write(audio: &Audio, encoding: Encoding) -> Result<Vec<u8>, Error> {
    let format = Format {
        channels: audio.channels,
        rate: audio.rate,
        encoding,
    };
    riff(format, &audio.samples, |out, x| put(out, x, encoding))
}

/// A WAV of `format` holding `samples`, frames interleaved by channel, each stored by
/// `store`.
pub(crate) fn riff<T: Copy>(
    format: Format,
    samples: &[T],
    store: impl Fn(&mut Vec<u8>, T),
) -> Result<Vec<u8>, Error> {
    let Format {
        channels,
        rate,
        encoding,
    } = format;
    match encoding {
        Encoding::Int {
            bits: 8 | 16 | 24 | 32,
        }
        | Encoding::Float { bits: 32 | 64 } => {}
        _ => return out_of_bounds(format!("{encoding:?}"), "an encoding this crate reads"),
    }
    if channels == 0 {
        return out_of_bounds("0 channels", "a positive channel count");
    }
    if rate == 0 {
        return out_of_bounds("0 Hz", "a positive sample rate");
    }
    if !samples.len().is_multiple_of(usize::from(channels)) {
        return out_of_bounds(
            format!("{} samples", samples.len()),
            format!("whole {}-channel frames", channels),
        );
    }
    let float = matches!(encoding, Encoding::Float { .. });
    let fmt_len: u32 = if float { 18 } else { 16 };
    let fact_len: u32 = if float { 12 } else { 0 };
    let header = 4 + 8 + fmt_len + fact_len + 8;
    let (Some(block), Some(frames)) = (
        u16::try_from(format.block()).ok(),
        u32::try_from(samples.len() / usize::from(channels)).ok(),
    ) else {
        return out_of_bounds(
            format!(
                "{} frames of {} channels",
                samples.len() / usize::from(channels),
                channels
            ),
            "a WAV whose frame size fits u16 and frame count u32",
        );
    };
    let data = u32::from(block)
        .checked_mul(frames)
        .filter(|&n| n < u32::MAX - header);
    let byte_rate = rate.checked_mul(u32::from(block));
    let (Some(data), Some(byte_rate)) = (data, byte_rate) else {
        return out_of_bounds(
            format!("{frames} frames at {} Hz", rate),
            "a WAV whose RIFF length and byte rate fit u32",
        );
    };

    let pad = data % 2;
    let mut out = Vec::with_capacity(8 + (header + data + pad) as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(header + data + pad).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    let tag = if float {
        WAVE_FORMAT_IEEE_FLOAT
    } else {
        WAVE_FORMAT_PCM
    };
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&(encoding.bytes() as u16 * 8).to_le_bytes());
    if float {
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(b"fact");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&frames.to_le_bytes());
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for &x in samples {
        store(&mut out, x);
    }
    if pad == 1 {
        out.push(0);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(channels: u16, samples: &[f32]) -> Audio {
        Audio {
            rate: 44_100,
            channels,
            samples: samples.to_vec(),
        }
    }

    const EVERY_ENCODING: [Encoding; 6] = [
        Encoding::Int { bits: 8 },
        Encoding::Int { bits: 16 },
        Encoding::Int { bits: 24 },
        Encoding::Int { bits: 32 },
        Encoding::Float { bits: 32 },
        Encoding::Float { bits: 64 },
    ];

    #[test]
    fn every_encoding_reads_back_what_it_wrote() {
        let want = [0.0, 0.5, -0.5, -1.0, 0.25, -0.25];
        for encoding in EVERY_ENCODING {
            let wav = write(&audio(2, &want), encoding).unwrap();
            let read = read(&wav).unwrap();
            assert_eq!((read.rate, read.channels, read.frames()), (44_100, 2, 3));
            assert_eq!(read.samples, want, "{encoding:?}");
            assert_eq!(layout(&wav).unwrap().format.encoding, encoding);
        }
    }

    #[test]
    fn twenty_four_bit_keeps_every_step() {
        let steps: Vec<f32> = [-8_388_608, -1, 0, 1, 12_345, 8_388_607]
            .iter()
            .map(|&s| s as f32 / 8_388_608.0)
            .collect();
        let wav = write(&audio(1, &steps), Encoding::Int { bits: 24 }).unwrap();
        assert_eq!(read(&wav).unwrap().samples, steps);
        assert_eq!(
            &wav[44..47],
            &[0, 0, 0x80],
            "-1.0 is stored as the lowest step"
        );
    }

    #[test]
    fn odd_length_data_is_padded_to_a_whole_chunk() {
        let wav = write(&audio(1, &[0.5]), Encoding::Int { bits: 24 }).unwrap();
        assert_eq!(wav.len(), 44 + 3 + 1);
        assert_eq!(read(&wav).unwrap().samples, [0.5]);
    }

    #[test]
    fn integer_writes_clip_to_their_range() {
        let wav = write(&audio(1, &[1.0, -1.5, 2.0]), Encoding::Int { bits: 16 }).unwrap();
        let ints: Vec<i16> = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&s| i16::from_le_bytes(s))
            .collect();
        assert_eq!(ints, [i16::MAX, i16::MIN, i16::MAX]);
    }

    #[test]
    fn a_float_file_carries_a_fact_chunk_counting_its_frames() {
        let wav = write(&audio(2, &[0.0; 6]), Encoding::Float { bits: 32 }).unwrap();
        let chunks = chunks(&wav).unwrap();
        let fact = only(&chunks, b"fact").unwrap().unwrap();
        assert_eq!(u32_at(&wav, fact.body.start), 3);
    }

    /// `wav` with its plain 16-byte fmt chunk rewritten as `WAVE_FORMAT_EXTENSIBLE`
    /// over `sub_tag`, declaring `valid_bits`.
    fn extensible(wav: &[u8], valid_bits: u16, sub_tag: u16) -> Vec<u8> {
        let mut fmt = wav[20..36].to_vec();
        fmt[0..2].copy_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        fmt.extend_from_slice(&EXTENSIBLE_CB_SIZE.to_le_bytes());
        fmt.extend_from_slice(&valid_bits.to_le_bytes());
        fmt.extend_from_slice(&0x3u32.to_le_bytes());
        fmt.extend_from_slice(&sub_tag.to_le_bytes());
        fmt.extend_from_slice(&SUBTYPE_TAIL);
        let mut out = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(&wav[36..]);
        let riff = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&riff.to_le_bytes());
        out
    }

    #[test]
    fn extensible_files_read_as_their_sub_format() {
        let want = [0.5, -0.25];
        let pcm = write(&audio(2, &want), Encoding::Int { bits: 24 }).unwrap();
        assert_eq!(read(&extensible(&pcm, 20, 1)).unwrap().samples, want);

        let float = write(&audio(2, &want), Encoding::Float { bits: 32 }).unwrap();
        let mut plain = float[..36].to_vec();
        plain[16..20].copy_from_slice(&16u32.to_le_bytes());
        plain.extend_from_slice(&float[50..]); // drop cbSize and the fact chunk
        let riff = (plain.len() - 8) as u32;
        plain[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(read(&extensible(&plain, 32, 3)).unwrap().samples, want);
    }

    #[test]
    fn encodings_this_crate_does_not_read_are_refused_by_name() {
        let wav = write(&audio(1, &[0.0, 0.0]), Encoding::Int { bits: 16 }).unwrap();
        let cases = [
            (extensible(&wav, 16, 2), "format 2"),
            (extensible(&wav, 17, 1), "17 valid bits"),
            (
                {
                    let mut w = wav.clone();
                    w[20] = 6; // A-law
                    w
                },
                "format 6",
            ),
            (
                {
                    let mut w = wav.clone();
                    w[20] = 3; // 16-bit float
                    w
                },
                "16-bit samples in format 3",
            ),
        ];
        for (wav, reason) in cases {
            let err = read(&wav).unwrap_err().to_string();
            assert!(err.contains(reason), "want {reason:?}, got {err}");
        }
    }

    #[test]
    fn malformed_files_are_refused() {
        let wav = write(&audio(1, &[0.0, 0.0]), Encoding::Int { bits: 16 }).unwrap();
        let edit = |at: usize, bytes: &[u8]| {
            let mut w = wav.clone();
            w[at..at + bytes.len()].copy_from_slice(bytes);
            w
        };
        let cases = [
            (b"not a wav at all".to_vec(), "not a RIFF/WAVE"),
            (edit(4, &99u32.to_le_bytes()), "RIFF declares"),
            (edit(40, &999u32.to_le_bytes()), "file ends first"),
            (
                {
                    let mut w = edit(40, &3u32.to_le_bytes());
                    w.pop();
                    let riff = w.len() as u32 - 8;
                    w[4..8].copy_from_slice(&riff.to_le_bytes());
                    w
                },
                "pad byte",
            ),
            (edit(22, &0u16.to_le_bytes()), "no channels"),
            (edit(24, &0u32.to_le_bytes()), "zero sample rate"),
            (edit(32, &4u16.to_le_bytes()), "block size 4"),
            (edit(36, b"junk"), "no data chunk"),
            (edit(12, b"junk"), "no fmt chunk"),
        ];
        for (wav, reason) in cases {
            let err = read(&wav).unwrap_err().to_string();
            assert!(err.contains(reason), "want {reason:?}, got {err}");
        }

        let mut two_data = wav.clone();
        two_data.extend_from_slice(&wav[36..]);
        let riff = (two_data.len() - 8) as u32;
        two_data[4..8].copy_from_slice(&riff.to_le_bytes());
        assert!(read(&two_data)
            .unwrap_err()
            .to_string()
            .contains("more than one data"));

        let stereo = edit(22, &2u16.to_le_bytes());
        let stereo = {
            let mut w = stereo;
            w[28..32].copy_from_slice(&(44_100u32 * 4).to_le_bytes());
            w[32..34].copy_from_slice(&4u16.to_le_bytes());
            w[40..44].copy_from_slice(&2u32.to_le_bytes());
            w.truncate(46);
            let riff = (w.len() - 8) as u32;
            w[4..8].copy_from_slice(&riff.to_le_bytes());
            w
        };
        assert!(read(&stereo)
            .unwrap_err()
            .to_string()
            .contains("whole 4-byte frame"));
    }

    #[test]
    fn writes_that_cannot_form_a_file_are_refused() {
        assert!(write(&audio(0, &[]), Encoding::Int { bits: 16 }).is_err());
        assert!(write(&audio(2, &[0.0]), Encoding::Int { bits: 16 }).is_err());
        assert!(write(&audio(1, &[0.0]), Encoding::Int { bits: 12 }).is_err());
        let silent = Audio {
            rate: 0,
            ..audio(1, &[])
        };
        assert!(write(&silent, Encoding::Int { bits: 16 }).is_err());
    }
}
