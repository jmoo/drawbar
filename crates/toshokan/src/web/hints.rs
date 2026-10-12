//! When to refresh or rescan: other tabs' commits, what the browser's watcher saw
//! change in the folder, and, for a folder another program may write, the moments
//! a user would expect to see its changes.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use js_sys::{Array, Function, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    BroadcastChannel, Document, FileSystemDirectoryHandle, MessageEvent, VisibilityState, Window,
};

use super::{deferred, describe, field, object, private_dir, wait, Folder};
use crate::ids::EntryHash;
use crate::io::IoError;
use crate::layout::is_swap_file;
use crate::path::RelPath;

/// What to read again, and why.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Hint {
    /// Another tab of this origin committed to the library; its newest entry. A
    /// refresh reads it.
    Committed(EntryHash),
    /// The browser's watcher saw these paths of the folder change, sorted: a
    /// rescan of the paths reads them, and a refresh, which it includes, reads
    /// what changed in toshokan's own directory.
    Changed(Vec<RelPath>),
    /// The page was shown or focused again, the period passed while it was
    /// visible, or the watcher could not say what changed: a program outside the
    /// browser may have written the folder, and a rescan of every file finds it.
    Look,
}

/// The most paths one [`Hint::Changed`] names; more become a [`Hint::Look`].
const MOST_CHANGED: usize = 1024;

/// Hints for one library, received in the order they came.
pub struct Hints {
    channel: BroadcastChannel,
    inbox: Rc<RefCell<Inbox>>,
    _hear: Closure<dyn FnMut(MessageEvent)>,
    looking: Option<Looking>,
    watching: Option<Watching>,
}

#[derive(Default)]
struct Inbox {
    hints: VecDeque<Hint>,
    /// Resolves the promise [`Hints::next`] waits on while no hint is queued.
    wake: Option<Function>,
}

impl Inbox {
    /// Queues `hint`, joined with a queued one it repeats: a look with a look, and
    /// changed paths with changed paths unless a look will read them anyway.
    fn push(&mut self, hint: Hint) {
        let looking = self.hints.contains(&Hint::Look);
        match hint {
            Hint::Look if looking => {}
            Hint::Changed(paths) if looking || paths.is_empty() => {}
            Hint::Changed(paths) => self.changed(paths),
            hint => self.hints.push_back(hint),
        }
        if let Some(wake) = self.wake.take() {
            let _ = wake.call0(&JsValue::NULL);
        }
    }

    fn changed(&mut self, paths: Vec<RelPath>) {
        let queued = self.hints.iter_mut().find_map(|hint| match hint {
            Hint::Changed(queued) => Some(queued),
            _ => None,
        });
        let mut joined: BTreeSet<RelPath> = paths.into_iter().collect();
        if let Some(queued) = queued {
            joined.extend(queued.drain(..));
            self.hints.retain(|hint| !matches!(hint, Hint::Changed(_)));
        }
        match joined.len() > MOST_CHANGED {
            true => self.hints.push_back(Hint::Look),
            false => self
                .hints
                .push_back(Hint::Changed(joined.into_iter().collect())),
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
            watching: None,
        })
    }

    /// Also hints what the browser's `FileSystemObserver` sees change in
    /// `folder`, where the browser has one; whether it does. Only Chromium has
    /// one, and it reports changes made through the browser's own file system
    /// handles; that it reports writes by other programs to a picked folder is
    /// reported by Chromium's documentation, not confirmed here.
    pub async fn watch(&mut self, folder: &Folder) -> Result<bool, IoError> {
        let Some(observer) = field(&js_sys::global(), "FileSystemObserver") else {
            return Ok(false);
        };
        let dir = match folder {
            Folder::Private(path) => private_dir(path).await?,
            Folder::Picked { dir, .. } => dir.clone(),
        };
        let watching = Watching::start(&self.inbox, observer.unchecked_ref(), &dir).await;
        self.watching = Some(watching.map_err(|error| IoError::Other(describe(&error)))?);
        Ok(true)
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
        if let Some(watching) = self.watching.take() {
            watching.stop();
        }
    }
}

/// A `FileSystemObserver` of the folder, disconnected on drop.
struct Watching {
    observer: JsValue,
    _saw: Closure<dyn FnMut(JsValue)>,
}

impl Watching {
    async fn start(
        inbox: &Rc<RefCell<Inbox>>,
        observer: &Function,
        dir: &FileSystemDirectoryHandle,
    ) -> Result<Self, JsValue> {
        let saw = {
            let inbox = Rc::clone(inbox);
            Closure::<dyn FnMut(JsValue)>::new(move |records: JsValue| {
                inbox.borrow_mut().push(hint(&records));
            })
        };
        let observer = Reflect::construct(observer, &Array::of1(saw.as_ref()))?;
        let observe: Function = Reflect::get(&observer, &"observe".into())?.dyn_into()?;
        let options = object(&[("recursive", JsValue::TRUE)]);
        let observing = observe.call2(&observer, dir, &options)?;
        wait(observing.dyn_into::<js_sys::Promise>()?).await?;
        Ok(Self {
            observer,
            _saw: saw,
        })
    }

    fn stop(self) {
        let disconnect =
            field(&self.observer, "disconnect").map(JsCast::unchecked_into::<Function>);
        if let Some(disconnect) = disconnect {
            let _ = disconnect.call0(&self.observer);
        }
    }
}

/// What a batch of the watcher's records asks: the paths they name, or a look
/// when one cannot say what changed.
fn hint(records: &JsValue) -> Hint {
    let Some(records) = records.dyn_ref::<Array>() else {
        return Hint::Look;
    };
    let mut paths = Vec::new();
    for record in records.iter() {
        let kind = field(&record, "type").and_then(|kind| kind.as_string());
        if !matches!(
            kind.as_deref(),
            Some("appeared" | "disappeared" | "modified" | "moved")
        ) {
            return Hint::Look;
        }
        for member in ["relativePathComponents", "relativePathMovedFrom"] {
            let Some(components) = field(&record, member) else {
                continue;
            };
            match changed_path(&components) {
                Some(path) if path.name().is_some_and(is_swap_file) => {}
                Some(path) => paths.push(path),
                None => return Hint::Look,
            }
        }
    }
    paths.sort();
    paths.dedup();
    Hint::Changed(paths)
}

/// The path a record's components name; `None` for the folder itself or a name
/// toshokan cannot hold.
fn changed_path(components: &JsValue) -> Option<RelPath> {
    let names: Vec<String> = components
        .dyn_ref::<Array>()?
        .iter()
        .map(|name| name.as_string())
        .collect::<Option<_>>()?;
    if names.is_empty() {
        return None;
    }
    RelPath::new(&names.join("/")).ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(paths: &[&str]) -> Hint {
        Hint::Changed(paths.iter().map(|at| RelPath::new(at).unwrap()).collect())
    }

    #[test]
    fn queued_hints_join_and_a_look_takes_in_changed_paths() {
        let mut inbox = Inbox::default();
        inbox.push(changed(&["b", "a"]));
        inbox.push(Hint::Committed(EntryHash::from_u128(1)));
        inbox.push(changed(&["c", "a"]));
        inbox.push(changed(&[]));
        assert_eq!(
            Vec::from(inbox.hints.clone()),
            [
                Hint::Committed(EntryHash::from_u128(1)),
                changed(&["a", "b", "c"])
            ]
        );
        inbox.push(Hint::Look);
        inbox.push(changed(&["d"]));
        inbox.push(Hint::Look);
        assert_eq!(
            inbox
                .hints
                .iter()
                .filter(|hint| **hint == Hint::Look)
                .count(),
            1
        );
        assert!(!inbox.hints.contains(&changed(&["d"])));

        let mut inbox = Inbox::default();
        let many: Vec<String> = (0..=MOST_CHANGED).map(|n| format!("f{n}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        inbox.push(changed(&many));
        assert_eq!(Vec::from(inbox.hints), [Hint::Look], "too many to name");
    }
}
