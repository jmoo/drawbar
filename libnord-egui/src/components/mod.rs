mod menu_bar;

use egui::Ui;
pub use menu_bar::*;

mod preview;

pub use preview::*;
use crate::Handle;

mod song;

pub fn close_button(ui: &mut Ui, handler: &mut Handle) {
    if ui
        .button("close")
        .on_hover_text("Close this window")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked() {
        handler.close();
    }
}