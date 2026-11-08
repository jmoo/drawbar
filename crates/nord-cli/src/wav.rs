//! Reading the WAVs `piano build` takes, and refusing a bad one by name.
//!
//! The format sets what a stroke can hold, so checking each file as it is read names the
//! one that is wrong. The sample encoders check theirs with
//! [`nord_format::formats::nsmpproj::build::source_pcm`].

use std::path::Path;

use nord_format::formats::npno::Bank;
use npno_prep::sustain::{self, SustainOptions};
use npno_prep::wav::{self, Audio, Loop, Marks, Pcm16, Region};

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

/// A take as a build reads it: every sample, mono or stereo, and the regions and loops
/// it declares.
pub fn take(path: &Path) -> Result<(Audio, Marks), String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| named(&e))?;
    let audio = wav::read(&bytes).map_err(|e| named(&e))?;
    stroke_channels(audio.channels).map_err(|e| named(&e))?;
    let marks = wav::read_marks(&bytes).map_err(|e| named(&e))?;
    Ok((audio, marks))
}

/// The frames of `region`, the take's samples within it.
/// The region lies within the take, as [`wav::read_marks`] checks.
pub fn frames<'a>(take: &'a Audio, region: &Region) -> &'a [f32] {
    let channels = usize::from(take.channels);
    let start = region.start as usize * channels;
    &take.samples[start..start + region.frames as usize * channels]
}

/// The frames of `region`, a stroke of `bank`, shaped, raised by `gain` and rounded to
/// 16-bit PCM, with `sample_loop` counted from its start.
///
/// An attack fades in over 0.3 ms, and a release over 5 ms, fading out over at most
/// 40 ms. A looped region's loop is evened out and crossfaded so its end meets its
/// start. A region that fades from frame `fade` fades to silence at its end and loops
/// on silence after it.
pub fn cut(
    take: &Audio,
    region: &Region,
    bank: Bank,
    sample_loop: Option<Loop>,
    fade: Option<u32>,
    gain: f32,
) -> Pcm16 {
    let channels = usize::from(take.channels.max(1));
    let options = SustainOptions::default();
    let mut shaped = frames(take, region).to_vec();
    let (fade_in, fade_out) = match bank {
        Bank::Release => (
            sustain::RELEASE_FADE_IN,
            sustain::RELEASE_FADE_OUT.min(f64::from(region.frames) / f64::from(take.rate) / 2.0),
        ),
        Bank::Attack | Bank::Resonance => (sustain::ATTACK_FADE_IN, 0.0),
    };
    sustain::fade_edges(&mut shaped, channels, take.rate, fade_in, fade_out);
    let mut sample_loop = sample_loop;
    match (&sample_loop, fade) {
        (Some(held), _) => sustain::even_loop(&mut shaped, channels, take.rate, held, &options),
        (None, Some(fade)) => {
            let (faded, silent) =
                sustain::fade_out(&shaped, channels, take.rate, fade as usize, &options);
            (shaped, sample_loop) = (faded, Some(silent));
        }
        (None, None) => {}
    }
    for v in &mut shaped {
        *v *= gain;
    }
    Pcm16 {
        rate: take.rate,
        channels: take.channels,
        samples: wav::quantize(&shaped),
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
