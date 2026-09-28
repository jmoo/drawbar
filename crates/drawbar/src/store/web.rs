//! The browser's stand-in library: the tree held in memory and written whole to
//! `localStorage` after every change, under the same budget the string store had.
//!
//! ⚠️ A stand-in. It answers every [`Cmd`] inline, so a browser build has the same model
//! of a library as the desktop, but `localStorage` holds a few megabytes, so a sample or
//! a piano still cannot be kept. The origin private file system replaces it.

use std::collections::{BTreeMap, VecDeque};
use std::io;

use base64::prelude::{Engine as _, BASE64_STANDARD};
use eframe::egui;
use serde::{Deserialize, Serialize};

use super::exec::{self, Entry, Fs, Kind};
use super::{names, Cmd, Event, Stat};

/// Where the tree is kept.
const KEY: &str = "drawbar.library";

/// The most the kept tree may take, as text.
///
/// ⚠️ A browser gives an origin about 5 MiB and refuses a write past it without telling
/// the page, so the budget is enforced here, below the quota.
const BUDGET: usize = 3 * 1024 * 1024;

/// The browser's library has no path a user could open, so there is nothing to find.
pub fn default_root() -> Option<()> {
    Some(())
}

/// Commands answered as they are sent.
pub struct Backend {
    fs: Mem,
    events: VecDeque<Event>,
    ctx: egui::Context,
}

impl Backend {
    pub fn start(ctx: &egui::Context, _root: ()) -> Backend {
        Backend {
            fs: Mem::load(),
            events: VecDeque::new(),
            ctx: ctx.clone(),
        }
    }

    pub fn label(&self) -> String {
        "this browser".to_string()
    }

    pub fn reveal(&self) -> Option<String> {
        None
    }

    pub fn send(&mut self, cmd: Cmd) {
        if let Some(event) = exec::execute(&mut self.fs, cmd) {
            self.events.push_back(event);
            self.ctx.request_repaint();
        }
    }

    pub fn try_recv(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    pub fn finish(&mut self) {}
}

#[derive(Clone, Serialize, Deserialize)]
enum Node {
    Dir,
    /// Base64 of the contents, their length, and the change counter when they were
    /// written.
    File {
        bytes: String,
        len: u64,
        modified: u64,
    },
}

/// The tree, by path.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Mem {
    entries: BTreeMap<String, Node>,
    /// Counts writes, standing in for a modification time.
    clock: u64,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn missing() -> io::Error {
    io::ErrorKind::NotFound.into()
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

impl Mem {
    fn load() -> Mem {
        storage()
            .and_then(|store| store.get_item(KEY).ok()?)
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Apply `change` to a copy, keep the copy if it fits the budget, and leave the tree
    /// as it was otherwise.
    fn change(&mut self, change: impl FnOnce(&mut Mem) -> io::Result<()>) -> io::Result<()> {
        let mut next = self.clone();
        next.clock += 1;
        change(&mut next)?;
        let text = ron::to_string(&next).map_err(io::Error::other)?;
        if text.len() > BUDGET {
            return Err(io::Error::other(
                "this browser's storage for drawbar is full",
            ));
        }
        let store = storage().ok_or_else(|| io::Error::other("this browser keeps nothing"))?;
        store
            .set_item(KEY, &text)
            .map_err(|_| io::Error::other("this browser refused to keep the library"))?;
        *self = next;
        Ok(())
    }

    fn put(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        if !matches!(self.entries.get(parent(path)), Some(Node::Dir)) && !parent(path).is_empty() {
            return Err(missing());
        }
        let modified = self.clock;
        self.entries.insert(
            path.to_string(),
            Node::File {
                bytes: BASE64_STANDARD.encode(bytes),
                len: bytes.len() as u64,
                modified,
            },
        );
        Ok(())
    }
}

impl Fs for Mem {
    fn prepare(&mut self) -> io::Result<()> {
        if [".drawbar", exec::TMP, exec::WORKING]
            .iter()
            .all(|dir| self.entries.contains_key(*dir))
        {
            return Ok(());
        }
        self.change(|mem| {
            for dir in [".drawbar", exec::TMP, exec::WORKING] {
                mem.entries.insert(dir.to_string(), Node::Dir);
            }
            Ok(())
        })
    }

    fn lock(&mut self) -> io::Result<bool> {
        Ok(true)
    }

    fn list(&self) -> io::Result<Vec<Entry>> {
        Ok(self
            .entries
            .iter()
            .map(|(path, node)| Entry {
                path: path.clone(),
                kind: match node {
                    Node::Dir => Kind::Dir,
                    Node::File { len, modified, .. } => Kind::File(Stat {
                        len: *len,
                        modified: Some(*modified),
                    }),
                },
            })
            .collect())
    }

    fn names(&self, dir: &str) -> io::Result<Vec<String>> {
        Ok(self
            .entries
            .keys()
            .filter(|path| parent(path) == dir)
            .map(|path| path[dir.len() + 1..].to_string())
            .collect())
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        match self.entries.get(path) {
            Some(Node::File { bytes, .. }) => {
                BASE64_STANDARD.decode(bytes).map_err(io::Error::other)
            }
            _ => Err(missing()),
        }
    }

    fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
        Ok(match self.entries.get(path) {
            Some(Node::File { len, modified, .. }) => Some(Stat {
                len: *len,
                modified: Some(*modified),
            }),
            Some(Node::Dir) => Some(Stat {
                len: 0,
                modified: None,
            }),
            None => None,
        })
    }

    fn create(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        if self.entries.contains_key(path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        self.change(|mem| mem.put(path, bytes))
    }

    fn replace(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        self.change(|mem| mem.put(path, bytes))
    }

    fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        if self.entries.contains_key(to) && names::key(from) != names::key(to) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        if !self.entries.contains_key(from) {
            return Err(missing());
        }
        self.change(|mem| {
            let moving: Vec<String> = mem
                .entries
                .keys()
                .filter(|path| {
                    path.as_str() == from
                        || path
                            .strip_prefix(from)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
                .cloned()
                .collect();
            for path in moving {
                if let Some(node) = mem.entries.remove(&path) {
                    mem.entries
                        .insert(format!("{to}{}", &path[from.len()..]), node);
                }
            }
            Ok(())
        })
    }

    fn make_dir(&mut self, path: &str) -> io::Result<()> {
        self.change(|mem| {
            let mut at = String::new();
            for part in path.split('/') {
                if !at.is_empty() {
                    at.push('/');
                }
                at.push_str(part);
                mem.entries.entry(at.clone()).or_insert(Node::Dir);
            }
            Ok(())
        })
    }

    fn remove_file(&mut self, path: &str) -> io::Result<()> {
        if !matches!(self.entries.get(path), Some(Node::File { .. })) {
            return Err(missing());
        }
        self.change(|mem| {
            mem.entries.remove(path);
            Ok(())
        })
    }

    fn remove_dir(&mut self, path: &str) -> io::Result<()> {
        if self.entries.keys().any(|held| parent(held) == path) {
            return Err(io::Error::other("the folder is not empty"));
        }
        self.change(|mem| {
            mem.entries.remove(path);
            Ok(())
        })
    }
}
