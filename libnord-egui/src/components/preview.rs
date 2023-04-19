use crate::components::{close_button, song};
use crate::{Handle, Window};



use libnord::{Entity};
use poll_promise::Promise;
use rfd::{AsyncFileDialog};

use std::io::Cursor;




pub struct Preview {
    promise: Promise<Result<(String, Entity), String>>,
    preview: bool,
}

impl Preview {
    pub fn open() -> Self {
        Self {
            promise: Promise::spawn_thread("open file", move || {
                pollster::block_on(async {
                    if let Some(file) = AsyncFileDialog::new().pick_file().await {
                        let name = file.file_name();

                        return if let mut contents = file.read().await {
                            match libnord::from_stream(&mut Cursor::new(&mut contents)) {
                                Ok(entity) => Ok((name, entity)),
                                Err(e) => Err(e.to_string()),
                            }
                        } else {
                            Err("failed loading file".to_string())
                        };
                    }

                    Err("cancelled".to_string())
                })
            }),
            preview: true,
        }
    }

    pub fn new(name: String, entity: Entity) -> Self {
        Self {
            promise: Promise::from_ready(Ok((name, entity))),
            preview: false,
        }
    }
}

impl Window for Preview {
    fn render(&mut self, ctx: &egui::Context, handle: &mut Handle) {
        if let Some(promise) = self.promise.ready() {
            return match promise {
                Ok((name, entity)) => match entity {
                    Entity::Song(song) => {
                        song::Preview::new(name.to_string(), song).render(ctx, handle);
                    }
                    _ => {
                        egui::Window::new("preview")
                            .id(handle.id())
                            .title_bar(true)
                            .collapsible(true)
                            .resizable(true)
                            .show(ctx, |ui| {
                                ui.label(format!("{:?}", entity));
                                close_button(ui, handle)
                            });
                    }
                },

                Err(e) => {
                    egui::Window::new("error")
                        .id(handle.id())
                        .title_bar(true)
                        .show(ctx, |ui| {
                            ui.label(e);

                            close_button(ui, handle)
                        });
                }
            };
        }

        egui::Window::new("loading")
            .id(handle.id())
            .title_bar(false)
            .show(ctx, |ui| {
                ui.spinner();
            });
    }
}
