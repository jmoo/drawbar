//! Playback of decoded zones through the speakers.
//!
//! Each finger on the keys sounds its own voice, mixed with the others: the pointer holds
//! one, and each controller key holds one until it is released. Only the backend differs
//! between targets: a `rodio` mixer on the desktop and Web Audio buffer sources in a
//! browser. Each opens an output and returns voices.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::{Output, Voice};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web::{Output, Voice};

use std::collections::VecDeque;

/// Which zone of which asset is sounding: the document's id, and the zone's index.
pub type Zone = (u64, usize);

/// How many voices sound at once. A voice past this replaces the oldest.
pub const VOICES: usize = 16;

/// What holds a voice, and so what may release it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finger {
    /// A click, on a zone's play control or on the keyboard. There is one pointer, so a
    /// click replaces the previous click's voice.
    Pointer,
    /// A key held on a MIDI controller, whose release stops it.
    Key(u8),
}

/// The playback rate that shifts a stroke `semitones` from the key it was recorded at.
///
/// One octave doubles the rate. This is the resampling any sampler does to play a key
/// from a stroke recorded at another.
pub fn rate(semitones: i16) -> f32 {
    2.0_f32.powf(f32::from(semitones) / 12.0)
}

#[derive(Default)]
pub struct Player {
    output: Output,
    voices: Voices<Voice>,
}

impl Player {
    /// Whether any voice is sounding `zone`.
    pub fn sounds(&self, zone: Zone) -> bool {
        self.voices.sounds(zone)
    }

    /// Every zone sounding, oldest voice first.
    pub fn sounding(&self) -> impl Iterator<Item = Zone> + '_ {
        self.voices.held.iter().map(|held| held.zone)
    }

    /// Stop `zone` where it sounds, or start it on the pointer's voice. `samples` are
    /// interleaved by channel, at whatever rate the backend was built for.
    ///
    /// ⚠️ Nothing is marked as sounding until the backend accepts it: a device that
    /// refuses must not leave a Stop button over silence.
    pub fn toggle(&mut self, zone: Zone, samples: &[i16], channels: u16) -> Result<(), String> {
        if self.voices.sounds(zone) {
            self.silence(zone);
            return Ok(());
        }
        self.strike(Finger::Pointer, zone, samples, channels, 1.0)
    }

    /// Sound `zone` at `rate` times its recorded pitch on `finger`'s voice, replacing
    /// whatever that finger was sounding.
    ///
    /// ⚠️ Not a toggle: a struck key must sound even when the zone answering it is
    /// already playing, and two keys of one zone are two different notes.
    pub fn strike(
        &mut self,
        finger: Finger,
        zone: Zone,
        samples: &[i16],
        channels: u16,
        rate: f32,
    ) -> Result<(), String> {
        self.voices.release(finger);
        let voice = self.output.play(samples, channels, rate)?;
        self.voices.start(finger, zone, voice);
        Ok(())
    }

    /// Stop every voice sounding `zone`.
    pub fn silence(&mut self, zone: Zone) {
        self.voices.silence(zone);
    }

    /// Stop whatever the controller key `key` is sounding.
    pub fn release(&mut self, key: u8) {
        self.voices.release(Finger::Key(key));
    }

    pub fn stop(&mut self) {
        self.voices.held.clear();
    }

    /// Drop the voices that have finished, so their control reads Play again.
    ///
    /// Called once a frame while something sounds; the app also requests a repaint so
    /// the change shows without the pointer moving.
    pub fn settle(&mut self) {
        self.voices.held.retain(|held| !held.voice.finished());
    }
}

/// The voices sounding, oldest first, and the finger and zone each belongs to.
///
/// ⚠️ Dropping a voice silences it, so every voice removed here stops.
struct Voices<V> {
    held: VecDeque<Held<V>>,
}

struct Held<V> {
    finger: Finger,
    zone: Zone,
    voice: V,
}

impl<V> Default for Voices<V> {
    fn default() -> Self {
        Voices {
            held: VecDeque::new(),
        }
    }
}

impl<V> Voices<V> {
    /// Hold `voice` for `finger`, dropping the oldest voice when all are taken.
    fn start(&mut self, finger: Finger, zone: Zone, voice: V) {
        self.release(finger);
        if self.held.len() == VOICES {
            self.held.pop_front();
        }
        self.held.push_back(Held {
            finger,
            zone,
            voice,
        });
    }

    fn release(&mut self, finger: Finger) {
        self.held.retain(|held| held.finger != finger);
    }

    fn silence(&mut self, zone: Zone) {
        self.held.retain(|held| held.zone != zone);
    }

    fn sounds(&self, zone: Zone) -> bool {
        self.held.iter().any(|held| held.zone == zone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONE: Zone = (7, 0);

    /// The voices sounding, each named by the note it was started for.
    fn heard(voices: &Voices<u8>) -> Vec<u8> {
        voices.held.iter().map(|held| held.voice).collect()
    }

    fn chord(keys: &[u8]) -> Voices<u8> {
        let mut voices = Voices::default();
        for key in keys {
            voices.start(Finger::Key(*key), ZONE, *key);
        }
        voices
    }

    #[test]
    fn a_chord_sounds_a_voice_for_each_key() {
        assert_eq!(heard(&chord(&[60, 64, 67])), [60, 64, 67]);
    }

    #[test]
    fn letting_go_of_one_key_stops_only_its_voice() {
        let mut voices = chord(&[60, 64, 67]);
        voices.release(Finger::Key(64));
        assert_eq!(heard(&voices), [60, 67]);
        voices.release(Finger::Key(62));
        assert_eq!(
            heard(&voices),
            [60, 67],
            "a key that holds nothing stops nothing"
        );
    }

    #[test]
    fn a_voice_past_the_limit_takes_the_place_of_the_oldest() {
        let keys: Vec<u8> = (0..=u8::try_from(VOICES).expect("under 256")).collect();
        let voices = chord(&keys);
        assert_eq!(heard(&voices), keys[1..]);
    }

    #[test]
    fn a_key_struck_again_takes_its_own_voice_back_first() {
        let mut voices = chord(&[60, 64]);
        voices.start(Finger::Key(60), ZONE, 61);
        assert_eq!(heard(&voices), [64, 61]);
    }

    #[test]
    fn a_click_takes_the_last_click_s_voice_and_leaves_the_keys_theirs() {
        let mut voices = chord(&[60]);
        voices.start(Finger::Pointer, ZONE, 1);
        voices.start(Finger::Pointer, ZONE, 2);
        assert_eq!(heard(&voices), [60, 2]);
    }

    #[test]
    fn silencing_a_zone_stops_every_voice_on_it() {
        let mut voices = chord(&[60]);
        voices.start(Finger::Key(64), (7, 1), 64);
        voices.start(Finger::Pointer, ZONE, 1);
        assert!(voices.sounds(ZONE));
        voices.silence(ZONE);
        assert_eq!(heard(&voices), [64]);
        assert!(!voices.sounds(ZONE));
    }

    #[test]
    fn a_shifted_key_plays_at_the_rate_that_carries_it() {
        assert_eq!(rate(0), 1.0);
        assert_eq!(rate(12), 2.0);
        assert_eq!(rate(-12), 0.5);
        assert!((rate(7) - 1.498_307).abs() < 1e-5, "{}", rate(7));
    }
}
