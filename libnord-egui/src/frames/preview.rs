use libnord::Entity;
use poll_promise::Promise;
use rfd::AsyncFileDialog;

use std::io::Cursor;
use egui::{Context, Ui};
use crate::{Frame, FrameHandle, FrameRenderer};

pub struct Preview {
    promise: Promise<Result<(String, Entity), String>>,
    preview: bool,
    close: bool
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
            close: false
        }
    }

    pub fn new(name: String, entity: Entity) -> Self {
        Self {
            promise: Promise::from_ready(Ok((name, entity))),
            preview: false,
            close: false
        }
    }
}

impl<T> Frame<T> for Preview where T: FrameRenderer {
    fn render(&mut self, ctx: &Context, mut handle: FrameHandle<T>) -> FrameHandle<T> {
        if self.close {
            handle.close();
        }

        if let Some(promise) = self.promise.ready() {
            match promise {
                Ok((name, entity)) => match entity {
                    Entity::Song(song) => {
                        handle.set_title(format!("song: {}", name.to_string()));

                        handle.commit(ctx, |ui| {
                            ui.label(format!("{:?}", song));

                            if ui
                                .button("close")
                                .on_hover_text("Close this window")
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.close = true;
                                println!("close!")
                            }
                        })
                    }
                    _ => handle.commit(ctx, |ui: &mut egui::Ui| {
                        ui.label(format!("{:?}", entity));

                        if ui
                            .button("close")
                            .on_hover_text("Close this window")
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                             self.close = true;
                        }
                    })
                },

                Err(e) => handle.commit(ctx, |ui: &mut Ui| {
                    ui.label(format!("{:?}", e));

                    if ui
                        .button("close")
                        .on_hover_text("Close this window")
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.close = true;
                    }
                })
            }
        } else {
            handle.commit(ctx, |ui| {
                ui.label("loading...");

                if ui
                    .button("close")
                    .on_hover_text("Close this window")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                   self.close = true;
                }
            })
        }
    }
}
