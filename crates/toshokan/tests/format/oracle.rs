//! What a reader shows of a library, as the facts a state's `view.oracle.json`
//! lists, and how a difference is reported: by the fact that changed.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use toshokan::report::{By, ItemKind, Mode, Opened, Start, What};
use toshokan::{Field, FileState, MemDisk, Root};

use crate::fixture::{fixtures, Library, NOTE, ORIGIN, TAGS};

fn register<T: serde::Serialize>(field: Field<T>) -> Option<Value> {
    match field {
        Field::Unset => None,
        Field::Value(value) => Some(json!(value)),
        Field::Conflict { all, .. } => {
            let values: Vec<&T> = all.iter().map(|written| &written.value).collect();
            Some(json!({ "conflict": values }))
        }
        Field::Unreadable(raw) => Some(json!({ "unreadable": raw.as_str() })),
    }
}

fn mode(mode: &Mode) -> String {
    match mode {
        Mode::Writable => "writable".into(),
        Mode::ReadOnly(why) => format!("read-only: {why}"),
    }
}

fn start(start: &Start) -> String {
    match start {
        Start::Resumed(writer) => format!("resumed {writer}"),
        Start::New => "new".into(),
        Start::Rekeyed { old, why } => format!("rekeyed {old}: {why:?}"),
    }
}

fn file_state(state: FileState) -> &'static str {
    match state {
        FileState::InSync => "in-sync",
        FileState::ChangedOutside => "changed-outside",
        FileState::Missing => "missing",
        FileState::Unscanned => "unscanned",
    }
}

/// The facts the library shows, after a scan of every file, with what the folder
/// holds at each path they name.
pub fn observed(library: &Library, opened: &Opened, folder: &MemDisk) -> Value {
    let files = folder.files(Root::Folder);
    let bytes = |path: &toshokan::RelPath| {
        let text = files
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned());
        json!(text)
    };
    let view = library.view();
    let mut entities = Map::new();
    for entity in view.entities() {
        let mut facts = Map::new();
        if let Some(origin) = register(entity.get(ORIGIN)) {
            facts.insert("origin".into(), origin);
        }
        if let Some(note) = register(entity.get(NOTE)) {
            facts.insert("note".into(), note);
        }
        let tags = entity.members(TAGS);
        if !tags.values.is_empty() {
            facts.insert("tags".into(), json!(tags.values));
        }
        if let Some(file) = entity.file() {
            let file = json!({
                "path": file.path.as_str(),
                "state": file_state(file.state),
                "bytes": bytes(&file.path),
            });
            facts.insert("file".into(), file);
        }
        if entity.deletion_conflicted() {
            facts.insert("deletion-conflict".into(), json!(true));
        }
        entities.insert(entity.id().to_string(), Value::Object(facts));
    }
    let writers: BTreeMap<String, String> = view
        .writers()
        .iter()
        .map(|w| (w.writer.to_string(), w.label.clone()))
        .collect();
    let unbound: Vec<&str> = view.unbound().map(|p| p.as_str()).collect();
    let items: Vec<Value> = view
        .items()
        .iter()
        .map(|item| {
            let kind = match item.kind {
                ItemKind::Run => "run",
                ItemKind::Gap => "gap",
                ItemKind::Unplaced => "unplaced",
            };
            let changes: Vec<String> = item
                .changes
                .iter()
                .map(|change| {
                    let what = match &change.what {
                        What::Created => "created".into(),
                        What::Deleted => "deleted".into(),
                        What::Field(key) => key.clone(),
                        What::File => "file".into(),
                    };
                    format!("{} {what}", change.entity)
                })
                .collect();
            let by = match &item.by {
                By::This => "this".into(),
                By::Writer { label, .. } => label.clone(),
                By::Outside => "outside".into(),
            };
            json!({
                "kind": kind,
                "run": item.run,
                "by": by,
                "entries": item.entries,
                "changes": changes,
            })
        })
        .collect();
    let forks: Vec<Value> = view
        .forks()
        .iter()
        .map(|fork| json!({ "writer": fork.writer.to_string(), "at": fork.at }))
        .collect();
    let gaps: Vec<Value> = view
        .gaps()
        .iter()
        .map(|gap| json!({ "writer": gap.writer.to_string(), "held": gap.held }))
        .collect();
    let orphans: Vec<Value> = opened
        .orphaned
        .iter()
        .map(|orphan| {
            let paths: Vec<&str> = orphan.paths.iter().map(|p| p.as_str()).collect();
            json!({
                "writer": orphan.writer.to_string(),
                "record": orphan.record.to_string(),
                "label": orphan.label,
                "paths": paths,
            })
        })
        .collect();
    let paths = |paths: &[toshokan::RelPath]| -> Vec<String> {
        paths.iter().map(|p| p.as_str().to_owned()).collect()
    };
    let settled: Vec<[String; 2]> = view
        .folded()
        .settlements()
        .iter()
        .map(|(writer, record)| [writer.to_string(), record.to_string()])
        .collect();
    let mut trash = Vec::new();
    for writer in view.writers() {
        for item in view.folded().trash(writer.writer) {
            let at = crate::fixture::layout().trash(item.writer, item.item);
            trash.push(json!({
                "writer": item.writer.to_string(),
                "item": item.item.to_string(),
                "from": item.from.as_str(),
                "len": item.len,
                "bytes": bytes(&at),
            }));
        }
    }
    let by_text = |list: &mut Vec<Value>| list.sort_by_key(Value::to_string);
    let (mut items, mut orphans) = (items, orphans);
    for list in [&mut items, &mut orphans, &mut trash] {
        by_text(list);
    }
    json!({
        "mode": mode(library.mode()),
        "start": start(&opened.start),
        "writers": writers,
        "entities": entities,
        "unbound": unbound,
        "items": items,
        "forks": forks,
        "gaps": gaps,
        "orphans": orphans,
        "ignored": paths(&opened.ignored),
        "unreadable": paths(&opened.unreadable),
        "settled": settled,
        "trash": trash,
        "unknown": view.folded().unknown().count(),
        "extended": view.folded().extended().count(),
    })
}

/// Each fact on which `found` differs from `expected`, by its place.
pub fn differences(at: &str, expected: &Value, found: &Value) -> Vec<String> {
    match (expected, found) {
        (Value::Object(expected), Value::Object(found)) => {
            let mut names: Vec<&String> = expected.keys().chain(found.keys()).collect();
            names.sort();
            names.dedup();
            let member = |object: &Map<String, Value>, name: &str| object.get(name).cloned();
            names
                .into_iter()
                .flat_map(|name| {
                    let place = format!("{at}/{name}");
                    match (member(expected, name), member(found, name)) {
                        (Some(e), Some(f)) => differences(&place, &e, &f),
                        (Some(e), None) => vec![format!("{place}: expected {e}, found nothing")],
                        (None, Some(f)) => vec![format!("{place}: expected nothing, found {f}")],
                        (None, None) => Vec::new(),
                    }
                })
                .collect()
        }
        _ if expected == found => Vec::new(),
        _ => vec![format!("{at}: expected {expected}, found {found}")],
    }
}

/// The oracle `name` of state `state`.
pub fn oracle(state: &str, name: &str) -> Value {
    let at = fixtures().join(state).join(name);
    let text = std::fs::read(&at).unwrap_or_else(|e| panic!("{}: {e}", at.display()));
    serde_json::from_slice(&text).unwrap_or_else(|e| panic!("{}: {e}", at.display()))
}
