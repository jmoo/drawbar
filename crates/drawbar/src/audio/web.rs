//! Browser sound: one `AudioBuffer` filled from the decoded samples for each voice,
//! played by a buffer source of its own. Every source goes to one destination, which
//! mixes them.
//!
//! ⚠️ An `AudioBufferSourceNode` is single-use (the spec forbids starting one twice), so
//! each play builds a new one and a voice ends by stopping its own.

use std::cell::Cell;
use std::num::NonZero;
use std::rc::Rc;

use nord_format::formats::nsmp::codec::FIELD_RATE;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{AudioBufferSourceNode, AudioContext, AudioScheduledSourceNode};

use super::Sample;

#[derive(Default)]
pub struct Output {
    context: Option<AudioContext>,
}

/// One stroke sounding.
///
/// ⚠️ Dropping it stops it, as a desktop voice does.
pub struct Voice {
    source: AudioBufferSourceNode,
    /// The context's clock when the source started, to tell how far it has played.
    context: AudioContext,
    started: f64,
    /// Set by the source's own `ended` event, which the spec fires once.
    played_out: Rc<Cell<bool>>,
    /// ⚠️ Kept as long as the source that holds it: a closure dropped while the page
    /// still references it throws when the event fires.
    _watch: Closure<dyn FnMut()>,
}

impl Output {
    pub fn play<S: Sample>(
        &mut self,
        samples: &[S],
        channels: u16,
        rate: f32,
    ) -> Result<Voice, String> {
        let channels = NonZero::new(channels).ok_or("this zone declares no channels")?;
        self.start(samples, channels, rate)
            .map_err(|e| format!("the browser refused to play this zone: {e:?}"))
    }

    fn start<S: Sample>(
        &mut self,
        samples: &[S],
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
        let source = buffer_source(context, samples, channels, frames, rate)?;
        let (played_out, watch) = watched(&source);
        source.start()?;
        Ok(Voice {
            source,
            context: context.clone(),
            started: context.current_time(),
            played_out,
            _watch: watch,
        })
    }
}

/// A buffer source holding `frames` of `samples`, `channels` interleaved, played at
/// `rate` times its recorded pitch, wired to the destination and not started.
fn buffer_source<S: Sample>(
    context: &AudioContext,
    samples: &[S],
    channels: u32,
    frames: u32,
    rate: f32,
) -> Result<AudioBufferSourceNode, JsValue> {
    let buffer = context.create_buffer(channels, frames, FIELD_RATE as f32)?;
    // Web Audio takes one plane per channel; the samples come interleaved.
    for channel in 0..channels {
        let plane: Vec<f32> = samples
            .iter()
            .skip(channel as usize)
            .step_by(channels as usize)
            .map(|s| s.level())
            .collect();
        buffer.copy_to_channel(&plane, channel as i32)?;
    }
    let source = context.create_buffer_source()?;
    source.set_buffer(Some(&buffer));
    if rate.is_finite() && rate > 0.0 {
        source.playback_rate().set_value(rate);
    }
    source.connect_with_audio_node(&context.destination())?;
    Ok(source)
}

/// A flag `source`'s own `ended` event sets, and the closure that sets it.
fn watched(source: &AudioBufferSourceNode) -> (Rc<Cell<bool>>, Closure<dyn FnMut()>) {
    let played_out = Rc::new(Cell::new(false));
    let flag = played_out.clone();
    let watch = Closure::wrap(Box::new(move || flag.set(true)) as Box<dyn FnMut()>);
    AudioScheduledSourceNode::set_onended(source, Some(watch.as_ref().unchecked_ref()));
    (played_out, watch)
}

impl Voice {
    pub fn finished(&self) -> bool {
        self.played_out.get()
    }

    /// Seconds played since the source started. The buffer is declared at the codec's
    /// rate and played faster or slower by the source's rate, so these are seconds at the
    /// clock the desktop declares its buffer at.
    pub fn played(&self) -> f64 {
        (self.context.current_time() - self.started).max(0.0)
    }

    /// Play `samples` from `at` seconds of the voice on, in place of its own from there:
    /// a new source started on the context's clock as the old one stops.
    pub fn splice<S: Sample>(
        &mut self,
        at: f64,
        samples: &[S],
        channels: u16,
        rate: f32,
    ) -> Result<(), String> {
        self.spliced(at, samples, channels, rate)
            .map_err(|e| format!("the browser refused to play this zone: {e:?}"))
    }

    fn spliced<S: Sample>(
        &mut self,
        at: f64,
        samples: &[S],
        channels: u16,
        rate: f32,
    ) -> Result<(), JsValue> {
        let channels = u32::from(channels.max(1));
        let frames = u32::try_from(samples.len())
            .map_err(|_| JsValue::from_str("this zone holds more samples than one buffer takes"))?
            / channels;
        if frames == 0 {
            return Err(JsValue::from_str("this zone decoded to no frames"));
        }
        let source = buffer_source(&self.context, samples, channels, frames, rate)?;
        let (when, now) = (self.started + at, self.context.current_time());
        match when > now {
            true => source.start_with_when(when)?,
            // Late: as far into the new samples as the old ones played past `at`.
            false => {
                let speed = if rate.is_finite() && rate > 0.0 {
                    f64::from(rate)
                } else {
                    1.0
                };
                source.start_with_when_and_grain_offset(now, (now - when) * speed)?
            }
        }
        AudioScheduledSourceNode::set_onended(&self.source, None);
        AudioScheduledSourceNode::stop_with_when(&self.source, when.max(now))?;
        let (played_out, watch) = watched(&source);
        // ⚠️ The old source is let go of without Drop's stop, so it plays on to `when`.
        self.source = source;
        self.played_out = played_out;
        self._watch = watch;
        Ok(())
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        // ⚠️ Clear the handler before its closure is dropped: stopping a source fires
        // `ended`, and a dropped `Closure` the page still holds throws.
        AudioScheduledSourceNode::set_onended(&self.source, None);
        // Through the base interface: the buffer-source spelling is deprecated.
        let _ = AudioScheduledSourceNode::stop(&self.source);
    }
}
