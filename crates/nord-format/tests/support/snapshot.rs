//! Comparing a generated report against its committed copy under
//! `tests/snapshots/`, or rewriting it under `UPDATE_SNAPSHOTS=1`.
//!
//! ⚠️ A rustc-visible support module, not a test target — each test target that
//! includes it compiles its own copy.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// The committed copy of one snapshot, by its name under `tests/snapshots/`.
pub fn path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name)
}

/// Compare against the committed snapshot, or rewrite it under `UPDATE_SNAPSHOTS=1`.
pub fn compare(name: &str, actual: &str) {
    if let Err(report) = check(name, actual) {
        panic!("{report}");
    }
}

/// [`compare`] without the panic, for a caller reporting several snapshots at once.
pub fn check(name: &str, actual: &str) -> Result<(), String> {
    let path = path(name);

    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        println!("wrote {}", path.display());
        return Ok(());
    }

    let expected = fs::read_to_string(&path).map_err(|e| {
        format!(
            "{} is missing ({e}) — generate it with UPDATE_SNAPSHOTS=1",
            path.display()
        )
    })?;

    if expected == actual {
        return Ok(());
    }

    let mut diff = String::new();
    for (n, (want, got)) in expected.lines().zip(actual.lines()).enumerate() {
        if want != got {
            let _ = write!(diff, "\n  line {}:\n    want {want}\n    got  {got}", n + 1);
        }
    }
    if expected.lines().count() != actual.lines().count() {
        let _ = write!(
            diff,
            "\n  length: want {} lines, got {}",
            expected.lines().count(),
            actual.lines().count()
        );
    }

    Err(format!(
        "{} no longer matches the decode:{diff}\n\nIf the change is intended, re-bless with \
         UPDATE_SNAPSHOTS=1 — after reading the diff.",
        path.display()
    ))
}
