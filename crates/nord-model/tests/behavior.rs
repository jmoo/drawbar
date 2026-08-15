//! What the Electro 5 model answers, state by state.

use nord_format::formats::ne5;
use nord_format::layout::BodyLayout;
use nord_model::electro5::{
    electro5, LOWER_PART, ORGAN_SECTION, ORGAN_TYPE, TRANSPOSE, TRANSPOSE_ENABLED, UPPER_PART,
};
use nord_model::{DeviceModel, Provenance, State};

/// A program with `sets` applied, as the model sees it: every field's value, and the
/// body the unclaimed bits live in.
fn state(sets: &[(&str, &str)]) -> State {
    let mut body = ne5::program::new((0, 0).try_into().expect("a program slot")).body;
    for (path, value) in sets {
        body.set_field(path, value)
            .unwrap_or_else(|e| panic!("{path} = {value:?}: {e}"));
    }
    let raw = <[u8; ne5::program::BODY_LEN]>::from(&body);
    State::from_fields(&body.fields()).with_body(raw)
}

fn model() -> DeviceModel {
    electro5()
}

/// The organ models a selection leaves live, as path prefixes.
fn live_models(model: &DeviceModel, state: &State) -> Vec<&'static str> {
    ["b3_", "vox_", "farfisa_", "pipe_"]
        .into_iter()
        .filter(|prefix| {
            let path = format!("organ_panel.{prefix}preset2_drawbars");
            model.live(state, &path)
        })
        .collect()
}

// ── surface ─────────────────────────────────────────────────────────────────────

/// An engine no part is playing answers to nothing, and pointing a part back at it
/// brings the whole section round again.
#[test]
fn the_organ_section_is_live_only_while_a_part_plays_it() {
    let model = model();

    let quiet = state(&[(LOWER_PART, "Piano"), (UPPER_PART, "Sample")]);
    assert!(!model.section_live(&quiet, ORGAN_SECTION));
    assert!(!model.surface(&quiet).live("organ_panel.b3_vib"));

    let playing = state(&[(LOWER_PART, "Organ"), (UPPER_PART, "Sample")]);
    assert!(model.section_live(&playing, ORGAN_SECTION));
    assert!(model.surface(&playing).live("organ_panel.b3_vib"));

    // Either part is enough.
    let upper = state(&[(LOWER_PART, "Piano"), (UPPER_PART, "Organ")]);
    assert!(model.section_live(&upper, ORGAN_SECTION));
}

/// One model's storage is live at a time, and b3+bass reads the B3's.
#[test]
fn only_the_selected_organ_models_paths_are_live() {
    let model = model();
    for (selection, expected) in [
        ("B3", "b3_"),
        ("B3Bass", "b3_"),
        ("Vox", "vox_"),
        ("Farfisa", "farfisa_"),
        ("Pipe", "pipe_"),
    ] {
        let state = state(&[(ORGAN_TYPE, selection)]);
        assert_eq!(live_models(&model, &state), [expected], "{selection}");
    }
}

/// A selection the library cannot name leaves no model live, but the section itself is
/// still the section the panel shows.
#[test]
fn an_unnamed_selection_leaves_the_section_up_and_every_model_dark() {
    let model = model();
    let state = state(&[(ORGAN_TYPE, "unknown (6)")]);
    assert!(model.section_live(&state, ORGAN_SECTION));
    assert!(live_models(&model, &state).is_empty());
}

/// ⚠️ In b3+bass, preset 1 is the bass manual: the pair outside the nine-nibble block is
/// live and the block itself is not. The two are never live together.
#[test]
fn b3_bass_swaps_the_first_preset_for_the_bass_manual() {
    let model = model();
    let bars = "organ_panel.b3_bass_bar1";
    let block = "organ_panel.b3_preset1_drawbars";

    let bass = state(&[(ORGAN_TYPE, "B3Bass")]);
    assert!(model.live_within(&bass, ORGAN_SECTION, bars));
    assert!(!model.live_within(&bass, ORGAN_SECTION, block));

    let plain = state(&[(ORGAN_TYPE, "B3")]);
    assert!(!model.live_within(&plain, ORGAN_SECTION, bars));
    assert!(model.live_within(&plain, ORGAN_SECTION, block));
}

/// The EQ's routing means nothing until the EQ is engaged: a stored 0 there is *lower*,
/// not off, so the enable has to be read first.
#[test]
fn the_equalizer_routing_is_inert_until_the_equalizer_is_on() {
    let model = model();
    let part = "effects_panel.equalizer_part";
    assert!(!model.live(&state(&[("effects_panel.equalizer_on", "false")]), part));
    assert!(model.live(&state(&[("effects_panel.equalizer_on", "true")]), part));
}

/// The organ section speaks for every model's own state, selected or not; a field
/// naming no model is left to render as itself.
#[test]
fn the_section_speaks_for_every_models_state_and_nothing_else() {
    let model = model();
    for path in [
        "organ_panel.b3_vib",
        "organ_panel.vox_preset2_drawbars",
        "organ_panel.farfisa_vib",
        "organ_panel.pipe_preset1_drawbars",
    ] {
        assert!(model.gated_within(ORGAN_SECTION, path), "{path}");
    }
    assert!(!model.gated_within(ORGAN_SECTION, "organ_panel.something_new"));
    assert!(!model.gated_within(ORGAN_SECTION, "center_panel.gain"));
}

// ── narrow ──────────────────────────────────────────────────────────────────────

/// One piano engine, so the parts cannot both have it — and a program that already does
/// keeps the value listed, or there would be no way to edit out of it.
#[test]
fn a_part_is_not_offered_the_engine_the_other_part_has() {
    let model = model();
    let legal = ["Organ", "Piano", "Sample"].map(str::to_string);

    let taken = state(&[(LOWER_PART, "Piano"), (UPPER_PART, "Organ")]);
    assert_eq!(
        model.choices(&taken, UPPER_PART, &legal, "Organ"),
        ["Organ", "Sample"],
    );

    let both = state(&[(LOWER_PART, "Piano"), (UPPER_PART, "Piano")]);
    assert_eq!(
        model.choices(&both, UPPER_PART, &legal, "Piano"),
        ["Organ", "Sample", "Piano"],
    );
}

/// Two spellings of off would read as two different settings.
#[test]
fn the_older_spelling_of_off_is_not_offered_alongside_off() {
    let model = model();
    let state = state(&[]);
    let legal = ["Off", "Unknown", "Lower", "Upper"].map(str::to_string);

    assert_eq!(
        model.choices(&state, "effects_panel.fx1", &legal, "Off"),
        ["Off", "Lower", "Upper"],
    );
    assert_eq!(
        model.choices(&state, "effects_panel.fx1", &legal, "Unknown"),
        ["Off", "Lower", "Upper", "Unknown"],
    );
    // The same variant name on a field no rule narrows is a real choice.
    assert!(model.offers(&state, "some_other_field", "Unknown"));
}

/// A value the library could not name is never something to choose, whatever the field.
#[test]
fn a_value_the_library_could_not_name_is_kept_but_never_offered() {
    let model = model();
    let state = state(&[]);
    let legal = ["B3", "B3Bass", "Pipe", "unknown (6)"].map(str::to_string);

    assert_eq!(
        model.choices(&state, ORGAN_TYPE, &legal, "B3"),
        ["B3", "B3Bass", "Pipe"],
    );
    assert_eq!(
        model.choices(&state, ORGAN_TYPE, &legal, "unknown (6)"),
        ["B3", "B3Bass", "Pipe", "unknown (6)"],
    );
}

// ── couple ──────────────────────────────────────────────────────────────────────

/// Moving the semitones turns the light on, which is what the panel's own knob does.
#[test]
fn an_edit_to_transpose_also_writes_the_enable() {
    let model = model();
    let before = state(&[]);
    assert_eq!(before.value(TRANSPOSE_ENABLED), Some("false"));

    let applied = model.apply(&before, &[(TRANSPOSE.into(), "-5".into())]);
    assert_eq!(
        applied.sets,
        [
            (TRANSPOSE.to_string(), "-5".to_string()),
            (TRANSPOSE_ENABLED.to_string(), "true".to_string()),
        ],
    );
    assert_eq!(applied.state.value(TRANSPOSE_ENABLED), Some("true"));
    assert_eq!(applied.state.value(TRANSPOSE), Some("-5"));
}

/// An edit that names both halves means what it says: the couple does not overwrite it.
#[test]
fn an_edit_naming_both_halves_is_left_alone() {
    let applied = model().apply(
        &state(&[]),
        &[
            (TRANSPOSE.into(), "0".into()),
            (TRANSPOSE_ENABLED.into(), "false".into()),
        ],
    );
    assert_eq!(applied.sets.len(), 2);
    assert_eq!(applied.state.value(TRANSPOSE_ENABLED), Some("false"));
}

// ── check ───────────────────────────────────────────────────────────────────────

fn finding<'a>(findings: &'a [nord_model::Finding], path: &str) -> &'a nord_model::Finding {
    findings
        .iter()
        .find(|finding| finding.path == path)
        .unwrap_or_else(|| panic!("{path} was not reported, got {findings:?}"))
}

/// A fresh program is a state the panel could have produced, so the lint is quiet.
#[test]
fn a_fresh_program_has_nothing_to_report() {
    assert_eq!(model().check(&state(&[])), Vec::new());
}

/// The codec writes states the panel cannot reach — that is the round-trip invariant
/// doing its job — and this is where they get named.
#[test]
fn a_state_the_panel_could_not_produce_is_reported_with_its_provenance() {
    let model = model();

    let both_piano = model.check(&state(&[(LOWER_PART, "Piano"), (UPPER_PART, "Piano")]));
    let reported = finding(&both_piano, UPPER_PART);
    assert_eq!(reported.value, "Piano");
    assert_eq!(reported.provenance, Provenance::ConfirmedOnHardware);

    let older = model.check(&state(&[("effects_panel.fx1", "Unknown")]));
    assert_eq!(finding(&older, "effects_panel.fx1").value, "Unknown");

    let unnamed = model.check(&state(&[(ORGAN_TYPE, "unknown (6)")]));
    assert_eq!(
        finding(&unnamed, ORGAN_TYPE).provenance,
        Provenance::Unexplained
    );
}

/// ⚠️ Where every other model keeps its preset-1 vib, Pipe keeps a bit no field claims
/// and the panel cannot set. It rides through a re-encode verbatim; `check` is the only
/// place it has a name.
#[test]
fn the_pipe_vib_bit_no_field_claims_is_reported_as_unexplained() {
    let model = model();
    let clean = state(&[(ORGAN_TYPE, "Pipe")]);
    assert!(model.check(&clean).is_empty());

    let organ_panel = <ne5::Program as BodyLayout>::layout()
        .iter()
        .find(|field| field.path == "organ_panel")
        .expect("declared")
        .lo;
    let bit = organ_panel + 492;

    let mut body = clean.body().expect("the state carries its body").to_vec();
    body[bit as usize / 8] |= 0x80 >> (bit % 8);

    let findings = model.check(&clean.with_body(body));
    let reported = finding(&findings, "organ_panel.pipe_preset1_vib");
    assert_eq!(reported.provenance, Provenance::Unexplained);
    assert_eq!(reported.value, "0x1");
}

// ── capability ──────────────────────────────────────────────────────────────────

/// The keybed is not in the file, and only the 73-key split-point table has been read
/// off an instrument. An unanswered keybed claims nothing rather than claiming freedom.
#[test]
fn a_keybed_whose_split_points_are_unknown_gets_no_rule() {
    use nord_model::{electro5::electro5_variant, Keybed, Product, Variant};

    let variant = |keys| Variant {
        product: Product::Electro5,
        keys,
    };
    let known = electro5_variant(variant(Keybed::Keys73));
    let unknown = electro5_variant(variant(Keybed::Keys61));
    assert_eq!(known.rules().len(), unknown.rules().len() + 1);

    let state = state(&[]);
    let legal = ["C3", "F3", "Upper"].map(str::to_string);
    assert_eq!(
        unknown.choices(&state, "center_panel.split_point", &legal, "C3"),
        ["C3", "F3", "Upper"],
    );
}
