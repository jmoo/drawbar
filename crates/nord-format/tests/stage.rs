//! The Stage bodies in the default suite: a synthetic specimen per decoded body, so
//! placing, gating and the round-trip invariant are exercised without the corpus.
//!
//! A zeroed body is a legal one for every Stage format; a body of patterned bytes is
//! not (ranged fields refuse it) — so the re-encode check runs on zeros, and the
//! pattern is only asked to fail cleanly rather than panic.

use nord_format::bits::Packed;
use nord_format::cbin::{Cbin, Header};
use nord_format::components::ProgramCategory;
use nord_format::fields::{ControlKind, FieldSpec, Unit};
use nord_format::formats::{ns2, ns3, ns4};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Synth};

macro_rules! stage_body {
    ($name:ident, $body:ty, $len:expr, $format:expr, $versions:expr, $wrap:expr, $unwrap:pat => $inner:expr) => {
        mod $name {
            use super::*;

            fn file(body: $body, version: u32) -> Vec<u8> {
                let file = Cbin {
                    header: Header::new($format, (0, 0), version),
                    body,
                };
                nord_format::to_bytes(&$wrap(file)).expect("a synthetic file encodes")
            }

            #[test]
            fn a_zeroed_body_decodes_and_re_encodes_byte_for_byte() {
                let body = <$body>::try_from([0u8; $len]).expect("a zeroed body decodes");
                let version = *$versions.last().unwrap();
                let bytes = file(body, version);
                let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
                    .expect("the file reads back");
                match &entity {
                    $unwrap => assert_eq!($inner.header.version, version),
                    other => panic!("decoded to {other:?}"),
                }
                assert_eq!(nord_format::to_bytes(&entity).unwrap(), bytes);
            }

            #[test]
            fn unclaimed_bits_ride_through_a_re_encode() {
                // Every byte set: what no field claims must come back as it went in,
                // and what a field claims either decodes or is refused — never wrapped.
                let raw = [0xffu8; $len];
                if let Ok(body) = <$body>::try_from(raw) {
                    assert_eq!(<[u8; $len]>::from(&body), raw);
                }
            }

            #[test]
            fn an_unknown_version_is_refused() {
                let body = <$body>::try_from([0u8; $len]).unwrap();
                let bytes = file(body, 999_999);
                let err = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
                    .expect_err("a version the offsets were never checked against");
                assert!(err.to_string().contains("999999"), "{err}");
            }
        }
    };
}

stage_body!(
    stage2_program,
    ns2::Program,
    ns2::program::BODY_LEN,
    ns2::program::FORMAT,
    ns2::program::KNOWN_VERSIONS,
    |f| Entity::Program(Program::Stage2(f)),
    Entity::Program(Program::Stage2(f)) => f
);
stage_body!(
    stage2_live,
    ns2::Program,
    ns2::program::BODY_LEN,
    ns2::live::FORMAT,
    ns2::program::KNOWN_VERSIONS,
    |f| Entity::Live(Live::Stage2(f)),
    Entity::Live(Live::Stage2(f)) => f
);
stage_body!(
    stage3_program,
    ns3::Program,
    ns3::program::BODY_LEN,
    ns3::program::FORMAT,
    ns3::program::KNOWN_VERSIONS,
    |f| Entity::Program(Program::Stage3(f)),
    Entity::Program(Program::Stage3(f)) => f
);
stage_body!(
    stage3_live,
    ns3::Program,
    ns3::program::BODY_LEN,
    ns3::live::FORMAT,
    ns3::program::KNOWN_VERSIONS,
    |f| Entity::Live(Live::Stage3(f)),
    Entity::Live(Live::Stage3(f)) => f
);
stage_body!(
    stage3_synth,
    ns3::SynthPreset,
    ns3::synth::BODY_LEN,
    ns3::synth::FORMAT,
    ns3::synth::KNOWN_VERSIONS,
    |f| Entity::Synth(Synth::Stage3(f)),
    Entity::Synth(Synth::Stage3(f)) => f
);
stage_body!(
    stage4_program,
    ns4::Program,
    ns4::program::BODY_LEN,
    ns4::program::FORMAT,
    ns4::program::KNOWN_VERSIONS,
    |f| Entity::Program(Program::Stage4(f)),
    Entity::Program(Program::Stage4(f)) => f
);
stage_body!(
    stage4_live,
    ns4::Program,
    ns4::program::BODY_LEN,
    ns4::live::FORMAT,
    ns4::program::KNOWN_VERSIONS,
    |f| Entity::Live(Live::Stage4(f)),
    Entity::Live(Live::Stage4(f)) => f
);
stage_body!(
    stage4_synth,
    ns4::synth::SynthPreset,
    ns4::synth::BODY_LEN,
    ns4::synth::FORMAT,
    ns4::synth::KNOWN_VERSIONS,
    |f| Entity::Synth(Synth::Stage4(f)),
    Entity::Synth(Synth::Stage4(f)) => f
);
stage_body!(
    stage4_piano_preset,
    ns4::piano_preset::PianoPreset,
    ns4::piano_preset::BODY_LEN,
    ns4::piano_preset::FORMAT,
    ns4::piano_preset::KNOWN_VERSIONS,
    |f| Entity::PianoPreset(PianoPreset::Stage4(f)),
    Entity::PianoPreset(PianoPreset::Stage4(f)) => f
);
stage_body!(
    stage4_organ_preset,
    ns4::organ_preset::OrganPreset,
    ns4::organ_preset::BODY_LEN,
    ns4::organ_preset::FORMAT,
    ns4::organ_preset::KNOWN_VERSIONS,
    |f| Entity::OrganPreset(OrganPreset::Stage4(f)),
    Entity::OrganPreset(OrganPreset::Stage4(f)) => f
);

#[test]
fn program_split_bits_have_exact_placements() {
    let mut raw = [0u8; ns2::program::BODY_LEN];
    raw[3] = 0x04;
    let stage2 = ns2::Program::try_from(raw).unwrap();
    assert!(stage2.split_enabled());
    assert_eq!(<[u8; ns2::program::BODY_LEN]>::from(&stage2), raw);

    let mut raw = [0u8; ns3::program::BODY_LEN];
    raw[5] = 0x10;
    let stage3 = ns3::Program::try_from(raw).unwrap();
    assert!(stage3.split_enabled);
    assert_eq!(<[u8; ns3::program::BODY_LEN]>::from(&stage3), raw);

    let mut raw = [0u8; ns4::program::BODY_LEN];
    raw[5] = 0x80;
    let stage4 = ns4::Program::try_from(raw).unwrap();
    assert!(stage4.split_enabled);
    assert_eq!(<[u8; ns4::program::BODY_LEN]>::from(&stage4), raw);
}

/// The placements in the Stage 4 presets the corpus confirms rather than a published
/// table: layer A's zone, one layer stride above B's in each preset.
#[test]
fn stage4_preset_zones_sit_one_stride_apart() {
    // Zone 9 is placed directly into both layers at each preset type's declared offsets.
    let mut raw = [0u8; ns4::synth::BODY_LEN];
    raw[42] |= 0b0010_0100;
    raw[93] |= 0b0010_0100;
    let body = ns4::synth::SynthPreset::try_from(raw).unwrap();
    assert_eq!(format!("{:?}", body.synth_a_performance.kb_zones), "V9");
    assert_eq!(format!("{:?}", body.synth_b_performance.kb_zones), "V9");
    assert_eq!(<[u8; ns4::synth::BODY_LEN]>::from(&body), raw);

    let mut raw = [0u8; ns4::organ_preset::BODY_LEN];
    raw[23] |= 0b1001_0000;
    raw[54] |= 0b1001_0000;
    let body = ns4::organ_preset::OrganPreset::try_from(raw).unwrap();
    assert_eq!(format!("{:?}", body.organ_a.kb_zones), "V9");
    assert_eq!(format!("{:?}", body.organ_b.kb_zones), "V9");
    assert_eq!(<[u8; ns4::organ_preset::BODY_LEN]>::from(&body), raw);

    let mut raw = [0u8; ns4::piano_preset::BODY_LEN];
    raw[18] |= 0b1001_0000;
    raw[30] |= 0b1001_0000;
    let body = ns4::piano_preset::PianoPreset::try_from(raw).unwrap();
    assert_eq!(format!("{:?}", body.piano_a.kb_zones), "V9");
    assert_eq!(format!("{:?}", body.piano_b.kb_zones), "V9");
    assert_eq!(<[u8; ns4::piano_preset::BODY_LEN]>::from(&body), raw);
}

/// Each Stage 4 preset places the layer type its program already declares, so the check
/// that matters is that nesting moved nothing. The bits below are the ones the preset
/// spelled out before it nested, taken at the far end of every layer — where a block
/// placed one byte out would show first.
#[test]
fn stage4_preset_layers_end_where_the_offsets_say() {
    let mut raw = [0u8; ns4::organ_preset::BODY_LEN];
    raw[51] = 0b0000_1000; // organ A percussion volume soft, bit 412
    raw[82] = 0b0000_1000; // organ B percussion volume soft, bit 660
    let body = ns4::organ_preset::OrganPreset::try_from(raw).unwrap();
    assert!(body.organ_a.percussion_volume_soft_enabled);
    assert!(body.organ_b.percussion_volume_soft_enabled);
    assert_eq!(<[u8; ns4::organ_preset::BODY_LEN]>::from(&body), raw);

    let mut raw = [0u8; ns4::piano_preset::BODY_LEN];
    raw[24] = 0b0000_1000; // piano A soft release, bit 196
    raw[36] = 0b0000_1000; // piano B soft release, bit 292
    let body = ns4::piano_preset::PianoPreset::try_from(raw).unwrap();
    assert!(body.piano_a.soft_rel_enabled);
    assert!(body.piano_b.soft_rel_enabled);
    assert_eq!(<[u8; ns4::piano_preset::BODY_LEN]>::from(&body), raw);

    let mut raw = [0u8; ns4::synth::BODY_LEN];
    raw[46] = 0b1000_0000; // synth A extern, bit 368
    raw[97] = 0b1000_0000; // synth B extern, bit 776
    raw[148] = 0b1000_0000; // synth C extern, bit 1184
    let body = ns4::synth::SynthPreset::try_from(raw).unwrap();
    assert!(body.synth_a_performance.extern_enabled);
    assert!(body.synth_b_performance.extern_enabled);
    assert!(body.synth_c_performance.extern_enabled);
    assert_eq!(<[u8; ns4::synth::BODY_LEN]>::from(&body), raw);
}

/// Look one field up in a body's registry, failing by name when it is not there.
fn spec<'a>(specs: &'a [FieldSpec], name: &str) -> &'a FieldSpec {
    specs
        .iter()
        .find(|spec| spec.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
}

/// A Stage 2 delay parameter is followed by the three slots that morph it, as every
/// other run in the body is — so the slots carry the parameter's own name and one width.
#[test]
fn stage2_delay_slots_carry_the_name_of_what_they_morph() {
    let specs = ns2::Slot::field_specs();
    for parameter in [
        "delay_tempo_master_clock_divisor",
        "delay_tempo",
        "delay_amount",
    ] {
        let widths: Vec<u32> = ["wheel", "aftertouch", "ctrl_pedal"]
            .iter()
            .map(|control| spec(&specs, &format!("{parameter}_{control}")).width)
            .collect();
        assert_eq!(widths[0], widths[1], "{parameter}");
        assert_eq!(widths[1], widths[2], "{parameter}");
    }
}

/// A morph slot beside a switch is still a morph slot: it is drawn on the switch, not as
/// a control of its own, whichever body declares it.
#[test]
fn switch_morph_slots_bind_to_the_switch_beside_them() {
    let bound = |specs: &[FieldSpec], slot: &str, parent: &str| {
        assert_eq!(
            spec(specs, slot).morph_parent().as_deref(),
            Some(parent),
            "{slot}",
        );
    };

    let globals = ns2::Program::field_specs();
    let slot = ns2::Slot::field_specs();
    let panel = ns3::Panel::field_specs();
    let voice = ns4::SynthVoice::field_specs();
    for control in ["wheel", "aftertouch", "ctrl_pedal"] {
        bound(
            &globals,
            &format!("rotary_speaker_speed_{control}"),
            "rotary_speaker_speed",
        );
        bound(
            &slot,
            &format!("synth_skip_sample_attack_{control}"),
            "synth_skip_sample_attack",
        );
        bound(
            &panel,
            &format!("extern_midi_cc_{control}"),
            "extern_midi_cc",
        );
        bound(
            &voice,
            &format!("filter_resonance_{control}"),
            "filter_resonance_freq_hp",
        );
    }
}

/// One representation of a concept: a Stage 2 slot spells every section's keyboard zone
/// and every clocked run's divisor the same way, so a caller finds them by section.
#[test]
fn a_stage2_slot_spells_its_repeated_concepts_alike() {
    let specs = ns2::Slot::field_specs();
    for section in ["organ", "piano", "synth", "extern"] {
        spec(&specs, &format!("{section}_kb_zone"));
    }
    for run in ["effect_1_rate", "effect_2_rate", "delay_tempo"] {
        spec(&specs, &format!("{run}_master_clock_divisor"));
    }
}

/// The rotor speed is one switch on every Stage, so the values a caller sets it to are
/// the same on every Stage.
#[test]
fn every_stage_offers_the_same_rotor_speeds() {
    let legal = |specs: &[FieldSpec], name: &str| (spec(specs, name).legal)();
    let stage2 = ns2::Program::field_specs();
    let stage3 = ns3::Program::field_specs();
    let stage4 = ns4::Program::field_specs();
    assert_eq!(
        legal(&stage2, "rotary_speaker_speed"),
        legal(&stage3, "rotary_speaker_speed"),
    );
    assert_eq!(
        legal(&stage3, "rotary_speaker_speed"),
        legal(&stage4, "rotary_speaker_slow_fast"),
    );
}

/// A vibrato/chorus mode is set by the name the panel prints on it, so the variant a
/// caller spells and the label it reads are the same word.
#[test]
fn stage3_organ_vibrato_modes_are_named_for_what_the_panel_prints() {
    for stored in 0..6u64 {
        let mode = ns3::program::OrganVibratoMode::from_bits(stored).expect("decoding is total");
        assert_eq!(format!("{mode:?}"), mode.label().expect("a named mode"));
    }
}

/// A MIDI number, a filter cutoff and half a split word are not panel `0..10` knobs. Each
/// is typed for what it is, so an interface never labels one with a reading it does not
/// have.
#[test]
fn slots_that_are_not_panel_knobs_are_not_typed_as_knobs() {
    let panel_knob = ControlKind::Knob(Unit::Panel10);

    let slot = ns2::Slot::field_specs();
    for midi in [
        "extern_midi_cc_number",
        "extern_midi_program",
        "extern_midi_bank_select_cc00",
        "extern_midi_bank_select_cc32",
    ] {
        assert_eq!(spec(&slot, midi).control, ControlKind::Number, "{midi}");
    }

    let panel = ns3::Panel::field_specs();
    for half in [
        "delay_tempo_lsw",
        "delay_tempo_wheel_lsw",
        "delay_tempo_aftertouch_lsw",
        "delay_tempo_ctrl_pedal_lsw",
    ] {
        assert_eq!(spec(&panel, half).control, ControlKind::Number, "{half}");
    }

    let voice = ns4::SynthVoice::field_specs();
    assert_eq!(
        spec(&voice, "filter_freq").control,
        ControlKind::Knob(Unit::Hertz),
    );
    assert_ne!(spec(&voice, "filter_freq").control, panel_knob);
}

/// The category is the whole id the header carries or nothing: a wider value names no
/// category rather than being truncated into one.
#[test]
fn a_program_category_reads_the_whole_aux_id() {
    let mut header = Header::new(ns3::program::FORMAT, (0, 0), 304);

    header.aux = 0x07;
    assert_eq!(ProgramCategory::of(&header), Some(ProgramCategory::Organ));

    header.aux = 0x0107;
    assert_eq!(ProgramCategory::of(&header), None, "0x0107 is not Organ");

    header.aux = 0xffff_ffff;
    assert_eq!(ProgramCategory::of(&header), None, "no category at all");
}
