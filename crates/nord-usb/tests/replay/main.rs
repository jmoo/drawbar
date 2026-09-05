//! The replay sweep: one test per script, built at runtime with `libtest-mimic`.
//!
//! Two trees feed it. `tests/scripts/` — protocol framing this repo may hold, committed
//! so the sweep has something to read in any checkout — always; the private corpus under
//! `NORD_CORPUS_ROOT` with `--features corpus`. Every `*.script` under either, wherever
//! it sits, is a trial: it parses, and every frame's length field agrees with its bytes.
//! A script that declares an `intent` is also *driven* — its sections replayed in order
//! through an exact-match transport, each judged against its `expect`, the whole script
//! required to be consumed. Nothing here names a directory.
//!
//! ```sh
//! cargo test -p nord-usb --features replay --test replay        # the fixtures
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-usb --features corpus --test replay      # and the corpus
//! ```
//!
//! Filter like any other test: `--test replay program/move` runs the trials whose name
//! contains the string.

mod drive;

#[path = "../support/scripts.rs"]
mod scripts;

use libtest_mimic::{Arguments, Failed, Trial};
use nord_usb::transport::{ReplayTransport, Script, Step};
use std::fs;
use std::path::Path;

/// Every frame is one whole protocol message, so its leading length word must equal the
/// bytes recorded for it. A frame that fails this was captured across a buffer boundary
/// or edited by hand, and every offset after it is suspect.
fn framing(steps: &[Step]) -> Result<(), Failed> {
    for (i, step) in steps.iter().enumerate() {
        let head = step.bytes.get(..4).ok_or_else(|| {
            Failed::from(format!(
                "frame {i} is {} bytes, too short to be a message",
                step.bytes.len()
            ))
        })?;
        let declared = u32::from_be_bytes(head.try_into().expect("four bytes")) as usize;
        if declared != step.bytes.len() {
            return Err(format!(
                "frame {i} declares {declared} bytes and carries {}",
                step.bytes.len()
            )
            .into());
        }
    }
    Ok(())
}

/// Replay one script's sections in order, on one transport.
fn replay(script: &Script, dir: &Path) -> Result<(), Failed> {
    let mut t = ReplayTransport::new(script.steps());
    // What a `device geometry` section read, for the later sections a walk or a library
    // write bounds itself by.
    let mut geometry = None;

    for (i, section) in script.sections.iter().enumerate() {
        let before = t.position();
        let intent = section.intent.as_deref().expect("checked by the caller");
        let words = drive::words(intent).map_err(|e| where_(i, intent, e))?;
        let (class, verb) = match words.split_first() {
            Some((class, rest)) if !rest.is_empty() => (class, rest),
            _ => return Err(where_(i, intent, "an intent is `<class> <verb> <args…>`")),
        };
        let class = drive::class_of(class).map_err(|e| where_(i, intent, e))?;

        let outcome = pollster::block_on(drive::drive(
            &mut t,
            &mut geometry,
            class,
            &verb[0],
            &verb[1..],
            dir,
        ));
        section
            .expect()
            .check(&outcome)
            .map_err(|e| where_(i, intent, e))?;

        // Each section accounts for its own frames. Without this a section that stopped
        // short would be reported against whichever later one first ran out of script.
        if t.position() != before + section.steps.len() {
            return Err(where_(
                i,
                intent,
                format!(
                    "consumed {} of the section's {} frames",
                    t.position() - before,
                    section.steps.len()
                ),
            ));
        }

        if let Ok(Some(produced)) = outcome {
            let expected = fs::read(&produced.expected)
                .map_err(|e| where_(i, intent, format!("{}: {e}", produced.expected.display())))?;
            if produced.bytes != expected {
                return Err(where_(
                    i,
                    intent,
                    format!(
                        "produced {} bytes, and {} holds {}",
                        produced.bytes.len(),
                        produced
                            .expected
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                        expected.len()
                    ),
                ));
            }
        }
    }

    if !t.is_exhausted() {
        return Err(format!(
            "{} of {} frames left unread",
            script.steps().len() - t.position(),
            script.steps().len()
        )
        .into());
    }
    Ok(())
}

fn where_(i: usize, intent: &str, what: impl std::fmt::Display) -> Failed {
    Failed::from(format!("section {} ({intent}): {what}", i + 1))
}

/// One script: it parses, its frames are framed, and if it says what it was doing, it
/// does it again.
fn trial(path: &Path) -> Result<(), Failed> {
    let text = fs::read_to_string(path).map_err(|e| Failed::from(format!("read: {e}")))?;
    let script = Script::parse(&text).map_err(|e| Failed::from(e.to_string()))?;
    framing(&script.steps())?;

    let declared = script
        .sections
        .iter()
        .filter(|s| s.intent.is_some())
        .count();
    if declared == 0 {
        return Ok(());
    }
    if declared != script.sections.len() {
        return Err(
            "some sections declare an intent and some do not, so the frames in \
                    between belong to nothing"
                .into(),
        );
    }
    replay(&script, path.parent().expect("a script has a directory"))
}

/// The trials for one tree, named `<label>/<path under root>` and kinded by the first
/// path component, so `--kind program` and a path filter both work.
fn trials_for(label: &str, root: &Path, trials: &mut Vec<Trial>) {
    let found = scripts::walk(root);
    if found.is_empty() {
        let missing = format!("no script under {}", root.display());
        trials.push(Trial::test(format!("{label}: present"), move || {
            Err(missing.into())
        }));
    }
    for path in found {
        let name = scripts::rel(root, &path);
        let kind = name.split('/').next().unwrap_or_default().to_string();
        trials.push(Trial::test(format!("{label}/{name}"), move || trial(&path)).with_kind(kind));
    }
}

fn main() {
    let args = Arguments::from_args();
    let mut trials = Vec::new();

    trials_for("fixtures", &scripts::fixtures(), &mut trials);

    #[cfg(feature = "corpus")]
    trials_for("corpus", &scripts::corpus(), &mut trials);

    libtest_mimic::run(&args, trials).exit();
}
