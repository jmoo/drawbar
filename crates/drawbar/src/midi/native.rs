//! Desktop MIDI in: one `midir` connection per input port the machine has.
//!
//! A connection's callback runs on the driver's own thread, so it does the least it can:
//! decode, queue, and wake the window.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use eframe::egui;
use midir::{Ignore, MidiInput, MidiInputConnection};

use super::{Queue, State, Stream};

/// The name this app gives itself in the machine's MIDI port list.
const CLIENT: &str = "drawbar";

/// How long an open port list is trusted, in seconds. A controller plugged in is noticed
/// on the first frame drawn after this, so hot-plug costs a look rather than a thread.
const RESCAN: f64 = 1.0;

/// One input port, open or refused. Its id is what the machine calls it, which is
/// stable across a rescan where its name is not.
struct Port {
    id: String,
    name: String,
}

struct Open {
    port: Port,
    /// ⚠️ Held for as long as the port is open: dropping the connection closes it.
    _connection: MidiInputConnection<()>,
}

/// One session of listening: the client the port list is read through, the queue the
/// connections fill, and the window to wake when they do.
///
/// ⚠️ One client for every look at the list. On Linux a client is an ALSA sequencer
/// client, and one made per look is a client that appears and vanishes every second.
struct Wire {
    listing: MidiInput,
    queue: Arc<Mutex<Queue>>,
    /// The queue's clock: seconds since this instant.
    epoch: Instant,
    ctx: egui::Context,
}

#[derive(Default)]
pub struct Ports {
    open: Vec<Open>,
    /// Ports that would not open, which another program may be holding. Not tried again
    /// until they leave the list, or listening starts over.
    refused: Vec<Port>,
    /// `None` while nothing is listening.
    wire: Option<Wire>,
    /// When the machine's port list was last read, on egui's frame clock.
    scanned: f64,
    failed: Option<String>,
}

impl Ports {
    pub fn supported() -> bool {
        true
    }

    pub fn listen(&mut self, ctx: &egui::Context) {
        self.stop();
        let listing = match MidiInput::new(CLIENT) {
            Ok(listing) => listing,
            Err(why) => {
                self.failed = Some(why.to_string());
                return;
            }
        };
        self.wire = Some(Wire {
            listing,
            queue: Arc::default(),
            epoch: Instant::now(),
            ctx: ctx.clone(),
        });
        self.scanned = f64::MIN;
    }

    pub fn stop(&mut self) {
        self.open.clear();
        self.refused.clear();
        self.wire = None;
        self.failed = None;
    }

    pub fn state(&self) -> State {
        match (&self.failed, &self.wire) {
            (Some(why), _) => State::Failed(why.clone()),
            (None, None) => State::Off,
            (None, Some(_)) => State::On {
                ports: self
                    .open
                    .iter()
                    .map(|open| open.port.name.clone())
                    .collect(),
                refused: self.refused.iter().map(|port| port.name.clone()).collect(),
            },
        }
    }

    pub fn drain(&mut self, now: f64) -> Vec<super::Note> {
        if self.wire.is_some() && now - self.scanned >= RESCAN {
            self.scanned = now;
            self.scan();
        }
        let Some(wire) = &self.wire else {
            return Vec::new();
        };
        let at = wire.epoch.elapsed().as_secs_f64();
        wire.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(at)
    }

    /// Open every input port that is neither open nor refused, and let go of the ones
    /// that have gone.
    fn scan(&mut self) {
        let Some(wire) = &self.wire else {
            return;
        };
        let live = wire.listing.ports();
        let listed = |port: &Port| live.iter().any(|live| live.id() == port.id);
        self.open.retain(|open| listed(&open.port));
        self.refused.retain(listed);
        for found in &live {
            let id = found.id();
            let mut known = self.open.iter().map(|open| &open.port).chain(&self.refused);
            if known.any(|port| port.id == id) {
                continue;
            }
            let port = Port {
                id,
                name: wire
                    .listing
                    .port_name(found)
                    .unwrap_or_else(|_| "unnamed port".to_string()),
            };
            match open(&port.id, wire) {
                Ok(Some(connection)) => self.open.push(Open {
                    port,
                    _connection: connection,
                }),
                // Gone between the list being read and the port being asked for.
                Ok(None) => {}
                Err(_) => self.refused.push(port),
            }
        }
    }
}

/// Connect to the port the machine calls `id`, through a client of its own: a `midir`
/// connection consumes the client it is made from.
fn open(id: &str, wire: &Wire) -> Result<Option<MidiInputConnection<()>>, String> {
    let mut input = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
    // System exclusive, timing and active sensing are not keys: a controller that sends
    // active sensing three times a second has nothing to say to an audition.
    input.ignore(Ignore::All);
    let Some(port) = input.find_port_by_id(id) else {
        return Ok(None);
    };
    let queue = wire.queue.clone();
    let epoch = wire.epoch;
    let ctx = wire.ctx.clone();
    let mut stream = Stream::default();
    input
        .connect(
            &port,
            CLIENT,
            move |_micros, bytes, ()| {
                let at = epoch.elapsed().as_secs_f64();
                let mut heard = false;
                // ⚠️ This is the driver's thread. The lock is only ever held to move a
                // few messages in or out, never across anything that waits.
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                stream.feed(bytes, |note| {
                    heard = true;
                    queue.push(at, note);
                });
                drop(queue);
                if heard {
                    ctx.request_repaint();
                }
            },
            (),
        )
        .map(Some)
        .map_err(|e| e.to_string())
}
