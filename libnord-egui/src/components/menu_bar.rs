use crate::components::Preview;
use crate::{Manager};


pub fn menu_bar(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    frame: &mut eframe::Frame,
    windows: &mut Manager,
) {
    egui::menu::bar(ui, |ui| {
        egui::widgets::global_dark_light_mode_switch(ui);

        ui.separator();

        ui.menu_button("File", |ui| {
            if ui.button("Open").clicked() {
                windows.open(Box::new(Preview::open()));
            }

            if ui.button("Quit").clicked() {
                frame.close()
            }
        });
    });
}
