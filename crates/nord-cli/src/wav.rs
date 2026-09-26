//! Reading the WAVs the encoders take, and refusing a bad one by name.
//!
//! `sample encode`, `sample build` and `piano build` all turn recordings into encoded
//! strokes, and the formats set what a stroke can hold. Checking each file as it is read
//! names the one that is wrong.

use std::path::Path;

use nord_format::wav::Pcm16;

/// One WAV as an encoder takes it: 16-bit PCM, mono or stereo.
///
/// ⚠️ A stereo file becomes a stereo stroke, with both channels under one header.
/// Neither format has a stroke that holds more than two channels.
pub fn pcm16(path: &Path) -> Result<Pcm16, String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| named(&e))?;
    let source = nord_format::wav::read_pcm16(&bytes).map_err(|e| named(&e))?;
    if source.channels != 1 && source.channels != 2 {
        return Err(named(&format!(
            "{} channels, but a stroke holds one or two",
            source.channels
        )));
    }
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The channel count is refused as the file is read, not after a build has taken
    /// every other WAV in the directory.
    #[test]
    fn a_wav_with_more_than_two_channels_is_refused_by_name() {
        let dir = crate::edit::tests::scratch("wav-channels");
        let path = dir.join("quad.wav");
        let quad = nord_format::wav::pcm16(&[0i16; 16], 44_100, 4).unwrap();
        std::fs::write(&path, quad).unwrap();

        let err = pcm16(&path).unwrap_err();
        assert!(err.contains("quad.wav"), "{err}");
        assert!(err.contains("4 channels"), "{err}");

        let stereo = nord_format::wav::pcm16(&[0i16; 16], 44_100, 2).unwrap();
        std::fs::write(&path, stereo).unwrap();
        assert_eq!(pcm16(&path).unwrap().channels, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
