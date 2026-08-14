//! Oracle-sidecar checking: `<specimen>.oracle.json` is the corpus's
//! machine-readable record of what a differential capture pinned, and this is
//! the reader the corpus README promises — a specimen joins the sweep by
//! gaining a sidecar there, not by anyone adding a case here.

use crate::lookup;
use libtest_mimic::Failed;
use nord_format::formats::ne5::OrganModel;
use nord_format::formats::nsmp;
use nord_format::{Entity, Live, Program, Sample};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// The keys a specimen sidecar may carry. An unknown key is an error, so the
/// vocabulary cannot drift apart between the corpus and this reader.
const SPECIMEN_KEYS: &[&str] = &[
    "fields",
    "note",
    "same_body_as",
    "schema",
    "traits",
    "unoracled",
];

/// The keys `dir.oracle.json` may carry.
const DIR_KEYS: &[&str] = &[
    "dependencies",
    "note",
    "piano_categories",
    "schema",
    "unoracled",
];

pub fn specimen_of(sidecar: &Path) -> PathBuf {
    let name = sidecar.file_name().unwrap().to_string_lossy();
    sidecar.with_file_name(name.trim_end_matches(".oracle.json").to_string())
}

fn sidecar_of(specimen: &Path) -> PathBuf {
    let mut name = specimen.file_name().unwrap().to_os_string();
    name.push(".oracle.json");
    specimen.with_file_name(name)
}

/// Parse a sidecar, refusing an unknown schema or vocabulary rather than
/// skipping it.
fn load(path: &Path, allowed: &[&str]) -> Result<Value, Failed> {
    let text = fs::read_to_string(path).map_err(|e| Failed::from(format!("sidecar: {e}")))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| Failed::from(format!("sidecar: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Failed::from("sidecar is not an object"))?;
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err("sidecar schema is not 1 — refusing rather than skipping".into());
    }
    if let Some(unknown) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("unknown sidecar key {unknown:?}").into());
    }
    if object.get("unoracled").is_some() && object.get("fields").is_some() {
        return Err("unoracled beside fields — the two are mutually exclusive".into());
    }
    Ok(value)
}

/// Does this path owe the corpus an oracle? Differential trees — a model's
/// `programs/`, `settings/` and `samples/` — must say something about every
/// specimen; factory material and captures carry no filename oracle at all.
fn oracle_required(root: &Path, specimen: &Path) -> bool {
    let Ok(rel) = specimen.strip_prefix(root) else {
        return false;
    };
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.len() >= 2
        && matches!(parts[1].as_str(), "programs" | "settings" | "samples")
        && !parts.iter().any(|p| p == "factory")
}

/// Does the specimen's directory answer for it with an `unoracled` block?
fn dir_unoracled(specimen: &Path) -> Result<bool, Failed> {
    let dir_sidecar = specimen.parent().unwrap().join("dir.oracle.json");
    if !dir_sidecar.exists() {
        return Ok(false);
    }
    Ok(load(&dir_sidecar, DIR_KEYS)?.get("unoracled").is_some())
}

/// Check one specimen against its sidecar — or against the rule that the
/// absence of an oracle has to be a decision, not an omission.
pub fn check_specimen(
    root: &Path,
    path: &Path,
    bytes: &[u8],
    entity: &Entity,
) -> Result<(), Failed> {
    let sidecar = sidecar_of(path);
    if !sidecar.exists() {
        if oracle_required(root, path) && !dir_unoracled(path)? {
            return Err(
                "differential specimen with neither a sidecar nor an unoracled reason — \
                 the suite would stay green having checked nothing"
                    .into(),
            );
        }
        return Ok(());
    }

    let v = load(&sidecar, SPECIMEN_KEYS)?;
    if v.get("unoracled").is_some() {
        return Ok(());
    }

    let mut wrong: Vec<String> = Vec::new();

    if let Some(sibling) = v.get("same_body_as").and_then(Value::as_str) {
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

    if let Some(fields) = v.get("fields").and_then(Value::as_object) {
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

    if let Some(traits) = v.get("traits").and_then(Value::as_array) {
        for t in traits {
            let name = t.as_str().unwrap_or_default();
            check_trait(name, entity, &mut wrong);
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

/// A field expectation: a bare string is exact, an object is `{value, slack}`
/// with both sides read as numbers.
fn expectation(v: &Value) -> Result<(String, Option<f64>), String> {
    match v {
        Value::String(s) => Ok((s.clone(), None)),
        Value::Object(o) => {
            let value = o
                .get("value")
                .and_then(Value::as_str)
                .ok_or("expectation object without a value")?;
            let slack = o
                .get("slack")
                .and_then(Value::as_f64)
                .ok_or("expectation object without a slack")?;
            Ok((value.to_string(), Some(slack)))
        }
        other => Err(format!("unreadable expectation {other}")),
    }
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
pub fn matches(want: &str, spellings: &[String], slack: Option<f64>) -> bool {
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
        // b3+bass preset 1 keeps its two bass drawbars outside the nine-nibble
        // block, and the nibbles they shadow there hold stale leftovers — so
        // the accessor genuinely reading elsewhere is the claim to check.
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
            let main = organ.drawbars(OrganModel::B3, 1);
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

/// Check a `dir.oracle.json`: validate it, and hold any `dependencies` table it
/// carries against every specimen in its directory.
pub fn check_dir_sidecar(dir: &Path, sidecar: &Path) -> Result<(), Failed> {
    let v = load(sidecar, DIR_KEYS)?;
    // `unoracled` is applied per-specimen and `piano_categories` is read by the
    // consumers that need the map; only `dependencies` asserts here.
    let Some(dep) = v.get("dependencies") else {
        return Ok(());
    };
    check_dependencies(dir, dep)
}

/// A golden id table: `field` names the path holding a library id, `keyed_by`
/// the paths forming its slot coordinate, and every specimen in the directory
/// must land on exactly one row and carry that row's id. Every row must be
/// covered, so the table cannot outlive the specimens it was written for.
fn check_dependencies(dir: &Path, dep: &Value) -> Result<(), Failed> {
    let field = dep
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| Failed::from("dependencies without a field"))?;
    let keyed_by: Vec<&str> = dep
        .get("keyed_by")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .ok_or_else(|| Failed::from("dependencies without keyed_by"))?;
    let table = dep
        .get("table")
        .and_then(Value::as_array)
        .ok_or_else(|| Failed::from("dependencies without a table"))?;

    struct Row {
        key: Vec<String>,
        id: String,
        hits: usize,
    }
    let mut rows: Vec<Row> = Vec::new();
    for row in table {
        rows.push(Row {
            key: row
                .get("key")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .ok_or_else(|| Failed::from("table row without a key"))?,
            id: row
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| Failed::from("table row without an id"))?
                .to_string(),
            hits: 0,
        });
    }

    let mut wrong: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for entry in fs::read_dir(dir).map_err(|e| Failed::from(e.to_string()))? {
        let path = entry.unwrap().path();
        if !path.is_file() || !crate::wanted(&path) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let entity = match nord_format::from_path(&path) {
            Ok(e) => e,
            Err(e) => {
                // The parse itself is the specimen trial's problem; here it
                // only ends this file's part in the table.
                wrong.push(format!("{name}: {e}"));
                continue;
            }
        };
        let key: Vec<Vec<String>> = keyed_by
            .iter()
            .map(|p| lookup::lookup(&entity, p))
            .collect::<Result<_, _>>()
            .map_err(Failed::from)?;
        let id = lookup::lookup(&entity, field).map_err(Failed::from)?;

        let row = rows.iter_mut().find(|r| {
            r.key.len() == key.len()
                && r.key
                    .iter()
                    .zip(&key)
                    .all(|(want, spellings)| matches(want, spellings, None))
        });
        match row {
            None => wrong.push(format!("{name}: key {key:?} has no table row")),
            Some(row) => {
                if !matches(&row.id, &id, None) {
                    wrong.push(format!(
                        "{name}: {field} decodes to {id:?}, table says {}",
                        row.id
                    ));
                }
                row.hits += 1;
            }
        }
        checked += 1;
    }

    if checked == 0 {
        wrong.push("no specimens beside the table — is the corpus present?".into());
    }
    for row in &rows {
        if row.hits == 0 {
            wrong.push(format!(
                "table row {:?} is covered by no specimen — the table has outlived it",
                row.key
            ));
        }
    }

    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!("{}:\n  {}", wrong.len(), wrong.join("\n  ")).into())
    }
}
