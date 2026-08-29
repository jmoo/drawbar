//! A minimal RIFF/WAVE reader and writer, for moving audio in and out of the codec.

use crate::error::{Error, ParseError};

/// Write mono 16-bit PCM at `rate` without resampling.
pub fn mono_pcm16(samples: &[i16], rate: u32) -> Result<Vec<u8>, Error> {
    pcm16(samples, rate, 1)
}

/// Write interleaved 16-bit PCM at `rate` without resampling.
pub fn pcm16(samples: &[i16], rate: u32, channels: u16) -> Result<Vec<u8>, Error> {
    const BITS: u16 = 16;
    let block = channels
        .checked_mul(BITS / 8)
        .filter(|&bytes| bytes > 0)
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("{channels} channels"),
            bound: "a positive channel count whose frame size fits u16".into(),
        })?;
    if rate == 0 {
        return Err(ParseError::OutOfBounds {
            value: "0 Hz".into(),
            bound: "a positive sample rate".into(),
        }
        .into());
    }
    if samples.len() % usize::from(channels) != 0 {
        return Err(ParseError::OutOfBounds {
            value: format!("{} samples", samples.len()),
            bound: format!("whole {channels}-channel frames"),
        }
        .into());
    }
    let data = samples
        .len()
        .checked_mul(usize::from(BITS / 8))
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n <= u32::MAX - 36)
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("{} samples", samples.len()),
            bound: "a WAV whose RIFF and data lengths fit u32".into(),
        })?;
    let byte_rate = rate
        .checked_mul(u32::from(block))
        .ok_or_else(|| ParseError::OutOfBounds {
            value: format!("{rate} Hz"),
            bound: "a WAV byte rate that fits u32".into(),
        })?;

    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM, uncompressed
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&BITS.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    Ok(out)
}

/// Uncompressed 16-bit PCM read off a RIFF/WAVE file.
#[derive(Debug)]
pub struct Pcm16 {
    pub rate: u32,
    pub channels: u16,
    /// Frames interleaved by channel, as stored.
    pub samples: Vec<i16>,
}

impl Pcm16 {
    /// Frames, whatever the channel count.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels).max(1)
    }
}

/// Read uncompressed 16-bit PCM, preserving its stored channel count and rate.
pub fn read_pcm16(bytes: &[u8]) -> Result<Pcm16, Error> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());

    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(ParseError::AssertFail("not a RIFF/WAVE file".into()).into());
    }

    let declared = u32_at(4) as usize;
    let riff_len = declared
        .checked_add(8)
        .filter(|&n| n == bytes.len())
        .ok_or_else(|| {
            ParseError::AssertFail(format!(
                "RIFF declares {declared} payload bytes but the file is {} bytes",
                bytes.len()
            ))
        })?;
    let bytes = &bytes[..riff_len];
    let mut format = None;
    let mut data = None;
    let mut at = 12;
    while bytes.len() - at >= 8 {
        let id = &bytes[at..at + 4];
        let size = u32_at(at + 4) as usize;
        let body = at + 8;
        let end = body.checked_add(size).filter(|&e| e <= bytes.len());
        let Some(end) = end else {
            return Err(ParseError::AssertFail(format!(
                "chunk {} claims {size} bytes but the file ends first",
                String::from_utf8_lossy(id)
            ))
            .into());
        };
        match id {
            b"fmt " if size >= 16 => {
                format = Some((
                    u16_at(body),
                    u16_at(body + 2),
                    u32_at(body + 4),
                    u32_at(body + 8),
                    u16_at(body + 12),
                    u16_at(body + 14),
                ))
            }
            b"fmt " => {
                return Err(ParseError::AssertFail(format!(
                    "fmt chunk is {size} bytes; PCM requires at least 16"
                ))
                .into())
            }
            b"data" => data = Some(body..end),
            _ => {}
        }
        at = end
            .checked_add(size % 2)
            .filter(|&next| next <= bytes.len())
            .ok_or_else(|| ParseError::AssertFail("an odd-sized chunk has no pad byte".into()))?;
    }
    if at != bytes.len() {
        return Err(ParseError::AssertFail(format!(
            "{} trailing byte(s) do not form a chunk",
            bytes.len() - at
        ))
        .into());
    }

    let Some((encoding, channels, rate, byte_rate, block, bits)) = format else {
        return Err(ParseError::AssertFail("no fmt chunk".into()).into());
    };
    if encoding != 1 {
        return Err(ParseError::AssertFail(format!(
            "encoding {encoding} is not uncompressed PCM; only PCM (1) is read"
        ))
        .into());
    }
    if bits != 16 {
        return Err(
            ParseError::AssertFail(format!("{bits}-bit samples; only 16-bit PCM is read")).into(),
        );
    }
    if channels == 0 {
        return Err(ParseError::AssertFail("the fmt chunk declares no channels".into()).into());
    }
    if rate == 0 {
        return Err(
            ParseError::AssertFail("the fmt chunk declares a zero sample rate".into()).into(),
        );
    }
    let expected_block = channels
        .checked_mul(bits / 8)
        .ok_or_else(|| ParseError::AssertFail("the channel frame size overflows u16".into()))?;
    let expected_rate = rate
        .checked_mul(u32::from(expected_block))
        .ok_or_else(|| ParseError::AssertFail("the byte rate overflows u32".into()))?;
    if block != expected_block || byte_rate != expected_rate {
        return Err(ParseError::AssertFail(format!(
            "fmt declares byte rate {byte_rate} and block size {block}; expected {expected_rate} and {expected_block}"
        ))
        .into());
    }
    let Some(data) = data else {
        return Err(ParseError::AssertFail("no data chunk".into()).into());
    };
    if data.len() % usize::from(expected_block) != 0 {
        return Err(ParseError::AssertFail(format!(
            "data chunk is {} bytes, not a whole {expected_block}-byte frame",
            data.len()
        ))
        .into());
    }

    let samples = bytes[data]
        .chunks_exact(2)
        .map(|s| i16::from_le_bytes([s[0], s[1]]))
        .collect();
    Ok(Pcm16 {
        rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_this_writes_it_reads_back() {
        let want = [0i16, 1, -1, i16::MIN, 12_345];
        let read = read_pcm16(&mono_pcm16(&want, 44_100).unwrap()).unwrap();
        assert_eq!(read.rate, 44_100);
        assert_eq!(read.channels, 1);
        assert_eq!(read.samples, want);
        assert_eq!(read.frames(), 5);

        let stereo = [1, -1, 2, -2];
        let read = read_pcm16(&pcm16(&stereo, 35_002, 2).unwrap()).unwrap();
        assert_eq!((read.rate, read.channels, read.frames()), (35_002, 2, 2));
        assert_eq!(read.samples, stereo);
    }

    #[test]
    fn a_chunk_before_the_data_is_walked_past() {
        let mut wav = mono_pcm16(&[7i16, 8], 44_100).unwrap();
        // A 3-byte LIST chunk, which pads to 4, spliced in ahead of the data chunk.
        let extra: Vec<u8> = b"LIST\x03\x00\x00\x00abc\x00".to_vec();
        wav.splice(36..36, extra.iter().copied());
        let size = u32::from_le_bytes(wav[4..8].try_into().unwrap()) + extra.len() as u32;
        wav[4..8].copy_from_slice(&size.to_le_bytes());
        assert_eq!(read_pcm16(&wav).unwrap().samples, vec![7, 8]);
    }

    #[test]
    fn anything_but_sixteen_bit_pcm_is_refused_by_name() {
        assert!(read_pcm16(b"not a wav at all").is_err());

        let mut wav = mono_pcm16(&[1i16], 44_100).unwrap();
        wav[34] = 24; // bits per sample
        assert!(read_pcm16(&wav).unwrap_err().to_string().contains("24-bit"));

        let mut wav = mono_pcm16(&[1i16], 44_100).unwrap();
        wav[20] = 3; // IEEE float
        assert!(read_pcm16(&wav).unwrap_err().to_string().contains("PCM"));

        let mut wav = mono_pcm16(&[1i16], 44_100).unwrap();
        wav[40..44].copy_from_slice(&999u32.to_le_bytes()); // data chunk overruns
        assert!(read_pcm16(&wav).is_err());
    }

    #[test]
    fn malformed_rates_and_frames_are_refused() {
        assert!(mono_pcm16(&[], 0).is_err());
        assert!(pcm16(&[], 44_100, 0).is_err());
        assert!(pcm16(&[1], 44_100, 2).is_err());

        let mut zero_rate = mono_pcm16(&[1], 44_100).unwrap();
        zero_rate[24..28].fill(0);
        zero_rate[28..32].fill(0);
        assert!(read_pcm16(&zero_rate).is_err());

        let mut partial = mono_pcm16(&[1], 44_100).unwrap();
        partial.push(0);
        partial[4..8].copy_from_slice(&37u32.to_le_bytes());
        partial[40..44].copy_from_slice(&3u32.to_le_bytes());
        assert!(read_pcm16(&partial).is_err());

        let mut stereo_half_frame = mono_pcm16(&[1], 44_100).unwrap();
        stereo_half_frame[22..24].copy_from_slice(&2u16.to_le_bytes());
        stereo_half_frame[28..32].copy_from_slice(&176_400u32.to_le_bytes());
        stereo_half_frame[32..34].copy_from_slice(&4u16.to_le_bytes());
        assert!(read_pcm16(&stereo_half_frame).is_err());
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
}
