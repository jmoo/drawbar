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

#[derive(Default)]
pub struct Sound {
    device: Option<MixerDeviceSink>,
    player: Option<rodio::Player>,
}

impl Sound {
    pub fn play(&mut self, samples: &[i16], channels: u16) -> Result<(), String> {
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
        player.append(SamplesBuffer::new(channels, RATE, source));
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
