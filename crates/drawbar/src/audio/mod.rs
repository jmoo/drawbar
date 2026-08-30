//! Hearing a zone: one decoded stroke out of the speakers, and nothing else.
//!
//! Playback is a preview of what a zone holds, not an instrument — so **one zone
//! sounds at a time**, and asking for another replaces it. The backend is the only
//! part that differs between targets: a `rodio` player on the desktop and one Web
//! Audio buffer source in a browser tab, each with a `play`/`stop` pair and no state
//! of its own that the app has to mirror.

use nord_format::formats::nsmp::codec::Audio;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::Sound;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web::Sound;

/// Which zone of which asset is sounding: the document's id, and the zone's index.
pub type Zone = (u64, usize);

/// What one click on a zone's play control means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Play(Zone),
    Stop,
}

/// What clicking `zone` does while `playing` sounds.
///
/// Clicking the sounding zone stops it; clicking any other takes its place. There is no
/// third answer, because there is no second voice.
pub fn act(playing: Option<Zone>, zone: Zone) -> Act {
    match playing == Some(zone) {
        true => Act::Stop,
        false => Act::Play(zone),
    }
}

#[derive(Default)]
pub struct Player {
    sound: Sound,
    playing: Option<Zone>,
}

impl Player {
    /// The zone the speakers are on, if any.
    pub fn playing(&self) -> Option<Zone> {
        self.playing
    }

    /// Start `zone`, or stop it if it is the one already sounding.
    ///
    /// ⚠️ Nothing is marked as sounding until the backend has taken it: a device that
    /// refuses must not leave a Stop button over silence.
    pub fn toggle(&mut self, zone: Zone, audio: &Audio) -> Result<(), String> {
        let asked = act(self.playing, zone);
        self.stop();
        if let Act::Play(zone) = asked {
            self.sound.play(&audio.samples, audio.channels)?;
            self.playing = Some(zone);
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.playing.take().is_some() {
            self.sound.stop();
        }
    }

    /// Forget a zone that has played itself out, so its control reads Play again.
    ///
    /// Called once a frame while something sounds; the app also asks for a repaint so
    /// the change is seen without the pointer moving.
    pub fn settle(&mut self) {
        if self.playing.is_some() && self.sound.finished() {
            self.playing = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One voice: the sounding zone toggles off, and any other zone takes it over.
    #[test]
    fn only_one_zone_ever_sounds() {
        let first = (7, 0);
        let second = (7, 1);
        let elsewhere = (9, 0);

        assert_eq!(act(None, first), Act::Play(first));
        assert_eq!(act(Some(first), first), Act::Stop);
        assert_eq!(act(Some(first), second), Act::Play(second));
        // The same zone index in another document is another zone.
        assert_eq!(act(Some(first), elsewhere), Act::Play(elsewhere));
    }
}
