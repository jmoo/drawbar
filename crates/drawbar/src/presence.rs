//! Whether the user is at the page, so the library can look for changes made outside
//! drawbar when they come back.
//!
//! egui counts the page focused while its canvas is the focused element, which it stays
//! while the user is in another window, so the page hears the window's own `focus` and
//! `blur`, and the document's `visibilitychange`, instead.

/// The user's comings and goings, as heard between two frames.
pub struct Presence {
    here: bool,
    /// The user left since the last frame read it, whether or not they are back.
    left: bool,
}

impl Default for Presence {
    fn default() -> Presence {
        Presence {
            here: true,
            left: false,
        }
    }
}

impl Presence {
    /// The browser says the user is at the page, or away from it.
    pub fn heard(&mut self, here: bool) {
        self.left |= !here;
        self.here = here;
    }

    /// Whether the user is at the page, read once a frame. A user who left and came back
    /// between two frames reads as away for one frame, so the return is not missed.
    pub fn read(&mut self) -> bool {
        !std::mem::take(&mut self.left) && self.here
    }
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static PRESENCE: std::cell::RefCell<Presence> = std::cell::RefCell::default();
    static LISTENING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`Presence::read`] for this page, listening from the first call. A frame is asked
/// for whenever the user comes or goes, and after a frame that read a return as away.
#[cfg(target_arch = "wasm32")]
pub fn here(ctx: &eframe::egui::Context) -> bool {
    listen(ctx);
    PRESENCE.with_borrow_mut(|presence| {
        let here = presence.read();
        if here != presence.here {
            ctx.request_repaint();
        }
        here
    })
}

#[cfg(target_arch = "wasm32")]
fn listen(ctx: &eframe::egui::Context) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast as _;

    if LISTENING.replace(true) {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let ctx = ctx.clone();
    let heard = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let here = match event.type_().as_str() {
            "blur" => false,
            "focus" => true,
            _ => web_sys::window()
                .and_then(|window| window.document())
                .is_none_or(|document| !document.hidden()),
        };
        PRESENCE.with_borrow_mut(|presence| presence.heard(here));
        ctx.request_repaint();
    });
    let callback = heard.as_ref().unchecked_ref();
    let _ = window.add_event_listener_with_callback("blur", callback);
    let _ = window.add_event_listener_with_callback("focus", callback);
    if let Some(document) = window.document() {
        let _ = document.add_event_listener_with_callback("visibilitychange", callback);
    }
    // The page keeps the listeners for as long as it lives.
    heard.forget();
}
