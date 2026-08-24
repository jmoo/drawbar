//! Finding and reading replay scripts. Nothing here knows how a tree is laid out — a
//! script joins by having the extension, wherever it sits.
//!
//! ⚠️ A rustc-visible support module, not a test target — each test target that
//! includes it compiles its own copy.
#![allow(dead_code)]

use nord_usb::transport::Script;
use std::fs;
use std::path::{Path, PathBuf};

/// The committed scripts, which every checkout has.
pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts")
}

/// The private corpus checkout: `NORD_CORPUS_ROOT`.
pub fn corpus() -> PathBuf {
    std::env::var_os("NORD_CORPUS_ROOT")
        .map(PathBuf::from)
        .expect("set NORD_CORPUS_ROOT to a nord-corpus checkout for --features corpus")
}

/// Every `*.script` under `root`, in a stable order.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != ".git") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "script") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// One committed script, by its path under `tests/scripts`.
pub fn fixture(rel: &str) -> Script {
    let path = fixtures().join(rel);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    Script::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The path under its root, `/`-joined on every platform so the documented filters and
/// the trial kinds read the same everywhere.
pub fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
