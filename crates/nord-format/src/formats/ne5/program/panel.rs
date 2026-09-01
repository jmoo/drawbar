//! The Electro 5 program as its panel is divided.
//!
//! The body keeps every organ model's registration and both parts' settings at once, so
//! most of it is state rather than controls: what the instrument is *using* is decided by
//! the part selectors and the organ model, and that is what the conditions here say.
//!
//! Reading order is the panel's, left to right: the keyboard section leads because its
//! part pickers decide which engine sections mean anything, and a picker that brings a
//! section back must never be inside the section it brings back.

use crate::panel::{Group, Match, Panel, Relevance, Selection};

/// Either part playing `instrument` — the condition an engine section hangs on.
macro_rules! part_plays {
    ($instrument:expr) => {
        Some(Relevance {
            any_of: &[
                Match {
                    field: "center_panel.lower_part",
                    is: &[$instrument],
                },
                Match {
                    field: "center_panel.upper_part",
                    is: &[$instrument],
                },
            ],
        })
    };
}

/// The organ model the program has selected.
macro_rules! organ_is {
    ($($model:expr),+) => {
        Some(Relevance {
            any_of: &[Match {
                field: "center_panel.organ_type",
                is: &[$($model),+],
            }],
        })
    };
}

/// One effect routed to a part. `Unknown` is how older firmware spelled *off* and
/// presents as off, so it is not one of these.
/// Confirmed on hardware.
macro_rules! routed {
    ($field:expr) => {
        Some(Relevance {
            any_of: &[Match {
                field: $field,
                is: &["Lower", "Upper"],
            }],
        })
    };
}

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

pub const PANEL: Panel = Panel {
    exhaustive: false,
    selections: &[
        Selection {
            member: "organ_panel.b3_preset1_drawbars",
            field: "organ_panel.b3_preset2_selected",
            value: "false",
        },
        Selection {
            member: "organ_panel.b3_bass_bar1",
            field: "organ_panel.b3_preset2_selected",
            value: "false",
        },
        Selection {
            member: "organ_panel.b3_preset2_drawbars",
            field: "organ_panel.b3_preset2_selected",
            value: "true",
        },
        Selection {
            member: "organ_panel.vox_preset1_drawbars",
            field: "organ_panel.vox_preset2_selected",
            value: "false",
        },
        Selection {
            member: "organ_panel.vox_preset2_drawbars",
            field: "organ_panel.vox_preset2_selected",
            value: "true",
        },
        Selection {
            member: "organ_panel.farfisa_preset1_drawbars",
            field: "organ_panel.farfisa_preset2_selected",
            value: "false",
        },
        Selection {
            member: "organ_panel.farfisa_preset2_drawbars",
            field: "organ_panel.farfisa_preset2_selected",
            value: "true",
        },
        Selection {
            member: "organ_panel.pipe_preset1_drawbars",
            field: "organ_panel.pipe_preset2_selected",
            value: "false",
        },
        Selection {
            member: "organ_panel.pipe_preset2_drawbars",
            field: "organ_panel.pipe_preset2_selected",
            value: "true",
        },
    ],
    groups: &[
        Group {
            title: "Keyboard & split",
            when: None,
            groups: &[],
            members: &[
                "center_panel.lower_part",
                "center_panel.lower_octave_shift",
                "center_panel.lower_sustain",
                "center_panel.lower_control",
                "center_panel.upper_part",
                "center_panel.upper_octave_shift",
                "center_panel.upper_sustain",
                "center_panel.upper_control",
                "center_panel.split",
                "center_panel.split_point",
                "center_panel.part_mix",
                // ⚠️ Two fields, one control: the enable is sticky and an untouched
                // program stores +1 rather than 0, so neither reads on its own. They are
                // adjacent because a caller drawing them as one needs them together.
                "center_panel.transpose_enabled",
                "center_panel.transpose",
                "center_panel.gain",
            ],
        },
        Group {
            title: "Organ",
            when: part_plays!("Organ"),
            // The model selector stays here rather than inside a model's cluster: it is
            // what a reader changes to make another cluster relevant.
            members: &["center_panel.organ_type", "center_panel.drawbar_live"],
            groups: &[
                Group {
                    title: "B3",
                    when: organ_is!("B3", "B3Bass"),
                    members: &[
                        "organ_panel.b3_vib",
                        "organ_panel.b3_perc_third",
                        "organ_panel.b3_perc_speed",
                        "organ_panel.b3_preset2_selected",
                    ],
                    groups: &[
                        Group {
                            title: "Preset 1",
                            // b3+bass replaces this registration with the bass manual
                            // below, and the nine nibbles it would draw hold stale
                            // leftovers there — showing them asserts a registration that
                            // plays nothing.
                            when: organ_is!("B3"),
                            members: &[
                                "organ_panel.b3_preset1_drawbars",
                                "organ_panel.b3_preset1_vib",
                                "organ_panel.b3_preset1_perc",
                            ],
                            groups: &[],
                        },
                        Group {
                            title: "Preset 1, bass manual",
                            // ⚠️ Two live bars, outside the nine-nibble block. The vib
                            // and percussion flags of preset 1 are in the group above,
                            // so they read as not-relevant here; whether the bass manual
                            // answers them is not established.
                            when: organ_is!("B3Bass"),
                            members: &["organ_panel.b3_bass_bar1", "organ_panel.b3_bass_bar2"],
                            groups: &[],
                        },
                        Group {
                            title: "Preset 2",
                            when: None,
                            members: &[
                                "organ_panel.b3_preset2_drawbars",
                                "organ_panel.b3_preset2_vib",
                                "organ_panel.b3_preset2_perc",
                            ],
                            groups: &[],
                        },
                    ],
                },
                Group {
                    title: "Vox",
                    when: organ_is!("Vox"),
                    members: &["organ_panel.vox_vib", "organ_panel.vox_preset2_selected"],
                    groups: &[
                        Group {
                            title: "Preset 1",
                            when: None,
                            members: &[
                                "organ_panel.vox_preset1_drawbars",
                                "organ_panel.vox_preset1_vib",
                            ],
                            groups: &[],
                        },
                        Group {
                            title: "Preset 2",
                            when: None,
                            members: &[
                                "organ_panel.vox_preset2_drawbars",
                                "organ_panel.vox_preset2_vib",
                            ],
                            groups: &[],
                        },
                    ],
                },
                Group {
                    title: "Farfisa",
                    // The registers are stored as drawbar positions and read by the
                    // instrument as on/off tabs, at a threshold of 5.
                    when: organ_is!("Farfisa"),
                    members: &[
                        "organ_panel.farfisa_vib",
                        "organ_panel.farfisa_preset2_selected",
                    ],
                    groups: &[
                        Group {
                            title: "Preset 1",
                            when: None,
                            members: &[
                                "organ_panel.farfisa_preset1_drawbars",
                                "organ_panel.farfisa_preset1_vib",
                            ],
                            groups: &[],
                        },
                        Group {
                            title: "Preset 2",
                            when: None,
                            members: &[
                                "organ_panel.farfisa_preset2_drawbars",
                                "organ_panel.farfisa_preset2_vib",
                            ],
                            groups: &[],
                        },
                    ],
                },
                Group {
                    title: "Pipe",
                    // No vibrato and no percussion the panel can reach: the bit the other
                    // models use for preset-1 vib is set in nearly every real program,
                    // and the vib button does not respond while pipe is selected.
                    // Confirmed on hardware.
                    when: organ_is!("Pipe"),
                    members: &["organ_panel.pipe_preset2_selected"],
                    groups: &[
                        Group {
                            title: "Preset 1",
                            when: None,
                            members: &["organ_panel.pipe_preset1_drawbars"],
                            groups: &[],
                        },
                        Group {
                            title: "Preset 2",
                            when: None,
                            members: &["organ_panel.pipe_preset2_drawbars"],
                            groups: &[],
                        },
                    ],
                },
            ],
        },
        Group {
            title: "Piano",
            when: part_plays!("Piano"),
            members: &[
                "piano_panel.category",
                "piano_panel.piano_model",
                "piano_panel.acoustics",
                "piano_panel.touch",
                "piano_panel.mono",
            ],
            groups: &[Group {
                title: "Clavinet",
                // Inferred from specimens; not confirmed on hardware.
                when: Some(Relevance {
                    any_of: &[Match {
                        field: "piano_panel.category",
                        is: &["Clavinet"],
                    }],
                }),
                members: &["piano_panel.clav_model"],
                groups: &[],
            }],
        },
        Group {
            title: "Sample",
            when: part_plays!("Sample"),
            members: &[
                "sample_panel.number",
                "sample_panel.attack",
                "sample_panel.decay_release",
                "sample_panel.dynamics",
                "sample_panel.filter",
            ],
            groups: &[],
        },
        Group {
            title: "Effects",
            when: None,
            // The five routing switches, which are what turns each effect on. They stay
            // out of the clusters they govern for the same reason the organ model does.
            members: &[
                "effects_panel.fx1",
                "effects_panel.fx2",
                "effects_panel.fx3",
                "effects_panel.fx4",
                "effects_panel.fx5",
            ],
            groups: &[
                Group {
                    title: "Effect 1",
                    when: routed!("effects_panel.fx1"),
                    members: &[
                        "effects_panel.fx1_type",
                        "effects_panel.fx1_rate",
                        "effects_panel.fx1_control",
                    ],
                    groups: &[],
                },
                Group {
                    title: "Effect 2",
                    when: routed!("effects_panel.fx2"),
                    members: &[
                        "effects_panel.fx2_type",
                        "effects_panel.fx2_rate",
                        "effects_panel.fx2_deep",
                    ],
                    groups: &[],
                },
                Group {
                    title: "Amp / compressor",
                    when: routed!("effects_panel.fx3"),
                    members: &["effects_panel.fx3_type", "effects_panel.fx3_compression"],
                    groups: &[Group {
                        title: "Rotary speaker",
                        // Nested, so it needs both: the amp block routed to a part *and*
                        // the rotary chosen as its model.
                        when: Some(Relevance {
                            any_of: &[Match {
                                field: "effects_panel.fx3_type",
                                is: &["Rotary"],
                            }],
                        }),
                        members: &["effects_panel.rotary_speed", "effects_panel.rotary_stop"],
                        groups: &[],
                    }],
                },
                Group {
                    title: "Delay",
                    when: routed!("effects_panel.fx4"),
                    members: &[
                        "effects_panel.fx4_tempo",
                        "effects_panel.fx4_feedback",
                        "effects_panel.fx4_moisture",
                        "effects_panel.fx4_ping_pong",
                    ],
                    groups: &[],
                },
                Group {
                    title: "Reverb",
                    when: switched_on!("effects_panel.fx5"),
                    members: &["effects_panel.fx5_type", "effects_panel.fx5_moisture"],
                    groups: &[],
                },
            ],
        },
        Group {
            title: "EQ",
            when: None,
            members: &["effects_panel.equalizer_on"],
            groups: &[Group {
                title: "Bands",
                when: switched_on!("effects_panel.equalizer_on"),
                members: &[
                    "effects_panel.equalizer_part",
                    "effects_panel.equalizer_bass",
                    "effects_panel.equalizer_freq",
                    "effects_panel.equalizer_freq_gain",
                    "effects_panel.equalizer_treble",
                ],
                groups: &[],
            }],
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::Field;
    use crate::formats::ne5::Program;

    /// A program with `sets` applied, as the registry reads it back.
    fn program(sets: &[(&str, &str)]) -> Vec<Field> {
        let mut body = Program::default();
        for (path, value) in sets {
            body.set_field(path, value).expect(path);
        }
        body.fields()
    }

    fn group(title: &str) -> &'static Group {
        PANEL
            .walk()
            .into_iter()
            .find(|group| group.title == title)
            .unwrap_or_else(|| panic!("no group {title}"))
    }

    fn relevant(title: &str, fields: &[Field]) -> bool {
        group(title).is_relevant(fields)
    }

    /// An engine section is relevant while a part is playing it, and the pickers that
    /// bring one back are in a section that always is.
    #[test]
    fn a_section_follows_the_parts() {
        let fresh = program(&[]);
        assert!(relevant("Organ", &fresh), "a fresh program plays organ");
        assert!(!relevant("Piano", &fresh));
        assert!(!relevant("Sample", &fresh));
        for always in ["Keyboard & split", "Effects", "EQ"] {
            assert!(relevant(always, &fresh), "{always}");
        }

        let split = program(&[("center_panel.upper_part", "Piano")]);
        assert!(relevant("Piano", &split));
        assert!(relevant("Organ", &split), "the lower part still plays it");

        let neither = program(&[
            ("center_panel.lower_part", "Sample"),
            ("center_panel.upper_part", "Piano"),
        ]);
        assert!(!relevant("Organ", &neither));
        // The part pickers are in the section that never goes away.
        let keyboard = group("Keyboard & split").members;
        assert!(keyboard.contains(&"center_panel.lower_part"));
        assert!(keyboard.contains(&"center_panel.upper_part"));
    }

    /// One model's registration means anything at a time, and the model selector is not
    /// inside the cluster it selects.
    #[test]
    fn only_the_selected_organ_is_relevant() {
        let b3 = program(&[("center_panel.organ_type", "B3")]);
        assert!(relevant("B3", &b3));
        for other in ["Vox", "Farfisa", "Pipe"] {
            assert!(!relevant(other, &b3), "{other}");
        }
        assert!(group("Organ").members.contains(&"center_panel.organ_type"));

        let vox = program(&[("center_panel.organ_type", "Vox")]);
        assert!(relevant("Vox", &vox));
        assert!(!relevant("B3", &vox));

        // A selection the library cannot name matches nothing, so no model speaks for
        // the state and every registration reads as what it is.
        let unknown = program(&[("center_panel.organ_type", "unknown (6)")]);
        for model in ["B3", "Vox", "Farfisa", "Pipe"] {
            assert!(!relevant(model, &unknown), "{model}");
        }
    }

    /// b3+bass: preset 1 is the bass manual's two bars, and the nine nibbles they shadow
    /// hold stale leftovers, so that registration is not the one to draw.
    #[test]
    fn b3_bass_replaces_the_first_registration() {
        let b3 = program(&[("center_panel.organ_type", "B3")]);
        assert!(relevant("Preset 1", &b3));
        assert!(!relevant("Preset 1, bass manual", &b3));

        let bass = program(&[("center_panel.organ_type", "B3Bass")]);
        assert!(!relevant("Preset 1", &bass));
        assert!(relevant("Preset 1, bass manual", &bass));
        // Preset 2 is an ordinary B3 either way.
        assert!(relevant("Preset 2", &bass));
        assert!(
            relevant("B3", &bass),
            "the shared registration is still B3's"
        );
    }

    /// The rotary knobs need both the amp block routed to a part and the rotary chosen
    /// as its model — a conjunction, which is what nesting is for.
    #[test]
    fn a_nested_group_needs_its_parent_too() {
        let rotary = program(&[
            ("effects_panel.fx3", "Upper"),
            ("effects_panel.fx3_type", "Rotary"),
        ]);
        assert!(relevant("Amp / compressor", &rotary));
        assert!(relevant("Rotary speaker", &rotary));

        // The model alone: the nested group's own condition still holds, and the parent's
        // does not — which is the caller's cue to stop descending.
        let unrouted = program(&[("effects_panel.fx3_type", "Rotary")]);
        assert!(!relevant("Amp / compressor", &unrouted));
        assert!(relevant("Rotary speaker", &unrouted));

        let amp = program(&[
            ("effects_panel.fx3", "Upper"),
            ("effects_panel.fx3_type", "Twin"),
        ]);
        assert!(relevant("Amp / compressor", &amp));
        assert!(!relevant("Rotary speaker", &amp));
    }

    /// ⚠️ `Unknown` is how older firmware spelled *off* and presents as off, so it does
    /// not make an effect relevant.
    #[test]
    fn the_older_spelling_of_off_reads_as_off() {
        let off = program(&[("effects_panel.fx1", "Unknown")]);
        assert!(!relevant("Effect 1", &off));
        let on = program(&[("effects_panel.fx1", "Lower")]);
        assert!(relevant("Effect 1", &on));
    }
}
