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
        Ok(FileType::Cbin
            | FileType::Cne3
            | FileType::Midi
            | FileType::SampleProject
            | FileType::Sysex
            | FileType::Zip)
    )
}

/// A CBIN file's tag and header generation — the shape its registry reads
/// through. `None` for anything else.
pub fn shape(path: &Path) -> Option<(Vec<u8>, u8)> {
    use std::io::Read;
    let mut head = [0u8; 12];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .ok()?;
    head.starts_with(b"CBIN")
        .then(|| (head[8..12].to_vec(), head[4]))
}

/// Every wanted file under `root`, and every sidecar, each in a stable order.
pub fn walk(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut specimens = Vec::new();
    let mut sidecars = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if name.ends_with(".oracle.json") {
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

/// What one tree yielded: the specimens that parsed, and the files the reader
/// recognized but could not parse, each with its error.
struct Tree {
    specimens: Vec<Specimen>,
    unparsed: Vec<(PathBuf, String)>,
}

/// Every specimen under `root`, read and parsed. A file that fails to parse
/// lands in `unparsed` rather than ending the run: the sweep in `tests/corpus`
/// is where a parse failure is a failing trial, named. An empty tree is still
/// fatal — that is a broken `root`, not a broken specimen.
fn read_tree(root: &Path) -> Tree {
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

/// The specimens under `root`, with anything that did not parse named on
/// stderr. The suites that call this run on what parsed; the sweep is what
/// fails on what did not.
fn parsed(root: &Path) -> Vec<Specimen> {
    let tree = read_tree(root);
    if !tree.unparsed.is_empty() {
        // ⚠️ libtest captures the print macros per test, so a direct write is
        // what reaches the terminal when the suite goes on to pass.
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

/// The committed fixtures, the tree that is there in any checkout, parsed once
/// per test binary.
pub fn fixtures() -> &'static [Specimen] {
    static FIXTURES: OnceLock<Vec<Specimen>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| parsed(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn name(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("nord-scan-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_specimen_that_does_not_parse_is_collected_and_the_rest_are_read() {
        let dir = scratch("unparsed");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cbin/npsy.g0.cbin");
        fs::copy(&fixture, dir.join("readable.cbin")).unwrap();
        // A CBIN container carrying a tag no reader claims: the sniffer takes
        // the file, `from_stream` refuses it.
        fs::write(dir.join("garbage.cbin"), b"CBIN\0\0\0\0zzzz\0\0\0\0").unwrap();

        let tree = read_tree(&dir);
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            tree.specimens
                .iter()
                .map(|s| name(&s.path))
                .collect::<Vec<_>>(),
            ["readable.cbin"]
        );
        assert_eq!(
            tree.unparsed
                .iter()
                .map(|(p, _)| name(p))
                .collect::<Vec<_>>(),
            ["garbage.cbin"]
        );
        assert!(
            tree.unparsed[0].1.contains("zzzz"),
            "an unparsed file carries the reader's error, got {:?}",
            tree.unparsed[0].1
        );
    }
}
