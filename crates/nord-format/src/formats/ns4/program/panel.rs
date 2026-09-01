//! The Stage 4 program as its panel is divided.
//!
//! Three sections, each with its layers, each layer with its own effects chain — the
//! shape the body's nested bodies already have, plus the two things they do not say:
//! which layer's fields are the ones the instrument is playing, and where the loose
//! fields at the top level belong.
//!
//! A layer's enable and volume are **not** in its nested body — the file packs those with
//! the other layers' — so each layer's group names them beside the body's own fields.
//! Morph slots are named by nothing: most of this body's fields are morph targets, and
//! each belongs to the parameter its name binds it to.

use crate::panel::{Group, Match, Panel, Relevance};

macro_rules! switched_on {
    ($field:expr) => {
        Some(Relevance {
            any_of: &[Match {
                field: $field,
                is: &["true"],
            }],
        })
    };
}

/// One keyboard-zone boundary: its note and its crossfade, under its own enable.
macro_rules! split_point {
    ($title:expr, $zones:expr) => {
        Group {
            title: $title,
            when: switched_on!(concat!("kb_zones_", $zones, "_split_point_enabled")),
            members: &[
                concat!("kb_zones_", $zones, "_split_point"),
                concat!("kb_zones_", $zones, "_split_point_xfade"),
            ],
            groups: &[],
        }
    };
}

/// One layer of a section: its enable lives with its siblings, so the group carries the
/// volume, the layer body and the effects chain that follows it.
macro_rules! layer {
    ($title:expr, $enable:expr, $members:expr, $fx:expr) => {
        Group {
            title: $title,
            when: switched_on!($enable),
            members: $members,
            groups: &[Group {
                title: "Effects",
                when: None,
                members: &[$fx],
                groups: &[],
            }],
        }
    };
}

pub const PANEL: Panel = Panel {
    // The three effects chains and the layer bodies are named whole, so everything the
    // body registers has a place — see the tests in `crate::panel`.
    exhaustive: true,
    groups: &[
        Group {
            // The switches that decide which of the sections below mean anything. They
            // lead, and they stay out of the groups they govern — a switch inside the
            // section it turns off is a switch nobody can turn back on.
            title: "Sections",
            when: None,
            members: &[
                "organ_section_enabled",
                "piano_section_enabled",
                "synth_section_enabled",
                "fx_enabled",
            ],
            groups: &[],
        },
        Group {
            title: "Keyboard & split",
            when: None,
            members: &[
                "split_enabled",
                "program_transpose_enabled",
                "program_transpose_amount",
            ],
            groups: &[Group {
                title: "Split points",
                when: switched_on!("split_enabled"),
                // Each boundary's own enable, outside the group it governs.
                members: &[
                    "kb_zones_1_2_split_point_enabled",
                    "kb_zones_2_3_split_point_enabled",
                    "kb_zones_3_4_split_point_enabled",
                ],
                groups: &[
                    split_point!("Zones 1–2", "1_2"),
                    split_point!("Zones 2–3", "2_3"),
                    split_point!("Zones 3–4", "3_4"),
                ],
            }],
        },
        Group {
            title: "Organ",
            when: switched_on!("organ_section_enabled"),
            members: &[
                "organ_a_layer_enabled",
                "organ_b_layer_enabled",
                "organ_pitch_stick_enabled",
                "organ_vib_chorus_type",
                "organ_rotary_speaker_enabled",
            ],
            groups: &[
                Group {
                    title: "Layer A",
                    when: switched_on!("organ_a_layer_enabled"),
                    members: &["organ_a_volume", "organ_a.*"],
                    groups: &[],
                },
                Group {
                    title: "Layer B",
                    when: switched_on!("organ_b_layer_enabled"),
                    members: &["organ_b_volume", "organ_b.*"],
                    groups: &[],
                },
                Group {
                    title: "Rotary speaker",
                    when: switched_on!("organ_rotary_speaker_enabled"),
                    members: &[
                        "rotary_speaker_drive",
                        "rotary_speaker_slow_fast",
                        "rotary_speaker_stop_enabled",
                        "rotary_speaker_stop_position",
                    ],
                    groups: &[],
                },
                Group {
                    // Both organ layers play through one chain, so it belongs to the
                    // section rather than to either layer.
                    title: "Effects",
                    when: None,
                    members: &["organ_fx.*"],
                    groups: &[],
                },
            ],
        },
        Group {
            title: "Piano",
            when: switched_on!("piano_section_enabled"),
            members: &["piano_a_layer_enabled", "piano_b_layer_enabled"],
            groups: &[
                layer!(
                    "Layer A",
                    "piano_a_layer_enabled",
                    &["piano_a_volume", "piano_a.*"],
                    "piano_a_fx.*"
                ),
                layer!(
                    "Layer B",
                    "piano_b_layer_enabled",
                    &["piano_b_volume", "piano_b.*"],
                    "piano_b_fx.*"
                ),
            ],
        },
        Group {
            title: "Synth",
            when: switched_on!("synth_section_enabled"),
            members: &[
                "synth_a_layer_enabled",
                "synth_b_layer_enabled",
                "synth_c_layer_enabled",
                "synth_arp_group_enabled",
                "synth_kb_hold_enabled",
            ],
            groups: &[
                layer!(
                    "Layer A",
                    "synth_a_layer_enabled",
                    &[
                        "synth_a_volume",
                        "synth_a_pan",
                        "synth_a_performance.*",
                        "synth_a_voice.*"
                    ],
                    "synth_a_fx.*"
                ),
                layer!(
                    "Layer B",
                    "synth_b_layer_enabled",
                    &[
                        "synth_b_volume",
                        "synth_b_pan",
                        "synth_b_performance.*",
                        "synth_b_voice.*"
                    ],
                    "synth_b_fx.*"
                ),
                layer!(
                    "Layer C",
                    "synth_c_layer_enabled",
                    &[
                        "synth_c_volume",
                        "synth_c_pan",
                        "synth_c_performance.*",
                        "synth_c_voice.*"
                    ],
                    "synth_c_fx.*"
                ),
            ],
        },
        Group {
            title: "Effects, globally",
            when: switched_on!("fx_enabled"),
            members: &[
                "fx_comp_global_enabled",
                "fx_delay_global_enabled",
                "fx_reverb_global_enabled",
            ],
            groups: &[],
        },
        Group {
            // The program stores a second set of section and layer enables, and a flag
            // that reads as which set is live. ⚠️ Which value of `active_layer_scene`
            // means scene 2 is not established, so nothing here is conditional on it —
            // asserting the wrong way round would hide the half that is playing.
            title: "Scene 2",
            when: None,
            members: &[
                "active_layer_scene",
                "organ_section_enabled_scene_2",
                "piano_section_enabled_scene_2",
                "synth_section_enabled_scene_2",
                "organ_a_layer_enabled_scene_2",
                "organ_b_layer_enabled_scene_2",
                "piano_a_layer_enabled_scene_2",
                "piano_b_layer_enabled_scene_2",
                "synth_a_layer_enabled_scene_2",
                "synth_b_layer_enabled_scene_2",
                "synth_c_layer_enabled_scene_2",
            ],
            groups: &[],
        },
        Group {
            title: "Not on the panel",
            when: None,
            // The header's schema version, echoed into the body. A read carries it; no
            // control moves it.
            members: &["version_echo"],
            groups: &[],
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{ControlKind, Field};
    use crate::formats::ns4::program::{Program, BODY_LEN};
    use crate::panel::Section;

    fn program(sets: &[(&str, &str)]) -> Vec<Field> {
        let mut body = Program::try_from([0u8; BODY_LEN]).expect("every field decodes totally");
        for (path, value) in sets {
            body.set_field(path, value).expect(path);
        }
        body.fields()
    }

    /// ⚠️ The first group with this title, in layout order — three sections have a
    /// "Layer A" and four have an "Effects", and the organ's come first.
    fn group(title: &str) -> &'static Group {
        PANEL
            .walk()
            .into_iter()
            .find(|group| group.title == title)
            .unwrap_or_else(|| panic!("no group {title}"))
    }

    /// A section is relevant while it is switched on, and its layers while they are —
    /// the nesting is the conjunction.
    #[test]
    fn a_layer_needs_its_section_and_its_own_enable() {
        let off = program(&[]);
        assert!(!group("Organ").is_relevant(&off));

        let a = program(&[
            ("organ_section_enabled", "true"),
            ("organ_a_layer_enabled", "true"),
        ]);
        assert!(group("Organ").is_relevant(&a));
        assert!(group("Layer A").is_relevant(&a));
        assert!(!group("Layer B").is_relevant(&a));

        // The switches that bring a section and a layer back are never inside what they
        // govern.
        assert!(group("Sections").members.contains(&"organ_section_enabled"));
        assert!(group("Organ").members.contains(&"organ_a_layer_enabled"));
    }

    /// The nine bars of a layer are consecutive and in register order — leftmost first —
    /// which the registry alone does not give: each bar is followed by its three morph
    /// slots there.
    #[test]
    fn an_organ_layers_drawbars_read_in_order() {
        let specs = Program::field_specs();
        let members = group("Layer A").members_of(&specs);
        let bars: Vec<&str> = members
            .iter()
            .copied()
            .filter(|path| {
                specs
                    .iter()
                    .find(|spec| spec.name == *path)
                    .is_some_and(|spec| matches!(spec.control, ControlKind::Drawbar { .. }))
            })
            .collect();
        assert_eq!(
            bars,
            (1..=9)
                .map(|n| format!("organ_a.drawbar_{n}"))
                .collect::<Vec<_>>(),
        );
        // ...and they are one run, not nine scattered through the layer.
        let first = members.iter().position(|p| *p == bars[0]).unwrap();
        assert_eq!(&members[first..first + 9], &bars[..]);
    }

    /// A morph slot is named by no group and is nobody's leftover: it is drawn on the
    /// parameter its name binds it to, and that parameter is grouped.
    ///
    /// ⚠️ The exception is a slot whose parameter this body does not declare — the three
    /// filter-resonance runs. Those have nothing to ride on, so they are named like any
    /// other field.
    #[test]
    fn morph_slots_ride_on_the_parameters_they_move() {
        let specs = Program::field_specs();
        let named = PANEL.named(&specs);
        assert!(!named.contains(&"organ_a.drawbar_1_wheel"));
        assert!(named.contains(&"organ_a.drawbar_1"));
        assert!(named.contains(&"synth_a_voice.filter_resonance_wheel"));
        assert!(PANEL.leftovers(&specs).is_empty());
    }

    /// The render path and the inspection path have to agree: one resolves against a
    /// body's values and the other against its specs, and a layout means one thing.
    #[test]
    fn resolving_a_body_names_what_the_specs_say_it_will() {
        let fields = program(&[]);
        let specs = Program::field_specs();
        let resolved = PANEL.resolve(&fields);

        fn named<'a>(sections: &[Section<'a>]) -> Vec<&'a str> {
            sections
                .iter()
                .flat_map(|section| {
                    section
                        .fields
                        .iter()
                        .map(|field| field.path.as_str())
                        .chain(named(&section.groups))
                })
                .collect()
        }

        assert_eq!(named(&resolved.sections), PANEL.named(&specs));
        assert!(resolved.leftovers.is_empty());
    }

    /// A nested group's resolved relevance carries its parent's, so a caller drawing the
    /// tree does not have to walk back up.
    #[test]
    fn a_resolved_section_carries_its_parents_relevance() {
        let layer_only = program(&[("organ_a_layer_enabled", "true")]);
        let organ = PANEL
            .resolve(&layer_only)
            .sections
            .into_iter()
            .find(|section| section.group.title == "Organ")
            .expect("the organ section");
        assert!(!organ.relevant, "the section is switched off");

        let layer = organ
            .groups
            .iter()
            .find(|section| section.group.title == "Layer A")
            .expect("layer A");
        assert!(!layer.relevant, "and nothing inside it is being played");
        // Its own condition still holds, which is what tells "the section is off" from
        // "this layer is off".
        assert!(layer.group.is_relevant(&layer_only));
    }
}
