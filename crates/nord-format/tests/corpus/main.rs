//! The whole-corpus sweep: one test per specimen, built at runtime with
//! `libtest-mimic`.
//!
//! Every specimen of every model — the R2 tier included, when the assembly
//! projects it in — parses, passes its container checksum, re-encodes
//! byte-exactly, decodes no value its components cannot name, and satisfies its
//! oracle sidecar where the corpus ships one. A specimen joins the sweep by
//! existing; a model joins by having a directory. Nothing here names a model.
//!
//! ```sh
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-format --features corpus --test corpus
//! ```
//!
//! Filter like any other test: `--test corpus ne5/settings` runs the trials
//! whose corpus-relative path contains the string.

mod lookup;
mod oracle;

#[path = "../support/corpus.rs"]
mod corpus_loc;
#[path = "../support/sidecar.rs"]
mod sidecar;

use libtest_mimic::{Arguments, Failed, Trial};
use nord_format::cbin::{self, Generation};
use nord_format::Entity;
use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Extensions the sweep reads as entities. Everything else in the corpus —
/// captures, sidecars, manifests, documentation — is deliberately not a format.
fn wanted(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    // `.body` files are bare bodies with no container; `.skip.` marks a
    // specimen the suite is told to ignore.
    let name = path.file_name().unwrap().to_string_lossy();
    if name.contains(".skip.") || name.ends_with(".body") {
        return false;
    }
    !matches!(
        ext,
        "md" | "json"
            | "xml"
            | "nix"
            | "lock"
            | "tsv"
            | "bin"
            | "txt"
            | "pcapng"
            // Recorded wire exchanges, replayed by nord-usb rather than parsed.
            | "script"
            | "nsmpproj"
            | "html"
            | "pdf"
            | "gitignore"
    )
}

/// Directories the walk never enters: `pending/` is an untracked staging area,
/// `tools/` is the corpus's own CLI.
fn skipped_dir(name: &str) -> bool {
    matches!(name, "pending" | "tools" | ".git")
}

/// Every wanted specimen and every oracle sidecar under the corpus.
fn walk(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut specimens = Vec::new();
    let mut sidecars = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if !skipped_dir(&name) {
                    stack.push(path);
                }
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

/// The corpus-relative path, `/`-joined on every platform so the documented
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
/// values, and the oracle sidecar if the corpus ships one.
fn specimen(root: &Path, path: &Path) -> Result<(), Failed> {
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
    if !matches!(entity, Entity::Bundle(_)) {
        let back =
            nord_format::to_bytes(&entity).map_err(|e| Failed::from(format!("re-encode: {e}")))?;
        if back != bytes {
            return Err("re-encode changed the bytes".into());
        }
    }

    // A decoded value no component can name is worth investigating, not
    // suppressing. An all-ones body is exempt: it is a slot the instrument
    // never wrote, so every field there is legitimately out of table.
    let all_ones = info
        .as_ref()
        .map(|i| {
            let body = cbin_body(&bytes, i);
            !body.is_empty() && body.iter().all(|&b| b == 0xff)
        })
        .unwrap_or(false);
    if !all_ones {
        if let Some(values) = lookup::field_values(&entity) {
            let unknown: Vec<String> = values
                .into_iter()
                // An out-of-table value renders as `unknown (raw)` / `Unknown(raw)`.
                // A *named* variant spelled Unknown — the Electro 5's old-firmware
                // off routing — is vocabulary, not a gap, and renders bare.
                .filter(|v| v.value.contains("unknown (") || v.value.contains("Unknown("))
                .filter(|v| !known_unexplained(&v.name, &v.value))
                .map(|v| format!("{} = {}", v.name, v.value))
                .collect();
            if !unknown.is_empty() {
                return Err(format!("values no component names: {unknown:?}").into());
            }
        }
    }

    oracle::check_specimen(root, path, &bytes, &entity)
}

/// Out-of-table values the corpus is known to hold, exempted by exact field and
/// rendering so anything new still fails. Each entry restates a doc on the
/// component itself — the exemption lives where the value does.
fn known_unexplained(field: &str, value: &str) -> bool {
    // Unexplained: Stage 4 factory programs reach a stored 10 in `KbZone4`
    // fields, which the zone table does not name — see `KbZone4`'s rustdoc.
    field.ends_with(".kb_zones") && value == "unknown (10)"
}

/// Floors on what the sweep read, so a whole directory, a whole format or a
/// whole header generation silently dropping out of the walk fails the run
/// rather than shrinking it. Each floor sits well under the committed tier, so
/// corpus growth never trips one and only a loss does.
fn coverage(specimens: &[PathBuf], root: &Path) -> Result<(), Failed> {
    if specimens.len() < 9000 {
        return Err(format!("only {} specimens read — corpus present?", specimens.len()).into());
    }
    // Every format has an extension of its own, so the spread of extensions is
    // the spread of formats: one slipping out of `wanted` lands here.
    let extensions: BTreeSet<_> = specimens.iter().filter_map(|s| s.extension()).collect();
    if extensions.len() < 50 {
        return Err(format!("only {} specimen extensions read", extensions.len()).into());
    }
    // Type-0 containers (the trailing crc16) must come from instrument-written
    // files, or that generation is verified against synthetic bodies alone.
    let type0 = specimens
        .iter()
        .filter(|s| {
            let mut head = [0u8; 5];
            fs::File::open(s)
                .and_then(|mut f| f.read_exact(&mut head))
                .is_ok()
                && head.starts_with(b"CBIN")
                && head[4] == 0
        })
        .count();
    if type0 < 1000 {
        return Err(format!("only {type0} type-0 containers read").into());
    }
    for entry in fs::read_dir(root).map_err(|e| Failed::from(e.to_string()))? {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !path.is_dir() || skipped_dir(&name) {
            continue;
        }
        if !specimens.iter().any(|s| s.starts_with(&path)) {
            return Err(format!("model directory {name}/ contributed no specimen").into());
        }
    }
    Ok(())
}

fn main() {
    let args = Arguments::from_args();
    let root = corpus_loc::root();
    let (specimens, sidecars) = walk(&root);

    let mut trials = Vec::new();
    for path in specimens.clone() {
        let name = rel(&root, &path);
        let kind = name.split('/').next().unwrap_or_default().to_string();
        let root = root.clone();
        trials.push(Trial::test(name, move || specimen(&root, &path)).with_kind(kind));
    }

    for sidecar in sidecars {
        let name = rel(&root, &sidecar);
        if sidecar.file_name().is_some_and(|n| n == "dir.oracle.json") {
            let dir = sidecar.parent().unwrap().to_path_buf();
            trials.push(Trial::test(name, move || {
                oracle::check_dir_sidecar(&dir, &sidecar)
            }));
        } else {
            // A sidecar whose specimen is gone is an error, not a leftover.
            let target = sidecar::specimen_of(&sidecar);
            trials.push(Trial::test(name, move || {
                if target.exists() {
                    Ok(())
                } else {
                    Err(format!(
                        "sidecar for {}, which the corpus no longer holds",
                        target.file_name().unwrap().to_string_lossy()
                    )
                    .into())
                }
            }));
        }
    }

    trials.push(Trial::test("coverage", move || coverage(&specimens, &root)));
    libtest_mimic::run(&args, trials).exit();
}
