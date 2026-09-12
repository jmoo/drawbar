//! Desktop sound: a `rodio` player over the default output.
//!
//! The device is opened on the first play and kept, because opening one takes long
//! enough to be heard as a gap and an app that has never played anything should not
//! be holding the sound card.

use std::num::NonZero;

use nord_format::formats::nsmp::codec::FIELD_RATE;
use rodio::buffer::SamplesBuffer;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};

/// The rate the codec decodes to, in the mixer's units. A zero here is a compile error.
const RATE: NonZero<u32> = match NonZero::new(FIELD_RATE) {
    Some(rate) => rate,
    None => panic!("the field rate is not zero"),
};

/// The rate a buffer is declared at to play `rate` times its recorded pitch: handing the
/// mixer a faster clock is the resampling, and there is no second copy of the samples.
fn clocked(rate: f32) -> NonZero<u32> {
    let asked = (RATE.get() as f32 * rate).round();
    match asked.is_finite() && asked >= 1.0 {
        true => NonZero::new(asked.min(u32::MAX as f32) as u32).unwrap_or(RATE),
        false => RATE,
    }
}

#[derive(Default)]
pub struct Sound {
    device: Option<MixerDeviceSink>,
    player: Option<rodio::Player>,
}

impl Sound {
    pub fn play(&mut self, samples: &[i16], channels: u16, rate: f32) -> Result<(), String> {
        let channels = NonZero::new(channels).ok_or("this zone declares no channels")?;
        // One voice: whatever is sounding gives way rather than mixing with this.
        self.stop();
        let device = match &self.device {
            Some(device) => device,
            None => self
                .device
                .insert(DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?),
        };
        let player = rodio::Player::connect_new(device.mixer());
        // rodio mixes in f32; the codec's own units are the 16-bit ones it decoded to.
        let source: Vec<f32> = samples.iter().map(|s| f32::from(*s) / 32768.0).collect();
        player.append(SamplesBuffer::new(channels, clocked(rate), source));
        self.player = Some(player);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(player) = self.player.take() {
            player.stop();
        }
    }

    pub fn finished(&self) -> bool {
        self.player.as_ref().is_none_or(rodio::Player::empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An octave up is twice the clock, and a rate that is not a rate leaves the
    /// recorded pitch alone rather than handing the mixer a zero.
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
