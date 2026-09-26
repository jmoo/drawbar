//! Browser sound: one `AudioBuffer` filled from the decoded samples for each voice,
//! played by a buffer source of its own. Every source goes to one destination, which
//! mixes them.
//!
//! ⚠️ A `AudioBufferSourceNode` is single-use — the spec forbids starting one twice —
//! so each play builds a new one and a voice ends by stopping the one it holds.

use std::cell::Cell;
use std::num::NonZero;
use std::rc::Rc;

use nord_format::formats::nsmp::codec::FIELD_RATE;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{AudioBufferSourceNode, AudioContext, AudioScheduledSourceNode};

#[derive(Default)]
pub struct Output {
    context: Option<AudioContext>,
}

/// One stroke sounding.
///
/// ⚠️ Dropping it stops it, as a desktop voice does.
pub struct Voice {
    source: AudioBufferSourceNode,
    /// Set by the source's own `ended` event, which the spec fires once.
    played_out: Rc<Cell<bool>>,
    /// ⚠️ Held for as long as the source it was handed to: a closure dropped here while
    /// the page still holds it throws the moment the event fires.
    _watch: Closure<dyn FnMut()>,
}

impl Output {
    pub fn play(&mut self, samples: &[i16], channels: u16, rate: f32) -> Result<Voice, String> {
        let channels = NonZero::new(channels).ok_or("this zone declares no channels")?;
        self.start(samples, channels, rate)
            .map_err(|e| format!("the browser refused to play this zone: {e:?}"))
    }

    fn start(
        &mut self,
        samples: &[i16],
        channels: NonZero<u16>,
        rate: f32,
    ) -> Result<Voice, JsValue> {
        let channels = u32::from(channels.get());
        let frames = u32::try_from(samples.len())
            .map_err(|_| JsValue::from_str("this zone holds more samples than one buffer takes"))?
            / channels;
        if frames == 0 {
            return Err(JsValue::from_str("this zone decoded to no frames"));
        }
        let context = match &self.context {
            Some(context) => context,
            None => self.context.insert(AudioContext::new()?),
        };
        let buffer = context.create_buffer(channels, frames, FIELD_RATE as f32)?;
        // Web Audio wants one plane per channel; the codec hands back interleaved.
        for channel in 0..channels {
            let plane: Vec<f32> = samples
                .iter()
                .skip(channel as usize)
                .step_by(channels as usize)
                .map(|s| f32::from(*s) / 32768.0)
                .collect();
            buffer.copy_to_channel(&plane, channel as i32)?;
        }
        let source = context.create_buffer_source()?;
        source.set_buffer(Some(&buffer));
        if rate.is_finite() && rate > 0.0 {
            source.playback_rate().set_value(rate);
        }
        source.connect_with_audio_node(&context.destination())?;

        let played_out = Rc::new(Cell::new(false));
        let flag = played_out.clone();
        let watch = Closure::wrap(Box::new(move || flag.set(true)) as Box<dyn FnMut()>);
        AudioScheduledSourceNode::set_onended(&source, Some(watch.as_ref().unchecked_ref()));
        source.start()?;
        Ok(Voice {
            source,
            played_out,
            _watch: watch,
        })
    }
}

impl Voice {
    pub fn finished(&self) -> bool {
        self.played_out.get()
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        // ⚠️ The handler goes before the closure behind it does: stopping a source fires
        // `ended`, and a dropped `Closure` the page still holds throws.
        AudioScheduledSourceNode::set_onended(&self.source, None);
        // Through the base interface: the buffer-source spelling is deprecated.
        let _ = AudioScheduledSourceNode::stop(&self.source);
    }
}
