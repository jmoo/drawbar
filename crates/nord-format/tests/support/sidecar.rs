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

/// Parse a sidecar, refusing an unknown schema or vocabulary rather than
/// skipping it.
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
    if object.get("unoracled").is_some() && object.get("fields").is_some() {
        return Err("unoracled beside fields — the two are mutually exclusive".into());
    }
    Ok(value)
}
