//! The welcome sheet in a window: no version to remember, and no notes to fetch.

use eframe::egui;

use super::{welcome, Wanted, VERSION};
use crate::browser::Act;

pub struct Splash {
    showing: bool,
}

impl Splash {
    /// A window opens on the app itself; the welcome is Help's to ask for.
    pub fn new(_ctx: &egui::Context) -> Splash {
        Splash { showing: false }
    }

    pub fn open_welcome(&mut self) {
        self.showing = true;
    }

    /// Draw the sheet while it is up, and hand on whatever the reader asked for.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Act> {
        if !self.showing {
            return None;
        }
        match welcome(ctx)? {
            Wanted::Done => self.showing = false,
            // Nothing here can fetch the notes, so this goes where Help ▸ What's new goes.
            Wanted::News => {
                let page = crate::about::release_page(VERSION);
                ctx.open_url(egui::OpenUrl::new_tab(page));
            }
            Wanted::Act(act) => {
                self.showing = false;
                return Some(act);
            }
        }
        None
    }
}
