//! Browser MIDI in: Web MIDI's input ports, each with a handler of its own.
//!
//! ⚠️ `requestMIDIAccess()` asks the reader for permission, and a browser only lets the
//! page ask while a click's user activation is live. The promise is taken in the click
//! and awaited in a task, as the device chooser is — see [`crate::device::web`].
//!
//! There is one thread, so a handler does what the desktop's driver thread does: decode,
//! queue, and ask for a repaint. Nothing here waits for anything.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use eframe::egui;
use js_sys::Promise;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{MidiAccess, MidiInput, MidiMessageEvent};

use super::{Note, State, Stream, QUEUE};

/// One input port, and the handler the page calls when it sends something.
struct Attached {
    /// ⚠️ Held so the handler can be taken off the port it was put on. A closure dropped
    /// while the page still holds it throws the next time the port sends anything.
    input: MidiInput,
    handler: Closure<dyn FnMut(MidiMessageEvent)>,
}

#[derive(Default)]
struct Inner {
    queue: VecDeque<Note>,
    names: Vec<String>,
    attached: Vec<Attached>,
    /// The permission the reader gave, kept so the ports can be read again when one
    /// comes or goes.
    access: Option<MidiAccess>,
    /// ⚠️ Held for as long as the access it was handed to.
    watch: Option<Closure<dyn FnMut()>>,
    asking: bool,
    failed: Option<String>,
}

#[derive(Default)]
pub struct Ports {
    inner: Rc<RefCell<Inner>>,
}

impl Ports {
    pub fn listen(&mut self, ctx: &egui::Context) {
        let request = match request() {
            Ok(request) => request,
            Err(e) => {
                self.inner.borrow_mut().failed = Some(describe(&e));
                return;
            }
        };
        {
            let mut inner = self.inner.borrow_mut();
            inner.asking = true;
            inner.failed = None;
        }

        let held = self.inner.clone();
        let ctx = ctx.clone();
        spawn_local(async move {
            let answer = JsFuture::from(request).await;
            // ⚠️ The reader may have turned MIDI off while the browser was asking, and
            // an answer that lands after that belongs to nothing.
            if !held.borrow().asking {
                return;
            }
            match answer {
                Ok(granted) => {
                    let access: MidiAccess = granted.unchecked_into();
                    attach(&held, &access, &ctx);
                    watch(&held, &access, &ctx);
                    let mut inner = held.borrow_mut();
                    inner.access = Some(access);
                    inner.asking = false;
                }
                Err(e) => {
                    let mut inner = held.borrow_mut();
                    inner.failed = Some(describe(&e));
                    inner.asking = false;
                }
            }
            ctx.request_repaint();
        });
    }

    pub fn stop(&mut self) {
        let mut inner = self.inner.borrow_mut();
        detach(&mut inner);
        if let Some(access) = inner.access.take() {
            access.set_onstatechange(None);
        }
        inner.watch = None;
        inner.queue.clear();
        inner.asking = false;
        inner.failed = None;
    }

    pub fn state(&self) -> State {
        let inner = self.inner.borrow();
        match (&inner.failed, inner.asking, inner.access.is_some()) {
            (Some(why), _, _) => State::Failed(why.clone()),
            (None, true, _) => State::Asking,
            (None, false, false) => State::Off,
            (None, false, true) => State::On(inner.names.clone()),
        }
    }

    pub fn drain(&mut self, _now: f64) -> Vec<Note> {
        // The browser says when a port comes or goes, so nothing here is timed.
        self.inner.borrow_mut().queue.drain(..).collect()
    }
}

/// Ask for access. Taken synchronously, inside the click: awaiting anything first spends
/// the user activation the browser requires.
fn request() -> Result<Promise, JsValue> {
    web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .navigator()
        .request_midi_access()
}

/// Put a handler on every input port, taking off the ones already placed.
///
/// Called again whenever a port comes or goes, so it must be the whole truth rather than
/// a difference: a port that has gone is one the page may not touch again.
fn attach(inner: &Rc<RefCell<Inner>>, access: &MidiAccess, ctx: &egui::Context) {
    detach(&mut inner.borrow_mut());
    let inputs = access.inputs();
    let Ok(Some(entries)) = js_sys::try_iter(inputs.as_ref()) else {
        return;
    };
    let mut attached = Vec::new();
    let mut names = Vec::new();
    for entry in entries.flatten() {
        // A maplike's iterator yields `[id, port]`, and the port is what is listened to.
        let Ok(input) = js_sys::Array::from(&entry).get(1).dyn_into::<MidiInput>() else {
            continue;
        };
        names.push(input.name().unwrap_or_else(|| "unnamed port".to_string()));
        let held = inner.clone();
        let ctx = ctx.clone();
        let mut stream = Stream::default();
        let handler =
            Closure::<dyn FnMut(MidiMessageEvent)>::new(move |event: MidiMessageEvent| {
                let Ok(bytes) = event.data() else {
                    return;
                };
                let mut heard = false;
                stream.feed(&bytes, |note| {
                    heard = true;
                    let mut inner = held.borrow_mut();
                    // A full queue drops what would sound late.
                    if inner.queue.len() < QUEUE {
                        inner.queue.push_back(note);
                    }
                });
                if heard {
                    ctx.request_repaint();
                }
            });
        // Setting a message handler is what opens a Web MIDI input port.
        input.set_onmidimessage(Some(handler.as_ref().unchecked_ref()));
        attached.push(Attached { input, handler });
    }
    let mut inner = inner.borrow_mut();
    inner.attached = attached;
    inner.names = names;
}

/// Take every handler off the port it was put on, before the closure behind it goes.
fn detach(inner: &mut Inner) {
    for held in inner.attached.drain(..) {
        held.input.set_onmidimessage(None);
        drop(held.handler);
    }
    inner.names.clear();
}

/// Follow the ports the machine has: a controller plugged in after access was given is
/// one the reader expects to be able to play.
fn watch(inner: &Rc<RefCell<Inner>>, access: &MidiAccess, ctx: &egui::Context) {
    let held = inner.clone();
    let following = access.clone();
    let ctx = ctx.clone();
    let watch = Closure::<dyn FnMut()>::new(move || {
        attach(&held, &following, &ctx);
        ctx.request_repaint();
    });
    access.set_onstatechange(Some(watch.as_ref().unchecked_ref()));
    inner.borrow_mut().watch = Some(watch);
}

/// A rejected promise carries a `DOMException`, whose text is on the object rather than
/// reachable by downcasting to `Error`.
fn describe(err: &JsValue) -> String {
    let field = |k: &str| {
        js_sys::Reflect::get(err, &JsValue::from_str(k))
            .ok()
            .and_then(|v| v.as_string())
    };
    match (field("name"), field("message")) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => err.as_string().unwrap_or_else(|| format!("{err:?}")),
    }
}
