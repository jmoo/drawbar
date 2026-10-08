//! Where toshokan keeps its files.
//!
//! ```text
//! <folder>/<root>/writers/<w>/
//!   <segment>.txt              log segments, one JSON entry a line
//!   snapshot-<nonce>.json      snapshots
//!   pending/<nonce>.json       journal records of multi-step effects
//!   pending/<nonce>.<step>     a record's step that copies has started
//!   trash/<nonce>              displaced user bytes
//!   tmp/<nonce>.<extension>    staged files, under their destination's extension
//! <local>/<genesis>/           one writer of this install, by its genesis entry
//!   head.json                  the head this writer last wrote
//!   view.bin                   the cached view
//!   view.log                   what the cached view gained since
//!   drafts/<entity>.json       unsaved edits
//!   lock                       held while an instance writes as this writer
//!   retired                    present once the writer is never written again
//! ```
//!
//! The app names `<root>`. Only writer `w` writes under `writers/<w>/`. Names of
//! segments and snapshots are advisory: every file directly in a writer's directory
//! is read by its contents.

use crate::error::{Error, Result};
use crate::ids::{EntityId, EntryHash, Nonce, SegmentName, WriterId};
use crate::path::RelPath;

pub const WRITERS: &str = "writers";
pub const PENDING: &str = "pending";
pub const TRASH: &str = "trash";
pub const TMP: &str = "tmp";
/// Segments end in `.txt`, a type whose writes in a picked folder Chromium's Safe
/// Browsing check samples rather than always checks.
// Confirmed on hardware.
// A `.txt` close takes about 1.6 ms in Chrome; one of an unlisted type, such as
// `.jsonl`, about 45 ms.
// Reported by public documentation; not confirmed on hardware.
// Chromium's `download_file_types.asciipb` samples `.txt` and `.json` with
// probability 0.01, so about 1 close in 100 still pays the full check.
pub const SEGMENT_EXTENSION: &str = ".txt";
/// The longest name, in bytes, that common file systems take.
const MAX_NAME: usize = 255;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    root: RelPath,
}

impl Layout {
    /// The layout under `<folder>/<name>`; `name` is one path component.
    pub fn new(name: &str) -> Result<Self> {
        Ok(Self {
            root: RelPath::ROOT.join(name)?,
        })
    }

    pub fn root(&self) -> &RelPath {
        &self.root
    }

    /// Whether `path` in the folder is toshokan's: its root or anything under it.
    pub fn owns(&self, path: &RelPath) -> bool {
        path.starts_with(&self.root)
    }

    /// Refuses a path in the folder that is not a library file: toshokan's root,
    /// anything inside it, the folder itself, and a browser's swap file.
    pub fn check_library_path(&self, path: &RelPath) -> Result<()> {
        let swap = path.name().is_some_and(is_swap_file);
        match path.is_root() || self.owns(path) || swap {
            true => Err(Error::InvalidPath {
                path: path.as_str().to_owned(),
                reason: "not a library file",
            }),
            false => Ok(()),
        }
    }

    pub fn writers(&self) -> RelPath {
        child(&self.root, WRITERS)
    }

    pub fn writer(&self, writer: WriterId) -> RelPath {
        child(&self.writers(), &writer.to_string())
    }

    pub fn segment(&self, writer: WriterId, name: SegmentName) -> RelPath {
        child(&self.writer(writer), &format!("{name}{SEGMENT_EXTENSION}"))
    }

    pub fn snapshot(&self, writer: WriterId, name: Nonce) -> RelPath {
        child(&self.writer(writer), &format!("snapshot-{name}.json"))
    }

    pub fn pending_dir(&self, writer: WriterId) -> RelPath {
        child(&self.writer(writer), PENDING)
    }

    pub fn pending(&self, writer: WriterId, name: Nonce) -> RelPath {
        child(&self.pending_dir(writer), &format!("{name}.json"))
    }

    /// Present once step `step` of the record `record`, which moves by copying,
    /// has started.
    pub fn marker(&self, writer: WriterId, record: Nonce, step: usize) -> RelPath {
        child(&self.pending_dir(writer), &format!("{record}.{step}"))
    }

    pub fn trash_dir(&self, writer: WriterId) -> RelPath {
        child(&self.writer(writer), TRASH)
    }

    pub fn trash(&self, writer: WriterId, name: Nonce) -> RelPath {
        child(&self.trash_dir(writer), &name.to_string())
    }

    pub fn tmp_dir(&self, writer: WriterId) -> RelPath {
        child(&self.writer(writer), TMP)
    }

    /// Where the file `name` that becomes `destination` is staged: under the
    /// extension of `destination`'s name, so a browser judges it as the file it
    /// becomes, unless that name has none or the staged name would be longer
    /// than 255 bytes.
    pub fn staged(&self, writer: WriterId, name: Nonce, destination: &RelPath) -> RelPath {
        let staged = destination
            .name()
            .and_then(|named| named.rsplit_once('.'))
            .map(|(_, extension)| extension)
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!("{name}.{extension}"))
            .filter(|staged| staged.len() <= MAX_NAME)
            .unwrap_or_else(|| name.to_string());
        child(&self.tmp_dir(writer), &staged)
    }

    /// This install's directory for the writer whose genesis entry is `genesis`, in
    /// the local root. It is created only after that entry is durable in the
    /// folder, so the directories in the local root are the install's writer pool.
    pub fn local(genesis: EntryHash) -> RelPath {
        child(&RelPath::ROOT, &genesis.to_string())
    }

    pub fn head(genesis: EntryHash) -> RelPath {
        child(&Self::local(genesis), "head.json")
    }

    pub fn cached_view(genesis: EntryHash) -> RelPath {
        child(&Self::local(genesis), "view.bin")
    }

    /// What the cached view gained since `view.bin` was written, one record after
    /// another.
    pub fn view_journal(genesis: EntryHash) -> RelPath {
        child(&Self::local(genesis), "view.log")
    }

    pub fn drafts(genesis: EntryHash) -> RelPath {
        child(&Self::local(genesis), "drafts")
    }

    pub fn draft(genesis: EntryHash, entity: EntityId) -> RelPath {
        child(&Self::drafts(genesis), &format!("{entity}.json"))
    }

    pub fn lock(genesis: EntryHash) -> RelPath {
        child(&Self::local(genesis), "lock")
    }

    /// The entries this install let go once the folder no longer held them, by
    /// writer.
    pub fn let_go() -> RelPath {
        child(&RelPath::ROOT, "let-go.json")
    }

    /// The identities this install's scans read that no fact gives.
    pub fn identities() -> RelPath {
        child(&RelPath::ROOT, "identities.json")
    }

    /// Present once the writer can no longer be continued: it leaves the pool.
    pub fn retired(genesis: EntryHash) -> RelPath {
        child(&Self::local(genesis), "retired")
    }
}

/// Whether `name` is a swap file Chromium writes beside a picked folder's file
/// while a writable stream on it is open: bytes that never landed, which
/// toshokan neither reads nor scans.
// Confirmed on hardware.
// Chromium shows the swap file `<name>.crswap` beside a file while a writable
// stream on it is open.
// Inferred from specimens; not confirmed on hardware.
// A tab stopped while the stream is open leaves the swap file behind.
pub(crate) fn is_swap_file(name: &str) -> bool {
    name.ends_with(".crswap")
}

fn child(parent: &RelPath, name: &str) -> RelPath {
    parent
        .join(name)
        .expect("layout names are single components")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_folder_path_lives_in_its_writer_directory() {
        let layout = Layout::new(".drawbar").unwrap();
        let w = WriterId::from_u128(0xab);
        let n = Nonce::from_u128(0xcd);
        let dir = format!(".drawbar/writers/{w}");
        let expected = [
            (
                layout.segment(w, SegmentName::from_u128(0xcd)),
                format!("{dir}/{n}.txt"),
            ),
            (layout.snapshot(w, n), format!("{dir}/snapshot-{n}.json")),
            (layout.pending(w, n), format!("{dir}/pending/{n}.json")),
            (layout.trash(w, n), format!("{dir}/trash/{n}")),
        ];
        for (path, text) in expected {
            assert_eq!(path.as_str(), text);
            assert!(path.starts_with(&layout.writer(w)), "{path}");
        }
    }

    #[test]
    fn a_staged_file_takes_its_destinations_extension() {
        let layout = Layout::new(".drawbar").unwrap();
        let w = WriterId::from_u128(0xab);
        let n = Nonce::from_u128(0xcd);
        let tmp = format!(".drawbar/writers/{w}/tmp");
        let long = format!("a.{}", "x".repeat(MAX_NAME - 33));
        let longer = format!("a.{}", "x".repeat(MAX_NAME - 32));
        for (destination, staged) in [
            ("d/a.syx", format!("{n}.syx")),
            ("a.tar.gz", format!("{n}.gz")),
            (".profile", format!("{n}.profile")),
            ("a", n.to_string()),
            ("a.", n.to_string()),
            (&long, format!("{n}.{}", "x".repeat(MAX_NAME - 33))),
            (&longer, n.to_string()),
        ] {
            let path = layout.staged(w, n, &RelPath::new(destination).unwrap());
            assert_eq!(path.as_str(), format!("{tmp}/{staged}"), "{destination}");
            assert!(path.name().unwrap().len() <= MAX_NAME, "{destination}");
        }
    }

    #[test]
    fn local_paths_live_in_the_writer_directory_named_by_its_genesis() {
        let genesis = EntryHash::from_u128(1);
        let entity = EntityId::from_u128(2);
        for path in [
            Layout::head(genesis),
            Layout::cached_view(genesis),
            Layout::view_journal(genesis),
            Layout::draft(genesis, entity),
            Layout::lock(genesis),
            Layout::retired(genesis),
        ] {
            assert_eq!(path.components().next(), Some(&*genesis.to_string()));
        }
    }

    #[test]
    fn library_paths_exclude_the_root_the_folder_and_swap_files() {
        let layout = Layout::new(".drawbar").unwrap();
        for text in [".drawbar", ".drawbar/writers", "", "a.syx.crswap"] {
            let path = RelPath::new(text).unwrap();
            assert!(layout.check_library_path(&path).is_err(), "{text:?}");
        }
        for text in [".drawbarx", "a/.drawbar"] {
            let path = RelPath::new(text).unwrap();
            assert!(layout.check_library_path(&path).is_ok(), "{text:?}");
        }
        assert!(Layout::new("a/b").is_err());
    }
}
