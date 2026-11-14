//! Reading the WAVs `piano build` takes, and refusing a bad one by name.
//!
//! The format sets what a stroke can hold, so checking each file as it is read names the
//! one that is wrong. The sample encoders check theirs with
//! [`nord_format::formats::nsmpproj::build::source_pcm`].

use std::path::Path;

use nord_format::wav::{self, Audio, Marks, Pcm16, Settings};

/// ⚠️ A stereo file becomes a stereo stroke, with both channels under one header.
/// Neither format has a stroke that holds more than two channels.
fn stroke_channels(channels: u16) -> Result<(), String> {
    match channels {
        1 | 2 => Ok(()),
        _ => Err(format!(
            "{channels} channels, but a stroke holds one or two"
        )),
    }
}

/// One WAV as an encoder takes it: 16-bit PCM, mono or stereo.
pub fn pcm16(path: &Path) -> Result<Pcm16, String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| named(&e))?;
    let source = wav::read_pcm16(&bytes).map_err(|e| named(&e))?;
    stroke_channels(source.channels).map_err(|e| named(&e))?;
    Ok(source)
}

/// A take as a build reads it: every sample, mono or stereo, the regions and loops it
/// declares, and the settings the take editor saved in it.
pub fn take(path: &Path) -> Result<(Audio, Marks, Settings), String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| named(&e))?;
    let audio = wav::read(&bytes).map_err(|e| named(&e))?;
    stroke_channels(audio.channels).map_err(|e| named(&e))?;
    let marks = wav::read_marks(&bytes).map_err(|e| named(&e))?;
    let settings = wav::read_settings(&bytes).map_err(|e| named(&e))?;
    Ok((audio, marks, settings))
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
        let quad = wav::pcm16(&[0i16; 16], 44_100, 4).unwrap();
        std::fs::write(&path, quad).unwrap();

        for err in [pcm16(&path).unwrap_err(), take(&path).unwrap_err()] {
            assert!(err.contains("quad.wav"), "{err}");
            assert!(err.contains("4 channels"), "{err}");
        }

        let stereo = wav::pcm16(&[0i16; 16], 44_100, 2).unwrap();
        std::fs::write(&path, stereo).unwrap();
        assert_eq!(pcm16(&path).unwrap().channels, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
