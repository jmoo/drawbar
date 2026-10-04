//! WAV files: the extension they are kept under, and how their bytes are told apart.

/// The extension a WAV carries, and the tag it is kept under.
pub const EXTENSION: &str = "wav";

/// Whether these bytes are in a RIFF/WAVE container.
///
/// Tests the container only: a 24-bit WAV still opens as one, and its document says why
/// it does not read.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_riff_wave_container_is_a_wav_by_its_bytes() {
        assert!(is_wav(
            &nord_format::wav::mono_pcm16(&[0; 8], 44_100).unwrap()
        ));
        assert!(!is_wav(b"RIFF"));
        assert!(!is_wav(b"not a wav at all"));
        assert!(!is_wav(&[]));
    }
}
