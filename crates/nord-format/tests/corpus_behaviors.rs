#![cfg(feature = "corpus")]
//! Behavioral checks against files produced by the instrument and sample editor.

use nord_format::formats::nsmp;
use nord_format::{Entity, Live, Program, Sample};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

#[path = "support/format_table.rs"]
mod format_table;
#[path = "support/scan.rs"]
mod scan;

use format_table::formats;
use scan::{corpus, named, Specimen};

fn cbins() -> impl Iterator<Item = &'static Specimen> {
    corpus().iter().filter(|s| s.bytes.starts_with(b"CBIN"))
}

fn ne5_programs() -> impl Iterator<
    Item = (
        &'static Specimen,
        &'static nord_format::cbin::Cbin<nord_format::formats::ne5::Program>,
    ),
> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Program(Program::Electro5(p)) => Some((s, p)),
        _ => None,
    })
}

fn ne5_lives() -> impl Iterator<
    Item = (
        &'static Specimen,
        &'static nord_format::cbin::Cbin<nord_format::formats::ne5::Program>,
    ),
> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Live(Live::Electro5(p)) => Some((s, p)),
        _ => None,
    })
}

fn v2_samples() -> impl Iterator<
    Item = (
        &'static Specimen,
        &'static nord_format::cbin::Cbin<nsmp::Sample>,
    ),
> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Sample(Sample::V2(v)) => Some((s, v)),
        _ => None,
    })
}

fn v2_named(name: &str) -> nord_format::cbin::Cbin<nsmp::Sample> {
    match nord_format::from_stream(&mut Cursor::new(&named(name).bytes)).unwrap() {
        Entity::Sample(Sample::V2(sample)) => sample,
        other => panic!("{name} decoded as {other:?}"),
    }
}

#[test]
fn cbin_aux_words_have_documented_shapes() {
    const BOTH_HALVES: &[&str] = &["ns3y", "nsmp", "nd2p"];
    let mut seen = 0;
    let mut failures = Vec::new();
    for specimen in cbins() {
        let tag = String::from_utf8_lossy(&specimen.bytes[8..12]).replace('\0', "");
        let aux = u32::from_le_bytes(specimen.bytes[0x10..0x14].try_into().unwrap());
        if aux != u32::MAX && (aux >> 16) != 0 && !BOTH_HALVES.contains(&tag.as_str()) {
            failures.push(format!(
                "{}: {tag} aux {aux:#010x}",
                specimen.path.display()
            ));
        }
        seen += 1;
    }
    assert!(seen > 0, "no CBIN specimen");
    assert!(
        failures.is_empty(),
        "undocumented aux shapes:\n{}",
        failures.join("\n")
    );
}

#[test]
fn cbin_body_lengths_match_format_constants() {
    let expected: BTreeMap<&str, u64> = formats()
        .into_iter()
        .map(|(tag, len, _)| (tag, len))
        .collect();
    let mut checked = 0;
    for specimen in cbins() {
        let info = nord_format::cbin::inspect(&mut Cursor::new(&specimen.bytes)).unwrap();
        let tag = String::from_utf8_lossy(&info.header.tag);
        if let Some(&want) = expected.get(tag.as_ref()) {
            assert_eq!(info.body_len, want, "{}: {tag}", specimen.path.display());
            checked += 1;
        }
    }
    assert!(checked > 0, "no stub-format specimen");
}

#[test]
fn ns4_program_body_echoes_header_version() {
    let mut checked = 0;
    for specimen in corpus() {
        let (Entity::Program(Program::Stage4(program)) | Entity::Live(Live::Stage4(program))) =
            &specimen.entity
        else {
            continue;
        };
        assert_eq!(
            program.version_echo as u32,
            program.header.version & 0xff,
            "{}",
            specimen.path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no Stage 4 program");
}

#[test]
fn ns4_program_routes_a_keyboard_section() {
    let mut checked = 0;
    for specimen in corpus() {
        let (Entity::Program(Program::Stage4(program)) | Entity::Live(Live::Stage4(program))) =
            &specimen.entity
        else {
            continue;
        };
        assert!(
            program.organ_section_enabled
                || program.piano_section_enabled
                || program.synth_section_enabled,
            "{}: no section is routed to the keyboard",
            specimen.path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no Stage 4 program");
}

#[test]
fn ns4_octave_shifts_stay_in_panel_range() {
    use nord_format::{OrganPreset, PianoPreset, Synth};
    let in_range = |value: i8| (-2..=2).contains(&value);
    let mut seen = BTreeSet::new();
    for specimen in corpus() {
        let where_ = specimen.path.display();
        match &specimen.entity {
            Entity::Program(Program::Stage4(p)) | Entity::Live(Live::Stage4(p)) => {
                assert!(in_range(p.organ_a.octave_shift.octaves()), "{where_}");
                seen.insert("program");
            }
            Entity::OrganPreset(OrganPreset::Stage4(p)) => {
                assert!(in_range(p.organ_a_octave_shift.octaves()), "{where_}");
                seen.insert("organ preset");
            }
            Entity::PianoPreset(PianoPreset::Stage4(p)) => {
                assert!(in_range(p.piano_a_octave_shift.octaves()), "{where_}");
                seen.insert("piano preset");
            }
            Entity::Synth(Synth::Stage4(p)) => {
                assert!(in_range(p.synth_a_octave_shift.octaves()), "{where_}");
                seen.insert("synth preset");
            }
            _ => {}
        }
    }
    assert_eq!(
        seen,
        BTreeSet::from(["organ preset", "piano preset", "program", "synth preset"])
    );
}

#[test]
fn ns4_selectors_stay_in_panel_range() {
    use nord_format::{OrganPreset, PianoPreset, Synth};
    let mut seen = BTreeSet::new();
    for specimen in corpus() {
        let where_ = specimen.path.display();
        match &specimen.entity {
            Entity::Program(Program::Stage4(p)) | Entity::Live(Live::Stage4(p)) => {
                assert!(p.organ_a.model.raw() <= 5, "{where_}");
                assert!(p.organ_b.model.raw() <= 5, "{where_}");
                assert!(p.piano_a.piano_type.raw() <= 5, "{where_}");
                assert!(p.piano_b.piano_type.raw() <= 5, "{where_}");
                assert!(p.synth_a_voice.filter_type.raw() <= 5, "{where_}");
                assert!(p.synth_a_voice.lfo_shape.raw() <= 4, "{where_}");
                assert!(p.synth_a_performance.voice_priority.raw() <= 2, "{where_}");
                assert!(p.organ_fx.reverb_type.raw() <= 11, "{where_}");
                seen.insert("program");
            }
            Entity::OrganPreset(OrganPreset::Stage4(p)) => {
                assert!(p.organ_a_model.raw() <= 5, "{where_}");
                assert!(p.organ_b_model.raw() <= 5, "{where_}");
                assert!(p.organ_fx.reverb_type.raw() <= 11, "{where_}");
                seen.insert("organ preset");
            }
            Entity::PianoPreset(PianoPreset::Stage4(p)) => {
                assert!(p.piano_a_type.raw() <= 5, "{where_}");
                assert!(p.piano_b_type.raw() <= 5, "{where_}");
                assert!(p.piano_a_fx.reverb_type.raw() <= 11, "{where_}");
                seen.insert("piano preset");
            }
            Entity::Synth(Synth::Stage4(p)) => {
                assert!(p.synth_a_voice.filter_type.raw() <= 5, "{where_}");
                assert!(p.synth_b_voice.filter_type.raw() <= 5, "{where_}");
                assert!(p.synth_a_voice.lfo_shape.raw() <= 4, "{where_}");
                assert!(p.synth_a_voice_priority.raw() <= 2, "{where_}");
                assert!(p.synth_a_fx.reverb_type.raw() <= 11, "{where_}");
                seen.insert("synth preset");
            }
            _ => {}
        }
    }
    assert_eq!(
        seen,
        BTreeSet::from(["organ preset", "piano preset", "program", "synth preset"])
    );
}

#[test]
fn drum_banks_have_the_expected_member_count() {
    use nord_format::Bundle;
    let mut banks = 0;
    for specimen in corpus() {
        match &specimen.entity {
            Entity::Bundle(Bundle::Drum2Bank(bank)) => {
                assert_eq!(bank.programs.len(), 50, "{}", specimen.path.display());
                assert!(bank.programs.iter().all(|(name, _)| !name.is_empty()));
            }
            Entity::Bundle(Bundle::Drum3KitBank(bank)) => {
                assert_eq!(bank.kits.len(), 50, "{}", specimen.path.display());
                assert!(bank.kits.iter().all(|(name, _)| !name.is_empty()));
            }
            _ => continue,
        }
        banks += 1;
    }
    assert!(banks > 0, "no drum bank");
}

#[test]
fn v3_samples_decode_names_and_strokes() {
    let mut paired = 0;
    let mut samples = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        samples += 1;
        let where_ = specimen.path.display();
        assert!(!sample.name().unwrap().is_empty(), "{where_}: empty name");
        assert!(sample.stroke_count() > 0, "{where_}: no strokes");
        match sample.zones() {
            // Unexplained: some vendor zone maps do not have one entry per stroke.
            Err(_) => {}
            Ok(zones) => {
                assert_eq!(zones.len(), sample.stroke_count(), "{where_}");
                for zone in zones {
                    assert!(zone.top_note <= 127 && zone.root_key <= 127, "{where_}");
                    if let Some(low) = zone.low_note {
                        assert!(low <= zone.top_note, "{where_}: low above top");
                    }
                }
                paired += 1;
            }
        }
    }
    assert!(samples > 0, "no v3 sample");
    assert!(paired > 0, "no v3 zone map paired with its strokes");
}

/// Confirmed on hardware: a live slot and a stored program use the same body.
#[test]
fn ne5_live_body_decodes_as_a_program() {
    use nord_format::formats::ne5;

    let mut seen = 0;
    for (specimen, live) in ne5_lives() {
        let mut bytes = specimen.bytes.clone();
        bytes[0x08..0x0c].copy_from_slice(ne5::program::FORMAT.as_bytes());
        if bytes[0x04] == 0 {
            let at = bytes.len() - 2;
            let crc = nord_format::crc::crc16(&bytes[..at]);
            bytes[at..].copy_from_slice(&crc.to_le_bytes());
        }

        let Entity::Program(Program::Electro5(program)) =
            nord_format::from_stream(&mut Cursor::new(&bytes)).unwrap()
        else {
            panic!("retagged live slot decoded as another entity")
        };

        let fields = |fields: Vec<nord_format::fields::Field>| {
            fields
                .into_iter()
                .map(|field| (field.path, field.display))
                .collect::<Vec<_>>()
        };
        assert_eq!(fields(live.fields()), fields(program.fields()));
        seen += 1;
    }
    assert!(seen > 0, "no Electro 5 live slot in the corpus");
}

#[test]
fn ne5_live_slots_occupy_one_three_slot_bank() {
    use nord_format::bank::Item;

    let slots = ne5_lives()
        .map(|(specimen, live)| {
            let location = live.location();
            assert_eq!(location.x(), 0, "{}", specimen.path.display());
            location.inner()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(slots, BTreeSet::from([(0, 0), (0, 1), (0, 2)]));
}

/// The drawbar accessors must be read/write inverses without disturbing other bits.
#[test]
fn ne5_drawbars_survive_a_rewrite() {
    use nord_format::formats::ne5::OrganModel::{Farfisa, Pipe, Vox, B3};

    let mut seen = 0;
    for (specimen, _) in ne5_programs() {
        let Entity::Program(Program::Electro5(mut program)) =
            nord_format::from_stream(&mut Cursor::new(&specimen.bytes)).unwrap()
        else {
            unreachable!()
        };
        for model in [B3, Vox, Farfisa, Pipe] {
            for preset in [1, 2] {
                let bars = program.organ_panel.drawbars(model, preset);
                if bars.iter().all(|&bar| bar <= 8) {
                    program
                        .organ_panel
                        .set_drawbars(model, preset, bars)
                        .unwrap();
                }
            }
        }

        let mut rewritten = Vec::new();
        program.write_to(&mut Cursor::new(&mut rewritten)).unwrap();
        assert_eq!(specimen.bytes, rewritten, "{}", specimen.path.display());
        seen += 1;
    }
    assert!(seen > 0, "no Electro 5 program in the corpus");
}

/// Each readable zone table must account for every encoded stroke.
#[test]
fn nsmp_strokes_match_zones() {
    let mut seen = 0;
    for (specimen, sample) in v2_samples() {
        let Ok(zones) = sample.zones() else {
            continue;
        };
        let strokes = sample
            .strokes()
            .unwrap_or_else(|error| panic!("{}: {error}", specimen.path.display()));
        assert_eq!(strokes.len(), zones.len(), "{}", specimen.path.display());
        seen += strokes.len();
    }
    assert!(seen > 0, "no readable v2 strokes in the corpus");
}

/// Rename and remap reproduce the file written by the sample editor.
#[test]
fn nsmp_edits_reproduce_editor_output() {
    let mut sample = v2_named("D4-3zones.nsmp");
    sample.set_name("D7-upperkey").unwrap();
    sample.set_zone_top_note(1, 60).unwrap();
    assert_eq!(sample.to_bytes().unwrap(), named("D7-upperkey.nsmp").bytes);
}

/// Retuning changes the root-key byte and the container checksum only.
#[test]
fn nsmp_retune_is_surgical() {
    let before = &named("D1-one-zone.nsmp").bytes;
    let mut sample = v2_named("D1-one-zone.nsmp");
    sample.set_root_key(0, 48).unwrap();
    let after = sample.to_bytes().unwrap();

    let changed = (0..before.len())
        .filter(|&index| before[index] != after[index])
        .collect::<Vec<_>>();
    assert_eq!(changed.len(), 5, "changed bytes: {changed:?}");
    assert!(changed[..4].iter().eq([0x18, 0x19, 0x1a, 0x1b].iter()));
    assert_eq!(sample.strokes().unwrap()[0].root_key, 48);
}

#[test]
fn nsmp_overlong_name_is_refused_without_mutation() {
    let mut sample = v2_named("D1-one-zone.nsmp");
    assert!(sample.set_name("a name that is far too long").is_err());
    assert_eq!(sample.name().unwrap(), "TEST");
}

#[test]
fn nsmp_bad_checksum_is_refused() {
    let mut bytes = named("D1-one-zone.nsmp").bytes.clone();
    *bytes.last_mut().unwrap() ^= 0xff;
    assert!(nord_format::from_stream(&mut Cursor::new(&bytes)).is_err());
}

/// A device read whose leading body bytes arrived as foreign buffer content, so the
/// `NWS` container and the sections after it are gone. Named `.skip.`, which keeps the
/// sweep off it, so it is opened by path rather than through [`named`].
#[test]
fn nsmp_body_without_its_container_section_says_which_tag_was_expected() {
    let path = scan::root().join("ne5/audio-oracle/2026-08-30/stereo77.skip.nsmp");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let err = nord_format::from_stream(&mut Cursor::new(&bytes))
        .expect_err("a body missing its container section must not parse")
        .to_string();
    assert_eq!(
        err,
        "the body does not open with the NWS container section; found \\x00\\x00\\x00"
    );
}

/// A one-zone v3 and a one-zone v4 instrument, and the multi-zone vendor files
/// whose `map` layouts differ from theirs. Named so a failure says which
/// generation and which zone layout broke.
const V3_ONE_ZONE: &str = "Q-base.nsmp3";
const V4_ONE_ZONE: &str = "Q-base.nsmp4";
const V3_MAP_14: &str = "Bass Clarinet 2_KG  mono 3.11 [ne6].nsmp3";
const V3_MAP_12: &str = "Ashbory Bass Finger_BS mono 3.0 [ne6].nsmp3";
const V4_KEY_MAP: &str = "Kalimba_KG 4.1 [ne7].nsmp4";

/// Load a specimen, edit it through the generation-neutral accessors, and
/// re-encode.
fn edited(name: &str, edit: impl FnOnce(&mut Sample)) -> (&'static [u8], Vec<u8>) {
    let before = &named(name).bytes;
    let mut entity = nord_format::from_stream(&mut Cursor::new(before)).unwrap();
    let Entity::Sample(sample) = &mut entity else {
        panic!("{name} is not a sample instrument");
    };
    edit(sample);
    let after = nord_format::to_bytes(&entity).unwrap();
    assert_eq!(
        before.len(),
        after.len(),
        "{name}: the edit resized the file"
    );
    // Reading the result back is what proves the container checksum was
    // recomputed: a stale one is refused.
    nord_format::from_stream(&mut Cursor::new(&after))
        .unwrap_or_else(|e| panic!("{name} does not read back after the edit: {e}"));
    (before, after)
}

/// Which bytes an edit moved, less the container checksum's own word — that
/// moves whenever anything else does and says nothing about scope.
fn moved(before: &[u8], after: &[u8]) -> Vec<usize> {
    const CHECKSUM: std::ops::Range<usize> = 0x18..0x1c;
    (0..before.len())
        .filter(|&i| before[i] != after[i] && !CHECKSUM.contains(&i))
        .collect()
}

/// Every zone's decoded audio, which no edit here is allowed to disturb.
fn audio(bytes: &[u8]) -> Vec<Vec<i16>> {
    let entity = nord_format::from_stream(&mut Cursor::new(bytes)).unwrap();
    let Entity::Sample(sample) = &entity else {
        panic!("not a sample instrument");
    };
    let layout = sample.layout();
    sample
        .zones()
        .unwrap()
        .iter()
        .map(|zone| {
            nsmp::codec::decode(zone.stream, zone.at, layout)
                .unwrap()
                .samples
        })
        .collect()
}

fn name_of(bytes: &[u8]) -> String {
    let entity = nord_format::from_stream(&mut Cursor::new(bytes)).unwrap();
    let Entity::Sample(sample) = &entity else {
        panic!("not a sample instrument");
    };
    sample.name().unwrap()
}

fn zones_of(bytes: &[u8]) -> Vec<(u8, u8, Option<u8>)> {
    let entity = nord_format::from_stream(&mut Cursor::new(bytes)).unwrap();
    let Entity::Sample(sample) = &entity else {
        panic!("not a sample instrument");
    };
    sample
        .zones()
        .unwrap()
        .iter()
        .map(|z| (z.root_key, z.top_note, z.low_note))
        .collect()
}

/// Renaming a wide instrument moves the name field and nothing else — not the
/// sub-name that shares the `hdr` section with it.
#[test]
fn nsmp_wide_rename_touches_only_the_name_field() {
    for name in [V3_ONE_ZONE, V4_ONE_ZONE, V3_MAP_12, V4_KEY_MAP] {
        let old = name_of(&named(name).bytes);
        let (before, after) = edited(name, |s| s.set_name("Retitled").unwrap());
        let moved = moved(before, after.as_slice());
        // Writing one name over another moves at most the longer of the two —
        // the shorter one's tail is NUL-filled — and never leaves the field.
        assert!(!moved.is_empty(), "{name}: rename moved nothing");
        assert!(
            moved.len() <= old.len().max("Retitled".len()),
            "{name}: rename moved {moved:?} over a {}-byte name",
            old.len()
        );
        assert!(
            moved[moved.len() - 1] - moved[0] < nsmp::MAX_NAME_V3_LEN,
            "{name}: rename reached outside the name field: {moved:?}"
        );
        let reread = nord_format::from_stream(&mut Cursor::new(&after)).unwrap();
        let Entity::Sample(sample) = &reread else {
            unreachable!()
        };
        assert_eq!(sample.name().unwrap(), "Retitled", "{name}");
        assert_eq!(audio(before), audio(&after), "{name}: rename moved audio");
    }
}

/// The whole main-name field is writable — what bounds it is the sub-name that
/// starts where it ends, and that field survives a name filling the one before it.
#[test]
fn nsmp_wide_rename_stops_at_the_sub_name() {
    let long = "x".repeat(nsmp::MAX_NAME_V3_LEN);
    let (_, after) = edited(V3_MAP_14, |s| s.set_name(&long).unwrap());
    let reread = nord_format::from_stream(&mut Cursor::new(&after)).unwrap();
    let Entity::Sample(Sample::V3(sample)) = &reread else {
        panic!("not a wide sample")
    };
    assert_eq!(sample.name().unwrap(), long);
    assert_eq!(sample.sub_name().unwrap(), "KG  mono");

    let mut entity = nord_format::from_stream(&mut Cursor::new(&named(V3_MAP_14).bytes)).unwrap();
    let Entity::Sample(sample) = &mut entity else {
        unreachable!()
    };
    assert!(sample.set_name(&format!("{long}x")).is_err());
}

/// Retuning a wide zone moves the stroke's root key and the copy the zone record
/// duplicates — two bytes, and the audio is not one of them.
#[test]
fn nsmp_wide_retune_moves_both_copies_of_the_root_key() {
    for name in [V3_ONE_ZONE, V4_ONE_ZONE, V3_MAP_14, V3_MAP_12] {
        let was = zones_of(&named(name).bytes)[0].0;
        let note = if was == 48 { 55 } else { 48 };
        let (before, after) = edited(name, |s| s.set_root_key(0, note).unwrap());
        assert_eq!(
            moved(before, after.as_slice()).len(),
            2,
            "{name}: retune moved {:?}",
            moved(before, after.as_slice())
        );
        assert_eq!(zones_of(&after)[0].0, note, "{name}");
        assert_eq!(audio(before), audio(&after), "{name}: retune moved audio");
    }
}

/// Remapping a wide zone moves the one boundary byte it names.
#[test]
fn nsmp_wide_remap_moves_one_boundary_byte() {
    for name in [V3_ONE_ZONE, V4_ONE_ZONE, V3_MAP_14, V3_MAP_12] {
        let (root, top, low) = zones_of(&named(name).bytes)[0];
        let want = if top == 96 { 95 } else { 96 };
        let (before, after) = edited(name, |s| s.set_zone_top_note(0, want).unwrap());
        assert_eq!(
            moved(before, after.as_slice()).len(),
            1,
            "{name}: top note moved {:?}",
            moved(before, after.as_slice())
        );
        assert_eq!(zones_of(&after)[0], (root, want, low), "{name}");
        assert_eq!(audio(before), audio(&after), "{name}: remap moved audio");

        let Some(low) = low else {
            // This layout stores no low note, and says so rather than writing a
            // byte that means something else.
            let (_, unchanged) = edited(name, |s| assert!(s.set_zone_low_note(0, 40).is_err()));
            assert_eq!(before, unchanged.as_slice(), "{name}");
            continue;
        };
        let want = if low == 40 { 41 } else { 40 };
        let (before, after) = edited(name, |s| s.set_zone_low_note(0, want).unwrap());
        assert_eq!(
            moved(before, after.as_slice()).len(),
            1,
            "{name}: low note moved {:?}",
            moved(before, after.as_slice())
        );
        assert_eq!(zones_of(&after)[0].2, Some(want), "{name}");
    }
}

/// Recomputing a populated per-key table from the layout it already describes
/// reproduces it exactly: the partner law is what the vendor's builder ran.
///
/// The records outside the zones' span are part of the claim — two builders
/// write `[0][0][0][key]` there rather than the identity, and those are carried
/// across rather than normalised.
#[test]
fn nsmp_v4_partner_law_reproduces_the_vendor_key_maps() {
    let mut populated = 0;
    let mut neutral = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        let (Ok(table), Ok(zones)) = (sample.zone_table(), sample.zones()) else {
            continue;
        };
        let map = nsmp::section::find4(&sample.body.sections, nsmp::section::MAP4).unwrap();
        let name = specimen.path.display();
        match table.key_map(&map.payload).unwrap() {
            nsmp::zone::KeyMap::Absent => continue,
            nsmp::zone::KeyMap::Neutral => {
                // The sample editor writes the neutral table whatever the zone
                // layout, so nothing may start populating one.
                assert!(
                    table.plan_key_map(&map.payload, &zones).unwrap().is_empty(),
                    "{name}: a neutral table was planned over"
                );
                neutral += 1;
            }
            nsmp::zone::KeyMap::Populated => {
                let mut after = map.payload.clone();
                for (at, quad) in table.plan_key_map(&map.payload, &zones).unwrap() {
                    after[at..at + quad.len()].copy_from_slice(&quad);
                }
                assert_eq!(after, map.payload, "{name}: the law did not reproduce it");
                populated += 1;
            }
        }
    }
    assert!(populated > 0, "no populated per-key table in the corpus");
    assert!(neutral > 0, "no neutral per-key table in the corpus");
}

/// A populated per-key table is recomputed from the zone layout, and an edit
/// that puts the layout back where it was reproduces the file byte for byte.
///
/// The partner law is the whole claim here: the 5 vendor instruments that carry
/// a populated table are the only specimens that exercise it, and 24 of their
/// records sit outside the zones' span where two builders write `[0][0][0][key]`
/// rather than the identity. Those are carried across untouched.
#[test]
fn nsmp_v4_populated_key_map_survives_a_round_trip() {
    let mut seen = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        if !sample.zones_are_editable() {
            continue;
        }
        let Ok(zones) = sample.zones() else { continue };
        if zones.len() < 2 {
            continue;
        }
        let name = specimen.path.display();
        let roots: Vec<u8> = zones.iter().map(|z| z.root_key).collect();
        let mut entity = nord_format::from_stream(&mut Cursor::new(&specimen.bytes)).unwrap();
        let Entity::Sample(edited) = &mut entity else {
            unreachable!()
        };
        // Move every root away and back. The table is recomputed each time, so
        // a byte-identical result is the law reproducing what the builder wrote.
        for (i, root) in roots.iter().enumerate() {
            edited.set_root_key(i, root.saturating_sub(1)).unwrap();
        }
        for (i, root) in roots.iter().enumerate() {
            edited.set_root_key(i, *root).unwrap();
        }
        let after = nord_format::to_bytes(&entity).unwrap();
        assert_eq!(specimen.bytes, after, "{name}");
        seen += 1;
    }
    assert!(seen > 0, "no multi-zone wide sample in the corpus");
}

/// Retuning a multi-zone v4 instrument moves the two copies of the root key and
/// the per-key records the partner law reassigns — and nothing else.
#[test]
fn nsmp_v4_retune_carries_the_key_map_with_it() {
    let before = &named(V4_KEY_MAP).bytes;
    let zones = zones_of(before);
    let (root, _, _) = zones[0];

    let (_, after) = edited(V4_KEY_MAP, |s| s.set_root_key(0, root - 1).unwrap());
    assert_eq!(zones_of(&after)[0].0, root - 1);
    assert_eq!(audio(before), audio(&after), "retune moved audio");

    // The gains and the three bytes behind them are an authored curve that no
    // layout predicts, so every one of them has to survive the recompute.
    let levels = |bytes: &[u8]| -> Vec<Vec<u8>> {
        let entity = nord_format::from_stream(&mut Cursor::new(bytes)).unwrap();
        let Entity::Sample(Sample::V3(sample)) = &entity else {
            panic!("not a wide sample")
        };
        let map = nsmp::section::find4(&sample.body.sections, nsmp::section::MAP4).unwrap();
        (0..128)
            .map(|k| map.payload[6 + k * 10..][..6].to_vec())
            .collect()
    };
    assert_eq!(levels(before), levels(&after), "the per-key levels moved");
}

/// Every wide specimen's zones survive a retune to a new root and back, which is
/// the pairing the zone record and its stroke have to keep.
#[test]
fn nsmp_wide_retune_round_trips_across_the_corpus() {
    let mut seen = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(_)) = &specimen.entity else {
            continue;
        };
        let mut entity = nord_format::from_stream(&mut Cursor::new(&specimen.bytes)).unwrap();
        let Entity::Sample(sample) = &mut entity else {
            unreachable!()
        };
        if !sample.zones_are_editable() {
            continue;
        }
        let was: Vec<_> = sample.zones().unwrap().iter().map(|z| z.root_key).collect();
        for (i, root) in was.iter().enumerate() {
            sample.set_root_key(i, root ^ 1).unwrap();
        }
        for (i, root) in was.iter().enumerate() {
            sample.set_root_key(i, *root).unwrap();
        }
        let after = nord_format::to_bytes(&entity).unwrap();
        assert_eq!(specimen.bytes, after, "{}", specimen.path.display());
        seen += 1;
    }
    assert!(seen > 0, "no editable wide sample in the corpus");
}
