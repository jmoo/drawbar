//! Oracle-sidecar checking: `<specimen>.oracle.json` is the corpus's
//! machine-readable record of what a differential capture pinned, and this is
//! the reader the corpus README promises — a specimen joins the sweep by
//! gaining a sidecar there, not by anyone adding a case here.

use crate::lookup;
use crate::sidecar::{expectation, sidecar_of, SPECIMEN_KEYS};
use libtest_mimic::Failed;
use nord_format::formats::ne5::{OrganModel, Preset};
use nord_format::formats::nsmp;
use nord_format::{Entity, Live, Program, Sample};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Check one specimen against its sidecar, if it has one.
pub fn check_specimen(path: &Path, bytes: &[u8], entity: &Entity) -> Result<(), Failed> {
    let sidecar = sidecar_of(path);
    if !sidecar.exists() {
        return Ok(());
    }

    // `load` type-checks every key it returns, and refuses a claim beside
    // `unoracled`, so each access below reads a value of the declared type.
    let v = crate::sidecar::load(&sidecar, SPECIMEN_KEYS).map_err(Failed::from)?;
    if v.get("unoracled").is_some() {
        return Ok(());
    }

    let mut wrong: Vec<String> = Vec::new();

    if let Some(sibling) = v.get("same_body_as").map(as_str) {
        let other = path.parent().unwrap().join(sibling);
        let other_bytes =
            fs::read(&other).map_err(|e| Failed::from(format!("same_body_as {sibling}: {e}")))?;
        if other_bytes != bytes {
            wrong.push(format!(
                "no longer byte-identical to {sibling} — the corpus gained a capture that \
                 moves something, so this specimen can now say more"
            ));
        }
    }

    if let Some(fields) = v
        .get("fields")
        .map(|f| f.as_object().expect("a checked object"))
    {
        for (field_path, expected) in fields {
            let (want, slack) =
                expectation(expected).map_err(|e| Failed::from(format!("{field_path}: {e}")))?;
            match lookup::lookup(entity, field_path) {
                Err(e) => wrong.push(format!("{field_path}: {e}")),
                Ok(spellings) => {
                    if !matches(&want, &spellings, slack) {
                        wrong.push(format!(
                            "{field_path}: decodes to {spellings:?}, sidecar says {want:?}"
                        ));
                    }
                }
            }
        }
    }

    if let Some(traits) = v
        .get("traits")
        .map(|t| t.as_array().expect("a checked array"))
    {
        for t in traits {
            check_trait(as_str(t), entity, &mut wrong);
        }
    }

    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} oracle mismatches:\n  {}",
            wrong.len(),
            wrong.join("\n  ")
        )
        .into())
    }
}

fn as_str(v: &Value) -> &str {
    v.as_str().expect("a checked string")
}

/// `+5`, `5` and ` 5 ` are one value, and case never distinguishes two.
fn normalize(s: &str) -> String {
    s.trim().trim_start_matches('+').to_ascii_lowercase()
}

/// A value as a number, if it is one: `0x…` as hex, anything else as decimal.
fn number(s: &str) -> Option<f64> {
    if let Some(hex) = s.strip_prefix("0x") {
        return u64::from_str_radix(hex, 16).ok().map(|v| v as f64);
    }
    s.parse().ok()
}

/// A sidecar value matches if it equals any of the field's spellings, or —
/// where both sides read as numbers — sits within `slack` of one (exactly on
/// it, when no slack is given).
fn matches(want: &str, spellings: &[String], slack: Option<f64>) -> bool {
    let want = normalize(want);
    if spellings.iter().any(|s| normalize(s) == want) {
        return true;
    }
    let Some(a) = number(&want) else {
        return false;
    };
    let tolerance = slack.unwrap_or(0.0);
    spellings
        .iter()
        .filter_map(|s| number(&normalize(s)))
        .any(|b| (a - b).abs() <= tolerance)
}

/// The trait vocabulary, one checker per name. A trait this reader does not
/// know is an error, so the vocabulary cannot drift.
fn check_trait(name: &str, entity: &Entity, wrong: &mut Vec<String>) {
    match name {
        // B3+bass preset 1 must read its separate bass drawbars, not stale nibbles.
        "b3_bass_manual" => {
            let organ = match entity {
                Entity::Program(Program::Electro5(p)) | Entity::Live(Live::Electro5(p)) => {
                    &p.organ_panel
                }
                _ => {
                    wrong.push("b3_bass_manual on a non-Electro-5 entity".into());
                    return;
                }
            };
            let bass = organ.b3_bass_drawbars();
            let main = organ.drawbars(OrganModel::B3, Preset::One);
            if bass != [0, 0] && [main[0], main[1]] == bass {
                wrong.push(
                    "bass drawbars also appear in the main block's shadow nibbles — \
                     the accessor may be reading the wrong place"
                        .into(),
                );
            }
        }
        // The zone key ranges were moved by hand, so they must *not* be the
        // layout the root keys imply.
        "zone_top_notes_overridden" => {
            let Entity::Sample(Sample::V2(s)) = entity else {
                wrong.push("zone_top_notes_overridden on a non-sample entity".into());
                return;
            };
            let (roots, stored) = match (s.strokes(), s.zones()) {
                (Ok(strokes), Ok(zones)) => (
                    strokes.iter().map(|s| s.root_key).collect::<Vec<u8>>(),
                    zones.iter().map(|z| z.top_note).collect::<Vec<u8>>(),
                ),
                _ => {
                    wrong.push("zone layout unreadable".into());
                    return;
                }
            };
            if stored == nsmp::zone::derive_top_notes(&roots) {
                wrong.push("listed as hand-edited but matches the default layout".into());
            }
        }
        other => wrong.push(format!("unknown trait {other:?}")),
    }
}
