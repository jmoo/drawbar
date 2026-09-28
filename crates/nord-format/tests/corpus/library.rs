//! Where `library.json`, the index a corpus tree may carry at its root, projects
//! the R2 objects the tree's git tier does not hold. A tree without the index
//! projects nothing.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// The projected paths of `root`'s index, relative to `root`.
pub fn projected(root: &Path) -> Result<BTreeSet<String>, String> {
    let path = root.join("library.json");
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("library.json: {e}"))?;
    let index: Value = serde_json::from_str(&text).map_err(|e| format!("library.json: {e}"))?;
    projections(&index)
}

/// An entry with a `path` projects there and any other to `<ext>/<filename>`.
/// Paths that differ only in ASCII case each take their sha256's first eight hex
/// digits ahead of the extension, as the corpus assembly writes them. An entry
/// the git tier holds (`in_git`) is left out.
pub fn projections(index: &Value) -> Result<BTreeSet<String>, String> {
    let entries = index
        .get("files")
        .and_then(Value::as_array)
        .ok_or("library.json has no files array")?
        .iter()
        .map(entry)
        .collect::<Result<Vec<_>, _>>()?;
    let mut folded: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        *folded.entry(e.path.to_ascii_lowercase()).or_default() += 1;
    }
    Ok(entries
        .iter()
        .filter(|e| !e.in_git)
        .map(|e| match folded[&e.path.to_ascii_lowercase()] {
            1 => e.path.clone(),
            _ => with_suffix(&e.path, &e.sha256[..8]),
        })
        .collect())
}

struct Entry {
    path: String,
    sha256: String,
    in_git: bool,
}

fn entry(value: &Value) -> Result<Entry, String> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("library.json entry without a string {key}: {value}"))
    };
    let path = match value.get("path") {
        Some(_) => text("path")?.to_string(),
        None => format!("{}/{}", text("ext")?, text("filename")?),
    };
    let sha256 = text("sha256")?;
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("library.json entry {path:?} has sha256 {sha256:?}"));
    }
    Ok(Entry {
        path,
        sha256: sha256.to_string(),
        in_git: value
            .get("in_git")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn with_suffix(path: &str, tag: &str) -> String {
    match path.rfind('.') {
        Some(dot) if !path[dot..].contains('/') => {
            format!("{}-{tag}{}", &path[..dot], &path[dot..])
        }
        _ => format!("{path}-{tag}"),
    }
}
