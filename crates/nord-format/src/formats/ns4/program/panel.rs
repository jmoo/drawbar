//! Stage 4 program controls and the enables that make each one relevant.
//!
//! Relevance follows the Nord Stage 4 User Manual, OS v1.6x, Edition N
//! (<https://www.nordkeyboards.com/wt/documents/951/Nord%20Stage%204%20User%20Manual%20v1.6X-Edition-N.pdf>),
//! by printed page: sections and layers 43; transpose and split 38–39; Extern 46–47;
//! mono and legato 34–35; filter 31–33; arpeggiator and pattern 35–37; keyboard hold 36;
//! effects 48; rotary 48 and 52–53. The manual describes operation, not storage: where a
//! selector's encoding is not established, the layout keeps its dependents relevant
//! rather than guess, and the group concerned says so.
//!
//! Morph slots are named by nothing: each belongs to the parameter its name binds it to.

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

macro_rules! enabled_in_either_scene {
    ($field:expr) => {
        Some(Relevance {
            any_of: &[
                Match {
                    field: $field,
                    is: &["true"],
                },
                Match {
                    field: concat!($field, "_scene_2"),
                    is: &["true"],
                },
            ],
        })
    };
}

macro_rules! controls {
    ($title:expr, $when:expr, $prefix:expr, [$($field:literal),* $(,)?]) => {
        Group {
            title: $title,
            selected_by: None,
            when: $when,
            members: &[$(concat!($prefix, $field)),*],
            groups: &[],
        }
    };
}

macro_rules! split_point {
    ($title:expr, $zones:expr) => {
        controls!(
            $title,
            switched_on!(concat!("kb_zones_", $zones, "_split_point_enabled")),
            concat!("kb_zones_", $zones),
            ["_split_point", "_split_point_xfade"]
        )
    };
}

macro_rules! effects {
    ($prefix:expr) => {
        Group {
            title: "Effects",
            selected_by: None,
            when: switched_on!("fx_enabled"),
            members: &[
                concat!($prefix, "mod_1_enabled"),
                concat!($prefix, "mod_2_enabled"),
                concat!($prefix, "amp_sim_eq_enabled"),
                concat!($prefix, "comp_enabled"),
                concat!($prefix, "delay_enabled"),
                concat!($prefix, "reverb_enabled"),
            ],
            groups: &[
                controls!(
                    "Mod 1",
                    switched_on!(concat!($prefix, "mod_1_enabled")),
                    $prefix,
                    [
                        "mod_1_mode",
                        "mod_1_master_clock_enabled",
                        "mod_1_rate",
                        "mod_1_amount"
                    ]
                ),
                controls!(
                    "Mod 2",
                    switched_on!(concat!($prefix, "mod_2_enabled")),
                    $prefix,
                    ["mod_2_mode", "mod_2_rate", "mod_2_amount"]
                ),
                controls!(
                    "Amp / EQ",
                    switched_on!(concat!($prefix, "amp_sim_eq_enabled")),
                    $prefix,
                    [
                        "amp_sim_eq_mode",
                        "amp_sim_eq_treb",
                        "amp_sim_eq_mid",
                        "amp_sim_eq_bass",
                        "amp_sim_eq_freq",
                        "amp_sim_eq_drive"
                    ]
                ),
                controls!(
                    "Compressor",
                    switched_on!(concat!($prefix, "comp_enabled")),
                    $prefix,
                    ["comp_amount", "comp_response"]
                ),
                controls!(
                    "Delay",
                    switched_on!(concat!($prefix, "delay_enabled")),
                    $prefix,
                    [
                        "delay_tempo_master_clock_enabled",
                        "delay_tempo",
                        "delay_mix",
                        "delay_normal_analog",
                        "delay_ping_pong_enabled",
                        "delay_filter_type",
                        "delay_feedback",
                        "delay_effects"
                    ]
                ),
                controls!(
                    "Reverb",
                    switched_on!(concat!($prefix, "reverb_enabled")),
                    $prefix,
                    ["reverb_amount", "reverb_dark_bright", "reverb_type"]
                ),
            ],
        }
    };
}

macro_rules! organ_layer {
    ($title:expr, $layer:literal) => {
        Group {
            title: $title,
            selected_by: None,
            when: enabled_in_either_scene!(concat!("organ_", $layer, "_layer_enabled")),
            members: &[
                concat!("organ_", $layer, "_volume"),
                concat!("organ_", $layer, ".octave_shift"),
                concat!("organ_", $layer, ".sustain_pedal_enabled"),
                concat!("organ_", $layer, ".model"),
                concat!("organ_", $layer, ".preset_enabled"),
                concat!("organ_", $layer, ".drawbar_1"),
                concat!("organ_", $layer, ".drawbar_2"),
                concat!("organ_", $layer, ".drawbar_3"),
                concat!("organ_", $layer, ".drawbar_4"),
                concat!("organ_", $layer, ".drawbar_5"),
                concat!("organ_", $layer, ".drawbar_6"),
                concat!("organ_", $layer, ".drawbar_7"),
                concat!("organ_", $layer, ".drawbar_8"),
                concat!("organ_", $layer, ".drawbar_9"),
                concat!("organ_", $layer, ".vib_chorus_enabled"),
                concat!("organ_", $layer, ".percussion_enabled"),
            ],
            groups: &[
                controls!(
                    "Keyboard zones",
                    switched_on!("split_enabled"),
                    concat!("organ_", $layer, "."),
                    ["kb_zones"]
                ),
                controls!(
                    "Percussion",
                    switched_on!(concat!("organ_", $layer, ".percussion_enabled")),
                    concat!("organ_", $layer, "."),
                    [
                        "percussion_harmonic_3rd_enabled",
                        "percussion_decay_fast_enabled",
                        "percussion_volume_soft_enabled"
                    ]
                ),
            ],
        }
    };
}

macro_rules! piano_layer {
    ($title:expr, $layer:literal) => {
        Group {
            title: $title,
            selected_by: None,
            when: enabled_in_either_scene!(concat!("piano_", $layer, "_layer_enabled")),
            members: &[
                concat!("piano_", $layer, "_volume"),
                concat!("piano_", $layer, ".octave_shift"),
                concat!("piano_", $layer, ".pitch_stick_enabled"),
                concat!("piano_", $layer, ".sustain_pedal_enabled"),
                concat!("piano_", $layer, ".piano_type"),
                concat!("piano_", $layer, ".model_slot"),
                concat!("piano_", $layer, ".model_variation"),
                concat!("piano_", $layer, ".model_id"),
                concat!("piano_", $layer, ".soft_rel_enabled"),
                concat!("piano_", $layer, ".string_res_enabled"),
                concat!("piano_", $layer, ".pedal_noise_enabled"),
                concat!("piano_", $layer, ".touch"),
                concat!("piano_", $layer, ".unison_level"),
                concat!("piano_", $layer, ".dyn_comp"),
                concat!("piano_", $layer, ".timbre"),
            ],
            groups: &[
                controls!(
                    "Keyboard zones",
                    switched_on!("split_enabled"),
                    concat!("piano_", $layer, "."),
                    ["kb_zones"]
                ),
                effects!(concat!("piano_", $layer, "_fx.")),
            ],
        }
    };
}

macro_rules! synth_layer {
    ($title:expr, $layer:literal) => {
        Group {
            title: $title,
            selected_by: None,
            when: enabled_in_either_scene!(concat!("synth_", $layer, "_layer_enabled")),
            members: &[
                concat!("synth_", $layer, "_volume"),
                concat!("synth_", $layer, "_performance.extern_enabled"),
                concat!("synth_", $layer, "_performance.octave_shift"),
                concat!("synth_", $layer, "_performance.pitch_stick_enabled"),
                concat!("synth_", $layer, "_performance.sustain_pedal_enabled"),
            ],
            groups: &[
                controls!(
                    "Keyboard zones",
                    switched_on!("split_enabled"),
                    concat!("synth_", $layer, "_performance."),
                    ["kb_zones"]
                ),
                controls!(
                    "Extern",
                    switched_on!(concat!("synth_", $layer, "_performance.extern_enabled")),
                    concat!("synth_", $layer, "_performance."),
                    ["extern_program", "extern_cc_val1", "extern_cc_val2"]
                ),
                Group {
                    title: "Internal sound",
                    selected_by: None,
                    when: Some(Relevance {
                        any_of: &[Match {
                            field: concat!("synth_", $layer, "_performance.extern_enabled"),
                            is: &["false"],
                        }],
                    }),
                    members: &[
                        concat!("synth_", $layer, "_pan"),
                        concat!("synth_", $layer, "_performance.samples_analog"),
                        concat!("synth_", $layer, "_performance.sample_slot"),
                        concat!("synth_", $layer, "_performance.sample_id"),
                        concat!("synth_", $layer, "_performance.mono_enabled"),
                        concat!("synth_", $layer, "_performance.legato_enabled"),
                        concat!("synth_", $layer, "_performance.unison_level"),
                        concat!("synth_", $layer, "_performance.vibrato_mode"),
                        concat!("synth_", $layer, "_performance.vibrato_delay"),
                        concat!("synth_", $layer, "_performance.kb_sync_enabled"),
                        concat!("synth_", $layer, "_performance.arpeggiator_run_enabled"),
                        concat!("synth_", $layer, "_voice.filter_enabled"),
                    ],
                    groups: &[
                        controls!(
                            "Pitch stick",
                            switched_on!(concat!(
                                "synth_",
                                $layer,
                                "_performance.pitch_stick_enabled"
                            )),
                            concat!("synth_", $layer, "_performance."),
                            ["pitch_stick_range"]
                        ),
                        controls!(
                            "Mono / legato",
                            Some(Relevance {
                                any_of: &[
                                    Match {
                                        field: concat!(
                                            "synth_",
                                            $layer,
                                            "_performance.mono_enabled"
                                        ),
                                        is: &["true"]
                                    },
                                    Match {
                                        field: concat!(
                                            "synth_",
                                            $layer,
                                            "_performance.legato_enabled"
                                        ),
                                        is: &["true"]
                                    },
                                ]
                            }),
                            concat!("synth_", $layer, "_performance."),
                            ["voice_priority", "glide"]
                        ),
                        controls!(
                            "Keyboard hold",
                            switched_on!("synth_kb_hold_enabled"),
                            concat!("synth_", $layer, "_performance."),
                            ["kb_hold"]
                        ),
                        Group {
                            title: "Arpeggiator / gate",
                            selected_by: None,
                            when: switched_on!(concat!(
                                "synth_",
                                $layer,
                                "_performance.arpeggiator_run_enabled"
                            )),
                            members: &[
                                concat!("synth_", $layer, "_performance.arpeggiator_mode"),
                                concat!("synth_", $layer, "_performance.arp_pattern_enabled"),
                                concat!("synth_", $layer, "_performance.arp_range_env"),
                                concat!("synth_", $layer, "_performance.arp_direction"),
                                concat!("synth_", $layer, "_performance.arp_zigzag_enabled"),
                                concat!("synth_", $layer, "_performance.arp_master_clock_enabled"),
                                concat!("synth_", $layer, "_performance.arp_rate_time"),
                            ],
                            groups: &[controls!(
                                "Pattern",
                                switched_on!(concat!(
                                    "synth_",
                                    $layer,
                                    "_performance.arp_pattern_enabled"
                                )),
                                concat!("synth_", $layer, "_performance."),
                                [
                                    "arp_pattern_length",
                                    "arpeggiator_accent",
                                    "arpeggiator_gate",
                                    "arpeggiator_pan"
                                ]
                            )],
                        },
                        controls!(
                            "Oscillators",
                            None,
                            concat!("synth_", $layer, "_voice."),
                            [
                                "analog_type_knob_1",
                                "analog_cat_knob_2",
                                "analog_wave_partial_knob_3",
                                "osc_ctrl",
                                "pitch_fine",
                                "pitch_coarse",
                                "osc_env_attack",
                                "osc_env_decay",
                                "osc_env_release",
                                "osc_env_amount",
                                "osc_env_to_pitch_enabled",
                                "osc_env_velocity_enabled",
                                "sample_options",
                                "sample_bright_enabled"
                            ]
                        ),
                        controls!(
                            "LFO",
                            None,
                            concat!("synth_", $layer, "_voice."),
                            [
                                "lfo_target",
                                "lfo_shape",
                                "lfo_master_clock_enabled",
                                "lfo_rate_time",
                                "lfo_mod_amount"
                            ]
                        ),
                        controls!(
                            "Amp envelope",
                            None,
                            concat!("synth_", $layer, "_voice."),
                            [
                                "amp_env_attack",
                                "amp_env_decay",
                                "amp_env_release",
                                "amp_env_velocity"
                            ]
                        ),
                        controls!(
                            "Filter",
                            switched_on!(concat!("synth_", $layer, "_voice.filter_enabled")),
                            concat!("synth_", $layer, "_voice."),
                            [
                                "filter_type",
                                "filter_freq",
                                "filter_resonance_freq_hp",
                                "filter_resonance_wheel",
                                "filter_resonance_aftertouch",
                                "filter_resonance_ctrl_pedal",
                                "filter_track",
                                "filter_drive",
                                "filter_env_amount",
                                "filter_env_attack",
                                "filter_env_decay",
                                "filter_env_release",
                                "filter_velocity_enabled"
                            ]
                        ),
                        controls!(
                            "Vibrato",
                            None,
                            concat!("synth_", $layer, "_voice."),
                            ["vibrato_rate", "vibrato_amount"]
                        ),
                        effects!(concat!("synth_", $layer, "_fx.")),
                    ],
                },
            ],
        }
    };
}

pub const PANEL: Panel = Panel {
    notice: Some("Scene selection and some mode mappings are unverified. Controls include settings from both scenes; Global effect edits do not synchronize layers."),
    exhaustive: false,
    groups: &[
        Group {
            title: "Sections",
            selected_by: None,
            when: None,
            members: &[
                "organ_section_enabled",
                "piano_section_enabled",
                "synth_section_enabled",
                "fx_enabled",
                // Hold is released from here with the Synth section off.
                "synth_kb_hold_enabled",
            ],
            groups: &[],
        },
        Group {
            title: "Keyboard & split",
            selected_by: None,
            when: None,
            members: &["split_enabled", "program_transpose_enabled"],
            groups: &[
                controls!(
                    "Transpose",
                    switched_on!("program_transpose_enabled"),
                    "",
                    ["program_transpose_amount"]
                ),
                Group {
                    title: "Split points",
                    selected_by: None,
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
                },
            ],
        },
        Group {
            title: "Organ",
            selected_by: None,
            when: enabled_in_either_scene!("organ_section_enabled"),
            members: &[
                "organ_a_layer_enabled",
                "organ_b_layer_enabled",
                "organ_pitch_stick_enabled",
                "organ_vib_chorus_type",
                "organ_rotary_speaker_enabled",
            ],
            groups: &[
                organ_layer!("Layer A", "a"),
                organ_layer!("Layer B", "b"),
                effects!("organ_fx."),
            ],
        },
        Group {
            title: "Piano",
            selected_by: None,
            when: enabled_in_either_scene!("piano_section_enabled"),
            members: &["piano_a_layer_enabled", "piano_b_layer_enabled"],
            groups: &[
                piano_layer!("Layer A", "a"),
                piano_layer!("Layer B", "b"),
            ],
        },
        Group {
            title: "Synth",
            selected_by: None,
            when: enabled_in_either_scene!("synth_section_enabled"),
            members: &[
                "synth_a_layer_enabled",
                "synth_b_layer_enabled",
                "synth_c_layer_enabled",
                "synth_arp_group_enabled",
            ],
            groups: &[
                synth_layer!("Layer A", "a"),
                synth_layer!("Layer B", "b"),
                synth_layer!("Layer C", "c"),
            ],
        },
        Group {
            title: "Rotary speaker",
            selected_by: None,
            // To Rotary also routes piano and synth layers here; its selector encoding is unknown.
            when: None,
            members: &[
                "rotary_speaker_drive",
                "rotary_speaker_slow_fast",
                "rotary_speaker_stop_enabled",
            ],
            groups: &[Group {
                title: "Stop mode",
                selected_by: None,
                when: switched_on!("rotary_speaker_stop_enabled"),
                members: &[],
                groups: &[controls!(
                    "Stop position",
                    Some(Relevance {
                        any_of: &[Match {
                            field: "rotary_speaker_slow_fast",
                            is: &["Slow"],
                        }]
                    }),
                    "",
                    ["rotary_speaker_stop_position"]
                )],
            }],
        },
        Group {
            // Which layer's stored chain a global effect plays is not established, so the
            // flags sit here and every layer keeps its own chain.
            title: "Effects, globally",
            selected_by: None,
            when: switched_on!("fx_enabled"),
            members: &[
                "fx_comp_global_enabled",
                "fx_delay_global_enabled",
                "fx_reverb_global_enabled",
            ],
            groups: &[],
        },
        Group {
            // ⚠️ Which value of `active_layer_scene` means scene 2 is not established, so a
            // section or layer counts as enabled by either scene — asserting the wrong way
            // round would hide the half that is playing.
            title: "Scene 2",
            selected_by: None,
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

    fn relevant(fields: &[Field], path: &str) -> bool {
        fn find(sections: &[Section<'_>], path: &str) -> Option<bool> {
            sections.iter().find_map(|section| {
                if section.fields.iter().any(|field| field.path == path) {
                    Some(section.relevant)
                } else {
                    find(&section.groups, path)
                }
            })
        }
        find(&PANEL.resolve(fields).sections, path)
            .unwrap_or_else(|| panic!("no panel control for {path}"))
    }

    #[test]
    fn scene_two_only_layers_remain_accessible_without_assuming_scene_bit_polarity() {
        for (section, layer) in [
            ("organ", "a"),
            ("organ", "b"),
            ("piano", "a"),
            ("piano", "b"),
            ("synth", "a"),
            ("synth", "b"),
            ("synth", "c"),
        ] {
            let section_enable = format!("{section}_section_enabled_scene_2");
            let layer_enable = format!("{section}_{layer}_layer_enabled_scene_2");
            let volume = format!("{section}_{layer}_volume");
            for scene in ["false", "true"] {
                let fields = program(&[
                    (&section_enable, "true"),
                    (&layer_enable, "true"),
                    ("active_layer_scene", scene),
                ]);
                assert!(relevant(&fields, &volume), "{volume}, scene bit {scene}");
            }
        }
    }

    #[test]
    fn each_effect_needs_the_master_switch_and_its_own_enable() {
        for prefix in [
            "organ_fx",
            "piano_a_fx",
            "piano_b_fx",
            "synth_a_fx",
            "synth_b_fx",
            "synth_c_fx",
        ] {
            for (effect, parameter) in [
                ("mod_1", "mod_1_rate"),
                ("mod_2", "mod_2_amount"),
                ("amp_sim_eq", "amp_sim_eq_drive"),
                ("comp", "comp_amount"),
                ("delay", "delay_feedback"),
                ("reverb", "reverb_amount"),
            ] {
                let enable = format!("{prefix}.{effect}_enabled");
                let path = format!("{prefix}.{parameter}");
                for master in ["false", "true"] {
                    for enabled in ["false", "true"] {
                        let fields = program(&[
                            ("organ_section_enabled", "true"),
                            ("organ_a_layer_enabled", "true"),
                            ("piano_section_enabled", "true"),
                            ("piano_a_layer_enabled", "true"),
                            ("piano_b_layer_enabled", "true"),
                            ("synth_section_enabled", "true"),
                            ("synth_a_layer_enabled", "true"),
                            ("synth_b_layer_enabled", "true"),
                            ("synth_c_layer_enabled", "true"),
                            ("fx_enabled", master),
                            (&enable, enabled),
                        ]);
                        assert_eq!(
                            relevant(&fields, &path),
                            master == "true" && enabled == "true",
                            "{path}: master={master}, effect={enabled}"
                        );
                        assert_eq!(
                            relevant(&fields, &enable),
                            master == "true",
                            "{enable} must remain reachable"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn extern_replaces_internal_sound_but_keeps_keyboard_routing() {
        for layer in ["a", "b", "c"] {
            let enable = format!("synth_{layer}_layer_enabled");
            let prefix = format!("synth_{layer}_performance");
            let extern_enable = format!("{prefix}.extern_enabled");
            let filter_enable = format!("synth_{layer}_voice.filter_enabled");
            let delay_enable = format!("synth_{layer}_fx.delay_enabled");
            for external in ["false", "true"] {
                let fields = program(&[
                    ("synth_section_enabled", "true"),
                    (&enable, "true"),
                    ("split_enabled", "true"),
                    (&extern_enable, external),
                    (&filter_enable, "true"),
                    ("fx_enabled", "true"),
                    (&delay_enable, "true"),
                ]);
                for parameter in [
                    "kb_zones",
                    "octave_shift",
                    "pitch_stick_enabled",
                    "sustain_pedal_enabled",
                    "extern_enabled",
                ] {
                    assert!(
                        relevant(&fields, &format!("{prefix}.{parameter}")),
                        "{layer}: {parameter}"
                    );
                }
                assert_eq!(
                    relevant(&fields, &format!("{prefix}.extern_cc_val1")),
                    external == "true"
                );
                for path in [
                    format!("synth_{layer}_voice.filter_freq"),
                    format!("synth_{layer}_voice.amp_env_attack"),
                    format!("synth_{layer}_fx.delay_feedback"),
                ] {
                    assert_eq!(relevant(&fields, &path), external == "false", "{path}");
                }
            }
        }
    }

    #[test]
    fn filter_and_pattern_controls_follow_their_independent_enables() {
        for layer in ["a", "b", "c"] {
            let layer_enable = format!("synth_{layer}_layer_enabled");
            let filter_enable = format!("synth_{layer}_voice.filter_enabled");
            let arp_enable = format!("synth_{layer}_performance.arpeggiator_run_enabled");
            let pattern_enable = format!("synth_{layer}_performance.arp_pattern_enabled");
            for filter in ["false", "true"] {
                for arp in ["false", "true"] {
                    for pattern in ["false", "true"] {
                        let fields = program(&[
                            ("synth_section_enabled", "true"),
                            (&layer_enable, "true"),
                            (&filter_enable, filter),
                            (&arp_enable, arp),
                            (&pattern_enable, pattern),
                        ]);
                        assert!(relevant(&fields, &filter_enable));
                        assert!(relevant(&fields, &arp_enable));
                        assert!(relevant(
                            &fields,
                            &format!("synth_{layer}_performance.kb_sync_enabled")
                        ));
                        assert_eq!(
                            relevant(&fields, &format!("synth_{layer}_voice.filter_env_amount")),
                            filter == "true"
                        );
                        assert_eq!(
                            relevant(&fields, &format!("synth_{layer}_performance.arp_rate_time")),
                            arp == "true"
                        );
                        assert_eq!(relevant(&fields, &pattern_enable), arp == "true");
                        assert_eq!(
                            relevant(
                                &fields,
                                &format!("synth_{layer}_performance.arpeggiator_gate")
                            ),
                            arp == "true" && pattern == "true",
                            "{layer}: arp={arp}, pattern={pattern}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn glide_and_note_priority_require_mono_or_legato() {
        for mono in ["false", "true"] {
            for legato in ["false", "true"] {
                let fields = program(&[
                    ("synth_section_enabled", "true"),
                    ("synth_a_layer_enabled", "true"),
                    ("synth_a_performance.mono_enabled", mono),
                    ("synth_a_performance.legato_enabled", legato),
                ]);
                for parameter in ["glide", "voice_priority"] {
                    assert_eq!(
                        relevant(&fields, &format!("synth_a_performance.{parameter}")),
                        mono == "true" || legato == "true",
                        "{parameter}: mono={mono}, legato={legato}"
                    );
                }
            }
        }
    }

    #[test]
    fn rotary_controls_remain_accessible_with_the_organ_off() {
        for stop in ["false", "true"] {
            for speed in ["Slow", "Fast"] {
                let fields = program(&[
                    ("rotary_speaker_stop_enabled", stop),
                    ("rotary_speaker_slow_fast", speed),
                ]);
                for path in [
                    "rotary_speaker_drive",
                    "rotary_speaker_slow_fast",
                    "rotary_speaker_stop_enabled",
                ] {
                    assert!(
                        relevant(&fields, path),
                        "{path} is shared with piano and synth routing"
                    );
                }
                assert_eq!(
                    relevant(&fields, "rotary_speaker_stop_position"),
                    stop == "true" && speed == "Slow"
                );
            }
        }
    }

    #[test]
    fn percussion_options_and_pitch_range_follow_their_switches() {
        for enabled in ["false", "true"] {
            let fields = program(&[
                ("organ_section_enabled", "true"),
                ("organ_a_layer_enabled", "true"),
                ("organ_a.percussion_enabled", enabled),
                ("synth_section_enabled", "true"),
                ("synth_a_layer_enabled", "true"),
                ("synth_a_performance.pitch_stick_enabled", enabled),
            ]);
            assert!(relevant(&fields, "organ_a.percussion_enabled"));
            assert!(relevant(&fields, "synth_a_performance.pitch_stick_enabled"));
            assert_eq!(
                relevant(&fields, "organ_a.percussion_decay_fast_enabled"),
                enabled == "true"
            );
            assert_eq!(
                relevant(&fields, "synth_a_performance.pitch_stick_range"),
                enabled == "true"
            );
        }
    }

    #[test]
    fn keyboard_hold_can_be_released_with_the_synth_section_off() {
        let fields = program(&[("synth_kb_hold_enabled", "true")]);
        assert!(relevant(&fields, "synth_kb_hold_enabled"));
    }

    #[test]
    fn transpose_and_split_boundaries_require_their_enables() {
        for enabled in ["false", "true"] {
            let fields = program(&[
                ("program_transpose_enabled", enabled),
                ("split_enabled", enabled),
                ("kb_zones_1_2_split_point_enabled", "true"),
            ]);
            assert!(relevant(&fields, "program_transpose_enabled"));
            assert_eq!(
                relevant(&fields, "program_transpose_amount"),
                enabled == "true"
            );
            assert_eq!(
                relevant(&fields, "kb_zones_1_2_split_point_xfade"),
                enabled == "true"
            );
            assert!(!relevant(&fields, "kb_zones_2_3_split_point_xfade"));
        }
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
        assert_eq!(PANEL.leftovers(&specs), ["version_echo"]);
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
        assert_eq!(
            resolved
                .leftovers
                .iter()
                .map(|field| field.path.as_str())
                .collect::<Vec<_>>(),
            ["version_echo"]
        );
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
