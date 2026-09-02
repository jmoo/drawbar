#![cfg(feature = "corpus")]
//! Behavioral checks against files produced by the instrument and sample editor.

use nord_format::formats::{nsmp, nsmpproj};
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

fn projects() -> impl Iterator<Item = (&'static Specimen, &'static nsmpproj::Project)> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::SampleProject(p) => Some((s, p)),
        _ => None,
    })
}

#[test]
fn nsmpproj_stroke_fields_move_alone() {
    use nsmpproj::StrokeField as F;

    let mut seen = 0;
    for (specimen, project) in projects() {
        let at = specimen.path.display();
        let before = project.render();
        for stroke in project.strokes().unwrap() {
            // A probe has to differ from what the stroke holds, or nothing moves.
            let fields = [
                ("start", F::Start(3.0)),
                ("stop", F::Stop(4000.0)),
                ("gain", F::Gain(0.75)),
                ("velocity_min", F::VelocityMin(10)),
                ("velocity_max", F::VelocityMax(100)),
                ("loop_enabled", F::LoopEnabled(!stroke.loop_enabled)),
                ("loop_start", F::LoopStart(1234.5)),
                ("loop_length", F::LoopLength(600.0)),
                ("loop_crossfade", F::LoopCrossfade(90.0)),
                ("loop_crossfade_mode", F::LoopCrossfadeMode(1)),
                (
                    "loop_decay_enabled",
                    F::LoopDecayEnabled(!stroke.loop_decay_enabled),
                ),
                ("loop_decay", F::LoopDecay(3.25)),
                ("loop_detune", F::LoopDetune(-12)),
                (
                    "short_loop_enabled",
                    F::ShortLoopEnabled(!stroke.short_loop_enabled),
                ),
                ("short_loop_length", F::ShortLoopLength(64.0)),
                ("short_loop_crossfade", F::ShortLoopCrossfade(5)),
                (
                    "short_loop_uses_pitch",
                    F::ShortLoopUsesPitch(!stroke.short_loop_uses_pitch),
                ),
            ];
            for (name, field) in fields {
                let mut edited = project.clone();
                edited.set_stroke_field(stroke.global_id, field).unwrap();
                let after = edited.render();
                let changed = before
                    .lines()
                    .zip(after.lines())
                    .filter(|(a, b)| a != b)
                    .count();
                assert_eq!(changed, 1, "{at}: stroke {} {name}", stroke.global_id);
                assert_eq!(
                    before.lines().count(),
                    after.lines().count(),
                    "{at}: {name}"
                );
            }
        }
        seen += 1;
    }
    assert!(seen > 0, "no sample-editor project in the corpus");
}

#[test]
fn nsmpproj_velocity_defaults_move_alone() {
    for (specimen, project) in projects() {
        let before = project.render();
        let mut edited = project.clone();
        let defaults = nsmpproj::VelocityDefaults {
            attack_amount: 64,
            amplitude: 0,
            timbre: 0,
        };
        edited.set_velocity_defaults(defaults).unwrap();
        assert_eq!(edited.velocity_defaults().unwrap(), defaults);
        let after = edited.render();
        let changed = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(changed, 3, "{}", specimen.path.display());
    }
}

fn project_named(name: &str) -> &'static nsmpproj::Project {
    match &named(name).entity {
        Entity::SampleProject(project) => project,
        other => panic!("{name} decoded as {other:?}"),
    }
}

/// A project's zones, each with audio of the length the project gives it.
///
/// The editor's own WAVs are not corpus material, so the audio is generated. Only the
/// frame count reaches anything asserted below: a stroke's field count comes from its
/// length, and every other field compared is metadata.
fn built_zones(project: &nsmpproj::Project) -> Vec<BuiltZone> {
    let strokes = project.strokes().unwrap();
    project
        .zones()
        .unwrap()
        .iter()
        .map(|zone| {
            let layer = &zone.strokes[0];
            let stroke = strokes
                .iter()
                .find(|s| s.global_id == layer.global_id)
                .unwrap_or_else(|| panic!("no stroke {}", layer.global_id));
            let frames = (stroke.stop - stroke.start) as usize;
            let audio = (0..frames).map(|k| (k % 512) as i16 * 16 - 4096).collect();
            BuiltZone {
                global_id: layer.global_id,
                root_key: zone.root_key,
                top_note: zone.top_note,
                audio,
                secondary_start: stroke.encoded_secondary_start() - stroke.start,
            }
        })
        .collect()
}

struct BuiltZone {
    global_id: u32,
    root_key: u8,
    top_note: u8,
    audio: Vec<i16>,
    secondary_start: f64,
}

impl BuiltZone {
    fn new_zone(&self) -> nsmp::encode::NewZone<'_> {
        nsmp::encode::NewZone {
            source: &self.audio,
            channels: 1,
            root_key: self.root_key,
            top_note: self.top_note,
            global_id: self.global_id,
            loops: None,
            secondary_start: self.secondary_start,
            shift: None,
            gain: nsmp::zone::GAIN_UNITY,
        }
    }
}

#[test]
fn nsmp_building_a_project_reproduces_its_editor_twin() {
    // Expected bytes come from paired Sample Editor project/instrument fixtures.
    for name in ["D3-2zones", "D4-3zones", "D8-2zones-hi", "D7-upperkey"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let zones = built_zones(project);
        let built = nsmp::encode::multi_zone(
            &zones.iter().map(BuiltZone::new_zone).collect::<Vec<_>>(),
            &project.name().unwrap(),
            nsmp::encode::Predictor::Minimising,
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));

        let twin = v2_named(&format!("{name}.nsmp"));
        let ours = built.to_bytes().unwrap();
        assert_eq!(
            &ours[..0x18],
            &named(&format!("{name}.nsmp")).bytes[..0x18],
            "{name}: container header"
        );

        let sections = |body: &nsmp::Sample| -> Vec<(String, u8, usize)> {
            body.sections
                .iter()
                .map(|s| (s.tag_str(), s.version, s.payload.len()))
                .collect()
        };
        let theirs = sections(&twin.body);
        let mine = sections(&built.body);
        assert_eq!(
            theirs.iter().map(|s| (&s.0, s.1)).collect::<Vec<_>>(),
            mine.iter().map(|s| (&s.0, s.1)).collect::<Vec<_>>(),
            "{name}: section chain"
        );
        for tag in [nsmp::section::HDR, nsmp::section::CAT, nsmp::section::MAP] {
            assert_eq!(
                nsmp::section::find(&built.body.sections, tag).map(|s| &s.payload),
                nsmp::section::find(&twin.body.sections, tag).map(|s| &s.payload),
                "{name}: {} section",
                String::from_utf8_lossy(tag)
            );
        }

        assert_eq!(built.name().unwrap(), twin.name().unwrap(), "{name}: name");
        assert_eq!(
            built.zones().unwrap(),
            twin.zones().unwrap(),
            "{name}: zone table"
        );
        assert_eq!(
            built
                .strokes()
                .unwrap()
                .iter()
                .map(|s| s.root_key)
                .collect::<Vec<_>>(),
            twin.strokes()
                .unwrap()
                .iter()
                .map(|s| s.root_key)
                .collect::<Vec<_>>(),
            "{name}: root keys"
        );
    }
}

#[test]
fn nsmp_a_built_zone_is_as_long_as_the_editors() {
    // Expected lengths come from the Sample Editor instrument fixtures.
    for name in ["D1-one-zone", "D3-2zones", "D4-3zones", "D8-2zones-hi"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let twin = v2_named(&format!("{name}.nsmp"));
        for (index, zone) in built_zones(project).iter().enumerate() {
            let (at, stream) = twin.zone_stream(index).unwrap();
            let editor = nsmp::codec::decode(stream, at, nsmp::codec::Layout::V2).unwrap();
            assert_eq!(
                nsmp::encode::Plan::new(zone.audio.len(), 1, zone.secondary_start)
                    .unwrap()
                    .fields,
                editor.samples.len(),
                "{name} zone {index}: {} frames",
                zone.audio.len()
            );
        }
    }
}

#[test]
fn nsmp_statistic_a_is_the_file_peaks_reciprocal_scaled_by_the_zones_gain() {
    // Every self-generated v2 specimen, whatever its gain. Library instruments are left
    // out: their strokes keep the mantissa of whatever file first encoded them.
    let layout = nsmp::codec::Layout::V2;
    let mut seen = 0;
    for (specimen, sample) in v2_samples() {
        if !specimen
            .path
            .components()
            .any(|c| c.as_os_str() == "samples")
        {
            continue;
        }
        let zones = sample.zones().unwrap();
        let streams = sample.stroke_streams();
        let peak = streams
            .iter()
            .filter_map(|(_, s)| nsmp::codec::peak(s, layout))
            .max()
            .unwrap_or(0) as u32;
        for (_, stroke) in streams {
            let gain = zones
                .iter()
                .find(|z| z.stroke_id == stroke[3])
                .map_or(nsmp::zone::GAIN_UNITY, |z| z.gain);
            let (mantissa, _) = nsmp::encode::statistic_a(peak, 0, gain);
            assert_eq!(
                stroke[9..12],
                mantissa.to_be_bytes()[1..],
                "{} stroke {} at gain {gain}",
                specimen.path.display(),
                stroke[3]
            );
            seen += 1;
        }
    }
    assert!(seen > 1000, "{seen} strokes");
}

#[test]
fn nsmp_a_built_instrument_walks_and_agrees_with_its_directory() {
    for name in ["D3-2zones", "D4-3zones", "D8-2zones-hi", "D7-upperkey"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let zones = built_zones(project);
        let built = nsmp::encode::multi_zone(
            &zones.iter().map(BuiltZone::new_zone).collect::<Vec<_>>(),
            &project.name().unwrap(),
            nsmp::encode::Predictor::Minimising,
        )
        .unwrap();

        let map_len = nsmp::section::find(&built.body.sections, nsmp::section::MAP)
            .unwrap()
            .payload
            .len();
        let cat_len = nsmp::section::find(&built.body.sections, nsmp::section::CAT)
            .unwrap()
            .payload
            .len();
        for (index, (at, stream)) in built.stroke_streams().iter().enumerate() {
            let head = nsmp::stroke::header_len(index, cat_len, map_len);
            assert_eq!(
                (stream.len() - head) % nsmp::stroke::PACKET_LEN,
                0,
                "{name} stroke {index}: {} bytes over a {head}-byte header",
                stream.len()
            );
            let walk = nsmp::codec::walk(stream, *at, nsmp::codec::Layout::V2)
                .unwrap_or_else(|e| panic!("{name} stroke {index}: {e}"));
            let directory = nsmp::codec::Directory::read(stream).unwrap();
            let resolve = |p| nsmp::codec::Directory::resolve(p, *at, nsmp::codec::Layout::V2);
            assert_eq!(
                resolve(directory.first_record),
                walk.first_record,
                "{name} stroke {index}: first record"
            );
            assert_eq!(
                resolve(directory.terminator),
                walk.terminator,
                "{name} stroke {index}: terminator"
            );
            assert!(
                walk.records
                    .iter()
                    .any(|r| r.at == resolve(directory.resync)),
                "{name} stroke {index}: resync names no record"
            );
        }
    }
}

fn v2_map_payload(sample: &nord_format::cbin::Cbin<nsmp::Sample>) -> Vec<u8> {
    sample
        .body
        .sections
        .iter()
        .find(|s| s.is(nsmp::section::MAP))
        .expect("a v2 instrument has a map section")
        .payload
        .clone()
}

#[test]
fn v2_keyboard_maps_round_trip_byte_exactly() {
    let mut seen = 0;
    for (specimen, sample) in v2_samples() {
        if sample.header.version < nsmp::LIBRARY_2_VERSION {
            continue;
        }
        let payload = v2_map_payload(sample);
        let table = sample
            .key_table()
            .unwrap_or_else(|e| panic!("{}: {e}", specimen.path.display()));
        let mut copy = payload.clone();
        table.write(&mut copy).unwrap();
        assert_eq!(copy, payload, "{}", specimen.path.display());
        seen += 1;
    }
    assert!(seen > 0, "no Sample Library 2.0 instrument in the corpus");
}

#[test]
fn keyboard_map_records_read_as_the_editor_wrote_them() {
    use nsmp::keymap::{Level, GAIN_UNITY};
    let key = |name: &str, note: u8| v2_named(name).key_table().unwrap().key(note);
    let instrument = |name: &str| v2_named(name).key_table().unwrap().instrument;

    assert_eq!(
        v2_named("MN-00base.nsmp").key_table().unwrap(),
        nsmp::KeyTable::NEUTRAL
    );
    assert_eq!(
        key("MN-05ng60h.nsmp", 60),
        Level::new(GAIN_UNITY / 2, 0).unwrap()
    );
    assert_eq!(key("MN-06ng17h.nsmp", 17).gain, GAIN_UNITY / 2);
    assert_eq!(key("MN-07ng108h.nsmp", 108).gain, GAIN_UNITY / 2);
    assert_eq!(key("MN-01nd60p8.nsmp", 60).detune, 20);
    assert_eq!(key("MN-02nd60m8.nsmp", 60).detune, -20);
    assert_eq!(key("MN-03nd17p1.nsmp", 17).detune, 2);
    assert_eq!(key("MN-04nd108p1.nsmp", 108).detune, 2);
    // The editor's +9 dB ceiling and -9 dB floor on a key's gain.
    assert_eq!(key("MN-08ng60x4.nsmp", 60).gain, 0x2d_1819);
    assert_eq!(key("MN-09ng60tny.nsmp", 60).gain, 0x05_ad51);
    assert_eq!(instrument("MN-13mgn4.nsmp").gain, 0x2d_1819);
    assert_eq!(instrument("MN-14mgn001.nsmp").gain, 0x419);
    assert_eq!(instrument("MN-15mdt100.nsmp").detune, 256);
    assert_eq!(instrument("MN-16mdtm100.nsmp").detune, -256);
    assert_eq!(instrument("MN-17mdt1200.nsmp").detune, 3072);

    let macro3 = v2_named("MN-10mac3h.nsmp").key_table().unwrap();
    assert_eq!(
        macro3.adjusted().collect::<Vec<_>>(),
        (49..=71).collect::<Vec<_>>()
    );
    assert_eq!(macro3.key(60).gain, GAIN_UNITY / 2);
}

#[test]
fn setting_the_keyboard_map_touches_only_the_keyboard_map() {
    use nsmp::keymap::Level;
    let mut edited = v2_named("MN-05ng60h.nsmp");
    let strokes_before = edited.stroke_streams().len();
    let mut table = edited.key_table().unwrap();
    table.set_key(60, Level::NEUTRAL);
    edited.set_key_table(&table).unwrap();
    assert_eq!(
        v2_map_payload(&edited),
        v2_map_payload(&v2_named("MN-00base.nsmp"))
    );
    assert_eq!(edited.stroke_streams().len(), strokes_before);
    let bytes = edited.to_bytes().unwrap();
    let reread = nsmp::from_bytes(&bytes).unwrap();
    assert_eq!(reread.key_table().unwrap(), nsmp::KeyTable::NEUTRAL);
}

/// The corpus's table lists each tap as an `f32` with a class: `unique` where the
/// specimens pin a single `f32`, `excluded` where they rule out the closed form's own
/// rounding, `ideal` where the closed form's rounding is admissible but not proven, and
/// `zero` outside the support. The kernel must match the first two to the bit and the
/// third to within one ulp.
#[test]
fn nsmp_the_kernel_matches_the_corpus_f32_tap_table() {
    let path = scan::root().join("tools/nsmp-pitch/table-fl32.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let bank = nsmp::kernel::taps();
    let mut seen = 0;
    let mut pinned = 0;
    let mut ideal_off = 0;
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("k\t"))
    {
        let mut cols = line.split('\t');
        let k: usize = cols.next().unwrap().parse().unwrap();
        let m: i64 = cols.next().unwrap().parse().unwrap();
        let g: f32 = cols.next().unwrap().parse().unwrap();
        let class = cols.next().unwrap();
        let ours = match usize::try_from(m + 15) {
            Ok(j) if j < nsmp::kernel::TAPS => bank[k][j],
            _ => 0.0,
        };
        let ulps = if ours.is_sign_negative() == g.is_sign_negative() {
            i64::from(ours.to_bits()).abs_diff(i64::from(g.to_bits()))
        } else {
            u64::MAX
        };
        match class {
            "unique" | "excluded" => {
                assert_eq!(
                    ours.to_bits(),
                    g.to_bits(),
                    "{class} phase {k} m {m}: ours {ours} table {g}"
                );
                pinned += 1;
            }
            "ideal" => {
                assert!(ulps <= 1, "phase {k} m {m}: ours {ours} table {g}");
                ideal_off += usize::from(ulps == 1);
            }
            "zero" => assert_eq!(ours, 0.0, "phase {k} m {m}"),
            other => panic!("phase {k} m {m}: class {other}"),
        }
        seen += 1;
    }
    assert_eq!(seen, 512 * 32);
    assert_eq!(pinned, 100 + 88);
    println!("{ideal_off} ideal-class taps one ulp off the table");
}
