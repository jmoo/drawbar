//! Walking a specimen tree: every file the reader recognizes, wherever it sits,
//! and every oracle sidecar beside one. Nothing here knows how a tree is laid
//! out — a specimen joins by being readable, an oracle by existing.
//!
//! ⚠️ A rustc-visible support module, not a test target — each test target that
//! includes it compiles its own copy.
#![allow(dead_code)]

use nord_format::util::{peek, FileType};
use nord_format::Entity;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The private corpus checkout: `NORD_CORPUS_ROOT`.
pub fn root() -> PathBuf {
    std::env::var_os("NORD_CORPUS_ROOT")
        .map(PathBuf::from)
        .expect("set NORD_CORPUS_ROOT to a nord-corpus checkout for --features corpus")
}

/// Is this a file the reader takes? Decided by the leading bytes, the way
/// `from_stream` decides. `.skip.` in the name is the corpus's marker for a
/// file left out on purpose.
pub fn wanted(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    if name.contains(".skip.") || name.ends_with(".oracle.json") {
        return false;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    matches!(
        peek(&mut file).map(|p| p.file_type),
        Ok(FileType::Cbin | FileType::Cne3 | FileType::Midi | FileType::Sysex | FileType::Zip)
    )
}

/// Every wanted file under `root`, and every sidecar, each in a stable order.
pub fn walk(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut specimens = Vec::new();
    let mut sidecars = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != ".git") {
                    stack.push(path);
                }
            } else if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".oracle.json"))
            {
                sidecars.push(path);
            } else if wanted(&path) {
                specimens.push(path);
            }
        }
    }
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

/// The whole corpus, parsed once per test binary and shared by every test in
/// it. A file that fails to parse panics here; the sweep in `tests/corpus`
/// is where it is reported by name.
pub fn corpus() -> &'static [Specimen] {
    static CORPUS: OnceLock<Vec<Specimen>> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let root = root();
        let (paths, _) = walk(&root);
        assert!(!paths.is_empty(), "no specimen under {}", root.display());
        paths
            .into_iter()
            .map(|path| {
                let bytes = fs::read(&path).unwrap();
                let entity = nord_format::from_stream(&mut Cursor::new(&bytes))
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                Specimen {
                    path,
                    bytes,
                    entity,
                }
            })
            .collect()
    })
}

/// The one specimen with this file name.
pub fn named(name: &str) -> &'static Specimen {
    let mut hits = corpus()
        .iter()
        .filter(|s| s.path.file_name().is_some_and(|n| n == name));
    let found = hits
        .next()
        .unwrap_or_else(|| panic!("no specimen named {name}"));
    assert!(hits.next().is_none(), "more than one specimen named {name}");
    found
}
