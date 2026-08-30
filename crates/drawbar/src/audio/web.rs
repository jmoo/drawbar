//! Browser sound: one `AudioBuffer` filled from the decoded samples, played by a
//! buffer source that is dropped when it stops.
//!
//! ⚠️ A `AudioBufferSourceNode` is single-use — the spec forbids starting one twice —
//! so each play builds a new one and Stop simply ends the one in hand.

use nord_format::formats::nsmp::codec::FIELD_RATE;
use wasm_bindgen::JsValue;
use web_sys::{AudioBufferSourceNode, AudioContext, AudioScheduledSourceNode};

#[derive(Default)]
pub struct Sound {
    context: Option<AudioContext>,
    source: Option<AudioBufferSourceNode>,
}

impl Sound {
    pub fn play(&mut self, samples: &[i16], channels: u16) -> Result<(), String> {
        self.start(samples, channels)
            .map_err(|e| format!("the browser refused to play this zone: {e:?}"))
    }

    fn start(&mut self, samples: &[i16], channels: u16) -> Result<(), JsValue> {
        // One voice: whatever is sounding gives way rather than mixing with this.
        self.stop();
        let channels = u32::from(channels).max(1);
        let frames = u32::try_from(samples.len()).unwrap_or(u32::MAX) / channels;
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
        source.connect_with_audio_node(&context.destination())?;
        source.start()?;
        self.source = Some(source);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(source) = self.source.take() {
            // Through the base interface: the buffer-source spelling is deprecated.
            let _ = AudioScheduledSourceNode::stop(&source);
        }
    }

    /// The browser gives no cheap "has it ended" flag without an event listener, and a
    /// source that has run out is silent either way — so the control stays on Stop
    /// until it is clicked or another zone takes over.
    pub fn finished(&self) -> bool {
        false
    }
}
