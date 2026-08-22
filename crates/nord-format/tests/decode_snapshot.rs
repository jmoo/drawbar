#![cfg(feature = "corpus")]
//! Per-field decode snapshots for the Electro 5 formats, and for the Stage 4 —
//! whose 878 placements came from an external offset table and were never read
//! by eye, so the snapshot is the only thing standing between a mis-transcribed
//! range and a decode nobody notices is wrong.
//!
//! ⚠️ Byte-exact round-trip cannot catch a wrong bit range: a panel keeps the bytes it
//! was decoded from, so a field reading its neighbor's bits still writes the file back
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
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-format --features corpus --test decode_snapshot
//! ```

use nord_format::cbin::Cbin;
use nord_format::formats::ne5;
use nord_format::formats::ne5::program::OrganPanel;
use nord_format::formats::ne5::{OrganModel, Program};
use nord_format::{Entity, Live, Settings};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

#[path = "support/scan.rs"]
mod scan;

use scan::{corpus, named};

/// The files pinned field-by-field by [`specimens`]: one per constructed panel,
/// each with a non-default value in the panel it was captured for, plus one factory
/// program from the full backup as a specimen nobody constructed. Named by their
/// trailing path components, wherever the corpus keeps them.
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

/// Every Electro 5 program the corpus holds, in a stable order.
fn all_programs() -> Vec<&'static Cbin<Program>> {
    let found: Vec<_> = corpus()
        .iter()
        .filter_map(|s| match &s.entity {
            Entity::Program(nord_format::Program::Electro5(p)) => Some(p),
            _ => None,
        })
        .collect();
    assert!(!found.is_empty(), "no Electro 5 program in the corpus");
    found
}

/// Chooses the specimens one snapshot group is over, and reads their rows.
type Pick = fn(&Entity) -> Option<Vec<Row>>;

/// Every specimen `pick` accepts, as its registry rows, in a stable order.
fn rows_of(pick: Pick) -> Vec<Vec<Row>> {
    let found: Vec<_> = corpus().iter().filter_map(|s| pick(&s.entity)).collect();
    assert!(!found.is_empty(), "no specimen of that kind in the corpus");
    found
}

/// How many distinct values a snapshot line lists before it just counts them.
const SHOWN: usize = 10;

/// `6 [Organ, Piano, …]` — the distinct count, then as many values as fit.
///
/// The count is what stops a long list from hiding a change past the cut: it moves
/// whenever the set does, even when the first `SHOWN` entries do not.
fn summarize(seen: &BTreeSet<String>) -> String {
    let head: Vec<_> = seen.iter().take(SHOWN).cloned().collect();
    let more = if seen.len() > head.len() { ", …" } else { "" };
    format!("{:<4} [{}{more}]", seen.len(), head.join(", "))
}

/// Whether a field's decoded column would only restate its raw one.
///
/// A `bool` renders `0`/`1` as `false`/`true`, which is a second spelling of the same
/// fact on every flag in the body — and the Stage bodies are a third flags by count. The
/// decoded column is for where a *type* says something, so this keeps it for those.
fn decode_adds_nothing(raw: &BTreeSet<String>, decoded: &BTreeSet<String>) -> bool {
    raw == decoded
        || decoded
            .iter()
            .all(|v| v == "false" || v == "true" || v == "—")
}

#[test]
fn fields() {
    let programs = all_programs();

    // Insertion-ordered by first sighting, which is declaration order within each panel.
    let mut order = Vec::new();
    let mut placements: BTreeMap<String, String> = BTreeMap::new();
    let mut raws: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for program in &programs {
        for row in rows(program) {
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
            summarize(raw),
            summarize(&values[key]),
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

/// The Stage 4 counterpart to [`fields`], over all four decoded bodies at once.
///
/// Same reasoning, one more reason: nothing here was placed by hand, so the raw
/// column is the whole point. Slide a range by a bit and it moves on nearly every
/// field of every specimen.
#[test]
fn stage4_fields() {
    let mut out = String::new();
    out.push_str(
        "# Per-field decode over every Stage 4 specimen in the corpus: where each\n\
         # field sits and every raw value the corpus has been seen to put there. No\n\
         # specimen count, so adding specimens only shows up when they reach a value\n\
         # not reached before. UNVARYING marks a field the corpus cannot check — on a\n\
         # factory bank that is most of the morph targets.\n\
         #\n\
         # `raw` pins the placement; `decoded` pins the interpretation, and the two fail\n\
         # in different places. A field whose type makes no claim renders the raw value\n\
         # back, so the columns agree until a type says something — which is exactly\n\
         # when a reader wants to look.\n",
    );

    let groups: [(&str, Pick); 5] = [
        ("ns4p program", |e| match e {
            Entity::Program(nord_format::Program::Stage4(p)) => Some(packed("", p.field_values())),
            _ => None,
        }),
        ("ns4l live", |e| match e {
            Entity::Live(Live::Stage4(p)) => Some(packed("", p.field_values())),
            _ => None,
        }),
        ("ns4y synth preset", |e| match e {
            Entity::Synth(nord_format::Synth::Stage4(y)) => Some(packed("", y.field_values())),
            _ => None,
        }),
        ("ns4n piano preset", |e| match e {
            Entity::PianoPreset(nord_format::PianoPreset::Stage4(n)) => {
                Some(packed("", n.field_values()))
            }
            _ => None,
        }),
        ("ns4o organ preset", |e| match e {
            Entity::OrganPreset(nord_format::OrganPreset::Stage4(o)) => {
                Some(packed("", o.field_values()))
            }
            _ => None,
        }),
    ];
    for (label, pick) in groups {
        let files = rows_of(pick);
        let mut order = Vec::new();
        let mut placements: BTreeMap<String, String> = BTreeMap::new();
        let mut raws: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        for rows in &files {
            for row in rows.iter() {
                match placements.get(&row.key) {
                    None => {
                        order.push(row.key.clone());
                        placements.insert(row.key.clone(), row.placement.clone());
                    }
                    Some(known) => assert_eq!(known, &row.placement, "{}", row.key),
                }
                let raw = row.raw_str();
                raws.entry(row.key.clone()).or_default().insert(raw);
                values
                    .entry(row.key.clone())
                    .or_default()
                    .insert(row.value.clone());
            }
        }

        let unvarying = |k: &String| raws[k].len() == 1 && values[k].len() == 1;
        let flat = order.iter().filter(|k| unvarying(k)).count();
        let _ = write!(
            out,
            "\n\n### {label} — {} fields, {flat} unvarying\n\n",
            order.len()
        );
        for key in &order {
            let single = if unvarying(key) { "UNVARYING" } else { "" };
            let _ = writeln!(
                out,
                "{key:<44} {:<13} {single:<9} raw {}",
                placements[key],
                summarize(&raws[key]),
            );
            // Only when the type says something the raw column does not already say.
            if !decode_adds_nothing(&raws[key], &values[key]) {
                let _ = writeln!(
                    out,
                    "{:<44} {:<13} {:<9} dec {}",
                    "",
                    "",
                    "",
                    summarize(&values[key])
                );
            }
        }
        println!("{label}: {} specimens, {} fields", files.len(), order.len());
    }

    compare("decode_stage4_fields.snapshot", &out);
}

/// The Stage 2 and Stage 3 programs, same idea and the same reason: these placements
/// were transcribed from a source with known errors in it, so the raw column is what
/// says a run still sits where it was read.
#[test]
fn stage23_fields() {
    let mut out = String::new();
    out.push_str(
        "# Per-field decode over every Stage 2 / Stage 3 program in the corpus: where\n\
         # each field sits, and every value the corpus has been seen to put there.\n\
         # UNVARYING marks a field the corpus cannot check.\n\
         #\n\
         # `raw` pins the placement; `dec` pins the interpretation, and shows only where\n\
         # the field's type says something the raw value does not already say.\n",
    );

    let groups: [(&str, Pick); 5] = [
        ("ns3f program", |e| match e {
            Entity::Program(nord_format::Program::Stage3(p)) => Some(packed("", p.field_values())),
            _ => None,
        }),
        ("ns3l live", |e| match e {
            Entity::Live(Live::Stage3(p)) => Some(packed("", p.field_values())),
            _ => None,
        }),
        ("ns2p program", |e| match e {
            Entity::Program(nord_format::Program::Stage2(p)) => Some(packed("", p.field_values())),
            _ => None,
        }),
        ("ns2l live", |e| match e {
            Entity::Live(Live::Stage2(p)) => Some(packed("", p.field_values())),
            _ => None,
        }),
        ("ns3y synth preset", |e| match e {
            Entity::Synth(nord_format::Synth::Stage3(y)) => Some(packed("", y.field_values())),
            _ => None,
        }),
    ];
    for (label, pick) in groups {
        let files = rows_of(pick);
        let mut order = Vec::new();
        let mut placements: BTreeMap<String, String> = BTreeMap::new();
        let mut raws: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        for rows in &files {
            for row in rows.iter() {
                match placements.get(&row.key) {
                    None => {
                        order.push(row.key.clone());
                        placements.insert(row.key.clone(), row.placement.clone());
                    }
                    Some(known) => assert_eq!(known, &row.placement, "{}", row.key),
                }
                let raw = row.raw_str();
                raws.entry(row.key.clone()).or_default().insert(raw);
                values
                    .entry(row.key.clone())
                    .or_default()
                    .insert(row.value.clone());
            }
        }

        let unvarying = |k: &String| raws[k].len() == 1 && values[k].len() == 1;
        let flat = order.iter().filter(|k| unvarying(k)).count();
        let _ = write!(
            out,
            "\n\n### {label} — {} fields, {flat} unvarying\n\n",
            order.len()
        );
        for key in &order {
            let single = if unvarying(key) { "UNVARYING" } else { "" };
            let _ = writeln!(
                out,
                "{key:<48} {:<13} {single:<9} raw {}",
                placements[key],
                summarize(&raws[key]),
            );
            if !decode_adds_nothing(&raws[key], &values[key]) {
                let _ = writeln!(
                    out,
                    "{:<48} {:<13} {:<9} dec {}",
                    "",
                    "",
                    "",
                    summarize(&values[key])
                );
            }
        }
        println!("{label}: {} specimens, {} fields", files.len(), order.len());
    }

    compare("decode_stage23_fields.snapshot", &out);
}

#[test]
fn specimens() {
    let mut out = String::new();
    out.push_str(
        "# Every field of a fixed handful of specimens. The companion to\n\
         # decode_fields.snapshot: one concrete file per constructed panel, so a decode\n\
         # change has a readable place to land.\n",
    );

    for name in PINNED {
        let mut hits = corpus().iter().filter(|s| s.path.ends_with(name));
        let specimen = hits
            .next()
            .unwrap_or_else(|| panic!("pinned specimen {name} is missing; update PINNED"));
        assert!(hits.next().is_none(), "pinned specimen {name} is ambiguous");
        let Entity::Program(nord_format::Program::Electro5(program)) = &specimen.entity else {
            panic!("{name} is not an Electro 5 program")
        };
        let _ = write!(out, "\n=== {name}\n");
        for row in rows(program) {
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

/// Every Electro 5 settings file the corpus holds, in a stable order.
fn all_settings() -> Vec<&'static Cbin<ne5::Settings>> {
    let found: Vec<_> = corpus()
        .iter()
        .filter_map(|s| match &s.entity {
            Entity::Settings(Settings::Electro5(f)) => Some(f),
            _ => None,
        })
        .collect();
    assert!(!found.is_empty(), "no Electro 5 settings in the corpus");
    found
}

/// The settings panel over the whole `.ne5s` corpus, then one specimen in full.
///
/// Same two views as [`fields`] and [`specimens`], in one file because there is one panel:
/// where each field sits with every value the corpus puts there, then the baseline capture
/// field by field.
#[test]
fn settings() {
    let paths = all_settings();

    let mut order = Vec::new();
    let mut placements: BTreeMap<String, String> = BTreeMap::new();
    let mut raws: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for settings in &paths {
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
            summarize(&raws[key]),
            summarize(&values[key]),
        );
    }

    // The sweep's own reference capture: every setting at once, so a moved range has a
    // concrete place to show itself as well as an aggregate one.
    let Entity::Settings(Settings::Electro5(baseline)) = &named("baseline.ne5s").entity else {
        panic!("baseline.ne5s is not Electro 5 settings")
    };
    let _ = write!(out, "\n=== settings/baseline.ne5s\n");
    for row in packed("", baseline.field_values()) {
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
