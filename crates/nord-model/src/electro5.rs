//! The Nord Electro 5's panel, as rules.
//!
//! Every rule carries how it is known. A rule with no provenance is not a rule, and a
//! guess dressed as an observation costs hours downstream.

use nord_format::formats::ne5;
use nord_format::layout::BodyLayout;

use crate::rules::{Cond, PathPattern, Rule, Vestige};
use crate::{DeviceModel, Keybed, Product, Provenance::*, Variant};

pub const LOWER_PART: &str = "center_panel.lower_part";
pub const UPPER_PART: &str = "center_panel.upper_part";
pub const ORGAN_TYPE: &str = "center_panel.organ_type";
pub const SPLIT_POINT: &str = "center_panel.split_point";
pub const TRANSPOSE: &str = "center_panel.transpose";
pub const TRANSPOSE_ENABLED: &str = "center_panel.transpose_enabled";
pub const EQUALIZER_ON: &str = "effects_panel.equalizer_on";
pub const EQUALIZER_PART: &str = "effects_panel.equalizer_part";

/// The organ section, as a path prefix.
pub const ORGAN_SECTION: &str = "organ_panel.";

/// The four effect slots that route to a part.
const FX_SLOTS: [&str; 4] = [
    "effects_panel.fx1",
    "effects_panel.fx2",
    "effects_panel.fx3",
    "effects_panel.fx4",
];

/// The Electro 5 as a 73-key instrument.
///
/// ⚠️ The keybed is the one axis a program file does not record, and the split-point
/// table is the one rule that turns on it — see [`Keybed::split_points`]. A consumer
/// reading a file alone cannot know better than this default.
pub fn electro5() -> DeviceModel {
    electro5_variant(Variant {
        product: Product::Electro5,
        keys: Keybed::Keys73,
    })
}

pub fn electro5_variant(variant: Variant) -> DeviceModel {
    let organ_panel = panel_lo("organ_panel");
    let mut model = DeviceModel::new(variant)
        // The organ section answers to nothing while no part is playing it. Confirmed on
        // hardware.
        .rule(Rule::Gate {
            when: Cond::Any(vec![
                Cond::Is(LOWER_PART.into(), "Organ".into()),
                Cond::Is(UPPER_PART.into(), "Organ".into()),
            ]),
            controls: vec![PathPattern::new("organ_panel.*")],
            provenance: ConfirmedOnHardware,
        })
        // b3+bass is a selection, not a fifth model: it reads the B3's storage.
        // Confirmed on hardware.
        .rule(model_gate("organ_panel.b3_*", &["B3", "B3Bass"]))
        .rule(model_gate("organ_panel.vox_*", &["Vox"]))
        .rule(model_gate("organ_panel.farfisa_*", &["Farfisa"]))
        .rule(model_gate("organ_panel.pipe_*", &["Pipe"]))
        // ⚠️ In b3+bass, preset 1 is the bass manual: two live drawbars outside the
        // nine-nibble block, which holds stale leftovers. The block and the pair are
        // never live together. Confirmed on hardware.
        .rule(Rule::Gate {
            when: Cond::Is(ORGAN_TYPE.into(), "B3Bass".into()),
            controls: vec![PathPattern::new("organ_panel.b3_bass_*")],
            provenance: ConfirmedOnHardware,
        })
        .rule(Rule::Gate {
            when: Cond::Not(Box::new(Cond::Is(ORGAN_TYPE.into(), "B3Bass".into()))),
            controls: vec![PathPattern::new("organ_panel.b3_preset1_drawbars")],
            provenance: ConfirmedOnHardware,
        })
        // One piano engine, so the two parts cannot both select it. Confirmed on
        // hardware.
        .rule(part_excludes(UPPER_PART, LOWER_PART))
        .rule(part_excludes(LOWER_PART, UPPER_PART))
        // Sticky: the instrument sets the enable the first time transposition is touched
        // and never clears it. The transpose light is on for `transpose_enabled &&
        // transpose != 0`, so neither field answers on its own. Confirmed on hardware.
        .rule(Rule::Couple {
            edit: TRANSPOSE.into(),
            also: TRANSPOSE_ENABLED.into(),
            value: "true".into(),
            provenance: ConfirmedOnHardware,
        })
        // ⚠️ `equalizer_part` stores 0 for *lower*, not off, so the enable is the only
        // thing that says whether the EQ is doing anything. Inferred from specimens; not
        // confirmed on hardware.
        .rule(Rule::Gate {
            when: Cond::Is(EQUALIZER_ON.into(), "true".into()),
            controls: vec![PathPattern::new(EQUALIZER_PART)],
            provenance: InferredFromSpecimens,
        })
        // ⚠️ Bit 492 of the organ panel is where every other model keeps its preset-1
        // vib, and it is set in nearly every real program — but the vib button does not
        // respond while Pipe is selected. Unexplained: real programs hold this, and the
        // panel cannot produce it.
        .vestige(Vestige {
            name: "organ_panel.pipe_preset1_vib",
            bits: (organ_panel + 492, organ_panel + 492),
            panel_writes: 0,
            provenance: Unexplained,
        });

    for slot in FX_SLOTS {
        // ⚠️ `Unknown` is a second spelling of off, from older firmware: it presents as
        // off and the current panel writes 0. Two entries both meaning off is a puzzle,
        // not a choice. Confirmed on hardware.
        model = model.rule(Rule::Narrow {
            path: slot.into(),
            when: Cond::always(),
            to: spellings(&["Off", "Lower", "Upper"]),
            provenance: ConfirmedOnHardware,
        });
    }

    match model.variant.keys.split_points() {
        // Inferred from specimens; not confirmed on hardware.
        Some(points) => model.rule(Rule::Narrow {
            path: SPLIT_POINT.into(),
            when: Cond::always(),
            to: spellings(points),
            provenance: InferredFromSpecimens,
        }),
        // Unanswered rather than unrestricted: a keybed whose table nobody has read gets
        // no rule, so nothing is claimed about it either way.
        None => model,
    }
}

/// The organ models whose storage a selection reads.
fn model_gate(controls: &str, types: &[&str]) -> Rule {
    Rule::Gate {
        when: Cond::In(ORGAN_TYPE.into(), types.iter().map(|&t| t.into()).collect()),
        controls: vec![PathPattern::new(controls)],
        provenance: ConfirmedOnHardware,
    }
}

/// `part` cannot select the piano while the other part has it.
fn part_excludes(part: &str, other: &str) -> Rule {
    Rule::Narrow {
        path: part.into(),
        when: Cond::Is(other.into(), "Piano".into()),
        to: spellings(&["Organ", "Sample"]),
        provenance: ConfirmedOnHardware,
    }
}

fn spellings(values: &[&str]) -> Vec<crate::rules::Value> {
    values.iter().map(|&value| value.to_string()).collect()
}

/// Where `panel` starts in the program body, from the layout the codec publishes — so a
/// vestige is placed by the same map, and a panel that moves takes its vestiges with it.
fn panel_lo(panel: &str) -> u32 {
    <ne5::Program as BodyLayout>::layout()
        .iter()
        .find(|field| field.path == panel)
        .expect("the Electro 5 program body declares this panel")
        .lo
}
