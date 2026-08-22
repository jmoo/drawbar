//! The committed synthetic specimens: files this crate's own writers produced,
//! with oracle sidecars saying what was set, so the sweep in `tests/corpus`
//! reads them like any corpus tree — in any checkout, with no corpus.
//!
//! The one test here pins the writer: the files are golden, so an encoding
//! change shows up as a byte diff in git rather than silently moving with the
//! code. Regenerate after an intentional writer change, and **read the diff**:
//!
//! ```sh
//! UPDATE_FIXTURES=1 cargo test -p nord-format --test fixtures
//! ```

#[path = "support/format_table.rs"]
mod format_table;

use format_table::formats;
use nord_format::cbin::{Cbin, Generation, Header, RawBody};
use nord_format::formats::ne5::{self, EqualizerPart};
use nord_format::formats::nsmpproj::{self, NewZone};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn to_bytes<B>(file: &Cbin<B>) -> Vec<u8>
where
    Cbin<B>: WriteTo,
{
    let mut out = Vec::new();
    file.write_to_cursor(&mut out);
    out
}

/// One bound for "any Cbin the fixture set writes" — the generated `write_to`
/// inherent methods are not trait-backed, so the trait lives here.
trait WriteTo {
    fn write_to_cursor(&self, out: &mut Vec<u8>);
}

macro_rules! write_to {
    ($($body:ty),*) => {$(
        impl WriteTo for Cbin<$body> {
            fn write_to_cursor(&self, out: &mut Vec<u8>) {
                self.write_to(&mut Cursor::new(out)).unwrap();
            }
        }
    )*};
}
write_to!(RawBody, ne5::Program, ne5::Settings, ne5::Song);

/// A sidecar pinning `fields`, in the corpus's vocabulary.
fn sidecar(fields: &[(&str, &str)]) -> Vec<u8> {
    let fields: BTreeMap<&str, &str> = fields.iter().copied().collect();
    let mut out = serde_json::to_vec_pretty(&json!({ "schema": 1, "fields": fields })).unwrap();
    out.push(b'\n');
    out
}

/// A mutated Electro 5 program: the file it lands in, the edit, and what the
/// sidecar then pins.
type Mutant = (
    &'static str,
    fn(&mut Cbin<ne5::Program>),
    &'static [(&'static str, &'static str)],
);

/// One mutation per panel, plus two fields that straddle a byte boundary.
fn mutants() -> Vec<Mutant> {
    vec![
        (
            "center-gain-96.ne5p",
            |p| p.center_panel.gain = 96u8.try_into().unwrap(),
            &[("center_panel.gain", "96")],
        ),
        (
            "piano-touch-3.ne5p",
            |p| p.piano_panel.touch = 3u8.try_into().unwrap(),
            &[("piano_panel.touch", "3")],
        ),
        (
            "sample-number-211.ne5p",
            |p| p.sample_panel.number = 211,
            &[("sample_panel.number", "211")],
        ),
        (
            "organ-perc-third.ne5p",
            |p| p.organ_panel.set_b3_perc_third(true),
            &[("organ_panel.b3_perc_third", "true")],
        ),
        (
            "fx3-compression-101.ne5p",
            |p| p.effects_panel.fx3_compression = 101u8.try_into().unwrap(),
            &[("effects_panel.fx3_compression", "101")],
        ),
        (
            // Three bits in 0x9a, four in 0x9b.
            "eq-freq-gain-85.ne5p",
            |p| p.effects_panel.equalizer_freq_gain = 85u8.try_into().unwrap(),
            &[("effects_panel.equalizer_freq_gain", "85")],
        ),
        (
            // Five bits in 0x9e, two in 0x9f.
            "fx5-moisture-42.ne5p",
            |p| p.effects_panel.fx5_moisture = 42u8.try_into().unwrap(),
            &[("effects_panel.fx5_moisture", "42")],
        ),
        (
            "eq-part-both.ne5p",
            |p| p.effects_panel.equalizer_part = EqualizerPart::Both,
            &[("effects_panel.equalizer_part", "Both")],
        ),
    ]
}

fn default_program() -> Cbin<ne5::Program> {
    ne5::program::new((0, 0).try_into().unwrap())
}

/// Every fixture the suite owns, as `relative path -> bytes`, written entirely
/// by this crate's own writers.
fn synthesized() -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut put = |name: String, bytes: Vec<u8>| {
        assert!(
            out.insert(name.clone(), bytes).is_none(),
            "two fixtures land on {name}"
        );
    };

    // Every registered CBIN tag, both header generations, zero body: the
    // container is the specimen.
    for (tag, body_len, version) in formats() {
        for generation in [Generation::V0, Generation::V1] {
            let mut header = Header::new(tag, (0, 0), version);
            header.generation = generation;
            let file = Cbin {
                header,
                body: RawBody(vec![0u8; body_len as usize]),
            };
            let g = match generation {
                Generation::V0 => 0,
                Generation::V1 => 1,
            };
            put(
                format!("cbin/{}.g{g}.cbin", tag.trim_end_matches('\0')),
                to_bytes(&file),
            );
        }
    }

    // The decoded formats, through their constructors and setters, each edit
    // pinned by a sidecar.
    put("ne5/default.ne5p".into(), to_bytes(&default_program()));
    put(
        "ne5/default.ne5l".into(),
        to_bytes(&ne5::live::new((0, 0).try_into().unwrap())),
    );
    put("ne5/default.ne5s".into(), to_bytes(&ne5::settings::new()));
    {
        let mut settings = ne5::settings::new();
        settings.set_field("global_transpose", "-3").unwrap();
        put("ne5/transpose-minus-3.ne5s".into(), to_bytes(&settings));
        put(
            "ne5/transpose-minus-3.ne5s.oracle.json".into(),
            sidecar(&[("panel.global_transpose", "-3")]),
        );
    }
    {
        let slot = |x: u16, y: u16| (x, y).try_into().unwrap();
        let song = ne5::song::new(
            (0, 2).try_into().unwrap(),
            1,
            [slot(5, 9), slot(0, 1), slot(0, 2), slot(5, 8)],
        );
        put("ne5/song.ne5t".into(), to_bytes(&song));
        put(
            "ne5/song.ne5t.oracle.json".into(),
            sidecar(&[
                ("location", "(0, 2)"),
                ("programs", "[(5, 9), (0, 1), (0, 2), (5, 8)]"),
            ]),
        );
    }
    for (name, mutate, pinned) in mutants() {
        let mut program = default_program();
        mutate(&mut program);
        put(format!("ne5/{name}"), to_bytes(&program));
        put(format!("ne5/{name}.oracle.json"), sidecar(pinned));
    }

    // Sample Editor projects: the one-pass layout for one zone and for three,
    // and a three-zone project with its middle zone retuned and widened.
    {
        let zone = |path: &str, root_key| NewZone {
            path: format!("audio/{path}"),
            sample_rate: 44100,
            frames: 4394,
            root_key,
        };
        let one = nsmpproj::Project::new("one-zone", &[zone("c4.wav", 60)], 1_700_000_000).unwrap();
        put(
            "nsmpproj/one-zone.nsmpproj".into(),
            one.render().into_bytes(),
        );
        put(
            "nsmpproj/one-zone.nsmpproj.oracle.json".into(),
            sidecar(&[
                ("name", "one-zone"),
                ("version", "54"),
                ("root_keys", "[60]"),
                ("bottom_notes", "[17]"),
                ("top_notes", "[84]"),
                ("audio_files", "[\"audio/c4.wav\"]"),
            ]),
        );
        let three = || {
            nsmpproj::Project::new(
                "three-zones",
                &[zone("c3.wav", 48), zone("c4.wav", 60), zone("c5.wav", 72)],
                1_700_000_000,
            )
            .unwrap()
        };
        put(
            "nsmpproj/three-zones.nsmpproj".into(),
            three().render().into_bytes(),
        );
        put(
            "nsmpproj/three-zones.nsmpproj.oracle.json".into(),
            sidecar(&[
                ("name", "three-zones"),
                ("root_keys", "[72, 60, 48]"),
                ("bottom_notes", "[66, 54, 17]"),
                ("top_notes", "[96, 65, 53]"),
            ]),
        );
        let mut edited = three();
        edited.set_root_key(130, 61).unwrap();
        edited.set_key_range(130, 54, 70).unwrap();
        edited.set_key_range(131, 71, 96).unwrap();
        put(
            "nsmpproj/middle-zone-edited.nsmpproj".into(),
            edited.render().into_bytes(),
        );
        put(
            "nsmpproj/middle-zone-edited.nsmpproj.oracle.json".into(),
            sidecar(&[
                ("root_keys", "[72, 61, 48]"),
                ("bottom_notes", "[71, 54, 17]"),
                ("top_notes", "[96, 70, 53]"),
            ]),
        );
    }

    out
}

/// Every committed fixture file, as `relative path -> bytes`.
fn committed() -> BTreeMap<String, Vec<u8>> {
    let root = fixtures_dir();
    let mut out = BTreeMap::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n != "README.md" && !n.starts_with('.'))
            {
                let rel = path.strip_prefix(&root).unwrap().display().to_string();
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// The committed files are exactly what the writers write today — no drifted
/// bytes, no missing fixture, no stale leftover. `UPDATE_FIXTURES=1`
/// regenerates instead of comparing.
#[test]
fn committed_fixtures_are_what_the_writers_write() {
    let generated = synthesized();

    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        let root = fixtures_dir();
        for (name, _) in committed() {
            if !generated.contains_key(&name) {
                fs::remove_file(root.join(&name)).unwrap();
            }
        }
        for (name, bytes) in &generated {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
        }
        return;
    }

    let committed = committed();
    let mut wrong: Vec<String> = Vec::new();
    for (name, bytes) in &generated {
        match committed.get(name) {
            None => wrong.push(format!("{name}: not committed")),
            Some(disk) if disk != bytes => wrong.push(format!("{name}: bytes differ")),
            Some(_) => {}
        }
    }
    for name in committed.keys() {
        if !generated.contains_key(name) {
            wrong.push(format!("{name}: committed but no longer generated"));
        }
    }
    assert!(
        wrong.is_empty(),
        "fixtures out of step with the writers — regenerate with UPDATE_FIXTURES=1 \
         and read the diff:\n  {}",
        wrong.join("\n  ")
    );
}

/// The sidecars the fixtures carry are valid oracles in the corpus's vocabulary.
#[test]
fn fixture_sidecars_parse() {
    for (name, bytes) in synthesized() {
        if name.ends_with(".oracle.json") {
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["schema"], 1, "{name}");
            assert!(v["fields"].is_object(), "{name}");
        }
    }
}
