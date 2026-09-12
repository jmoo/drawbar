//! The registry plumbing under the document: which bodies have fields, how a change is
//! applied, and which control a field asks for.
//!
//! Field paths, values and refusals all come from `nord-format`, so a field becomes
//! editable by being declared and nothing here can fall behind the library.

use std::io::Cursor;
use std::ops::Range;

use nord_format::fields::Field;
use nord_format::{Entity, Settings, Song};

/// Every registered field's current value, for a body that has a registry.
///
/// The dispatch is `nord-format`'s own [`Entity::registry`], so a body becomes
/// editable here by being declared there — this app keeps no list to fall
/// behind.
pub fn fields_of(entity: &Entity) -> Option<Vec<Field>> {
    entity.registry().map(|body| body.fields())
}

/// Whether the body carries the generated registry, and so has a friendly view at all.
pub fn has_registry(entity: &Entity) -> bool {
    entity.registry().is_some()
}

pub fn is_electro5_settings(entity: &Entity) -> bool {
    matches!(entity, Entity::Settings(Settings::Electro5(_)))
}

/// Whether the body is an Electro 5 set list: the four programs it points at, which is
/// the whole of it.
///
/// ⚠️ Its own view rather than a field strip, because `ne5::Song` declares no registry —
/// its four slots are private fields, edited through `Song::set` in
/// `document::setlist`. The Stage 3's song is an undecoded stub with no view at all,
/// so it is not one of these: claiming it were would put an empty Basic page in front
/// of the byte record, which is all it has.
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
        let body = entity
            .registry_mut()
            .ok_or("this entity has no field registry")?;
        for (path, value) in sets {
            body.set_field(path, value).map_err(|e| e.to_string())?;
        }
    }
    let fields = entity
        .registry()
        .ok_or("this entity has no field registry")?
        .fields();
    let out = nord_format::to_bytes(&entity).map_err(|e| e.to_string())?;
    Ok((fields, out))
}

/// Every registered field of a body, decoded straight from bytes.
///
/// The document draws the working copy and measures it against the bytes it was last
/// saved as, so both decodes come through here.
pub fn decoded(bytes: &[u8]) -> Option<Vec<Field>> {
    let entity = nord_format::from_stream(&mut Cursor::new(bytes)).ok()?;
    fields_of(&entity)
}

/// The registry paths the two sets of bytes spell differently, in registry order.
///
/// ⚠️ An edit lands on the working copy in the frame it is made, so a pending change is
/// this document against its own saved bytes — not `raw != bits` inside one decode,
/// which an applied edit has already settled. Bytes that do not decode name nothing
/// rather than claiming every field moved.
pub fn changed(saved: &[u8], current: &[u8]) -> Vec<String> {
    let (Some(before), Some(after)) = (decoded(saved), decoded(current)) else {
        return Vec::new();
    };
    before
        .iter()
        .zip(&after)
        .filter(|(before, after)| before.path == after.path && before.value != after.value)
        .map(|(_, after)| after.path.clone())
        .collect()
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
    use crate::drawbar_widget;
    use nord_format::formats::ne5;
    use nord_format::Program;

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

    /// Pending is the working copy against the saved bytes, and it names the edited
    /// paths and nothing else — the checksum that moved with them is no field.
    #[test]
    fn changed_names_the_edited_paths_and_nothing_else() {
        let bytes = program();
        let (_, once) = apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        assert_eq!(changed(&bytes, &once), ["center_panel.gain"]);

        let (_, twice) = apply(&once, &[("center_panel.split".into(), "true".into())]).unwrap();
        assert_eq!(
            changed(&bytes, &twice),
            ["center_panel.split", "center_panel.gain"],
            "registry order, not the order the edits were made in",
        );
        assert!(changed(&bytes, &bytes).is_empty());
        // Bytes that do not decode name nothing rather than claiming every field moved.
        assert!(changed(b"not a nord file", &bytes).is_empty());
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
