//! The browser's files, read in slices of the `File` the page took of each.
//!
//! ⚠️ A `File` reads what its file held when it was taken, and fails once the file has
//! been written since. A file moved keeps its contents, so the library takes it again
//! where it went ([`OnDisk::resnapshot`]); a file written over is another file, which is
//! indexed again.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use eframe::egui;
use js_sys::Uint8Array;
use nord_format::crc::Crc32Stream;
use wasm_bindgen_futures::JsFuture;

use super::{Index, Missing, OnDisk, Slices, CHUNK};
use crate::work::{self, Job, Progress};

thread_local! {
    /// The `File` each file indexed in this tab is read through, by its serial.
    static SNAPSHOTS: RefCell<HashMap<u64, web_sys::File>> = RefCell::default();
    /// Repainted once a range a read on the frame asked for has landed.
    static REPAINT: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Repaint `ctx` whenever a range a read on the frame asked for lands.
pub fn repaint_with(ctx: &egui::Context) {
    REPAINT.with(|held| *held.borrow_mut() = Some(ctx.clone()));
}

/// The most one read on the frame fetches. A stroke runs to a few megabytes; more is
/// read off the frame.
const MOST_ON_FRAME: u64 = 64 << 20;

/// How many ranges fetched for the frame wait to be asked for again. An older one goes.
const KEPT: usize = 8;

/// What the frame's reads have fetched and are fetching.
#[derive(Debug, Default)]
pub(super) struct Source {
    fetched: Arc<Mutex<Fetched>>,
}

#[derive(Debug, Default)]
struct Fetched {
    ready: VecDeque<(Range<u64>, io::Result<Vec<u8>>)>,
    flying: Vec<Range<u64>>,
}

impl OnDisk {
    /// Index the file `snapshot` was taken of, when it is a piano or sample instrument,
    /// fetching only the slices its index reads: of any other file, its first bytes.
    /// `None` as [`Index::read`] answers it.
    pub async fn open(snapshot: web_sys::File, crc: Option<u32>) -> io::Result<Option<OnDisk>> {
        let len = snapshot.size() as u64;
        let head = slice(&snapshot, 0..len.min(Index::HEAD as u64)).await?;
        if Index::tagged(&head).is_none() {
            return Ok(None);
        }
        let mut slices = Slices::new(len);
        slices.hold(0..head.len() as u64, head)?;
        let index = loop {
            let missing = match Index::read(&mut slices.reader(), len) {
                Ok(index) => break index,
                Err(e) => Missing::of(&e).ok_or(e)?,
            };
            let range = slices.widen(&missing);
            let bytes = slice(&snapshot, range.clone()).await?;
            slices.hold(range, bytes)?;
        };
        let Some(index) = index else {
            return Ok(None);
        };
        let disk = OnDisk::indexed(Source::default(), len, crc, index);
        SNAPSHOTS.with(|held| held.borrow_mut().insert(disk.serial, snapshot));
        Ok(Some(disk))
    }

    /// The `File` this is read through, which a download hands over unread.
    pub fn snapshot(&self) -> Option<web_sys::File> {
        SNAPSHOTS.with(|held| held.borrow().get(&self.serial).cloned())
    }

    /// Read through `snapshot` from now on: a `File` taken again of the same contents,
    /// after the file moved.
    pub fn resnapshot(&self, snapshot: web_sys::File) {
        if snapshot.size() as u64 != self.len {
            return;
        }
        SNAPSHOTS.with(|held| held.borrow_mut().insert(self.serial, snapshot));
    }
}

impl Drop for OnDisk {
    fn drop(&mut self) {
        SNAPSHOTS.with(|held| held.borrow_mut().remove(&self.serial));
    }
}

impl Source {
    /// A range fetched already, handed over once; otherwise
    /// [`io::ErrorKind::WouldBlock`], and the range is fetched.
    pub(super) fn read(&self, serial: u64, range: Range<u64>) -> io::Result<Vec<u8>> {
        let mut fetched = self.fetched.lock().expect("unpoisoned");
        if let Some(at) = fetched.ready.iter().position(|(held, _)| *held == range) {
            return fetched.ready.remove(at).expect("found above").1;
        }
        if range.end - range.start > MOST_ON_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "a range that large is read off the frame",
            ));
        }
        if !fetched.flying.contains(&range) {
            fetched.flying.push(range.clone());
            wasm_bindgen_futures::spawn_local(fetch(serial, range.clone(), self.fetched.clone()));
        }
        Err(Missing::error(range))
    }

    pub(super) fn whole(&self, _len: u64) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "it rests in its file, and is read whole only off the frame",
        ))
    }

    pub(super) async fn crc_now(&self, serial: u64, len: u64) -> io::Result<u32> {
        let mut crc = Crc32Stream::new();
        stream(serial, len, |chunk| {
            crc.update(chunk);
            Ok(())
        })
        .await
        .map_err(io::Error::other)?;
        Ok(crc.value())
    }

    pub(super) fn pass<S: Send + 'static, T: Send + 'static>(
        file: &Arc<OnDisk>,
        ctx: &egui::Context,
        mut state: S,
        mut each: impl FnMut(&mut S, &[u8]) -> Result<(), String> + Send + 'static,
        then: impl FnOnce(&Progress, Result<S, String>) -> Result<T, String> + Send + 'static,
    ) -> Job<Result<T, String>> {
        // Held until the pass ends, so the snapshot stays.
        let file = file.clone();
        work::spawn(ctx, move |progress| async move {
            progress.say("reading the file");
            let read = stream(file.serial, file.len, |chunk| each(&mut state, chunk)).await;
            then(&progress, read.map(|()| state))
        })
    }
}

/// Fetch `range` for a read on the frame, and repaint once it has landed.
async fn fetch(serial: u64, range: Range<u64>, fetched: Arc<Mutex<Fetched>>) {
    let got = match SNAPSHOTS.with(|held| held.borrow().get(&serial).cloned()) {
        Some(snapshot) => slice(&snapshot, range.clone()).await,
        None => Err(gone()),
    };
    {
        let mut held = fetched.lock().expect("unpoisoned");
        held.flying.retain(|flying| *flying != range);
        held.ready.push_back((range, got));
        while held.ready.len() > KEPT {
            held.ready.pop_front();
        }
    }
    REPAINT.with(|held| {
        if let Some(ctx) = &*held.borrow() {
            ctx.request_repaint();
        }
    });
}

/// Hand every byte of the file to `each`, in order, a slice at a time.
async fn stream(
    serial: u64,
    len: u64,
    mut each: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let snapshot = SNAPSHOTS
        .with(|held| held.borrow().get(&serial).cloned())
        .ok_or_else(|| gone().to_string())?;
    let mut at = 0;
    while at < len {
        let end = len.min(at + CHUNK as u64);
        let bytes = slice(&snapshot, at..end).await.map_err(|e| e.to_string())?;
        each(&bytes)?;
        at = end;
    }
    Ok(())
}

fn gone() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "drawbar no longer reads that file")
}

/// The bytes at `range` of `file`.
pub(crate) async fn slice(file: &web_sys::File, range: Range<u64>) -> io::Result<Vec<u8>> {
    let failed = |e| io::Error::other(crate::js::describe(&e));
    let blob = file
        .slice_with_f64_and_f64(range.start as f64, range.end as f64)
        .map_err(failed)?;
    let buffer = JsFuture::from(blob.array_buffer()).await.map_err(failed)?;
    let bytes = Uint8Array::new(&buffer);
    if u64::from(bytes.length()) != range.end - range.start {
        return Err(io::Error::other("the file changed while it was read"));
    }
    Ok(bytes.to_vec())
}
