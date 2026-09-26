//! The specimen sweep: one test per file, built at runtime with `libtest-mimic`.
//!
//! Two trees feed it. `tests/fixtures/` holds files written by this crate's own
//! writers, committed so the sweep has files to read in any checkout. With
//! `--features corpus`, the private corpus under `NORD_CORPUS_ROOT` joins it.
//!
//! Every file the reader recognizes, wherever it sits, is a specimen. Each one
//! must pass its container checksum, parse, re-encode to the same bytes, decode
//! no value its components cannot name, and match its `<file>.oracle.json`
//! sidecar if it has one. On a sample (every fixture, every specimen with a
//! sidecar, and one of each container shape among the rest), every registry
//! field must also take a new value without changing another. Nothing here names
//! a model or a directory.
//!
//! ```sh
//! cargo test -p nord-format --test corpus                        # the fixtures
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-format --features corpus --test corpus    # and the corpus
//! ```
//!
//! Filter like any other test: `--test corpus ne5/settings` runs the trials
//! whose path contains the string.

mod lookup;
mod oracle;

#[path = "../support/registry.rs"]
mod registry;
#[path = "../support/scan.rs"]
mod scan;
#[path = "../support/sidecar.rs"]
mod sidecar;

use libtest_mimic::{Arguments, Failed, Trial};
use nord_format::cbin::{self, Generation};
#[cfg(feature = "bundle")]
use nord_format::Entity;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// The path under its root, joined with `/` on every platform so filters and trial
/// kinds are the same everywhere.
fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The body span of a CBIN file, which depends on its generation: a V1 body runs
/// to the end of the file, and a V0 body stops before the trailing CRC-16.
fn cbin_body<'a>(bytes: &'a [u8], info: &cbin::Info) -> &'a [u8] {
    let len = info.body_len as usize;
    match info.header.generation {
        Generation::V1 => &bytes[bytes.len() - len..],
        Generation::V0 => &bytes[bytes.len() - 2 - len..bytes.len() - 2],
    }
}

/// One specimen: checksum, parse, byte-exact round trip, no unnamed decoded
/// values, the oracle sidecar if there is one, and, if `mutate`, the per-field
/// mutation check.
fn specimen(path: &Path, mutate: bool) -> Result<(), Failed> {
    let bytes = fs::read(path).map_err(|e| Failed::from(format!("read: {e}")))?;

    let info = if bytes.starts_with(b"CBIN") {
        let info = cbin::inspect(&mut Cursor::new(&bytes))
            .map_err(|e| Failed::from(format!("inspect: {e}")))?;
        if !info.checksum_ok {
            return Err(format!("container checksum mismatch ({:?})", info.header).into());
        }
        Some(info)
    } else {
        None
    };

    let entity = nord_format::from_stream(&mut Cursor::new(&bytes))
        .map_err(|e| Failed::from(format!("parse: {e}")))?;

    // The archive layer does not re-encode, so for a bundle the check is the
    // parse, which reads and verifies every member.
    #[cfg(feature = "bundle")]
    let is_bundle = matches!(entity, Entity::Bundle(_));
    #[cfg(not(feature = "bundle"))]
    let is_bundle = false;
    if !is_bundle {
        let back =
            nord_format::to_bytes(&entity).map_err(|e| Failed::from(format!("re-encode: {e}")))?;
        if back != bytes {
            return Err("re-encode changed the bytes".into());
        }
    }

    let unwritten = info
        .as_ref()
        .is_some_and(|i| scan::unwritten(cbin_body(&bytes, i)));
    if !unwritten {
        if let Some(values) = registry::field_values(&entity) {
            let unknown: Vec<String> = values
                .into_iter()
                // Parenthesized `unknown` is out-of-table; a bare named Unknown is valid.
                .filter(|v| v.value.contains("unknown (") || v.value.contains("Unknown("))
                .filter(|v| !known_unexplained(&v.name, &v.value))
                .map(|v| format!("{} = {}", v.name, v.value))
                .collect();
            if !unknown.is_empty() {
                return Err(format!("values no component names: {unknown:?}").into());
            }
        }
    }

    if mutate {
        registry::each_field_moves_alone(&bytes)?;
    }

    oracle::check_specimen(path, &bytes, &entity)
}

/// Out-of-table values the corpus is known to hold, exempted by field and
/// rendering so that any new one still fails. Each entry repeats a fact
/// documented on its component.
fn known_unexplained(field: &str, value: &str) -> bool {
    // Unexplained: Stage 4 factory programs store 10 in `KbZone4` fields, which
    // the zone table does not name. See `KbZone4`.
    field.ends_with(".kb_zones") && value == "unknown (10)"
}

/// The trials for one tree, named `<label>/<path under root>`. The mutation
/// check runs on the whole tree when `mutate_all`, else on [`scan::sampled`].
fn trials_for(label: &str, root: &Path, mutate_all: bool, trials: &mut Vec<Trial>) {
    let (specimens, sidecars) = scan::walk(root);
    let mut shapes_seen = std::collections::BTreeSet::new();
    if specimens.is_empty() {
        let missing = format!("no specimen under {}", root.display());
        trials.push(Trial::test(format!("{label}: present"), move || {
            Err(missing.into())
        }));
    }

    for path in specimens {
        let name = rel(root, &path);
        let kind = name.split('/').next().unwrap_or_default().to_string();
        let mutate = mutate_all || scan::sampled(&path, &mut shapes_seen);
        trials.push(
            Trial::test(format!("{label}/{name}"), move || specimen(&path, mutate)).with_kind(kind),
        );
    }

    // A sidecar without its specimen is an error.
    for sidecar in sidecars {
        let name = format!("{label}/{}", rel(root, &sidecar));
        let target = sidecar::specimen_of(&sidecar);
        trials.push(Trial::test(name, move || {
            if target.exists() {
                Ok(())
            } else {
                Err(format!(
                    "sidecar for {}, which does not exist",
                    target.file_name().unwrap().to_string_lossy()
                )
                .into())
            }
        }));
    }
}

/// The field-path reader's contract, as a trial: this target has its own harness,
/// so `#[test]` does not run here.
fn lookup_trial(fixtures: &Path) -> Trial {
    let program = fixtures.join("ne5/default.ne5p");
    Trial::test(
        "lookup: an organ accessor takes preset 1 or 2 only",
        move || {
            let bytes = fs::read(&program)
                .map_err(|e| Failed::from(format!("{}: {e}", program.display())))?;
            let entity = nord_format::from_stream(&mut Cursor::new(&bytes))
                .map_err(|e| Failed::from(e.to_string()))?;
            let asked = |preset: &str| format!("organ_panel.b3_perc_on({preset})");
            for preset in ["1", "2"] {
                lookup::lookup(&entity, &asked(preset))
                    .map_err(|e| Failed::from(format!("{}: {e}", asked(preset))))?;
            }
            for preset in ["", "0", "3", "9", "12", "+1", "one"] {
                if let Ok(spellings) = lookup::lookup(&entity, &asked(preset)) {
                    return Err(format!(
                        "{} returned {spellings:?} for a preset the organ does not have",
                        asked(preset)
                    )
                    .into());
                }
            }
            Ok(())
        },
    )
}

fn main() {
    let args = Arguments::from_args();
    let mut trials = Vec::new();

    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    trials.push(lookup_trial(&fixtures));
    trials_for("fixtures", &fixtures, true, &mut trials);

    #[cfg(feature = "corpus")]
    trials_for("corpus", &scan::root(), false, &mut trials);

    libtest_mimic::run(&args, trials).exit();
}
