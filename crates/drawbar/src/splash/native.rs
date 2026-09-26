//! The welcome sheet on the desktop, which remembers no version and fetches no notes.

use eframe::egui;

use super::{welcome, Wanted};
use crate::browser::Act;

pub struct Splash {
    showing: bool,
}

impl Splash {
    /// The desktop app opens without a sheet; Help opens the welcome.
    pub fn new(_ctx: &egui::Context) -> Splash {
        Splash { showing: false }
    }

    pub fn open_welcome(&mut self) {
        self.showing = true;
    }

    /// Draw the sheet while it is up, and return whatever the reader asked for.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Act> {
        if !self.showing {
            return None;
        }
        match welcome(ctx)? {
            Wanted::Done => self.showing = false,
            Wanted::Act(act) => {
                self.showing = false;
                return Some(act);
            }
        }
        None
    }
}
