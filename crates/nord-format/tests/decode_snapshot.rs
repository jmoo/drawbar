#![cfg(feature = "corpus")]
//! Per-field decode snapshots for the Electro 5 formats.
//!
//! ⚠️ Byte-exact round-trip cannot catch a wrong bit range: a panel keeps the bytes it
//! was decoded from, so a field reading its neighbour's bits still writes the file back
//! identically. These snapshots watch the decode itself.
//!
//! [`fields`] pins **where every field sits and which values the corpus has ever shown
//! there**. It deliberately records no specimen count and no per-file detail, so adding
//! specimens changes it only when they exercise a value the corpus had not reached before
//! — which is a result worth seeing, not noise. Move a range by one bit and the observed
//! values change on nearly every field.
//!
//! [`specimens`] pins every field of a short fixed list of files, so a change has one
//! concrete, readable place to show itself. [`settings`] is both views of the one `.ne5s`
//! panel in a single file.
//!
//! Regenerate them with `UPDATE_SNAPSHOTS=1`, and **read the diff** — these files are the
//! record of what the decode claims, so an unexamined re-bless costs exactly what the
//! snapshot was bought for.
//!
//! ```sh
//! NORD_CORPUS_DIR=/path/to/nord-corpus/ne5 \
//!   cargo test -p nord-format --features corpus --test decode_snapshot
//! ```

use nord_format::cbin::Cbin;
use nord_format::formats::ne5;
use nord_format::formats::ne5::program::OrganPanel;
use nord_format::formats::ne5::{OrganModel, Program};
use nord_format::Entity;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Root of the Electro 5 specimen corpus — see `tests/ne5.rs`.
fn corpus_dir() -> PathBuf {
    std::env::var_os("NORD_CORPUS_DIR")
        .map(PathBuf::from)
        .expect("set NORD_CORPUS_DIR to a nord-corpus/ne5 checkout for --features corpus")
}

/// The files pinned field-by-field by [`specimens`]: one per `programs/` subdirectory,
/// each with a non-default value in the panel it was captured for, plus one factory
/// program from the full backup as a specimen nobody constructed.
const PINNED: &[&str] = &[
    "programs/center_panel/o00_0_p000_0_1_6_50_50.ne5p",
    "programs/equalizer/1_000000000064.ne5p",
    "programs/fx/fx1_100_5.ne5p",
    "programs/gain/10.ne5p",
    "programs/organ/1000000876543210.ne5p",
    "programs/piano/0000_02_01.ne5p",
    "programs/sample/100_01_000_s064.ne5p",
    "usb/backup/full_backup/contents/Program/Bank 1/Amped Vox.ne5p",
];

/// One field's decode: a panel-qualified key, where its bits sit, the bits themselves,
/// and what they decoded to.
struct Row {
    key: String,
    placement: String,
    /// The field's bits shifted down to bit 0, carrying no type — so this survives a
    /// field being retyped and pins the placement on its own. `None` where the bits are
    /// not reachable: some organ accessors return a decoded value with no way to ask for
    /// the pattern behind it.
    raw: Option<u64>,
    value: String,
}

impl Row {
    fn new(
        key: String,
        placement: impl Into<String>,
        raw: impl Into<Option<u64>>,
        value: impl Into<String>,
    ) -> Row {
        Row {
            key,
            placement: placement.into(),
            raw: raw.into(),
            value: value.into(),
        }
    }

    fn raw_str(&self) -> String {
        match self.raw {
            Some(raw) => raw.to_string(),
            None => "—".to_string(),
        }
    }
}

/// One `#[bitbody]` registry's fields, in declaration order, keyed by `prefix`
/// plus the field's own path (which a nested body has already qualified with its
/// own name).
fn packed(prefix: &str, values: Vec<nord_format::fields::FieldValue>) -> Vec<Row> {
    values
        .into_iter()
        .map(|f| {
            let key = if prefix.is_empty() {
                f.name.clone()
            } else {
                format!("{prefix}.{}", f.name)
            };
            Row::new(key, f.placement, f.raw, f.value)
        })
        .collect()
}

/// Where one organ model's state sits, as absolute Electro 5 file offsets.
///
/// Restated here rather than read from `nord-format`: the panel's own copy of these
/// offsets is what is under test, and a snapshot compared against the numbers it came
/// from would pin nothing.
struct Bytes {
    model: OrganModel,
    /// Nine-nibble drawbar block, preset 1 then preset 2.
    drawbars: (usize, usize),
    /// Holds the selected preset in bit 6.
    preset: usize,
    /// Per-preset vibrato/percussion byte. Pipe has neither.
    effect: Option<(usize, usize)>,
    /// Holds the model's vib/chorus type in bits 7..5, shared across presets.
    vib_type: Option<usize>,
}

const ORGAN_BYTES: [Bytes; 4] = [
    Bytes {
        model: OrganModel::B3,
        drawbars: (0x55, 0x5c),
        preset: 0x53,
        effect: Some((0x59, 0x60)),
        vib_type: Some(0x51),
    },
    Bytes {
        model: OrganModel::Vox,
        drawbars: (0x67, 0x6d),
        preset: 0x65,
        effect: Some((0x6b, 0x71)),
        vib_type: Some(0x63),
    },
    Bytes {
        model: OrganModel::Farfisa,
        drawbars: (0x77, 0x7d),
        preset: 0x75,
        effect: Some((0x7b, 0x81)),
        vib_type: Some(0x73),
    },
    Bytes {
        model: OrganModel::Pipe,
        drawbars: (0x87, 0x8d),
        preset: 0x85,
        effect: None,
        vib_type: None,
    },
];

/// The organ panel through its accessors, in the same shape as [`packed`].
///
/// Hand-written on purpose: walking the panel's own [`Panel`] metadata would pin the
/// declaration against itself. The accessors and the offsets above are a second,
/// independent statement of where the organ's state lives.
fn organ(o: &OrganPanel) -> Vec<Row> {
    let mut rows = Vec::new();
    let key = |what: &str| format!("OrganPanel.{what}");

    for b in ORGAN_BYTES {
        let model = b.model;

        for (preset, at) in [(1u8, b.drawbars.0), (2, b.drawbars.1)] {
            // No raw column: the nibbles are stored identity, so the decoded array *is*
            // the bits.
            rows.push(Row::new(
                key(&format!("drawbars({model:?},{preset})")),
                format!("{:#04x}..{:#04x}", at, at + 4),
                None,
                format!("{:?}", o.drawbars(model, preset)),
            ));
        }

        rows.push(Row::new(
            key(&format!("preset({model:?})")),
            format!("{:#04x}[6:6]", b.preset),
            u64::from(o.preset(model) == 2),
            o.preset(model).to_string(),
        ));

        // The 3-bit type indexes a per-model table and the index itself is not reachable
        // from outside, so only the decoded value is pinned.
        rows.push(Row::new(
            key(&format!("vib_type({model:?})")),
            match b.vib_type {
                Some(at) => format!("{at:#04x}[7:5]"),
                None => "—".to_string(),
            },
            None,
            format!("{:?}", o.vib_type(model)),
        ));

        if let Some((e1, e2)) = b.effect {
            for (preset, at) in [(1u8, e1), (2, e2)] {
                rows.push(Row::new(
                    key(&format!("vib_on({model:?},{preset})")),
                    format!("{at:#04x}[3:3]"),
                    u64::from(o.vib_on(model, preset)),
                    o.vib_on(model, preset).to_string(),
                ));
            }
        }
    }

    for preset in [1u8, 2] {
        rows.push(Row::new(
            key(&format!("b3_perc_on({preset})")),
            format!("{:#04x}[2:2]", if preset == 2 { 0x60 } else { 0x59 }),
            u64::from(o.b3_perc_on(preset)),
            o.b3_perc_on(preset).to_string(),
        ));
    }
    rows.push(Row::new(
        key("b3_perc_third"),
        "0x51[4:4]",
        u64::from(o.b3_perc_third()),
        o.b3_perc_third().to_string(),
    ));
    rows.push(Row::new(
        key("b3_perc_speed"),
        "0x51[3:2]",
        None,
        format!("{:?}", o.b3_perc_speed()),
    ));

    // The bass manual's two bars live outside the nibble block — see
    // `OrganPanel::b3_bass_drawbars`.
    rows.push(Row::new(
        key("b3_bass_drawbars"),
        "0x59[3:0]+0x5a[7:0]",
        None,
        format!("{:?}", o.b3_bass_drawbars()),
    ));

    rows
}

/// Every field of every panel of one program, in file order.
fn rows(p: &Program) -> Vec<Row> {
    let mut rows = packed("CenterPanel", p.center_panel.field_values());
    rows.extend(packed("PianoPanel", p.piano_panel.field_values()));
    rows.extend(packed("SamplePanel", p.sample_panel.field_values()));
    rows.extend(organ(&p.organ_panel));
    rows.extend(packed("EffectsPanel", p.effects_panel.field_values()));
    rows
}

fn read_program(path: &Path) -> Cbin<Program> {
    match nord_format::from_path(path)
        .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()))
    {
        Entity::Program(nord_format::Program::Electro5(p)) => p,
        other => panic!("{} is not an Electro 5 program: {other:?}", path.display()),
    }
}

/// `pending/` is a staging area of untracked local files, not corpus content — the
/// snapshots would otherwise record whatever happened to be sitting in one checkout.
const UNTRACKED: &str = "pending";

/// Every `.ne5p` the corpus ships, in a stable order.
fn all_programs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != UNTRACKED) {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "ne5p") {
                found.push(path);
            }
        }
    }
    found.sort();
    assert!(
        found.len() > 100,
        "found only {} programs under {} — is this a nord-corpus/ne5 checkout?",
        found.len(),
        root.display()
    );
    found
}

/// How many distinct values a snapshot line lists before it just counts them.
const SHOWN: usize = 10;

/// `6 [Organ, Piano, …]` — the distinct count, then as many values as fit.
///
/// The count is what stops a long list from hiding a change past the cut: it moves
/// whenever the set does, even when the first `SHOWN` entries do not.
fn summarise(seen: &BTreeSet<String>) -> String {
    let head: Vec<_> = seen.iter().take(SHOWN).cloned().collect();
    let more = if seen.len() > head.len() { ", …" } else { "" };
    format!("{:<4} [{}{more}]", seen.len(), head.join(", "))
}

#[test]
fn fields() {
    let root = corpus_dir();
    let programs = all_programs(&root);

    // Insertion-ordered by first sighting, which is declaration order within each panel.
    let mut order = Vec::new();
    let mut placements: BTreeMap<String, String> = BTreeMap::new();
    let mut raws: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for path in &programs {
        for row in rows(&read_program(path)) {
            match placements.get(&row.key) {
                None => {
                    order.push(row.key.clone());
                    placements.insert(row.key.clone(), row.placement.clone());
                }
                Some(known) => assert_eq!(
                    known, &row.placement,
                    "{} reports two placements — {known} and {}",
                    row.key, row.placement
                ),
            }
            raws.entry(row.key.clone())
                .or_default()
                .insert(row.raw_str());
            values.entry(row.key).or_default().insert(row.value);
        }
    }

    let mut out = String::new();
    out.push_str(
        "# Per-field decode over every .ne5p in the corpus: where each field sits and\n\
         # every value the corpus has been seen to put there. No specimen count, so\n\
         # adding specimens only shows up when they reach a value not reached before.\n\
         # A field listing one value is a field the corpus cannot check.\n",
    );
    // A field the corpus cannot check: neither its bits nor its decoded value ever move.
    // Both views have to be flat — a field whose bits are unreachable (`raw` is `—`) still
    // varies if its value does.
    let unvarying = |key: &String| raws[key].len() == 1 && values[key].len() == 1;

    for key in &order {
        let raw = &raws[key];
        let single = if unvarying(key) { "  UNVARYING" } else { "" };
        let _ = write!(
            out,
            "\n{key}\n  at      {}{single}\n  raw     {}\n  decoded {}\n",
            placements[key],
            summarise(raw),
            summarise(&values[key]),
        );
    }

    let flat = order.iter().filter(|k| unvarying(k)).count();
    println!(
        "{} programs, {} fields; {flat} unvarying across the whole corpus",
        programs.len(),
        order.len()
    );

    compare("decode_fields.snapshot", &out);
}

#[test]
fn specimens() {
    let root = corpus_dir();
    let mut out = String::new();
    out.push_str(
        "# Every field of a fixed handful of specimens. The companion to\n\
         # decode_fields.snapshot: one concrete file per constructed panel, so a decode\n\
         # change has a readable place to land.\n",
    );

    for name in PINNED {
        let path = root.join(name);
        assert!(
            path.is_file(),
            "pinned specimen {name} is missing — the corpus moved; update PINNED"
        );
        let _ = write!(out, "\n=== {name}\n");
        for row in rows(&read_program(&path)) {
            let _ = writeln!(
                out,
                "{:<34} {:<22} raw {:<12} {}",
                row.key,
                row.placement,
                row.raw_str(),
                row.value
            );
        }
    }

    compare("decode_specimens.snapshot", &out);
}

/// Every `.ne5s` the corpus ships, in a stable order.
fn all_settings(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![
        root.join("settings.ne5s"),
        root.join("usb/backup/full_backup/contents/Settings/Settings/Settings.ne5s"),
    ];
    for entry in fs::read_dir(root.join("settings")).expect("settings corpus") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ne5s") {
            found.push(path);
        }
    }
    found.sort();
    assert!(
        found.len() > 100,
        "found only {} settings files under {} — is this a nord-corpus/ne5 checkout?",
        found.len(),
        root.display()
    );
    found
}

fn read_settings(path: &Path) -> Cbin<ne5::Settings> {
    match nord_format::from_path(path)
        .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()))
    {
        Entity::Settings(nord_format::Settings::Electro5(s)) => s,
        other => panic!("{} is not Electro 5 settings: {other:?}", path.display()),
    }
}

/// The settings panel over the whole `.ne5s` corpus, then one specimen in full.
///
/// Same two views as [`fields`] and [`specimens`], in one file because there is one panel:
/// where each field sits with every value the corpus puts there, then the baseline capture
/// field by field.
#[test]
fn settings() {
    let root = corpus_dir();
    let paths = all_settings(&root);

    let mut order = Vec::new();
    let mut placements: BTreeMap<String, String> = BTreeMap::new();
    let mut raws: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for path in &paths {
        let settings = read_settings(path);
        // The flat body registers the startup settings too, so they are recorded
        // next to the menu settings rather than going unwatched.
        let rows = packed("", settings.field_values());
        for row in rows {
            let raw = row.raw_str();
            if placements.insert(row.key.clone(), row.placement).is_none() {
                order.push(row.key.clone());
            }
            raws.entry(row.key.clone()).or_default().insert(raw);
            values.entry(row.key).or_default().insert(row.value);
        }
    }

    let mut out = String::new();
    out.push_str(
        "# Per-field decode over every .ne5s in the corpus: where each field sits and\n\
         # every value the corpus has been seen to put there. A field listing one value\n\
         # is a field the corpus cannot check.\n",
    );
    for key in &order {
        let single = if raws[key].len() == 1 && values[key].len() == 1 {
            "  UNVARYING"
        } else {
            ""
        };
        let _ = write!(
            out,
            "\n{key}\n  at      {}{single}\n  raw     {}\n  decoded {}\n",
            placements[key],
            summarise(&raws[key]),
            summarise(&values[key]),
        );
    }

    // The sweep's own reference capture: every setting at once, so a moved range has a
    // concrete place to show itself as well as an aggregate one.
    let baseline = root.join("settings/baseline.ne5s");
    let _ = write!(out, "\n=== settings/baseline.ne5s\n");
    for row in packed("", read_settings(&baseline).field_values()) {
        let _ = writeln!(
            out,
            "{:<44} {:<12} raw {:<6} {}",
            row.key,
            row.placement,
            row.raw_str(),
            row.value
        );
    }

    compare("decode_settings.snapshot", &out);
}

/// Compare against the committed snapshot, or rewrite it under `UPDATE_SNAPSHOTS=1`.
fn compare(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name);

    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        println!("wrote {}", path.display());
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{} is missing ({e}) — generate it with UPDATE_SNAPSHOTS=1",
            path.display()
        )
    });

    if expected == actual {
        return;
    }

    let mut diff = String::new();
    for (n, (want, got)) in expected.lines().zip(actual.lines()).enumerate() {
        if want != got {
            let _ = write!(diff, "\n  line {}:\n    want {want}\n    got  {got}", n + 1);
        }
    }
    if expected.lines().count() != actual.lines().count() {
        let _ = write!(
            diff,
            "\n  length: want {} lines, got {}",
            expected.lines().count(),
            actual.lines().count()
        );
    }

    panic!(
        "{} no longer matches the decode:{diff}\n\nIf the change is intended, re-bless with \
         UPDATE_SNAPSHOTS=1 — after reading the diff.",
        path.display()
    );
}
