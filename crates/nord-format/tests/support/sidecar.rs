//! Reading `<specimen>.oracle.json`: the sidecar vocabulary and the validation
//! every corpus suite applies before trusting a sidecar. A sidecar that fails
//! validation is an error, never skipped.
//!
//! A path a sidecar names is relative to the specimen's directory.
//!
//! ⚠️ Not a test target. Each test target that includes this module compiles its
//! own copy.
#![allow(dead_code)]

use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// What one sidecar states about its specimen.
pub struct Sidecar {
    /// The specimen pins nothing yet.
    pub unoracled: bool,
    /// Field paths and the values they decode to.
    pub fields: Vec<(String, Expectation)>,
    /// Named claims, each with its own checker.
    pub traits: Vec<String>,
    /// A file with the same bytes.
    pub same_body_as: Option<String>,
    /// The Sample Editor project the instrument was rendered from.
    pub source: Option<String>,
    /// The same source rendered in the wide generations.
    pub wide_renders: Vec<String>,
    /// The instrument this one was saved from after a rename and zone remap.
    pub edited_from: Option<String>,
    /// An instrument whose decoded audio this one's must differ from.
    pub audio_differs_from: Option<String>,
    /// What the instrument's first stroke was rendered from.
    pub render: Option<Render>,
    /// Where each channel of a stereo first stroke has an impulse.
    pub impulses: Option<Impulses>,
    /// The piano library whose decoded strokes this one was built from by rules.
    pub recordings_from: Option<String>,
    /// Text the reader's refusal of the file must contain.
    pub refusal: Option<String>,
}

/// A field expectation: exact, or within `slack` when both sides read as numbers.
pub struct Expectation {
    pub value: String,
    pub slack: Option<f64>,
}

/// The source of a rendered first stroke: `frames` frames per channel and the
/// project's secondary start, in frames from the stroke's start. A `silent` source is
/// digital silence, so this crate's render must reproduce the file, except at the
/// offsets in `differs_at`, where it must differ.
pub struct Render {
    pub frames: usize,
    pub channels: u16,
    pub secondary_start: f64,
    pub silent: bool,
    pub differs_at: Vec<usize>,
}

/// Source frame positions of the impulses in each channel.
pub struct Impulses {
    pub left: Vec<usize>,
    pub right: Vec<usize>,
}

/// The keys a specimen sidecar may carry. An unknown key is an error, so the
/// vocabulary cannot drift apart between the corpus and this reader.
pub const SPECIMEN_KEYS: &[&str] = &[
    "audio_differs_from",
    "edited_from",
    "fields",
    "impulses",
    "note",
    "recordings_from",
    "refusal",
    "render",
    "same_body_as",
    "schema",
    "source",
    "traits",
    "unoracled",
    "wide_renders",
];

/// The keys `unoracled` contradicts: a sidecar either states what the specimen
/// pins or states that it pins nothing.
const CLAIMS: &[&str] = &[
    "audio_differs_from",
    "edited_from",
    "fields",
    "impulses",
    "recordings_from",
    "refusal",
    "render",
    "same_body_as",
    "source",
    "traits",
    "wide_renders",
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

/// The sidecar beside `specimen`, if it has one.
pub fn of(specimen: &Path) -> Result<Option<Sidecar>, String> {
    let path = sidecar_of(specimen);
    if path.exists() {
        load(&path).map(Some)
    } else {
        Ok(None)
    }
}

/// Parse a sidecar. An unknown schema, an unknown key, a value of the wrong type,
/// or a claim beside `unoracled` is an error.
pub fn load(path: &Path) -> Result<Sidecar, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("sidecar: {e}"))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("sidecar: {e}"))?;
    let object = value.as_object().ok_or("sidecar is not an object")?;
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err("sidecar schema is not 1".into());
    }
    if let Some(unknown) = object.keys().find(|k| !SPECIMEN_KEYS.contains(&k.as_str())) {
        return Err(format!("unknown sidecar key {unknown:?}"));
    }
    if object.contains_key("unoracled") {
        if let Some(claim) = CLAIMS.iter().find(|key| object.contains_key(**key)) {
            return Err(format!(
                "unoracled beside {claim}; the two are mutually exclusive"
            ));
        }
    }
    string(object, "note")?;
    Ok(Sidecar {
        unoracled: object.contains_key("unoracled"),
        fields: match object.get("fields") {
            None => Vec::new(),
            Some(fields) => fields
                .as_object()
                .ok_or("fields is not an object")?
                .iter()
                .map(|(path, expected)| {
                    expectation(expected)
                        .map(|e| (path.clone(), e))
                        .map_err(|e| format!("fields.{path}: {e}"))
                })
                .collect::<Result<_, _>>()?,
        },
        traits: strings(object, "traits")?,
        same_body_as: string(object, "same_body_as")?,
        source: string(object, "source")?,
        wide_renders: strings(object, "wide_renders")?,
        edited_from: string(object, "edited_from")?,
        audio_differs_from: string(object, "audio_differs_from")?,
        render: object.get("render").map(render).transpose()?,
        impulses: object.get("impulses").map(impulses).transpose()?,
        recordings_from: string(object, "recordings_from")?,
        refusal: string(object, "refusal")?,
    })
}

fn string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    object
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{key} is {value}, which is not a string"))
        })
        .transpose()
}

fn strings(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or_else(|| format!("{key} is not an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{key} holds {item}, which is not a string"))
        })
        .collect()
}

fn indices(object: &Map<String, Value>, key: &str, within: &str) -> Result<Vec<usize>, String> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or_else(|| format!("{within}.{key} is not an array"))?
        .iter()
        .map(|item| {
            item.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| format!("{within}.{key} holds {item}, which is not an index"))
        })
        .collect()
}

/// An object holding only `keys`.
fn only<'a>(value: &'a Value, key: &str, keys: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{key} is not an object"))?;
    match object.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(unknown) => Err(format!("unknown key {key}.{unknown}")),
        None => Ok(object),
    }
}

fn render(value: &Value) -> Result<Render, String> {
    let object = only(
        value,
        "render",
        &[
            "channels",
            "differs_at",
            "frames",
            "secondary_start",
            "silent",
        ],
    )?;
    let count = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("render.{key} is not a count"))
    };
    let silent = match object.get("silent") {
        None => false,
        Some(value) => value.as_bool().ok_or("render.silent is not a boolean")?,
    };
    let differs_at = indices(object, "differs_at", "render")?;
    if !silent && !differs_at.is_empty() {
        return Err("render.differs_at without render.silent".into());
    }
    Ok(Render {
        frames: usize::try_from(count("frames")?).map_err(|e| format!("render.frames: {e}"))?,
        channels: match count("channels")? {
            1 => 1,
            2 => 2,
            other => return Err(format!("render.channels is {other}, not 1 or 2")),
        },
        secondary_start: object
            .get("secondary_start")
            .and_then(Value::as_f64)
            .ok_or("render.secondary_start is not a number")?,
        silent,
        differs_at,
    })
}

fn impulses(value: &Value) -> Result<Impulses, String> {
    let object = only(value, "impulses", &["left", "right"])?;
    Ok(Impulses {
        left: indices(object, "left", "impulses")?,
        right: indices(object, "right", "impulses")?,
    })
}

/// A field expectation: a bare string is exact, an object is `{value, slack}`
/// with both sides read as numbers.
pub fn expectation(v: &Value) -> Result<Expectation, String> {
    match v {
        Value::String(s) => Ok(Expectation {
            value: s.clone(),
            slack: None,
        }),
        Value::Object(o) => {
            let value = o
                .get("value")
                .and_then(Value::as_str)
                .ok_or("expectation object without a string value")?;
            let slack = o
                .get("slack")
                .and_then(Value::as_f64)
                .ok_or("expectation object without a numeric slack")?;
            Ok(Expectation {
                value: value.to_string(),
                slack: Some(slack),
            })
        }
        other => Err(format!("unreadable expectation {other}")),
    }
}
