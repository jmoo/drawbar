#![cfg(feature = "corpus")]
//! Corpus-backed checks that one-file-at-a-time trials cannot express: floors
//! and cross-file invariants over factory content that carries no oracle, and
//! behavior pins against files the vendor's own editor wrote.
//!
//! The per-specimen sweep is `tests/corpus`; this file holds what it can't say.
//!
//! ```sh
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-format --features corpus --test decode_sanity
//! ```

use nord_format::formats::nsmp;
use nord_format::Entity;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::read;
use std::io::Cursor;
use std::path::{Path, PathBuf};

#[path = "support/corpus.rs"]
mod corpus_loc;

/// Every file under `dir` with extension `ext`, recursively, skipping the
/// staging area, in a stable order.
fn files_with(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).unwrap_or_else(|e| panic!("{}: {e}", next.display())) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "pending" | "tools" | ".git") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == ext) && !name.contains(".skip.") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Container-level facts across every model
// ---------------------------------------------------------------------------

/// The header's `aux` word holds one of three shapes everywhere: `0xFFFFFFFF`
/// (no category), a low u16 under a zero high u16 (the program category id), or
/// — on the preset/library tags alone — both halves set. A fourth shape, or a
/// both-halves value on a program tag, means the word carries something the
/// container docs don't model.
#[test]
fn aux_matches_one_of_the_three_documented_shapes() {
    // The tags observed holding both u16 halves. (`nd2p` does too, but lives
    // inside `.nd2_bank` archives, which this standalone walk does not open.)
    const BOTH_HALVES: &[&str] = &["ns3y", "nsmp"];

    let mut failures: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for path in every_cbin(&corpus_loc::root()) {
        let bytes = fs::read(&path).unwrap();
        let tag = String::from_utf8_lossy(&bytes[8..12]).replace('\0', "");
        let aux = u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap());
        let ok = aux == 0xFFFF_FFFF || (aux >> 16) == 0 || BOTH_HALVES.contains(&tag.as_str());
        if !ok {
            failures.push(format!("{}: {tag} aux {aux:#010x}", path.display()));
        }
        seen += 1;
    }
    assert!(seen > 8000, "only {seen} CBIN files walked");
    assert!(
        failures.is_empty(),
        "{} specimens hold an undocumented aux shape:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Every file in the corpus whose first four bytes are `CBIN`.
fn every_cbin(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "pending" | "tools" | ".git") {
                    stack.push(path);
                }
            } else if !name.contains(".skip.")
                && !name.ends_with(".body")
                && fs::read(&path).is_ok_and(|b| b.len() >= 0x18 && b.starts_with(b"CBIN"))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The stub modules' observed body lengths hold across every specimen.
#[test]
fn observed_body_lengths_match_the_documented_constants() {
    use nord_format::formats::{
        nc2, nc2d, ne3, ne4, ne6, ne7, ng2, nl4, nla1, no3, np, np2, np3, np4, np5, ns2, ns3, ns4,
        nsclassic, nw, nw2,
    };

    let expected: BTreeMap<&str, u64> = BTreeMap::from([
        (nc2::program::FORMAT, nc2::program::BODY_LEN),
        (nc2::settings::FORMAT, nc2::settings::BODY_LEN),
        (nc2d::program::FORMAT, nc2d::program::BODY_LEN),
        (nc2d::settings::FORMAT, nc2d::settings::BODY_LEN),
        (ne3::program::FORMAT, ne3::program::BODY_LEN),
        (ne3::organ_preset::FORMAT, ne3::organ_preset::BODY_LEN),
        (ne4::program::FORMAT, ne4::program::BODY_LEN),
        (ne4::live::FORMAT, ne4::live::BODY_LEN),
        (ne4::settings::FORMAT, ne4::settings::BODY_LEN),
        (ne6::program::FORMAT, ne6::program::BODY_LEN),
        (ne6::live::FORMAT, ne6::live::BODY_LEN),
        (ne6::settings::FORMAT, ne6::settings::BODY_LEN),
        (ne7::program::FORMAT, ne7::program::BODY_LEN),
        (ne7::live::FORMAT, ne7::live::BODY_LEN),
        (ne7::settings::FORMAT, ne7::settings::BODY_LEN),
        (ng2::program::FORMAT, ng2::program::BODY_LEN),
        (ng2::live::FORMAT, ng2::live::BODY_LEN),
        (ng2::settings::FORMAT, ng2::settings::BODY_LEN),
        (nl4::program::FORMAT, nl4::program::BODY_LEN),
        (nl4::performance::FORMAT, nl4::performance::BODY_LEN),
        (nl4::settings::FORMAT, nl4::settings::BODY_LEN),
        (nla1::program::FORMAT, nla1::program::BODY_LEN),
        (nla1::performance::FORMAT, nla1::performance::BODY_LEN),
        (nla1::settings::FORMAT, nla1::settings::BODY_LEN),
        (no3::program::FORMAT, no3::program::BODY_LEN),
        (no3::settings::FORMAT, no3::settings::BODY_LEN),
        (np::program::FORMAT, np::program::BODY_LEN),
        (np::live::FORMAT, np::live::BODY_LEN),
        (np::settings::FORMAT, np::settings::BODY_LEN),
        (np2::program::FORMAT, np2::program::BODY_LEN),
        (np2::live::FORMAT, np2::live::BODY_LEN),
        (np2::settings::FORMAT, np2::settings::BODY_LEN),
        (np3::program::FORMAT, np3::program::BODY_LEN),
        (np3::live::FORMAT, np3::live::BODY_LEN),
        (np3::settings::FORMAT, np3::settings::BODY_LEN),
        (np4::program::FORMAT, np4::program::BODY_LEN),
        (np4::live::FORMAT, np4::live::BODY_LEN),
        (np4::settings::FORMAT, np4::settings::BODY_LEN),
        (np5::program::FORMAT, np5::program::BODY_LEN),
        (np5::live::FORMAT, np5::live::BODY_LEN),
        (np5::settings::FORMAT, np5::settings::BODY_LEN),
        (ns2::program::FORMAT, ns2::program::BODY_LEN as u64),
        (ns2::live::FORMAT, ns2::program::BODY_LEN as u64),
        (ns2::synth::FORMAT, ns2::synth::BODY_LEN),
        (ns2::settings::FORMAT, ns2::settings::BODY_LEN),
        (ns3::program::FORMAT, ns3::program::BODY_LEN as u64),
        (ns3::live::FORMAT, ns3::program::BODY_LEN as u64),
        (ns3::song::FORMAT, ns3::song::BODY_LEN),
        (ns3::synth::FORMAT, ns3::synth::BODY_LEN as u64),
        (ns3::settings::FORMAT, ns3::settings::BODY_LEN),
        (ns4::program::FORMAT, ns4::program::BODY_LEN as u64),
        (ns4::live::FORMAT, ns4::program::BODY_LEN as u64),
        (ns4::synth::FORMAT, ns4::synth::BODY_LEN as u64),
        (
            ns4::piano_preset::FORMAT,
            ns4::piano_preset::BODY_LEN as u64,
        ),
        (
            ns4::organ_preset::FORMAT,
            ns4::organ_preset::BODY_LEN as u64,
        ),
        (ns4::settings::FORMAT, ns4::settings::BODY_LEN),
        (nsclassic::program::FORMAT, nsclassic::program::BODY_LEN),
        (nsclassic::synth::FORMAT, nsclassic::synth::BODY_LEN),
        (nw::program::FORMAT, nw::program::BODY_LEN),
        (nw::settings::FORMAT, nw::settings::BODY_LEN),
        (nw2::program::FORMAT, nw2::program::BODY_LEN),
        (nw2::live::FORMAT, nw2::live::BODY_LEN),
        (nw2::settings::FORMAT, nw2::settings::BODY_LEN),
    ]);

    let mut checked = 0usize;
    for path in every_cbin(&corpus_loc::root()) {
        let mut file = std::fs::File::open(&path).unwrap();
        let Ok(info) = nord_format::cbin::inspect(&mut file) else {
            continue; // not a whole container — the sweep already classified it
        };
        let tag = String::from_utf8_lossy(&info.header.tag).into_owned();
        if let Some(&want) = expected.get(tag.as_str()) {
            assert_eq!(
                info.body_len,
                want,
                "{}: {tag} body is {} bytes where every prior specimen held {want}",
                path.display(),
                info.body_len,
            );
            checked += 1;
        }
    }
    assert!(checked > 8000, "only {checked} bodies measured");
}

// ---------------------------------------------------------------------------
// Stage decode floors — factory content has no oracle, so distribution
// sanity is the strongest available check that the placements are right
// ---------------------------------------------------------------------------

/// The Stage 2/3 globals decode reads values the panel could actually show,
/// across every factory program of both models.
#[test]
fn stage_globals_decode_to_panel_values() {
    use nord_format::{Live, Program};

    let root = corpus_loc::root();
    let mut ns2_seen = 0usize;
    let mut ns3_seen = 0usize;
    let mut ns3_split_on = 0usize;
    let mut ns3_at_default_clock = 0usize;

    for ext in ["ns2p", "ns2l"] {
        for path in files_with(&root, ext) {
            let entity = nord_format::from_path(&path).unwrap();
            match &entity {
                Entity::Program(Program::Stage2(_)) | Entity::Live(Live::Stage2(_)) => {}
                other => panic!("{}: decoded to {other:?}", path.display()),
            }
            ns2_seen += 1;
        }
    }
    for ext in ["ns3f", "ns3l"] {
        for path in files_with(&root, ext) {
            let entity = nord_format::from_path(&path).unwrap();
            let p = match &entity {
                Entity::Program(Program::Stage3(p)) | Entity::Live(Live::Stage3(p)) => p,
                other => panic!("{}: decoded to {other:?}", path.display()),
            };
            ns3_split_on += usize::from(p.split_enabled);
            ns3_at_default_clock += usize::from(p.master_clock.bpm() == 120);
            ns3_seen += 1;
        }
    }

    assert!(ns2_seen > 700, "only {ns2_seen} Stage 2 programs read");
    assert!(ns3_seen > 290, "only {ns3_seen} Stage 3 programs read");
    // A decode where no factory program ever splits is reading the wrong bits,
    // and one where the master clock is not overwhelmingly at its 120 bpm
    // default is reading the wrong bits shifted — either failure moves these.
    assert!(ns3_split_on > 0, "no ns3f decodes with a split enabled");
    assert!(
        ns3_at_default_clock * 2 > ns3_seen,
        "only {ns3_at_default_clock}/{ns3_seen} programs read the 120 bpm default"
    );
}

/// The Stage 4 decode, whose placements came from an external offset table and
/// no hardware. Two independent checks that the table was read into the right
/// bit space: the body echoes the header's version byte at its own offset 3, and
/// the selector fields — each a fixed-width slot holding a short list of panel
/// choices — never hold a value past the end of that list. A base offset off by
/// a byte, or bits numbered the other way round, breaks both at once.
#[test]
fn stage4_bodies_decode_to_panel_values() {
    use nord_format::{Live, OrganPreset, PianoPreset, Program, Synth};

    let root = corpus_loc::root();
    let (mut programs, mut organs, mut pianos, mut synths) = (0usize, 0usize, 0usize, 0usize);
    let mut split_on = 0usize;

    // A selector's slot is wider than the choices the panel offers, so the
    // unused encodings are the check: they must never appear. The octave shift
    // reads through `OctaveShiftNibble`, so the two's-complement wrap that puts
    // -1 at a stored 15 is the type's business rather than this test's.
    let octave_shift = |v: i8| (-2..=2).contains(&v);

    for ext in ["ns4p", "ns4l", "ns4o", "ns4n", "ns4y"] {
        for path in files_with(&root, ext) {
            let entity = nord_format::from_path(&path).unwrap();
            let where_ = path.display();

            match &entity {
                Entity::Program(Program::Stage4(p)) | Entity::Live(Live::Stage4(p)) => {
                    assert_eq!(
                        p.version_echo as u32,
                        p.header.version & 0xff,
                        "{where_}: the body's version echo disagrees with the header"
                    );
                    assert!(
                        p.organ_section_enabled
                            || p.piano_section_enabled
                            || p.synth_section_enabled,
                        "{where_}: no section is routed to the keyboard"
                    );
                    assert!(p.organ_a.model.raw() <= 5, "{where_}");
                    assert!(p.organ_b.model.raw() <= 5, "{where_}");
                    assert!(p.piano_a.piano_type.raw() <= 5, "{where_}");
                    assert!(p.piano_b.piano_type.raw() <= 5, "{where_}");
                    assert!(p.synth_a_voice.filter_type.raw() <= 5, "{where_}");
                    assert!(p.synth_a_voice.lfo_shape.raw() <= 4, "{where_}");
                    assert!(p.synth_a_performance.voice_priority.raw() <= 2, "{where_}");
                    assert!(p.organ_fx.reverb_type.raw() <= 11, "{where_}");
                    assert!(octave_shift(p.organ_a.octave_shift.octaves()), "{where_}");
                    split_on += usize::from(p.split_enabled);
                    programs += 1;
                }
                Entity::OrganPreset(OrganPreset::Stage4(o)) => {
                    assert!(o.organ_a_model.raw() <= 5, "{where_}");
                    assert!(o.organ_b_model.raw() <= 5, "{where_}");
                    assert!(o.organ_fx.reverb_type.raw() <= 11, "{where_}");
                    assert!(octave_shift(o.organ_a_octave_shift.octaves()), "{where_}");
                    organs += 1;
                }
                Entity::PianoPreset(PianoPreset::Stage4(n)) => {
                    assert!(n.piano_a_type.raw() <= 5, "{where_}");
                    assert!(n.piano_b_type.raw() <= 5, "{where_}");
                    assert!(n.piano_a_fx.reverb_type.raw() <= 11, "{where_}");
                    assert!(octave_shift(n.piano_a_octave_shift.octaves()), "{where_}");
                    pianos += 1;
                }
                Entity::Synth(Synth::Stage4(y)) => {
                    assert!(y.synth_a_voice.filter_type.raw() <= 5, "{where_}");
                    assert!(y.synth_b_voice.filter_type.raw() <= 5, "{where_}");
                    assert!(y.synth_a_voice.lfo_shape.raw() <= 4, "{where_}");
                    assert!(y.synth_a_voice_priority.raw() <= 2, "{where_}");
                    assert!(y.synth_a_fx.reverb_type.raw() <= 11, "{where_}");
                    assert!(octave_shift(y.synth_a_octave_shift.octaves()), "{where_}");
                    synths += 1;
                }
                other => panic!("{where_}: decoded to {other:?}"),
            }
        }
    }

    assert!(programs > 380, "only {programs} Stage 4 programs read");
    assert!(organs > 60, "only {organs} organ presets read");
    assert!(pianos > 90, "only {pianos} piano presets read");
    assert!(synths > 380, "only {synths} synth presets read");
    // A decode where no factory program ever splits is reading the wrong bits.
    assert!(split_on > 0, "no Stage 4 program reads a split");
}

/// Every nsmp3/nsmp4 specimen decodes as the wide section chain, with a name
/// and at least one stroke — in both container generations — and the zone maps
/// that pair with their strokes check out zone by zone.
#[test]
fn v3_samples_decode_names_and_strokes() {
    use nord_format::Sample;

    let root = corpus_loc::root();
    let mut paired = 0usize;
    let mut unpaired = 0usize;
    for ext in ["nsmp3", "nsmp4"] {
        for path in files_with(&root, ext) {
            match nord_format::from_path(&path).unwrap() {
                Entity::Sample(Sample::V3(s)) => {
                    let name = s.name().unwrap();
                    assert!(!name.is_empty(), "{}: empty name", path.display());
                    assert!(s.stroke_count() > 0, "{}: no strokes", path.display());
                    match s.zones() {
                        // Unexplained: a large share of the vendor sample pool
                        // carries a zone map holding roughly one entry per
                        // keyboard key (108, 107, 96 entries) — or none at all —
                        // rather than one per stroke. The reader refuses to pair
                        // those, and this test accepts the refusal; a decode of
                        // the wide map would turn these back into assertions.
                        Err(_) => unpaired += 1,
                        Ok(zones) => {
                            assert_eq!(zones.len(), s.stroke_count(), "{}", path.display());
                            for z in &zones {
                                assert!(
                                    z.top_note <= 127 && z.root_key <= 127,
                                    "{}",
                                    path.display()
                                );
                                if let Some(low) = z.low_note {
                                    assert!(low <= z.top_note, "{}: low above top", path.display());
                                }
                            }
                            paired += 1;
                        }
                    }
                }
                other => panic!("{}: decoded to {other:?}", path.display()),
            }
        }
    }
    // The committed tier pairs completely, and even with the full pool
    // projected in the paired files stay the majority — a reader change that
    // stops pairing what used to pair moves both of these.
    assert!(paired >= 12, "only {paired} v3 zone maps paired");
    assert!(
        paired > unpaired,
        "{unpaired} of {} v3 zone maps failed to pair with their strokes",
        paired + unpaired
    );
}

/// The drum banks: every member of every bank parses and the counts match the
/// devices' bank sizes.
#[test]
fn drum_banks_walk_to_their_members() {
    use nord_format::Bundle;

    let root = corpus_loc::root();
    let mut banks = 0usize;
    for (dir, ext) in [
        ("nd2/factory/banks", "nd2_bank"),
        ("nd3p/factory/kitbanks", "nd3_kitbank"),
    ] {
        for path in files_with(&root.join(dir), ext) {
            match nord_format::from_path(&path).unwrap() {
                Entity::Bundle(Bundle::Drum2Bank(b)) => assert_eq!(b.programs.len(), 50),
                Entity::Bundle(Bundle::Drum3KitBank(b)) => assert_eq!(b.kits.len(), 50),
                other => panic!("{}: decoded to {other:?}", path.display()),
            }
            banks += 1;
        }
    }
    assert_eq!(banks, 8, "the corpus ships four banks per drum model");
}

// ---------------------------------------------------------------------------
// Electro 5: the backup's dependency graph, and the live/program equivalence
// ---------------------------------------------------------------------------

/// The stored piano category, mapped to the export directory that holds it,
/// from the backup's own `dir.oracle.json`. The dial order on disk starts at
/// Grand, so the two are not in step.
fn piano_categories(backup: &Path) -> Vec<(String, String)> {
    let sidecar: Value =
        serde_json::from_str(&fs::read_to_string(backup.join("dir.oracle.json")).unwrap()).unwrap();
    sidecar["piano_categories"]
        .as_array()
        .expect("piano_categories in the backup's dir.oracle.json")
        .iter()
        .map(|row| {
            (
                row["category"].as_str().unwrap().to_string(),
                row["directory"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The piano and sample ids across all 624 backup programs form a bijection
/// with the (category, model) slots, and the slots per category are exactly
/// what the backup's member list says the instrument shipped.
#[test]
fn ne5_backup_dependency_ids() {
    let backup = corpus_loc::root().join("ne5/usb/backup/full_backup");

    // The backup's member list: what the instrument actually shipped. The blobs
    // themselves are private-tier and may be absent here, but the listing tells
    // us how many pianos each category held.
    let members = fs::read_to_string(backup.join("backup.members.tsv")).unwrap();
    let mut shipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut samples = 0usize;

    for line in members.lines().skip(1) {
        let Some(name) = line.split('\t').next() else {
            continue;
        };
        if name.ends_with(".nsmp") {
            samples += 1;
        } else if name.ends_with(".npno") {
            let mut parts = name.split('/');
            let (Some("Piano"), Some(category)) = (parts.next(), parts.next()) else {
                panic!("unexpected piano member path: {name}")
            };
            *shipped.entry(category.to_string()).or_default() += 1;
        }
    }
    assert!(
        !shipped.is_empty() && samples > 0,
        "member list looks empty"
    );

    let mut slot_of: BTreeMap<u32, (String, u8)> = BTreeMap::new();
    let mut id_of: BTreeMap<(String, u8), u32> = BTreeMap::new();
    let mut sample_ids: BTreeSet<u32> = BTreeSet::new();
    let mut programs = 0usize;

    for path in files_with(&backup.join("contents/Program"), "ne5p") {
        let name = path.display().to_string();
        let Entity::Program(nord_format::Program::Electro5(program)) =
            nord_format::from_path(&path).unwrap()
        else {
            panic!("expected an ne5 program in {name}")
        };
        let (piano, sample) = (&program.piano_panel, &program.sample_panel);

        if piano.id != 0 {
            let slot = (format!("{:?}", piano.category), piano.piano_model.as_u8());

            // (category, model) and id are two names for the same piano, so the
            // map between them is a bijection across all 624 programs.
            assert_eq!(
                *slot_of.entry(piano.id).or_insert_with(|| slot.clone()),
                slot,
                "piano id {:#010x} spans more than one (category, model) slot, at {name}",
                piano.id,
            );
            assert_eq!(
                *id_of.entry(slot.clone()).or_insert(piano.id),
                piano.id,
                "slot {slot:?} names more than one piano id, at {name}",
            );
        }

        if sample.id != 0 {
            sample_ids.insert(sample.id);
        }

        programs += 1;
    }

    assert!(programs > 0, "no backup programs found — corpus present?");

    // Per category: the model slots the programs reference must be exactly
    // `0..n`, and `n` must be what the backup shipped — with one exception. The
    // programs reference a seventh Upright that the instrument no longer holds,
    // and that single dangling reference is the whole reason this field matters:
    // it is what Nord Sound Manager sees as a missing dependency and gates
    // "Restore" on. If this trips, check whether the corpus gained or lost a
    // piano before assuming the decode moved.
    for (category, directory) in piano_categories(&backup) {
        let models: BTreeSet<u8> = id_of
            .keys()
            .filter(|(c, _)| *c == category)
            .map(|(_, model)| *model)
            .collect();
        let expected = shipped[&directory] + usize::from(directory == "Upright");

        assert_eq!(
            models.len(),
            expected,
            "{directory}: programs reference {} model slots, expected {expected}",
            models.len(),
        );
        assert!(
            models.iter().copied().eq(0..expected as u8),
            "{directory}: model slots are not contiguous from 0: {models:?}",
        );
    }

    // Samples have no slot coordinate to cross-check against — `number` is a
    // volatile Samp Lib position that the corpus reuses across ids — so bound
    // the id count by the shipped library instead.
    assert!(
        sample_ids.len() <= samples,
        "{} distinct sample ids referenced but only {samples} `.nsmp` members shipped",
        sample_ids.len(),
    );
}

/// Every `.ne5l` in the corpus. The live slots ship inside the full backup
/// rather than in a directory of their own, so this searches from the root.
fn ne5l_files() -> Vec<PathBuf> {
    let paths = files_with(&corpus_loc::root().join("ne5"), "ne5l");
    assert!(!paths.is_empty(), "no live slots — corpus present?");
    paths
}

/// The three live slots are `0:0`, `0:1` and `0:2` on the wire, and nothing else.
#[test]
fn ne5_live_occupies_three_slots_of_one_bank() {
    use nord_format::bank::Item;

    let mut seen: BTreeSet<(u16, u16)> = BTreeSet::new();
    for path in ne5l_files() {
        let name = path.display().to_string();
        let Entity::Live(nord_format::Live::Electro5(live)) =
            nord_format::from_path(&path).unwrap()
        else {
            panic!("expected an ne5 live slot in {name}")
        };
        let at = live.location();
        assert_eq!(at.x(), 0, "live slot outside bank 0 in {name}");
        seen.insert(at.inner());
    }

    assert_eq!(
        seen,
        BTreeSet::from([(0, 0), (0, 1), (0, 2)]),
        "the corpus no longer covers all three live slots",
    );
}

/// A live specimen with its tag swapped is a valid program, down to the last field.
///
/// Confirmed on hardware: the same panel state read as a live slot and as a program gives
/// byte-identical bodies. One body serves both, so the field values cannot disagree —
/// what this pins on real specimens is that everything around them agrees too: the slot
/// falls in the program space, the version is one programs accept, and the body checksum
/// still holds.
#[test]
fn ne5_a_live_body_decodes_as_a_program() {
    use nord_format::formats::ne5;

    for path in ne5l_files() {
        let name = path.display().to_string();
        let Entity::Live(nord_format::Live::Electro5(live)) =
            nord_format::from_path(&path).unwrap()
        else {
            panic!("expected an ne5 live slot in {name}")
        };

        // The tag is the whole difference. On a type-1 file the crc32 covers the body
        // and never sees the header, so the retag alone leaves a valid file; a type-0
        // file's trailing crc16 covers the header too, so it gets restamped.
        let mut bytes = read(&path).unwrap();
        bytes[0x08..0x0c].copy_from_slice(ne5::program::FORMAT.as_bytes());
        if bytes[0x04] == 0 {
            let at = bytes.len() - 2;
            let crc = nord_format::crc::crc16(&bytes[..at]);
            bytes[at..].copy_from_slice(&crc.to_le_bytes());
        }

        let Entity::Program(nord_format::Program::Electro5(program)) =
            nord_format::from_stream(&mut Cursor::new(&bytes)).unwrap()
        else {
            panic!("a retagged live slot did not decode as a program: {name}")
        };

        let named = |fields: Vec<nord_format::fields::Field>| -> Vec<(String, String)> {
            fields.into_iter().map(|f| (f.path, f.display)).collect()
        };
        let fields = named(live.fields());
        assert!(!fields.is_empty(), "no fields to compare");
        assert_eq!(
            fields,
            named(program.fields()),
            "live and program decodes disagree on {name}",
        );
    }
}

/// Reading nine drawbar nibbles and writing them back is a no-op, on every
/// program the corpus holds. The organ accessors sit outside the field
/// registry, so the fixture suite's registry sweep cannot cover them — this is
/// their read/write inverse proof, against real data.
#[test]
fn ne5_organ_drawbars_survive_a_rewrite() {
    use nord_format::formats::ne5::OrganModel::*;

    let mut checked = 0usize;
    for path in files_with(&corpus_loc::root().join("ne5/programs"), "ne5p") {
        let name = path.display().to_string();
        let original = read(&path).unwrap();
        let Entity::Program(nord_format::Program::Electro5(mut program)) =
            nord_format::from_stream(&mut Cursor::new(&original)).unwrap()
        else {
            panic!("expected an ne5 program in {name}")
        };

        let organ = &mut program.organ_panel;
        for model in [B3, Vox, Farfisa, Pipe] {
            for preset in [1u8, 2] {
                let bars = organ.drawbars(model, preset);
                if bars.iter().any(|&b| b > 8) {
                    continue;
                }
                organ.set_drawbars(model, preset, bars).unwrap();
            }
        }

        let mut rewritten: Vec<u8> = Vec::new();
        program.write_to(&mut Cursor::new(&mut rewritten)).unwrap();
        assert_eq!(
            original.as_slice(),
            rewritten.as_slice(),
            "rewriting the drawbars changed {name}",
        );
        checked += 1;
    }
    assert!(checked > 200, "only {checked} programs rewritten");
}

// ---------------------------------------------------------------------------
// Sample instruments (`.nsmp`): behavior pins against the editor's own files
// ---------------------------------------------------------------------------

/// Every specimen in `ne5/samples/`, as `(path, stem)`.
fn sample_specimens() -> Vec<(PathBuf, String)> {
    let dir = corpus_loc::root().join("ne5/samples");
    let out: Vec<(PathBuf, String)> = files_with(&dir, "nsmp")
        .into_iter()
        .map(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            (p, stem)
        })
        .collect();
    assert!(!out.is_empty(), "no .nsmp specimens in {}", dir.display());
    out
}

fn read_sample(path: &PathBuf) -> nord_format::cbin::Cbin<nsmp::Sample> {
    match nord_format::from_path(path).unwrap() {
        Entity::Sample(nord_format::Sample::V2(s)) => s,
        other => panic!("{} decoded as {other:?}", path.display()),
    }
}

/// Every stroke decomposes into its header plus whole packets.
///
/// A wrong header rule shows up here as a leftover remainder, on every specimen at once.
#[test]
fn nsmp_strokes_decompose() {
    let mut seen = 0;
    for (path, stem) in sample_specimens() {
        let sample = read_sample(&path);
        let zones = sample.zones().unwrap();
        let strokes = sample.strokes().unwrap_or_else(|e| panic!("{stem}: {e}"));
        assert_eq!(
            strokes.len(),
            zones.len(),
            "{stem}: {} zones but {} strokes",
            zones.len(),
            strokes.len()
        );
        seen += strokes.len();
    }
    assert!(
        seen >= 30,
        "only {seen} strokes walked; is the corpus stale?"
    );
}

/// Zone key ranges match what the editor lays out from the root keys — except
/// where a sidecar's `zone_top_notes_overridden` trait says they were moved by
/// hand, which the per-specimen sweep asserts from the other side.
///
/// So the derivation is the editor's default, not a rule the format enforces: the top
/// note really is stored, and a reader must take it as read rather than recompute it.
#[test]
fn nsmp_zone_ranges_are_the_editors_default_unless_overridden() {
    let mut checked = 0;
    for (path, stem) in sample_specimens() {
        let sidecar = path.with_file_name(format!(
            "{}.oracle.json",
            path.file_name().unwrap().to_string_lossy()
        ));
        let overridden = fs::read_to_string(&sidecar)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| v.get("traits").cloned())
            .is_some_and(|t| {
                t.as_array()
                    .is_some_and(|a| a.iter().any(|t| t == "zone_top_notes_overridden"))
            });

        let sample = read_sample(&path);
        let roots: Vec<u8> = sample
            .strokes()
            .unwrap()
            .iter()
            .map(|s| s.root_key)
            .collect();
        let stored: Vec<u8> = sample.zones().unwrap().iter().map(|z| z.top_note).collect();
        let derived = nsmp::zone::derive_top_notes(&roots);

        if overridden {
            continue; // the sweep's trait checker asserts the inequality
        }
        assert_eq!(
            stored, derived,
            "{stem}: stored key ranges disagree with the ones its root keys {roots:?} imply"
        );
        checked += 1;
    }
    assert!(
        checked > 30,
        "only {checked} specimens checked; corpus stale?"
    );
}

/// Rename and remap reproduce a specimen the editor itself wrote.
///
/// `D7-upperkey` is `D4-3zones` with the middle zone's upper key moved and a new name.
/// Making the same two edits must give back the same bytes — which also pins that a
/// remap leaves the encoded audio alone, and that the checksum is recomputed correctly.
#[test]
fn nsmp_edits_reproduce_the_editors_own_output() {
    let dir = corpus_loc::root().join("ne5/samples");
    let mut sample = read_sample(&dir.join("D4-3zones.nsmp"));

    sample.set_name("D7-upperkey").unwrap();
    sample.set_zone_top_note(1, 60).unwrap();

    assert_eq!(
        sample.to_bytes().unwrap(),
        read(dir.join("D7-upperkey.nsmp")).unwrap(),
        "rename + remap did not reproduce the editor's own file"
    );
}

/// Retuning a zone moves the root key and nothing else but the checksum.
#[test]
fn nsmp_retune_touches_one_byte() {
    let dir = corpus_loc::root().join("ne5/samples");
    let before = read(dir.join("D1-one-zone.nsmp")).unwrap();
    let mut sample = read_sample(&dir.join("D1-one-zone.nsmp"));

    sample.set_root_key(0, 48).unwrap();
    let after = sample.to_bytes().unwrap();

    let differing: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    // The checksum at 0x18..0x1c, plus the one root-key byte.
    assert_eq!(differing.len(), 5, "changed bytes: {differing:?}");
    assert!(differing[..4].iter().eq([0x18, 0x19, 0x1a, 0x1b].iter()));
    assert_eq!(sample.strokes().unwrap()[0].root_key, 48);
}

/// A name longer than the writer is willing to emit is refused, not truncated.
#[test]
fn nsmp_overlong_name_is_refused() {
    let dir = corpus_loc::root().join("ne5/samples");
    let mut sample = read_sample(&dir.join("D1-one-zone.nsmp"));
    assert!(sample.set_name("a name that is far too long").is_err());
    assert_eq!(
        sample.name().unwrap(),
        "TEST",
        "a refused rename still changed it"
    );
}

/// A corrupted body is refused on read rather than decoded.
#[test]
fn nsmp_bad_checksum_is_refused() {
    let mut bytes = read(corpus_loc::root().join("ne5/samples/D1-one-zone.nsmp")).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert!(nord_format::from_stream(&mut Cursor::new(&bytes)).is_err());
}
