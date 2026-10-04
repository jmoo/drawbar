//! The specimen sweep: one test per file, built at runtime with `libtest-mimic`.
//!
//! Two trees feed it. `tests/fixtures/` holds files written by this crate's own
//! writers, committed so the sweep has files to read in any checkout. With
//! `--features corpus`, the tree under `NORD_CORPUS_ROOT` joins it: any tree of
//! Nord files is a corpus.
//!
//! Every file the reader recognizes, wherever it sits, is a specimen. Each one
//! must pass its container checksum, parse, re-encode to the same bytes, decode
//! no value its components cannot name, and match its `<file>.oracle.json`
//! sidecar if it has one. A piano library or sample instrument must also index to
//! the bytes a whole read gives each stroke or zone, without reading the audio. On
//! a sample (every fixture, every specimen with a sidecar, and one of each
//! container shape among the rest), every registry field must also take a new
//! value without changing another. The fixtures must hold a file of every type the
//! reader dispatches. In the corpus, each claim about every specimen of a kind runs
//! once per specimen of that kind, so a tree without that kind runs none. A file
//! ending `.kernel.tsv` is an oracle for the sample codec's interpolation kernel.
//! Nothing here names a model, a directory, or a file in the corpus.
//!
//! ```sh
//! cargo test -p nord-format --test corpus                        # the fixtures
//! NORD_CORPUS_ROOT=/path/to/nord/files \
//!   cargo test -p nord-format --features corpus --test corpus    # and a corpus
//! ```
//!
//! Filter like any other test: `--test corpus ne5/settings` runs the trials
//! whose path contains the string.

/// Returns an error made of the format arguments unless `$cond` holds.
macro_rules! ensure {
    ($cond:expr, $($message:tt)+) => {
        if !$cond {
            return Err(format!($($message)+));
        }
    };
}

mod index;
#[cfg(feature = "corpus")]
mod invariants;
mod kernel;
mod lookup;
mod oracle;
mod samples;

#[cfg(feature = "corpus")]
#[path = "../support/format_table.rs"]
mod format_table;
#[path = "../support/registry.rs"]
mod registry;
#[path = "../support/scan.rs"]
mod scan;
#[path = "../support/sidecar.rs"]
mod sidecar;

use libtest_mimic::{Arguments, Failed, Trial};
use nord_format::cbin::{self, Generation};
use nord_format::util::{peek, FileType};
use nord_format::Entity;
use std::collections::BTreeSet;
use std::fmt::Display;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Prefixes an error with what was being done.
trait Context<T> {
    fn context(self, what: impl Display) -> Result<T, String>;
}

impl<T, E: Display> Context<T> for Result<T, E> {
    fn context(self, what: impl Display) -> Result<T, String> {
        self.map_err(|e| format!("{what}: {e}"))
    }
}

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
/// A [`cbin::Verifier`] fed the file in uneven chunks reports what `inspect` reported.
fn streamed_check_agrees(bytes: &[u8], info: &cbin::Info) -> Result<(), String> {
    let mut verifier = cbin::Verifier::new();
    for chunk in bytes.chunks(4093) {
        verifier.update(chunk).context("a streamed check")?;
    }
    let streamed = verifier.finish().context("a streamed check")?;
    let facts = |i: &cbin::Info| {
        (
            i.header.clone(),
            i.body_len,
            i.checksum_ok,
            i.stored_checksum,
            i.body_crc32,
        )
    };
    ensure!(
        facts(&streamed) == facts(info),
        "a streamed check reports {streamed:?} and inspect {info:?}"
    );
    Ok(())
}

/// The format an entity's file carries says, by name, the entity it decoded to. A bundle
/// is said by no name.
fn said_by_its_format(entity: &Entity) -> Result<(), String> {
    #[cfg(feature = "bundle")]
    if matches!(entity, Entity::Bundle(_)) {
        return Ok(());
    }
    let tag = entity.identity().format;
    let said = nord_format::formats::by_extension(tag.trim_end_matches('\0'));
    ensure!(
        said.map(|format| (format.tag, format.entity)) == Some((tag, entity.kind())),
        "a {tag:?} file decodes to {:?}, and its name says {said:?}",
        entity.kind()
    );
    Ok(())
}

fn specimen(path: &Path, mutate: bool) -> Result<(), Failed> {
    let bytes = fs::read(path).map_err(|e| Failed::from(format!("read: {e}")))?;

    let info = if bytes.starts_with(b"CBIN") {
        let info = cbin::inspect(&mut Cursor::new(&bytes))
            .map_err(|e| Failed::from(format!("inspect: {e}")))?;
        if !info.checksum_ok {
            return Err(format!("container checksum mismatch ({:?})", info.header).into());
        }
        let body_crc32 = nord_format::crc::crc32(cbin_body(&bytes, &info));
        if info.body_crc32 != body_crc32 {
            return Err(format!(
                "inspect reports a body crc32 of {:#010x}, and the body's is {body_crc32:#010x}",
                info.body_crc32
            )
            .into());
        }
        streamed_check_agrees(&bytes, &info).map_err(Failed::from)?;
        Some(info)
    } else {
        None
    };

    let entity = nord_format::from_stream(&mut Cursor::new(&bytes))
        .map_err(|e| Failed::from(format!("parse: {e}")))?;
    said_by_its_format(&entity).map_err(Failed::from)?;

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

    index::check(&bytes, &entity).map_err(Failed::from)?;

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

    // Sample instruments and piano libraries decode their framing, not named fields.
    let decoded = info.is_some()
        && entity.raw().is_none()
        && !matches!(entity, Entity::Sample(_) | Entity::Piano(_));
    if decoded && registry::fields(&entity).is_none() {
        return Err(format!(
            "{} decodes named fields, and with_registry! in tests/support/registry.rs has no \
             arm for it",
            entity.identity().kind
        )
        .into());
    }

    if mutate {
        registry::each_field_moves_alone(&bytes)?;
    }

    oracle::check_specimen(path, &bytes, &entity).map_err(Failed::from)
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
    let mut shapes_seen = BTreeSet::new();
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

    // A sidecar without its specimen is an error, and one stating a refusal is
    // checked here, since the sweep does not read a file the reader refuses.
    for sidecar in sidecars {
        let name = format!("{label}/{}", rel(root, &sidecar));
        trials.push(Trial::test(name, move || {
            let target = sidecar::specimen_of(&sidecar);
            if !target.exists() {
                return Err(format!(
                    "sidecar for {}, which does not exist",
                    target.file_name().unwrap().to_string_lossy()
                )
                .into());
            }
            match sidecar::load(&sidecar)?.refusal {
                Some(refusal) => oracle::check_refusal(&target, &refusal).map_err(Failed::from),
                None => Ok(()),
            }
        }));
    }

    for table in scan::kernel_tables(root) {
        let name = format!("{label}/{}", rel(root, &table));
        trials.push(Trial::test(name, move || {
            kernel::check(&table).map_err(Failed::from)
        }));
    }
}

/// One trial per specimen under `root` for each claim its kinds carry, named
/// `<label>/<path under root>: <claim>`. A file that does not parse has none; its
/// sweep trial reports it.
#[cfg(feature = "corpus")]
fn invariant_trials(label: &str, root: &Path, trials: &mut Vec<Trial>) {
    for path in scan::walk(root).0 {
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(entity) = nord_format::from_stream(&mut Cursor::new(&bytes)) else {
            continue;
        };
        let kinds = invariants::kinds(&bytes, &entity);
        let name = rel(root, &path);
        for invariant in invariants::INVARIANTS
            .iter()
            .filter(|invariant| kinds.contains(&invariant.kind))
        {
            let path = path.clone();
            trials.push(
                Trial::test(format!("{label}/{name}: {}", invariant.name), move || {
                    let bytes = fs::read(&path).map_err(|e| Failed::from(format!("read: {e}")))?;
                    let entity = samples::parse(&bytes)?;
                    (invariant.check)(&bytes, &entity).map_err(Failed::from)
                })
                .with_kind("invariant"),
            );
        }
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

/// The fixtures hold a file of every class `from_stream` reads and of every CBIN tag
/// it dispatches.
fn coverage_trial(fixtures: &Path) -> Trial {
    let fixtures = fixtures.to_path_buf();
    Trial::test(
        "fixtures: every file type the reader dispatches has one",
        move || {
            let mut classes = BTreeSet::new();
            let mut tags = BTreeSet::new();
            for path in scan::walk(&fixtures).0 {
                let mut file = fs::File::open(&path).map_err(|e| Failed::from(e.to_string()))?;
                let peeked = peek(&mut file).map_err(|e| Failed::from(e.to_string()))?;
                classes.insert(peeked.file_type.as_str().to_string());
                if matches!(peeked.file_type, FileType::Cbin) {
                    tags.insert(peeked.format);
                }
            }
            let mut missing: Vec<String> = scan::READ
                .iter()
                .map(|class| class.as_str().to_string())
                .filter(|class| !classes.contains(class))
                .collect();
            missing.extend(
                nord_format::cbin_formats()
                    .filter(|tag| !tags.contains(*tag))
                    .map(|tag| format!("CBIN {tag:?}")),
            );
            if missing.is_empty() {
                Ok(())
            } else {
                Err(format!("no fixture for: {}", missing.join(", ")).into())
            }
        },
    )
}

fn main() {
    let args = Arguments::from_args();
    let mut trials = Vec::new();

    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    trials.push(lookup_trial(&fixtures));
    trials.push(coverage_trial(&fixtures));
    trials_for("fixtures", &fixtures, true, &mut trials);

    #[cfg(feature = "corpus")]
    {
        let corpus = scan::root();
        trials_for("corpus", &corpus, false, &mut trials);
        invariant_trials("corpus", &corpus, &mut trials);
    }

    libtest_mimic::run(&args, trials).exit();
}
