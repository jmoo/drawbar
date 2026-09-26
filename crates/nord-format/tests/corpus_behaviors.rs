#![cfg(feature = "corpus")]
//! Behavior checks against files written by the instruments and the Sample Editor.

use nord_format::formats::{npno, nsmp, nsmpproj};
use nord_format::{Entity, Live, Program, Sample};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

#[path = "support/format_table.rs"]
mod format_table;
#[path = "support/scan.rs"]
mod scan;
#[path = "support/sidecar.rs"]
mod sidecar;

use format_table::formats;
use scan::{corpus, named, v2_named, v2_samples, Specimen};

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
        .map(|(tag, len, _)| (tag, len as u64))
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
    assert!(checked > 0, "no specimen of a format in the table");
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
                assert!(in_range(p.organ_a.octave_shift.octaves()), "{where_}");
                seen.insert("organ preset");
            }
            Entity::PianoPreset(PianoPreset::Stage4(p)) => {
                assert!(in_range(p.piano_a.octave_shift.octaves()), "{where_}");
                seen.insert("piano preset");
            }
            Entity::Synth(Synth::Stage4(p)) => {
                assert!(
                    in_range(p.synth_a_performance.octave_shift.octaves()),
                    "{where_}"
                );
                seen.insert("synth preset");
            }
            _ => {}
        }
    }
    assert_eq!(
        seen,
        BTreeSet::from(["organ preset", "piano preset", "program", "synth preset"]),
        "Stage 4 entity kinds checked"
    );
}

/// Asserts that a Stage 4 selector holds a value the panel can select. `top` is the
/// highest stored value the panel reaches.
fn in_panel_range(specimen: &Specimen, field: &str, value: u8, top: u8) {
    assert!(
        value <= top,
        "{}: {field} = {value}, and the panel stops at {top}",
        specimen.path.display()
    );
}

#[test]
fn ns4_selectors_stay_in_panel_range() {
    use nord_format::{OrganPreset, PianoPreset, Synth};
    let mut seen = BTreeSet::new();
    for specimen in corpus() {
        match &specimen.entity {
            Entity::Program(Program::Stage4(p)) | Entity::Live(Live::Stage4(p)) => {
                in_panel_range(specimen, "organ_a.model", p.organ_a.model.raw(), 5);
                in_panel_range(specimen, "organ_b.model", p.organ_b.model.raw(), 5);
                in_panel_range(
                    specimen,
                    "piano_a.piano_type",
                    p.piano_a.piano_type.raw(),
                    5,
                );
                in_panel_range(
                    specimen,
                    "piano_b.piano_type",
                    p.piano_b.piano_type.raw(),
                    5,
                );
                let voice = &p.synth_a_voice;
                in_panel_range(
                    specimen,
                    "synth_a_voice.filter_type",
                    voice.filter_type.raw(),
                    5,
                );
                in_panel_range(
                    specimen,
                    "synth_a_voice.lfo_shape",
                    voice.lfo_shape.raw(),
                    4,
                );
                let priority = p.synth_a_performance.voice_priority.raw();
                in_panel_range(specimen, "synth_a_performance.voice_priority", priority, 2);
                in_panel_range(
                    specimen,
                    "organ_fx.reverb_type",
                    p.organ_fx.reverb_type.raw(),
                    11,
                );
                seen.insert("program");
            }
            Entity::OrganPreset(OrganPreset::Stage4(p)) => {
                in_panel_range(specimen, "organ_a.model", p.organ_a.model.raw(), 5);
                in_panel_range(specimen, "organ_b.model", p.organ_b.model.raw(), 5);
                in_panel_range(
                    specimen,
                    "organ_fx.reverb_type",
                    p.organ_fx.reverb_type.raw(),
                    11,
                );
                seen.insert("organ preset");
            }
            Entity::PianoPreset(PianoPreset::Stage4(p)) => {
                in_panel_range(
                    specimen,
                    "piano_a.piano_type",
                    p.piano_a.piano_type.raw(),
                    5,
                );
                in_panel_range(
                    specimen,
                    "piano_b.piano_type",
                    p.piano_b.piano_type.raw(),
                    5,
                );
                let reverb = p.piano_a_fx.reverb_type.raw();
                in_panel_range(specimen, "piano_a_fx.reverb_type", reverb, 11);
                seen.insert("piano preset");
            }
            Entity::Synth(Synth::Stage4(p)) => {
                let (a, b) = (&p.synth_a_voice, &p.synth_b_voice);
                in_panel_range(
                    specimen,
                    "synth_a_voice.filter_type",
                    a.filter_type.raw(),
                    5,
                );
                in_panel_range(
                    specimen,
                    "synth_b_voice.filter_type",
                    b.filter_type.raw(),
                    5,
                );
                in_panel_range(specimen, "synth_a_voice.lfo_shape", a.lfo_shape.raw(), 4);
                let priority = p.synth_a_performance.voice_priority.raw();
                in_panel_range(specimen, "synth_a_performance.voice_priority", priority, 2);
                in_panel_range(
                    specimen,
                    "synth_a_fx.reverb_type",
                    p.synth_a_fx.reverb_type.raw(),
                    11,
                );
                seen.insert("synth preset");
            }
            _ => {}
        }
    }
    assert_eq!(
        seen,
        BTreeSet::from(["organ preset", "piano preset", "program", "synth preset"]),
        "Stage 4 entity kinds checked"
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
                assert!(
                    bank.programs.iter().all(|(name, _)| !name.is_empty()),
                    "{}: unnamed member",
                    specimen.path.display()
                );
            }
            Entity::Bundle(Bundle::Drum3KitBank(bank)) => {
                assert_eq!(bank.kits.len(), 50, "{}", specimen.path.display());
                assert!(
                    bank.kits.iter().all(|(name, _)| !name.is_empty()),
                    "{}: unnamed member",
                    specimen.path.display()
                );
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

/// A `map` v14 zone stores the velocity window its project set, so a reader must read
/// the field and not assume the full range.
///
/// Inferred from specimens; not confirmed on hardware.
#[test]
fn a_wide_zone_answers_to_the_velocities_its_project_asked_for() {
    let asked: BTreeMap<&str, (u8, u8)> = BTreeMap::from([
        ("LY-30vmin64.nsmp4", (64, 127)),
        ("LY-31vmax63.nsmp4", (0, 63)),
        ("LY-32vwin.nsmp4", (64, 100)),
        ("LY-50two-en.nsmp4", (0, 63)),
        ("LY-51two-axis.nsmp4", (0, 63)),
        ("LY-52four-en.nsmp4", (0, 31)),
        ("LY-54bracket.nsmp4", (0, 63)),
    ]);
    for (file, window) in &asked {
        let Entity::Sample(Sample::V3(sample)) = &named(file).entity else {
            panic!("{file} is not a wide sample");
        };
        let zones = sample.zones().unwrap_or_else(|e| panic!("{file}: {e}"));
        let narrowed = zones
            .iter()
            .filter_map(|zone| zone.velocity)
            .find(|held| *held != nsmp::zone::VelocityWindow::FULL)
            .unwrap_or_else(|| panic!("{file}: every zone spans the full velocity range"));
        assert_eq!((narrowed.low, narrowed.high), *window, "{file}");
    }

    let mut narrowed = BTreeSet::new();
    let mut with_window = 0;
    let mut without = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        let Ok(zones) = sample.zones() else { continue };
        let name = specimen.path.file_name().unwrap().to_string_lossy();
        for zone in zones {
            match zone.velocity {
                Some(window) => {
                    with_window += 1;
                    if window != nsmp::zone::VelocityWindow::FULL {
                        narrowed.insert(name.to_string());
                    }
                }
                None => without += 1,
            }
        }
    }
    let unasked = narrowed
        .iter()
        .filter(|name| !asked.contains_key(name.as_str()))
        .collect::<Vec<_>>();
    assert!(
        unasked.is_empty(),
        "narrowed velocity windows in files whose project asked for none: {unasked:?}"
    );
    assert!(with_window > 0, "no zone record carrying a velocity window");
    assert!(without > 0, "no v12 zone record, whose layout stores none");
}

/// The wide zone record holds the same relative strength field as the v2 record, two
/// bytes later, and the field is wider than a byte: `LY-21rs300` holds 300.
///
/// Inferred from specimens; not confirmed on hardware.
#[test]
fn a_wide_zone_records_its_strokes_relative_strength() {
    for (file, weight) in [
        ("LY-1base.nsmp4", 1u16),
        ("LY-20rs0.nsmp4", 0),
        ("LY-21rs300.nsmp4", 300),
        ("LY-22rs32767.nsmp4", 32767),
        ("LY-32vwin.nsmp4", 16384),
    ] {
        let Entity::Sample(Sample::V3(sample)) = &named(file).entity else {
            panic!("{file} is not a wide sample");
        };
        let zones = sample.zones().unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(zones[0].rel_strength, Some(weight), "{file}");
    }
}

/// The v4 `sty` dynamics group is the only part of the preset a wide project can
/// set, and the enable controls all of it: `SP-dynen1` leaves the project's curve
/// field at its default and still moves the curve byte off its sentinel, while
/// `SP-dyn1` sets that field and renders the base values.
#[test]
fn the_v4_preset_holds_the_dynamics_a_project_asked_for() {
    for (file, enabled, curve, response) in [
        ("LY-1base.nsmp4", false, None, [127u8; 3]),
        ("LY-70dynen.nsmp4", true, Some(1), [74, 82, 90]),
        ("SP-00base.nsmp4", false, None, [127u8; 3]),
        ("SP-dyn1.nsmp4", false, None, [127u8; 3]),
        ("SP-dynen1.nsmp4", true, Some(1), [74, 82, 90]),
    ] {
        let Entity::Sample(Sample::V3(sample)) = &named(file).entity else {
            panic!("{file} is not a wide sample");
        };
        let nsmp::Sty::V4(sty) = sample.sty().unwrap_or_else(|e| panic!("{file}: {e}")) else {
            panic!("{file} carries no v4 preset");
        };
        assert_eq!(sty.dynamics_enabled(), enabled, "{file}");
        assert_eq!(sty.dynamics_curve(), curve, "{file}");
        assert_eq!(sty.dynamics_response(), response, "{file}");
    }
}

/// v3 stores the same dynamics group at its own offsets and on its own scale: the
/// enable is a level on the block's 0..127 grid, not a flag, and the response is
/// stored once, not once per layer.
#[test]
fn the_v3_preset_holds_the_same_dynamics_group() {
    for (file, enabled, curve, response) in [
        ("SP-00base.nsmp3", false, 2, 127),
        ("SP-dyn1.nsmp3", false, 2, 127),
        ("SP-dynen1.nsmp3", true, 1, 74),
    ] {
        let Entity::Sample(Sample::V3(sample)) = &named(file).entity else {
            panic!("{file} is not a wide sample");
        };
        let nsmp::Sty::V3(sty) = sample.sty().unwrap_or_else(|e| panic!("{file}: {e}")) else {
            panic!("{file} carries no v3 preset");
        };
        assert_eq!(sty.dynamics_enabled(), enabled, "{file}");
        assert_eq!(sty.dynamics_curve(), curve, "{file}");
        assert_eq!(sty.dynamics_response(), response, "{file}");
    }
}

/// The editor's loader replaces `samplib_attrs` with a preset chosen by the
/// instrument's category, and the v2 encoder copies that preset's two velocity
/// depths into the file. No project can set the depths directly, so pairing each
/// written-back project with the file it produced identifies the bytes.
#[test]
fn a_v2_preset_carries_the_velocity_depths_the_category_installed() {
    let mut seen = 0;
    for (specimen, sample) in v2_samples() {
        let Some(stem) = specimen.path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !stem.starts_with("SP-") {
            continue;
        }
        let project = project_named(&format!("{stem}.nsmpproj"));
        let installed = project.velocity_defaults().unwrap();
        let sty = sample.sty().unwrap_or_else(|e| panic!("{stem}: {e}"));
        let amplitude = nsmp::velocity_level(installed.amplitude)
            .unwrap_or_else(|| panic!("{stem}: unsupported amplitude {}", installed.amplitude));
        let timbre = nsmp::velocity_level(installed.timbre)
            .unwrap_or_else(|| panic!("{stem}: unsupported timbre {}", installed.timbre));
        assert_eq!(
            (sty.velocity_to_amplitude(), sty.velocity_to_timbre()),
            (amplitude, timbre),
            "{stem}: preset {installed:?}"
        );
        seen += 1;
    }
    assert!(seen > 0, "no SP specimen");
}

/// The instrument EQ is applied to the audio and not stored: every specimen that
/// turns a band on renders a different stroke and leaves all three `sty` EQ
/// records zero.
#[test]
fn the_instrument_eq_never_reaches_the_wide_preset() {
    let Entity::Sample(Sample::V3(base)) = &named("SP-00base.nsmp4").entity else {
        panic!("the SP base is not a wide sample");
    };
    let decode = |sample: &nord_format::cbin::Cbin<nsmp::SampleV3>| {
        sample
            .stroke_streams()
            .into_iter()
            .map(|(at, stroke)| {
                nsmp::codec::decode(stroke, at, nsmp::codec::Layout::V4)
                    .unwrap()
                    .samples
            })
            .collect::<Vec<_>>()
    };
    let quiet = decode(base);
    let mut seen = 0;
    for stem in [
        "SP-eq1lcon",
        "SP-eq4m0on",
        "SP-eq5m0freq",
        "SP-eq6m0gain",
        "SP-eq7m0q",
        "SP-eq8m1on",
        "SP-eq9m1freq",
        "SP-eqAm1gain",
        "SP-eqBm1q",
        "SP-eqDf12000",
        "SP-eqEg12",
        "SP-eqFq8",
    ] {
        let Entity::Sample(Sample::V3(sample)) = &named(&format!("{stem}.nsmp4")).entity else {
            panic!("{stem} is not a wide sample");
        };
        let nsmp::Sty::V4(sty) = sample.sty().unwrap_or_else(|e| panic!("{stem}: {e}")) else {
            panic!("{stem} carries no v4 preset");
        };
        for band in sty.eq() {
            assert_eq!(
                band,
                nsmp::EqBand {
                    frequency: 0,
                    gain: 0,
                    q: 0
                },
                "{stem}"
            );
        }
        assert_ne!(decode(sample), quiet, "{stem}: the EQ left PCM unchanged");
        seen += 1;
    }
    assert!(seen > 0, "no SP EQ specimen");
}

/// A v4 `sty` dynamics-response triple is all 127 exactly when no curve is selected.
///
/// Inferred from specimens; not confirmed on hardware.
#[test]
fn a_v4_dynamics_response_is_pinned_while_no_curve_is_selected() {
    let mut seen = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        let Ok(nsmp::Sty::V4(sty)) = sample.sty() else {
            continue;
        };
        assert_eq!(
            sty.dynamics_curve().is_none(),
            sty.dynamics_response() == [127; 3],
            "{}: curve {:?} against response {:?}",
            specimen.path.display(),
            sty.dynamics_curve(),
            sty.dynamics_response()
        );
        seen += 1;
    }
    assert!(seen > 0, "no v4 preset");
}

/// Every wide body's `meta` states the length of the chain before it. A writer that
/// resizes a section must update this field.
#[test]
fn a_wide_body_states_the_length_of_the_chain_ahead_of_its_meta() {
    let mut seen = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        let meta = sample
            .meta()
            .unwrap_or_else(|e| panic!("{}: {e}", specimen.path.display()));
        assert_eq!(
            meta.chain_len as usize,
            sample.chain_len_before_meta(),
            "{}",
            specimen.path.display()
        );
        seen += 1;
    }
    assert!(seen > 0, "no wide sample");
}

/// The v4 `sty` payload comes in two widths under one section version, so each
/// specimen's schema is chosen by its length.
#[test]
fn every_sample_preset_parses_under_its_own_schema() {
    let mut v2 = 0;
    let (mut v3, mut v4) = (0, 0);
    for (specimen, sample) in v2_samples() {
        let where_ = specimen.path.display();
        let sty = sample.sty().unwrap_or_else(|e| panic!("{where_}: {e}"));
        assert_eq!(sty.raw.len(), nsmp::sty::V2_LEN, "{where_}");
        v2 += 1;
    }
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        let where_ = specimen.path.display();
        match sample.sty().unwrap_or_else(|e| panic!("{where_}: {e}")) {
            nsmp::Sty::V2(_) => panic!("{where_}: a wide chain read a v2 preset"),
            nsmp::Sty::V3(block) => {
                assert_eq!(block.raw.len(), nsmp::sty::V3_LEN, "{where_}");
                v3 += 1;
            }
            nsmp::Sty::V4(block) => {
                assert!(
                    block.raw.len() == nsmp::sty::V4_LEN
                        || block.raw.len() == nsmp::sty::V4_LEN_LONG,
                    "{where_}: v4 sty of {} bytes",
                    block.raw.len()
                );
                for band in block.eq() {
                    assert!(
                        band.frequency <= 20_000,
                        "{where_}: EQ band at {} Hz",
                        band.frequency
                    );
                }
                v4 += 1;
            }
        }
    }
    assert!(
        v2 > 0 && v3 > 0 && v4 > 0,
        "a sty generation has no specimen: v2 {v2}, v3 {v3}, v4 {v4}"
    );
}

/// A live slot and a stored program use the same body. Confirmed on hardware.
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
        assert_eq!(
            fields(live.fields()),
            fields(program.fields()),
            "{}",
            specimen.path.display()
        );
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
    assert_eq!(
        slots,
        BTreeSet::from([(0, 0), (0, 1), (0, 2)]),
        "Electro 5 live slots"
    );
}

/// Setting each organ model's drawbars to the values just read leaves every byte
/// unchanged.
#[test]
fn ne5_drawbars_survive_a_rewrite() {
    use nord_format::formats::ne5::OrganModel::{Farfisa, Pipe, Vox, B3};
    use nord_format::formats::ne5::Preset;

    let mut seen = 0;
    for (specimen, _) in ne5_programs() {
        let Entity::Program(Program::Electro5(mut program)) =
            nord_format::from_stream(&mut Cursor::new(&specimen.bytes)).unwrap()
        else {
            unreachable!()
        };
        for model in [B3, Vox, Farfisa, Pipe] {
            for preset in [Preset::One, Preset::Two] {
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

/// Libraries before Sample Library 2.0 write a narrower chain with its own `map`
/// version: no `cat`, an 18-byte `hdr` with no name field, and a zone record three
/// bytes shorter. The content version does not distinguish the two chains, so this
/// sweeps by [`nsmp::Chain`].
#[test]
fn pre_library_2_instruments_decode_their_narrower_zone_table() {
    let mut early = 0;
    let mut library2 = 0;
    for (specimen, sample) in v2_samples() {
        let where_ = specimen.path.display();
        let chain = sample.chain().unwrap_or_else(|e| panic!("{where_}: {e}"));
        if chain == nsmp::Chain::Library2 {
            library2 += 1;
            continue;
        }
        assert_eq!(chain, nsmp::Chain::Early, "{where_}");
        early += 1;

        assert!(
            !chain.names_instrument() && sample.name().unwrap().is_empty(),
            "{where_}: this chain has no name field"
        );
        let mut copy = nsmp::from_bytes(&specimen.bytes).unwrap();
        assert!(
            copy.set_name("Renamed").is_err(),
            "{where_}: renamed an instrument whose chain has no name field"
        );
        assert!(
            sample.categories().is_empty(),
            "{where_}: this chain has no cat section"
        );

        let zones = sample.zones().unwrap_or_else(|e| panic!("{where_}: {e}"));
        let strokes = sample.strokes().unwrap_or_else(|e| panic!("{where_}: {e}"));
        assert_eq!(zones.len(), strokes.len(), "{where_}");
        assert_eq!(
            zones.len() * chain.zone_record_len() + nsmp::zone::RECORDS_AT,
            v2_map_payload(sample).len(),
            "{where_}: the map payload length does not match its zone count"
        );
        for pair in zones.windows(2) {
            assert!(
                pair[0].top_note > pair[1].top_note,
                "{where_}: zones are not stored high to low"
            );
        }
        for (zone, stroke) in zones.iter().zip(&strokes) {
            assert!(zone.top_note <= 127, "{where_}");
            assert!(stroke.root_key <= 127, "{where_}");
            assert!(
                stroke.packets.is_some(),
                "{where_}: stroke length is not this chain's header plus whole packets"
            );
            assert_eq!(
                zone.rel_strength,
                nsmp::zone::REL_STRENGTH_DEFAULT,
                "{where_}"
            );
        }
        assert!(sample.sty().is_ok(), "{where_}: sty");
        assert!(sample.key_table().is_ok(), "{where_}: keyboard map");
    }
    assert!(early > 0, "no pre-2.0 instrument in the corpus");
    assert!(
        library2 > 0,
        "no Sample Library 2.0 instrument in the corpus"
    );
}

/// A zone record names its stroke in one byte, but a stroke's id is a u32, and some
/// library instruments have stroke ids past 255. Pairing on the whole u32 would lose
/// those zones, and no instrument with smaller ids would show it.
#[test]
fn zones_pair_with_strokes_whose_ids_run_past_a_byte() {
    let mut aliased = 0;
    for (specimen, sample) in v2_samples() {
        let where_ = specimen.path.display();
        let ids: Vec<u32> = sample
            .stroke_streams()
            .iter()
            .map(|(_, s)| u32::from_be_bytes(s[0..4].try_into().unwrap()))
            .collect();
        if ids.iter().all(|id| *id <= u32::from(u8::MAX)) {
            continue;
        }
        aliased += 1;
        assert_eq!(
            sample
                .strokes()
                .unwrap_or_else(|e| panic!("{where_}: {e}"))
                .len(),
            sample.zones().unwrap().len(),
            "{where_}"
        );
    }
    assert!(aliased > 0, "no instrument with a stroke id past 255");
}

/// A rename and a zone remap reproduce the file the Sample Editor wrote.
#[test]
fn nsmp_edits_reproduce_editor_output() {
    let mut sample = v2_named("D4-3zones.nsmp");
    sample.set_name("D7-upperkey").unwrap();
    sample.set_zone_top_note(1, 60).unwrap();
    assert_eq!(
        sample.to_bytes().unwrap(),
        named("D7-upperkey.nsmp").bytes,
        "D4-3zones.nsmp edited into D7-upperkey.nsmp"
    );
}

/// Retuning changes the root-key byte and the container checksum only.
#[test]
fn nsmp_retune_is_surgical() {
    let before = &named("D1-one-zone.nsmp").bytes;
    let mut sample = v2_named("D1-one-zone.nsmp");
    let was = sample.strokes().unwrap()[0].root_key;
    assert_ne!(
        was, 48,
        "the specimen already holds root key 48, so the retune would change nothing"
    );
    sample.set_root_key(0, 48).unwrap();
    let after = sample.to_bytes().unwrap();

    let changed = (0..before.len())
        .filter(|&index| before[index] != after[index])
        .collect::<Vec<_>>();
    assert_eq!(changed.len(), 5, "changed bytes: {changed:?}");
    assert!(
        changed[..4].iter().eq([0x18, 0x19, 0x1a, 0x1b].iter()),
        "the first four changed bytes are not the container's body CRC-32: {changed:?}"
    );

    let (stroke_at, stroke) = sample.stroke_streams()[0];
    let payload = sample.header.generation.body_start() as usize + stroke_at;
    assert!(
        (payload..payload + stroke.len()).contains(&changed[4]),
        "the fifth changed byte {:#x} is outside stroke 0's payload",
        changed[4]
    );
    assert_eq!(before[changed[4]], was, "the root key the specimen held");
    assert_eq!(after[changed[4]], 48, "the root key the edit asked for");
    assert_eq!(
        sample.strokes().unwrap()[0].root_key,
        48,
        "root key read back"
    );
}

#[test]
fn nsmp_overlong_name_is_refused_without_mutation() {
    let mut sample = v2_named("D1-one-zone.nsmp");
    let over = "M".repeat(nsmp::MAX_NAME_LEN + 1);
    assert!(
        sample.set_name(&over).is_err(),
        "an overlong name was accepted"
    );
    assert_eq!(
        sample.name().unwrap(),
        "TEST",
        "the refused rename changed the name"
    );
}

#[test]
fn nsmp_bad_checksum_is_refused() {
    let mut bytes = named("D1-one-zone.nsmp").bytes.clone();
    *bytes.last_mut().unwrap() ^= 0xff;
    assert!(
        nord_format::from_stream(&mut Cursor::new(&bytes)).is_err(),
        "a corrupted checksum was accepted"
    );
}

/// A device read whose leading body bytes are foreign buffer content, so the `NWS`
/// container and the sections after it are missing. Its `.skip.` name keeps it out of
/// the sweep, so it is found through [`scan::named_skipped`].
#[test]
fn nsmp_body_without_its_container_section_says_which_tag_was_expected() {
    let path = scan::named_skipped("stereo77.skip.nsmp");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let error = nord_format::from_stream(&mut Cursor::new(&bytes))
        .expect_err("a body missing its container section must not parse");
    let nord_format::error::Error::Parse(nord_format::error::ParseError::AssertFail(said)) = &error
    else {
        panic!("refused as {error:?}, not as a violated body assertion");
    };
    assert!(
        said.contains("NWS"),
        "the refusal does not name the container tag it wanted: {said}"
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
    // Reading the result back proves the container checksum was recomputed,
    // because a stale one is refused.
    nord_format::from_stream(&mut Cursor::new(&after))
        .unwrap_or_else(|e| panic!("{name} does not read back after the edit: {e}"));
    (before, after)
}

/// The offsets of the bytes an edit changed, excluding the container checksum,
/// which changes whenever anything else does.
fn moved(before: &[u8], after: &[u8]) -> Vec<usize> {
    const CHECKSUM: std::ops::Range<usize> = 0x18..0x1c;
    (0..before.len())
        .filter(|&i| before[i] != after[i] && !CHECKSUM.contains(&i))
        .collect()
}

/// Every zone's decoded audio, which no edit here may change.
fn audio(bytes: &[u8]) -> Vec<Vec<i16>> {
    let entity = nord_format::from_stream(&mut Cursor::new(bytes)).unwrap();
    let Entity::Sample(sample) = &entity else {
        panic!("not a sample instrument");
    };
    let layout = sample.layout().unwrap();
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
        // Writing one name over another changes at most as many bytes as the longer
        // name (the shorter one's tail is NUL-filled), all within the field.
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
    assert_eq!(sample.name().unwrap(), long, "{V3_MAP_14}: name");
    assert_eq!(
        sample.sub_name().unwrap(),
        "KG  mono",
        "{V3_MAP_14}: sub-name"
    );

    let mut entity = nord_format::from_stream(&mut Cursor::new(&named(V3_MAP_14).bytes)).unwrap();
    let Entity::Sample(sample) = &mut entity else {
        unreachable!()
    };
    assert!(
        sample.set_name(&format!("{long}x")).is_err(),
        "{V3_MAP_14}: a name past MAX_NAME_V3_LEN was accepted"
    );
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
            // This layout stores no low note, so the setter refuses instead of
            // writing a byte that means something else.
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
                // The Sample Editor writes the neutral table for any zone layout,
                // so the planner must leave it neutral.
                assert!(
                    table.plan_key_map(&map.payload, &zones).unwrap().is_empty(),
                    "{name}: the planner wrote into a neutral table"
                );
                neutral += 1;
            }
            nsmp::zone::KeyMap::Populated => {
                let mut after = map.payload.clone();
                for (at, quad) in table.plan_key_map(&map.payload, &zones).unwrap() {
                    after[at..at + quad.len()].copy_from_slice(&quad);
                }
                assert_eq!(
                    after, map.payload,
                    "{name}: the planned key map differs from the stored one"
                );
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
        // Move every root away and back. The table is recomputed on each edit, so
        // a byte-identical result shows the planner reproduces what the builder wrote.
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
    assert_eq!(zones_of(&after)[0].0, root - 1, "root key after the retune");
    assert_eq!(audio(before), audio(&after), "retune moved audio");

    // The gains and the three bytes after them are an authored curve that no
    // layout predicts, so the recompute must keep all of them.
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
        // A probe must differ from what the stroke holds, or nothing changes, and
        // the LY projects hold velocity windows other than the default.
        let windows: BTreeMap<u32, (u8, u8)> = project
            .zones()
            .unwrap()
            .iter()
            .flat_map(|z| z.strokes.iter().map(|s| (s.global_id, s.velocity)))
            .collect();
        let elsewhere = |held: u8, probe: u8| if held == probe { probe ^ 1 } else { probe };
        for stroke in project.strokes().unwrap() {
            let (vmin, vmax) = windows.get(&stroke.global_id).copied().unwrap_or((0, 127));
            let fields = [
                ("start", F::Start(3.0)),
                ("stop", F::Stop(4000.0)),
                ("gain", F::Gain(0.75)),
                ("velocity_min", F::VelocityMin(elsewhere(vmin, 10))),
                ("velocity_max", F::VelocityMax(elsewhere(vmax, 100))),
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
    let mut seen = 0;
    for (specimen, project) in projects() {
        let before = project.render();
        let was = project.velocity_defaults().unwrap();
        let mut edited = project.clone();
        let defaults = nsmpproj::VelocityDefaults {
            attack_amount: 64,
            amplitude: 0,
            timbre: 0,
        };
        edited.set_velocity_defaults(defaults).unwrap();
        assert_eq!(
            edited.velocity_defaults().unwrap(),
            defaults,
            "{}: velocity defaults read back",
            specimen.path.display()
        );
        let after = edited.render();
        let changed = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .count();
        let asked = [
            was.attack_amount != defaults.attack_amount,
            was.amplitude != defaults.amplitude,
            was.timbre != defaults.timbre,
        ]
        .into_iter()
        .filter(|moved| *moved)
        .count();
        assert_eq!(changed, asked, "{}", specimen.path.display());
        seen += 1;
    }
    assert!(seen > 0, "no sample-editor project in the corpus");
}

fn project_named(name: &str) -> &'static nsmpproj::Project {
    match &named(name).entity {
        Entity::SampleProject(project) => project,
        other => panic!("{name} decoded as {other:?}"),
    }
}

/// A project's zones, each with audio of the length the project gives it.
///
/// The editor's WAVs are not corpus material, so the audio is generated. Only the frame
/// count affects anything asserted below: a stroke's field count comes from its length,
/// and every other field compared is metadata.
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

/// A narrow instrument built from a project's zones, with the editor's predictor
/// choice.
fn built_v2(
    zones: &[nsmp::encode::NewZone<'_>],
    name: &str,
) -> Result<nord_format::cbin::Cbin<nsmp::Sample>, nord_format::error::Error> {
    match nsmp::encode::multi_zone(
        nsmp::encode::Instrument {
            name,
            map_gain: 1.0,
            predictor: nsmp::encode::Predictor::Minimising,
            layout: nsmp::codec::Layout::V2,
            preset: nsmp::encode::Preset::default(),
        },
        zones,
    )? {
        Sample::V2(file) => Ok(file),
        Sample::V3(_) => panic!("the narrow layout builds the narrow chain"),
    }
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
            gain: 1.0,
            loop_decay: nsmp::encode::DEFAULT_LOOP_DECAY,
        }
    }
}

#[test]
fn nsmp_building_a_project_reproduces_its_editor_twin() {
    // Expected bytes come from paired Sample Editor project and instrument specimens.
    for name in ["D3-2zones", "D4-3zones", "D8-2zones-hi", "D7-upperkey"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let zones = built_zones(project);
        let built = built_v2(
            &zones.iter().map(BuiltZone::new_zone).collect::<Vec<_>>(),
            &project.name().unwrap(),
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
    // Expected lengths come from the Sample Editor instrument specimens.
    for name in ["D1-one-zone", "D3-2zones", "D4-3zones", "D8-2zones-hi"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let twin = v2_named(&format!("{name}.nsmp"));
        for (index, zone) in built_zones(project).iter().enumerate() {
            let (at, stream) = twin.zone_stream(index).unwrap();
            let editor = nsmp::codec::decode(stream, at, nsmp::codec::Layout::V2).unwrap();
            assert_eq!(
                nsmp::encode::Plan::new(
                    nsmp::codec::Layout::V2,
                    zone.audio.len(),
                    1,
                    zone.secondary_start
                )
                .unwrap()
                .fields,
                editor.samples.len(),
                "{name} zone {index}: {} frames",
                zone.audio.len()
            );
        }
    }
}

/// Converts decibels to a linear gain with [`nsmp::zone::GAIN_BITS`] fractional bits,
/// computed at higher precision than the field and rounded once, as the writer does.
/// Silence (`-inf`) and a negative gain (NaN) both convert to zero.
fn gain_units(decibels: f32) -> u64 {
    (10f64.powf(f64::from(decibels) / 20.0) * f64::from(nsmp::zone::GAIN_UNITY)).round() as u64
}

/// The wide render of the same instrument, where the corpus holds one. Both wide
/// generations state the gain the same way, so either one works.
fn wide_twin(path: &std::path::Path) -> Option<&'static nord_format::cbin::Cbin<nsmp::SampleV3>> {
    let stem = path.file_stem()?.to_string_lossy();
    ["nsmp3", "nsmp4"].iter().find_map(|extension| {
        let name = format!("{stem}.{extension}");
        corpus().iter().find_map(|s| match &s.entity {
            Entity::Sample(Sample::V3(wide)) if s.path.ends_with(&name) => Some(wide),
            _ => None,
        })
    })
}

/// The gain stroke `id` was built with, read from a wide render's decibel field.
fn wide_stroke_gain(wide: &'static nord_format::cbin::Cbin<nsmp::SampleV3>, id: u8) -> Option<u64> {
    let layout = nsmp::codec::Layout::from_version(wide.header.version)?;
    let (_, stroke) = wide
        .stroke_streams()
        .into_iter()
        .find(|(_, s)| s[3] == id)?;
    Some(gain_units(nsmp::codec::zone_gain_db(stroke, layout)?))
}

/// A wide stroke's statistic A is the reciprocal of the file peak, scaled by the gain
/// the stroke's decibel field converts back to. The project's float gain agrees below
/// `2^24` and differs above it, where the file follows the decibel field.
///
/// Inferred from specimens; not confirmed on hardware.
#[test]
fn nsmp_wide_statistic_a_is_built_from_the_decibel_the_header_stores() {
    let mut seen = 0;
    for specimen in corpus() {
        let Entity::Sample(Sample::V3(sample)) = &specimen.entity else {
            continue;
        };
        // Library instruments are left out: their strokes keep the mantissa of
        // whatever file first encoded them.
        if !specimen
            .path
            .components()
            .any(|c| c.as_os_str() == "samples")
        {
            continue;
        }
        let layout = nsmp::codec::Layout::from_version(sample.header.version)
            .expect("a corpus specimen states a content version the codec models");
        let streams = sample.stroke_streams();
        let peak = streams
            .iter()
            .filter_map(|(_, s)| nsmp::codec::peak(s, layout))
            .map(|p| p.unsigned_abs())
            .max()
            .unwrap_or(0)
            .max(1) as u64;
        let bits = 64 - peak.leading_zeros();
        let exact_power = u32::from(peak.is_power_of_two());
        let reciprocal = (1u64 << (21 + bits + (1 - exact_power))) / peak;
        for (_, stroke) in streams {
            let decibels = nsmp::codec::zone_gain_db(stroke, layout).expect("a wide header");
            let mantissa = (reciprocal * gain_units(decibels)) >> (nsmp::zone::GAIN_BITS + 3);
            assert_eq!(
                stroke[9..12],
                ((mantissa % (1 << 24)) as u32).to_be_bytes()[1..],
                "{} at {decibels} dB",
                specimen.path.display()
            );
            seen += 1;
        }
    }
    assert!(seen > 0, "no self-generated wide stroke");
}

/// The same rule for narrow strokes, over every self-generated v2 specimen at any gain.
/// Library instruments are left out: their strokes keep the mantissa of whatever file
/// first encoded them.
///
/// A narrow header has no decibel field, and the zone record stores the gain mod
/// `2^24`, so above a gain of 16 the record reads back quieter than the gain the
/// mantissa was built from. Where the corpus holds a wide render of the same
/// instrument, the test uses that render's decibel field; otherwise it uses the zone
/// record.
#[test]
fn nsmp_statistic_a_is_the_file_peaks_reciprocal_scaled_by_the_zones_gain() {
    let layout = nsmp::codec::Layout::V2;
    let (mut seen, mut twinned) = (0, 0);
    for (specimen, sample) in v2_samples() {
        if !specimen
            .path
            .components()
            .any(|c| c.as_os_str() == "samples")
        {
            continue;
        }
        let twin = wide_twin(&specimen.path);
        let zones = sample.zones().unwrap();
        let streams = sample.stroke_streams();
        let peak = streams
            .iter()
            .filter_map(|(_, s)| nsmp::codec::peak(s, layout))
            .max()
            .unwrap_or(0) as u32;
        for (_, stroke) in streams {
            let (gain, whence) = match twin.and_then(|wide| wide_stroke_gain(wide, stroke[3])) {
                Some(units) => {
                    twinned += 1;
                    (units, "a wide twin's decibel")
                }
                None => (
                    u64::from(
                        zones
                            .iter()
                            .find(|z| z.stroke_id == stroke[3])
                            .map_or(nsmp::zone::GAIN_UNITY, |z| z.gain),
                    ),
                    "the zone record",
                ),
            };
            let peak = u64::from(peak.max(1));
            let bits = 64 - peak.leading_zeros();
            let exact_power = u32::from(peak.is_power_of_two());
            let reciprocal = (1u64 << (21 + bits + (1 - exact_power))) / peak;
            let mantissa = (reciprocal * gain) >> (nsmp::zone::GAIN_BITS + 3);
            assert_eq!(
                stroke[9..12],
                ((mantissa % (1 << 24)) as u32).to_be_bytes()[1..],
                "{} stroke {} at gain {gain} from {whence}",
                specimen.path.display(),
                stroke[3]
            );
            seen += 1;
        }
    }
    assert!(seen > 0, "no self-generated v2 stroke");
    assert!(twinned > 0, "no narrow stroke had a wide twin");
}

#[test]
fn nsmp_a_built_instrument_walks_and_agrees_with_its_directory() {
    for name in ["D3-2zones", "D4-3zones", "D8-2zones-hi", "D7-upperkey"] {
        let project = project_named(&format!("{name}.nsmpproj"));
        let zones = built_zones(project);
        let built = built_v2(
            &zones.iter().map(BuiltZone::new_zone).collect::<Vec<_>>(),
            &project.name().unwrap(),
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
            let head = nsmp::stroke::header_len(
                nsmp::codec::Layout::V2,
                nsmp::Chain::Library2,
                index,
                cat_len,
                map_len,
            );
            assert_eq!(
                (stream.len() - head) % nsmp::stroke::packet_len(nsmp::codec::Layout::V2),
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
                "{name} stroke {index}: resync points at no record"
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
        let payload = v2_map_payload(sample);
        let table = sample
            .key_table()
            .unwrap_or_else(|e| panic!("{}: {e}", specimen.path.display()));
        let mut copy = payload.clone();
        table.write(&mut copy).unwrap();
        assert_eq!(copy, payload, "{}", specimen.path.display());
        seen += 1;
    }
    assert!(seen > 0, "no narrow instrument in the corpus");
}

#[test]
fn keyboard_map_records_read_as_the_editor_wrote_them() {
    use nsmp::keymap::{Level, GAIN_UNITY};
    let key = |name: &str, note: u8| v2_named(name).key_table().unwrap().key(note).unwrap();
    let instrument = |name: &str| v2_named(name).key_table().unwrap().instrument;

    assert_eq!(
        v2_named("MN-00base.nsmp").key_table().unwrap(),
        nsmp::KeyTable::NEUTRAL
    );
    assert_eq!(
        key("MN-05ng60h.nsmp", 60),
        Level::new(GAIN_UNITY / 2, 0).unwrap()
    );
    assert_eq!(key("MN-06ng17h.nsmp", 17).gain(), GAIN_UNITY / 2);
    assert_eq!(key("MN-07ng108h.nsmp", 108).gain(), GAIN_UNITY / 2);
    assert_eq!(key("MN-01nd60p8.nsmp", 60).detune(), 20);
    assert_eq!(key("MN-02nd60m8.nsmp", 60).detune(), -20);
    assert_eq!(key("MN-03nd17p1.nsmp", 17).detune(), 2);
    assert_eq!(key("MN-04nd108p1.nsmp", 108).detune(), 2);
    // The editor's +9 dB ceiling and -9 dB floor on a key's gain.
    assert_eq!(key("MN-08ng60x4.nsmp", 60).gain(), 0x2d_1819);
    assert_eq!(key("MN-09ng60tny.nsmp", 60).gain(), 0x05_ad51);
    assert_eq!(instrument("MN-13mgn4.nsmp").gain(), 0x2d_1819);
    assert_eq!(instrument("MN-14mgn001.nsmp").gain(), 0x419);
    assert_eq!(instrument("MN-15mdt100.nsmp").detune(), 256);
    assert_eq!(instrument("MN-16mdtm100.nsmp").detune(), -256);
    assert_eq!(instrument("MN-17mdt1200.nsmp").detune(), 3072);

    let macro3 = v2_named("MN-10mac3h.nsmp").key_table().unwrap();
    assert_eq!(
        macro3.adjusted().collect::<Vec<_>>(),
        (49..=71).collect::<Vec<_>>()
    );
    assert_eq!(macro3.key(60).unwrap().gain(), GAIN_UNITY / 2);
}

#[test]
fn setting_the_keyboard_map_touches_only_the_keyboard_map() {
    use nsmp::keymap::Level;
    let mut edited = v2_named("MN-05ng60h.nsmp");
    let strokes_before = edited.stroke_streams().len();
    let mut table = edited.key_table().unwrap();
    table.set_key(60, Level::NEUTRAL).unwrap();
    edited.set_key_table(&table).unwrap();
    assert_eq!(
        v2_map_payload(&edited),
        v2_map_payload(&v2_named("MN-00base.nsmp")),
        "map payload with key 60 reset, against MN-00base.nsmp"
    );
    assert_eq!(
        edited.stroke_streams().len(),
        strokes_before,
        "stroke count"
    );
    let bytes = edited.to_bytes().unwrap();
    let reread = nsmp::from_bytes(&bytes).unwrap();
    assert_eq!(
        reread.key_table().unwrap(),
        nsmp::KeyTable::NEUTRAL,
        "key table read back"
    );
}

#[test]
fn nsmp_the_kernel_matches_the_corpus_f32_tap_table() {
    let path = std::env::var_os("NORD_NSMP_KERNEL_ORACLE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| scan::root().join("tools/nsmp-pitch/table-fl32.tsv"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let bank = nsmp::kernel::taps();
    let mut points = BTreeSet::new();
    for (line_number, line) in text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.starts_with('#') && !line.starts_with("k\t"))
    {
        let mut cols = line.split('\t');
        let mut column = |name: &str| {
            cols.next()
                .unwrap_or_else(|| panic!("{}:{} has no {name}", path.display(), line_number + 1))
        };
        let k: usize = column("phase")
            .parse()
            .unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), line_number + 1));
        let m: i64 = column("tap")
            .parse()
            .unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), line_number + 1));
        let g: f32 = column("value")
            .parse()
            .unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), line_number + 1));
        let class = column("class");
        assert!(k < nsmp::kernel::PHASES, "phase {k}");
        assert!((-16..=15).contains(&m), "phase {k} tap {m}");
        assert!(points.insert((k, m)), "duplicate phase {k} tap {m}");
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
            }
            "ideal" => {
                assert!(ulps <= 1, "phase {k} m {m}: ours {ours} table {g}");
            }
            "zero" => assert_eq!(ours, 0.0, "phase {k} m {m}"),
            other => panic!("phase {k} m {m}: class {other}"),
        }
    }
    assert_eq!(
        points.len(),
        nsmp::kernel::PHASES * (nsmp::kernel::TAPS + 2),
        "{}: phase and tap points",
        path.display()
    );
}

/// Renaming an instrument to its current name changes no byte, in any generation: the
/// read stops at the terminator within the generation's name span, and the write covers
/// that span. Shipped libraries have names longer than the narrowest field, so a writer
/// sized to that field fails here.
#[test]
fn renaming_a_sample_to_the_name_it_holds_moves_no_byte() {
    let mut seen = 0;
    let mut longest = String::new();
    for specimen in corpus() {
        if !matches!(specimen.entity, Entity::Sample(_)) {
            continue;
        }
        let where_ = specimen.path.display();
        let mut entity = nord_format::from_stream(&mut Cursor::new(&specimen.bytes))
            .unwrap_or_else(|e| panic!("{where_}: {e}"));
        let Entity::Sample(sample) = &mut entity else {
            unreachable!("re-read as a different entity");
        };
        let name = sample.name().unwrap_or_else(|e| panic!("{where_}: {e}"));
        // The oldest narrow `hdr` is 18 bytes and has no name field.
        if name.is_empty() {
            continue;
        }
        sample
            .set_name(&name)
            .unwrap_or_else(|e| panic!("{where_}: renaming to {name:?}: {e}"));
        let back = nord_format::to_bytes(&entity).unwrap_or_else(|e| panic!("{where_}: {e}"));
        assert!(
            back == specimen.bytes,
            "{where_}: renaming to {name:?} changed a byte"
        );
        if name.len() > longest.len() {
            longest = name;
        }
        seen += 1;
    }
    assert!(seen > 0, "no named sample instrument");
    /// The longest name the editor's name box accepts. Without a longer name from a
    /// shipped library, the assertion above proves nothing.
    const EDITOR_BOX: usize = 14;
    assert!(
        longest.len() > EDITOR_BOX,
        "no name longer than the editor's box to test a short writer against: {longest:?}"
    );
}

fn pianos() -> impl Iterator<Item = (&'static Specimen, &'static npno::Piano)> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Piano(piano) => Some((s, piano)),
        _ => None,
    })
}

/// The writer recomputes the per-root counts, every audio offset, the alignment gap,
/// and the container checksum from the model. A byte-exact rebuild shows those are
/// derived correctly and nothing else was lost.
#[test]
fn a_piano_rebuilds_from_its_model_byte_for_byte() {
    let mut seen = 0;
    for (specimen, piano) in pianos() {
        let where_ = specimen.path.display();
        let library = piano
            .library()
            .unwrap_or_else(|e| panic!("{where_}: parse: {e}"));
        let rebuilt = library
            .to_piano()
            .and_then(|p| nord_format::to_bytes(&Entity::Piano(p)))
            .unwrap_or_else(|e| panic!("{where_}: rebuild: {e}"));
        let at = rebuilt
            .iter()
            .zip(&specimen.bytes)
            .position(|(a, b)| a != b)
            .map(|i| format!("{i:#x}"))
            .unwrap_or_else(|| "the length".to_string());
        assert!(
            rebuilt == specimen.bytes,
            "{where_}: the rebuild differs at {at} (in {} bytes, out {})",
            specimen.bytes.len(),
            rebuilt.len()
        );
        seen += 1;
    }
    assert!(seen > 0, "no piano library");
}

/// Every stroke decodes: each block repeats the previous block's last frames bit for
/// bit, and the blocks' own frames add up to the count the record states. Both check
/// the decoder against the file, so a decoder that drifts fails here instead of
/// producing noise.
#[test]
fn every_piano_stroke_decodes_with_its_overlap_and_frame_count_intact() {
    let mut strokes = 0;
    let mut overlap = 0;
    for (specimen, piano) in pianos() {
        let where_ = specimen.path.display();
        let library = piano.library().unwrap();
        for stroke in library.strokes() {
            let audio = npno::codec::decode(stroke, library.channels())
                .unwrap_or_else(|e| panic!("{where_}: {stroke:?}: {e}"));
            assert_eq!(
                audio.frames(),
                stroke.frames() as usize,
                "{where_}: {stroke:?}"
            );
            assert_eq!(
                audio.clipped, 0,
                "{where_}: {stroke:?} decoded outside the int16 range"
            );
            overlap += audio.overlap_checked;
            strokes += 1;
        }
    }
    assert!(strokes > 0, "no piano stroke");
    assert!(overlap > 0, "no stroke long enough to repeat a block");
}

/// The coder inverts the decoder: given the frames a stroke decodes to, it lays out the
/// same blocks, with the same segmentation, width, order, residuals, and clear bits
/// after the last field. This holds for every stroke of every library, including the
/// ones this crate wrote.
///
/// A block's attenuation byte is the only value that can differ. The vendor's encoder
/// recorded it as a statistic that does not depend on the stored frames, and the
/// decoder does not read it.
#[test]
fn every_piano_stroke_codes_back_to_the_blocks_it_came_from() {
    let mut strokes = 0;
    for (specimen, piano) in pianos() {
        let where_ = specimen.path.display();
        let library = piano.library().unwrap();
        let again =
            npno::encode::rebuild(&library).unwrap_or_else(|e| panic!("{where_}: recode: {e}"));
        for (stroke, recoded) in library.strokes().iter().zip(&again.strokes) {
            assert_eq!(
                recoded.blocks,
                usize::from(stroke.blocks()),
                "{where_}: {stroke:?} came back as a different number of blocks"
            );
            assert_eq!(
                recoded.recoded(),
                0,
                "{where_}: {stroke:?}: {} of {} block(s) came back with different residuals, \
                 a different width, or a different order",
                recoded.recoded(),
                recoded.blocks
            );
            strokes += 1;
        }
    }
    assert!(strokes > 0, "no piano stroke");
}

/// Recoding a library from its own audio reproduces the library: the same size, prefix,
/// and directory records, and the same blocks in the same places. Only the blocks'
/// declared attenuation can change, so the written file plays the same audio as the
/// file that was read.
#[test]
fn recoding_a_piano_from_its_own_audio_leaves_the_container_alone() {
    let mut seen = 0;
    for (specimen, piano) in pianos() {
        let where_ = specimen.path.display();
        let library = piano.library().unwrap();
        let before = library.to_body().unwrap();
        let again = npno::encode::rebuild(&library).unwrap();
        let after = again.library.to_body().unwrap();
        assert_eq!(
            after.len(),
            before.len(),
            "{where_}: the recode is a different size"
        );

        let audio: usize = again
            .library
            .strokes()
            .iter()
            .map(|s| s.audio().len())
            .sum();
        let head = before.len() - audio;
        assert!(
            after[..head] == before[..head],
            "{where_}: the recode changed a byte outside the audio"
        );
        for (stroke, recoded) in again.library.strokes().iter().zip(&again.strokes) {
            assert_eq!(
                recoded.identical + recoded.restated,
                recoded.blocks,
                "{where_}: {stroke:?} did not come back block for block"
            );
        }
        seen += 1;
    }
    assert!(seen > 0, "no piano library");
}

/// Coding reaches a fixed point in one pass: recoding a recode writes the same bytes,
/// blocks and container alike. A stroke states the frames its blocks own, so the second
/// search has the same room and chooses the same widths. That is why a library this
/// crate writes survives a rebuild unchanged.
#[test]
fn coding_a_piano_again_from_the_recode_reaches_the_same_file() {
    let mut seen = 0;
    for (specimen, piano) in pianos() {
        let where_ = specimen.path.display();
        let library = piano.library().unwrap();
        let once = npno::encode::rebuild(&library).unwrap();
        let twice = npno::encode::rebuild(&once.library).unwrap();
        let before = once.library.to_body().unwrap();
        let after = twice.library.to_body().unwrap();
        let at = before
            .iter()
            .zip(&after)
            .position(|(a, b)| a != b)
            .map(|i| format!("{i:#x}"))
            .unwrap_or_else(|| "the length".to_string());
        assert!(
            before == after,
            "{where_}: coding the recode again differs at {at} (in {} bytes, out {})",
            before.len(),
            after.len()
        );
        seen += 1;
    }
    assert!(seen > 0, "no piano library");
}

/// A library built from recordings alone, with no template bytes, reproduces
/// `from-scratch.npno`. The recordings are `full.npno`'s decoded strokes. A script
/// outside this crate wrote the expected bytes, so they are an independent oracle for
/// the rules.
///
/// Confirmed on hardware.
#[test]
fn a_piano_written_from_rules_alone_is_the_library_that_was_played() {
    let specimen = named("from-scratch.npno");
    let Entity::Piano(source) = &named("full.npno").entity else {
        panic!("full.npno is not a piano library");
    };
    let library = source.library().unwrap();
    let recordings: Vec<npno::encode::Recording> = library
        .strokes()
        .iter()
        .map(|stroke| {
            let audio = npno::codec::decode(stroke, library.channels()).unwrap();
            assert_eq!(audio.clipped, 0, "{stroke:?} saturates the decode");
            npno::encode::Recording {
                root: stroke.root,
                bank: stroke.bank().expect("a named bank"),
                layer: stroke.layer(),
                channels: audio.lanes,
            }
        })
        .collect();

    let (name, variant) = library.name();
    let built = npno::encode::build(
        &npno::encode::Donor::Rules(npno::encode::Rules::new(npno::encode::Kind::Grand)),
        &npno::encode::Options::new(&name).variant(&variant),
        &recordings,
    )
    .unwrap();
    let bytes = built
        .to_piano()
        .and_then(|p| nord_format::to_bytes(&Entity::Piano(p)))
        .unwrap();
    let differing: Vec<String> = bytes
        .iter()
        .zip(&specimen.bytes)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(at, (a, b))| format!("{at:#x}: wrote {a:#04x}, played {b:#04x}"))
        .take(8)
        .collect();
    assert!(
        bytes == specimen.bytes,
        "the rule-written library is not the one that was played ({} bytes out, {} in); \
         first differences: {}",
        bytes.len(),
        specimen.bytes.len(),
        differing.join(", ")
    );

    let again = npno::encode::rebuild(&built).unwrap();
    assert_eq!(
        again.library.to_body().unwrap(),
        built.to_body().unwrap(),
        "recoding the rule-written library changed it"
    );
}

/// The corpus libraries this crate's coder wrote, listed by name because nothing in a
/// file says who wrote it: a stereo build, a mono build, a synthetic library, a vendor
/// library recoded from its own audio, and one written with no template.
const OUR_LIBRARIES: [&str; 5] = [
    "full.npno",
    "mono.npno",
    "synth.npno",
    "rebuilt-clavinet.npno",
    "from-scratch.npno",
];

/// Recoding a library this crate wrote reproduces it byte for byte: every block is
/// identical, including its declared attenuation, and the container is laid out the
/// same way. The only value a recode may change is a statistic another encoder
/// measured, and these libraries hold none.
#[test]
fn a_library_this_crate_wrote_recodes_byte_for_byte() {
    for name in OUR_LIBRARIES {
        let specimen = named(name);
        let Entity::Piano(piano) = &specimen.entity else {
            panic!("{name} is not a piano library");
        };
        let library = piano
            .library()
            .unwrap_or_else(|e| panic!("{name}: parse: {e}"));
        let again = npno::encode::rebuild(&library).unwrap_or_else(|e| panic!("{name}: {e}"));
        for (stroke, recoded) in library.strokes().iter().zip(&again.strokes) {
            assert_eq!(
                recoded.identical,
                recoded.blocks,
                "{name}: {stroke:?} came back with {} block(s) restated",
                recoded.blocks - recoded.identical
            );
        }
        let rebuilt = again
            .library
            .to_piano()
            .and_then(|p| nord_format::to_bytes(&Entity::Piano(p)))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let at = rebuilt
            .iter()
            .zip(&specimen.bytes)
            .position(|(a, b)| a != b)
            .map(|i| format!("{i:#x}"))
            .unwrap_or_else(|| "the length".to_string());
        assert!(
            rebuilt == specimen.bytes,
            "{name}: the recode differs at {at} (in {} bytes, out {})",
            specimen.bytes.len(),
            rebuilt.len()
        );
    }
}

/// Every transform leaves a library the reader accepts: spans that tile the body to its
/// end, counts that sum to the directory, and every covered key naming a root that
/// still has strokes. The remaining strokes keep their audio byte for byte, because a
/// transform moves spans and never re-encodes one.
#[test]
fn piano_surgery_leaves_a_library_the_reader_accepts() {
    let mut seen = 0;
    for (specimen, piano) in pianos() {
        let where_ = specimen.path.display();
        let source = piano.library().unwrap();
        let covered: Vec<u8> = (0..128)
            .filter(|&k| source.key_map()[k as usize] != npno::UNCOVERED)
            .collect();
        let middle = covered[covered.len() / 2];

        let mut cases: Vec<(String, npno::Library<'_>)> = Vec::new();
        for bank in npno::Bank::ALL {
            let mut cut = source.clone();
            cut.drop_bank(bank);
            cases.push((format!("without the {bank} bank"), cut));
        }
        let mut loudest = source.clone();
        loudest.keep_layers(&npno::Layers::Loudest(2));
        cases.push(("two loudest layers".into(), loudest));
        let mut narrowed = source.clone();
        narrowed
            .cut_range(middle..=*covered.last().unwrap())
            .unwrap();
        cases.push(("upper half of the keyboard".into(), narrowed));
        let (low, high) = source.split_at(middle).unwrap();
        cases.push(("split low".into(), low));
        cases.push(("split high".into(), high));

        for (label, cut) in cases {
            let rebuilt = cut
                .to_piano()
                .unwrap_or_else(|e| panic!("{where_}: {label}: {e}"));
            let back = rebuilt
                .library()
                .unwrap_or_else(|e| panic!("{where_}: {label}: reading it back: {e}"));
            assert_eq!(
                back.strokes().len(),
                cut.strokes().len(),
                "{where_}: {label}"
            );
            assert_eq!(back.channels(), source.channels(), "{where_}: {label}");
            for (before, after) in cut.strokes().iter().zip(back.strokes()) {
                assert_eq!(before.root, after.root, "{where_}: {label}");
                assert_eq!(before.id(), after.id(), "{where_}: {label}");
                assert!(
                    before.audio() == after.audio(),
                    "{where_}: {label}: {before:?} audio was re-encoded"
                );
            }
            for (key, &root) in back.key_map().iter().enumerate() {
                assert!(
                    root == npno::UNCOVERED || back.roots().contains(&root),
                    "{where_}: {label}: key {key} plays root {root}, which has no stroke"
                );
            }
        }
        seen += 1;
    }
    assert!(seen > 0, "no piano library");
}

/// Dropping the resonance bank turns a large library into a small one. The remaining
/// strokes must be exactly the non-resonance strokes, and only their audio offsets may
/// change.
#[test]
fn dropping_a_pianos_resonance_bank_keeps_every_other_stroke_verbatim() {
    let mut seen = 0;
    for (specimen, piano) in pianos() {
        let where_ = specimen.path.display();
        let source = piano.library().unwrap();
        let expected: Vec<_> = source
            .strokes()
            .iter()
            .filter(|s| s.bank() != Some(npno::Bank::Resonance))
            .collect();
        let mut cut = source.clone();
        let change = cut.drop_bank(npno::Bank::Resonance);
        assert_eq!(
            change.strokes_removed,
            source.strokes().len() - expected.len(),
            "{where_}"
        );
        for (before, after) in expected.iter().zip(cut.strokes()) {
            // The record's first four bytes are the audio offset, which may change.
            assert_eq!(&before.record()[4..], &after.record()[4..], "{where_}");
        }
        seen += 1;
    }
    assert!(seen > 0, "no piano library");
}
