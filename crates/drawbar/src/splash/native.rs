//! The welcome sheet in a window: no version to remember, and no notes to fetch.

use eframe::egui;

use super::{welcome, Wanted};
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
    pub fn show(&mut self, ctx: &egui::Context, usb: bool) -> Option<Act> {
        if !self.showing {
            return None;
        }
        match welcome(ctx, usb)? {
            Wanted::Done => self.showing = false,
            Wanted::Act(act) => {
                self.showing = false;
                return Some(act);
            }
        }
        None
    }
}
