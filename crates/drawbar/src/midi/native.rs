//! Desktop MIDI input: one `midir` connection per input port.
//!
//! A connection's callback runs on the driver's thread, so it does as little as possible:
//! decode, queue, and wake the window.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use eframe::egui;
use midir::{Ignore, MidiInput, MidiInputConnection};

use super::{Queue, State, Stream};

/// The name this app gives itself in the system's MIDI port list.
const CLIENT: &str = "drawbar";

/// How long a port list is trusted, in seconds. A newly connected controller is noticed
/// on the first frame after this, so hot-plugging costs a poll, not a thread.
const RESCAN: f64 = 1.0;

/// One input port, open or refused. Its id is the system's identifier, which stays stable
/// across a rescan when its name may not.
struct Port {
    id: String,
    name: String,
}

struct Open {
    port: Port,
    /// ⚠️ Held for as long as the port is open: dropping the connection closes it.
    _connection: MidiInputConnection<()>,
}

/// One listening session: the client that reads the port list, the queue the connections
/// fill, and the window to wake when they do.
///
/// ⚠️ One client for every read of the list. On Linux a client is an ALSA sequencer
/// client, and one per read would appear and vanish every second.
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
    /// Ports that would not open, perhaps because another program holds them. Not
    /// retried until they leave the list or listening restarts.
    refused: Vec<Port>,
    /// `None` while nothing is listening.
    wire: Option<Wire>,
    /// When the system's port list was last read, on egui's frame clock.
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

    /// Open every input port that is neither open nor refused, and drop the ones that
    /// have disappeared.
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
                // Removed between reading the list and opening the port.
                Ok(None) => {}
                Err(_) => self.refused.push(port),
            }
        }
    }
}

/// Connect to the port with system id `id`, through a new client: a `midir` connection
/// consumes the client it is made from.
fn open(id: &str, wire: &Wire) -> Result<Option<MidiInputConnection<()>>, String> {
    let mut input = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
    // System exclusive, timing, and active sensing carry no notes, and a controller may
    // send active sensing three times a second.
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
