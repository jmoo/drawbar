//! The piano-library document.
//!
//! A `.npno` is hundreds of megabytes of encoded strokes, so nothing here reads past the
//! prefix: the name and the variant are the `Name#Variant` field at body `0x1c`, which
//! the header edits, and the strokes are never touched to draw a frame.

use std::io::Cursor;

use nord_format::formats::npno;
use nord_format::Entity;

use super::controls::Sets;

pub fn is_piano(entity: &Entity) -> bool {
    matches!(entity, Entity::Piano(_))
}

fn piano(entity: &Entity) -> Option<&npno::Piano> {
    match entity {
        Entity::Piano(piano) => Some(piano),
        _ => None,
    }
}

/// The two halves of the name field, split on its separator.
#[derive(Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    /// The text after the `#`, where the vendor records the voicing and the library's
    /// size. Empty where the field carries none.
    pub variant: String,
}

pub fn snapshot(entity: &Entity) -> Option<Result<Snapshot, String>> {
    Some(read(piano(entity)?))
}

fn read(piano: &npno::Piano) -> Result<Snapshot, String> {
    let (name, variant) = piano.name().map_err(|e| e.to_string())?;
    Ok(Snapshot { name, variant })
}

/// Apply one `path = value`. Paths are the CLI's `piano edit` flags: `name`, `variant`.
fn set(library: &mut npno::Library<'_>, path: &str, value: &str) -> Result<(), String> {
    match path {
        "name" => library.set_name(value),
        "variant" => library.set_variant(value),
        _ => return Err(format!("unknown field {path:?}")),
    }
    .map_err(|e| e.to_string())
}

/// Apply every set to a fresh decode and re-encode, the same all-or-nothing rule the
/// registry bodies follow.
pub fn apply(bytes: &[u8], sets: &Sets) -> Result<Vec<u8>, String> {
    let entity = nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let piano = piano(&entity).ok_or("not a piano library")?;
    let mut library = piano.library().map_err(|e| e.to_string())?;
    for (path, value) in sets {
        set(&mut library, path, value)?;
    }
    let edited = library.to_piano().map_err(|e| e.to_string())?;
    nord_format::to_bytes(&Entity::Piano(edited)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes that are not a piano library never reach a setter, so a name typed over
    /// the wrong document cannot half-write one.
    ///
    /// What the two setters themselves do with a name, a variant and the separator
    /// neither half may hold is `nord_format::formats::npno`'s own contract: building a
    /// library here would mean restating its body layout.
    #[test]
    fn a_body_that_is_no_piano_library_is_refused_before_anything_is_written() {
        let refused = apply(
            &crate::fields::blank::electro5_song(),
            &vec![("name".into(), "Wurly 200A".into())],
        );
        assert!(refused.is_err(), "a set list is not a piano library");
    }
}
