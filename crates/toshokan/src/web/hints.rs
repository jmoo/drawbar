//! When to refresh: other tabs' commits, and, for a folder another program may
//! write, the moments a user would expect to see its changes.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use js_sys::Function;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{BroadcastChannel, Document, MessageEvent, VisibilityState, Window};

use super::{deferred, describe, wait};
use crate::ids::EntryHash;
use crate::io::IoError;

/// Why to refresh.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hint {
    /// Another tab of this origin committed to the library; its newest entry.
    Committed(EntryHash),
    /// The page was shown or focused again, or the period passed while it was
    /// visible: a program outside the browser may have written the folder.
    Look,
}

/// Hints for one library, received in the order they came.
pub struct Hints {
    channel: BroadcastChannel,
    inbox: Rc<RefCell<Inbox>>,
    _hear: Closure<dyn FnMut(MessageEvent)>,
    looking: Option<Looking>,
}

#[derive(Default)]
struct Inbox {
    hints: VecDeque<Hint>,
    /// Resolves the promise [`Hints::next`] waits on while no hint is queued.
    wake: Option<Function>,
}

impl Inbox {
    fn push(&mut self, hint: Hint) {
        if hint == Hint::Look && self.hints.contains(&Hint::Look) {
            return;
        }
        self.hints.push_back(hint);
        if let Some(wake) = self.wake.take() {
            let _ = wake.call0(&JsValue::NULL);
        }
    }
}

/// The page's listeners and timer for [`Hint::Look`], removed on drop.
struct Looking {
    window: Window,
    document: Document,
    look: Closure<dyn FnMut()>,
    timer: i32,
}

impl Hints {
    /// Hints for the library `name` names, the same in every tab, such as the
    /// local root's path. With `look_every`, also [`Hint::Look`] on the page's
    /// window: at focus, when shown, and every `look_every` while visible.
    pub fn new(name: &str, look_every: Option<Duration>) -> Result<Self, IoError> {
        let other = |error: JsValue| IoError::Other(describe(&error));
        let channel = BroadcastChannel::new(&format!("toshokan:{name}")).map_err(other)?;
        let inbox = Rc::new(RefCell::new(Inbox::default()));
        let hear = {
            let inbox = Rc::clone(&inbox);
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let entry = event.data().as_string().and_then(|text| text.parse().ok());
                if let Some(entry) = entry {
                    inbox.borrow_mut().push(Hint::Committed(entry));
                }
            })
        };
        channel.set_onmessage(Some(hear.as_ref().unchecked_ref()));
        let looking = match look_every {
            Some(period) => Some(Looking::start(&inbox, period).map_err(other)?),
            None => None,
        };
        Ok(Self {
            channel,
            inbox,
            _hear: hear,
            looking,
        })
    }

    /// Tells the library's other tabs that `entry` is this tab's newest commit.
    pub fn announce(&self, entry: EntryHash) {
        let _ = self
            .channel
            .post_message(&JsValue::from_str(&entry.to_string()));
    }

    /// The next hint, once one comes.
    pub async fn next(&mut self) -> Hint {
        loop {
            if let Some(hint) = self.inbox.borrow_mut().hints.pop_front() {
                return hint;
            }
            let (woken, wake) = deferred();
            self.inbox.borrow_mut().wake = Some(wake);
            let _ = wait(woken).await;
        }
    }
}

impl Drop for Hints {
    fn drop(&mut self) {
        self.channel.set_onmessage(None);
        self.channel.close();
        if let Some(looking) = self.looking.take() {
            looking.stop();
        }
    }
}

impl Looking {
    fn start(inbox: &Rc<RefCell<Inbox>>, period: Duration) -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or("looking needs the page's window")?;
        let document = window
            .document()
            .ok_or("looking needs the page's document")?;
        let look = {
            let (inbox, document) = (Rc::clone(inbox), document.clone());
            Closure::<dyn FnMut()>::new(move || {
                if document.visibility_state() == VisibilityState::Visible {
                    inbox.borrow_mut().push(Hint::Look);
                }
            })
        };
        let function: &Function = look.as_ref().unchecked_ref();
        window.add_event_listener_with_callback("focus", function)?;
        document.add_event_listener_with_callback("visibilitychange", function)?;
        let period = i32::try_from(period.as_millis()).unwrap_or(i32::MAX);
        let timer =
            window.set_interval_with_callback_and_timeout_and_arguments_0(function, period)?;
        Ok(Self {
            window,
            document,
            look,
            timer,
        })
    }

    fn stop(self) {
        let function: &Function = self.look.as_ref().unchecked_ref();
        let _ = self
            .window
            .remove_event_listener_with_callback("focus", function);
        let _ = self
            .document
            .remove_event_listener_with_callback("visibilitychange", function);
        self.window.clear_interval_with_handle(self.timer);
    }
}
