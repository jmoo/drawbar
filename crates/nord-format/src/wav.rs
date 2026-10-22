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
    if !samples.len().is_multiple_of(usize::from(channels)) {
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

    let mut out = Vec::new();
    out.try_reserve(44 + data as usize)
        .map_err(|_| ParseError::OutOfBounds {
            value: format!("{} samples", samples.len()),
            bound: "a WAV whose allocation fits memory".into(),
        })?;
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

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// The length of the extensible extension: valid bits, channel mask and sub-format GUID.
const EXTENSIBLE_CB_SIZE: u16 = 22;
/// `KSDATAFORMAT_SUBTYPE_PCM`, 00000001-0000-0010-8000-00aa00389b71, as stored.
const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

/// The fmt chunk fields the reader checks, once its encoding is known to be PCM.
struct Fmt {
    channels: u16,
    rate: u32,
    byte_rate: u32,
    block: u16,
    bits: u16,
}

/// Read a fmt chunk that declares PCM, plainly or as `WAVE_FORMAT_EXTENSIBLE` with the
/// PCM sub-format.
fn read_fmt(fmt: &[u8]) -> Result<Fmt, Error> {
    let u16_at = |at: usize| u16::from_le_bytes([fmt[at], fmt[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes([fmt[at], fmt[at + 1], fmt[at + 2], fmt[at + 3]]);

    if fmt.len() < 16 {
        return Err(ParseError::AssertFail(format!(
            "fmt chunk is {} bytes; PCM requires at least 16",
            fmt.len()
        ))
        .into());
    }
    let read = Fmt {
        channels: u16_at(2),
        rate: u32_at(4),
        byte_rate: u32_at(8),
        block: u16_at(12),
        bits: u16_at(14),
    };
    match u16_at(0) {
        WAVE_FORMAT_PCM => Ok(read),
        WAVE_FORMAT_EXTENSIBLE => {
            check_extensible(fmt, read.bits)?;
            Ok(read)
        }
        encoding => Err(ParseError::AssertFail(format!(
            "encoding {encoding} is not uncompressed PCM; only PCM (1) and extensible \
             PCM (65534) are read"
        ))
        .into()),
    }
}

/// Check the extension of a `WAVE_FORMAT_EXTENSIBLE` fmt chunk of at least 16 bytes.
fn check_extensible(fmt: &[u8], bits: u16) -> Result<(), Error> {
    let refuse = |why: String| Err(ParseError::AssertFail(why).into());
    let Some(cb_size) = fmt.get(16..18) else {
        return refuse(format!(
            "the extensible fmt chunk is {} bytes, too short to declare its extension",
            fmt.len()
        ));
    };
    let cb_size = u16::from_le_bytes([cb_size[0], cb_size[1]]);
    if cb_size != EXTENSIBLE_CB_SIZE {
        return refuse(format!(
            "the extensible fmt chunk declares a {cb_size}-byte extension; \
             the extension is {EXTENSIBLE_CB_SIZE} bytes"
        ));
    }
    let Some(extension) = fmt.get(18..18 + usize::from(EXTENSIBLE_CB_SIZE)) else {
        return refuse(format!(
            "the extensible fmt chunk is {} bytes, too short for its {EXTENSIBLE_CB_SIZE}-byte \
             extension",
            fmt.len()
        ));
    };
    let sub_format: [u8; 16] = std::array::from_fn(|i| extension[6 + i]);
    if sub_format != KSDATAFORMAT_SUBTYPE_PCM {
        return refuse(format!(
            "the extensible sub-format {} is not PCM; only PCM ({}) is read",
            guid(&sub_format),
            guid(&KSDATAFORMAT_SUBTYPE_PCM)
        ));
    }
    let valid_bits = u16::from_le_bytes([extension[0], extension[1]]);
    if valid_bits == 0 || valid_bits > bits {
        return refuse(format!(
            "the extensible fmt chunk declares {valid_bits} valid bits in a {bits}-bit sample"
        ));
    }
    Ok(())
}

/// A stored GUID in its registry spelling: three little-endian fields, then eight bytes.
fn guid(stored: &[u8; 16]) -> String {
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!(
        "{:08x}-{:04x}-{:04x}-{}-{}",
        u32::from_le_bytes([stored[0], stored[1], stored[2], stored[3]]),
        u16::from_le_bytes([stored[4], stored[5]]),
        u16::from_le_bytes([stored[6], stored[7]]),
        hex(&stored[8..10]),
        hex(&stored[10..16])
    )
}

/// Read uncompressed 16-bit PCM, preserving its stored channel count and rate.
pub fn read_pcm16(bytes: &[u8]) -> Result<Pcm16, Error> {
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);

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
            b"fmt " => format = Some(body..end),
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

    let Some(format) = format else {
        return Err(ParseError::AssertFail("no fmt chunk".into()).into());
    };
    let Fmt {
        channels,
        rate,
        byte_rate,
        block,
        bits,
    } = read_fmt(&bytes[format])?;
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

    /// `wav` with its plain 16-byte fmt chunk rewritten as `WAVE_FORMAT_EXTENSIBLE`,
    /// followed by `extension` as its cbSize-prefixed tail.
    fn extensible(wav: &[u8], cb_size: u16, extension: &[u8]) -> Vec<u8> {
        let mut fmt = wav[20..36].to_vec();
        fmt[0..2].copy_from_slice(&0xFFFEu16.to_le_bytes());
        fmt.extend_from_slice(&cb_size.to_le_bytes());
        fmt.extend_from_slice(extension);
        let mut out = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        out.extend_from_slice(&u32::try_from(fmt.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(&wav[36..]);
        let riff = u32::try_from(out.len() - 8).unwrap();
        out[4..8].copy_from_slice(&riff.to_le_bytes());
        out
    }

    /// Valid bits, channel mask, then the sub-format GUID as stored.
    fn extension(valid_bits: u16, sub_format: [u8; 16]) -> Vec<u8> {
        let mut out = valid_bits.to_le_bytes().to_vec();
        out.extend_from_slice(&0x3u32.to_le_bytes());
        out.extend_from_slice(&sub_format);
        out
    }

    #[test]
    fn extensible_pcm_reads_as_plain_pcm_does() {
        let stereo = [1i16, -1, 2, -2];
        let wav = pcm16(&stereo, 44_100, 2).unwrap();
        let read = read_pcm16(&extensible(
            &wav,
            22,
            &extension(16, KSDATAFORMAT_SUBTYPE_PCM),
        ))
        .unwrap();
        assert_eq!((read.rate, read.channels), (44_100, 2));
        assert_eq!(read.samples, stereo);

        let twelve_bit = extensible(&wav, 22, &extension(12, KSDATAFORMAT_SUBTYPE_PCM));
        assert_eq!(read_pcm16(&twelve_bit).unwrap().samples, stereo);
    }

    #[test]
    fn extensible_formats_other_than_pcm_are_refused_by_sub_format() {
        let wav = mono_pcm16(&[1i16], 44_100).unwrap();
        let mut float = KSDATAFORMAT_SUBTYPE_PCM;
        float[0] = 3;
        let err = read_pcm16(&extensible(&wav, 22, &extension(16, float)))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("00000003-0000-0010-8000-00aa00389b71 is not PCM"),
            "refused for the wrong reason: {err}"
        );
    }

    #[test]
    fn a_malformed_extensible_fmt_chunk_is_refused() {
        let wav = mono_pcm16(&[1i16], 44_100).unwrap();
        let pcm = extension(16, KSDATAFORMAT_SUBTYPE_PCM);
        let cases = [
            (extensible(&wav, 0, &[]), "0-byte extension"),
            (
                extensible(&wav, 22, &pcm[..10]),
                "too short for its 22-byte",
            ),
            (
                extensible(&wav, 22, &extension(0, KSDATAFORMAT_SUBTYPE_PCM)),
                "0 valid bits",
            ),
            (
                extensible(&wav, 22, &extension(17, KSDATAFORMAT_SUBTYPE_PCM)),
                "17 valid bits",
            ),
        ];
        for (wav, reason) in cases {
            let err = read_pcm16(&wav).unwrap_err().to_string();
            assert!(err.contains(reason), "want {reason:?}, got {err}");
        }

        let mut no_cb_size = extensible(&wav, 0, &[]);
        no_cb_size.drain(36..38);
        no_cb_size[16..20].copy_from_slice(&16u32.to_le_bytes());
        let riff = u32::try_from(no_cb_size.len() - 8).unwrap();
        no_cb_size[4..8].copy_from_slice(&riff.to_le_bytes());
        let err = read_pcm16(&no_cb_size).unwrap_err().to_string();
        assert!(err.contains("too short to declare"), "{err}");
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

        // An odd-sized chunk pads to even, and the pad byte is missing here: the data
        // chunk claims 3 bytes of the 3 that follow it, leaving no room for the pad.
        let mut no_pad = mono_pcm16(&[1], 44_100).unwrap();
        no_pad.push(0);
        no_pad[4..8].copy_from_slice(&39u32.to_le_bytes());
        no_pad[40..44].copy_from_slice(&3u32.to_le_bytes());
        let err = read_pcm16(&no_pad).unwrap_err().to_string();
        assert!(
            err.contains("pad byte"),
            "refused for the wrong reason: {err}"
        );

        let mut stereo_half_frame = mono_pcm16(&[1], 44_100).unwrap();
        stereo_half_frame[22..24].copy_from_slice(&2u16.to_le_bytes());
        stereo_half_frame[28..32].copy_from_slice(&176_400u32.to_le_bytes());
        stereo_half_frame[32..34].copy_from_slice(&4u16.to_le_bytes());
        assert!(read_pcm16(&stereo_half_frame).is_err());
    }

    /// The reader collects both chunks wherever they sit.
    #[test]
    fn a_data_chunk_before_the_fmt_chunk_still_reads() {
        let wav = mono_pcm16(&[7i16, -8], 44_100).unwrap();
        let mut swapped = wav[..12].to_vec();
        swapped.extend_from_slice(&wav[36..]); // the data chunk
        swapped.extend_from_slice(&wav[12..36]); // then the fmt chunk
        assert_eq!(swapped.len(), wav.len());
        assert_eq!(read_pcm16(&swapped).unwrap().samples, vec![7, -8]);
    }

    #[test]
    fn trailing_bytes_that_form_no_chunk_are_refused() {
        let mut trailing = mono_pcm16(&[1i16], 44_100).unwrap();
        trailing.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let declared = u32::from_le_bytes(trailing[4..8].try_into().unwrap()) + 4;
        trailing[4..8].copy_from_slice(&declared.to_le_bytes());
        let err = read_pcm16(&trailing).unwrap_err().to_string();
        assert!(
            err.contains("trailing"),
            "refused for the wrong reason: {err}"
        );
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
