//! Storage that counts the files its listings of the folder find, so a scan can say how
//! far it has got, and the requests it has answered, so a long command shows it lives.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use toshokan::asynch::Fs;
use toshokan::io::{Capabilities, Io, IoResult, Kind, Reply, Root};
use toshokan::RelPath;

/// What a library's storage has done so far.
#[derive(Default)]
pub struct Signs {
    /// The files each directory of the folder holds, counted the first time it is
    /// listed. Names that start with a dot, toshokan's root among them, are not the
    /// library's, and neither is anything under them.
    pub listed: AtomicUsize,
    /// The requests answered.
    pub answered: AtomicUsize,
}

/// `F`, counting into `signs` what it does.
pub struct Counted<F> {
    inner: F,
    signs: Arc<Signs>,
    counted: RefCell<BTreeSet<RelPath>>,
}

impl<F> Counted<F> {
    pub fn new(inner: F, signs: Arc<Signs>) -> Counted<F> {
        Counted {
            inner,
            signs,
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
        self.signs.listed.fetch_add(found, Ordering::Relaxed);
        self.signs.answered.fetch_add(1, Ordering::Relaxed);
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
