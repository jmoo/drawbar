//! Reading `<specimen>.oracle.json`: the sidecar vocabulary and the validation
//! every corpus suite applies before trusting one. A sidecar that fails here is
//! refused, never skipped.
//!
//! ⚠️ A rustc-visible support module, not a test target — each test target that
//! includes it compiles its own copy.
#![allow(dead_code)]

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// The keys a specimen sidecar may carry. An unknown key is an error, so the
/// vocabulary cannot drift apart between the corpus and this reader.
pub const SPECIMEN_KEYS: &[&str] = &[
    "fields",
    "note",
    "same_body_as",
    "schema",
    "traits",
    "unoracled",
];

pub fn specimen_of(sidecar: &Path) -> PathBuf {
    let name = sidecar.file_name().unwrap().to_string_lossy();
    sidecar.with_file_name(name.trim_end_matches(".oracle.json"))
}

pub fn sidecar_of(specimen: &Path) -> PathBuf {
    let mut name = specimen.file_name().unwrap().to_os_string();
    name.push(".oracle.json");
    specimen.with_file_name(name)
}

/// The keys `unoracled` contradicts: a sidecar either states what the specimen
/// pins or states that it pins nothing.
const CLAIMS: &[&str] = &["fields", "same_body_as", "traits"];

/// Parse a sidecar, refusing an unknown schema, an unknown key, a key whose value
/// has the wrong type, or a claim beside `unoracled` — rather than skipping it. A
/// reader may take any present key at its declared type without re-checking.
pub fn load(path: &Path, allowed: &[&str]) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("sidecar: {e}"))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("sidecar: {e}"))?;
    let object = value.as_object().ok_or("sidecar is not an object")?;
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err("sidecar schema is not 1 — refusing rather than skipping".into());
    }
    if let Some(unknown) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("unknown sidecar key {unknown:?}"));
    }
    if let Some(fields) = object.get("fields") {
        let fields = fields.as_object().ok_or("fields is not an object")?;
        for (path, expected) in fields {
            expectation(expected).map_err(|e| format!("fields.{path}: {e}"))?;
        }
    }
    if let Some(traits) = object.get("traits") {
        let traits = traits.as_array().ok_or("traits is not an array")?;
        for name in traits {
            name.as_str()
                .ok_or_else(|| format!("traits holds {name}, which is not a string"))?;
        }
    }
    for key in ["same_body_as", "note"] {
        if let Some(value) = object.get(key) {
            value
                .as_str()
                .ok_or_else(|| format!("{key} is {value}, which is not a string"))?;
        }
    }
    if object.contains_key("unoracled") {
        if let Some(claim) = CLAIMS.iter().find(|key| object.contains_key(**key)) {
            return Err(format!(
                "unoracled beside {claim} — the two are mutually exclusive"
            ));
        }
    }
    Ok(value)
}

/// A field expectation: a bare string is exact, an object is `{value, slack}`
/// with both sides read as numbers.
pub fn expectation(v: &Value) -> Result<(String, Option<f64>), String> {
    match v {
        Value::String(s) => Ok((s.clone(), None)),
        Value::Object(o) => {
            let value = o
                .get("value")
                .and_then(Value::as_str)
                .ok_or("expectation object without a string value")?;
            let slack = o
                .get("slack")
                .and_then(Value::as_f64)
                .ok_or("expectation object without a numeric slack")?;
            Ok((value.to_string(), Some(slack)))
        }
        other => Err(format!("unreadable expectation {other}")),
    }
}
