//! The committed synthetic baseline: specimen files written by this crate's own
//! writers, checked in so a round trip against real bytes on disk runs
//! anywhere — no corpus, no hardware, no vendor material.
//!
//! Two jobs. [`committed_fixtures_are_what_the_writers_write`] pins the writer:
//! the files are golden, so an encoding change shows up as a byte diff in git
//! rather than silently moving with the code. The rest of the suite exercises
//! the read side against them — parse, classify, byte-exact round trip, and
//! the decode of every deliberately-set field.
//!
//! Regenerate after an intentional writer change, and **read the diff**:
//!
//! ```sh
//! UPDATE_FIXTURES=1 cargo test -p nord-format --test fixtures
//! ```

#[path = "support/format_table.rs"]
mod format_table;

use format_table::formats;
use nord_format::cbin::{Cbin, Generation, Header, RawBody};
use nord_format::formats::ne5::{self, EqualizerPart};
use nord_format::Entity;
use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::ops::RangeInclusive;
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

/// A mutated Electro 5 program fixture: the field it sets, the file it lands
/// in, the byte span the mutation may touch, and the check its decode must
/// pass. The spans restate the panels' places in the 165-byte type-1 file, so a
/// write that bleeds outside its panel fails here.
type Mutant = (
    &'static str,
    RangeInclusive<usize>,
    fn(&mut Cbin<ne5::Program>),
    fn(&Cbin<ne5::Program>),
);

/// One mutation per panel, plus two fields that straddle a byte boundary.
fn mutants() -> Vec<Mutant> {
    vec![
        (
            "center-gain-96.ne5p",
            0x2e..=0x34,
            |p| p.center_panel.gain = 96u8.try_into().unwrap(),
            |p| assert_eq!(p.center_panel.gain, 96),
        ),
        (
            "piano-touch-3.ne5p",
            0x3a..=0x41,
            |p| p.piano_panel.touch = 3u8.try_into().unwrap(),
            |p| assert_eq!(p.piano_panel.touch, 3),
        ),
        (
            "sample-number-211.ne5p",
            0x46..=0x4d,
            |p| p.sample_panel.number = 211,
            |p| assert_eq!(p.sample_panel.number, 211),
        ),
        (
            "organ-perc-third.ne5p",
            0x4e..=0x92,
            |p| p.organ_panel.set_b3_perc_third(true),
            |p| assert!(p.organ_panel.b3_perc_third()),
        ),
        (
            "fx3-compression-101.ne5p",
            0x93..=0x9f,
            |p| p.effects_panel.fx3_compression = 101u8.try_into().unwrap(),
            |p| assert_eq!(p.effects_panel.fx3_compression, 101),
        ),
        (
            // Three bits in 0x9a, four in 0x9b.
            "eq-freq-gain-85.ne5p",
            0x93..=0x9f,
            |p| p.effects_panel.equalizer_freq_gain = 0x55u8.try_into().unwrap(),
            |p| assert_eq!(p.effects_panel.equalizer_freq_gain, 0x55),
        ),
        (
            // Five bits in 0x9e, two in 0x9f.
            "fx5-moisture-42.ne5p",
            0x93..=0x9f,
            |p| p.effects_panel.fx5_moisture = 0x2au8.try_into().unwrap(),
            |p| assert_eq!(p.effects_panel.fx5_moisture, 0x2a),
        ),
        (
            "eq-part-both.ne5p",
            0xa1..=0xa4,
            |p| p.effects_panel.equalizer_part = EqualizerPart::Both,
            |p| assert_eq!(p.effects_panel.equalizer_part, EqualizerPart::Both),
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

    // The decoded formats, through their constructors and setters.
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
    }
    {
        let slot = |x: u16, y: u16| (x, y).try_into().unwrap();
        let song = ne5::song::new(
            (0, 2).try_into().unwrap(),
            1,
            [slot(5, 9), slot(0, 1), slot(0, 2), slot(5, 8)],
        );
        put("ne5/song.ne5t".into(), to_bytes(&song));
    }
    for (name, _, mutate, _) in mutants() {
        let mut program = default_program();
        mutate(&mut program);
        put(format!("ne5/{name}"), to_bytes(&program));
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

/// Every committed fixture parses, classifies to the tag its filename carries,
/// and re-encodes byte-exactly.
#[test]
fn every_fixture_parses_classifies_and_round_trips() {
    let files = committed();
    assert!(files.len() > 100, "only {} fixtures on disk", files.len());

    for (name, bytes) in &files {
        let entity = nord_format::from_stream(&mut Cursor::new(bytes))
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        if let Some(stub) = name.strip_prefix("cbin/") {
            let tag = stub.split('.').next().unwrap();
            assert_eq!(
                entity.identity().format.trim_end_matches('\0'),
                tag,
                "{name} dispatched to {:?}",
                entity.identity()
            );
        }

        let back = nord_format::to_bytes(&entity).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&back, bytes, "{name} did not round-trip byte-exactly");
    }
}

/// Each mutated program decodes to the value that was set, and differs from the
/// default program only inside its panel's byte span (plus the header's body
/// checksum) — a write that bleeds outside its panel fails here.
#[test]
fn mutants_decode_and_stay_inside_their_panel() {
    let files = committed();
    let default = &files["ne5/default.ne5p"];

    for (name, span, _, check) in mutants() {
        let bytes = &files[&format!("ne5/{name}")];
        let Entity::Program(nord_format::Program::Electro5(program)) =
            nord_format::from_stream(&mut Cursor::new(bytes)).unwrap()
        else {
            panic!("{name}: not an Electro 5 program")
        };
        check(&program);

        for (at, (a, b)) in default.iter().zip(bytes.iter()).enumerate() {
            let allowed = span.contains(&at) || (0x18..=0x1b).contains(&at);
            assert!(
                allowed || a == b,
                "{name}: mutation changed byte {at:#04x}, outside its panel"
            );
        }
    }
}

/// Every declared field takes every value its type declares, reaches the bytes,
/// and reads back — and moves no other field doing it. The registry is the
/// loop, so a field is covered by being declared.
macro_rules! field_sweep {
    ($make:expr, $body:ty, $variant:pat => $file:ident) => {{
        let baseline = $make.fields();
        let specs = <$body>::field_specs();
        assert!(!specs.is_empty());
        for spec in specs {
            for value in (spec.legal)() {
                let mut file = $make;
                file.set_field(&spec.name, &value)
                    .unwrap_or_else(|e| panic!("{} = {value}: {e}", spec.name));
                let bytes = to_bytes(&file);
                let $variant = nord_format::from_stream(&mut Cursor::new(&bytes))
                    .unwrap_or_else(|e| panic!("{} = {value}: {e}", spec.name))
                else {
                    panic!("{} = {value}: reparse changed the entity kind", spec.name)
                };
                for (before, after) in baseline.iter().zip($file.fields()) {
                    if after.path == spec.name {
                        assert_eq!(after.value, value, "{} did not read back", spec.name);
                    } else {
                        assert_eq!(
                            before.value, after.value,
                            "{} = {value} also moved {}",
                            spec.name, after.path
                        );
                    }
                }
            }
        }
    }};
}

#[test]
fn every_program_field_takes_every_declared_value() {
    field_sweep!(
        default_program(),
        ne5::Program,
        Entity::Program(nord_format::Program::Electro5(file)) => file
    );
}

#[test]
fn every_settings_field_takes_every_declared_value() {
    field_sweep!(
        ne5::settings::new(),
        ne5::Settings,
        Entity::Settings(nord_format::Settings::Electro5(file)) => file
    );
}
