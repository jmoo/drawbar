//! The browser's part: which sheet a session opens on unasked, and fetching the release
//! notes for the change list.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};

use super::{https, news, opening, plain, welcome, Notes, Opening, Wanted, VERSION};
use crate::about::RELEASES;
use crate::browser::Act;

/// Which version's sheet has already been dismissed.
///
/// ⚠️ Written straight to `localStorage`, bypassing [`eframe::Storage`]. eframe 0.32's
/// web runner saves on its auto-save timer and on focus loss, and subscribes its
/// save-on-close to `onbeforeunload`, a name `addEventListener` never fires. A tab closed
/// between two saves would lose the dismissal and show the sheet again.
const KEY: &str = "drawbar.splash";

const TAG: &str = concat!(
    "https://api.github.com/repos/jmoo/drawbar/releases/tags/drawbar-v",
    env!("CARGO_PKG_VERSION")
);

/// The longest release body shown. A longer one is refused, because a truncated note
/// would read as a complete one.
const MOST: usize = 64 * 1024;

/// Which sheet is up.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sheet {
    Welcome,
    News,
}

pub struct Splash {
    showing: Option<Sheet>,
    notes: Notes,
    inbox: Receiver<Notes>,
    outbox: Sender<Notes>,
}

impl Splash {
    /// Open on whichever sheet the version last read calls for.
    pub fn new(ctx: &egui::Context) -> Splash {
        let (outbox, inbox) = channel();
        let mut splash = Splash {
            showing: None,
            notes: Notes::Unasked,
            inbox,
            outbox,
        };
        match opening(seen().as_deref()) {
            Opening::Welcome => splash.open_welcome(),
            Opening::News => splash.open_news(ctx),
            Opening::Nothing => {}
        }
        splash
    }

    pub fn open_welcome(&mut self) {
        self.showing = Some(Sheet::Welcome);
    }

    /// Show the change list, fetching the notes unless they are loaded or loading.
    pub fn open_news(&mut self, ctx: &egui::Context) {
        self.showing = Some(Sheet::News);
        if matches!(self.notes, Notes::Loading | Notes::Read { .. }) {
            return;
        }
        self.notes = Notes::Loading;
        fetch(ctx.clone(), self.outbox.clone());
    }

    /// Draw whichever sheet is up, record the version once it is dismissed, and return
    /// what the reader asked for.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Act> {
        while let Ok(notes) = self.inbox.try_recv() {
            self.notes = notes;
        }
        match self.showing? {
            Sheet::Welcome => match welcome(ctx)? {
                Wanted::Done => {
                    self.dismiss();
                    None
                }
                Wanted::Act(act) => {
                    self.dismiss();
                    Some(act)
                }
            },
            Sheet::News => {
                if news(ctx, &self.notes) {
                    self.dismiss();
                }
                None
            }
        }
    }

    fn dismiss(&mut self) {
        self.showing = None;
        remember();
    }
}

/// Fetch the notes from GitHub and hand them to the next frame.
fn fetch(ctx: egui::Context, sender: Sender<Notes>) {
    spawn_local(async move {
        let notes = read(TAG).await.unwrap_or(Notes::Unavailable);
        let _ = sender.send(notes);
        ctx.request_repaint();
    });
}

async fn read(url: &str) -> Option<Notes> {
    let options = web_sys::RequestInit::new();
    options.set_method("GET");
    let request = web_sys::Request::new_with_str_and_init(url, &options).ok()?;
    request
        .headers()
        .set("Accept", "application/vnd.github+json")
        .ok()?;
    let response: web_sys::Response =
        JsFuture::from(web_sys::window()?.fetch_with_request(&request))
            .await
            .ok()?
            .dyn_into()
            .ok()?;
    if !response.ok() {
        return None;
    }
    let json = JsFuture::from(response.json().ok()?).await.ok()?;
    let body = text(&json, "body")?;
    if body.len() > MOST {
        return None;
    }
    let body = plain(&body);
    let page = text(&json, "html_url");
    let page = page
        .as_deref()
        .and_then(https)
        .unwrap_or(RELEASES)
        .to_owned();
    Some(Notes::Read { body, page })
}

/// A field of the reply, when it is a string.
fn text(json: &JsValue, field: &str) -> Option<String> {
    js_sys::Reflect::get(json, &JsValue::from_str(field))
        .ok()?
        .as_string()
}

fn store() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn seen() -> Option<String> {
    store()?.get_item(KEY).ok()?
}

fn remember() {
    if let Some(store) = store() {
        let _ = store.set_item(KEY, VERSION);
    }
}
