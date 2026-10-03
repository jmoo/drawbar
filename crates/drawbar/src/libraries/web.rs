//! The libraries a browser tab opens: its own storage, and folders on this computer the
//! user picked, where the browser lets a page pick one.
//!
//! A picked folder is known by its handle, which only IndexedDB can keep, so the list
//! of those opened lately is kept there rather than in eframe's store. A handle comes
//! back without its permission: the browser asks the user again, and only from a click.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;
use js_sys::{Array, Object, Promise, Reflect};
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DirectoryPickerOptions, FileSystemDirectoryHandle, FileSystemHandle, FileSystemPermissionMode,
    IdbTransactionMode,
};

use super::{Picking, THIS_COMPUTER};
use crate::folders::Library;
use crate::idb::{committed, database, done, LIBRARIES as STORE};
use crate::js::{describe, field};
use crate::store::{permission, Picked, Root};

const KEY: &str = "recent";

/// Whether this browser lets a page open a folder on this computer. Brave has
/// `showDirectoryPicker` only once a flag turns it on.
pub fn picking() -> Picking {
    let Some(window) = web_sys::window() else {
        return Picking::Absent;
    };
    if field(&window, "showDirectoryPicker").is_some() {
        return Picking::On;
    }
    match field(&window.navigator().into(), "brave") {
        Some(_) => Picking::TurnedOff,
        None => Picking::Absent,
    }
}

/// What the libraries' requests came back with.
pub enum Heard {
    /// The list the last session left, most recent first, and whether the browser still
    /// lets drawbar into the folder at its head without asking.
    Restored { recent: Vec<Root>, permitted: bool },
    /// A library the user chose, which may be opened now.
    Open(Root),
    /// Something the user asked for did not happen, and why, in their terms.
    Trouble(String),
}

/// The libraries opened lately, and the requests about them still to answer.
pub struct Libraries {
    /// Most recent first.
    recent: Vec<Root>,
    tx: Sender<Heard>,
    rx: Receiver<Heard>,
}

impl Default for Libraries {
    fn default() -> Libraries {
        let (tx, rx) = channel();
        Libraries {
            recent: Vec::new(),
            tx,
            rx,
        }
    }
}

impl Libraries {
    const MOST: usize = 10;

    /// Read back the list the last session left. Answered by [`Heard::Restored`].
    pub fn restore(&self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        crate::workspace::spawn(async move {
            let recent = read().await.unwrap_or_default();
            let permitted = match recent.first() {
                Some(Root::Picked(picked)) => {
                    permission(&picked.handle, false).await.unwrap_or(false)
                }
                _ => true,
            };
            let _ = tx.send(Heard::Restored { recent, permitted });
            ctx.request_repaint();
        });
    }

    /// Take the list [`Libraries::restore`] read.
    pub fn restored(&mut self, recent: Vec<Root>) {
        self.recent = recent;
    }

    /// The library open last, if any.
    pub fn last(&self) -> Option<&Root> {
        self.recent.first()
    }

    /// Show the browser's folder picker. The folder comes back as [`Heard::Open`], under
    /// the id it already has if it was opened before.
    ///
    /// ⚠️ Only from a click the user has just made.
    pub fn pick(&self, ctx: &egui::Context) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let options = DirectoryPickerOptions::new();
        options.set_mode(FileSystemPermissionMode::Readwrite);
        let picking = window.show_directory_picker_with_options(&options);
        let known = self.recent.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        crate::workspace::spawn(async move {
            let picked = match picking {
                Ok(picking) => JsFuture::from(picking.unchecked_into::<Promise>()).await,
                Err(e) => Err(e),
            };
            let heard = match picked {
                Ok(handle) => Heard::Open(identify(handle.unchecked_into(), known).await),
                Err(e) if is(&e, "AbortError") => return,
                Err(e) => {
                    Heard::Trouble(format!("The folder picker did not open: {}.", describe(&e)))
                }
            };
            let _ = tx.send(heard);
            ctx.request_repaint();
        });
    }

    /// Make sure the browser lets drawbar into `root`, asking the user where it has not
    /// said, and that the folder is still there. Answered by [`Heard::Open`] or
    /// [`Heard::Trouble`].
    ///
    /// ⚠️ Only from a click the user has just made, since the browser may ask.
    pub fn allow(&self, ctx: &egui::Context, root: Root) {
        let Root::Picked(picked) = &root else {
            let _ = self.tx.send(Heard::Open(root));
            return;
        };
        let handle = picked.handle.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        crate::workspace::spawn(async move {
            let name = handle.name();
            let heard = match allowed(&handle).await {
                Ok(()) => Heard::Open(root),
                Err(why) => Heard::Trouble(format!("{name} did not open as the library: {why}.")),
            };
            let _ = tx.send(heard);
            ctx.request_repaint();
        });
    }

    /// The next answer to a request, if one has come.
    pub fn heard(&self) -> Option<Heard> {
        self.rx.try_recv().ok()
    }

    /// Put `root` first, and keep the list for the next session, after the folders
    /// other tabs have kept since this one read it.
    pub fn opened(&mut self, root: &Root) {
        self.recent.retain(|held| held != root);
        self.recent.insert(0, root.clone());
        self.recent.truncate(Libraries::MOST);
        let mut recent = self.recent.clone();
        let tx = self.tx.clone();
        crate::workspace::spawn(async move {
            for kept in read().await.unwrap_or_default() {
                if recent.len() < Libraries::MOST && !recent.contains(&kept) {
                    recent.push(kept);
                }
            }
            if let Err(e) = write(entries(&recent)).await {
                let _ = tx.send(Heard::Trouble(format!(
                    "The list of recent libraries was not kept: {e}."
                )));
            }
            let kept = recent.iter().filter_map(|root| match root {
                Root::Picked(picked) => Some(picked.id),
                Root::Private => None,
            });
            // What is left of a library forgotten here is only read again if it opens.
            let _ = crate::store::keep_libraries(kept.collect()).await;
        });
    }

    /// The libraries a menu offers, most recent first, with the browser's own always
    /// among them. `open` is checked.
    pub fn offered(&self, open: Option<&Root>) -> Vec<Library> {
        let own = (!self.recent.contains(&Root::Private)).then_some(Root::Private);
        self.recent
            .iter()
            .cloned()
            .chain(own)
            .map(|root| Library {
                name: root.name().unwrap_or_else(|| THIS_COMPUTER.to_string()),
                open: Some(&root) == open,
                root,
            })
            .collect()
    }
}

/// Why drawbar may not open the folder at `handle` as the library, if it may not.
async fn allowed(handle: &FileSystemDirectoryHandle) -> Result<(), String> {
    let granted = match permission(handle, false).await {
        Ok(true) => true,
        _ => permission(handle, true).await.map_err(|e| e.to_string())?,
    };
    if !granted {
        return Err("this browser was not let into it".to_string());
    }
    let first = handle.values().next().map_err(|e| describe(&e))?;
    match JsFuture::from(first).await {
        Ok(_) => Ok(()),
        Err(e) if is(&e, "NotFoundError") => Err("it is not there any more".to_string()),
        Err(e) => Err(describe(&e)),
    }
}

/// Whether the browser refused with an error of this name.
fn is(err: &JsValue, name: &str) -> bool {
    field(err, "name")
        .and_then(|held| held.as_string())
        .as_deref()
        == Some(name)
}

/// The picked folder at `handle`, under the id `known` or the list kept in IndexedDB
/// already gives it, or a new one.
async fn identify(handle: FileSystemDirectoryHandle, known: Vec<Root>) -> Root {
    let kept = read().await.unwrap_or_default();
    for root in known.into_iter().chain(kept) {
        let Root::Picked(held) = root else {
            continue;
        };
        let same = handle.is_same_entry(held.handle.unchecked_ref::<FileSystemHandle>());
        if JsFuture::from(same)
            .await
            .is_ok_and(|same| same.is_truthy())
        {
            return Root::Picked(Picked {
                id: held.id,
                handle,
            });
        }
    }
    let id = (js_sys::Math::random() * f64::from(u32::MAX)) as u32;
    Root::Picked(Picked {
        id: id.max(1),
        handle,
    })
}

/// The list as IndexedDB holds it: `{id, handle}` for a picked folder, and `{id: 0}` for
/// the browser's own library.
fn entries(recent: &[Root]) -> Array {
    recent
        .iter()
        .map(|root| {
            let entry = Object::new();
            let (id, handle) = match root {
                Root::Private => (0, None),
                Root::Picked(picked) => (picked.id, Some(&picked.handle)),
            };
            let _ = Reflect::set(&entry, &"id".into(), &id.into());
            if let Some(handle) = handle {
                let _ = Reflect::set(&entry, &"handle".into(), handle);
            }
            JsValue::from(entry)
        })
        .collect()
}

/// The list IndexedDB holds. An entry that does not read is left out.
async fn read() -> Result<Vec<Root>, String> {
    let db = database().await?;
    let held = async {
        let get = db
            .transaction_with_str(STORE)
            .and_then(|transaction| transaction.object_store(STORE))
            .and_then(|store| store.get(&KEY.into()))
            .map_err(|e| describe(&e))?;
        done(&get).await
    }
    .await;
    db.close();
    let Ok(held) = held?.dyn_into::<Array>() else {
        return Ok(Vec::new());
    };
    Ok(held
        .iter()
        .filter_map(|entry| {
            let id = field(&entry, "id")?.as_f64()? as u32;
            match field(&entry, "handle") {
                None if id == 0 => Some(Root::Private),
                None => None,
                Some(handle) => Some(Root::Picked(Picked {
                    id,
                    handle: handle.dyn_into().ok()?,
                })),
            }
        })
        .collect())
}

/// Keep `entries` as the list, once IndexedDB has committed it.
async fn write(entries: Array) -> Result<(), String> {
    let db = database().await?;
    let written = async {
        let transaction = db
            .transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)
            .map_err(|e| describe(&e))?;
        transaction
            .object_store(STORE)
            .and_then(|store| store.put_with_key(&entries, &KEY.into()))
            .map_err(|e| describe(&e))?;
        committed(&transaction).await
    }
    .await;
    db.close();
    written
}
