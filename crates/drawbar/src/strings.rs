//! What things are called, in the instrument's own words.
//!
//! One table maps a registry path to the section it belongs in and the label the panel
//! prints beside it; a second maps a stored value's spelling to the words a player would
//! use for it. Both fall back rather than refusing — an unmapped field shows a
//! prettified path, so a field added to `nord-format` appears here unpolished instead of
//! invisibly.
//!
//! English is embedded. Another language is another pair of tables and one lookup;
//! nothing above this module spells a field name for itself.

use nord_usb::{Location, ObjectClass};

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
    /// Anything the table does not place. Never empty in the UI without a reason: this
    /// is where a newly declared field turns up.
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

/// The sections a program document shows.
///
/// Keyboard & split leads: its part pickers decide which engine sections exist at all,
/// so the control that brings a section back sits above the ones that come and go. The
/// rest run in panel order.
pub const PROGRAM_SECTIONS: [Section; 7] = [
    Section::Keyboard,
    Section::Organ,
    Section::Piano,
    Section::Sample,
    Section::Effects,
    Section::Eq,
    Section::Other,
];

/// The sections a settings document shows, in menu order.
pub const SETTINGS_SECTIONS: [Section; 5] = [
    Section::System,
    Section::Midi,
    Section::Sound,
    Section::Startup,
    Section::Other,
];

/// Registry path, the section it belongs in, and its label.
///
/// Grouped by section and alphabetical by path inside each group; a test holds it that
/// way. Display order is not this order — see `panel::reading_order`.
const FIELDS: &[(&str, Section, &str)] = &[
    // ── Keyboard & split ───────────────────────────────────────────────────────
    ("center_panel.gain", Section::Keyboard, "Program level"),
    (
        "center_panel.lower_control",
        Section::Keyboard,
        "Lower control pedal",
    ),
    (
        "center_panel.lower_enabled",
        Section::Keyboard,
        "Lower part enabled",
    ),
    (
        "center_panel.lower_octave_shift",
        Section::Keyboard,
        "Lower octave shift",
    ),
    ("center_panel.lower_part", Section::Keyboard, "Lower plays"),
    (
        "center_panel.lower_sustain",
        Section::Keyboard,
        "Lower sustain pedal",
    ),
    (
        "center_panel.part_mix",
        Section::Keyboard,
        "Lower / upper balance",
    ),
    ("center_panel.split", Section::Keyboard, "Split"),
    ("center_panel.split_point", Section::Keyboard, "Split point"),
    (
        "center_panel.transpose",
        Section::Keyboard,
        "Transpose (semitones)",
    ),
    (
        "center_panel.transpose_enabled",
        Section::Keyboard,
        "Transpose touched",
    ),
    (
        "center_panel.unknown_boolean1",
        Section::Keyboard,
        "Unnamed bit 18",
    ),
    (
        "center_panel.upper_control",
        Section::Keyboard,
        "Upper control pedal",
    ),
    (
        "center_panel.upper_enabled",
        Section::Keyboard,
        "Upper part enabled",
    ),
    (
        "center_panel.upper_octave_shift",
        Section::Keyboard,
        "Upper octave shift",
    ),
    ("center_panel.upper_part", Section::Keyboard, "Upper plays"),
    (
        "center_panel.upper_sustain",
        Section::Keyboard,
        "Upper sustain pedal",
    ),
    // ── Organ ──────────────────────────────────────────────────────────────────
    ("center_panel.drawbar_live", Section::Organ, "Drawbars live"),
    ("center_panel.organ_type", Section::Organ, "Organ model"),
    ("organ_panel.b3_bass_bar1", Section::Organ, "Bass drawbar 1"),
    ("organ_panel.b3_bass_bar2", Section::Organ, "Bass drawbar 2"),
    (
        "organ_panel.b3_perc_speed",
        Section::Organ,
        "Percussion decay",
    ),
    (
        "organ_panel.b3_perc_third",
        Section::Organ,
        "Percussion third harmonic",
    ),
    (
        "organ_panel.b3_preset1_drawbars",
        Section::Organ,
        "B3 preset 1 drawbars",
    ),
    ("organ_panel.b3_preset1_perc", Section::Organ, "Percussion"),
    ("organ_panel.b3_preset1_vib", Section::Organ, "Vibrato"),
    (
        "organ_panel.b3_preset2_drawbars",
        Section::Organ,
        "B3 preset 2 drawbars",
    ),
    ("organ_panel.b3_preset2_perc", Section::Organ, "Percussion"),
    (
        "organ_panel.b3_preset2_selected",
        Section::Organ,
        "B3 preset",
    ),
    ("organ_panel.b3_preset2_vib", Section::Organ, "Vibrato"),
    ("organ_panel.b3_vib", Section::Organ, "Vibrato / chorus"),
    (
        "organ_panel.farfisa_preset1_drawbars",
        Section::Organ,
        "Farfisa preset 1 registers",
    ),
    ("organ_panel.farfisa_preset1_vib", Section::Organ, "Vibrato"),
    (
        "organ_panel.farfisa_preset2_drawbars",
        Section::Organ,
        "Farfisa preset 2 registers",
    ),
    (
        "organ_panel.farfisa_preset2_selected",
        Section::Organ,
        "Farfisa preset",
    ),
    ("organ_panel.farfisa_preset2_vib", Section::Organ, "Vibrato"),
    (
        "organ_panel.farfisa_vib",
        Section::Organ,
        "Vibrato / chorus",
    ),
    (
        "organ_panel.pipe_preset1_drawbars",
        Section::Organ,
        "Pipe preset 1 drawbars",
    ),
    (
        "organ_panel.pipe_preset2_drawbars",
        Section::Organ,
        "Pipe preset 2 drawbars",
    ),
    (
        "organ_panel.pipe_preset2_selected",
        Section::Organ,
        "Pipe preset",
    ),
    (
        "organ_panel.vox_preset1_drawbars",
        Section::Organ,
        "Vox preset 1 drawbars",
    ),
    ("organ_panel.vox_preset1_vib", Section::Organ, "Vibrato"),
    (
        "organ_panel.vox_preset2_drawbars",
        Section::Organ,
        "Vox preset 2 drawbars",
    ),
    (
        "organ_panel.vox_preset2_selected",
        Section::Organ,
        "Vox preset",
    ),
    ("organ_panel.vox_preset2_vib", Section::Organ, "Vibrato"),
    ("organ_panel.vox_vib", Section::Organ, "Vibrato"),
    // ── Piano ──────────────────────────────────────────────────────────────────
    ("piano_panel.acoustics", Section::Piano, "Acoustics"),
    ("piano_panel.category", Section::Piano, "Type"),
    ("piano_panel.clav_model", Section::Piano, "Clavinet model"),
    ("piano_panel.id", Section::Piano, "Piano library id"),
    ("piano_panel.mono", Section::Piano, "Mono"),
    ("piano_panel.piano_model", Section::Piano, "Model"),
    ("piano_panel.touch", Section::Piano, "Touch"),
    // ── Sample ─────────────────────────────────────────────────────────────────
    ("sample_panel.attack", Section::Sample, "Attack"),
    (
        "sample_panel.decay_release",
        Section::Sample,
        "Decay / release",
    ),
    ("sample_panel.dynamics", Section::Sample, "Dynamics"),
    ("sample_panel.filter", Section::Sample, "Filter"),
    ("sample_panel.id", Section::Sample, "Sample library id"),
    ("sample_panel.number", Section::Sample, "Sample number"),
    // ── Effects ────────────────────────────────────────────────────────────────
    ("effects_panel.fx1", Section::Effects, "Effect 1"),
    (
        "effects_panel.fx1_control",
        Section::Effects,
        "Effect 1 on the control pedal",
    ),
    ("effects_panel.fx1_rate", Section::Effects, "Effect 1 rate"),
    ("effects_panel.fx1_type", Section::Effects, "Effect 1"),
    ("effects_panel.fx2", Section::Effects, "Effect 2"),
    ("effects_panel.fx2_deep", Section::Effects, "Effect 2 deep"),
    ("effects_panel.fx2_rate", Section::Effects, "Effect 2 rate"),
    ("effects_panel.fx2_type", Section::Effects, "Effect 2"),
    ("effects_panel.fx3", Section::Effects, "Amp / compressor"),
    (
        "effects_panel.fx3_compression",
        Section::Effects,
        "Compression",
    ),
    ("effects_panel.fx3_type", Section::Effects, "Amp model"),
    ("effects_panel.fx4", Section::Effects, "Delay"),
    (
        "effects_panel.fx4_feedback",
        Section::Effects,
        "Delay feedback",
    ),
    ("effects_panel.fx4_moisture", Section::Effects, "Delay mix"),
    (
        "effects_panel.fx4_ping_pong",
        Section::Effects,
        "Delay ping-pong",
    ),
    ("effects_panel.fx4_tempo", Section::Effects, "Delay time"),
    ("effects_panel.fx5", Section::Effects, "Reverb"),
    ("effects_panel.fx5_moisture", Section::Effects, "Reverb mix"),
    ("effects_panel.fx5_type", Section::Effects, "Reverb"),
    (
        "effects_panel.rotary_speed",
        Section::Effects,
        "Rotary fast",
    ),
    ("effects_panel.rotary_stop", Section::Effects, "Rotary stop"),
    // ── EQ ─────────────────────────────────────────────────────────────────────
    ("effects_panel.equalizer_bass", Section::Eq, "Bass"),
    ("effects_panel.equalizer_freq", Section::Eq, "Mid frequency"),
    ("effects_panel.equalizer_freq_gain", Section::Eq, "Mid gain"),
    ("effects_panel.equalizer_on", Section::Eq, "Equalizer"),
    ("effects_panel.equalizer_part", Section::Eq, "Applies to"),
    ("effects_panel.equalizer_treble", Section::Eq, "Treble"),
    // ── Settings: System ───────────────────────────────────────────────────────
    ("b3_trig_mode", Section::System, "Organ key trigger"),
    ("ctrl_pedal_gain", Section::System, "Control pedal gain"),
    ("ctrl_pedal_type", Section::System, "Control pedal type"),
    ("fine_tune", Section::System, "Fine tune (cents)"),
    ("global_transpose", Section::System, "Transpose (semitones)"),
    ("output_routing", Section::System, "Outputs"),
    ("rotary_ctrl_type", Section::System, "Rotary control"),
    ("rotary_pedal_mode", Section::System, "Rotary pedal"),
    (
        "sustain_pedal_mode",
        Section::System,
        "Sustain pedal function",
    ),
    ("sustain_pedal_type", Section::System, "Sustain pedal type"),
    // ── Settings: MIDI ─────────────────────────────────────────────────────────
    ("control_change_mode", Section::Midi, "Control change"),
    ("global_channel", Section::Midi, "Global channel"),
    (
        "lower_receive_channel",
        Section::Midi,
        "Lower receive channel",
    ),
    ("program_change_mode", Section::Midi, "Program change"),
    ("transpose_at", Section::Midi, "Transpose applies at"),
    (
        "upper_receive_channel",
        Section::Midi,
        "Upper receive channel",
    ),
    ("upper_split_channel", Section::Midi, "Upper split channel"),
    // ── Settings: Sound ────────────────────────────────────────────────────────
    ("b3_key_bounce", Section::Sound, "Key bounce"),
    ("b3_key_click_level", Section::Sound, "Key click level"),
    (
        "b3_perc_db9_mute",
        Section::Sound,
        "Percussion mutes drawbar 9",
    ),
    (
        "b3_perc_decay_fast",
        Section::Sound,
        "Percussion decay, fast",
    ),
    (
        "b3_perc_decay_slow",
        Section::Sound,
        "Percussion decay, slow",
    ),
    (
        "b3_perc_volume_normal",
        Section::Sound,
        "Percussion volume, normal",
    ),
    (
        "b3_perc_volume_soft",
        Section::Sound,
        "Percussion volume, soft",
    ),
    ("b3_tonewheel_mode", Section::Sound, "Tonewheel mode"),
    (
        "piano_string_resonance",
        Section::Sound,
        "Piano string resonance (dB)",
    ),
    ("rotary_balance", Section::Sound, "Bass / horn balance"),
    (
        "rotary_horn_acceleration",
        Section::Sound,
        "Horn acceleration",
    ),
    ("rotary_horn_speed", Section::Sound, "Horn speed"),
    (
        "rotary_rotor_acceleration",
        Section::Sound,
        "Rotor acceleration",
    ),
    ("rotary_rotor_speed", Section::Sound, "Rotor speed"),
    ("rotary_speaker_type", Section::Sound, "Rotary speaker"),
    // ── Settings: at power-on ──────────────────────────────────────────────────
    ("startup_live_mode", Section::Startup, "Start in Live mode"),
    ("startup_live_slot", Section::Startup, "Live slot"),
    ("startup_program", Section::Startup, "Program"),
    (
        "startup_set_list_mode",
        Section::Startup,
        "Start in Set List mode",
    ),
    ("startup_song", Section::Startup, "Set list song"),
];

fn entry(path: &str) -> Option<&'static (&'static str, Section, &'static str)> {
    FIELDS.iter().find(|(known, _, _)| *known == path)
}

/// What a field is called.
///
/// An unmapped path falls back to its last segment with the underscores taken out, so a
/// field the table has not caught up with reads as a slightly rough label rather than
/// not appearing.
pub fn label(path: &str) -> String {
    if let Some((_, _, label)) = entry(path) {
        return (*label).to_string();
    }
    prettify(path)
}

/// Which part of the document a field belongs in.
pub fn section(path: &str) -> Section {
    entry(path).map_or(Section::Other, |(_, section, _)| *section)
}

/// Whether the table knows this path at all — what tells a rough label from a real one.
pub fn known(path: &str) -> bool {
    entry(path).is_some()
}

/// The last segment of a path, underscores turned back into spaces and the first letter
/// raised.
fn prettify(path: &str) -> String {
    title(path.rsplit('.').next().unwrap_or(path))
}

/// A stretch of a path as a heading: its separators turned back into spaces and the first
/// letter raised. Takes more than one segment, so a nested body's prefix reads as words
/// rather than as a path.
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

/// ⚠️ `Unknown` is a named variant, not an unrecognised value: it is how older firmware
/// spelled *off*, and it presents as off on the instrument. Confirmed on hardware.
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

/// How a stored value is spoken about.
///
/// `raw` is the spelling `nord-format` reads out and takes back, which is what a caller
/// must keep hold of — this is for showing only.
pub fn value_label(path: &str, raw: &str) -> String {
    if let Some(n) = unrecognised(raw) {
        return format!("unrecognized value ({n})");
    }
    if let Some(pair) = vocabulary(path).and_then(|v| v.iter().find(|(stored, _)| *stored == raw)) {
        return pair.1.to_string();
    }
    // A location pair is stored zero-indexed and labelled one-indexed everywhere else.
    if let Some(labelled) = slot_pair(raw) {
        return labelled;
    }
    raw.to_string()
}

/// The stored number behind a value the library could not name.
///
/// `nord-format` renders one as `unknown (5)`; nothing else does.
pub fn unrecognised(raw: &str) -> Option<u32> {
    raw.strip_prefix("unknown (")?
        .strip_suffix(')')?
        .parse()
        .ok()
}

/// `(0, 0)` — a zero-indexed bank/slot pair — as `1:1`.
fn slot_pair(raw: &str) -> Option<String> {
    let inner = raw.strip_prefix('(')?.strip_suffix(')')?;
    let (bank, slot) = inner.split_once(',')?;
    let bank: u32 = bank.trim().parse().ok()?;
    let slot: u32 = slot.trim().parse().ok()?;
    Some(format!("{}:{}", bank + 1, slot + 1))
}

// ── where things are ─────────────────────────────────────────────────────────────

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
    format!("{}:{}", at.bank + 1, at.slot + 1)
}

/// Where something is, the way a person would say it: `Programs 7:4`.
pub fn place(class: ObjectClass, at: Location) -> String {
    format!("{} {}", folder(class), shown(at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A path the table has not caught up with still reads as something, so a field
    /// added to the library is never invisible.
    #[test]
    fn an_unmapped_path_falls_back_to_a_prettified_leaf() {
        assert_eq!(label("center_panel.brand_new_knob"), "Brand new knob");
        assert_eq!(label("nonesuch"), "Nonesuch");
        assert_eq!(section("center_panel.brand_new_knob"), Section::Other);
        assert!(!known("center_panel.brand_new_knob"));
    }

    /// A mapped path takes the panel's word for it.
    #[test]
    fn a_mapped_path_uses_the_panels_own_word() {
        assert_eq!(label("center_panel.organ_type"), "Organ model");
        assert_eq!(section("center_panel.organ_type"), Section::Organ);
        assert_eq!(section("effects_panel.equalizer_bass"), Section::Eq);
        assert_eq!(section("effects_panel.fx1"), Section::Effects);
        assert_eq!(section("startup_program"), Section::Startup);
    }

    /// One path, one entry: a second would silently shadow the first.
    #[test]
    fn no_path_is_listed_twice() {
        let mut seen = HashSet::new();
        for (path, _, _) in FIELDS {
            assert!(seen.insert(*path), "{path} is in the table twice");
        }
    }

    /// The table is data, and data stays findable: grouped by section, alphabetical
    /// inside each group.
    #[test]
    fn the_table_is_alphabetical_within_each_section() {
        let mut previous: Option<(Section, &str)> = None;
        for (path, section, _) in FIELDS {
            if let Some((before, was)) = previous {
                if before == *section {
                    assert!(was < *path, "{was} is listed before {path}");
                }
            }
            previous = Some((*section, path));
        }
    }

    /// A value with no known meaning says so rather than showing a variant name that
    /// does not exist.
    #[test]
    fn an_unrecognised_value_is_named_as_one() {
        assert_eq!(
            value_label("center_panel.organ_type", "unknown (6)"),
            "unrecognized value (6)"
        );
        assert_eq!(unrecognised("unknown (6)"), Some(6));
        assert_eq!(unrecognised("B3"), None);
    }

    /// The value tables translate; anything they do not carry is passed through as the
    /// library spelled it.
    #[test]
    fn value_spellings_are_translated_where_they_are_unfriendly() {
        assert_eq!(
            value_label("center_panel.organ_type", "B3Bass"),
            "B3 + bass"
        );
        assert_eq!(value_label("effects_panel.fx3_type", "None_"), "none");
        assert_eq!(value_label("ctrl_pedal_type", "YamahaFc7"), "Yamaha FC-7");
        // Real vibrato names, left alone.
        assert_eq!(value_label("organ_panel.b3_vib", "C1"), "C1");
        // Numbers speak for themselves.
        assert_eq!(value_label("center_panel.gain", "96"), "96");
    }

    /// `Routing::Unknown` is off under an older spelling, not an unrecognised value.
    #[test]
    fn the_older_spelling_of_off_reads_as_off() {
        assert_eq!(
            value_label("effects_panel.fx1", "Unknown"),
            "off (older firmware)"
        );
    }

    /// A stored location pair is one-indexed everywhere a person reads it.
    #[test]
    fn a_stored_location_pair_is_labelled_the_way_the_panel_labels_it() {
        assert_eq!(value_label("startup_program", "(0, 0)"), "1:1");
        assert_eq!(value_label("startup_song", "(3, 49)"), "4:50");
    }

    /// The two singleton classes and the folders are named, never numbered.
    #[test]
    fn a_place_reads_as_a_folder_and_a_slot() {
        let at = Location { bank: 6, slot: 3 };
        assert_eq!(place(ObjectClass::Program, at), "Programs 7:4");
        assert_eq!(shown(at), "7:4");
        assert_eq!(folder(ObjectClass::Unknown(9)), "Other");
    }
}
