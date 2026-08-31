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
fn built_zones(project: &nsmpproj::Project) -> Vec<(u32, u8, u8, Vec<i16>)> {
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
            (layer.global_id, zone.root_key, zone.top_note, audio)
        })
        .collect()
}

#[test]
fn nsmp_building_a_project_reproduces_its_editor_twin() {
    // Expected bytes come from paired Sample Editor project/instrument fixtures.
    for name in ["D3-2zones", "D4-3zones", "D8-2zones-hi", "D7-upperkey"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let zones = built_zones(project);
        let built = nsmp::encode::multi_zone(
            &zones
                .iter()
                .map(
                    |(global_id, root_key, top_note, audio)| nsmp::encode::NewZone {
                        source: audio,
                        root_key: *root_key,
                        top_note: *top_note,
                        global_id: *global_id,
                    },
                )
                .collect::<Vec<_>>(),
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
        for (index, (_, _, _, audio)) in built_zones(project).iter().enumerate() {
            let (at, stream) = twin.zone_stream(index).unwrap();
            let editor = nsmp::codec::decode(stream, at, nsmp::codec::Layout::V2).unwrap();
            assert_eq!(
                nsmp::encode::Plan::new(audio.len()).unwrap().fields,
                editor.samples.len(),
                "{name} zone {index}: {} frames",
                audio.len()
            );
        }
    }
}

#[test]
fn nsmp_a_built_instrument_walks_and_agrees_with_its_directory() {
    for name in ["D3-2zones", "D4-3zones", "D8-2zones-hi", "D7-upperkey"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let zones = built_zones(project);
        let built = nsmp::encode::multi_zone(
            &zones
                .iter()
                .map(
                    |(global_id, root_key, top_note, audio)| nsmp::encode::NewZone {
                        source: audio,
                        root_key: *root_key,
                        top_note: *top_note,
                        global_id: *global_id,
                    },
                )
                .collect::<Vec<_>>(),
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
