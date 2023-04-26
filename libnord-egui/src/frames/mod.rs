use egui::{Context, Ui};
use crate::{FrameDescriptor, FrameHandle, FrameRenderer};

mod preview;
pub use preview::*;


#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Window,
    Tab
}

impl FrameRenderer for Type {
    fn render<F>(&mut self, ctx: &Context, handle: FrameHandle<Self>, frame_contents: F) -> FrameHandle<Self> where F: FnOnce(&mut Ui) -> () {
        match self {
            Type::Window => {
                egui::Window::new(handle.title())
                    .id(handle.id().egui())
                    .title_bar(true)
                    .collapsible(true)
                    .resizable(true)
                    .show(ctx, frame_contents);
            },

            Type::Tab => {
                egui::Window::new(handle.title())
                    .id(handle.id().egui())
                    .title_bar(false)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                    .fixed_rect(ctx.available_rect())
                    .scroll2([true, true])
                    .show(ctx, frame_contents);
            }
        }

        handle
    }
}

pub type Manager = crate::FrameManager<Type>;