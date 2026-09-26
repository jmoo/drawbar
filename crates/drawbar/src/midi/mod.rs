//! MIDI input: a controller's keys, played as clicks on the front key map.
//!
//! Listening belongs to the app, not to a document: it is started and stopped from the
//! Instrument menu, and whichever key map is showing responds to the keys.
//!
//! ⚠️ Input only. Nothing here opens an output port or sends a byte. A controller plays
//! this app's own audition and reaches no instrument: writes to a Nord go over USB, from
//! [`crate::device`], when the user asks for them.
//!
//! Only the backend differs between targets: `midir` on the desktop and Web MIDI in a
//! browser. Each opens every input port it can, decodes each port's bytes as they
//! arrive, and leaves note messages in a queue that each frame drains. A browser tab has
//! no thread to drain it on, and nothing here may block one.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::Ports;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web::Ports;

use std::collections::{BTreeSet, VecDeque};

use eframe::egui;

use crate::document::keys::Struck;
use crate::log::Log;

/// What the Instrument menu says where the browser has no Web MIDI.
pub const UNSUPPORTED: &str =
    "This browser cannot use MIDI controllers; use Chrome, Edge, or Firefox.";

/// How many note messages can wait for a frame. A full queue drops its oldest.
const QUEUE: usize = 64;

/// How long a strike may wait for a frame and still sound, in seconds.
///
/// A hidden tab or a minimized window draws nothing while the controller keeps sending,
/// and a key struck then is over by the time a frame could play it.
const STALE: f64 = 0.5;

/// One note message, whichever port carried it.
///
/// ⚠️ A note-on at velocity zero is a note-off (the running-status form of one), so
/// [`Note::On`] never holds a zero velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Note {
    On(Struck),
    Off(u8),
}

/// The keys played since the last frame, and the keys held now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Played {
    /// Every key struck, oldest first. A key struck twice is here once, at its last.
    pub struck: Vec<Struck>,
    /// Every key released, oldest first, except a release after the key's own strike.
    pub released: Vec<u8>,
    /// Every key held down, lowest first.
    pub down: Vec<u8>,
}

/// The state of MIDI input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing is listening.
    Off,
    /// Access has been requested, and the answer has not come back.
    Asking,
    /// Listening on `ports`, named as the system names them. `refused` lists the ports
    /// that would not open, perhaps because another program holds them. Both empty means
    /// no controller is connected.
    On {
        ports: Vec<String>,
        refused: Vec<String>,
    },
    /// The last attempt to listen failed, and why.
    Failed(String),
}

/// Every input port this app listens to, the note messages they have sent, and the keys
/// held down on them.
#[derive(Default)]
pub struct Midi {
    ports: Ports,
    down: BTreeSet<u8>,
    /// Whether the failure in [`State::Failed`] is already in the log.
    reported: bool,
}

/// Whether this build can hear a controller at all. A browser without Web MIDI cannot.
pub fn supported() -> bool {
    Ports::supported()
}

impl Midi {
    /// Start listening on every input port the machine has.
    ///
    /// ⚠️ In a browser this asks the user for permission, which the page may do only
    /// while a click's user activation is live: call it from the click itself.
    pub fn listen(&mut self, ctx: &egui::Context) {
        self.ports.listen(ctx);
    }

    pub fn stop(&mut self) {
        self.ports.stop();
        self.down.clear();
    }

    pub fn state(&self) -> State {
        self.ports.state()
    }

    /// Whether listening was asked for, whether or not it has worked.
    pub fn on(&self) -> bool {
        self.state() != State::Off
    }

    /// The keys played since the last frame. `now` is egui's frame clock, which a
    /// backend that polls for new ports measures against.
    pub fn played(&mut self, now: f64) -> Played {
        played(&self.ports.drain(now), &mut self.down)
    }

    /// Put a failure to listen in the activity log, once. The title bar says only that
    /// listening failed.
    pub fn report(&mut self, log: &mut Log) {
        let failed = match self.state() {
            State::Failed(why) => Some(why),
            State::Off | State::Asking | State::On { .. } => None,
        };
        if let (Some(why), false) = (&failed, self.reported) {
            log.error(why.clone());
            log.trouble("drawbar could not listen to MIDI controllers.");
        }
        self.reported = failed.is_some();
    }
}

/// Where the desktop app keeps whether it was listening, so the next session listens
/// too.
///
/// ⚠️ Not kept in a browser, which must ask the user for access from a click, so a tab
/// starts with MIDI off.
#[cfg(not(target_arch = "wasm32"))]
const KEY: &str = "drawbar.midi";

#[cfg(not(target_arch = "wasm32"))]
impl Midi {
    /// Listen again where the last session was listening.
    pub fn restore(&mut self, storage: &dyn eframe::Storage, ctx: &egui::Context) {
        if storage.get_string(KEY).as_deref() == Some("on") {
            self.listen(ctx);
        }
    }

    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        let said = match self.on() {
            true => "on",
            false => "off",
        };
        storage.set_string(KEY, said.to_string());
    }
}

/// What one frame's note messages play. `down` holds the keys held before the frame and
/// is updated to those held after it.
///
/// Every key struck sounds, each on its own voice. A key struck and released within one
/// frame still sounds: the queue does not keep the keyboard's timing, and a key that came
/// and went unheard would do nothing. A release before a key's strike is passed on, and
/// stops whatever that key sounded in an earlier frame.
fn played(notes: &[Note], down: &mut BTreeSet<u8>) -> Played {
    let mut played = Played::default();
    for note in notes {
        match *note {
            Note::On(struck) => {
                down.insert(struck.note);
                played.struck.retain(|held| held.note != struck.note);
                played.struck.push(struck);
            }
            Note::Off(key) => {
                down.remove(&key);
                if !played.struck.iter().any(|held| held.note == key) {
                    played.released.push(key);
                }
            }
        }
    }
    played.down = down.iter().copied().collect();
    played
}

/// Note messages waiting for a frame, each with the time it arrived.
///
/// ⚠️ Arrival times and the `now` passed to [`Queue::drain`] must come from one clock,
/// in seconds. Each backend chooses its own.
#[derive(Default)]
struct Queue {
    notes: VecDeque<(f64, Note)>,
}

impl Queue {
    fn push(&mut self, at: f64, note: Note) {
        if self.notes.len() == QUEUE {
            self.notes.pop_front();
        }
        self.notes.push_back((at, note));
    }

    /// Every waiting message except strikes older than [`STALE`]. A release is never too
    /// old: its key may still hold a voice.
    fn drain(&mut self, now: f64) -> Vec<Note> {
        self.notes
            .drain(..)
            .filter(|(at, note)| matches!(note, Note::Off(_)) || now - at <= STALE)
            .map(|(_, note)| note)
            .collect()
    }
}

/// One port's bytes, decoded a message at a time.
///
/// ⚠️ Running status is per stream: a data byte's meaning depends on the last channel
/// status seen on that port, so each port has its own `Stream`.
#[derive(Default)]
struct Stream {
    /// The channel status in effect for data bytes. `None` after anything that ends
    /// running status, and before the first status byte.
    status: Option<u8>,
    first: u8,
    have: usize,
}

impl Stream {
    /// Decode `bytes`, passing each note message to `note` as it completes.
    ///
    /// A message may be split across calls, and several may arrive in one. Both data
    /// bytes are below 0x80 by the time they are read, so the note and velocity passed
    /// on are always 7-bit values.
    fn feed(&mut self, bytes: &[u8], mut note: impl FnMut(Note)) {
        for byte in bytes {
            if let Some(message) = self.take(*byte) {
                note(message);
            }
        }
    }

    fn take(&mut self, byte: u8) -> Option<Note> {
        match byte {
            // System real-time bytes can appear anywhere, even inside a message, and
            // carry no note data.
            0xF8..=0xFF => None,
            // System common ends running status. Its data bytes, and those of a system
            // exclusive dump, then have no status and are dropped.
            0xF0..=0xF7 => {
                self.status = None;
                self.have = 0;
                None
            }
            0x80..=0xEF => {
                self.status = Some(byte);
                self.have = 0;
                None
            }
            _ => self.data(byte),
        }
    }

    /// A data byte, which completes a message or waits for the one after it. A data
    /// byte with no status in force is a stray and is dropped.
    fn data(&mut self, byte: u8) -> Option<Note> {
        let status = self.status?;
        if self.have + 1 < wants(status) {
            self.first = byte;
            self.have += 1;
            return None;
        }
        self.have = 0;
        match (status & 0xF0, self.first, byte) {
            (0x80, note, _) | (0x90, note, 0) => Some(Note::Off(note)),
            (0x90, note, velocity) => Some(Note::On(Struck { note, velocity })),
            _ => None,
        }
    }
}

/// How many data bytes a channel message carries. Program change and channel pressure
/// take one; every other channel status takes two.
fn wants(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(note: u8, velocity: u8) -> Note {
        Note::On(Struck { note, velocity })
    }

    fn struck(note: u8, velocity: u8) -> Struck {
        Struck { note, velocity }
    }

    /// Every note message in `bytes`, as one port's stream decodes them.
    fn notes(bytes: &[u8]) -> Vec<Note> {
        let mut stream = Stream::default();
        let mut heard = Vec::new();
        stream.feed(bytes, |note| heard.push(note));
        heard
    }

    #[test]
    fn a_note_on_is_a_strike_and_velocity_zero_is_a_release() {
        assert_eq!(notes(&[0x90, 0x3C, 0x64]), [on(60, 100)]);
        assert_eq!(notes(&[0x80, 0x3C, 0x40]), [Note::Off(60)]);
        assert_eq!(notes(&[0x90, 0x3C, 0x00]), [Note::Off(60)]);
        // The channel is ignored: all sixteen play.
        assert_eq!(notes(&[0x9F, 0x45, 0x01]), [on(69, 1)]);
    }

    #[test]
    fn running_status_carries_the_pairs_that_follow_it() {
        assert_eq!(
            notes(&[0x90, 0x3C, 0x40, 0x3E, 0x50, 0x3C, 0x00]),
            [on(60, 64), on(62, 80), Note::Off(60)]
        );
    }

    #[test]
    fn a_message_split_across_reads_is_still_one_message() {
        let mut stream = Stream::default();
        let mut heard = Vec::new();
        for part in [&[0x90u8][..], &[0x3C], &[0x64]] {
            stream.feed(part, |note| heard.push(note));
        }
        assert_eq!(heard, [on(60, 100)]);
    }

    #[test]
    fn a_real_time_byte_between_two_data_bytes_is_ignored() {
        assert_eq!(notes(&[0x90, 0xF8, 0x3C, 0xFE, 0x64, 0xF8]), [on(60, 100)]);
    }

    #[test]
    fn a_stray_data_byte_sounds_nothing() {
        assert!(notes(&[0x3C, 0x64]).is_empty());
        assert_eq!(notes(&[0x3C, 0x90, 0x3C, 0x64]), [on(60, 100)]);
    }

    #[test]
    fn a_system_message_ends_the_running_status_before_it() {
        assert_eq!(
            notes(&[0x90, 0x3C, 0x64, 0xF0, 0x7E, 0x00, 0x3E, 0xF7, 0x3E, 0x50]),
            [on(60, 100)]
        );
    }

    #[test]
    fn other_channel_messages_sound_nothing_and_take_their_own_bytes() {
        // Control change (two data bytes), program change (one), pitch bend (two), and
        // then a note that must still be read as a note.
        assert_eq!(
            notes(&[0xB0, 0x07, 0x64, 0xC0, 0x05, 0xE0, 0x00, 0x40, 0x90, 0x3C, 0x64]),
            [on(60, 100)]
        );
    }

    /// What `notes` play on a controller with no key held before them.
    fn frame(notes: &[Note]) -> Played {
        played(notes, &mut BTreeSet::new())
    }

    #[test]
    fn a_frame_plays_every_key_struck_in_it() {
        assert_eq!(frame(&[]), Played::default());
        assert_eq!(
            frame(&[on(60, 64), on(64, 70), on(67, 80)]).struck,
            [struck(60, 64), struck(64, 70), struck(67, 80)]
        );
        assert_eq!(
            frame(&[on(60, 64), on(64, 70), on(60, 30)]).struck,
            [struck(64, 70), struck(60, 30)],
            "a key struck twice sounds its last strike"
        );
    }

    #[test]
    fn a_key_struck_and_let_go_inside_one_frame_still_sounds() {
        assert_eq!(
            frame(&[on(60, 64), Note::Off(60)]),
            Played {
                struck: vec![struck(60, 64)],
                released: Vec::new(),
                down: Vec::new(),
            }
        );
    }

    #[test]
    fn a_frame_passes_on_every_release_but_a_key_s_after_its_own_strike() {
        // C4 held from an earlier frame; D4 struck, then both lifted.
        assert_eq!(
            frame(&[on(62, 90), Note::Off(62), Note::Off(60)]),
            Played {
                struck: vec![struck(62, 90)],
                released: vec![60],
                down: Vec::new(),
            }
        );
        // A release before the strike stops what the key sounded earlier.
        assert_eq!(
            frame(&[Note::Off(60), on(60, 30)]),
            Played {
                struck: vec![struck(60, 30)],
                released: vec![60],
                down: vec![60],
            }
        );
    }

    #[test]
    fn a_key_is_down_from_its_strike_to_its_release_across_frames() {
        let mut down = BTreeSet::new();
        assert_eq!(played(&[on(67, 1), on(60, 1)], &mut down).down, [60, 67]);
        assert_eq!(played(&[], &mut down).down, [60, 67]);
        assert_eq!(played(&[Note::Off(67)], &mut down).down, [60]);
        assert!(played(&[Note::Off(60)], &mut down).down.is_empty());
    }

    #[test]
    fn a_strike_older_than_the_stale_window_is_dropped_and_a_release_is_kept() {
        let mut queue = Queue::default();
        queue.push(0.0, on(60, 100));
        queue.push(0.0, Note::Off(60));
        queue.push(10.0, on(62, 100));
        assert_eq!(queue.drain(10.0 + STALE), [Note::Off(60), on(62, 100)]);
        assert!(queue.drain(20.0).is_empty(), "a drain empties the queue");
    }

    #[test]
    fn a_full_queue_drops_its_oldest_message() {
        let mut queue = Queue::default();
        let keys = 0..u8::try_from(QUEUE + 2).expect("the queue is under 256");
        for key in keys.clone() {
            queue.push(0.0, on(key, 1));
        }
        let kept: Vec<Note> = keys.skip(2).map(|key| on(key, 1)).collect();
        assert_eq!(queue.drain(0.0), kept);
    }
}
