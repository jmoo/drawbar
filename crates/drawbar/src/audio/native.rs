//! Desktop sound: a `rodio` player for each voice, mixed onto the default output.
//!
//! The device opens on the first play and stays open. Opening one takes long enough to
//! be heard as a gap, and an app that has never played anything should not hold the
//! sound card.

use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nord_format::formats::nsmp::codec::FIELD_RATE;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};
use rodio::{ChannelCount, SampleRate, Source};

use super::Sample;

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
pub struct Voice {
    player: rodio::Player,
    /// What a splice hands the voice's source.
    splice: Arc<Splice>,
    channels: ChannelCount,
    rate: SampleRate,
}

/// Samples waiting to take the place of a voice's own from a point on its clock, in
/// samples.
#[derive(Default)]
struct Splice {
    waiting: AtomicBool,
    next: Mutex<Option<(usize, Vec<f32>)>>,
}

/// What a voice plays: its samples, until a splice puts others in their place from a
/// frame on, at the sample the mixer takes there, so nothing is skipped or played twice.
struct Spliced {
    samples: Vec<f32>,
    /// Samples taken so far: the voice's own clock, which a splice does not reset.
    at: usize,
    /// Where on that clock `samples` starts.
    from: usize,
    splice: Arc<Splice>,
    channels: ChannelCount,
    rate: SampleRate,
}

impl Iterator for Spliced {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<rodio::Sample> {
        let frame_start = self.at.is_multiple_of(usize::from(self.channels.get()));
        if frame_start && self.splice.waiting.load(Ordering::Acquire) {
            // ⚠️ Never wait on the mixer's thread: a splice being handed over is taken a
            // sample later.
            if let Ok(mut next) = self.splice.next.try_lock() {
                if let Some((from, samples)) = next.take_if(|(from, _)| self.at >= *from) {
                    self.splice.waiting.store(false, Ordering::Release);
                    (self.from, self.samples) = (from, samples);
                }
            }
        }
        let sample = *self.samples.get(self.at.checked_sub(self.from)?)?;
        self.at += 1;
        Some(sample)
    }
}

impl Source for Spliced {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

impl Output {
    pub fn play<S: Sample>(
        &mut self,
        samples: &[S],
        channels: u16,
        rate: f32,
    ) -> Result<Voice, String> {
        let channels = NonZero::new(channels).ok_or("this zone declares no channels")?;
        let device = match &self.device {
            Some(device) => device,
            None => self
                .device
                .insert(DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?),
        };
        let player = rodio::Player::connect_new(device.mixer());
        // rodio mixes in f32 and owns what it plays.
        let source: Vec<f32> = samples.iter().map(|s| s.level()).collect();
        let splice = Arc::new(Splice::default());
        let rate = clocked(rate);
        player.append(Spliced {
            samples: source,
            at: 0,
            from: 0,
            splice: splice.clone(),
            channels,
            rate,
        });
        Ok(Voice {
            player,
            splice,
            channels,
            rate,
        })
    }
}

impl Voice {
    pub fn finished(&self) -> bool {
        self.player.empty()
    }

    /// Seconds of the buffer played, at the clock it was declared at.
    pub fn played(&self) -> f64 {
        self.player.get_pos().as_secs_f64()
    }

    /// Play `samples` from `at` seconds of the voice on, in place of its own from there.
    /// They are at the voice's clock and channels.
    pub fn splice<S: Sample>(
        &mut self,
        at: f64,
        samples: &[S],
        _channels: u16,
        _rate: f32,
    ) -> Result<(), String> {
        let frame = (at * f64::from(self.rate.get())).round().max(0.0) as usize;
        let from = frame * usize::from(self.channels.get());
        let samples = samples.iter().map(|s| s.level()).collect();
        *self
            .splice
            .next
            .lock()
            .map_err(|_| "a voice's splice was poisoned")? = Some((from, samples));
        self.splice.waiting.store(true, Ordering::Release);
        Ok(())
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
