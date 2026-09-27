//! Walking a specimen tree: every file the reader recognizes, wherever it sits,
//! and every oracle sidecar beside one. Nothing here depends on the tree's
//! layout: a file is a specimen if the reader recognizes it, and a sidecar
//! applies if it exists.
//!
//! ⚠️ Not a test target. Each test target that includes this module compiles its
//! own copy, and must also include `support/sidecar.rs` as `sidecar`, which
//! [`sampled`] uses to find oracle sidecars.
#![allow(dead_code)]

use nord_format::util::{peek, FileType};
use nord_format::Entity;
use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The corpus: `NORD_CORPUS_ROOT`, any tree of Nord files.
pub fn root() -> PathBuf {
    std::env::var_os("NORD_CORPUS_ROOT")
        .map(PathBuf::from)
        .expect("set NORD_CORPUS_ROOT to a tree of Nord files for --features corpus")
}

/// The classes of [`peek`] that `from_stream` reads in this build.
pub const READ: &[FileType] = &[
    FileType::Cbin,
    FileType::Cne3,
    FileType::Midi,
    FileType::SampleProject,
    FileType::Sysex,
    #[cfg(feature = "bundle")]
    FileType::Zip,
];

/// Whether the reader takes this file, decided by its leading bytes as
/// `from_stream` decides. `.skip.` in the name marks a file to leave out.
pub fn wanted(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    if name.contains(".skip.") || name.ends_with(".oracle.json") {
        return false;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    peek(&mut file).is_ok_and(|p| READ.iter().any(|t| t.as_str() == p.file_type.as_str()))
}

/// A CBIN file's tag and header generation.
pub type Shape = (Vec<u8>, u8);

/// A CBIN file's [`Shape`]. `None` for anything else.
pub fn shape(path: &Path) -> Option<Shape> {
    use std::io::Read;
    let mut head = [0u8; 12];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .ok()?;
    head.starts_with(b"CBIN")
        .then(|| (head[8..12].to_vec(), head[4]))
}

/// Whether the per-field mutation check runs on this specimen. It runs on every
/// specimen with an oracle sidecar, every file that is not CBIN, and the first
/// CBIN file of each [`Shape`]. `seen` holds the shapes already taken, so a sweep
/// of a tree takes one of each.
///
/// The check exercises code paths, so more specimens would only add baselines,
/// and this sample already varies them.
pub fn sampled(path: &Path, seen: &mut BTreeSet<Shape>) -> bool {
    crate::sidecar::sidecar_of(path).exists() || shape(path).is_none_or(|s| seen.insert(s))
}

/// An all-ones body is an unwritten slot, whose fields may be outside every table.
pub fn unwritten(body: &[u8]) -> bool {
    !body.is_empty() && body.iter().all(|&byte| byte == 0xff)
}

/// Every file under `root`, skipping dotfiles and dot directories, in directory
/// order.
fn visit(root: &Path, each: &mut impl FnMut(PathBuf)) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.file_name().unwrap().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                each(path);
            }
        }
    }
}

/// Every wanted file under `root`, and every sidecar, each in a stable order.
pub fn walk(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut specimens = Vec::new();
    let mut sidecars = Vec::new();
    visit(root, &mut |path| {
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".oracle.json")
        {
            sidecars.push(path);
        } else if wanted(&path) {
            specimens.push(path);
        }
    });
    specimens.sort();
    sidecars.sort();
    (specimens, sidecars)
}

/// One specimen, read and parsed once.
pub struct Specimen {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub entity: Entity,
}

/// What one tree yielded: the specimens that parsed, and the files the reader
/// recognized but could not parse, each with its error.
pub struct Tree {
    pub specimens: Vec<Specimen>,
    pub unparsed: Vec<(PathBuf, String)>,
}

/// Every specimen under `root`, read and parsed. A file that fails to parse goes
/// to `unparsed` and the run continues; the sweep in `tests/corpus` reports each
/// one as a failing trial. An empty tree panics, because it means `root` is wrong.
pub fn read_tree(root: &Path) -> Tree {
    let (paths, _) = walk(root);
    assert!(!paths.is_empty(), "no specimen under {}", root.display());
    let mut tree = Tree {
        specimens: Vec::new(),
        unparsed: Vec::new(),
    };
    for path in paths {
        let bytes = fs::read(&path).unwrap();
        match nord_format::from_stream(&mut Cursor::new(&bytes)) {
            Ok(entity) => tree.specimens.push(Specimen {
                path,
                bytes,
                entity,
            }),
            Err(e) => tree.unparsed.push((path, e.to_string())),
        }
    }
    tree
}

/// The specimens under `root` that parsed. Each file that did not parse is named
/// on stderr; the sweep in `tests/corpus` fails on it.
fn parsed(root: &Path) -> Vec<Specimen> {
    let tree = read_tree(root);
    if !tree.unparsed.is_empty() {
        // ⚠️ libtest captures the print macros, so only a direct write reaches the
        // terminal when the suite passes.
        use std::io::Write;
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "warning: {} of {} files under {} did not parse and are left out of this suite; \
             `--test corpus` reports each one as a failing trial",
            tree.unparsed.len(),
            tree.unparsed.len() + tree.specimens.len(),
            root.display()
        );
        for (path, error) in &tree.unparsed {
            let _ = writeln!(err, "  {}: {error}", path.display());
        }
    }
    tree.specimens
}

/// The whole corpus, parsed once per test binary and shared by every test in it.
pub fn corpus() -> &'static [Specimen] {
    static CORPUS: OnceLock<Vec<Specimen>> = OnceLock::new();
    CORPUS.get_or_init(|| parsed(&root()))
}

/// The committed fixtures, present in every checkout, parsed once per test
/// binary.
pub fn fixtures() -> &'static [Specimen] {
    static FIXTURES: OnceLock<Vec<Specimen>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| parsed(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")))
}

/// Every sample-codec kernel oracle under `root`: a file whose name ends
/// `.kernel.tsv`.
pub fn kernel_tables(root: &Path) -> Vec<PathBuf> {
    let mut tables = Vec::new();
    visit(root, &mut |path| {
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".kernel.tsv")
        {
            tables.push(path);
        }
    });
    tables.sort();
    tables
}
