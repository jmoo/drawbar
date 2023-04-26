use std::collections::HashMap;
use std::hash::Hash;
use egui::{Id, TextBuffer};
use crate::frames::Type;


pub trait Frame<T> where T: FrameRenderer {
    fn render(&mut self, ctx: &egui::Context, handle: FrameHandle<T>) -> FrameHandle<T>;
}

pub trait FrameRenderer: Clone + Hash + Eq {
    fn render<F>(&mut self, ctx: &egui::Context, handle: FrameHandle<Self>, frame_contents: F) -> FrameHandle<Self> where F: FnMut(&mut egui::Ui);
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct FrameId {
    inner: u64,
}

impl FrameId {
    pub fn new(index: u64) -> Self {
        Self { inner: index }
    }

    pub fn inner(&self) -> u64 {
        self.inner
    }

    pub fn egui(&self) -> Id {
        Id::new(self.inner)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameDescriptor {
    pub id: FrameId,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameHandle<T> where T: FrameRenderer {
    descriptor: FrameDescriptor,
    renderer: T,
    open: bool,
}

impl <T> FrameHandle<T> where T: FrameRenderer {
    pub fn id(&self) -> FrameId {
        self.descriptor.id
    }

    pub fn title(&self) -> String {
        self.descriptor.title.as_str().to_string()
    }

    pub fn set_title(&mut self, title: String) {
        self.descriptor.title = title.to_string();
    }

    pub fn descriptor(&self) -> FrameDescriptor {
        self.descriptor.clone()
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn commit<F>(&mut self, ctx: &egui::Context, contents: F) -> Self where F: FnMut(&mut egui::Ui) -> () {
        let Self { descriptor, renderer, open } = self;
        renderer.render(ctx, Self {
            descriptor: descriptor.clone(),
            renderer: renderer.clone(),
            open: *open,
        }, contents)
    }
}

pub struct Ref<T> where T: FrameRenderer {
    pub inner: Box<dyn Frame<T>>,
    pub handle: FrameHandle<T>,
}

pub struct FrameIdGenerator {
    next_id: u64,
}

impl FrameIdGenerator {
    pub fn next(&mut self) -> FrameId {
        self.next_id += 1;
        FrameId { inner: self.next_id }
    }
}

pub struct FrameManager<T> where T: FrameRenderer {
    references: HashMap<u64, Ref<T>>,
    id_generator: FrameIdGenerator,
}

impl<T> FrameManager<T> where T: FrameRenderer {
    pub fn new() -> Self {
        Self {
            references: HashMap::default(),
            id_generator: FrameIdGenerator { next_id: 0 },
        }
    }

    pub fn open(&mut self, renderer: T, frame: Box<dyn Frame<T>>) -> FrameDescriptor {
        let frame_descriptor = FrameDescriptor {
            id: self.id_generator.next(),
            title: String::default(),
        };

        self.references.insert(
            frame_descriptor.id.inner(),
            Ref {
                inner: frame,
                handle: FrameHandle {
                    open: true,
                    descriptor: frame_descriptor.clone(),
                    renderer: *Box::new(renderer.clone()),
                },
            },
        );

        frame_descriptor.clone()
    }

    pub fn render(&mut self, renderer: T, ctx: &egui::Context) {
        let mut cleanup: Vec<u64> = Vec::new();

        let Self { references, id_generator: _ } = self;

        {
            for (index, item) in references.iter_mut() {
                let handle = item.handle.clone();
                let renderer = item.handle.renderer.clone();

                let result = match renderer {
                    renderer => item.inner.render(ctx, handle),
                    _ => handle,
                };

                item.handle = result;

                if !item.handle.open {
                    cleanup.push(*index);
                }
            }
        }

        for index in cleanup {
            references.remove(&index);
        }
    }
}
