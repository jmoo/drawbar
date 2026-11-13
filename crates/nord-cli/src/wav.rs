//! Reading the WAVs `piano build` takes, and refusing a bad one by name.
//!
//! The format sets what a stroke can hold, so checking each file as it is read names the
//! one that is wrong. The sample encoders check theirs with
//! [`nord_format::formats::nsmpproj::build::source_pcm`].

use std::path::Path;

use nord_format::formats::npno::Bank;
use nord_format::take::sustain;
use nord_format::wav::{self, Audio, Loop, Marks, Pcm16, Settings};

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

/// `frames`, a region of `take` holding a stroke of `bank`, shaped, raised by `gain`
/// and dithered to 16-bit PCM with the noise `seed` picks, with `sample_loop` counted
/// from its start.
///
/// An attack fades in over 0.3 ms, and a release over 5 ms, fading out over at most
/// 40 ms. A looped region's loop is evened out and crossfaded so its end meets its
/// start. A region that fades from frame `fade` fades to silence at its end and loops
/// on silence after it.
pub fn cut(
    take: &Audio,
    frames: &[f32],
    bank: Bank,
    sample_loop: Option<Loop>,
    fade: Option<u32>,
    gain: f32,
    seed: u64,
) -> Pcm16 {
    let channels = usize::from(take.channels.max(1));
    let (mut shaped, sample_loop) = sustain::shaped(
        frames,
        channels,
        take.rate,
        bank == Bank::Release,
        sample_loop,
        fade,
    );
    for v in &mut shaped {
        *v *= gain;
    }
    Pcm16 {
        rate: take.rate,
        channels: take.channels,
        samples: wav::dithered(&shaped, seed),
        sample_loop,
    }
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
