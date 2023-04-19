use crate::components::close_button;
use crate::Handle;

use libnord::Song;

pub struct Preview<'a> {
    song: &'a Song,
    name: String,
}

impl<'a> Preview<'a> {
    pub fn new(name: String, song: &'a Song) -> Self {
        Self { name, song }
    }

    pub fn render(&mut self, ctx: &egui::Context, handle: &mut Handle) {
        egui::Window::new(format!("song: {}", self.name))
            .id(handle.id())
            .title_bar(true)
            .collapsible(true)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(format!("{:?}", self.song));
                close_button(ui, handle)
            });
    }
}
