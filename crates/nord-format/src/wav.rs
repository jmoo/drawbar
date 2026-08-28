//! A minimal RIFF/WAVE reader and writer, for moving audio in and out of the codec.

use crate::error::{Error, ParseError};

/// A mono 16-bit PCM WAV file.
///
/// `rate` goes into the header unchanged: decoded audio comes off its own lattice
/// rather than a standard rate, and resampling it here would guess at an
/// interpolator the instrument has not given up.
pub fn mono_pcm16(samples: &[i16], rate: u32) -> Vec<u8> {
    const BITS: u16 = 16;
    const CHANNELS: u16 = 1;
    let block = CHANNELS * BITS / 8;
    let data = samples.len() * usize::from(block);

    let mut out = Vec::with_capacity(44 + data);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM, uncompressed
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&BITS.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
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

/// Reads a 16-bit PCM WAV.
///
/// Only uncompressed PCM at 16 bits is accepted — every other encoding, including
/// float, 8/24/32-bit, and the extensible header, is refused by name rather than
/// misread. Channel count and rate come back as stored; what to do with them is the
/// caller's policy.
pub fn read_pcm16(bytes: &[u8]) -> Result<Pcm16, Error> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());

    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(ParseError::AssertFail("not a RIFF/WAVE file".into()).into());
    }

    let mut format = None;
    let mut data = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
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
                    u16_at(body + 14),
                ))
            }
            b"data" => data = Some(body..end),
            _ => {}
        }
        // Chunks are padded to an even length, and the pad is not in the size.
        at = end + end % 2;
    }

    let Some((encoding, channels, rate, bits)) = format else {
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
    let Some(data) = data else {
        return Err(ParseError::AssertFail("no data chunk".into()).into());
    };

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
        let read = read_pcm16(&mono_pcm16(&want, 44_100)).unwrap();
        assert_eq!(read.rate, 44_100);
        assert_eq!(read.channels, 1);
        assert_eq!(read.samples, want);
        assert_eq!(read.frames(), 5);
    }

    #[test]
    fn a_chunk_before_the_data_is_walked_past() {
        let mut wav = mono_pcm16(&[7i16, 8], 44_100);
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

        let mut wav = mono_pcm16(&[1i16], 44_100);
        wav[34] = 24; // bits per sample
        assert!(read_pcm16(&wav).unwrap_err().to_string().contains("24-bit"));

        let mut wav = mono_pcm16(&[1i16], 44_100);
        wav[20] = 3; // IEEE float
        assert!(read_pcm16(&wav).unwrap_err().to_string().contains("PCM"));

        let mut wav = mono_pcm16(&[1i16], 44_100);
        wav[40..44].copy_from_slice(&999u32.to_le_bytes()); // data chunk overruns
        assert!(read_pcm16(&wav).is_err());
    }

    #[test]
    fn the_header_describes_the_samples_that_follow() {
        let wav = mono_pcm16(&[0, 1, -1, i16::MIN], 35_002);
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
