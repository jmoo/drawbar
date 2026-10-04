use super::hub::{self, Instrument};
use super::*;
use crate::formats::nsmp::codec;
use crate::formats::nsmp::encode::{self, Loop, NewZone, Options, DEFAULT_LOOP_DECAY};
use crate::formats::nsmp::keymap::{KeyTable, Level, GAIN_UNITY};
use crate::formats::nsmp::zone::VelocityWindow;

const OPEN: Choices = Choices {
    gain: None,
    name: None,
    overlap: None,
    loop_mark: None,
};

/// Every choice answered, for conversions that are not about choices.
fn answered() -> Choices {
    Choices {
        gain: Some(GainChoice::Clamp),
        name: Some(NameChoice::Truncate),
        overlap: Some(OverlapChoice::Lower),
        loop_mark: Some(LoopMarkChoice::Push),
    }
}

fn tone(frames: usize, amplitude: f64) -> Vec<i16> {
    (0..frames)
        .map(|k| (amplitude * (k as f64 * 0.0627).sin()).round() as i16)
        .collect()
}

fn entity(sample: Sample) -> Entity {
    Entity::Sample(sample)
}

fn one_zone(layout: Layout, options: Options, source: &[i16]) -> Entity {
    entity(encode::instrument(source, &options.layout(layout)).unwrap())
}

/// Two zones of a quiet tone, the upper one an octave up.
fn two_zones(layout: Layout, gain: f64) -> Entity {
    let source = tone(12_000, 3_000.0);
    let zone = |root_key, top_note, global_id| NewZone {
        source: &source,
        channels: 1,
        root_key,
        top_note,
        global_id,
        loops: None,
        secondary_start: encode::default_secondary_start(source.len(), None),
        shift: None,
        loop_decay: DEFAULT_LOOP_DECAY,
        gain,
    };
    entity(
        encode::multi_zone(
            encode::Instrument {
                name: "Two",
                map_gain: 1.0,
                predictor: encode::Predictor::Minimizing,
                layout,
                preset: encode::Preset::default(),
            },
            &[zone(72, 96, 2), zone(60, 71, 1)],
        )
        .unwrap(),
    )
}

fn sample(entity: &Entity) -> &Sample {
    match entity {
        Entity::Sample(sample) => sample,
        _ => panic!("not a sample instrument"),
    }
}

fn model(entity: &Entity) -> Instrument {
    hub::read(sample(entity)).unwrap().0
}

fn convert(entity: &Entity, layout: Layout, choices: &Choices) -> (Report, Entity) {
    let plan = plan(entity, Target::Nsmp(layout), choices).unwrap();
    let report = plan.report().clone();
    (report, Entity::Sample(plan.apply().unwrap()))
}

fn bytes(entity: &Entity) -> Vec<u8> {
    crate::to_bytes(entity).unwrap()
}

fn names(lines: &[Line], field: &Field) -> bool {
    lines.iter().any(|line| line.field == *field)
}

fn zone_field(index: usize, field: ZoneField) -> Field {
    Field::Zone { index, field }
}

#[test]
fn apply_refuses_while_a_choice_is_open() {
    let mut long = one_zone(Layout::V4, Options::new("A"), &tone(8_000, 2_000.0));
    let Entity::Sample(file) = &mut long else {
        unreachable!()
    };
    file.set_name(&"N".repeat(40)).unwrap();
    let plan = plan(&long, Target::Nsmp(Layout::V2), &OPEN).unwrap();
    assert_eq!(plan.open().len(), 1);
    assert_eq!(plan.open()[0].choice, Choice::Name);
    assert!(matches!(plan.apply(), Err(ConvertError::Open(open)) if open.len() == 1));
}

#[test]
fn a_name_past_the_narrow_field_is_truncated_or_renamed() {
    let mut long = one_zone(Layout::V3, Options::new("A"), &tone(8_000, 2_000.0));
    let Entity::Sample(file) = &mut long else {
        unreachable!()
    };
    let name = "Ü".repeat(20);
    file.set_name(&name).unwrap();
    let truncate = Choices {
        name: Some(NameChoice::Truncate),
        ..OPEN
    };
    let (report, out) = convert(&long, Layout::V2, &truncate);
    let line = report
        .changed
        .iter()
        .find(|l| l.field == Field::Name)
        .unwrap();
    assert_eq!(line.reason, Reason::Truncated { capacity: 31 });
    assert_eq!(sample(&out).name().unwrap(), "Ü".repeat(15));

    let rename = Choices {
        name: Some(NameChoice::Rename("Short".into())),
        ..OPEN
    };
    let (report, out) = convert(&long, Layout::V2, &rename);
    assert!(report.changed.iter().any(|l| l.reason == Reason::Renamed));
    assert_eq!(sample(&out).name().unwrap(), "Short");
}

#[test]
fn a_zone_gain_past_the_narrow_record_is_clamped_or_baked() {
    let loud = two_zones(Layout::V4, 20.0);
    let open = plan(&loud, Target::Nsmp(Layout::V2), &OPEN).unwrap();
    assert_eq!(
        open.open().iter().map(|o| o.choice).collect::<Vec<_>>(),
        [Choice::Gain, Choice::Gain]
    );
    let clamp = Choices {
        gain: Some(GainChoice::Clamp),
        ..OPEN
    };
    let (report, out) = convert(&loud, Layout::V2, &clamp);
    let line = report
        .changed
        .iter()
        .find(|l| l.field == zone_field(0, ZoneField::Gain))
        .unwrap();
    assert!(matches!(line.reason, Reason::Clamped { .. }), "{line:?}");
    let Sample::V2(file) = sample(&out) else {
        panic!("a narrow target")
    };
    assert!(file.zones().unwrap().iter().all(|z| z.gain == 0xff_ffff));

    let bake = Choices {
        gain: Some(GainChoice::Bake),
        ..OPEN
    };
    let (report, out) = convert(&loud, Layout::V2, &bake);
    let line = report
        .changed
        .iter()
        .find(|l| l.field == zone_field(0, ZoneField::Gain))
        .unwrap();
    let Reason::Baked { factor, .. } = line.reason else {
        panic!("{line:?}")
    };
    let ceiling = (f64::from(1u32 << 24) - 1.0) / f64::from(GAIN_UNITY);
    let stated = model(&loud).zones[0].gain;
    assert!((stated - 20.0).abs() < 1e-5);
    assert_eq!(factor, stated / ceiling);
    let before = model(&loud).zones[0].audio.clone();
    let after = model(&out).zones[0].audio.clone();
    let level = |l: &codec::Lattice| {
        let peak = l.fields.iter().map(|v| i64::from(v.abs())).max().unwrap();
        (peak as f64) * 2f64.powi(l.shift)
    };
    let ratio = level(&after) / level(&before);
    assert!((ratio - factor).abs() < 0.01, "the fields scale by {ratio}");
}

#[test]
fn overlapping_zones_into_tiled_ones_keep_the_shared_keys_where_chosen() {
    let mut overlapping = two_zones(Layout::V4, 1.0);
    let Entity::Sample(file) = &mut overlapping else {
        unreachable!()
    };
    file.set_zone_low_note(0, 66).unwrap();
    let open = plan(&overlapping, Target::Nsmp(Layout::V2), &OPEN).unwrap();
    assert_eq!(open.open()[0].choice, Choice::Overlap);

    let lower = Choices {
        overlap: Some(OverlapChoice::Lower),
        ..OPEN
    };
    let (report, out) = convert(&overlapping, Layout::V2, &lower);
    let line = report
        .changed
        .iter()
        .find(|l| l.field == zone_field(0, ZoneField::LowNote))
        .unwrap();
    assert_eq!(line.reason, Reason::LowerKeeps);
    let tops = |e: &Entity| -> Vec<u8> {
        sample(e)
            .zones()
            .unwrap()
            .iter()
            .map(|z| z.top_note)
            .collect()
    };
    assert_eq!(tops(&out), [96, 71]);

    let upper = Choices {
        overlap: Some(OverlapChoice::Upper),
        ..OPEN
    };
    let (report, out) = convert(&overlapping, Layout::V2, &upper);
    let line = report
        .changed
        .iter()
        .find(|l| l.field == zone_field(1, ZoneField::TopNote))
        .unwrap();
    assert_eq!(line.reason, Reason::UpperKeeps);
    assert_eq!(tops(&out), [96, 65]);
}

#[test]
fn a_gap_between_zones_into_tiled_ones_is_dropped() {
    let mut gapped = two_zones(Layout::V3, 1.0);
    let Entity::Sample(file) = &mut gapped else {
        unreachable!()
    };
    file.set_zone_low_note(0, 75).unwrap();
    let (report, _) = convert(&gapped, Layout::V2, &OPEN);
    let line = report
        .dropped
        .iter()
        .find(|l| l.field == zone_field(0, ZoneField::LowNote))
        .unwrap();
    assert_eq!(line.reason, Reason::Tiled { reaches: 72 });
}

/// A wide loop whose mark sits in the eight fields the narrow floor adds.
fn tight_loop() -> Entity {
    let source = tone(30_000, 3_000.0);
    let options = Options::new("Loop")
        .loops(Loop::new(4_000, 20_000))
        .secondary_start(3_960.0);
    one_zone(Layout::V4, options, &source)
}

#[test]
fn a_loop_mark_inside_the_narrow_floor_moves_the_resync_or_the_mark() {
    let tight = tight_loop();
    let audio = model(&tight).zones[0].audio.clone();
    let mark = audio.mark.unwrap();
    assert!((audio.resync_at + 64..audio.resync_at + 72).contains(&mark));
    let open = plan(&tight, Target::Nsmp(Layout::V2), &OPEN).unwrap();
    assert_eq!(open.open()[0].choice, Choice::LoopMark);

    let resync = Choices {
        loop_mark: Some(LoopMarkChoice::Resync),
        ..OPEN
    };
    let (report, out) = convert(&tight, Layout::V2, &resync);
    let to = mark - 72;
    assert!(report
        .changed
        .iter()
        .any(|l| l.reason == Reason::ResyncMoved { to }));
    let moved = model(&out).zones[0].audio.clone();
    assert_eq!((moved.resync_at, moved.mark), (to, Some(mark)));
    assert_eq!(moved.fields.len(), audio.fields.len());

    let push = Choices {
        loop_mark: Some(LoopMarkChoice::Push),
        ..OPEN
    };
    let (report, out) = convert(&tight, Layout::V2, &push);
    let to = audio.resync_at + 72;
    assert!(report
        .changed
        .iter()
        .any(|l| l.reason == Reason::MarkPushed { to }));
    let pushed = model(&out).zones[0].audio.clone();
    assert_eq!(pushed.mark, Some(to));
    assert_eq!(pushed.fields.len(), audio.fields.len() + (to - mark));
}

#[test]
fn a_pushed_narrow_mark_moves_back_where_the_wide_floor_allows() {
    let tight = tight_loop();
    let push = Choices {
        loop_mark: Some(LoopMarkChoice::Push),
        ..OPEN
    };
    let (_, narrow) = convert(&tight, Layout::V2, &push);
    let (report, wide) = convert(&narrow, Layout::V4, &OPEN);
    assert!(report
        .from_rules
        .iter()
        .any(|l| matches!(l.reason, Reason::MarkPulled { .. })));
    assert_eq!(
        model(&wide).zones[0].audio.fields,
        model(&tight).zones[0].audio.fields
    );
}

/// The model fields a test can set, how to set one to a value no writer defaults to,
/// and the report field that names it.
struct Modeled {
    name: &'static str,
    /// Whether a generation stores the field at all.
    stores: fn(Layout) -> bool,
    set: fn(&mut Instrument),
    get: fn(&Instrument) -> String,
    field: fn(&Field) -> bool,
}

fn zone(field: ZoneField) -> impl Fn(&Field) -> bool {
    move |f| matches!(f, Field::Zone { field: z, .. } if *z == field)
}

const MODELED: &[Modeled] = &[
    Modeled {
        name: "aux",
        stores: |_| true,
        set: |m| m.aux = 0x0001_0003,
        get: |m| m.aux.to_string(),
        field: |f| *f == Field::Aux,
    },
    Modeled {
        name: "name",
        stores: |_| true,
        set: |m| m.name = "Renamed".into(),
        get: |m| m.name.clone(),
        field: |f| *f == Field::Name,
    },
    Modeled {
        name: "sub_name",
        stores: |l| l != Layout::V2,
        set: |m| m.sub_name = "Sub".into(),
        get: |m| m.sub_name.clone(),
        field: |f| *f == Field::SubName,
    },
    Modeled {
        name: "category",
        stores: |_| true,
        set: |m| m.category = Some(3),
        get: |m| format!("{:?}", m.category),
        field: |f| *f == Field::Category,
    },
    Modeled {
        name: "sub_category",
        stores: |_| true,
        set: |m| m.sub_category = Some(2),
        get: |m| format!("{:?}", m.sub_category),
        field: |f| *f == Field::SubCategory,
    },
    Modeled {
        name: "timbre",
        stores: |l| l == Layout::V2,
        set: |m| m.timbre = Some(4),
        get: |m| format!("{:?}", m.timbre),
        field: |f| *f == Field::Timbre,
    },
    Modeled {
        name: "envelope",
        stores: |l| l == Layout::V2,
        set: |m| m.envelope = Some(2),
        get: |m| format!("{:?}", m.envelope),
        field: |f| *f == Field::Envelope,
    },
    Modeled {
        name: "motion",
        stores: |l| l == Layout::V2,
        set: |m| m.motion = Some(0),
        get: |m| format!("{:?}", m.motion),
        field: |f| *f == Field::Motion,
    },
    Modeled {
        name: "production",
        stores: |l| l == Layout::V2,
        set: |m| m.production = Some("Studio".into()),
        get: |m| format!("{:?}", m.production),
        field: |f| *f == Field::Production,
    },
    Modeled {
        name: "origin",
        stores: |l| l == Layout::V2,
        set: |m| m.origin = Some("Here".into()),
        get: |m| format!("{:?}", m.origin),
        field: |f| *f == Field::Origin,
    },
    Modeled {
        name: "map level",
        stores: |_| true,
        set: |m| m.keys.instrument = Level::new(GAIN_UNITY / 2, -256).unwrap(),
        get: |m| format!("{:?}", m.keys.instrument),
        field: |_| false,
    },
    Modeled {
        name: "per-key table",
        stores: |l| l == Layout::V2,
        set: |m| {
            m.keys
                .set_key(64, Level::new(GAIN_UNITY / 4, 20).unwrap())
                .unwrap()
        },
        get: |m| format!("{:?}", m.keys.key(64).unwrap()),
        field: |f| *f == Field::KeyTable,
    },
    Modeled {
        name: "dynamics_enabled",
        stores: |_| true,
        set: |m| m.dynamics_enabled = true,
        get: |m| m.dynamics_enabled.to_string(),
        field: |_| false,
    },
    Modeled {
        name: "velocity_to_amplitude",
        stores: |l| l == Layout::V2,
        set: |m| m.velocity_to_amplitude = Some(2),
        get: |m| format!("{:?}", m.velocity_to_amplitude),
        field: |f| *f == Field::VelocityToAmplitude,
    },
    Modeled {
        name: "velocity_to_timbre",
        stores: |l| l == Layout::V2,
        set: |m| m.velocity_to_timbre = Some(0),
        get: |m| format!("{:?}", m.velocity_to_timbre),
        field: |f| *f == Field::VelocityToTimbre,
    },
    Modeled {
        name: "root_key",
        stores: |_| true,
        set: |m| m.zones[0].root_key = 74,
        get: |m| m.zones[0].root_key.to_string(),
        field: |_| false,
    },
    Modeled {
        name: "top_note",
        stores: |_| true,
        set: |m| m.zones[0].top_note = 100,
        get: |m| m.zones[0].top_note.to_string(),
        field: zone_top,
    },
    Modeled {
        name: "low_note",
        stores: |l| l != Layout::V2,
        set: |m| m.zones[1].low_note = m.zones[1].low_note.map(|_| 30),
        get: |m| format!("{:?}", m.zones[1].low_note),
        field: zone_low,
    },
    Modeled {
        name: "global_id",
        stores: |_| true,
        set: |m| m.zones[0].global_id = 9,
        get: |m| m.zones[0].global_id.to_string(),
        field: zone_id,
    },
    Modeled {
        name: "gain",
        stores: |_| true,
        set: |m| m.zones[0].gain = 0.5,
        get: |m| format!("{:.6}", m.zones[0].gain),
        field: zone_gain,
    },
    Modeled {
        name: "loop_decay",
        stores: |l| l != Layout::V2,
        set: |m| m.zones[0].loop_decay = m.zones[0].loop_decay.map(|_| 35.5),
        get: |m| format!("{:?}", m.zones[0].loop_decay),
        field: zone_decay,
    },
    Modeled {
        name: "rel_strength",
        stores: |_| true,
        set: |m| m.zones[0].rel_strength = 300,
        get: |m| m.zones[0].rel_strength.to_string(),
        field: |_| false,
    },
    Modeled {
        name: "velocity",
        stores: |l| l != Layout::V2,
        set: |m| {
            m.zones[0].velocity = m.zones[0]
                .velocity
                .map(|_| VelocityWindow { low: 10, high: 99 })
        },
        get: |m| format!("{:?}", m.zones[0].velocity),
        field: zone_velocity,
    },
];

fn zone_top(f: &Field) -> bool {
    zone(ZoneField::TopNote)(f)
}
fn zone_low(f: &Field) -> bool {
    zone(ZoneField::LowNote)(f)
}
fn zone_id(f: &Field) -> bool {
    zone(ZoneField::GlobalId)(f)
}
fn zone_gain(f: &Field) -> bool {
    zone(ZoneField::Gain)(f)
}
fn zone_decay(f: &Field) -> bool {
    zone(ZoneField::LoopDecay)(f)
}
fn zone_velocity(f: &Field) -> bool {
    zone(ZoneField::Velocity)(f)
}

/// Set each modeled field to a value no writer defaults to and convert: the target
/// reads the value back, or the report names the field.
#[test]
fn every_modeled_field_reaches_the_target_or_the_report() {
    for from in Layout::ALL {
        let base = model(&two_zones(from, 1.0));
        for modeled in MODELED {
            if !(modeled.stores)(from) {
                continue;
            }
            let mut set = base.clone();
            (modeled.set)(&mut set);
            assert_ne!(
                (modeled.get)(&set),
                (modeled.get)(&base),
                "{} is already at the value the test sets",
                modeled.name
            );
            let source = Entity::Sample(hub::write(&set, from).unwrap());
            assert_eq!(
                (modeled.get)(&model(&source)),
                (modeled.get)(&set),
                "{} does not read back from its own {from:?}",
                modeled.name
            );
            for to in Layout::ALL {
                let (report, out) = convert(&source, to, &answered());
                let lines = [&report.dropped, &report.changed, &report.from_rules];
                let named = lines
                    .iter()
                    .any(|lines| lines.iter().any(|l| (modeled.field)(&l.field)));
                assert!(
                    (modeled.get)(&model(&out)) == (modeled.get)(&set) || named,
                    "{} set in {from:?} neither reaches {to:?} nor its report: {report:?}",
                    modeled.name
                );
            }
        }
    }
}

#[test]
fn a_conversion_both_ways_with_nothing_reported_is_the_identity() {
    let stereo: Vec<i16> = tone(12_000, 5_000.0)
        .into_iter()
        .zip(tone(12_000, 2_000.0))
        .flat_map(|(l, r)| [l, r])
        .collect();
    let sources = [
        one_zone(Layout::V3, Options::new("Quiet"), &tone(12_000, 2_000.0)),
        one_zone(Layout::V4, Options::new("Quiet"), &tone(12_000, 2_000.0)),
        one_zone(Layout::V3, Options::new("Stereo").channels(2), &stereo),
        two_zones(Layout::V4, 0.7),
    ];
    let mut across = 0;
    for source in &sources {
        let from = sample(source).layout().unwrap();
        for to in Layout::ALL {
            let (there, out) = convert(source, to, &OPEN);
            let (back, home) = convert(&out, from, &OPEN);
            if there.is_empty() && back.is_empty() {
                across += usize::from(to != from);
                assert_eq!(bytes(&home), bytes(source), "{from:?} through {to:?}");
            }
        }
    }
    assert!(
        across > 0,
        "no round trip between generations reported nothing"
    );
}

#[test]
fn a_byte_no_reader_places_is_reported_by_section_and_range() {
    let mut source = one_zone(Layout::V3, Options::new("Bytes"), &tone(8_000, 2_000.0));
    let Entity::Sample(Sample::V3(file)) = &mut source else {
        panic!("a wide source")
    };
    let sty = crate::formats::nsmp::section::find_mut(
        &mut file.body.sections,
        crate::formats::nsmp::section::STY4,
    )
    .unwrap();
    sty.payload[0] = 0x55;
    let (report, _) = convert(&source, Layout::V4, &OPEN);
    let field = Field::Bytes {
        section: "sty".into(),
        range: 0..1,
        known_as: None,
    };
    let line = report.dropped.iter().find(|l| l.field == field).unwrap();
    assert_eq!(
        (line.value.as_str(), &line.reason),
        ("55", &Reason::Unmodeled)
    );
}

#[test]
fn widening_reports_what_the_narrow_chain_never_stored() {
    let mut source = two_zones(Layout::V2, 1.0);
    let Entity::Sample(Sample::V2(file)) = &mut source else {
        panic!("a narrow source")
    };
    let mut keys = KeyTable::NEUTRAL;
    keys.set_key(64, Level::new(GAIN_UNITY / 2, 0).unwrap())
        .unwrap();
    file.set_key_table(&keys).unwrap();
    let (report, out) = convert(&source, Layout::V3, &OPEN);
    assert!(names(&report.dropped, &Field::KeyTable));
    for index in 0..2 {
        assert!(names(
            &report.from_rules,
            &zone_field(index, ZoneField::LoopDecay)
        ));
        assert!(names(
            &report.from_rules,
            &zone_field(index, ZoneField::Peak)
        ));
    }
    assert_eq!(model(&out).keys.adjusted().count(), 0);
}

#[test]
fn narrowing_fills_the_narrow_preset_and_categories_by_rule() {
    let (report, _) = convert(&two_zones(Layout::V4, 1.0), Layout::V2, &OPEN);
    for field in [
        Field::Timbre,
        Field::Envelope,
        Field::Motion,
        Field::Production,
        Field::Origin,
        Field::VelocityToAmplitude,
        Field::VelocityToTimbre,
    ] {
        assert!(names(&report.from_rules, &field), "{field}");
    }
}

#[test]
fn a_shift_the_target_rule_coarsens_is_reported() {
    let loud = tone(20_000, 16_000.0);
    let v4 = one_zone(Layout::V4, Options::new("Loud"), &loud);
    let (report, out) = convert(&v4, Layout::V2, &OPEN);
    let from = model(&v4).zones[0].audio.shift;
    let to = model(&out).zones[0].audio.shift;
    assert_eq!(to, from + 1);
    let line = report
        .from_rules
        .iter()
        .find(|l| l.field == zone_field(0, ZoneField::Shift))
        .unwrap();
    assert_eq!(
        line.reason,
        Reason::ShiftRule {
            layout: Layout::V2,
            to
        }
    );
}

#[test]
fn a_file_that_is_not_a_sample_instrument_is_refused() {
    let program = crate::from_stream(&mut std::io::Cursor::new(
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ne5/default.ne5p"
        ))
        .unwrap(),
    ))
    .unwrap();
    assert!(matches!(
        plan(&program, Target::Nsmp(Layout::V2), &OPEN),
        Err(ConvertError::NotASample(_))
    ));
}

/// A loop too short for a generation's opening run or packets is laid out over more
/// periods, and the report says so: a loop the wide packets take in one period can
/// need more in the narrow chain's larger ones.
#[test]
fn a_loop_laid_out_over_more_periods_is_reported() {
    let base = model(&two_zones(Layout::V4, 1.0));
    let fields: Vec<i32> = (0..3_000).map(|f| (f * 37) % 401 - 200).collect();
    let mut repeated = 0;
    for length in (96..=480).step_by(16) {
        let mut instrument = base.clone();
        for zone in &mut instrument.zones {
            zone.audio = codec::Lattice {
                fields: fields.clone(),
                channels: 1,
                shift: 0,
                peak: codec::Peak::Signed(-50),
                resync_at: 400,
                mark: Some(fields.len() - length),
            };
        }
        let source = Entity::Sample(hub::write(&instrument, Layout::V4).unwrap());
        let given = model(&source).zones[0].audio.fields.len();
        let (report, out) = convert(&source, Layout::V2, &answered());
        let laid = model(&out).zones[0].audio.fields.len();
        let named = report.changed.iter().any(|l| {
            l.field == zone_field(0, ZoneField::Stream)
                && matches!(l.reason, Reason::LoopRepeated { .. })
        });
        assert_eq!(
            named,
            laid > given,
            "a {length}-field loop: {given} fields, {laid} laid"
        );
        repeated += usize::from(named);
    }
    assert!(repeated > 0, "no loop needed more periods");
}
