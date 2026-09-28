//! Field and value names, in the instrument's own words.
//!
//! One table maps a registry path to its section and the label the panel prints beside
//! it; a second maps a stored value's spelling to the words a player would use for it.
//! Both fall back when a lookup misses: an unmapped field shows a prettified path, so a
//! field added to `nord-format` still appears, unpolished.
//!
//! Only English is embedded. Another language needs another pair of tables and one
//! lookup; no module above this one spells a field name itself.

use std::borrow::Borrow;

use nord_format::accept::Family;
use nord_usb::{Location, ObjectClass};

use crate::browser::Kind;

/// A part of a document, named the way the panel divides itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    // A program, left to right across the panel.
    Keyboard,
    Organ,
    Piano,
    Sample,
    Effects,
    Eq,
    // The settings menus.
    System,
    Midi,
    Sound,
    Startup,
    /// Anything the table does not place, including a field newly declared in
    /// `nord-format`.
    Other,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Keyboard => "Keyboard & split",
            Section::Organ => "Organ",
            Section::Piano => "Piano",
            Section::Sample => "Sample",
            Section::Effects => "Effects",
            Section::Eq => "EQ",
            Section::System => "System",
            Section::Midi => "MIDI",
            Section::Sound => "Sound",
            Section::Startup => "At power-on",
            Section::Other => "Also stored",
        }
    }
}

/// The heading for fields with no path prefix, where the registry paths are the only
/// grouping.
pub const UNPREFIXED: &str = "General";

/// The sections a settings document shows, in menu order.
pub const SETTINGS_SECTIONS: [Section; 5] = [
    Section::System,
    Section::Midi,
    Section::Sound,
    Section::Startup,
    Section::Other,
];

/// Each section's registry paths and their labels.
///
/// Alphabetical by path inside each section; a test enforces this.
/// Display order is not this order: a document is laid out by the
/// `nord_format::panel::Panel` its format declares, resolved in `document::field`.
const FIELDS: &[(Section, &[(&str, &str)])] = &[
    (
        Section::Keyboard,
        &[
            ("center_panel.gain", "Program level"),
            ("center_panel.lower_control", "Lower control pedal"),
            ("center_panel.lower_enabled", "Lower part enabled"),
            ("center_panel.lower_octave_shift", "Lower octave shift"),
            ("center_panel.lower_part", "Lower plays"),
            ("center_panel.lower_sustain", "Lower sustain pedal"),
            ("center_panel.part_mix", "Lower / upper balance"),
            ("center_panel.split", "Split"),
            ("center_panel.split_point", "Split point"),
            ("center_panel.transpose", "Transpose (semitones)"),
            ("center_panel.transpose_enabled", "Transpose touched"),
            ("center_panel.unknown_boolean1", "Unnamed bit 18"),
            ("center_panel.upper_control", "Upper control pedal"),
            ("center_panel.upper_enabled", "Upper part enabled"),
            ("center_panel.upper_octave_shift", "Upper octave shift"),
            ("center_panel.upper_part", "Upper plays"),
            ("center_panel.upper_sustain", "Upper sustain pedal"),
        ],
    ),
    (
        Section::Organ,
        &[
            ("center_panel.drawbar_live", "Drawbars live"),
            ("center_panel.organ_type", "Organ model"),
            ("organ_panel.b3_bass_bar1", "Bass drawbar 1"),
            ("organ_panel.b3_bass_bar2", "Bass drawbar 2"),
            ("organ_panel.b3_perc_speed", "Percussion decay"),
            ("organ_panel.b3_perc_third", "Percussion third harmonic"),
            ("organ_panel.b3_preset1_drawbars", "B3 preset 1 drawbars"),
            ("organ_panel.b3_preset1_perc", "B3 preset 1 percussion"),
            ("organ_panel.b3_preset1_vib", "B3 preset 1 vibrato"),
            ("organ_panel.b3_preset2_drawbars", "B3 preset 2 drawbars"),
            ("organ_panel.b3_preset2_perc", "B3 preset 2 percussion"),
            ("organ_panel.b3_preset2_selected", "B3 preset"),
            ("organ_panel.b3_preset2_vib", "B3 preset 2 vibrato"),
            ("organ_panel.b3_vib", "B3 vibrato / chorus"),
            (
                "organ_panel.farfisa_preset1_drawbars",
                "Farfisa preset 1 registers",
            ),
            (
                "organ_panel.farfisa_preset1_vib",
                "Farfisa preset 1 vibrato",
            ),
            (
                "organ_panel.farfisa_preset2_drawbars",
                "Farfisa preset 2 registers",
            ),
            ("organ_panel.farfisa_preset2_selected", "Farfisa preset"),
            (
                "organ_panel.farfisa_preset2_vib",
                "Farfisa preset 2 vibrato",
            ),
            ("organ_panel.farfisa_vib", "Farfisa vibrato / chorus"),
            (
                "organ_panel.pipe_preset1_drawbars",
                "Pipe preset 1 drawbars",
            ),
            (
                "organ_panel.pipe_preset2_drawbars",
                "Pipe preset 2 drawbars",
            ),
            ("organ_panel.pipe_preset2_selected", "Pipe preset"),
            ("organ_panel.vox_preset1_drawbars", "Vox preset 1 drawbars"),
            ("organ_panel.vox_preset1_vib", "Vox preset 1 vibrato"),
            ("organ_panel.vox_preset2_drawbars", "Vox preset 2 drawbars"),
            ("organ_panel.vox_preset2_selected", "Vox preset"),
            ("organ_panel.vox_preset2_vib", "Vox preset 2 vibrato"),
            ("organ_panel.vox_vib", "Vox vibrato"),
        ],
    ),
    (
        Section::Piano,
        &[
            ("piano_panel.acoustics", "Acoustics"),
            ("piano_panel.category", "Type"),
            ("piano_panel.clav_model", "Clavinet model"),
            ("piano_panel.id", "Piano library id"),
            ("piano_panel.mono", "Mono"),
            ("piano_panel.piano_model", "Model"),
            ("piano_panel.touch", "Touch"),
        ],
    ),
    (
        Section::Sample,
        &[
            ("sample_panel.attack", "Attack"),
            ("sample_panel.decay_release", "Decay / release"),
            ("sample_panel.dynamics", "Dynamics"),
            ("sample_panel.filter", "Filter"),
            ("sample_panel.id", "Sample library id"),
            ("sample_panel.number", "Sample number"),
        ],
    ),
    (
        Section::Effects,
        &[
            ("effects_panel.fx1", "Effect 1"),
            ("effects_panel.fx1_control", "Effect 1 on the control pedal"),
            ("effects_panel.fx1_rate", "Effect 1 rate"),
            ("effects_panel.fx1_type", "Effect 1 type"),
            ("effects_panel.fx2", "Effect 2"),
            ("effects_panel.fx2_deep", "Effect 2 deep"),
            ("effects_panel.fx2_rate", "Effect 2 rate"),
            ("effects_panel.fx2_type", "Effect 2 type"),
            ("effects_panel.fx3", "Amp / compressor"),
            ("effects_panel.fx3_compression", "Compression"),
            ("effects_panel.fx3_type", "Amp model"),
            ("effects_panel.fx4", "Delay"),
            ("effects_panel.fx4_feedback", "Delay feedback"),
            ("effects_panel.fx4_moisture", "Delay mix"),
            ("effects_panel.fx4_ping_pong", "Delay ping-pong"),
            ("effects_panel.fx4_tempo", "Delay time"),
            ("effects_panel.fx5", "Reverb"),
            ("effects_panel.fx5_moisture", "Reverb mix"),
            ("effects_panel.fx5_type", "Reverb type"),
            ("effects_panel.rotary_speed", "Rotary fast"),
            ("effects_panel.rotary_stop", "Rotary stop"),
        ],
    ),
    (
        Section::Eq,
        &[
            ("effects_panel.equalizer_bass", "Bass"),
            ("effects_panel.equalizer_freq", "Mid frequency"),
            ("effects_panel.equalizer_freq_gain", "Mid gain"),
            ("effects_panel.equalizer_on", "Equalizer"),
            ("effects_panel.equalizer_part", "Applies to"),
            ("effects_panel.equalizer_treble", "Treble"),
        ],
    ),
    (
        Section::System,
        &[
            ("b3_trig_mode", "Organ key trigger"),
            ("ctrl_pedal_gain", "Control pedal gain"),
            ("ctrl_pedal_type", "Control pedal type"),
            ("fine_tune", "Fine tune (cents)"),
            ("global_transpose", "Transpose (semitones)"),
            ("output_routing", "Outputs"),
            ("rotary_ctrl_type", "Rotary control"),
            ("rotary_pedal_mode", "Rotary pedal"),
            ("sustain_pedal_mode", "Sustain pedal function"),
            ("sustain_pedal_type", "Sustain pedal type"),
        ],
    ),
    (
        Section::Midi,
        &[
            ("control_change_mode", "Control change"),
            ("global_channel", "Global channel"),
            ("lower_receive_channel", "Lower receive channel"),
            ("program_change_mode", "Program change"),
            ("transpose_at", "Transpose applies at"),
            ("upper_receive_channel", "Upper receive channel"),
            ("upper_split_channel", "Upper split channel"),
        ],
    ),
    (
        Section::Sound,
        &[
            ("b3_key_bounce", "Key bounce"),
            ("b3_key_click_level", "Key click level"),
            ("b3_perc_db9_mute", "Percussion mutes drawbar 9"),
            ("b3_perc_decay_fast", "Percussion decay, fast"),
            ("b3_perc_decay_slow", "Percussion decay, slow"),
            ("b3_perc_volume_normal", "Percussion volume, normal"),
            ("b3_perc_volume_soft", "Percussion volume, soft"),
            ("b3_tonewheel_mode", "Tonewheel mode"),
            ("piano_string_resonance", "Piano string resonance (dB)"),
            ("rotary_balance", "Bass / horn balance"),
            ("rotary_horn_acceleration", "Horn acceleration"),
            ("rotary_horn_speed", "Horn speed"),
            ("rotary_rotor_acceleration", "Rotor acceleration"),
            ("rotary_rotor_speed", "Rotor speed"),
            ("rotary_speaker_type", "Rotary speaker"),
        ],
    ),
    (
        Section::Startup,
        &[
            ("startup_live_mode", "Start in Live mode"),
            ("startup_live_slot", "Live slot"),
            ("startup_program", "Program"),
            ("startup_set_list_mode", "Start in Set List mode"),
            ("startup_song", "Set list song"),
        ],
    ),
];

fn entry(path: &str) -> Option<(Section, &'static str)> {
    FIELDS.iter().find_map(|(section, fields)| {
        fields
            .iter()
            .find(|(known, _)| *known == path)
            .map(|(_, label)| (*section, *label))
    })
}

/// What a field is called.
///
/// An unmapped path falls back to its last segment with underscores replaced by spaces,
/// so a field missing from the table still appears under a rough label.
pub fn label(path: &str) -> String {
    if let Some((_, label)) = entry(path) {
        return label.to_string();
    }
    prettify(path)
}

/// Which part of the document a field belongs in.
pub fn section(path: &str) -> Section {
    entry(path).map_or(Section::Other, |(section, _)| section)
}

/// Whether the table maps this path, which tells a real label from a fallback.
pub fn known(path: &str) -> bool {
    entry(path).is_some()
}

/// The last segment of a path as a heading.
fn prettify(path: &str) -> String {
    title(path.rsplit('.').next().unwrap_or(path))
}

/// Part of a path as a heading: dots and underscores become spaces and the first letter
/// is capitalized. Accepts more than one segment, so a nested body's prefix reads as
/// words.
pub fn title(segment: &str) -> String {
    let spaced = segment.replace(['.', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// Value spellings, keyed by the way `nord-format` spells them back.
///
/// Only where the stored spelling is unfriendly. `V1` and `C1` are the panel's own
/// vibrato names and are left alone; numbers and note names speak for themselves.
type Vocabulary = &'static [(&'static str, &'static str)];

const INSTRUMENT: Vocabulary = &[("Organ", "organ"), ("Piano", "piano"), ("Sample", "sample")];

const ORGAN_TYPE: Vocabulary = &[
    ("B3", "B3"),
    ("B3Bass", "B3 + bass"),
    ("Farfisa", "Farfisa"),
    ("Pipe", "pipe"),
    ("Vox", "Vox"),
];

/// ⚠️ `Unknown` is a named variant, not an unrecognized value: older firmware spelled
/// off this way, and the instrument treats it as off. Confirmed on hardware.
const ROUTING: Vocabulary = &[
    ("Lower", "lower"),
    ("Off", "off"),
    ("Unknown", "off (older firmware)"),
    ("Upper", "upper"),
];

const FX1_TYPE: Vocabulary = &[
    ("Pan1", "pan 1"),
    ("Pan1And2", "pan 1 & 2"),
    ("Pan2", "pan 2"),
    ("Rm", "ring modulator"),
    ("Trem1", "tremolo 1"),
    ("Trem1And2", "tremolo 1 & 2"),
    ("Trem2", "tremolo 2"),
    ("Wah", "wah"),
];

const FX2_TYPE: Vocabulary = &[
    ("Chorus1", "chorus 1"),
    ("Chorus2", "chorus 2"),
    ("Flanger", "flanger"),
    ("Phaser1", "phaser 1"),
    ("Phaser2", "phaser 2"),
    ("Vibe", "vibe"),
];

const FX3_TYPE: Vocabulary = &[
    ("Comp", "compressor"),
    ("Jc", "JC amp"),
    ("None_", "none"),
    ("Rotary", "rotary"),
    ("Small", "small amp"),
    ("Twin", "twin amp"),
];

const FX5_TYPE: Vocabulary = &[
    ("Hall", "hall"),
    ("HallSoft", "hall soft"),
    ("Room", "room"),
    ("Stage", "stage"),
    ("StageSoft", "stage soft"),
];

const EQ_PART: Vocabulary = &[
    ("Both", "lower + upper"),
    ("Lower", "lower"),
    ("Upper", "upper"),
];

const PIANO_CATEGORY: Vocabulary = &[
    ("Clavinet", "clavinet"),
    ("EPiano1", "electric piano 1"),
    ("EPiano2", "electric piano 2"),
    ("Grand", "grand"),
    ("Harpsichord", "harpsichord"),
    ("Upright", "upright"),
];

const PERC_SPEED: Vocabulary = &[
    ("Both", "both"),
    ("Fast", "fast"),
    ("Off", "off"),
    ("Soft", "soft"),
];

const SETTINGS_WORDS: Vocabulary = &[
    ("Auto", "auto"),
    ("Bass30Horn70", "30 / 70"),
    ("Bass40Horn60", "40 / 60"),
    ("Bass50Horn50", "50 / 50"),
    ("Bass60Horn40", "60 / 40"),
    ("Bass70Horn30", "70 / 30"),
    ("BossFv500L", "Boss FV-500L"),
    ("Clean", "clean"),
    ("Closed", "closed"),
    ("Fast", "fast"),
    ("FatarSl", "Fatar SL"),
    ("HalfMoon", "half moon"),
    ("High", "high"),
    ("Higher", "higher"),
    ("Hold", "hold"),
    ("KorgExp2", "Korg EXP-2"),
    ("KorgXvp10", "Korg XVP-10"),
    ("Live1", "live 1"),
    ("Live2", "live 2"),
    ("Live3", "live 3"),
    ("Long", "long"),
    ("Low", "low"),
    ("LowerLeftUpperRight", "lower left / upper right"),
    ("Medium", "medium"),
    ("MidiIn", "MIDI in"),
    ("MidiOut", "MIDI out"),
    ("Normal", "normal"),
    ("Off", "off"),
    ("Open", "open"),
    ("Receive", "receive"),
    ("RolandEv7", "Roland EV-7"),
    ("Rotary122", "122"),
    ("Rotary122Close", "122 close"),
    ("Send", "send"),
    ("SendReceive", "send & receive"),
    ("Short", "short"),
    ("Stereo", "stereo"),
    ("Sustain", "sustain"),
    ("SustainRotorHold", "sustain + rotor hold"),
    ("SustainRotorToggle", "sustain + rotor toggle"),
    ("Toggle", "toggle"),
    ("Vintage1", "vintage 1"),
    ("Vintage2", "vintage 2"),
    ("Vintage3", "vintage 3"),
    ("YamahaFc7", "Yamaha FC-7"),
];

fn vocabulary(path: &str) -> Option<Vocabulary> {
    Some(match path {
        "center_panel.lower_part" | "center_panel.upper_part" => INSTRUMENT,
        "center_panel.organ_type" => ORGAN_TYPE,
        "effects_panel.fx1" | "effects_panel.fx2" | "effects_panel.fx3" | "effects_panel.fx4" => {
            ROUTING
        }
        "effects_panel.fx1_type" => FX1_TYPE,
        "effects_panel.fx2_type" => FX2_TYPE,
        "effects_panel.fx3_type" => FX3_TYPE,
        "effects_panel.fx5_type" => FX5_TYPE,
        "effects_panel.equalizer_part" => EQ_PART,
        "piano_panel.category" => PIANO_CATEGORY,
        "organ_panel.b3_perc_speed" => PERC_SPEED,
        // The settings menus share one vocabulary: the wording is per value, and no two
        // settings spell the same variant differently.
        _ if !path.contains('.') => SETTINGS_WORDS,
        _ => return None,
    })
}

/// A stored value as the UI shows it.
///
/// For display only. A caller must keep `raw`, the spelling `nord-format` reads and
/// accepts.
pub fn value_label(path: &str, raw: &str) -> String {
    if let Some(n) = unrecognized(raw) {
        return format!("unrecognized value ({n})");
    }
    if let Some(pair) = vocabulary(path).and_then(|v| v.iter().find(|(stored, _)| *stored == raw)) {
        return pair.1.to_string();
    }
    // A location pair is stored zero-indexed and labeled one-indexed everywhere else.
    if let Some(labeled) = slot_pair(raw) {
        return labeled;
    }
    raw.to_string()
}

/// The stored number behind a value the library could not name.
///
/// `nord-format` renders one as `unknown (5)`; no named value has that form.
pub fn unrecognized(raw: &str) -> Option<u32> {
    raw.strip_prefix("unknown (")?
        .strip_suffix(')')?
        .parse()
        .ok()
}

/// A zero-indexed bank/slot pair such as `(0, 0)`, one-indexed as `1:1`.
fn slot_pair(raw: &str) -> Option<String> {
    let inner = raw.strip_prefix('(')?.strip_suffix(')')?;
    let (bank, slot) = inner.split_once(',')?;
    let bank: u32 = bank.trim().parse().ok()?;
    let slot: u32 = slot.trim().parse().ok()?;
    Some(format!("{}:{}", bank + 1, slot + 1))
}

/// Whether a name already ends in something shaped like a format tag (`patch.ne5p`,
/// `x.body`, `proj.nsmpproj`), so an export must not add a second one and the user need
/// not be shown it.
pub fn carries_tag(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, tag)| {
        !stem.trim().is_empty()
            && ((2..=5).contains(&tag.len())
                || tag.eq_ignore_ascii_case(nord_format::formats::nsmpproj::FORMAT))
            && tag.chars().all(|c| c.is_ascii_alphanumeric())
            && tag.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// `typed`, with the format tag `stored` carries, if any.
///
/// The kind glyph beside a name already shows the file type, so the edit box never shows
/// the tag. A rename, or a duplicate named after its original, must still keep it.
pub fn tagged(stored: &str, typed: &str) -> String {
    match stored.rsplit_once('.').filter(|_| carries_tag(stored)) {
        Some((_, tag)) => format!("{typed}.{tag}"),
        None => typed.to_string(),
    }
}

/// A name as the user sees it: without the format tag, which the kind glyph beside it
/// already says.
///
/// ⚠️ Showing only. The stored name keeps its tag, because that is what a rename edits,
/// what an export is named after, and what the log says happened.
pub fn display_name(name: &str) -> &str {
    match carries_tag(name) {
        true => name.rsplit_once('.').map_or(name, |(stem, _)| stem),
        false => name,
    }
}

/// What the browser calls a class's folder.
pub fn folder(class: ObjectClass) -> &'static str {
    match class {
        ObjectClass::Piano => "Pianos",
        ObjectClass::Sample => "Samples",
        ObjectClass::Program => "Programs",
        ObjectClass::SetList => "Set lists",
        ObjectClass::Live => "Live",
        ObjectClass::Settings => "Settings",
        ObjectClass::Unknown(_) => "Other",
    }
}

/// One-indexed `BANK:SLOT`, the way the instrument and Nord Sound Manager label a
/// location.
pub fn shown(at: Location) -> String {
    format!("{}:{}", at.user_bank(), at.user_slot())
}

/// Where something is, the way a person would say it: `Programs 7:4`.
pub fn place(class: ObjectClass, at: Location) -> String {
    format!("{} {}", folder(class), shown(at))
}

/// How much of a set the attached instrument takes: `6 of 9 fit the Nord Electro 5D 73`.
///
/// A clause, because the library footer joins it with others. `None` when everything
/// fits.
pub fn fitting(fits: usize, of: usize, product: &str) -> Option<String> {
    (fits < of).then(|| format!("{fits} of {of} fit the {product}"))
}

/// A row's kind, prefixed with the family where the kind alone would not say which
/// instrument the file is for: `Stage 4 program`.
///
/// [`crate::browser::qualifier`] decides when; this is the only place a family name is
/// prefixed.
pub fn kind_word(kind: Kind, family: Option<Family>) -> String {
    match family {
        Some(family) => format!("{} {}", family.label(), kind.chip()),
        None => kind.chip().to_string(),
    }
}

/// `n` and the noun phrase for that many: `1 file`, `0 files`, `2 files`.
pub fn counted(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// Words joined as a sentence lists them: `A`, `A and B`, `A, B and C`.
pub fn listed<S: Borrow<str>>(words: &[S]) -> String {
    match words {
        [] => String::new(),
        [one] => one.borrow().to_string(),
        [head @ .., last] => format!("{} and {}", head.join(", "), last.borrow()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_list_reads_as_a_sentence_would_name_it() {
        assert_eq!(listed::<&str>(&[]), "");
        assert_eq!(listed(&["Vox"]), "Vox");
        assert_eq!(listed(&["Vox", "Farfisa"]), "Vox and Farfisa");
        assert_eq!(listed(&["Vox", "Farfisa", "Pipe"]), "Vox, Farfisa and Pipe");
        let owned = ["soft layer".to_string(), "release samples".to_string()];
        assert_eq!(listed(&owned), "soft layer and release samples");
    }

    #[test]
    fn an_unmapped_path_falls_back_to_a_prettified_leaf() {
        assert_eq!(label("center_panel.brand_new_knob"), "Brand new knob");
        assert_eq!(label("nonesuch"), "Nonesuch");
        assert_eq!(section("center_panel.brand_new_knob"), Section::Other);
        assert!(!known("center_panel.brand_new_knob"));
    }

    /// A second entry would silently shadow the first.
    #[test]
    fn no_path_is_listed_twice() {
        let mut seen = HashSet::new();
        for (_, fields) in FIELDS {
            for (path, _) in *fields {
                assert!(seen.insert(*path), "{path} is in the table twice");
            }
        }
    }

    /// ⚠️ A queue diff names a field only by its label, so two fields a user can see
    /// together must not share one. Program and settings fields never share a list, so
    /// each is checked on its own.
    #[test]
    fn no_two_fields_of_one_document_answer_to_one_label() {
        for settings in [false, true] {
            let mut seen = HashSet::new();
            for (section, fields) in FIELDS {
                if SETTINGS_SECTIONS.contains(section) != settings {
                    continue;
                }
                for (path, label) in *fields {
                    assert!(
                        seen.insert(*label),
                        "{path} and another field are both “{label}”"
                    );
                }
            }
        }
    }

    #[test]
    fn the_table_is_alphabetical_within_each_section() {
        for (_, fields) in FIELDS {
            for pair in fields.windows(2) {
                let (was, path) = (pair[0].0, pair[1].0);
                assert!(was < path, "{was} is listed before {path}");
            }
        }
    }

    #[test]
    fn an_unrecognized_value_is_named_as_one() {
        assert_eq!(
            value_label("center_panel.organ_type", "unknown (6)"),
            "unrecognized value (6)"
        );
        assert_eq!(unrecognized("unknown (6)"), Some(6));
        assert_eq!(unrecognized("B3"), None);
    }

    /// A value missing from the tables passes through as the library spells it.
    #[test]
    fn value_spellings_are_translated_where_they_are_unfriendly() {
        assert_eq!(
            value_label("center_panel.organ_type", "B3Bass"),
            "B3 + bass"
        );
        assert_eq!(value_label("effects_panel.fx3_type", "None_"), "none");
        assert_eq!(value_label("ctrl_pedal_type", "YamahaFc7"), "Yamaha FC-7");
        // The panel's own vibrato names.
        assert_eq!(value_label("organ_panel.b3_vib", "C1"), "C1");
        // Numbers speak for themselves.
        assert_eq!(value_label("center_panel.gain", "96"), "96");
    }

    #[test]
    fn the_older_spelling_of_off_reads_as_off() {
        assert_eq!(
            value_label("effects_panel.fx1", "Unknown"),
            "off (older firmware)"
        );
    }

    #[test]
    fn a_stored_location_pair_is_labeled_the_way_the_panel_labels_it() {
        assert_eq!(value_label("startup_program", "(0, 0)"), "1:1");
        assert_eq!(value_label("startup_song", "(3, 49)"), "4:50");
    }

    #[test]
    fn a_shown_name_drops_a_format_tag_and_nothing_else() {
        assert_eq!(display_name("x.ne5p"), "x");
        assert_eq!(display_name("x"), "x");
        assert_eq!(display_name("proj.nsmpproj"), "proj");
        assert_eq!(display_name(".hidden"), ".hidden", "there is no stem");
        assert_eq!(display_name("Africa Split v1.2"), "Africa Split v1.2");
        assert_eq!(display_name("x."), "x.", "an empty tag is not a tag");
    }

    #[test]
    fn a_place_reads_as_a_folder_and_a_slot() {
        let at = Location { bank: 6, slot: 3 };
        assert_eq!(place(ObjectClass::Program, at), "Programs 7:4");
        assert_eq!(shown(at), "7:4");
        assert_eq!(folder(ObjectClass::Unknown(9)), "Other");
    }

    #[test]
    fn only_one_of_a_thing_is_singular() {
        assert_eq!(counted(1, "file", "files"), "1 file");
        assert_eq!(counted(0, "file", "files"), "0 files");
        assert_eq!(counted(2, "entry", "entries"), "2 entries");
    }
}
