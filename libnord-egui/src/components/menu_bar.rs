use std::io::stdout;
use crate::frames::{Manager, Preview, Type};

pub fn menu_bar(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    frame: &mut eframe::Frame,
    frames: &mut Manager,
) {
    egui::menu::bar(ui, |ui| {
        egui::widgets::global_dark_light_mode_switch(ui);

        ui.separator();

        ui.menu_button("File", |ui| {
            if ui.button("Open").clicked() {
                frames.open(Type::Tab, Box::new(Preview::open()));
            }

            if ui.button("Quit").clicked() {
                frame.close()
            }
        });

        // tabs.iter().for_each(|tab| {
        //     let index = tab.handle.index();
        //
        //     ui.menu_button(format!("tab #{}", index), |ui| {
        //         if ui.button("Close").clicked() {
        //             tabs.close(index);
        //         }
        //     });
        // });
    });
}
