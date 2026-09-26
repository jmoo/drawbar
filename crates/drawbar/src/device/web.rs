//! Browser link: the device chooser, and a pump that runs one command at a time.
//!
//! There is one thread, so the "worker" is a chain of `spawn_local` tasks: each takes
//! the device out of the shared cell, runs one command, and puts it back before
//! starting the next. Holding a `RefCell` borrow across an `await` would panic the
//! moment the UI touched the same cell, so the device is moved rather than borrowed.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use eframe::egui;
use js_sys::Promise;
use nord_usb::device::Device;
use nord_usb::transport::{web::WebUsbTransport, VENDOR_ID};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Usb, UsbConnectionEvent, UsbDevice, UsbDeviceFilter, UsbDeviceRequestOptions};

use super::worker::{self, Emit, Flow};
use super::{DeviceCard, DeviceCmd, DeviceEvent};

/// Where the connection stands, as one value: the device is in exactly one place.
#[derive(Default)]
enum Slot {
    #[default]
    Absent,
    /// Attached and free.
    Idle(Device<WebUsbTransport>),
    /// A running command has it.
    Busy,
    /// It went away, or was let go. Whatever is running is the last thing that runs.
    Gone,
}

impl Slot {
    /// Take the device out and mark the slot busy. Every other state is left as it is.
    fn take(&mut self) -> Option<Device<WebUsbTransport>> {
        match std::mem::replace(self, Slot::Busy) {
            Slot::Idle(device) => Some(device),
            held => {
                *self = held;
                None
            }
        }
    }
}

#[derive(Default)]
struct Inner {
    slot: Slot,
    /// The device the chooser handed over, kept so the browser's own disconnect event
    /// can be told apart from another Clavia's. WebUSB hands back the same object for
    /// the same device, so this is an identity and not a description.
    chosen: Option<UsbDevice>,
    queue: VecDeque<DeviceCmd>,
    /// Bumped by every [`Link::connect`]. ⚠️ A task spawned under an older number
    /// belongs to a connection that is over, and finishes into a cell another
    /// connection owns: its result is dropped rather than applied.
    generation: u64,
}

impl Inner {
    /// The next queued command and the device to run it on, if there is both. A device
    /// with nothing to run goes back where it was.
    fn start(&mut self) -> Option<(Device<WebUsbTransport>, DeviceCmd)> {
        let device = self.slot.take()?;
        match self.queue.pop_front() {
            Some(cmd) => Some((device, cmd)),
            None => {
                self.slot = Slot::Idle(device);
                None
            }
        }
    }
}

pub struct Link {
    emit: Emit,
    inner: Rc<RefCell<Inner>>,
    /// ⚠️ Held for as long as the link is. A closure handed to JS and then dropped here
    /// leaves the page calling into freed memory the next time the event fires.
    watch: Option<Closure<dyn FnMut(UsbConnectionEvent)>>,
}

impl Link {
    pub fn new(ctx: egui::Context, events: Sender<DeviceEvent>) -> Link {
        Link {
            emit: Emit::new(events, ctx),
            inner: Rc::new(RefCell::new(Inner::default())),
            watch: None,
        }
    }

    /// Open the chooser and, once a device comes back, claim its vendor interface.
    ///
    /// ⚠️ `requestDevice()` must be called while the click's transient user activation
    /// is still live. Awaiting anything first — even an already-resolved promise —
    /// spends it, and Chrome then rejects with `SecurityError`. So the promise is taken
    /// here, synchronously, and only awaited inside the spawned task.
    pub fn connect(&mut self) {
        let request = match request_device() {
            Ok(request) => request,
            Err(e) => {
                self.emit.send(DeviceEvent::ConnectFailed(describe(&e)));
                return;
            }
        };
        // Nothing queued against the last instrument is owed by this one.
        let generation = {
            let mut state = self.inner.borrow_mut();
            state.queue.clear();
            state.slot = Slot::Absent;
            state.generation = state.generation.wrapping_add(1);
            state.generation
        };
        if self.watch.is_none() {
            self.watch = watch_for_unplug(&self.inner, &self.emit);
        }

        let emit = self.emit.clone();
        let inner = self.inner.clone();
        spawn_local(async move {
            let chosen = match JsFuture::from(request).await {
                Ok(chosen) => chosen,
                Err(e) => {
                    emit.send(DeviceEvent::ConnectFailed(format!(
                        "no device chosen: {}",
                        describe(&e)
                    )));
                    return;
                }
            };
            // ⚠️ The WebUSB transport does not expose the endpoint-0 identity request.
            let card = DeviceCard {
                build: None,
                firmware: None,
                interface: None,
                kind: None,
                manufacturer: chosen.manufacturer_name(),
                max_transfer: None,
                product: chosen
                    .product_name()
                    .unwrap_or_else(|| "unnamed device".into()),
                product_id: chosen.product_id(),
                serial: chosen.serial_number(),
                vendor_id: chosen.vendor_id(),
            };
            match WebUsbTransport::open(chosen.clone()).await {
                Ok(transport) => {
                    let mut device = Device::new(transport);
                    emit.send(DeviceEvent::Connected(card));
                    // ⚠️ Awaited before the device goes into the cell — a borrow held
                    // across an await panics the moment the UI touches the same cell.
                    let flow = worker::announce(&mut device, &emit).await;
                    let keep = flow == Flow::Continue && inner.borrow().generation == generation;
                    match keep {
                        true => {
                            let mut state = inner.borrow_mut();
                            state.slot = Slot::Idle(device);
                            state.chosen = Some(chosen);
                        }
                        false => retire(&inner, &emit, device, flow, generation).await,
                    }
                }
                Err(e) => emit.send(DeviceEvent::ConnectFailed(e.to_string())),
            }
            pump(&inner, &emit);
        });
    }

    pub fn available() -> bool {
        usb().is_some()
    }

    pub fn disconnect(&mut self) {
        self.send(DeviceCmd::Disconnect);
    }

    pub fn send(&mut self, cmd: DeviceCmd) {
        self.inner.borrow_mut().queue.push_back(cmd);
        pump(&self.inner, &self.emit);
    }
}

/// Start the next queued command, if the device is free.
fn pump(inner: &Rc<RefCell<Inner>>, emit: &Emit) {
    let (started, generation) = {
        let mut state = inner.borrow_mut();
        let generation = state.generation;
        (state.start(), generation)
    };
    let Some((mut device, cmd)) = started else {
        return;
    };

    let inner = inner.clone();
    let emit = emit.clone();
    spawn_local(async move {
        let flow = worker::run(&mut device, cmd, &emit).await;
        let carry_on = {
            let state = inner.borrow();
            flow == Flow::Continue
                && state.generation == generation
                && !matches!(state.slot, Slot::Gone)
        };
        if carry_on {
            inner.borrow_mut().slot = Slot::Idle(device);
            return pump(&inner, &emit);
        }
        retire(&inner, &emit, device, flow, generation).await;
    });
}

/// The end of a connection: release the interface, drop what was queued against it, and
/// say once that the instrument has gone.
///
/// ⚠️ A task of an older generation lands here after another connection has taken the
/// cell. It closes its own transport, because that interface is claimed either way, and
/// touches nothing else.
async fn retire(
    inner: &Rc<RefCell<Inner>>,
    emit: &Emit,
    device: Device<WebUsbTransport>,
    flow: Flow,
    generation: u64,
) {
    // ⚠️ Release a connected interface so other hosts are not locked out.
    let closed = device.into_transport().close().await;
    let said = {
        let mut state = inner.borrow_mut();
        if state.generation != generation {
            return;
        }
        state.queue.clear();
        state.chosen = None;
        let said = matches!(state.slot, Slot::Gone);
        state.slot = Slot::Gone;
        said
    };
    // A device that is already gone cannot be closed, and saying so over its departure
    // is noise rather than news.
    if let (Err(e), false) = (closed, flow == Flow::Lost) {
        emit.send(DeviceEvent::OpFailed(e.to_string()));
    }
    // The unplug event may have got here first, and one departure is one message.
    if !said {
        emit.send(DeviceEvent::Disconnected {
            lost: flow == Flow::Lost,
        });
    }
}

/// Subscribe to the browser's own "that device is gone" event.
///
/// ⚠️ Without this a pulled cable is invisible until something is attempted. Nothing in
/// the app asks the browser whether the device is still there, so the instrument's column
/// would sit answering clicks with nothing behind it until one of them failed.
fn watch_for_unplug(
    inner: &Rc<RefCell<Inner>>,
    emit: &Emit,
) -> Option<Closure<dyn FnMut(UsbConnectionEvent)>> {
    let usb = usb()?;
    let held = inner.clone();
    let emit = emit.clone();
    let watch = Closure::wrap(Box::new(move |event: UsbConnectionEvent| {
        let went = event.device();
        // Another Clavia leaving the machine is not this one leaving.
        if held.borrow().chosen.as_ref() != Some(&went) {
            return;
        }
        let mut state = held.borrow_mut();
        state.queue.clear();
        state.chosen = None;
        // An unplugged device cannot be closed; whichever owner holds it drops it.
        let said = matches!(state.slot, Slot::Gone);
        state.slot = Slot::Gone;
        drop(state);
        if !said {
            emit.send(DeviceEvent::Disconnected { lost: true });
        }
    }) as Box<dyn FnMut(UsbConnectionEvent)>);
    usb.set_ondisconnect(Some(watch.as_ref().unchecked_ref()));
    Some(watch)
}

/// The browser's WebUSB entry point, where it has one.
///
/// ⚠️ `Navigator::usb` hands back `undefined` where there is none, and the first call on
/// it throws through the wasm frames of whatever called it. eframe's frame is one of
/// them: the exception skips the release of its runner borrow, and every later frame
/// finds the runner held and paints nothing.
fn usb() -> Option<Usb> {
    let navigator = web_sys::window()?.navigator();
    js_sys::Reflect::get(&navigator, &JsValue::from_str("usb"))
        .ok()?
        .dyn_into()
        .ok()
}

fn request_device() -> Result<Promise<UsbDevice>, JsValue> {
    let usb = usb().ok_or_else(|| JsValue::from_str("this browser has no WebUSB"))?;

    // Filtering by vendor alone: the chooser then lists any Clavia device, and the
    // vendor-interface check in `WebUsbTransport::open` is what rejects a wrong one.
    let filter = UsbDeviceFilter::new();
    filter.set_vendor_id(VENDOR_ID);
    Ok(usb.request_device(&UsbDeviceRequestOptions::new(&[filter])))
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
