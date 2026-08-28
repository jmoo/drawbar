//! The specimen sweep: one test per file, built at runtime with `libtest-mimic`.
//!
//! Two trees feed it. `tests/fixtures/` — files this crate's own writers
//! produced, committed so the sweep has something to read in any checkout —
//! always; the private corpus under `NORD_CORPUS_ROOT` with `--features corpus`.
//! Every file the reader recognizes, wherever it sits, is a specimen: it passes
//! its container checksum, parses, re-encodes byte-exactly, decodes no value its
//! components cannot name, and its `<file>.oracle.json` sidecar holds where
//! there is one. On a sample of them — every fixture, every specimen with a
//! sidecar, and one of each container shape among the rest — every registry
//! field also takes a new value without moving another. Nothing here names a
//! model or a directory.
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

/// The path under its root, `/`-joined on every platform so the documented
/// filters and the trial kinds read the same everywhere.
fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The body span of a CBIN file, from its generation: a type-1 body runs to end
/// of file, a type-0 body stops short of the trailing crc16.
fn cbin_body<'a>(bytes: &'a [u8], info: &cbin::Info) -> &'a [u8] {
    let len = info.body_len as usize;
    match info.header.generation {
        Generation::V1 => &bytes[bytes.len() - len..],
        Generation::V0 => &bytes[bytes.len() - 2 - len..bytes.len() - 2],
    }
}

/// One specimen: checksum, parse, byte-exact round trip, no unnameable decoded
/// values, the oracle sidecar if there is one, and — where `mutate` — every
/// field moves alone.
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

    // The archive layer does not re-encode, so for a bundle the parse itself —
    // every member read and container-verified — is the whole check.
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

    // All-ones bodies are unwritten slots, so their fields may be outside every table.
    let all_ones = info
        .as_ref()
        .map(|i| {
            let body = cbin_body(&bytes, i);
            !body.is_empty() && body.iter().all(|&b| b == 0xff)
        })
        .unwrap_or(false);
    if !all_ones {
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

/// Out-of-table values the corpus is known to hold, exempted by exact field and
/// rendering so anything new still fails. Each entry restates a doc on the
/// component itself — the exemption lives where the value does.
fn known_unexplained(field: &str, value: &str) -> bool {
    // Unexplained: Stage 4 factory programs reach a stored 10 in `KbZone4`
    // fields, which the zone table does not name — see `KbZone4`'s rustdoc.
    field.ends_with(".kb_zones") && value == "unknown (10)"
}

/// The trials for one tree, named `<label>/<path under root>`. The mutation
/// check runs on the whole tree when `mutate_all`, else on the specimens with a
/// sidecar plus the first of each container shape: the check is a property of
/// the code path, and what more specimens add is diverse baselines, which those
/// already are.
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
        let mutate = mutate_all
            || sidecar::sidecar_of(&path).exists()
            || scan::shape(&path).is_none_or(|s| shapes_seen.insert(s));
        trials.push(
            Trial::test(format!("{label}/{name}"), move || specimen(&path, mutate)).with_kind(kind),
        );
    }

    // A sidecar whose specimen is gone is an error, not a leftover.
    for sidecar in sidecars {
        let name = format!("{label}/{}", rel(root, &sidecar));
        let target = sidecar::specimen_of(&sidecar);
        trials.push(Trial::test(name, move || {
            if target.exists() {
                Ok(())
            } else {
                Err(format!(
                    "sidecar for {}, which is gone",
                    target.file_name().unwrap().to_string_lossy()
                )
                .into())
            }
        }));
    }
}

fn main() {
    let args = Arguments::from_args();
    let mut trials = Vec::new();

    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    trials_for("fixtures", &fixtures, true, &mut trials);

    #[cfg(feature = "corpus")]
    trials_for("corpus", &scan::root(), false, &mut trials);

    libtest_mimic::run(&args, trials).exit();
}
