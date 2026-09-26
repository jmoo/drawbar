//! Desktop sound: a `rodio` player for each voice, mixed onto the default output.
//!
//! The device opens on the first play and stays open. Opening one takes long enough to
//! be heard as a gap, and an app that has never played anything should not hold the
//! sound card.

use std::num::NonZero;

use nord_format::formats::nsmp::codec::FIELD_RATE;
use rodio::buffer::SamplesBuffer;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};

/// The rate the codec decodes to, in the mixer's units. A zero here is a compile error.
const RATE: NonZero<u32> = match NonZero::new(FIELD_RATE) {
    Some(rate) => rate,
    None => panic!("the field rate must not be zero"),
};

/// The rate to declare a buffer at so it plays at `rate` times its recorded pitch. The
/// mixer resamples from the declared clock, so the samples are not copied again.
fn clocked(rate: f32) -> NonZero<u32> {
    let asked = (RATE.get() as f32 * rate).round();
    match asked.is_finite() && asked >= 1.0 {
        true => NonZero::new(asked.min(u32::MAX as f32) as u32).unwrap_or(RATE),
        false => RATE,
    }
}

#[derive(Default)]
pub struct Output {
    device: Option<MixerDeviceSink>,
}

/// One stroke sounding on the mixer.
///
/// ⚠️ Dropping it stops it: a `rodio` player that is not detached stops on drop.
pub struct Voice(rodio::Player);

impl Output {
    pub fn play(&mut self, samples: &[i16], channels: u16, rate: f32) -> Result<Voice, String> {
        let channels = NonZero::new(channels).ok_or("this zone declares no channels")?;
        let device = match &self.device {
            Some(device) => device,
            None => self
                .device
                .insert(DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?),
        };
        let player = rodio::Player::connect_new(device.mixer());
        // rodio mixes in f32; the codec decodes to 16-bit samples.
        let source: Vec<f32> = samples.iter().map(|s| f32::from(*s) / 32768.0).collect();
        player.append(SamplesBuffer::new(channels, clocked(rate), source));
        Ok(Voice(player))
    }
}

impl Voice {
    pub fn finished(&self) -> bool {
        self.0.empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rate that is not positive and finite keeps the recorded pitch, so the mixer
    /// never gets a zero clock.
    #[test]
    fn the_clock_carries_the_pitch_shift() {
        assert_eq!(clocked(1.0), RATE);
        assert_eq!(clocked(2.0).get(), RATE.get() * 2);
        assert_eq!(clocked(0.5).get(), RATE.get() / 2);
        for refused in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(clocked(refused), RATE, "{refused}");
        }
    }
}
