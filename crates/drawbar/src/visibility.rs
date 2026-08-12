//! What a document shows, worked out from the decoded fields.
//!
//! The file keeps the full state of every organ model and both presets, so switching
//! model is lossless — but only one model's registration means anything at a time. The
//! rules here are the reading half of that: which controls the panel would be showing
//! for the selection the file holds. They are pure functions over the field list so they
//! can be checked without painting anything.

use nord_format::fields::Field;

use crate::strings::{self, Section};

/// What a preset's drawbars are.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Bars {
    /// Nine drawbars, in the nine-nibble register at this path.
    Nine(&'static str),
    /// Nine on/off tabs. ⚠️ The instrument reads a stored nibble of 5 or more as on and
    /// anything lower as off; the position itself means nothing beyond that.
    Tabs(&'static str),
    /// The bass manual: two drawbars, each in its own field.
    Bass(&'static str, &'static str),
}

/// One registration the organ section shows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Registration {
    pub preset: u8,
    /// The preset the instrument is playing.
    pub live: bool,
    pub bars: Bars,
    /// The per-preset vibrato switch, where the model has one.
    pub vib: Option<&'static str>,
    /// The per-preset percussion switch. B3 only.
    pub perc: Option<&'static str>,
}

/// What the organ section shows for the model the program has selected.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Organ {
    /// The selection as the registry spells it, which is what the picker sets back.
    pub selected: String,
    /// False when the stored selection names no model, so nothing below can be trusted.
    pub known: bool,
    /// The field that chooses between the two presets.
    pub preset_field: Option<&'static str>,
    /// The vibrato/chorus mode, shared across presets. `None` for pipe, which has none
    /// the panel can reach.
    pub vib_type: Option<&'static str>,
    /// Percussion third-harmonic and decay-speed fields, shared across presets. B3 only.
    pub perc: Option<(&'static str, &'static str)>,
    pub registrations: Vec<Registration>,
}

impl Organ {
    /// Whether the organ section speaks for this path already.
    ///
    /// Every field belonging to one of the four models is covered, selected or not: a
    /// registration for an organ that is not playing is state, not a control. A field
    /// naming no model is left to render as itself, so an organ field added to the
    /// library turns up rather than being swallowed.
    pub fn covers(&self, path: &str) -> bool {
        let Some(leaf) = path.strip_prefix("organ_panel.") else {
            return false;
        };
        self.known
            && ["b3_", "vox_", "farfisa_", "pipe_"]
                .iter()
                .any(|model| leaf.starts_with(model))
    }
}

/// Where one organ model keeps its state.
struct Paths {
    presets: [&'static str; 2],
    /// Set when preset 2 is the one playing.
    preset2_selected: &'static str,
    vib_type: Option<&'static str>,
    vib: [Option<&'static str>; 2],
    /// The instrument reads this model's drawbars as on/off tabs.
    tabs: bool,
    /// Only the B3 has percussion.
    perc: bool,
}

const B3: Paths = Paths {
    presets: [
        "organ_panel.b3_preset1_drawbars",
        "organ_panel.b3_preset2_drawbars",
    ],
    preset2_selected: "organ_panel.b3_preset2_selected",
    vib_type: Some("organ_panel.b3_vib"),
    vib: [
        Some("organ_panel.b3_preset1_vib"),
        Some("organ_panel.b3_preset2_vib"),
    ],
    tabs: false,
    perc: true,
};

const VOX: Paths = Paths {
    presets: [
        "organ_panel.vox_preset1_drawbars",
        "organ_panel.vox_preset2_drawbars",
    ],
    preset2_selected: "organ_panel.vox_preset2_selected",
    vib_type: Some("organ_panel.vox_vib"),
    vib: [
        Some("organ_panel.vox_preset1_vib"),
        Some("organ_panel.vox_preset2_vib"),
    ],
    tabs: false,
    perc: false,
};

const FARFISA: Paths = Paths {
    presets: [
        "organ_panel.farfisa_preset1_drawbars",
        "organ_panel.farfisa_preset2_drawbars",
    ],
    preset2_selected: "organ_panel.farfisa_preset2_selected",
    vib_type: Some("organ_panel.farfisa_vib"),
    vib: [
        Some("organ_panel.farfisa_preset1_vib"),
        Some("organ_panel.farfisa_preset2_vib"),
    ],
    tabs: true,
    perc: false,
};

/// ⚠️ Pipe has no vibrato the panel can reach: the bit the other models use for
/// preset-1 vib is set in nearly every real program, but the vib button does not respond
/// while pipe is selected. Confirmed on hardware.
const PIPE: Paths = Paths {
    presets: [
        "organ_panel.pipe_preset1_drawbars",
        "organ_panel.pipe_preset2_drawbars",
    ],
    preset2_selected: "organ_panel.pipe_preset2_selected",
    vib_type: None,
    vib: [None, None],
    tabs: false,
    perc: false,
};

const PERC_ON: [&str; 2] = ["organ_panel.b3_preset1_perc", "organ_panel.b3_preset2_perc"];

/// A field's current value, spelled the way `set_field` takes it back.
pub fn value_of<'a>(fields: &'a [Field], path: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.path == path)
        .map(|field| field.value.as_str())
}

fn flag(fields: &[Field], path: &str) -> bool {
    value_of(fields, path) == Some("true")
}

/// What the organ section shows, for a body that has an organ.
pub fn organ(fields: &[Field]) -> Option<Organ> {
    let selected = value_of(fields, "center_panel.organ_type")?.to_string();
    // b3+bass is a selection, not a fifth model: it reads the B3's storage, and its
    // preset 1 is the bass manual.
    let (paths, bass) = match selected.as_str() {
        "B3" => (&B3, false),
        "B3Bass" => (&B3, true),
        "Vox" => (&VOX, false),
        "Farfisa" => (&FARFISA, false),
        "Pipe" => (&PIPE, false),
        _ => {
            return Some(Organ {
                selected,
                known: false,
                preset_field: None,
                vib_type: None,
                perc: None,
                registrations: Vec::new(),
            })
        }
    };

    let live = match flag(fields, paths.preset2_selected) {
        true => 2,
        false => 1,
    };
    let registrations = [1u8, 2]
        .into_iter()
        .map(|preset| {
            let i = preset as usize - 1;
            Registration {
                preset,
                live: preset == live,
                // ⚠️ In b3+bass, preset 1 is the bass manual: only two drawbars are
                // live and they sit outside the nine-nibble block, which holds stale
                // leftovers. Showing those nine would assert a registration that plays
                // nothing.
                bars: match (bass && preset == 1, paths.tabs) {
                    (true, _) => Bars::Bass("organ_panel.b3_bass_bar1", "organ_panel.b3_bass_bar2"),
                    (false, true) => Bars::Tabs(paths.presets[i]),
                    (false, false) => Bars::Nine(paths.presets[i]),
                },
                vib: paths.vib[i],
                perc: paths.perc.then_some(PERC_ON[i]),
            }
        })
        .collect();

    Some(Organ {
        selected,
        known: true,
        preset_field: Some(paths.preset2_selected),
        vib_type: paths.vib_type,
        perc: paths
            .perc
            .then_some(("organ_panel.b3_perc_third", "organ_panel.b3_perc_speed")),
        registrations,
    })
}

/// Whether either keyboard part is set to `instrument`.
///
/// The same condition `nord-cli`'s summary uses to decide the organ block is worth
/// printing at all: a section no part points at is state the program carries, not a
/// sound it makes.
pub fn part_uses(fields: &[Field], instrument: &str) -> bool {
    ["center_panel.lower_part", "center_panel.upper_part"]
        .iter()
        .any(|path| value_of(fields, path) == Some(instrument))
}

/// Whether a section is worth showing at all.
///
/// Effects, EQ and the keyboard are always there — the physical panel always shows those
/// knobs. An engine section is there only while a part is playing it.
///
/// ⚠️ The organ case is the CLI summary's own rule: it prints the organ block only when
/// a part is set to Organ. Piano and sample it prints unconditionally, so hiding *those*
/// two is this app's choice rather than something the summary establishes. What makes it
/// safe either way is that the pickers which turn an engine back on live in Keyboard &
/// split, which never goes away.
pub fn shown(section: Section, fields: &[Field]) -> bool {
    match section {
        Section::Organ => part_uses(fields, "Organ"),
        Section::Piano => part_uses(fields, "Piano"),
        Section::Sample => part_uses(fields, "Sample"),
        _ => true,
    }
}

/// Fields the document does not show.
///
/// Everything here stays in the Advanced table — this is about what a player is offered,
/// not about what the file holds.
pub fn engineering_only(path: &str) -> bool {
    // Nothing in this build maps these to a control on the panel, and the CLI's summary
    // does not report them; they are hidden rather than guessed at.
    const UNMAPPED: [&str; 2] = ["center_panel.lower_enabled", "center_panel.upper_enabled"];
    // The transpose control owns both halves — see `transpose` below.
    const TRANSPOSE: [&str; 2] = ["center_panel.transpose", "center_panel.transpose_enabled"];
    // Library ids, not settings: they name the piano and the sample this program needs,
    // and `nord-cli` is where one gets rewritten.
    const IDS: [&str; 2] = ["piano_panel.id", "sample_panel.id"];

    UNMAPPED.contains(&path)
        || TRANSPOSE.contains(&path)
        || IDS.contains(&path)
        // Reserved bits and anything else the library declares as unexplained.
        || path.contains("unknown")
}

/// Whether a section lists its switches before its knobs.
///
/// For an effect that is the panel's own reading order: what the effect *is* comes
/// before how much of it there is.
pub fn switches_first(section: Section) -> bool {
    matches!(section, Section::Effects | Section::Eq)
}

/// The values a picker offers.
///
/// A value the library could not name is never something to choose. If the file holds
/// one it stays in the list all the same, so a change away from it can be put back.
pub fn choices(path: &str, legal: &[String], current: &str) -> Vec<String> {
    let mut out: Vec<String> = legal
        .iter()
        .filter(|value| offerable(path, value))
        .cloned()
        .collect();
    if !out.iter().any(|value| value == current) {
        out.push(current.to_string());
    }
    out
}

/// Whether a value is one a player would pick.
///
/// ⚠️ `Routing::Unknown` is a named variant rather than an unrecognised number, but it
/// is how older firmware spelled *off* and it presents as off — confirmed on hardware.
/// Two entries both meaning off is a puzzle, not a choice, so only the current one is
/// ever shown.
fn offerable(path: &str, value: &str) -> bool {
    if strings::unrecognised(value).is_some() {
        return false;
    }
    !(value == "Unknown"
        && matches!(
            path,
            "effects_panel.fx1" | "effects_panel.fx2" | "effects_panel.fx3" | "effects_panel.fx4"
        ))
}

/// The transpose control's state: whether the light is on, and the semitones under it.
///
/// ⚠️ Neither field answers on its own. `transpose_enabled` is sticky — the instrument
/// sets it the first time transposition is touched and never clears it — and an
/// untouched program stores `+1` in the value rather than `0`. Confirmed on hardware.
pub fn transpose(fields: &[Field]) -> Option<(bool, i64)> {
    let on = flag(fields, "center_panel.transpose_enabled");
    let semitones = value_of(fields, "center_panel.transpose")?
        .trim_start_matches('+')
        .parse()
        .ok()?;
    Some((on, semitones))
}

/// What a move of the transpose control writes: both halves, together.
///
/// Moving the semitones turns the light on, which is what the panel's own button does —
/// the two are one control there and are one control here.
pub fn set_transpose(on: bool, semitones: i64) -> Vec<(String, String)> {
    vec![
        ("center_panel.transpose_enabled".to_string(), on.to_string()),
        ("center_panel.transpose".to_string(), semitones.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::ne5;
    use nord_format::{Entity, Program};

    use crate::fields::apply;

    fn program(sets: &[(&str, &str)]) -> Vec<Field> {
        let entity = Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().unwrap(),
        )));
        let bytes = nord_format::to_bytes(&entity).unwrap();
        let sets: Vec<(String, String)> = sets
            .iter()
            .map(|(p, v)| ((*p).to_string(), (*v).to_string()))
            .collect();
        apply(&bytes, &sets).expect("the sets are legal").0
    }

    fn organ_of(sets: &[(&str, &str)]) -> Organ {
        organ(&program(sets)).expect("a program has an organ")
    }

    /// A B3 shows both nine-drawbar registrations, its vibrato and its percussion.
    #[test]
    fn a_b3_shows_two_registrations_with_vibrato_and_percussion() {
        let organ = organ_of(&[("center_panel.organ_type", "B3")]);
        assert!(organ.known);
        assert_eq!(organ.vib_type, Some("organ_panel.b3_vib"));
        assert!(organ.perc.is_some());
        assert_eq!(
            organ.registrations[0].bars,
            Bars::Nine("organ_panel.b3_preset1_drawbars")
        );
        assert_eq!(
            organ.registrations[1].bars,
            Bars::Nine("organ_panel.b3_preset2_drawbars")
        );
        assert_eq!(
            organ.registrations[0].perc,
            Some("organ_panel.b3_preset1_perc")
        );
    }

    /// Preset 1 plays until the stored flag says otherwise.
    #[test]
    fn the_marked_preset_is_the_one_the_instrument_is_playing() {
        let organ = organ_of(&[("center_panel.organ_type", "B3")]);
        assert!(organ.registrations[0].live);
        assert!(!organ.registrations[1].live);

        let organ = organ_of(&[
            ("center_panel.organ_type", "B3"),
            ("organ_panel.b3_preset2_selected", "true"),
        ]);
        assert!(!organ.registrations[0].live);
        assert!(organ.registrations[1].live);
    }

    /// Vox has vibrato and no percussion; the B3's registers are not shown for it.
    #[test]
    fn a_vox_shows_its_own_bars_and_no_percussion() {
        let organ = organ_of(&[("center_panel.organ_type", "Vox")]);
        assert_eq!(organ.vib_type, Some("organ_panel.vox_vib"));
        assert_eq!(organ.perc, None);
        assert_eq!(
            organ.registrations[0].bars,
            Bars::Nine("organ_panel.vox_preset1_drawbars")
        );
        assert!(organ.registrations.iter().all(|r| r.perc.is_none()));
    }

    /// Farfisa's drawbars are tabs at the instrument, and the view says so.
    #[test]
    fn a_farfisa_shows_registers_as_tabs() {
        let organ = organ_of(&[("center_panel.organ_type", "Farfisa")]);
        assert_eq!(
            organ.registrations[0].bars,
            Bars::Tabs("organ_panel.farfisa_preset1_drawbars")
        );
        assert_eq!(organ.vib_type, Some("organ_panel.farfisa_vib"));
    }

    /// Pipe has neither vibrato nor percussion the panel can reach.
    #[test]
    fn a_pipe_organ_has_no_vibrato_or_percussion() {
        let organ = organ_of(&[("center_panel.organ_type", "Pipe")]);
        assert_eq!(organ.vib_type, None);
        assert_eq!(organ.perc, None);
        assert!(organ.registrations.iter().all(|r| r.vib.is_none()));
        assert_eq!(
            organ.registrations[1].bars,
            Bars::Nine("organ_panel.pipe_preset2_drawbars")
        );
    }

    /// b3+bass: preset 1 is the bass manual's two drawbars, and the nine nibbles they
    /// shadow are not shown at all.
    #[test]
    fn b3_bass_replaces_preset_one_with_the_bass_manual() {
        let organ = organ_of(&[("center_panel.organ_type", "B3Bass")]);
        assert_eq!(
            organ.registrations[0].bars,
            Bars::Bass("organ_panel.b3_bass_bar1", "organ_panel.b3_bass_bar2")
        );
        // Preset 2 is an ordinary B3.
        assert_eq!(
            organ.registrations[1].bars,
            Bars::Nine("organ_panel.b3_preset2_drawbars")
        );
        // The stale block is spoken for, so nothing renders it.
        assert!(organ.covers("organ_panel.b3_preset1_drawbars"));
        assert!(!organ
            .registrations
            .iter()
            .any(|r| r.bars == Bars::Nine("organ_panel.b3_preset1_drawbars")));
    }

    /// A selection the library cannot name explains itself and asserts nothing about
    /// the registrations, which then show as the plain fields they are.
    #[test]
    fn an_unrecognised_organ_selection_shows_no_registration() {
        let organ = organ_of(&[("center_panel.organ_type", "unknown (6)")]);
        assert!(!organ.known);
        assert!(organ.registrations.is_empty());
        assert!(!organ.covers("organ_panel.b3_preset1_drawbars"));
        assert_eq!(
            strings::value_label("center_panel.organ_type", &organ.selected),
            "unrecognized value (6)"
        );
    }

    /// Every model's own state is spoken for; a field naming no model is not.
    #[test]
    fn the_organ_view_speaks_for_every_models_state() {
        let organ = organ_of(&[("center_panel.organ_type", "B3")]);
        for path in [
            "organ_panel.b3_vib",
            "organ_panel.vox_preset2_drawbars",
            "organ_panel.farfisa_vib",
            "organ_panel.pipe_preset1_drawbars",
        ] {
            assert!(organ.covers(path), "{path}");
        }
        assert!(!organ.covers("organ_panel.something_new"));
        assert!(!organ.covers("center_panel.gain"));
    }

    /// An engine no part is playing is not shown at all, and the part picker that
    /// brings it back is somewhere that always is.
    #[test]
    fn only_the_engines_a_part_is_playing_are_shown() {
        let fields = program(&[]);
        assert!(
            shown(Section::Organ, &fields),
            "a fresh program plays organ"
        );
        assert!(!shown(Section::Piano, &fields));
        assert!(!shown(Section::Sample, &fields));
        for always in [Section::Effects, Section::Eq, Section::Keyboard] {
            assert!(shown(always, &fields));
        }

        let fields = program(&[("center_panel.upper_part", "Piano")]);
        assert!(shown(Section::Piano, &fields));
        assert!(
            shown(Section::Organ, &fields),
            "the lower part still plays it"
        );

        // Both parts off the organ, and it goes — but its picker does not.
        let fields = program(&[
            ("center_panel.lower_part", "Sample"),
            ("center_panel.upper_part", "Piano"),
        ]);
        assert!(!shown(Section::Organ, &fields));
        assert_eq!(
            strings::section("center_panel.lower_part"),
            Section::Keyboard
        );
        assert_eq!(
            strings::section("center_panel.upper_part"),
            Section::Keyboard
        );
    }

    /// The two halves of the transpose pair move together or not at all.
    #[test]
    fn the_transpose_control_writes_both_halves() {
        let fields = program(&[]);
        // An untouched program: the light is off and the stored value is not zero.
        assert_eq!(transpose(&fields), Some((false, 0)));

        let sets = set_transpose(true, -5);
        let paths: Vec<&str> = sets.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            ["center_panel.transpose_enabled", "center_panel.transpose"]
        );
        assert_eq!(sets[0].1, "true");
        assert_eq!(sets[1].1, "-5");

        let after = program(&[
            ("center_panel.transpose_enabled", "true"),
            ("center_panel.transpose", "-5"),
        ]);
        assert_eq!(transpose(&after), Some((true, -5)));
    }

    /// The pair is never offered as two separate controls.
    #[test]
    fn neither_half_of_the_transpose_pair_is_a_field_of_its_own() {
        assert!(engineering_only("center_panel.transpose"));
        assert!(engineering_only("center_panel.transpose_enabled"));
        assert!(!engineering_only("center_panel.gain"));
        assert!(engineering_only("center_panel.unknown_boolean1"));
        assert!(engineering_only("piano_panel.id"));
    }

    /// An unrecognised value is not offered, but a file holding one keeps it reachable.
    #[test]
    fn an_unrecognised_value_is_kept_but_never_offered() {
        let legal: Vec<String> = ["B3", "B3Bass", "Pipe", "unknown (6)"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let ordinary = choices("center_panel.organ_type", &legal, "B3");
        assert_eq!(ordinary, ["B3", "B3Bass", "Pipe"]);

        // Holding one: it is the last entry, so changing away from it can be undone.
        let holding = choices("center_panel.organ_type", &legal, "unknown (6)");
        assert_eq!(holding, ["B3", "B3Bass", "Pipe", "unknown (6)"]);
    }

    /// Two spellings of off would read as two different settings.
    #[test]
    fn the_older_spelling_of_off_is_not_offered_alongside_off() {
        let legal: Vec<String> = ["Off", "Unknown", "Lower", "Upper"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            choices("effects_panel.fx1", &legal, "Off"),
            ["Off", "Lower", "Upper"]
        );
        // A file holding it keeps it, spelled for what it is.
        assert_eq!(
            choices("effects_panel.fx1", &legal, "Unknown"),
            ["Off", "Lower", "Upper", "Unknown"]
        );
        // The same variant name elsewhere is a real choice.
        assert!(offerable("some_other_field", "Unknown"));
    }
}
