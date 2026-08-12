//! Browser link: the device chooser, and a pump that runs one command at a time.
//!
//! There is one thread, so the "worker" is a chain of `spawn_local` tasks: each takes
//! the transport out of the shared cell, runs one command, and puts it back before
//! starting the next. Holding a `RefCell` borrow across an `await` would panic the
//! moment the UI touched the same cell, so the transport is moved rather than borrowed.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use eframe::egui;
use js_sys::Promise;
use nord_usb::transport::{web::WebUsbTransport, VENDOR_ID};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{UsbConnectionEvent, UsbDevice, UsbDeviceFilter, UsbDeviceRequestOptions};

use super::worker::{self, Emit, Flow};
use super::{DeviceCard, DeviceCmd, DeviceEvent};

#[derive(Default)]
struct Inner {
    transport: Option<WebUsbTransport>,
    /// The device the chooser handed over, kept so the browser's own disconnect event
    /// can be told apart from another Clavia's. WebUSB hands back the same object for
    /// the same device, so this is an identity and not a description.
    device: Option<UsbDevice>,
    queue: VecDeque<DeviceCmd>,
    /// A command is running, so the transport is out of the cell.
    busy: bool,
    /// The device went away. Whatever is running is the last thing that runs.
    lost: bool,
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
        self.inner.borrow_mut().lost = false;
        if self.watch.is_none() {
            self.watch = watch_for_unplug(&self.inner, &self.emit);
        }

        let emit = self.emit.clone();
        let inner = self.inner.clone();
        spawn_local(async move {
            let device = match JsFuture::from(request).await {
                Ok(device) => device,
                Err(e) => {
                    emit.send(DeviceEvent::ConnectFailed(format!(
                        "no device chosen: {}",
                        describe(&e)
                    )));
                    return;
                }
            };
            // ⚠️ The desktop reads the identity over vendor control transfers on
            // endpoint 0. WebUSB can issue one, but nothing in `nord_usb`'s web
            // transport does, so the browser build shows none of it rather than a guess
            // at any of it.
            let card = DeviceCard {
                build: None,
                firmware: None,
                interface: None,
                kind: None,
                manufacturer: device.manufacturer_name(),
                max_transfer: None,
                product: device
                    .product_name()
                    .unwrap_or_else(|| "unnamed device".into()),
                product_id: device.product_id(),
                serial: device.serial_number(),
                vendor_id: device.vendor_id(),
            };
            let chosen = device.clone();
            match WebUsbTransport::open(device).await {
                Ok(transport) => {
                    let mut state = inner.borrow_mut();
                    state.transport = Some(transport);
                    state.device = Some(chosen);
                    drop(state);
                    emit.send(DeviceEvent::Connected(card));
                }
                Err(e) => emit.send(DeviceEvent::ConnectFailed(e.to_string())),
            }
            pump(&inner, &emit);
        });
    }

    pub fn disconnect(&mut self) {
        self.send(DeviceCmd::Disconnect);
    }

    pub fn send(&mut self, cmd: DeviceCmd) {
        self.inner.borrow_mut().queue.push_back(cmd);
        pump(&self.inner, &self.emit);
    }
}

/// Start the next queued command, if the transport is free.
fn pump(inner: &Rc<RefCell<Inner>>, emit: &Emit) {
    let (transport, cmd) = {
        let mut state = inner.borrow_mut();
        if state.busy || state.transport.is_none() {
            return;
        }
        let Some(cmd) = state.queue.pop_front() else {
            return;
        };
        state.busy = true;
        (state.transport.take(), cmd)
    };
    let Some(mut transport) = transport else {
        return;
    };

    let inner = inner.clone();
    let emit = emit.clone();
    spawn_local(async move {
        let flow = worker::run(&mut transport, cmd, &emit).await;
        if flow == Flow::Continue && !inner.borrow().lost {
            {
                let mut state = inner.borrow_mut();
                state.transport = Some(transport);
                state.busy = false;
            }
            return pump(&inner, &emit);
        }
        // ⚠️ Release the interface, or Nord Sound Manager and the desktop backend stay
        // locked out for as long as the tab is open. A device that has already gone has
        // nothing to release, and closing it can only fail.
        if flow == Flow::Released {
            if let Err(e) = transport.close().await {
                emit.send(DeviceEvent::OpFailed(e.to_string()));
            }
        }
        let said = {
            let mut state = inner.borrow_mut();
            state.busy = false;
            state.queue.clear();
            state.device = None;
            let said = state.lost;
            state.lost = said || flow == Flow::Lost;
            said
        };
        // The unplug event may have got here first, and one departure is one message.
        if !said {
            emit.send(DeviceEvent::Disconnected {
                lost: flow == Flow::Lost,
            });
        }
    });
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
    let usb = web_sys::window()?.navigator().usb();
    let held = inner.clone();
    let emit = emit.clone();
    let watch = Closure::wrap(Box::new(move |event: UsbConnectionEvent| {
        let went = event.device();
        // Another Clavia leaving the machine is not this one leaving.
        if held.borrow().device.as_ref() != Some(&went) {
            return;
        }
        let mut state = held.borrow_mut();
        state.queue.clear();
        state.device = None;
        // A transport whose device has gone cannot be closed, and there is nothing left
        // to hand back: dropping it is the whole of the cleanup. While a command is
        // running the transport is out of the cell, and the pump drops it on the way out.
        state.transport = None;
        let said = std::mem::replace(&mut state.lost, true);
        drop(state);
        if !said {
            emit.send(DeviceEvent::Disconnected { lost: true });
        }
    }) as Box<dyn FnMut(UsbConnectionEvent)>);
    usb.set_ondisconnect(Some(watch.as_ref().unchecked_ref()));
    Some(watch)
}

fn request_device() -> Result<Promise<UsbDevice>, JsValue> {
    let usb = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .navigator()
        .usb();

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
