//! The Electro 5 global settings format (`.ne5s`).
//!
//! Reads top-down: the format's constants, the read that pairs a header with
//! [`Settings`], then the body itself — the 34 bytes after the header, one flat
//! `#[bitbody]`. A file is a `Cbin<Settings>`, which derefs to the body.
//!
//! The body holds the instrument's System, MIDI and Sound menus. Fields run from bit 38
//! to bit 141 in no particular menu order — the MIDI channels sit between two System
//! settings — so the declaration below is grouped the way the instrument's menus are and
//! the placements do the reordering.
//!
//! Every placement is confirmed on hardware: a capture that changed one setting on the
//! panel moves exactly the bits that setting's field claims, and nothing else. Where a
//! field's *range* runs past the values the captures reach, the field says so.
//!
//! Bits 0..=15 are the schema version echoed into the body, which every `ne5` format
//! carries at `0x2c` because the container header is not transmitted over USB — see
//! [`crate::formats::ne5::song`]. `ne5s` is version 0, so they read zero.
//!
//! Bits 16..=37 are the `startup_*` settings below — the selections the instrument
//! restores at power-up. **Bit 18 is the only bit below the menu settings that no
//! field claims.** It is clear in every specimen. Whatever it is, it survives a re-encode
//! untouched, as does everything past the last setting.
//!
//! **Two cataloged settings are not stored here.** Toggling *memory protect* and *local
//! control* on the panel — the change verified on the display — and re-reading the object
//! moves no bit of the body. Confirmed on hardware: both live outside this object, so
//! neither is decoded.

use crate::cbin::{self, Cbin, Header};
use crate::components::sparse_enum;
use crate::error::{Error, ParseError};
use crate::formats::ne5::{program, song};
use crate::types::RangedI8;
use nord_bits_derive::bitbody;

use std::fmt::{self, Debug, Display, Formatter};
use std::io::{Read, Seek};

pub const FORMAT: &str = "ne5s";
/// Schema versions validated against the corpus. Every corpus settings file reports 0.
pub const KNOWN_VERSIONS: &[u32] = &[0];
/// Length of the settings body block, `0x2c..=0x4d`.
pub const BODY_LEN: usize = 0x4e - 0x2c;
/// Type-1 file length: 44-byte CBIN header + 34-byte body.
pub const FILE_LEN: usize = 0x2c + BODY_LEN;

/// A default settings file.
///
/// There is no slot to speak of: the instrument holds exactly one of these, and every
/// specimen addresses it to bank 0 slot 0.
pub fn new() -> Cbin<Settings> {
    Cbin {
        header: Header::new(FORMAT, (0, 0), 0),
        body: Settings::default(),
    }
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Settings>, Error> {
    let file: Cbin<Settings> = cbin::read(reader, FORMAT)?;
    program::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    program::unset_aux(FORMAT, &file.header)?;
    // The instrument holds exactly one settings file, so the location field has
    // nothing to address; every specimen holds bank 0 slot 0.
    let (bank, slot) = file.header.slot();
    if (bank, slot) != (0, 0) {
        return Err(ParseError::AssertFail(format!(
            "{FORMAT}: location is {bank} {slot}, and settings live at 0 0"
        ))
        .into());
    }
    Ok(file)
}

/// Half-step global transposition, `-6..=6`, stored biased by 6.
pub type GlobalTranspose = RangedI8<6, -6, 6>;

/// Piano string-resonance level in dB, `-6..=6`, stored biased by 6.
pub type ResonanceLevel = RangedI8<6, -6, 6>;

/// Master tuning offset in cents, `-50..=50`, stored biased by 50.
pub type FineTune = RangedI8<50, -50, 50>;

/// The 34-byte settings body: the System, MIDI and Sound menus, interleaved in one
/// bit space with the `startup_*` settings the instrument restores at power-up.
/// Flat, because the two share bytes.
#[bitbody(34)]
pub struct Settings {
    // ── System ─────────────────────────────────────────────────────────────────
    #[bits(52..=53)]
    pub rotary_ctrl_type: RotaryCtrlType,
    #[bits(54..=54)]
    pub rotary_pedal_mode: RotaryPedalMode,
    #[bits(134..=135)]
    pub sustain_pedal_mode: SustainPedalMode,
    #[bits(43..=44)]
    pub sustain_pedal_type: SustainPedalType,
    #[bits(45..=47)]
    pub ctrl_pedal_type: CtrlPedalType,
    #[bits(138..=141)]
    pub ctrl_pedal_gain: CtrlPedalGain,
    #[bits(117..=117)]
    pub b3_trig_mode: B3TrigMode,
    #[bits(128..=128)]
    pub output_routing: OutputRouting,
    #[bits(68..=71)]
    pub global_transpose: GlobalTranspose,
    /// Only `-50`, `0` and `+50` appear in the sweep; the bias and the width come from
    /// those three plus the default of `+5`. Values between them are inferred from
    /// specimens; not confirmed on hardware.
    #[bits(55..=61)]
    pub fine_tune: FineTune,

    // ── MIDI ───────────────────────────────────────────────────────────────────
    #[bits(72..=76)]
    pub global_channel: MidiChannel,
    #[bits(118..=122)]
    pub lower_receive_channel: MidiChannel,
    #[bits(123..=127)]
    pub upper_receive_channel: MidiChannel,
    #[bits(129..=133)]
    pub upper_split_channel: MidiChannel,
    #[bits(38..=39)]
    pub control_change_mode: MidiMessageMode,
    #[bits(40..=41)]
    pub program_change_mode: MidiMessageMode,
    #[bits(137..=137)]
    pub transpose_at: TransposeAt,

    // ── Sound ──────────────────────────────────────────────────────────────────
    /// Only `-6`, `0` and `+6` dB appear in the sweep, so every odd value is inferred
    /// from specimens; not confirmed on hardware. The bias matches [`GlobalTranspose`],
    /// whose odd values the sweep does reach.
    #[bits(64..=67)]
    pub piano_string_resonance: ResonanceLevel,
    #[bits(109..=111)]
    pub b3_tonewheel_mode: TonewheelMode,
    #[bits(113..=114)]
    pub b3_key_click_level: KeyClickLevel,
    #[bits(116..=116)]
    pub b3_key_bounce: bool,
    #[bits(112..=112)]
    pub b3_perc_db9_mute: bool,
    #[bits(97..=99)]
    pub b3_perc_decay_fast: PercDecay,
    #[bits(100..=102)]
    pub b3_perc_decay_slow: PercDecay,
    #[bits(103..=105)]
    pub b3_perc_volume_normal: PercVolume,
    #[bits(106..=108)]
    pub b3_perc_volume_soft: PercVolume,
    /// The panel dials through more entries than the two the corpus names, so an
    /// unrecognized value here is expected rather than a decode failure.
    #[bits(79..=81)]
    pub rotary_speaker_type: RotarySpeakerType,
    #[bits(94..=96)]
    pub rotary_balance: RotaryBalance,
    #[bits(82..=84)]
    pub rotary_horn_speed: RotaryRate,
    #[bits(88..=90)]
    pub rotary_horn_acceleration: RotaryRate,
    /// The sweep's `low` specimen is byte-identical to its `high` one, so only `normal`
    /// and `high` are confirmed here; `low` is inferred from the three sibling rate
    /// fields, which share this encoding and do reach it.
    #[bits(85..=87)]
    pub rotary_rotor_speed: RotaryRate,
    #[bits(91..=93)]
    pub rotary_rotor_acceleration: RotaryRate,

    // ── Startup ────────────────────────────────────────────────────────────────
    //
    // Boot-state settings: none of these appears in a menu, and the instrument
    // restores each at power-up. Changing any of the 34 cataloged settings leaves
    // every bit here alone, and a settings file written by a different capture
    // session differs here and nowhere else.
    //
    // Both locations are `bank * 50 + slot`, zero-based — the packing
    // [`crate::formats::ne5::song`] uses for its program references.
    //
    // `startup_live_slot` survives leaving Live mode and `startup_program` survives
    // entering it, so each holds the last selection of its kind rather than the
    // current one.
    /// Inferred from specimens; not confirmed on hardware. One specimen holds it — the
    /// one capture made in set list mode. The full backup moves `startup_song` without
    /// it, so the bit tracks the mode, not the song.
    #[bits(16..=16)]
    pub startup_set_list_mode: bool,
    #[bits(17..=17)]
    pub startup_live_mode: bool,
    #[bits(19..=20)]
    pub startup_live_slot: LiveSlot,
    #[bits(21..=29)]
    pub startup_program: program::Location,
    /// Inferred from specimens; not confirmed on hardware. The sweep never leaves the
    /// first slot, and the two files that do move it are a full backup and a capture
    /// predating the sweep.
    #[bits(30..=37)]
    pub startup_song: song::Location,
}

sparse_enum!(
    /// Which of the three Live slots is selected.
    LiveSlot, 2, {
        0 => Live1, "live 1";
        1 => Live2, "live 2";
        2 => Live3, "live 3";
    }
);

/// The instrument's menu a setting is shown under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    System,
    Midi,
    Sound,
}

impl Menu {
    pub fn title(&self) -> &'static str {
        match self {
            Menu::System => "System",
            Menu::Midi => "MIDI",
            Menu::Sound => "Sound",
        }
    }
}

/// One setting as the instrument's menu presents it.
pub struct Setting {
    /// The field's name in [`Settings`].
    pub name: &'static str,
    /// The value spelled the way the instrument spells it — `yamaha fc-7`, not the
    /// variant name the bits decode to.
    pub value: String,
}

/// Every field decodes from zeroed bytes — program `1:1`, Live and set list mode
/// off — so this is the decode rather than a second statement of each default.
impl Default for Settings {
    fn default() -> Self {
        Settings::try_from([0; BODY_LEN]).expect("every settings field decodes totally")
    }
}

/// An on/off setting, as its menu entry names the two states.
fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

impl Settings {
    /// The panel's fields grouped by the menu the instrument shows them under, in menu
    /// order — which is neither declaration order nor the order they sit in the file.
    ///
    /// ⚠️ These renderings are for reading, not for feeding back: `Display` is the panel's
    /// wording, while [`Settings::set_field`] parses a field's `Debug`. A test holds the
    /// list to the panel's own field names, so a field added to [`Settings`] and not
    /// placed in a menu fails there.
    pub fn by_menu(&self) -> Vec<(Menu, Vec<Setting>)> {
        let at = |name, value: String| Setting { name, value };
        vec![
            (
                Menu::System,
                vec![
                    at("rotary_ctrl_type", self.rotary_ctrl_type.to_string()),
                    at("rotary_pedal_mode", self.rotary_pedal_mode.to_string()),
                    at("sustain_pedal_mode", self.sustain_pedal_mode.to_string()),
                    at("sustain_pedal_type", self.sustain_pedal_type.to_string()),
                    at("ctrl_pedal_type", self.ctrl_pedal_type.to_string()),
                    at("ctrl_pedal_gain", self.ctrl_pedal_gain.to_string()),
                    at("b3_trig_mode", self.b3_trig_mode.to_string()),
                    at("output_routing", self.output_routing.to_string()),
                    at(
                        "global_transpose",
                        format!("{:+}", self.global_transpose.inner()),
                    ),
                    at("fine_tune", format!("{:+} cent", self.fine_tune.inner())),
                ],
            ),
            (
                Menu::Midi,
                vec![
                    at("global_channel", self.global_channel.to_string()),
                    at(
                        "lower_receive_channel",
                        self.lower_receive_channel.to_string(),
                    ),
                    at(
                        "upper_receive_channel",
                        self.upper_receive_channel.to_string(),
                    ),
                    at("upper_split_channel", self.upper_split_channel.to_string()),
                    at("control_change_mode", self.control_change_mode.to_string()),
                    at("program_change_mode", self.program_change_mode.to_string()),
                    at("transpose_at", self.transpose_at.to_string()),
                ],
            ),
            (
                Menu::Sound,
                vec![
                    at(
                        "piano_string_resonance",
                        format!("{:+} dB", self.piano_string_resonance.inner()),
                    ),
                    at("b3_tonewheel_mode", self.b3_tonewheel_mode.to_string()),
                    at("b3_key_click_level", self.b3_key_click_level.to_string()),
                    at("b3_key_bounce", on_off(self.b3_key_bounce)),
                    at("b3_perc_db9_mute", on_off(self.b3_perc_db9_mute)),
                    at("b3_perc_decay_fast", self.b3_perc_decay_fast.to_string()),
                    at("b3_perc_decay_slow", self.b3_perc_decay_slow.to_string()),
                    at(
                        "b3_perc_volume_normal",
                        self.b3_perc_volume_normal.to_string(),
                    ),
                    at("b3_perc_volume_soft", self.b3_perc_volume_soft.to_string()),
                    at("rotary_speaker_type", self.rotary_speaker_type.to_string()),
                    at("rotary_balance", self.rotary_balance.to_string()),
                    at("rotary_horn_speed", self.rotary_horn_speed.to_string()),
                    at(
                        "rotary_horn_acceleration",
                        self.rotary_horn_acceleration.to_string(),
                    ),
                    at("rotary_rotor_speed", self.rotary_rotor_speed.to_string()),
                    at(
                        "rotary_rotor_acceleration",
                        self.rotary_rotor_acceleration.to_string(),
                    ),
                ],
            ),
        ]
    }
}

sparse_enum!(
    /// How the rotary speaker's speed is controlled.
    RotaryCtrlType, 2, {
        0 => Closed, "closed";
        1 => Open, "open";
        2 => HalfMoon, "half moon";
    }
);

sparse_enum!(
    /// Whether the rotary pedal runs fast while held or latches.
    RotaryPedalMode, 1, {
        0 => Hold, "hold";
        1 => Toggle, "toggle";
    }
);

sparse_enum!(
    /// What the sustain pedal drives besides sustain.
    SustainPedalMode, 2, {
        0 => Sustain, "sustain";
        1 => SustainRotorHold, "sustain + rotor hold";
        2 => SustainRotorToggle, "sustain + rotor toggle";
    }
);

sparse_enum!(
    /// Sustain pedal polarity. `Auto` detects it at power-up.
    SustainPedalType, 2, {
        0 => Auto, "auto";
        1 => Closed, "closed";
        2 => Open, "open";
    }
);

sparse_enum!(
    /// Which expression pedal is plugged into the control input.
    CtrlPedalType, 3, {
        0 => RolandEv7, "roland ev-7";
        1 => YamahaFc7, "yamaha fc-7";
        2 => KorgExp2, "korg exp-2";
        3 => KorgXvp10, "korg xvp-10";
        4 => BossFv500L, "boss fv-500l";
        5 => FatarSl, "fatar sl";
    }
);

sparse_enum!(
    /// How early the B3 key contacts trigger.
    B3TrigMode, 1, {
        0 => Normal, "normal";
        1 => Fast, "fast";
    }
);

sparse_enum!(
    /// How the two parts are laid across the outputs.
    OutputRouting, 1, {
        0 => Stereo, "stereo";
        1 => LowerLeftUpperRight, "lower L / upper R";
    }
);

sparse_enum!(
    /// Which directions a class of MIDI message travels.
    MidiMessageMode, 2, {
        0 => Off, "off";
        1 => Send, "send";
        2 => Receive, "receive";
        3 => SendReceive, "send/receive";
    }
);

sparse_enum!(
    /// Which side of the MIDI port transposition is applied to.
    TransposeAt, 1, {
        0 => MidiIn, "midi in";
        1 => MidiOut, "midi out";
    }
);

sparse_enum!(
    /// How much tonewheel leakage and crosstalk the B3 model adds.
    TonewheelMode, 3, {
        0 => Clean, "clean";
        1 => Vintage1, "vintage 1";
        2 => Vintage2, "vintage 2";
        3 => Vintage3, "vintage 3";
    }
);

sparse_enum!(
    /// B3 key-click level.
    KeyClickLevel, 2, {
        0 => Low, "low";
        1 => Normal, "normal";
        2 => High, "high";
        3 => Higher, "higher";
    }
);

sparse_enum!(
    /// B3 percussion decay length, per speed setting.
    PercDecay, 3, {
        0 => Short, "short";
        1 => Medium, "medium";
        2 => Long, "long";
    }
);

sparse_enum!(
    /// B3 percussion level, per volume setting.
    PercVolume, 3, {
        0 => Low, "low";
        1 => Medium, "medium";
        2 => High, "high";
    }
);

sparse_enum!(
    /// Which rotary cabinet the effect models.
    RotarySpeakerType, 3, {
        0 => Rotary122, "122";
        1 => Rotary122Close, "122 close";
    }
);

sparse_enum!(
    /// Rotary bass/horn mix, as the panel spells it.
    RotaryBalance, 3, {
        0 => Bass70Horn30, "70/30";
        1 => Bass60Horn40, "60/40";
        2 => Bass50Horn50, "50/50";
        3 => Bass40Horn60, "40/60";
        4 => Bass30Horn70, "30/70";
    }
);

sparse_enum!(
    /// A rotary speed or acceleration trim. Shared by all four horn/rotor fields.
    RotaryRate, 3, {
        0 => Low, "low";
        1 => Normal, "normal";
        2 => High, "high";
    }
);

/// A MIDI channel slot: `1..=16`, or off.
///
/// Stored zero-based, with 16 for off. A pattern above that has no meaning and is kept
/// as [`MidiChannel::Unknown`] rather than folded into a channel.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum MidiChannel {
    /// Channel `1..=16`, as the panel numbers them. ⚠️ Build it with
    /// [`MidiChannel::channel`]: a number outside that range has no five-bit encoding and
    /// would be written as some other channel.
    Channel(u8),
    Off,
    /// A stored value with no known meaning.
    Unknown(u8),
}

impl MidiChannel {
    /// How many channels the panel numbers.
    pub const CHANNELS: u8 = 16;

    /// Channel `1..=16`.
    pub fn channel(number: u8) -> Result<Self, ParseError> {
        if !(1..=Self::CHANNELS).contains(&number) {
            return Err(ParseError::OutOfBounds {
                value: format!("{number}"),
                bound: format!("1..={}", Self::CHANNELS),
            });
        }
        Ok(MidiChannel::Channel(number))
    }

    /// The panel's channel number, or `None` for off or unknown.
    pub fn number(&self) -> Option<u8> {
        match self {
            MidiChannel::Channel(n) => Some(*n),
            _ => None,
        }
    }
}

impl crate::bits::Packed for MidiChannel {
    const MAX_BITS: u32 = 5;
    type Error = std::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(match bits as u8 {
            n if n < Self::CHANNELS => MidiChannel::Channel(n + 1),
            16 => MidiChannel::Off,
            other => MidiChannel::Unknown(other),
        })
    }

    fn to_bits(&self) -> u64 {
        match self {
            MidiChannel::Channel(n) => u64::from(n.saturating_sub(1)),
            MidiChannel::Off => 16,
            MidiChannel::Unknown(raw) => u64::from(*raw),
        }
    }
}

impl Debug for MidiChannel {
    /// The channel number alone, so `1` is spelled `1` and not `Channel(1)`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            MidiChannel::Channel(n) => write!(f, "{n}"),
            MidiChannel::Off => f.write_str("off"),
            MidiChannel::Unknown(raw) => write!(f, "unknown ({raw})"),
        }
    }
}

impl Display for MidiChannel {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Control pedal gain, `1..=10` as the panel reads it. Stored one less.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CtrlPedalGain(u8);

impl CtrlPedalGain {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 10;

    pub fn new(gain: u8) -> Result<Self, ParseError> {
        if !(Self::MIN..=Self::MAX).contains(&gain) {
            return Err(ParseError::OutOfBounds {
                value: format!("{gain}"),
                bound: format!("{}..={}", Self::MIN, Self::MAX),
            });
        }
        Ok(CtrlPedalGain(gain))
    }

    /// The panel's reading, `1..=10`.
    pub fn as_u8(&self) -> u8 {
        self.0
    }
}

impl Default for CtrlPedalGain {
    fn default() -> Self {
        CtrlPedalGain(Self::MIN)
    }
}

impl crate::bits::Packed for CtrlPedalGain {
    const MAX_BITS: u32 = 4;
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        CtrlPedalGain::new((bits as u8).saturating_add(1))
    }

    fn to_bits(&self) -> u64 {
        u64::from(self.0 - Self::MIN)
    }
}

impl Debug for CtrlPedalGain {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Display for CtrlPedalGain {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<u8> for CtrlPedalGain {
    fn eq(&self, other: &u8) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::Packed;
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::Cursor;

    /// Body-relative index of the byte at absolute Electro 5 file offset `abs`.
    const fn body(abs: usize) -> usize {
        abs - 0x2c
    }

    /// Build a settings panel from `(absolute offset, byte)` pairs; everything else 0.
    fn panel(bytes: &[(usize, u8)]) -> Settings {
        let mut raw = [0u8; BODY_LEN];
        for &(at, b) in bytes {
            raw[body(at)] = b;
        }
        Settings::try_from(raw).expect("every settings field decodes totally")
    }

    #[test]
    fn settings_round_trip_at_the_declared_length() {
        let settings = new();
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);
        assert_eq!(&bytes[0x08..0x0c], FORMAT.as_bytes());

        let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();
        let mut again = Vec::new();
        back.write_to(&mut Cursor::new(&mut again)).unwrap();
        assert_eq!(bytes, again);
    }

    /// There is one settings file per instrument, so a file claiming a slot is not
    /// one of them.
    #[test]
    fn a_settings_file_addressed_to_a_slot_is_refused() {
        let mut settings = new();
        settings.header.set_slot((1, 0));
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();

        let err = read_from(&mut Cursor::new(&bytes))
            .expect_err("a located settings file must not decode");
        assert!(
            matches!(err, Error::Parse(ParseError::AssertFail(_))),
            "refused for the wrong reason: {err}",
        );
    }

    #[test]
    fn a_settings_field_set_by_name_survives_a_round_trip() {
        let mut settings = new();
        settings.set_field("global_transpose", "-3").unwrap();

        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();
        let listed = back
            .fields()
            .into_iter()
            .find(|f| f.path == "global_transpose")
            .expect("declared");
        assert_eq!(listed.display, "-3");
    }

    /// Every declared menu field belongs to exactly one menu, and every menu names
    /// only declared fields. The `startup_*` settings are in no menu — the
    /// instrument shows them nowhere — so they are excluded rather than missing.
    #[test]
    fn every_field_is_listed_under_one_menu() {
        let declared: BTreeSet<String> = Settings::field_specs()
            .into_iter()
            .map(|f| f.name)
            .filter(|name| !name.starts_with("startup_"))
            .collect();

        let mut grouped: BTreeSet<String> = BTreeSet::new();
        for (menu, settings) in Settings::default().by_menu() {
            for setting in settings {
                assert!(
                    declared.contains(setting.name),
                    "{} lists {}, which the panel does not declare",
                    menu.title(),
                    setting.name,
                );
                assert!(
                    grouped.insert(setting.name.to_string()),
                    "{} is listed under two menus",
                    setting.name,
                );
            }
        }
        let missing: Vec<_> = declared.difference(&grouped).collect();
        assert!(missing.is_empty(), "fields with no menu: {missing:?}");
    }

    /// A menu renders what the instrument's display says, not what the bits decode to.
    ///
    /// ⚠️ These spellings are read-only. `set_field` parses a field's `Debug`, so
    /// `yamaha fc-7` is not a value anything accepts back.
    #[test]
    fn a_menu_renders_the_panels_own_wording() {
        // The sweep's reference capture, rebuilt from the bytes it holds at 0x2c..=0x3d.
        let p = panel(&[
            (0x2e, 0x13),
            (0x2f, 0x24),
            (0x30, 0x03),
            (0x31, 0xc1),
            (0x32, 0x06),
            (0x33, 0xdc),
            (0x34, 0x66),
            (0x35, 0x00),
            (0x36, 0x09),
            (0x37, 0x25),
            (0x38, 0x12),
            (0x39, 0x49),
            (0x3a, 0x2d),
            (0x3b, 0x61),
            (0x3c, 0x06),
            (0x3d, 0x24),
        ]);
        let rendered: BTreeMap<&str, String> = p
            .by_menu()
            .into_iter()
            .flat_map(|(_, settings)| settings)
            .map(|s| (s.name, s.value))
            .collect();

        for (field, want) in [
            ("ctrl_pedal_type", "yamaha fc-7"),
            ("ctrl_pedal_gain", "10"),
            ("sustain_pedal_mode", "sustain + rotor toggle"),
            ("output_routing", "stereo"),
            ("global_transpose", "+0"),
            ("fine_tune", "+5 cent"),
            ("global_channel", "1"),
            ("control_change_mode", "send/receive"),
            ("transpose_at", "midi in"),
            ("piano_string_resonance", "+0 dB"),
            ("b3_tonewheel_mode", "vintage 1"),
            ("b3_key_bounce", "on"),
            ("b3_perc_db9_mute", "off"),
            ("rotary_speaker_type", "122"),
            ("rotary_balance", "50/50"),
            ("rotary_rotor_acceleration", "normal"),
        ] {
            assert_eq!(rendered[field], want, "{field}");
        }
    }

    /// An unnamed value says so rather than being rendered as a neighbor.
    #[test]
    fn a_menu_names_an_unrecognized_value_as_unknown() {
        // 0b111 is not a rotary speaker type; bits 79..=81 straddle 0x35 and 0x36.
        let p = panel(&[(0x35, 0x01), (0x36, 0xc0)]);
        let shown = p
            .by_menu()
            .into_iter()
            .flat_map(|(_, settings)| settings)
            .find(|s| s.name == "rotary_speaker_type")
            .expect("declared")
            .value;
        assert_eq!(shown, "unknown (7)");
    }

    /// The two signed fields are stored biased, so their endpoints are the cases worth
    /// pinning: the bias is what an off-by-one shows up in.
    #[test]
    fn global_transpose_stores_minus_six_as_zero() {
        // Bits 68..=71 are the low nibble of 0x34.
        for (semitones, stored) in [(-6i8, 0x0u8), (-1, 0x5), (0, 0x6), (1, 0x7), (6, 0xc)] {
            let p = panel(&[(0x34, stored)]);
            assert_eq!(p.global_transpose, semitones, "stored {stored:#x}");
            assert_eq!(
                <[u8; BODY_LEN]>::from(&p)[body(0x34)],
                stored,
                "{semitones} did not write back"
            );
        }
        // The high nibble is the string resonance and must not leak in.
        assert_eq!(panel(&[(0x34, 0xc6)]).global_transpose, 0);
        assert_eq!(panel(&[(0x34, 0xc6)]).piano_string_resonance, 6);
    }

    #[test]
    fn fine_tune_stores_minus_fifty_cents_as_zero() {
        // Bits 55..=61: the low bit of 0x32 and the top six of 0x33.
        for (cents, at32, at33) in [
            (-50i8, 0x00u8, 0x00u8),
            (0, 0x00, 0xc8),
            (5, 0x00, 0xdc),
            (50, 0x01, 0x90),
        ] {
            let p = panel(&[(0x32, at32), (0x33, at33)]);
            assert_eq!(p.fine_tune, cents, "stored {at32:#04x} {at33:#04x}");
            let back = <[u8; BODY_LEN]>::from(&p);
            assert_eq!(
                (back[body(0x32)], back[body(0x33)]),
                (at32, at33),
                "{cents} did not write back"
            );
        }
        // A value past +50 is not a fine tune, so the panel refuses to decode at all.
        let mut raw = [0u8; BODY_LEN];
        raw[body(0x32)] = 0x01;
        raw[body(0x33)] = 0xfc;
        assert!(Settings::try_from(raw).is_err(), "101 cents decoded");
    }

    /// Channels are stored zero-based with 16 for off, so the two ends and the off value
    /// are what pin the encoding.
    #[test]
    fn a_midi_channel_is_stored_zero_based_with_sixteen_for_off() {
        for (bits, channel) in [
            (0u64, MidiChannel::Channel(1)),
            (1, MidiChannel::Channel(2)),
            (15, MidiChannel::Channel(16)),
            (16, MidiChannel::Off),
            (17, MidiChannel::Unknown(17)),
            (31, MidiChannel::Unknown(31)),
        ] {
            assert_eq!(MidiChannel::from_bits(bits).unwrap(), channel);
            assert_eq!(channel.to_bits(), bits, "{channel:?} does not round-trip");
        }
        assert_eq!(format!("{:?}", MidiChannel::Channel(7)), "7");
        assert_eq!(format!("{:?}", MidiChannel::Off), "off");
    }

    /// Gain is one-based on the panel and zero-based in the file.
    #[test]
    fn ctrl_pedal_gain_is_stored_one_less_than_the_panel_reads() {
        for gain in CtrlPedalGain::MIN..=CtrlPedalGain::MAX {
            let stored = CtrlPedalGain::new(gain).unwrap().to_bits();
            assert_eq!(stored, u64::from(gain) - 1);
            assert_eq!(CtrlPedalGain::from_bits(stored).unwrap(), gain);
        }
        assert!(CtrlPedalGain::new(0).is_err());
        assert!(CtrlPedalGain::new(11).is_err());
        // Ten is the widest the four-bit slot may hold, so 10..=15 do not decode.
        assert!(CtrlPedalGain::from_bits(10).is_err());
    }

    /// A default panel is the decode of zeroed bytes, and re-encoding it gives them back.
    #[test]
    fn the_default_panel_encodes_and_decodes() {
        let p = Settings::default();
        assert_eq!(<[u8; BODY_LEN]>::from(&p), [0; BODY_LEN]);
        assert_eq!(p.global_transpose, -6);
        assert_eq!(p.fine_tune, -50);
        assert_eq!(p.ctrl_pedal_gain, 1);
        assert_eq!(p.global_channel, MidiChannel::Channel(1));
    }

    /// Setting a field lands in its own bits and disturbs no other byte.
    #[test]
    fn setting_a_field_moves_only_its_own_bytes() {
        let mut p = panel(&[]);
        p.b3_tonewheel_mode = TonewheelMode::Vintage3;
        let raw = <[u8; BODY_LEN]>::from(&p);

        let moved: Vec<usize> = (0..BODY_LEN).filter(|&i| raw[i] != 0).collect();
        // Bits 109..=111 are the low three of 0x39.
        assert_eq!(moved, vec![body(0x39)], "{moved:x?}");
        assert_eq!(raw[body(0x39)], 0x03);
    }

    /// Decoding never invents a value: an unnamed pattern comes back as it went in.
    #[test]
    fn an_unrecognized_pattern_round_trips() {
        // 0b111 is not a rotary speaker type; bits 79..=81 straddle 0x35 and 0x36.
        let p = panel(&[(0x35, 0x01), (0x36, 0xc0)]);
        assert!(p.rotary_speaker_type.is_unknown());
        let raw = <[u8; BODY_LEN]>::from(&p);
        assert_eq!((raw[body(0x35)], raw[body(0x36)]), (0x01, 0xc0));
    }
}
