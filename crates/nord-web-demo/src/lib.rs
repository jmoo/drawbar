//! Browser POC for [`nord_usb`]'s WebUSB backend: pick a Nord in Chrome's device
//! chooser and run read-only operations against it.
//!
//! ⚠️ **Read-only by construction.** The page never calls
//! `Session::allow_destructive_writes`, so no operation that changes the instrument is
//! reachable from here. This is first hardware contact for the WebUSB transport, and a
//! half-working write destroys patches.
//!
//! The device chooser is the reason this crate exists at all: `requestDevice()` needs
//! transient user activation, so device selection cannot live in `nord-usb` — only a
//! page can supply the click.

// A native build of the workspace reaches this crate too, where neither WebUSB nor
// `nord_usb::transport::web` exists. It compiles to an empty library there.
#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use nord_usb::transport::{web::WebUsbTransport, VENDOR_ID};
use nord_usb::{op, Location, ObjectClass, Session};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Document, HtmlInputElement, UsbDevice, UsbDeviceFilter, UsbDeviceRequestOptions};

/// Attach the click handler. Nothing touches USB until the button is pressed.
#[wasm_bindgen]
pub fn boot() -> Result<(), JsValue> {
    let button = document()?
        .get_element_by_id("connect")
        .ok_or_else(|| JsValue::from_str("the page has no #connect button"))?;

    let handler = Closure::<dyn FnMut()>::new(move || {
        let slot = match parse_slot(&input_value("slot")) {
            Ok(slot) => slot,
            Err(message) => return show(&message),
        };

        // ⚠️ `requestDevice()` must be called while the click's transient activation is
        // still live. Awaiting anything first — even an already-resolved promise —
        // spends it, and Chrome then rejects with `SecurityError`. So the promise is
        // taken here, synchronously, and only awaited inside the spawned task.
        let request = match request_device() {
            Ok(request) => request,
            Err(e) => return show(&js_text(&e)),
        };

        show("Pick the instrument in the chooser…");
        spawn_local(async move {
            match run(request, slot).await {
                Ok(report) => show(&report),
                Err(message) => show(&message),
            }
        });
    });

    button.add_event_listener_with_callback("click", handler.as_ref().unchecked_ref())?;
    // The listener has to outlive `boot`, and the page holds it for the tab's lifetime.
    handler.forget();
    show("Ready. Close Nord Sound Manager before connecting — it holds the interface.");
    Ok(())
}

fn request_device() -> Result<Promise<UsbDevice>, JsValue> {
    let usb = window()?.navigator().usb();

    // Filtering by vendor alone: the chooser then lists any Clavia device, and the
    // vendor-interface check in `WebUsbTransport::open` is what rejects a wrong one.
    let filter = UsbDeviceFilter::new();
    filter.set_vendor_id(VENDOR_ID);
    Ok(usb.request_device(&UsbDeviceRequestOptions::new(&[filter])))
}

async fn run(request: Promise<UsbDevice>, slot: Option<Location>) -> Result<String, String> {
    let device = JsFuture::from(request)
        .await
        .map_err(|e| format!("no device chosen: {}", js_text(&e)))?;

    let mut lines = vec![format!(
        "{} — {:04x}:{:04x}",
        device
            .product_name()
            .unwrap_or_else(|| "unnamed device".into()),
        device.vendor_id(),
        device.product_id(),
    )];

    let mut transport = WebUsbTransport::open(device).await.map_err(text)?;

    lines.push(String::new());
    for status in op::inventory(&mut transport).await.map_err(text)? {
        let slots = match status.slots() {
            Some(n) => format!(", {n} slots"),
            None => String::new(),
        };
        lines.push(format!(
            "{:<10} {:>4} items, {:>5.1}% used ({}/{} blocks{})",
            status.class.label(),
            status.count,
            status.used_percent(),
            status.used,
            status.total(),
            slots,
        ));
    }

    if let Some(at) = slot {
        let mut session = Session::open(&mut transport, ObjectClass::Program)
            .await
            .map_err(text)?;
        let info = op::info(&mut session, at).await;
        // ⚠️ Commit on the error path too. An abandoned transaction strands the
        // instrument on its progress screen until it is power-cycled.
        let closed = session.commit().await;
        let info = info.map_err(text)?;
        closed.map_err(text)?;

        lines.push(String::new());
        lines.push(format!(
            "{}:{}  {:?}  format={} version={} body={} bytes  crc32={}",
            info.location.bank + 1,
            info.location.slot + 1,
            info.name,
            info.format,
            info.version,
            info.body_len,
            info.crc32
                .map_or_else(|| "none".into(), |c| format!("{c:08x}")),
        ));
    }

    // Hand the interface back, or Nord Sound Manager and the CLI stay locked out for
    // as long as the tab is open.
    transport.close().await.map_err(text)?;
    Ok(lines.join("\n"))
}

/// The panel's one-indexed `bank:slot`, e.g. `7:4`. Empty means "skip the info step".
fn parse_slot(raw: &str) -> Result<Option<Location>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let (bank, slot) = raw
        .split_once(':')
        .ok_or_else(|| format!("a slot looks like 7:4; got {raw:?}"))?;
    let number = |s: &str| {
        s.trim()
            .parse::<u32>()
            .map_err(|_| format!("a slot looks like 7:4; got {raw:?}"))
    };
    let (bank, slot) = (number(bank)?, number(slot)?);
    // ⚠️ `Location::from_user` subtracts one from each, so zero underflows.
    if bank == 0 || slot == 0 {
        return Err("banks and slots count from 1, as on the panel".into());
    }
    Ok(Some(Location::from_user(bank, slot)))
}

fn text<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// A rejected promise carries a `DOMException`, whose text is on the object rather
/// than reachable by downcasting to `Error`.
fn js_text(err: &JsValue) -> String {
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

fn window() -> Result<web_sys::Window, JsValue> {
    web_sys::window().ok_or_else(|| JsValue::from_str("no window"))
}

fn document() -> Result<Document, JsValue> {
    window()?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))
}

fn input_value(id: &str) -> String {
    document()
        .ok()
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_default()
}

fn show(message: &str) {
    if let Some(out) = document().ok().and_then(|d| d.get_element_by_id("out")) {
        out.set_text_content(Some(message));
    }
}
