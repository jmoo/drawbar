//! Storage that counts the files its listings of the folder find, so a scan can say how
//! far it has got.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use toshokan::asynch::Fs;
use toshokan::io::{Capabilities, Io, IoResult, Kind, Reply, Root};
use toshokan::RelPath;

/// `F`, adding to `listed` the files each directory of the folder holds the first time
/// it is listed. Names that start with a dot, toshokan's root among them, are not the
/// library's, and neither is anything under them.
pub struct Counted<F> {
    inner: F,
    listed: Arc<AtomicUsize>,
    counted: RefCell<BTreeSet<RelPath>>,
}

impl<F> Counted<F> {
    pub fn new(inner: F, listed: Arc<AtomicUsize>) -> Counted<F> {
        Counted {
            inner,
            listed,
            counted: RefCell::default(),
        }
    }
}

impl<F: Fs> Fs for Counted<F> {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.inner.capabilities(root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        let first = match &io {
            Io::ListStat {
                root: Root::Folder,
                dir,
            } if !hidden(dir) => self.counted.borrow_mut().insert(dir.clone()),
            _ => false,
        };
        let reply = self.inner.perform(io).await;
        let found = match &reply {
            Ok(Reply::ListedStat(entries)) if first => entries
                .iter()
                .filter(|(name, meta)| meta.kind == Kind::File && !name.starts_with('.'))
                .count(),
            _ => 0,
        };
        self.listed.fetch_add(found, Ordering::Relaxed);
        reply
    }

    async fn pause(&self) {
        self.inner.pause().await;
    }

    fn own(&self, root: &RelPath) {
        self.inner.own(root);
    }
}

fn hidden(dir: &RelPath) -> bool {
    dir.components().any(|name| name.starts_with('.'))
}
