use egui::epaint::ahash::HashMap;
use egui::Id;

pub trait Window {
    fn render(&mut self, ctx: &egui::Context, reference: &mut Handle);
}

pub struct Handle {
    open: bool,
    id: Id,
}

impl Handle {
    pub fn id(&self) -> Id {
        self.id
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

pub struct Ref {
    pub inner: Box<dyn Window>,
    pub handle: Handle,
}

pub struct Manager {
    windows: HashMap<u64, Ref>,
    next_id: u64,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            windows: HashMap::default(),
            next_id: 0,
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn open(&mut self, window: Box<dyn Window>) -> Handle {
        let id = self.id();
        let hash = Id::new(id);
        let open = true;

        self.windows.insert(
            id,
            Ref {
                inner: window,
                handle: Handle { open, id: hash },
            },
        );

        Handle { open, id: hash }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let Self {
            windows,
            next_id: _,
        } = self;
        let mut cleanup: Vec<u64> = Vec::new();

        for (id, item) in windows {
            item.inner.render(ctx, &mut item.handle);

            if !item.handle.open {
                cleanup.push(*id);
            }
        }

        let Self {
            windows,
            next_id: _,
        } = self;
        for id in cleanup {
            windows.remove(&id);
        }
    }
}
