//! The replay sweep: one test per script, built at runtime with `libtest-mimic`.
//!
//! Two trees feed it: `tests/scripts/`, committed so the sweep has something to read in
//! any checkout, and the private corpus under `NORD_CORPUS_ROOT` with `--features
//! corpus`. Every `*.script` under either, wherever it sits, is a trial: it must parse,
//! and every frame's length field must agree with its bytes. A script that declares an
//! `intent` is also driven: its sections are replayed in order through an exact-match
//! transport, each is judged against its `expect`, and the whole script must be
//! consumed. A script that declares none must name the tests that drive it, or say why
//! nothing does.
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

#[path = "../support/geometry.rs"]
mod geometry;
#[path = "../support/scripts.rs"]
mod scripts;

use libtest_mimic::{Arguments, Failed, Trial};
use nord_usb::transport::{ErrKind, Expect, Header, ReplayTransport, Script, Step};
use std::fs;
use std::path::Path;

/// Every frame is one whole protocol message, so its leading length word must equal the
/// bytes recorded for it. A frame that fails this was captured across a buffer boundary
/// or edited by hand, and every offset after it is suspect.
fn framing(steps: &[Step]) -> Result<(), Failed> {
    for (i, frame) in steps
        .iter()
        .enumerate()
        .filter_map(|(i, step)| Some((i, step.frame()?)))
    {
        let head = frame.get(..4).ok_or_else(|| {
            Failed::from(format!(
                "step {i} is {} bytes, too short to be a message",
                frame.len()
            ))
        })?;
        let declared = u32::from_be_bytes(head.try_into().expect("four bytes")) as usize;
        if declared != frame.len() {
            return Err(format!(
                "step {i} declares {declared} bytes and carries {}",
                frame.len()
            )
            .into());
        }
    }
    Ok(())
}

/// A script with no intent is only framing-checked here, so it must name the test files
/// that drive it, each of which must mention it by its path under the tree, or say why
/// nothing does.
fn accounted_for(header: &Header, name: &str) -> Result<(), Failed> {
    let tests = match (&header.driven_by, &header.undriven) {
        (Some(tests), None) => tests,
        (None, Some(_)) => return Ok(()),
        (Some(_), Some(_)) => return Err("driven_by and undriven contradict each other".into()),
        (None, None) => {
            return Err(
                "declares no intent, so the sweep checks only its framing: name the \
                 tests that drive it with `driven_by`, or say why nothing does with \
                 `undriven`"
                    .into(),
            )
        }
    };
    for test in tests.split(',').map(str::trim) {
        let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(test))
            .map_err(|e| Failed::from(format!("driven_by {test}: {e}")))?;
        if !source.contains(name) {
            return Err(format!("driven_by names {test}, which never mentions {name}").into());
        }
    }
    Ok(())
}

/// Replay one script's sections in order, on one transport.
fn replay(script: &Script, dir: &Path) -> Result<(), Failed> {
    let mut t = ReplayTransport::new(script.steps());
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
            script.header.device.as_deref(),
            class,
            &verb[0],
            &verb[1..],
            dir,
        ));
        section
            .expect()
            .check(&outcome)
            .map_err(|e| where_(i, intent, e))?;
        if let Some(mismatch) = t.mismatch() {
            if section.expect() != Expect::Err(ErrKind::Replay) {
                return Err(where_(
                    i,
                    intent,
                    format!("the replay disagreed: {mismatch}"),
                ));
            }
        }

        // Each section accounts for its own frames. Without this a section that stopped
        // short would be reported against whichever later one first ran out of script.
        if t.position() != before + section.steps.len() {
            return Err(where_(
                i,
                intent,
                format!(
                    "consumed {} of the section's {} steps",
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

    if t.position() != script.steps().len() {
        return Err(format!(
            "{} of {} steps left unread",
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

/// One script: it parses, its frames are well formed, and any intents it declares are
/// driven. `name` is its path under its tree.
fn trial(path: &Path, name: &str) -> Result<(), Failed> {
    let text = fs::read_to_string(path).map_err(|e| Failed::from(format!("read: {e}")))?;
    let script = Script::parse(&text).map_err(|e| Failed::from(e.to_string()))?;
    framing(&script.steps())?;

    let declared = script
        .sections
        .iter()
        .filter(|s| s.intent.is_some())
        .count();
    if declared == 0 {
        return accounted_for(&script.header, name);
    }
    if script.header.driven_by.is_some() || script.header.undriven.is_some() {
        return Err(
            "driven_by and undriven account for a script with no intent, and this one \
             declares intents"
                .into(),
        );
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
        let trial_name = format!("{label}/{name}");
        trials.push(Trial::test(trial_name, move || trial(&path, &name)).with_kind(kind));
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
