//! MIDI in: a controller's keys, read as clicks on the key map.
//!
//! ⚠️ Input only. Nothing here opens an output port or sends a byte. A controller plays
//! this app's own audition and reaches no instrument: what is written to a Nord goes
//! over USB, from [`crate::device`], on the operator's word.
//!
//! The backend is the only part that differs between targets: `midir` on the desktop and
//! Web MIDI in a browser tab. Each opens every input port it can, decodes each port's
//! bytes where they arrive, and leaves note messages in a queue the frame drains — there
//! is no thread to drain it on in a browser tab, and nothing here may block one.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::Ports;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web::Ports;

use eframe::egui;

/// How many note messages wait for a frame to take them.
///
/// A queue that filled while nothing was repainting holds keys that were struck long
/// enough ago to be over; the ones that no longer fit are dropped rather than sounded
/// late.
const QUEUE: usize = 64;

/// One note message, whichever port carried it.
///
/// ⚠️ A note-on at velocity zero is a note-off — the running-status spelling of one —
/// so [`Note::On`] never holds a zero velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    On { note: u8, velocity: u8 },
    Off { note: u8 },
}

/// What a frame's note messages ask the key map for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Played {
    Struck { note: u8, velocity: u8 },
    Released { note: u8 },
}

/// Where MIDI in stands, as one value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing is listening.
    Off,
    /// Access has been asked for and the answer has not come back.
    Asking,
    /// Listening on these input ports, named as the machine names them. Empty is a
    /// machine with no controller on it.
    On(Vec<String>),
    /// The last attempt to listen failed, and why.
    Failed(String),
}

/// Every input port this app is listening to, and the note messages they have sent.
#[derive(Default)]
pub struct Midi {
    ports: Ports,
}

impl Midi {
    /// Start listening on every input port the machine has.
    ///
    /// ⚠️ In a browser tab this asks the reader for permission, which the page may only
    /// do while a click's user activation is live: call it from the click itself.
    pub fn listen(&mut self, ctx: &egui::Context) {
        self.ports.listen(ctx);
    }

    pub fn stop(&mut self) {
        self.ports.stop();
    }

    pub fn state(&self) -> State {
        self.ports.state()
    }

    /// What the keys played since the last frame ask for. `now` is egui's frame clock,
    /// which is what a backend that has to look for new ports measures against.
    pub fn played(&mut self, now: f64) -> Option<Played> {
        latest(&self.ports.drain(now))
    }
}

/// The one thing a frame's worth of note messages asks for.
///
/// One voice sounds, so the frame plays the last key struck in it. A key struck and let
/// go inside one frame still sounds: the queue is not the keyboard's own timing, and a
/// key that came and went unheard is a key that did nothing.
fn latest(notes: &[Note]) -> Option<Played> {
    let struck = notes.iter().rev().find_map(|note| match note {
        Note::On { note, velocity } => Some(Played::Struck {
            note: *note,
            velocity: *velocity,
        }),
        Note::Off { .. } => None,
    });
    struck.or_else(|| match notes.last() {
        Some(Note::Off { note }) => Some(Played::Released { note: *note }),
        _ => None,
    })
}

/// One port's bytes, decoded a message at a time.
///
/// ⚠️ Running status belongs to the stream that carries it: a data byte's meaning is the
/// last channel status seen **on that port**, so one of these belongs to each port
/// rather than to the app.
#[derive(Default)]
pub struct Stream {
    /// The channel status in force, which a data byte belongs to. `None` after anything
    /// that ends running status, and until the first status byte of all.
    status: Option<u8>,
    first: u8,
    have: usize,
}

impl Stream {
    /// Decode `bytes`, handing each note message to `note` as it completes.
    ///
    /// A message may be split across calls, and several may arrive in one. Both data
    /// bytes are below 0x80 by the time they are read, so the note and velocity handed
    /// on are 7-bit values whatever the port sent.
    pub fn feed(&mut self, bytes: &[u8], mut note: impl FnMut(Note)) {
        for byte in bytes {
            if let Some(message) = self.take(*byte) {
                note(message);
            }
        }
    }

    fn take(&mut self, byte: u8) -> Option<Note> {
        match byte {
            // System real time interleaves anywhere, including between the bytes of a
            // message, and carries nothing of its own.
            0xF8..=0xFF => None,
            // System common ends running status. Its own data bytes, and a system
            // exclusive dump's, then belong to no status and are dropped.
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
            (0x80, note, _) => Some(Note::Off { note }),
            (0x90, note, 0) => Some(Note::Off { note }),
            (0x90, note, velocity) => Some(Note::On { note, velocity }),
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

    /// Every note message in `bytes`, as one port's stream decodes them.
    fn notes(bytes: &[u8]) -> Vec<Note> {
        let mut stream = Stream::default();
        let mut heard = Vec::new();
        stream.feed(bytes, |note| heard.push(note));
        heard
    }

    /// The wire bytes of the three ways a key is struck and let go: note on, note off,
    /// and the note on at velocity zero that means the same as a note off.
    #[test]
    fn a_note_on_is_a_strike_and_velocity_zero_is_a_release() {
        assert_eq!(
            notes(&[0x90, 0x3C, 0x64]),
            [Note::On {
                note: 60,
                velocity: 100
            }]
        );
        assert_eq!(notes(&[0x80, 0x3C, 0x40]), [Note::Off { note: 60 }]);
        assert_eq!(notes(&[0x90, 0x3C, 0x00]), [Note::Off { note: 60 }]);
        // The channel is not the app's business: every one of the sixteen plays.
        assert_eq!(
            notes(&[0x9F, 0x45, 0x01]),
            [Note::On {
                note: 69,
                velocity: 1
            }]
        );
    }

    /// Running status: the status byte is sent once and the data pairs that follow it
    /// belong to it, which is how a controller sends a phrase of note ons whose
    /// velocity-zero members are the releases.
    #[test]
    fn running_status_carries_the_pairs_that_follow_it() {
        assert_eq!(
            notes(&[0x90, 0x3C, 0x40, 0x3E, 0x50, 0x3C, 0x00]),
            [
                Note::On {
                    note: 60,
                    velocity: 64
                },
                Note::On {
                    note: 62,
                    velocity: 80
                },
                Note::Off { note: 60 },
            ]
        );
    }

    /// A message split across two reads is one message, and a byte arriving on its own
    /// completes what it belongs to.
    #[test]
    fn a_message_split_across_reads_is_still_one_message() {
        let mut stream = Stream::default();
        let mut heard = Vec::new();
        for part in [&[0x90u8][..], &[0x3C], &[0x64]] {
            stream.feed(part, |note| heard.push(note));
        }
        assert_eq!(
            heard,
            [Note::On {
                note: 60,
                velocity: 100
            }]
        );
    }

    /// A clock or active-sensing byte may land between the bytes of a note message. It
    /// is not a message of this stream's and does not disturb the one being read.
    #[test]
    fn a_real_time_byte_between_two_data_bytes_is_ignored() {
        assert_eq!(
            notes(&[0x90, 0xF8, 0x3C, 0xFE, 0x64, 0xF8]),
            [Note::On {
                note: 60,
                velocity: 100
            }]
        );
    }

    /// What arrives is not trusted to be a message. A data byte with no status in force
    /// is dropped rather than read as the note of whatever comes next.
    #[test]
    fn a_stray_data_byte_sounds_nothing() {
        assert!(notes(&[0x3C, 0x64]).is_empty());
        assert_eq!(
            notes(&[0x3C, 0x90, 0x3C, 0x64]),
            [Note::On {
                note: 60,
                velocity: 100
            }]
        );
    }

    /// System common and system exclusive end running status: the bytes of a dump are
    /// not the note numbers of the message before it.
    #[test]
    fn a_system_message_ends_the_running_status_before_it() {
        assert_eq!(
            notes(&[0x90, 0x3C, 0x64, 0xF0, 0x7E, 0x00, 0x3E, 0xF7, 0x3E, 0x50]),
            [Note::On {
                note: 60,
                velocity: 100
            }]
        );
    }

    /// The channel messages that are not notes are read past rather than mistaken for
    /// them: a controller's own data bytes must not become keys.
    #[test]
    fn other_channel_messages_sound_nothing_and_take_their_own_bytes() {
        // Control change (two data bytes), program change (one), pitch bend (two), and
        // then a note that must still be read as a note.
        assert_eq!(
            notes(&[0xB0, 0x07, 0x64, 0xC0, 0x05, 0xE0, 0x00, 0x40, 0x90, 0x3C, 0x64]),
            [Note::On {
                note: 60,
                velocity: 100
            }]
        );
    }

    /// One voice sounds, so a frame that caught a flurry plays the last key struck in
    /// it. A key let go is answered only when nothing was struck after it.
    #[test]
    fn a_frame_plays_its_last_strike_and_releases_only_what_outlived_it() {
        let on = |note, velocity| Note::On { note, velocity };
        assert_eq!(latest(&[]), None);
        assert_eq!(
            latest(&[on(60, 64), on(64, 70)]),
            Some(Played::Struck {
                note: 64,
                velocity: 70
            })
        );
        // Struck and let go inside one frame: it sounds rather than being lost.
        assert_eq!(
            latest(&[on(60, 64), Note::Off { note: 60 }]),
            Some(Played::Struck {
                note: 60,
                velocity: 64
            })
        );
        assert_eq!(
            latest(&[Note::Off { note: 62 }, Note::Off { note: 60 }]),
            Some(Played::Released { note: 60 })
        );
    }
}
