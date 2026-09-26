//! Browser MIDI input: Web MIDI input ports, each with its own handler.
//!
//! ⚠️ `requestMIDIAccess()` asks the user for permission, and a browser lets the page ask
//! only while a click's user activation is live. The promise is created in the click and
//! awaited in a task, as the device chooser does (see [`crate::device::web`]).
//!
//! There is one thread, so a handler does what the desktop driver thread does: decode,
//! queue, and request a repaint. Nothing here waits.

use std::cell::RefCell;
use std::rc::Rc;

use eframe::egui;
use js_sys::Promise;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{MidiAccess, MidiInput, MidiMessageEvent};

use super::{Note, Queue, State, Stream};

/// One input port, and the handler the page calls when it sends a message.
struct Attached {
    /// ⚠️ Kept so the handler can be removed from its port. A closure dropped while the
    /// page still holds it throws the next time the port sends anything.
    input: MidiInput,
    handler: Closure<dyn FnMut(MidiMessageEvent)>,
}

#[derive(Default)]
struct Inner {
    queue: Queue,
    names: Vec<String>,
    attached: Vec<Attached>,
    /// The access the user granted, kept so the ports can be listed again when one is
    /// connected or disconnected.
    access: Option<MidiAccess>,
    /// ⚠️ Kept as long as the access that holds it.
    watch: Option<Closure<dyn FnMut()>>,
    asking: bool,
    failed: Option<String>,
}

#[derive(Default)]
pub struct Ports {
    inner: Rc<RefCell<Inner>>,
}

impl Ports {
    /// Whether the page has Web MIDI. Safari has none, and asking for it there throws.
    pub fn supported() -> bool {
        web_sys::window().is_some_and(|window| {
            js_sys::Reflect::has(&window.navigator(), &JsValue::from_str("requestMIDIAccess"))
                .unwrap_or(false)
        })
    }

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
            // ⚠️ The user may have turned MIDI off while the browser was asking; an
            // answer that arrives after that is ignored.
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
        inner.queue = Queue::default();
        inner.asking = false;
        inner.failed = None;
    }

    pub fn state(&self) -> State {
        let inner = self.inner.borrow();
        match (&inner.failed, inner.asking, inner.access.is_some()) {
            (Some(why), _, _) => State::Failed(why.clone()),
            (None, true, _) => State::Asking,
            (None, false, false) => State::Off,
            (None, false, true) => State::On {
                ports: inner.names.clone(),
                refused: Vec::new(),
            },
        }
    }

    /// The browser reports port changes, so the frame clock is unused. The queue uses the
    /// page's clock, which message event timestamps are measured on.
    pub fn drain(&mut self, _now: f64) -> Vec<Note> {
        let now = page_time();
        self.inner.borrow_mut().queue.drain(now)
    }
}

/// Request access. Called synchronously inside the click: awaiting anything first would
/// use up the user activation the browser requires.
fn request() -> Result<Promise, JsValue> {
    if !Ports::supported() {
        return Err(JsValue::from_str(super::UNSUPPORTED));
    }
    web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .navigator()
        .request_midi_access()
}

/// Attach a handler to every input port, removing any already attached.
///
/// Called again whenever a port is connected or disconnected, so it rebuilds the whole
/// set: the page may not touch a port that has gone.
fn attach(inner: &Rc<RefCell<Inner>>, access: &MidiAccess, ctx: &egui::Context) {
    detach(&mut inner.borrow_mut());
    let inputs = access.inputs();
    let Ok(Some(entries)) = js_sys::try_iter(inputs.as_ref()) else {
        return;
    };
    let mut attached = Vec::new();
    let mut names = Vec::new();
    for entry in entries.flatten() {
        // A maplike's iterator yields `[id, port]` pairs.
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
                let at = event.time_stamp() / 1000.0;
                let mut heard = false;
                stream.feed(&bytes, |note| {
                    heard = true;
                    held.borrow_mut().queue.push(at, note);
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

/// Remove every handler from its port before its closure is dropped.
fn detach(inner: &mut Inner) {
    for held in inner.attached.drain(..) {
        held.input.set_onmidimessage(None);
        drop(held.handler);
    }
    inner.names.clear();
}

/// Track the system's ports: the user expects a controller connected after access was
/// granted to play.
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

/// Seconds since the page's time origin, the clock an event's `timeStamp` uses. Without
/// a clock, everything drains as fresh.
fn page_time() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now() / 1000.0)
}

/// A rejected promise carries a `DOMException`, whose text must be read from its fields;
/// it does not downcast to `Error`.
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
