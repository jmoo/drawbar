//! The contracts of the support modules the corpus suites read a specimen tree
//! through.
//!
//! `tests/support/` is not a test target, so a `#[test]` there would run only
//! where a corpus-gated suite includes it. This target compiles the modules in
//! every build.

#[path = "support/scan.rs"]
mod scan;
#[path = "support/sidecar.rs"]
mod sidecar;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A new directory for each call. ⚠️ The process id alone is not unique: these tests
/// run on parallel threads, and two that shared a directory would delete each other's
/// files.
fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nord-support-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn a_specimen_that_does_not_parse_is_collected_and_the_rest_are_read() {
    let dir = scratch("unparsed");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cbin/npsy.g0.cbin");
    fs::copy(&fixture, dir.join("readable.cbin")).unwrap();
    // A CBIN container with a tag no reader claims: the sniffer takes the file,
    // and `from_stream` refuses it.
    fs::write(dir.join("garbage.cbin"), b"CBIN\0\0\0\0zzzz\0\0\0\0").unwrap();

    let tree = scan::read_tree(&dir);
    fs::remove_dir_all(&dir).unwrap();

    assert_eq!(
        tree.specimens
            .iter()
            .map(|s| name(&s.path))
            .collect::<Vec<_>>(),
        ["readable.cbin"],
        "parsed specimens"
    );
    assert_eq!(
        tree.unparsed
            .iter()
            .map(|(p, _)| name(p))
            .collect::<Vec<_>>(),
        ["garbage.cbin"],
        "unparsed files"
    );
    assert!(
        tree.unparsed[0].1.contains("zzzz"),
        "the unparsed file's error does not name its tag: {:?}",
        tree.unparsed[0].1
    );
}

/// Load `json` as a sidecar, cleaning up the file it had to be written to.
fn load(json: &str) -> Result<(), String> {
    let dir = scratch("sidecar");
    let path = dir.join("specimen.nsmp.oracle.json");
    fs::write(&path, json).unwrap();
    let result = sidecar::load(&path, sidecar::SPECIMEN_KEYS).map(|_| ());
    fs::remove_dir_all(&dir).unwrap();
    result
}

#[test]
fn a_sidecar_value_of_the_wrong_type_is_refused() {
    for (json, key) in [
        (r#"{"schema":1,"fields":["transpose"]}"#, "fields"),
        (
            r#"{"schema":1,"fields":{"transpose":-3}}"#,
            "fields.transpose",
        ),
        (
            r#"{"schema":1,"fields":{"transpose":{"value":-3,"slack":0.1}}}"#,
            "fields.transpose",
        ),
        (
            r#"{"schema":1,"fields":{"transpose":{"value":"-3","slack":"0.1"}}}"#,
            "fields.transpose",
        ),
        (r#"{"schema":1,"traits":"b3_bass_manual"}"#, "traits"),
        (r#"{"schema":1,"traits":[7]}"#, "traits"),
        (r#"{"schema":1,"same_body_as":7}"#, "same_body_as"),
        (r#"{"schema":1,"note":["a"]}"#, "note"),
    ] {
        let error = load(json).expect_err(&format!("{json} loaded"));
        assert!(
            error.contains(key),
            "{json} was refused as {error:?}, which does not name {key}"
        );
    }
}

#[test]
fn unoracled_beside_a_claim_is_refused() {
    for (json, claim) in [
        (
            r#"{"schema":1,"unoracled":true,"fields":{"transpose":"-3"}}"#,
            "fields",
        ),
        (
            r#"{"schema":1,"unoracled":true,"traits":["b3_bass_manual"]}"#,
            "traits",
        ),
        (
            r#"{"schema":1,"unoracled":true,"same_body_as":"other.ne5p"}"#,
            "same_body_as",
        ),
    ] {
        let error = load(json).expect_err(&format!("{json} loaded"));
        assert!(
            error.contains(claim),
            "{json} was refused as {error:?}, which does not name {claim}"
        );
    }
}

#[test]
fn a_sidecar_stating_every_key_at_its_declared_type_loads() {
    let json = r#"{
        "schema": 1,
        "note": "a hand-edited zone layout",
        "same_body_as": "other.nsmp",
        "fields": {"transpose": "-3", "gain": {"value": "3.4", "slack": 0.05}},
        "traits": ["zone_top_notes_overridden"]
    }"#;
    load(json).expect("a sidecar with every key");
    load(r#"{"schema":1,"unoracled":true,"note":"no capture yet"}"#)
        .expect("unoracled alone, with only a note beside it");
}

#[test]
fn an_unknown_schema_or_key_is_refused() {
    assert!(load(r#"{"schema":2,"note":"x"}"#)
        .expect_err("schema 2 loaded")
        .contains("schema"));
    assert!(load(r#"{"schema":1,"feilds":{}}"#)
        .expect_err("an unknown key loaded")
        .contains("feilds"));
}
