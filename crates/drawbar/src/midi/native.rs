//! Desktop MIDI in: one `midir` connection per input port the machine has.
//!
//! A connection's callback runs on the driver's own thread, so it does the least it can:
//! decode, queue, and wake the window. The queue is bounded, because a machine that is
//! playing has no use for the keys it struck while nothing was being painted.

use std::sync::mpsc::{self, Receiver, SyncSender};

use eframe::egui;
use midir::{Ignore, MidiInput, MidiInputConnection};

use super::{Note, State, Stream, QUEUE};

/// The name this app gives itself in the machine's MIDI port list.
const CLIENT: &str = "drawbar";

/// How long an open port list is trusted, in seconds. A controller plugged in is noticed
/// on the first frame drawn after this, so hot-plug costs a look rather than a thread.
const RESCAN: f64 = 1.0;

/// One open input port.
struct Open {
    /// What the machine calls this port, which is stable across a rescan where its name
    /// is not.
    id: String,
    name: String,
    /// ⚠️ Held for as long as the port is open: dropping the connection closes it.
    _connection: MidiInputConnection<()>,
}

/// The queue one session of listening fills, and the window to wake when it does.
struct Wire {
    tx: SyncSender<Note>,
    rx: Receiver<Note>,
    ctx: egui::Context,
}

#[derive(Default)]
pub struct Ports {
    open: Vec<Open>,
    names: Vec<String>,
    /// `None` while nothing is listening.
    wire: Option<Wire>,
    /// When the machine's port list was last read, on egui's frame clock.
    scanned: f64,
    failed: Option<String>,
}

impl Ports {
    pub fn listen(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        self.wire = Some(Wire {
            tx,
            rx,
            ctx: ctx.clone(),
        });
        self.failed = None;
        self.scanned = f64::MIN;
        if let Err(why) = self.scan() {
            self.stop();
            self.failed = Some(why);
        }
    }

    pub fn stop(&mut self) {
        self.open.clear();
        self.names.clear();
        self.wire = None;
        self.failed = None;
    }

    pub fn state(&self) -> State {
        match (&self.failed, &self.wire) {
            (Some(why), _) => State::Failed(why.clone()),
            (None, None) => State::Off,
            (None, Some(_)) => State::On(self.names.clone()),
        }
    }

    pub fn drain(&mut self, now: f64) -> Vec<Note> {
        if self.wire.is_some() && now - self.scanned >= RESCAN {
            self.scanned = now;
            // A machine that cannot be asked for its ports keeps the ones already open:
            // the driver is busy, not gone, and the reader is told nothing they can act
            // on.
            let _ = self.scan();
        }
        match &self.wire {
            Some(wire) => wire.rx.try_iter().collect(),
            None => Vec::new(),
        }
    }

    /// Open every input port that is not open yet, and let go of the ones that have
    /// gone. A port that refuses to open is left out rather than ending the session.
    fn scan(&mut self) -> Result<(), String> {
        let Some(wire) = &self.wire else {
            return Ok(());
        };
        let listing = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
        let live: Vec<String> = listing
            .ports()
            .iter()
            .map(midir::MidiInputPort::id)
            .collect();
        self.open.retain(|open| live.contains(&open.id));
        for id in live {
            if self.open.iter().any(|open| open.id == id) {
                continue;
            }
            if let Some(open) = open(&id, wire) {
                self.open.push(open);
            }
        }
        self.names = self.open.iter().map(|open| open.name.clone()).collect();
        Ok(())
    }
}

fn open(id: &str, wire: &Wire) -> Option<Open> {
    let mut input = MidiInput::new(CLIENT).ok()?;
    // System exclusive, timing and active sensing are not keys: a controller that sends
    // active sensing three times a second has nothing to say to an audition.
    input.ignore(Ignore::All);
    let port = input.find_port_by_id(id)?;
    let name = input
        .port_name(&port)
        .unwrap_or_else(|_| "unnamed port".to_string());
    let tx = wire.tx.clone();
    let ctx = wire.ctx.clone();
    let mut stream = Stream::default();
    let connection = input
        .connect(
            &port,
            CLIENT,
            move |_micros, bytes, ()| {
                let mut heard = false;
                stream.feed(bytes, |note| {
                    heard = true;
                    // A full queue drops what would sound late. ⚠️ Never block: this is
                    // the driver's thread.
                    let _ = tx.try_send(note);
                });
                if heard {
                    ctx.request_repaint();
                }
            },
            (),
        )
        .ok()?;
    Some(Open {
        id: id.to_string(),
        name,
        _connection: connection,
    })
}
