//! One behavior suite for every `Fs` backend: each behavior runs on a fresh, empty
//! library of each backend.

use pollster::block_on;
use toshokan::fs::{Capabilities, Capability, FileKind};
use toshokan::{Error, Fs, MemFs, RelPath};

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

macro_rules! assert_fails {
    ($result:expr, $pattern:pat) => {
        let result = $result;
        assert!(matches!(result, Err($pattern)), "{result:?}");
    };
}

async fn contents(fs: &impl Fs, text: &str) -> Vec<u8> {
    fs.read(&path(text)).await.unwrap()
}

/// The names in `dir`, with their kinds.
async fn names(fs: &impl Fs, dir: &str) -> Vec<(String, FileKind)> {
    let listing = fs.list(&path(dir)).await.unwrap();
    listing.into_iter().map(|e| (e.name, e.kind)).collect()
}

fn named(entries: &[(&str, FileKind)]) -> Vec<(String, FileKind)> {
    entries
        .iter()
        .map(|&(name, kind)| (name.to_owned(), kind))
        .collect()
}

/// Whether `result` is what the backend's declaration of `capability` promises:
/// success when declared, [`Error::Unsupported`] when not.
fn declared<T: std::fmt::Debug>(fs: &impl Fs, capability: Capability, result: &Result<T, Error>) {
    match fs.capabilities().has(capability) {
        true => assert!(result.is_ok(), "{capability:?}: {result:?}"),
        false => assert!(
            matches!(result, Err(Error::Unsupported(c)) if *c == capability),
            "{capability:?}: {result:?}"
        ),
    }
}

mod suite {
    use super::*;

    pub async fn a_created_file_reads_back_with_its_length(fs: &impl Fs) {
        fs.create_dir_all(&path("a/b")).await.unwrap();
        fs.create(&path("a/b/f"), b"hello").await.unwrap();
        assert_eq!(contents(fs, "a/b/f").await, b"hello");
        assert_eq!(fs.read_at(&path("a/b/f"), 0, 2).await.unwrap(), b"he");
        assert_eq!(fs.read_at(&path("a/b/f"), 3, 10).await.unwrap(), b"lo");
        assert_eq!(fs.read_at(&path("a/b/f"), 9, 10).await.unwrap(), b"");
        let metadata = fs.metadata(&path("a/b/f")).await.unwrap().unwrap();
        assert_eq!((metadata.kind, metadata.len), (FileKind::File, 5));
        assert!(metadata.modified.is_some(), "{metadata:?}");
        let again = fs.metadata(&path("a/b/f")).await.unwrap().unwrap();
        assert_eq!(
            again, metadata,
            "an unchanged file reports the same metadata"
        );
        let dir = fs.metadata(&path("a")).await.unwrap().unwrap();
        assert_eq!((dir.kind, dir.len), (FileKind::Directory, 0));
        assert_eq!(fs.metadata(&path("a/x")).await.unwrap(), None);
    }

    pub async fn reading_needs_an_existing_file(fs: &impl Fs) {
        fs.create_dir_all(&path("d")).await.unwrap();
        assert_fails!(fs.read(&path("d")).await, Error::IsDirectory { .. });
        assert_fails!(
            fs.read_at(&path("d"), 0, 1).await,
            Error::IsDirectory { .. }
        );
        assert_fails!(fs.read(&path("gone")).await, Error::NotFound { .. });
        assert_fails!(
            fs.read_at(&path("gone"), 0, 1).await,
            Error::NotFound { .. }
        );
    }

    pub async fn a_listing_is_sorted_by_name_and_reports_kinds(fs: &impl Fs) {
        for name in ["b", "a", "C"] {
            fs.create(&path(name), b"").await.unwrap();
        }
        fs.create_dir_all(&path("d")).await.unwrap();
        assert_eq!(
            names(fs, "").await,
            named(&[
                ("C", FileKind::File),
                ("a", FileKind::File),
                ("b", FileKind::File),
                ("d", FileKind::Directory),
            ])
        );
        assert_eq!(names(fs, "d").await, []);
        assert_fails!(fs.list(&path("a")).await, Error::NotDirectory { .. });
        assert_fails!(fs.list(&path("gone")).await, Error::NotFound { .. });
    }

    pub async fn creating_directories_makes_ancestors_and_tolerates_existing_ones(fs: &impl Fs) {
        fs.create_dir_all(&path("a/b/c")).await.unwrap();
        fs.create_dir_all(&path("a/b")).await.unwrap();
        fs.create_dir_all(&RelPath::ROOT).await.unwrap();
        fs.create(&path("a/f"), b"").await.unwrap();
        assert_fails!(
            fs.create_dir_all(&path("a/f/g")).await,
            Error::NotDirectory { .. }
        );
        assert_eq!(
            names(fs, "a").await,
            named(&[("b", FileKind::Directory), ("f", FileKind::File)])
        );
        assert_eq!(names(fs, "a/b").await, named(&[("c", FileKind::Directory)]));
    }

    pub async fn create_never_replaces_and_needs_an_existing_directory(fs: &impl Fs) {
        fs.create(&path("f"), b"old").await.unwrap();
        fs.create_dir_all(&path("d")).await.unwrap();
        assert_fails!(
            fs.create(&path("f"), b"new").await,
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            fs.create(&path("d"), b"new").await,
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            fs.create(&path("gone/f"), b"").await,
            Error::NotFound { .. }
        );
        assert_fails!(
            fs.create(&path("f/g"), b"").await,
            Error::NotDirectory { .. }
        );
        assert_fails!(
            fs.create(&RelPath::ROOT, b"").await,
            Error::InvalidPath { .. }
        );
        assert_eq!(contents(fs, "f").await, b"old");
        assert_eq!(
            names(fs, "").await,
            named(&[("d", FileKind::Directory), ("f", FileKind::File)])
        );
    }

    pub async fn append_extends_an_existing_file(fs: &impl Fs) {
        fs.create(&path("f"), b"ab").await.unwrap();
        let appended = fs.append(&path("f"), b"cd").await;
        declared(fs, Capability::Append, &appended);
        if appended.is_ok() {
            assert_eq!(contents(fs, "f").await, b"abcd");
            assert_fails!(fs.append(&path("gone"), b"x").await, Error::NotFound { .. });
            assert_eq!(fs.metadata(&path("gone")).await.unwrap(), None);
        }
    }

    pub async fn rename_moves_a_file_and_never_replaces(fs: &impl Fs) {
        fs.create_dir_all(&path("d")).await.unwrap();
        fs.create(&path("f"), b"1").await.unwrap();
        fs.create(&path("taken"), b"2").await.unwrap();
        let renamed = fs.rename(&path("f"), &path("d/g")).await;
        declared(fs, Capability::RenameFile, &renamed);
        if renamed.is_err() {
            return;
        }
        assert_eq!(contents(fs, "d/g").await, b"1");
        assert_eq!(fs.metadata(&path("f")).await.unwrap(), None);
        assert_fails!(
            fs.rename(&path("d/g"), &path("taken")).await,
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            fs.rename(&path("d/g"), &path("d/g")).await,
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            fs.rename(&path("gone"), &path("x")).await,
            Error::NotFound { .. }
        );
        assert_fails!(
            fs.rename(&path("d/g"), &path("gone/g")).await,
            Error::NotFound { .. }
        );
        assert_fails!(
            fs.rename(&RelPath::ROOT, &path("x")).await,
            Error::InvalidPath { .. }
        );
        assert_eq!(contents(fs, "d/g").await, b"1");
        assert_eq!(contents(fs, "taken").await, b"2");
    }

    pub async fn rename_moves_a_directory_whole_and_never_replaces(fs: &impl Fs) {
        fs.create_dir_all(&path("a/sub")).await.unwrap();
        fs.create(&path("a/sub/f"), b"1").await.unwrap();
        fs.create_dir_all(&path("empty")).await.unwrap();
        fs.create(&path("taken"), b"2").await.unwrap();
        let renamed = fs.rename(&path("a"), &path("b")).await;
        declared(fs, Capability::RenameDir, &renamed);
        if renamed.is_err() {
            return;
        }
        assert_eq!(contents(fs, "b/sub/f").await, b"1");
        assert_eq!(fs.metadata(&path("a")).await.unwrap(), None);
        assert_fails!(
            fs.rename(&path("b"), &path("taken")).await,
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            fs.rename(&path("b"), &path("empty")).await,
            Error::AlreadyExists { .. }
        );
        assert_fails!(
            fs.rename(&path("b"), &path("b/sub/in")).await,
            Error::InvalidPath { .. }
        );
        assert_eq!(names(fs, "empty").await, []);
        assert_eq!(names(fs, "b/sub").await, named(&[("f", FileKind::File)]));
    }

    pub async fn removal_refuses_the_wrong_kind_and_full_directories(fs: &impl Fs) {
        fs.create_dir_all(&path("d")).await.unwrap();
        fs.create(&path("d/f"), b"").await.unwrap();
        assert_fails!(
            fs.remove_dir(&path("d")).await,
            Error::DirectoryNotEmpty { .. }
        );
        assert_fails!(fs.remove_file(&path("d")).await, Error::IsDirectory { .. });
        assert_fails!(
            fs.remove_dir(&path("d/f")).await,
            Error::NotDirectory { .. }
        );
        assert_fails!(fs.remove_file(&path("gone")).await, Error::NotFound { .. });
        assert_fails!(fs.remove_dir(&path("gone")).await, Error::NotFound { .. });
        assert_fails!(
            fs.remove_dir(&RelPath::ROOT).await,
            Error::InvalidPath { .. }
        );
        fs.remove_file(&path("d/f")).await.unwrap();
        fs.remove_dir(&path("d")).await.unwrap();
        assert_eq!(names(fs, "").await, []);
    }

    pub async fn sync_accepts_files_directories_and_the_root(fs: &impl Fs) {
        fs.create_dir_all(&path("d")).await.unwrap();
        fs.create(&path("d/f"), b"x").await.unwrap();
        for synced in ["d/f", "d", ""] {
            fs.sync(&path(synced)).await.unwrap();
        }
        assert_fails!(fs.sync(&path("gone")).await, Error::NotFound { .. });
        assert_eq!(contents(fs, "d/f").await, b"x");
    }
}

/// One `#[test]` per behavior, each on a fresh library from `$make`, which returns
/// the backend and whatever must outlive it.
macro_rules! conformance {
    ($backend:ident, $make:expr) => {
        conformance!(
            $backend,
            $make,
            a_created_file_reads_back_with_its_length,
            reading_needs_an_existing_file,
            a_listing_is_sorted_by_name_and_reports_kinds,
            creating_directories_makes_ancestors_and_tolerates_existing_ones,
            create_never_replaces_and_needs_an_existing_directory,
            append_extends_an_existing_file,
            rename_moves_a_file_and_never_replaces,
            rename_moves_a_directory_whole_and_never_replaces,
            removal_refuses_the_wrong_kind_and_full_directories,
            sync_accepts_files_directories_and_the_root,
        );
    };
    ($backend:ident, $make:expr, $($behavior:ident,)+) => {
        mod $backend {
            use super::*;
            $(
                #[test]
                fn $behavior() {
                    let (fs, _keep) = $make();
                    block_on(suite::$behavior(&fs));
                }
            )+
        }
    };
}

conformance!(mem, || (MemFs::new(), ()));

conformance!(mem_without_capabilities, || (
    MemFs::with_capabilities(Capabilities::NONE),
    ()
));

#[cfg(not(target_arch = "wasm32"))]
conformance!(native, || {
    let dir = tempfile::tempdir().unwrap();
    (toshokan::native::NativeFs::new(dir.path()), dir)
});
