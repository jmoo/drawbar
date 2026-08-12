//! The registry plumbing under the document: which bodies have fields, how a change is
//! applied, and which control a field asks for.
//!
//! Field paths, values and refusals all come from `nord-format`, so a field becomes
//! editable by being declared and nothing here can fall behind the library.

use std::io::Cursor;
use std::ops::Range;

use nord_format::cbin::Cbin;
use nord_format::fields::{ControlKind, Field, FieldError};
use nord_format::formats::{ne5, ns2, ns3, ns4};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Settings, Song, Synth};

use crate::drawbar_widget;

/// The bodies the document drives: one vocabulary — `path = value` — over each.
trait Editable {
    fn fields(&self) -> Vec<Field>;
    fn set_field(&mut self, path: &str, value: &str) -> Result<(), FieldError>;
}

macro_rules! editable {
    ($body:ty) => {
        impl Editable for Cbin<$body> {
            fn fields(&self) -> Vec<Field> {
                self.body.fields()
            }
            fn set_field(&mut self, path: &str, value: &str) -> Result<(), FieldError> {
                self.body.set_field(path, value)
            }
        }
    };
}

editable!(ne5::Program);
editable!(ne5::Settings);
editable!(ns2::Program);
editable!(ns3::Program);
editable!(ns3::SynthPreset);
editable!(ns4::Program);
editable!(ns4::organ_preset::OrganPreset);
editable!(ns4::piano_preset::PianoPreset);
editable!(ns4::synth::SynthPreset);

/// Every entity whose body carries the generated registry, read in whichever direction
/// the caller asked for.
///
/// One list serving [`body`] and [`body_mut`] both: a body that lists its fields and
/// refuses to set them, or the reverse, cannot be written here. The live buffer is the
/// program body under another tag, so the two share an arm.
macro_rules! registry {
    ($entity:expr, $($reference:tt)*) => {
        match $entity {
            Entity::Live(Live::Electro5(f)) | Entity::Program(Program::Electro5(f)) => {
                Some(f as $($reference)* dyn Editable)
            }
            Entity::Live(Live::Stage2(f)) | Entity::Program(Program::Stage2(f)) => {
                Some(f as $($reference)* dyn Editable)
            }
            Entity::Live(Live::Stage3(f)) | Entity::Program(Program::Stage3(f)) => {
                Some(f as $($reference)* dyn Editable)
            }
            Entity::Live(Live::Stage4(f)) | Entity::Program(Program::Stage4(f)) => {
                Some(f as $($reference)* dyn Editable)
            }
            Entity::OrganPreset(OrganPreset::Stage4(f)) => Some(f as $($reference)* dyn Editable),
            Entity::PianoPreset(PianoPreset::Stage4(f)) => Some(f as $($reference)* dyn Editable),
            Entity::Settings(Settings::Electro5(f)) => Some(f as $($reference)* dyn Editable),
            Entity::Synth(Synth::Stage3(f)) => Some(f as $($reference)* dyn Editable),
            Entity::Synth(Synth::Stage4(f)) => Some(f as $($reference)* dyn Editable),
            _ => None,
        }
    };
}

fn body(entity: &Entity) -> Option<&dyn Editable> {
    registry!(entity, &)
}

fn body_mut(entity: &mut Entity) -> Option<&mut dyn Editable> {
    registry!(entity, &mut)
}

/// Every registered field's current value, for a body that has a registry.
pub fn fields_of(entity: &Entity) -> Option<Vec<Field>> {
    body(entity).map(|body| body.fields())
}

/// Whether the body carries the generated registry, and so has a friendly view at all.
pub fn has_registry(entity: &Entity) -> bool {
    body(entity).is_some()
}

/// Whether the body is an Electro 5 panel — the one the document knows section by
/// section. Everything else with a registry falls back to a plain field list.
pub fn is_electro5_panel(entity: &Entity) -> bool {
    matches!(
        entity,
        Entity::Program(Program::Electro5(_)) | Entity::Live(Live::Electro5(_))
    )
}

pub fn is_electro5_settings(entity: &Entity) -> bool {
    matches!(entity, Entity::Settings(Settings::Electro5(_)))
}

/// Whether the body is an Electro 5 set list: the four programs it points at, which is
/// the whole of it.
///
/// ⚠️ Its own view rather than a field strip, because `ne5::Song` lists nothing — its four
/// slots are private fields and no generated accessor reaches them. The Stage 3's song is
/// an undecoded stub with no view at all, so it is not one of these: claiming it were
/// would put an empty Basic page in front of the byte record, which is all it has.
pub fn is_set_list(entity: &Entity) -> bool {
    matches!(entity, Entity::Song(Song::Electro5(_)))
}

/// Apply every set to a fresh decode of `bytes` and re-encode.
///
/// Every change lands before anything is encoded, so a value the field cannot hold
/// cannot leave a half-edited body behind. Applying the sets together is what lets a
/// control that owns two fields — the transpose pair — move both or neither.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<(Vec<Field>, Vec<u8>), String> {
    let mut entity =
        nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    {
        let body = body_mut(&mut entity).ok_or("this entity has no field registry")?;
        for (path, value) in sets {
            body.set_field(path, value).map_err(|e| e.to_string())?;
        }
    }
    let fields = body(&entity)
        .ok_or("this entity has no field registry")?
        .fields();
    let out = nord_format::to_bytes(&entity).map_err(|e| e.to_string())?;
    Ok((fields, out))
}

/// Longest legal-value list that stays a menu. Past it, a run of consecutive integers is
/// a slider instead.
const CHOICE_MAX: usize = 24;

/// Which control a field asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    Toggle,
    Choice,
    Number {
        min: i64,
        max: i64,
    },
    /// One organ drawbar, where the file gives each bar its own nibble.
    Bar,
    /// A whole nine-bar organ register in one field.
    Register,
    /// No control: the stored value is the only reading this app has of it.
    Stored,
}

impl Control {
    /// The field's own [`ControlKind`] decides, and the legal values fill in only what a
    /// kind leaves open — a knob's travel, or whether a two-state field spells its states
    /// `true`/`false` or names them.
    pub fn of(field: &Field, legal: &[String]) -> Control {
        match field.spec.control {
            // ⚠️ `Drawbar` is the *bar*, not the registration: the Electro 5 packs all
            // nine into one field and the Stages give each bar its own nibble, and both
            // carry this one kind. The width is what tells them apart.
            ControlKind::Drawbar if field.spec.width == drawbar_widget::REGISTER_BITS => {
                Control::Register
            }
            ControlKind::Drawbar if field.spec.width == drawbar_widget::BAR_BITS => Control::Bar,
            // A step grid and a library id both need a control this app does not have: a
            // grid to draw, and the instrument's own catalogue to pick a name out of.
            ControlKind::Pattern | ControlKind::Reference => Control::Stored,
            ControlKind::Toggle if legal == ["false", "true"] => Control::Toggle,
            // A knob says so, so its values are travel however few of them there are.
            ControlKind::Bipolar(_)
            | ControlKind::Knob(_)
            | ControlKind::Morph
            | ControlKind::Shift(_) => turned(legal),
            // ⚠️ `Number` is the kind a field has when nothing has been claimed about it,
            // so it is not a knob — it is an integer, and a short run of those is a
            // four-position switch as often as it is travel. Only a run too long to read
            // as a list turns.
            _ => picked(legal),
        }
    }
}

/// A knob's control: the run its values cover, or a menu where they are named rather
/// than counted. A field too wide to enumerate lists nothing and has neither.
fn turned(legal: &[String]) -> Control {
    match contiguous(legal) {
        // A run of one value has no travel, so there is nothing to turn.
        Some((min, max)) if min < max => Control::Number { min, max },
        _ if legal.is_empty() => Control::Stored,
        _ => Control::Choice,
    }
}

/// A picker's control: its values, which are the positions.
///
/// ⚠️ Past [`CHOICE_MAX`] the list is longer than a menu can be read at — a `WideSelector`
/// over a sample library offers a thousand bare indices — so a long one turns instead.
fn picked(legal: &[String]) -> Control {
    match (1..=CHOICE_MAX).contains(&legal.len()) {
        true => Control::Choice,
        false => turned(legal),
    }
}

/// The range a legal-value list covers, when every value is an integer and none is
/// missing. A gapped set stays a menu — a slider over it would stop on values the field
/// refuses.
fn contiguous(legal: &[String]) -> Option<(i64, i64)> {
    let mut values = Vec::with_capacity(legal.len());
    for value in legal {
        values.push(value.trim_start_matches('+').parse::<i64>().ok()?);
    }
    let min = *values.iter().min()?;
    let max = *values.iter().max()?;
    (max.checked_sub(min)? + 1 == values.len() as i64).then_some((min, max))
}

/// One byte that moved.
pub struct DiffRow {
    pub at: usize,
    pub before: u8,
    pub after: u8,
    /// `  (body crc32)` where the byte is bookkeeping rather than an edit.
    pub note: &'static str,
}

/// Where a CBIN file keeps its checksum and what to call it, or `None` for bytes that
/// are not a CBIN file.
///
/// ⚠️ The two generations put it in different places, and a type-0 file's `0x18` is body
/// data — annotating it as the type-1 crc32 would label a real edit as bookkeeping.
fn checksum_bytes(file: &[u8]) -> Option<(Range<usize>, &'static str)> {
    if file.len() < 8 || &file[0..4] != nord_format::cbin::MAGIC {
        return None;
    }
    match u32::from_le_bytes(file[4..8].try_into().ok()?) {
        0 => Some((file.len() - 2..file.len(), "  (file crc16)")),
        1 => Some((0x18..0x1c, "  (body crc32)")),
        _ => None,
    }
}

/// The bytes that moved.
///
/// The checksum moves with any body change; those rows are annotated so they do not read
/// as a second unexplained edit. A length change is not a diff at all — nothing here can
/// pair the bytes up — so it comes back empty.
pub fn byte_diff(before: &[u8], after: &[u8]) -> Vec<DiffRow> {
    if before.len() != after.len() {
        return Vec::new();
    }
    let checksum = checksum_bytes(after);
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (b, a))| b != a)
        .map(|(at, (&b, &a))| DiffRow {
            at,
            before: b,
            after: a,
            note: match &checksum {
                Some((range, label)) if range.contains(&at) => label,
                _ => "",
            },
        })
        .collect()
}

/// Blank files of the formats the workspace has no fresh default for, so a test can open
/// a document of one.
///
/// A zeroed body is a legal one: every field's type decodes the whole of its slot, and
/// the version is the newest the decode is validated against.
#[cfg(test)]
pub mod blank {
    use nord_format::cbin::{Cbin, Header, RawBody};
    use nord_format::formats::{ne5, ns2, ns3, ns4};
    use nord_format::{Entity, OrganPreset, PianoPreset, Program, Song, Synth};

    /// A Stage 3 song, which decodes no further than its container.
    pub fn stage3_song() -> Vec<u8> {
        let file = Cbin {
            header: Header::new(ns3::song::FORMAT, (0, 0), 0),
            body: RawBody(vec![0u8; ns3::song::BODY_LEN as usize]),
        };
        nord_format::to_bytes(&Entity::Song(Song::Stage3(file))).expect("a stub encodes")
    }

    /// An Electro 5 set list pointing at the first four programs.
    pub fn electro5_song() -> Vec<u8> {
        let at =
            |slot: u16| -> ne5::program::Location { (0, slot).try_into().expect("a program slot") };
        let here: ne5::song::Location = (0, 0).try_into().expect("a song slot");
        let song = ne5::song::new(
            here,
            ne5::song::DEFAULT_VERSION,
            [at(0), at(1), at(2), at(3)],
        );
        nord_format::to_bytes(&Entity::Song(Song::Electro5(song))).expect("a song encodes")
    }

    macro_rules! blank {
        ($name:ident, $body:ty, $len:expr, $format:expr, $versions:expr, $wrap:expr) => {
            pub fn $name() -> Vec<u8> {
                let body = <$body>::try_from([0u8; $len]).expect("a zeroed body decodes");
                let version = *$versions.last().expect("a format knows a version");
                let file = Cbin {
                    header: Header::new($format, (0, 0), version),
                    body,
                };
                nord_format::to_bytes(&$wrap(file)).expect("a blank file encodes")
            }
        };
    }

    blank!(
        stage2_program,
        ns2::Program,
        ns2::program::BODY_LEN,
        ns2::program::FORMAT,
        ns2::program::KNOWN_VERSIONS,
        |f| Entity::Program(Program::Stage2(f))
    );
    blank!(
        stage3_synth,
        ns3::SynthPreset,
        ns3::synth::BODY_LEN,
        ns3::synth::FORMAT,
        ns3::synth::KNOWN_VERSIONS,
        |f| Entity::Synth(Synth::Stage3(f))
    );
    blank!(
        stage4_program,
        ns4::Program,
        ns4::program::BODY_LEN,
        ns4::program::FORMAT,
        ns4::program::KNOWN_VERSIONS,
        |f| Entity::Program(Program::Stage4(f))
    );
    blank!(
        stage4_organ_preset,
        ns4::organ_preset::OrganPreset,
        ns4::organ_preset::BODY_LEN,
        ns4::organ_preset::FORMAT,
        ns4::organ_preset::KNOWN_VERSIONS,
        |f| Entity::OrganPreset(OrganPreset::Stage4(f))
    );
    blank!(
        stage4_piano_preset,
        ns4::piano_preset::PianoPreset,
        ns4::piano_preset::BODY_LEN,
        ns4::piano_preset::FORMAT,
        ns4::piano_preset::KNOWN_VERSIONS,
        |f| Entity::PianoPreset(PianoPreset::Stage4(f))
    );
    blank!(
        stage4_synth,
        ns4::synth::SynthPreset,
        ns4::synth::BODY_LEN,
        ns4::synth::FORMAT,
        ns4::synth::KNOWN_VERSIONS,
        |f| Entity::Synth(Synth::Stage4(f))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program() -> Vec<u8> {
        let entity = Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().unwrap(),
        )));
        nord_format::to_bytes(&entity).unwrap()
    }

    #[test]
    fn a_set_changes_the_field_it_names_and_nothing_else() {
        let bytes = program();
        let (before, _) = apply(&bytes, &[]).unwrap();
        let (after, edited) = apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();

        let moved: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|(b, a)| b.display != a.display)
            .map(|(_, a)| a.path.as_str())
            .collect();
        assert_eq!(moved, ["center_panel.gain"]);
        assert_eq!(edited.len(), bytes.len());
    }

    /// A value the field cannot hold is refused before anything is encoded, and the
    /// message names what it does accept.
    #[test]
    fn an_out_of_range_value_is_refused_by_the_library() {
        let err = apply(&program(), &[("center_panel.gain".into(), "200".into())])
            .err()
            .expect("200 is not a gain");
        assert!(err.contains("not a value of gain"), "{err}");
        assert!(err.contains("0 .. 127"), "{err}");
    }

    /// One refused half of a two-field control must not leave the other half applied.
    #[test]
    fn a_refusal_anywhere_in_a_batch_applies_none_of_it() {
        let bytes = program();
        let sets = [
            ("center_panel.transpose_enabled".into(), "true".into()),
            ("center_panel.transpose".into(), "99".into()),
        ];
        assert!(apply(&bytes, &sets).is_err());
        let (fields, _) = apply(&bytes, &[]).unwrap();
        let enabled = fields
            .iter()
            .find(|f| f.path == "center_panel.transpose_enabled")
            .unwrap();
        assert_eq!(enabled.value, "false");
    }

    /// The crc moves with any body change; the row has to say so, or it reads as a
    /// second edit nobody made.
    #[test]
    fn the_checksum_bytes_are_annotated_as_bookkeeping() {
        let bytes = program();
        let (_, edited) = apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        let diff = byte_diff(&bytes, &edited);
        assert!(!diff.is_empty());
        // A fresh program is type-1, so the crc32 sits at 0x18..0x1c.
        let annotated: Vec<usize> = diff
            .iter()
            .filter(|row| row.note.contains("crc32"))
            .map(|row| row.at)
            .collect();
        assert!(annotated.iter().all(|at| (0x18..0x1c).contains(at)));
        assert!(!annotated.is_empty(), "the crc32 must have moved");
        assert!(
            diff.iter().any(|row| row.note.is_empty()),
            "the edit itself must show as an unannotated byte",
        );
    }

    /// A gapped legal set must stay a menu: a slider over it would stop on values the
    /// field refuses.
    #[test]
    fn only_a_gapless_run_of_integers_becomes_a_slider() {
        let full: Vec<String> = (0..128).map(|n| n.to_string()).collect();
        assert_eq!(contiguous(&full), Some((0, 127)));

        let gapped: Vec<String> = vec!["0".into(), "1".into(), "9".into()];
        assert_eq!(contiguous(&gapped), None);

        let named: Vec<String> = vec!["Organ".into(), "Piano".into()];
        assert_eq!(contiguous(&named), None);
    }

    /// What a field is drawn as comes off its own declared kind, so a body this app has
    /// never heard of arrives with its controls already chosen.
    fn control_of(fields: &[Field], path: &str) -> Control {
        let field = fields
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("{path} is declared"));
        Control::of(field, &(field.spec.legal)())
    }

    /// The nine-nibble register is the drawbar widget's, and nothing else is.
    #[test]
    fn a_register_field_picks_the_drawbar_control() {
        let bytes = program();
        let (fields, _) = apply(&bytes, &[]).unwrap();
        assert_eq!(
            control_of(&fields, "organ_panel.b3_preset1_drawbars"),
            Control::Register
        );
        assert_eq!(
            control_of(&fields, "center_panel.gain"),
            Control::Number { min: 0, max: 127 }
        );
    }

    /// ⚠️ Both spellings of a drawbar carry the one kind: the Electro 5 packs a whole
    /// registration into one field and the Stage 4 gives each bar its own nibble. Reading
    /// the width wrong puts nine bars where one belongs.
    #[test]
    fn a_drawbar_is_a_register_or_a_bar_by_its_width() {
        let (stage4, _) = apply(&blank::stage4_program(), &[]).unwrap();
        assert_eq!(control_of(&stage4, "organ_a.drawbar_1"), Control::Bar);

        let (electro5, _) = apply(&program(), &[]).unwrap();
        assert_eq!(
            control_of(&electro5, "organ_panel.vox_preset1_drawbars"),
            Control::Register
        );
    }

    /// A selector is its positions, a two-state field is a lamp, and a knob is travel —
    /// each because the field says so, not because this app knows the path.
    #[test]
    fn the_declared_kind_picks_the_control() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        assert_eq!(control_of(&fields, "split_enabled"), Control::Toggle);
        assert_eq!(control_of(&fields, "piano_a.piano_type"), Control::Choice);
        assert_eq!(
            control_of(&fields, "organ_a_volume"),
            Control::Number { min: 0, max: 127 }
        );
        // A library id names something only the instrument holds, so there is no control
        // for it here.
        assert_eq!(control_of(&fields, "piano_a.model_id"), Control::Stored);
        // A picker over four thousand bare indices is past reading as a list.
        assert_eq!(
            control_of(&fields, "synth_a_performance.sample_slot"),
            Control::Number { min: 0, max: 4095 }
        );
    }

    /// Every body the library decodes into fields is editable here, in both directions.
    #[test]
    fn every_registry_backed_body_reads_and_writes() {
        for bytes in [
            blank::stage2_program(),
            blank::stage3_synth(),
            blank::stage4_organ_preset(),
            blank::stage4_piano_preset(),
            blank::stage4_program(),
            blank::stage4_synth(),
        ] {
            let (fields, out) = apply(&bytes, &[]).expect("a blank body round-trips");
            assert!(!fields.is_empty());
            assert_eq!(out, bytes, "an empty set changes nothing");
        }
    }

    /// A drawbar pulled in the widget writes back exactly what the field reads out, so
    /// parking a bar where it already was is not a change.
    #[test]
    fn a_register_round_trips_through_the_widgets_spelling() {
        let bytes = program();
        let (fields, _) = apply(&bytes, &[]).unwrap();
        let register = fields
            .iter()
            .find(|f| f.path == "organ_panel.vox_preset1_drawbars")
            .unwrap();
        let bits = drawbar_widget::parse(&register.value).unwrap();
        let spelled = drawbar_widget::spell(drawbar_widget::bits(drawbar_widget::bars(bits)));
        assert_eq!(spelled, register.value);

        let (after, _) = apply(&bytes, &[(register.path.clone(), "0x888800000".into())]).unwrap();
        let edited = after.iter().find(|f| f.path == register.path).unwrap();
        assert_eq!(
            drawbar_widget::bars(drawbar_widget::parse(&edited.value).unwrap()),
            [8, 8, 8, 8, 0, 0, 0, 0, 0]
        );
    }
}
