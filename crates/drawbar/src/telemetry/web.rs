//! The browser half: whether this page may report, the queue, and the beacon.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::Storage;

use super::{firsts, instrument, language, record, Event, Undelivered, ENDPOINT, ORIGIN};
use crate::js::field;

/// `off` while the operator has turned sharing off. `index.html` reads the same key.
const SWITCH: &str = "drawbar.telemetry";

/// The UTC date of this browser's last visit, `YYYY-MM-DD`: the only thing the app keeps
/// for counting visitors. It is not an identifier, and it is gone once sharing is off.
const LAST_VISIT: &str = "drawbar.last-visit";

/// A collector to report to from a page not on [`ORIGIN`], for testing a local build:
/// `localStorage.setItem("drawbar.telemetry.collector", "http://127.0.0.1:8787")`.
///
/// ⚠️ Ignored on [`ORIGIN`], which reports to [`ENDPOINT`] and nowhere else.
const DEV_COLLECTOR: &str = "drawbar.telemetry.collector";

/// Written and removed once a page, to learn whether storage keeps a write.
const PROBE: &str = "drawbar.telemetry.probe";

/// Rows held while the page is offline or between flushes. The oldest go first.
const HELD: usize = 50;

/// How often held rows go out, in milliseconds.
const EVERY: i32 = 30_000;

thread_local! {
    static QUEUE: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
    static VISITED: Cell<bool> = const { Cell::new(false) };
    /// Turned off in this page, which holds even where storage refused to keep it.
    static TURNED_OFF: Cell<bool> = const { Cell::new(false) };
    /// Whether storage took the [`PROBE`], once asked.
    static WRITABLE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether this page reports, and if not, why.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sharing {
    On,
    /// Turned off in the Help menu.
    Off,
    /// The browser sends Do Not Track or Global Privacy Control.
    Refused,
    /// Not served from [`ORIGIN`], and no [`DEV_COLLECTOR`] named: a local or forked
    /// build.
    Elsewhere,
}

/// Where this page reports: [`ENDPOINT`] on [`ORIGIN`], else the [`DEV_COLLECTOR`] if one
/// is named.
fn collector() -> Option<String> {
    let origin = web_sys::window()?.location().origin().ok()?;
    match origin == ORIGIN {
        true => Some(ENDPOINT.to_string()),
        false => storage()?.get_item(DEV_COLLECTOR).ok().flatten(),
    }
}

pub fn sharing() -> Sharing {
    let Some(window) = web_sys::window() else {
        return Sharing::Elsewhere;
    };
    if collector().is_none() {
        return Sharing::Elsewhere;
    }
    if refused(&window.navigator()) {
        return Sharing::Refused;
    }
    if TURNED_OFF.with(Cell::get) {
        return Sharing::Off;
    }
    // ⚠️ Off when storage is blocked or will not take a write: a switch turned off there
    // could not be kept, and the next load would read it as on.
    let Some(storage) = storage() else {
        return Sharing::Off;
    };
    if !writable(&storage) {
        return Sharing::Off;
    }
    match storage.get_item(SWITCH) {
        Ok(None) => Sharing::On,
        Ok(Some(_)) | Err(_) => Sharing::Off,
    }
}

/// Whether `storage` keeps a write, asked once a page and again after a write fails.
fn writable(storage: &Storage) -> bool {
    if let Some(known) = WRITABLE.with(Cell::get) {
        return known;
    }
    let took = storage.set_item(PROBE, "1").is_ok() && storage.remove_item(PROBE).is_ok();
    WRITABLE.with(|writable| writable.set(Some(took)));
    took
}

/// Do Not Track or Global Privacy Control.
///
/// ⚠️ Both are read as properties. Chrome's `doNotTrack` is `null` when unset, and
/// `Navigator::do_not_track` then throws through the wasm frames of whatever called it,
/// eframe's frame among them, which leaves its runner borrowed and the page frozen.
fn refused(navigator: &web_sys::Navigator) -> bool {
    let dnt = field(navigator, "doNotTrack").and_then(|value| value.as_string());
    let gpc = field(navigator, "globalPrivacyControl").and_then(|value| value.as_bool());
    dnt.as_deref() == Some("1") || gpc == Some(true)
}

fn storage() -> Option<Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Turn sharing on or off. Off drops what is held and forgets the last visit.
pub fn share(on: bool) {
    TURNED_OFF.with(|off| off.set(!on));
    if !on {
        QUEUE.with(|queue| {
            if let Ok(mut queue) = queue.try_borrow_mut() {
                queue.clear();
            }
        });
    }
    let Some(storage) = storage() else {
        return;
    };
    match on {
        true => {
            let _ = storage.remove_item(SWITCH);
        }
        false => {
            if storage.set_item(SWITCH, "off").is_err() {
                WRITABLE.with(|writable| writable.set(Some(false)));
            }
            let _ = storage.remove_item(LAST_VISIT);
        }
    }
}

/// Hold a row for the next flush, if this page reports.
pub(super) fn queue(event: &Event) {
    if sharing() != Sharing::On {
        return;
    }
    QUEUE.with(|queue| {
        // ⚠️ A panic inside a flush reaches here with the queue borrowed.
        let Ok(mut queue) = queue.try_borrow_mut() else {
            return;
        };
        if queue.len() == HELD {
            queue.pop_front();
        }
        queue.push_back(event.json());
    });
}

/// Send what is held in one beacon. Offline, rows stay held; a beacon the browser refuses
/// is dropped, never retried. Rows held when sharing went off, here or in another tab,
/// are dropped unsent.
fn flush() {
    let Some(window) = web_sys::window() else {
        return;
    };
    if sharing() != Sharing::On {
        QUEUE.with(|queue| {
            if let Ok(mut queue) = queue.try_borrow_mut() {
                queue.clear();
            }
        });
        return;
    }
    let navigator = window.navigator();
    if !navigator.on_line() {
        return;
    }
    let rows = QUEUE.with(|queue| {
        queue
            .try_borrow_mut()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    });
    if rows.is_empty() {
        return;
    }
    let body = format!("[{}]", Vec::from(rows).join(","));
    let Some(collector) = collector() else {
        return;
    };
    let _ = navigator.send_beacon_with_opt_str(&format!("{collector}/e"), Some(&body));
}

/// Start reporting: the panic hook, and the flushes on a timer and when the page is
/// hidden or left. Call once, after eframe's `WebRunner::new`, whose own panic hook this
/// one runs ahead of.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(at) = info.location() {
            record(Event::Panic {
                location: format!("{}:{}:{}", at.file(), at.line(), at.column()),
                instrument: instrument(),
            });
            flush();
        }
        previous(info);
    }));

    let Some(window) = web_sys::window() else {
        return;
    };
    // Held for the life of the page, which is the life of the app.
    let tick = Closure::<dyn FnMut()>::new(flush);
    let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
        tick.as_ref().unchecked_ref(),
        EVERY,
    );
    tick.forget();
    let hidden = Closure::<dyn FnMut()>::new(|| {
        let hidden = web_sys::window()
            .and_then(|window| window.document())
            .is_some_and(|document| document.hidden());
        if hidden {
            flush();
        }
    });
    if let Some(document) = window.document() {
        let _ = document
            .add_event_listener_with_callback("visibilitychange", hidden.as_ref().unchecked_ref());
    }
    hidden.forget();
    let leaving = Closure::<dyn FnMut()>::new(flush);
    let _ = window.add_event_listener_with_callback("pagehide", leaving.as_ref().unchecked_ref());
    leaving.forget();
}

/// Report this page load, once. `fits` is whether the window is large enough for the
/// app, `webusb` whether the browser can reach an instrument.
pub fn visit(fits: bool, webusb: bool) {
    if VISITED.with(|visited| visited.replace(true)) || sharing() != Sharing::On {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let today = today();
    let last = storage().and_then(|storage| storage.get_item(LAST_VISIT).ok().flatten());
    if let Some(storage) = storage() {
        let _ = storage.set_item(LAST_VISIT, &today);
    }
    let (first_day, first_month) = firsts(last.as_deref(), &today);
    let navigator = window.navigator();
    record(Event::Visit {
        navigation: navigation(&window),
        first_day,
        first_month,
        webusb,
        fits,
        referrer: referrer(&window),
        language: language(navigator.language().as_deref()),
    });
}

/// Today's UTC date as `YYYY-MM-DD`.
fn today() -> String {
    let iso: String = js_sys::Date::new_0().to_iso_string().into();
    iso.chars().take(10).collect()
}

/// How the browser says this page was loaded: `navigate`, `reload`, `back_forward` or
/// `prerender`.
fn navigation(window: &web_sys::Window) -> String {
    window
        .performance()
        .map(|performance| performance.get_entries_by_type("navigation"))
        .and_then(|entries| field(&entries.get(0), "type"))
        .and_then(|kind| kind.as_string())
        .unwrap_or_default()
}

/// The host of the page that linked here, unless it is this site.
fn referrer(window: &web_sys::Window) -> String {
    let Some(text) = window.document().map(|document| document.referrer()) else {
        return String::new();
    };
    let host = web_sys::Url::new(&text)
        .map(|url| url.hostname())
        .unwrap_or_default();
    match ORIGIN.ends_with(&format!("//{host}")) {
        true => String::new(),
        false => host,
    }
}

/// Whether the browser believes it is online.
pub fn online() -> bool {
    web_sys::window().is_some_and(|window| window.navigator().on_line())
}

/// Send a report, `body` being its JSON. Sent whether or not sharing is on: the operator
/// asked for it.
pub async fn submit(body: String) -> Result<(), Undelivered> {
    let window = web_sys::window().ok_or(Undelivered::Unreachable)?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    // `text/plain` is a simple request, so the browser sends no preflight.
    init.set_body(&JsValue::from_str(&body));
    let collector = collector().unwrap_or_else(|| ENDPOINT.to_string());
    let request = web_sys::Request::new_with_str_and_init(&format!("{collector}/report"), &init)
        .map_err(|_| Undelivered::Unreachable)?;
    let _ = request.headers().set("content-type", "text/plain");
    let answer = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| Undelivered::Unreachable)?;
    let response: web_sys::Response = answer.dyn_into().map_err(|_| Undelivered::Unreachable)?;
    match response.ok() {
        true => Ok(()),
        false => Err(Undelivered::Refused(response.status())),
    }
}

/// A report's id, from the browser's cryptographic random source.
pub fn report_id() -> String {
    let mut random = [0u8; 10];
    if let Some(crypto) = web_sys::window().and_then(|window| window.crypto().ok()) {
        let _ = crypto.get_random_values_with_u8_array(&mut random);
    }
    super::id_from(&random)
}
