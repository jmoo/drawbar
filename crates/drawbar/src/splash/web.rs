//! The modal itself: the notice, the notes fetched from GitHub, and the version already
//! read.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};

use super::{classify, https, link, plain, title, Commit, Line, GAP, VERSION, WIDTH};
use crate::about::RELEASES;
use crate::app::warn;
use crate::icon::{icon, Glyph};

/// Which version's notes have already been read.
///
/// ⚠️ Written straight to `localStorage` rather than through [`eframe::Storage`]. eframe
/// 0.32's web runner saves on its auto-save timer and on focus loss, and subscribes its
/// save-on-close to `onbeforeunload` — a name `addEventListener` never fires. A tab
/// closed between two saves would lose the dismissal and open on this again.
const KEY: &str = "drawbar.splash";

const TAG: &str = concat!(
    "https://api.github.com/repos/jmoo/drawbar/releases/tags/drawbar-v",
    env!("CARGO_PKG_VERSION")
);

/// The most note text that is shown. A body past this is refused rather than cut: half a
/// note reads as a whole one.
const MOST: usize = 64 * 1024;

/// The most of the modal the notes may claim.
const NOTES: f32 = 320.0;

/// The alert beside the notice.
const GLYPH: f32 = 14.0;

enum Notes {
    /// Nobody has asked for them: this version's notice was read in an earlier session
    /// and Help has not reopened it.
    Unasked,
    Loading,
    Read {
        body: String,
        page: String,
    },
    /// No network, no such tag yet, a rate limit, or a body past [`MOST`].
    Unavailable,
}

pub struct Splash {
    showing: bool,
    notes: Notes,
    inbox: Receiver<Notes>,
    outbox: Sender<Notes>,
}

impl Splash {
    /// Open on the notice unless this version's notes have already been read.
    pub fn new(ctx: &egui::Context) -> Splash {
        let showing = seen().as_deref() != Some(VERSION);
        let (outbox, inbox) = channel();
        let notes = match showing {
            true => {
                fetch(ctx.clone(), outbox.clone());
                Notes::Loading
            }
            false => Notes::Unasked,
        };
        Splash {
            showing,
            notes,
            inbox,
            outbox,
        }
    }

    /// Show the notice again, reading the notes unless they are in hand or on the way.
    pub fn open(&mut self, ctx: &egui::Context) {
        self.showing = true;
        if matches!(self.notes, Notes::Loading | Notes::Read { .. }) {
            return;
        }
        self.notes = Notes::Loading;
        fetch(ctx.clone(), self.outbox.clone());
    }

    /// Draw the notice, if it is owed, and record the version once it is dismissed.
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.showing {
            return;
        }
        while let Ok(notes) = self.inbox.try_recv() {
            self.notes = notes;
        }
        if egui::Modal::new(egui::Id::new("splash"))
            .show(ctx, |ui| self.body(ui))
            .inner
        {
            self.showing = false;
            remember();
        }
    }

    /// Returns whether the reader is done with it.
    fn body(&self, ui: &mut egui::Ui) -> bool {
        ui.set_width(WIDTH);
        title(ui);
        ui.add_space(GAP);
        ui.horizontal(|ui| {
            let tint = warn(ui.visuals());
            icon(ui, Glyph::CircleAlert, GLYPH, tint);
            ui.label(
                egui::RichText::new("Use at your own risk, this is alpha software.").color(tint),
            );
        });
        ui.add_space(GAP * 2.0);
        ui.separator();
        self.paint_notes(ui);
        ui.separator();
        ui.add_space(GAP);
        let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
        let continued = ui
            .horizontal(|ui| {
                ui.add(egui::Button::new(egui::RichText::new("Continue").strong()))
                    .clicked()
            })
            .inner;
        continued || escaped
    }

    fn paint_notes(&self, ui: &mut egui::Ui) {
        ui.add_space(GAP);
        match &self.notes {
            Notes::Unasked | Notes::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Reading the release notes…").weak());
                });
            }
            Notes::Read { body, page } => {
                egui::ScrollArea::vertical()
                    .max_height(NOTES)
                    .show(ui, |ui| {
                        for line in body.lines() {
                            paint(ui, classify(line));
                        }
                        ui.add_space(GAP);
                        link(ui, "Release page", page);
                    });
            }
            Notes::Unavailable => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Release notes are unavailable.").weak());
                    link(ui, "Releases", RELEASES);
                });
            }
        }
        ui.add_space(GAP * 2.0);
    }
}

fn paint(ui: &mut egui::Ui, line: Line<'_>) {
    match line {
        Line::Blank => ui.add_space(GAP),
        Line::Heading(title) => {
            ui.add_space(GAP);
            ui.label(egui::RichText::new(title).strong());
        }
        Line::Item {
            scope,
            text,
            commit,
        } => bullet(ui, scope, text, commit),
        Line::Changelog(url) => link(ui, "Full changelog", url),
        Line::Text(text) => {
            ui.label(text);
        }
    }
}

fn bullet(ui: &mut egui::Ui, scope: Option<&str>, text: &str, commit: Option<Commit<'_>>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.label("•");
        if let Some(scope) = scope {
            ui.label(egui::RichText::new(format!("{scope}:")).strong());
        }
        ui.label(text);
        if let Some(commit) = commit {
            ui.add(
                egui::Hyperlink::from_label_and_url(
                    egui::RichText::new(commit.sha).weak(),
                    commit.url,
                )
                .open_in_new_tab(true),
            );
        }
    });
}

/// Read the notes off GitHub and hand them to whichever frame draws next.
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

/// A field of the reply, present only when it really is a string.
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
