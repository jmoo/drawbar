//! Uncompressed 16-bit PCM, the encoding the Nord codecs take and give, as integers.

use std::ops::Range;

use super::marks::read_loops;
use super::{chunks, layout, only, riff, to_int, Encoding, Format, Loop};
use crate::Error;

/// Uncompressed 16-bit PCM read off a RIFF/WAVE file.
#[derive(Debug)]
pub struct Pcm16 {
    pub rate: u32,
    pub channels: u16,
    /// Frames interleaved by channel, as stored.
    pub samples: Vec<i16>,
    /// The first loop a `smpl` chunk declares, if there is one.
    pub sample_loop: Option<Loop>,
}

impl Pcm16 {
    /// Frames, whatever the channel count.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels).max(1)
    }
}

/// Where a 16-bit PCM file keeps its samples, and how they are laid out, found without
/// copying them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pcm16Layout {
    pub rate: u32,
    pub channels: u16,
    /// The data chunk's body: frames interleaved by channel, each sample little-endian.
    /// Every byte outside it belongs to the container.
    pub data: Range<usize>,
    /// The first loop a `smpl` chunk declares, if there is one.
    pub sample_loop: Option<Loop>,
}

impl Pcm16Layout {
    /// Frames, whatever the channel count.
    pub fn frames(&self) -> usize {
        self.data.len() / (2 * usize::from(self.channels).max(1))
    }
}

/// Write interleaved 16-bit PCM at `rate` without resampling.
pub fn pcm16(samples: &[i16], rate: u32, channels: u16) -> Result<Vec<u8>, Error> {
    let format = Format {
        channels,
        rate,
        encoding: Encoding::Int { bits: 16 },
    };
    riff(format, samples, |out, s| {
        out.extend_from_slice(&s.to_le_bytes())
    })
}

/// Samples as 16-bit PCM stores them: rounded, and clipped to its range.
pub fn quantize(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&x| to_int(x, 32_768.0) as i16)
        .collect()
}

/// Write mono 16-bit PCM at `rate` without resampling.
pub fn mono_pcm16(samples: &[i16], rate: u32) -> Result<Vec<u8>, Error> {
    pcm16(samples, rate, 1)
}

/// Check a file as [`read_pcm16`] does, and say where its samples are.
pub fn pcm16_layout(bytes: &[u8]) -> Result<Pcm16Layout, Error> {
    let found = layout(bytes)?;
    match found.format.encoding {
        Encoding::Int { bits: 16 } => {}
        Encoding::Int { bits } => {
            return Err(Error::Unsupported(format!(
                "{bits}-bit samples; only 16-bit PCM is read"
            )))
        }
        Encoding::Float { bits } => {
            return Err(Error::Unsupported(format!(
                "{bits}-bit float samples; only 16-bit PCM is read"
            )))
        }
    }
    let frames = found.frames();
    let sample_loop = match only(&chunks(bytes)?, b"smpl")? {
        Some(smpl) => read_loops(&bytes[smpl.body.clone()], frames)?
            .first()
            .copied(),
        None => None,
    };
    Ok(Pcm16Layout {
        rate: found.format.rate,
        channels: found.format.channels,
        data: found.data,
        sample_loop,
    })
}

/// Read uncompressed 16-bit PCM, preserving its stored channel count and rate.
pub fn read_pcm16(bytes: &[u8]) -> Result<Pcm16, Error> {
    let layout = pcm16_layout(bytes)?;
    let samples = bytes[layout.data]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&s| i16::from_le_bytes(s))
        .collect();
    Ok(Pcm16 {
        rate: layout.rate,
        channels: layout.channels,
        samples,
        sample_loop: layout.sample_loop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::{with_marks, write, Audio, Marks, Play};

    #[test]
    fn what_this_writes_it_reads_back() {
        let want = [0i16, 1, -1, i16::MIN, 12_345];
        let read = read_pcm16(&mono_pcm16(&want, 44_100).unwrap()).unwrap();
        assert_eq!((read.rate, read.channels, read.frames()), (44_100, 1, 5));
        assert_eq!(read.samples, want);

        let stereo = [1, -1, 2, -2];
        let read = read_pcm16(&pcm16(&stereo, 35_002, 2).unwrap()).unwrap();
        assert_eq!((read.rate, read.channels, read.frames()), (35_002, 2, 2));
        assert_eq!(read.samples, stereo);
    }

    #[test]
    fn the_header_describes_the_samples_that_follow() {
        let wav = mono_pcm16(&[0, 1, -1, i16::MIN], 35_002).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 44);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 35_002);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(i16::from_le_bytes(wav[48..50].try_into().unwrap()), -1);
    }

    /// `wav` with `chunk` spliced in ahead of its data chunk.
    fn before_data(wav: &[u8], chunk: &[u8]) -> Vec<u8> {
        let mut out = wav.to_vec();
        out.splice(36..36, chunk.iter().copied());
        let riff = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&riff.to_le_bytes());
        out
    }

    #[test]
    fn the_layout_points_at_the_samples_past_any_chunk_before_them() {
        let extra = b"LIST\x03\x00\x00\x00abc\x00"; // pads 3 to 4
        let wav = before_data(&pcm16(&[7i16, 8, -7, -8], 44_100, 2).unwrap(), extra);
        let layout = pcm16_layout(&wav).unwrap();
        assert_eq!(
            (layout.rate, layout.channels, layout.frames()),
            (44_100, 2, 2)
        );
        assert_eq!(layout.data, 44 + extra.len()..wav.len());
        assert_eq!(read_pcm16(&wav).unwrap().samples, [7, 8, -7, -8]);
    }

    #[test]
    fn a_data_chunk_before_the_fmt_chunk_still_reads() {
        let wav = mono_pcm16(&[7i16, -8], 44_100).unwrap();
        let mut swapped = wav[..12].to_vec();
        swapped.extend_from_slice(&wav[36..]);
        swapped.extend_from_slice(&wav[12..36]);
        assert_eq!(read_pcm16(&swapped).unwrap().samples, [7, -8]);
    }

    #[test]
    fn trailing_bytes_that_form_no_chunk_are_refused() {
        let mut trailing = mono_pcm16(&[1i16], 44_100).unwrap();
        trailing.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let riff = (trailing.len() - 8) as u32;
        trailing[4..8].copy_from_slice(&riff.to_le_bytes());
        let err = read_pcm16(&trailing).unwrap_err().to_string();
        assert!(
            err.contains("trailing"),
            "refused for the wrong reason: {err}"
        );
    }

    #[test]
    fn anything_but_sixteen_bit_pcm_is_refused_by_name() {
        let audio = Audio {
            rate: 44_100,
            channels: 1,
            samples: vec![0.0; 2],
        };
        for (encoding, reason) in [
            (Encoding::Int { bits: 24 }, "24-bit samples"),
            (Encoding::Float { bits: 32 }, "32-bit float samples"),
        ] {
            let err = read_pcm16(&write(&audio, encoding).unwrap()).unwrap_err();
            assert!(
                err.to_string().contains(reason),
                "want {reason:?}, got {err}"
            );
        }
        assert!(read_pcm16(b"not a wav at all").is_err());
    }

    #[test]
    fn quantized_samples_are_what_a_sixteen_bit_file_stores() {
        let audio = Audio {
            rate: 44_100,
            channels: 1,
            samples: vec![0.0, 0.25, -0.5, 1.0, -1.5, 1.0 / 65_536.0, 0.7],
        };
        let stored = read_pcm16(&write(&audio, Encoding::Int { bits: 16 }).unwrap()).unwrap();
        assert_eq!(quantize(&audio.samples), stored.samples);
        assert_eq!(quantize(&[1.0, -1.5]), [i16::MAX, i16::MIN]);
    }

    #[test]
    fn the_first_loop_a_sampler_chunk_declares_is_read() {
        let wav = mono_pcm16(&[0; 100], 44_100).unwrap();
        assert_eq!(read_pcm16(&wav).unwrap().sample_loop, None);

        let first = Loop {
            start: 20,
            end: 79,
            play: Play::Forward,
        };
        let second = Loop {
            start: 0,
            end: 9,
            play: Play::Alternating,
        };
        let marks = |loops: Vec<Loop>| Marks {
            regions: vec![],
            loops,
        };
        let looped = with_marks(&wav, &marks(vec![first, second])).unwrap();
        assert_eq!(read_pcm16(&looped).unwrap().sample_loop, Some(first));
        let none = with_marks(&looped, &marks(vec![])).unwrap();
        assert_eq!(read_pcm16(&none).unwrap().sample_loop, None);
    }
}
