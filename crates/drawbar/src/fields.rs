//! The registry plumbing under the document: which bodies have fields, how a change is
//! applied, and what changed.
//!
//! Field paths, values, and refusals all come from `nord-format`, so declaring a field
//! there makes it editable here.

use std::io::Cursor;
use std::ops::Range;

use nord_format::fields::Field;
use nord_format::{Entity, Settings, Song};

/// Every registered field's current value, for a body that has a registry.
///
/// Dispatches through `nord-format`'s [`Entity::registry`], so this app keeps no list
/// of bodies.
pub fn fields_of(entity: &Entity) -> Option<Vec<Field>> {
    entity.registry().map(|body| body.fields())
}

/// Whether the body has the generated registry, and so a friendly view.
pub fn has_registry(entity: &Entity) -> bool {
    entity.registry().is_some()
}

pub fn is_electro5_settings(entity: &Entity) -> bool {
    matches!(entity, Entity::Settings(Settings::Electro5(_)))
}

/// Whether the body is an Electro 5 set list, which holds only the four programs it
/// points at.
///
/// ⚠️ It gets its own view instead of a field strip, because `ne5::Song` declares no
/// registry: its four slots are private fields, edited through `Song::set` in
/// `document::setlist`. A Stage 3 song is an undecoded stub with no view, so it is not a
/// set list here; claiming it were would put an empty Basic page in front of its byte
/// record.
pub fn is_set_list(entity: &Entity) -> bool {
    matches!(entity, Entity::Song(Song::Electro5(_)))
}

/// Apply every set to a fresh decode of `bytes` and re-encode.
///
/// Every set is applied before anything is encoded, so a value the field cannot hold
/// leaves no half-edited body. Applying the sets together lets a control that owns two
/// fields, such as the transpose pair, move both or neither.
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
/// The document draws the working copy and compares it with its last saved bytes; both
/// decodes come through here.
pub fn decoded(bytes: &[u8]) -> Option<Vec<Field>> {
    let entity = nord_format::from_stream(&mut Cursor::new(bytes)).ok()?;
    fields_of(&entity)
}

/// The registry paths the two sets of bytes spell differently, in registry order.
///
/// ⚠️ An edit reaches the working copy in the frame it is made, so a pending change
/// shows only when this document is compared with its saved bytes. Comparing `raw` with
/// `bits` inside one decode would miss it, because an applied edit has already settled
/// them. Bytes that do not decode name no paths.
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

/// One byte that changed.
pub struct DiffRow {
    pub at: usize,
    pub before: u8,
    pub after: u8,
    /// `  (body crc32)` when the byte is a checksum, not an edit.
    pub note: &'static str,
}

/// Where a CBIN file keeps its checksum and what to call it, or `None` for bytes that
/// are not a CBIN file.
///
/// ⚠️ The two generations store it in different places. In a type-0 file `0x18` is body
/// data, and annotating it as the type-1 crc32 would label a real edit as a checksum.
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

/// The bytes that changed.
///
/// The checksum changes with any body change; those rows are annotated so they do not
/// read as a second, unexplained edit. Bytes of different lengths cannot be paired, so
/// that returns no rows.
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

/// A blank Stage 3 song for tests.
///
/// ⚠️ Not a [`Fresh`]: the body is an undecoded stub, so the New menu does not offer one
/// and [`Fresh::bytes`] cannot build one. Every other blank body a test opens is a
/// `Fresh`.
///
/// [`Fresh`]: crate::workspace::Fresh
/// [`Fresh::bytes`]: crate::workspace::Fresh::bytes
#[cfg(test)]
pub mod blank {
    use nord_format::cbin::{Cbin, Header, RawBody};
    use nord_format::formats::ns3;
    use nord_format::{Entity, Song};

    pub fn stage3_song() -> Vec<u8> {
        let file = Cbin {
            header: Header::new(ns3::song::FORMAT, (0, 0), 0),
            body: RawBody(vec![0u8; ns3::song::BODY_LEN]),
        };
        nord_format::to_bytes(&Entity::Song(Song::Stage3(file))).expect("a stub encodes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawbar_widget;
    use crate::workspace::Fresh;
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

    /// The refusal names the accepted range.
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

    /// Every body the library decodes into fields reads and writes here.
    #[test]
    fn every_registry_backed_body_reads_and_writes() {
        for bytes in [
            Fresh::Stage2Program.bytes().unwrap(),
            Fresh::Stage3Synth.bytes().unwrap(),
            Fresh::Stage4Organ.bytes().unwrap(),
            Fresh::Stage4Piano.bytes().unwrap(),
            Fresh::Stage4Program.bytes().unwrap(),
            Fresh::Stage4Synth.bytes().unwrap(),
        ] {
            let (fields, out) = apply(&bytes, &[]).expect("a blank body round-trips");
            assert!(!fields.is_empty());
            assert_eq!(out, bytes, "an empty set changes nothing");
        }
    }

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
        assert!(changed(b"not a nord file", &bytes).is_empty());
    }

    #[test]
    fn a_register_round_trips_through_the_widgets_spelling() {
        let bytes = program();
        let (fields, _) = apply(&bytes, &[]).unwrap();
        let register = fields
            .iter()
            .find(|f| f.path == "organ_panel.vox_preset1_drawbars")
            .unwrap();
        let bits = drawbar_widget::parse(&register.value).unwrap();
        let parked = drawbar_widget::written(bits, drawbar_widget::bars(bits))
            .expect("every bar is where it was stored");
        assert_eq!(drawbar_widget::spell(parked), register.value);

        let (after, _) = apply(&bytes, &[(register.path.clone(), "0x888800000".into())]).unwrap();
        let edited = after.iter().find(|f| f.path == register.path).unwrap();
        assert_eq!(
            drawbar_widget::bars(drawbar_widget::parse(&edited.value).unwrap()),
            [8, 8, 8, 8, 0, 0, 0, 0, 0]
        );
    }
}
