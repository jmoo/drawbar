//! The file system toshokan runs on, as a trait each backend implements.
//!
//! Every path is a [`RelPath`] under the library folder. A backend declares what it
//! can do in [`Capabilities`]; callers choose their plan from that declaration, and a
//! call to an undeclared operation fails with [`Error::Unsupported`].

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};
use crate::value::BlobId;

mod mem;

pub use mem::MemFs;

/// A path relative to the library folder: `/`-separated names, none of them empty,
/// `.` or `..`. The empty path is the library folder itself.
///
/// Ordered by its text, so sorted paths are deterministic on every machine.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    pub const ROOT: Self = Self(String::new());

    pub fn new(text: &str) -> Result<Self> {
        if !text.is_empty() {
            text.split('/')
                .try_for_each(|name| check_name(text, name))?;
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path of `name` inside this directory. `name` must be one path component.
    pub fn join(&self, name: &str) -> Result<Self> {
        check_name(name, name)?;
        Ok(match self.is_root() {
            true => Self(name.to_owned()),
            false => Self(format!("{}/{name}", self.0)),
        })
    }

    /// The directory holding this path; `None` for the root.
    pub fn parent(&self) -> Option<Self> {
        match self.0.rsplit_once('/') {
            Some((parent, _)) => Some(Self(parent.to_owned())),
            None if self.is_root() => None,
            None => Some(Self::ROOT),
        }
    }

    /// The last component; `None` for the root.
    pub fn name(&self) -> Option<&str> {
        match self.is_root() {
            true => None,
            false => self.0.rsplit('/').next(),
        }
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|name| !name.is_empty())
    }

    /// Whether `prefix` is this path or one of its ancestors, compared by component.
    pub fn starts_with(&self, prefix: &RelPath) -> bool {
        prefix.is_root()
            || self.0 == prefix.0
            || (self.0.starts_with(&prefix.0) && self.0.as_bytes()[prefix.0.len()] == b'/')
    }

    /// This path moved from under `from` to under `to`; `None` when it is not under
    /// `from`.
    pub fn rebase(&self, from: &RelPath, to: &RelPath) -> Option<Self> {
        if !self.starts_with(from) {
            return None;
        }
        let rest = self.0[from.0.len()..].trim_start_matches('/');
        Some(match (to.is_root(), rest.is_empty()) {
            (_, true) => to.clone(),
            (true, false) => Self(rest.to_owned()),
            (false, false) => Self(format!("{}/{rest}", to.0)),
        })
    }
}

fn check_name(path: &str, name: &str) -> Result<()> {
    let reason = match name {
        "" => "a component is empty",
        "." | ".." => "a component is `.` or `..`",
        _ if name.contains('/') => "a name contains `/`",
        _ if name.contains('\0') => "a component contains NUL",
        _ => return Ok(()),
    };
    Err(Error::InvalidPath {
        path: path.to_owned(),
        reason,
    })
}

/// The root prints as `.`; every other path prints as its text.
impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_root() { "." } else { &self.0 })
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl Serialize for RelPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::new(&text).map_err(serde::de::Error::custom)
    }
}

/// One operation a backend may or may not provide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capability {
    Append,
    RenameFile,
    RenameDir,
    HardLink,
    ExclusiveCreate,
    Fsync,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Append => "append to a file",
            Self::RenameFile => "rename a file",
            Self::RenameDir => "rename a directory",
            Self::HardLink => "make a hard link",
            Self::ExclusiveCreate => "create a file exclusively",
            Self::Fsync => "sync to storage",
        })
    }
}

/// What a backend can do. Plans follow from this, never from errors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Capabilities {
    /// [`Fs::append`] works.
    pub append: bool,
    /// [`Fs::rename`] works on files, atomically.
    pub rename_file: bool,
    /// [`Fs::rename`] works on directories, atomically, with everything inside.
    pub rename_dir: bool,
    /// [`Fs::hard_link`] works.
    pub hard_link: bool,
    /// [`Fs::create`] checks for an existing file atomically, even against other
    /// processes. Without it the check and the write are separate steps.
    pub exclusive_create: bool,
    /// [`Fs::sync`] makes completed operations durable. Without it the backend gives
    /// no durability control, `sync` does nothing, and an operation's durability is the
    /// backend's own.
    pub fsync: bool,
}

impl Capabilities {
    pub const ALL: Self = Self {
        append: true,
        rename_file: true,
        rename_dir: true,
        hard_link: true,
        exclusive_create: true,
        fsync: true,
    };

    pub const NONE: Self = Self {
        append: false,
        rename_file: false,
        rename_dir: false,
        hard_link: false,
        exclusive_create: false,
        fsync: false,
    };

    pub fn has(&self, capability: Capability) -> bool {
        match capability {
            Capability::Append => self.append,
            Capability::RenameFile => self.rename_file,
            Capability::RenameDir => self.rename_dir,
            Capability::HardLink => self.hard_link,
            Capability::ExclusiveCreate => self.exclusive_create,
            Capability::Fsync => self.fsync,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Metadata {
    pub kind: FileKind,
    /// Bytes in a file; 0 for a directory.
    pub len: u64,
    /// Nanoseconds since the Unix epoch for a file whose backend reports a time.
    /// Only equality is meaningful: backends may use a logical clock.
    pub modified: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DirEntry {
    pub name: String,
    pub kind: FileKind,
}

/// A file system rooted at the library folder.
///
/// Each mutating operation either completes or leaves nothing behind, as far as the
/// caller of this trait can observe. Whether a completed operation survives a crash
/// is the business of [`Fs::sync`].
///
/// Futures carry no `Send` bound, so a single-threaded browser backend can implement
/// the trait; that is also why `async_fn_in_trait` is allowed.
#[allow(async_fn_in_trait)]
pub trait Fs {
    fn capabilities(&self) -> Capabilities;

    /// `None` when nothing is at `path`.
    async fn metadata(&self, path: &RelPath) -> Result<Option<Metadata>>;

    /// The entries of a directory, sorted by name.
    async fn list(&self, dir: &RelPath) -> Result<Vec<DirEntry>>;

    async fn read(&self, path: &RelPath) -> Result<Vec<u8>>;

    /// Up to `len` bytes from `offset`; fewer only at the end of the file.
    async fn read_at(&self, path: &RelPath, offset: u64, len: usize) -> Result<Vec<u8>>;

    /// Create `dir` and any missing ancestors. An existing directory is success; a
    /// file in the way is [`Error::NotDirectory`].
    async fn create_dir_all(&self, dir: &RelPath) -> Result<()>;

    /// Create a new file holding `bytes` in an existing directory. Refuses with
    /// [`Error::AlreadyExists`] rather than replace anything; see
    /// [`Capabilities::exclusive_create`] for how far that check reaches.
    async fn create(&self, path: &RelPath, bytes: &[u8]) -> Result<()>;

    /// Add `bytes` to the end of an existing file. Needs [`Capabilities::append`].
    async fn append(&self, path: &RelPath, bytes: &[u8]) -> Result<()>;

    /// Move a file or directory to a path where nothing is. Refuses with
    /// [`Error::AlreadyExists`] rather than replace anything, and refuses to move a
    /// directory inside itself. Needs [`Capabilities::rename_file`] or
    /// [`Capabilities::rename_dir`] for the kind being moved.
    async fn rename(&self, from: &RelPath, to: &RelPath) -> Result<()>;

    /// Give the file at `from` a second name `to`. Needs [`Capabilities::hard_link`].
    async fn hard_link(&self, from: &RelPath, to: &RelPath) -> Result<()>;

    async fn remove_file(&self, path: &RelPath) -> Result<()>;

    /// Remove an empty directory.
    async fn remove_dir(&self, path: &RelPath) -> Result<()>;

    /// Make durable a file's contents, or a directory's list of names. A new or
    /// renamed entry is durable only once its directory is synced, and a rename
    /// across directories needs both synced. Does nothing without
    /// [`Capabilities::fsync`].
    async fn sync(&self, path: &RelPath) -> Result<()>;
}

/// What is known of a file's contents without necessarily reading them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Fingerprint {
    pub len: u64,
    pub modified: Option<u64>,
    pub hash: Option<BlobId>,
}

/// Whether two fingerprints describe the same contents.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sameness {
    Same,
    Different,
    /// Only hashing both can tell.
    Unknown,
}

impl Fingerprint {
    pub fn of(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len,
            modified: metadata.modified,
            hash: None,
        }
    }

    /// Lengths decide when they differ and hashes when both are known. Otherwise an
    /// equal modification time is taken as the same contents.
    ///
    /// ⚠️ A file rewritten to the same length within the backend's timestamp
    /// resolution compares as `Same` unless both sides carry a hash.
    pub fn compare(&self, other: &Fingerprint) -> Sameness {
        if self.len != other.len {
            return Sameness::Different;
        }
        match (self.hash, other.hash, self.modified, other.modified) {
            (Some(a), Some(b), _, _) if a == b => Sameness::Same,
            (Some(_), Some(_), _, _) => Sameness::Different,
            (_, _, Some(a), Some(b)) if a == b => Sameness::Same,
            _ => Sameness::Unknown,
        }
    }
}

/// The fingerprint of the file at `path`, hashed when `hash` is set; `None` when no
/// file is there.
pub async fn fingerprint<F: Fs + ?Sized>(
    fs: &F,
    path: &RelPath,
    hash: bool,
) -> Result<Option<Fingerprint>> {
    let Some(metadata) = fs.metadata(path).await? else {
        return Ok(None);
    };
    if metadata.kind == FileKind::Directory {
        return Err(Error::IsDirectory { path: path.clone() });
    }
    let mut print = Fingerprint::of(&metadata);
    if hash {
        let (blob, len) = hash_file(fs, path).await?;
        print.hash = Some(blob);
        print.len = len;
    }
    Ok(Some(print))
}

const HASH_CHUNK: usize = 1 << 20;

/// The blob id and length of a file, read a chunk at a time.
pub async fn hash_file<F: Fs + ?Sized>(fs: &F, path: &RelPath) -> Result<(BlobId, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut len = 0u64;
    loop {
        let chunk = fs.read_at(path, len, HASH_CHUNK).await?;
        hasher.update(&chunk);
        len += chunk.len() as u64;
        if chunk.len() < HASH_CHUNK {
            return Ok((hasher.finalize().into(), len));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    #[test]
    fn a_path_refuses_empty_dot_and_nul_components() {
        for text in ["/a", "a/", "a//b", ".", "a/./b", "..", "a/../b", "a\0b"] {
            assert!(RelPath::new(text).is_err(), "{text:?} accepted");
        }
        assert!(RelPath::ROOT.join("a/b").is_err());
        assert!(RelPath::ROOT.join("").is_err());
    }

    #[test]
    fn a_path_knows_its_parent_and_name() {
        let file = path("Organ/B3/Gospel.npno");
        assert_eq!(file.parent(), Some(path("Organ/B3")));
        assert_eq!(file.name(), Some("Gospel.npno"));
        assert_eq!(path("top").parent(), Some(RelPath::ROOT));
        assert_eq!(RelPath::ROOT.parent(), None);
        assert_eq!(RelPath::ROOT.name(), None);
        assert_eq!(
            RelPath::ROOT.join("a").unwrap().join("b").unwrap(),
            path("a/b")
        );
        assert_eq!(
            file.components().collect::<Vec<_>>(),
            ["Organ", "B3", "Gospel.npno"]
        );
        assert_eq!(RelPath::ROOT.components().count(), 0);
    }

    #[test]
    fn a_path_starts_with_whole_components_only() {
        let file = path("ab/c");
        assert!(file.starts_with(&path("ab")));
        assert!(file.starts_with(&file));
        assert!(file.starts_with(&RelPath::ROOT));
        assert!(!file.starts_with(&path("a")));
        assert!(!path("ab").starts_with(&file));
    }

    #[test]
    fn rebasing_moves_a_path_between_trees() {
        let to = path("x/y");
        assert_eq!(path("a/b/c").rebase(&path("a"), &to), Some(path("x/y/b/c")));
        assert_eq!(path("a").rebase(&path("a"), &to), Some(to.clone()));
        assert_eq!(
            path("a/b").rebase(&path("a"), &RelPath::ROOT),
            Some(path("b"))
        );
        assert_eq!(path("b").rebase(&RelPath::ROOT, &to), Some(path("x/y/b")));
        assert_eq!(path("ab").rebase(&path("a"), &to), None);
    }

    #[test]
    fn the_root_prints_as_a_dot_and_serializes_empty() {
        assert_eq!(RelPath::ROOT.to_string(), ".");
        assert_eq!(serde_json::to_string(&RelPath::ROOT).unwrap(), "\"\"");
        assert_eq!(
            serde_json::from_str::<RelPath>("\"a/b\"").unwrap(),
            path("a/b")
        );
        assert!(serde_json::from_str::<RelPath>("\"a/../b\"").is_err());
    }

    #[test]
    fn fingerprints_compare_by_length_then_hash_then_time() {
        let base = Fingerprint {
            len: 3,
            modified: Some(1),
            hash: None,
        };
        let hashed = |byte: &[u8]| Some(BlobId::of(byte));
        let cases = [
            (Fingerprint { len: 4, ..base }, Sameness::Different),
            (base, Sameness::Same),
            (
                Fingerprint {
                    modified: Some(2),
                    ..base
                },
                Sameness::Unknown,
            ),
            (
                Fingerprint {
                    modified: None,
                    ..base
                },
                Sameness::Unknown,
            ),
        ];
        for (other, expected) in cases {
            assert_eq!(base.compare(&other), expected, "{other:?}");
        }
        let a = Fingerprint {
            hash: hashed(b"a"),
            ..base
        };
        assert_eq!(
            a.compare(&Fingerprint {
                hash: hashed(b"b"),
                ..base
            }),
            Sameness::Different
        );
        let later = Fingerprint {
            modified: Some(2),
            ..a
        };
        assert_eq!(a.compare(&later), Sameness::Same);
    }

    #[test]
    fn hashing_a_file_reads_across_chunk_boundaries() {
        let fs = MemFs::new();
        let bytes: Vec<u8> = (0..HASH_CHUNK * 2 + 7).map(|i| i as u8).collect();
        for len in [0, HASH_CHUNK, bytes.len()] {
            let file = path(&format!("f{len}"));
            pollster::block_on(fs.create(&file, &bytes[..len])).unwrap();
            let hashed = pollster::block_on(hash_file(&fs, &file)).unwrap();
            assert_eq!(
                hashed,
                (BlobId::of(&bytes[..len]), len as u64),
                "length {len}"
            );
        }
    }
}
