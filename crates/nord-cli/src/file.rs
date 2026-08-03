//! The read-only verbs (`get`, `info`, `deps`) pointed at a file instead of a slot.
//!
//! Same verbs, no instrument: the object is the file's bytes. What a file does not
//! carry — the slot name, the names behind dependency ids — is reported as living on
//! the instrument rather than guessed at.

use std::path::{Path, PathBuf};

use nord_format::{common, electro5, Entity, Live, Program};
use nord_usb::ObjectClass;

use crate::slot::shown;
use crate::ui::Ui;

/// The format tag a class's files carry, or `None` for a class with no known tag.
pub(crate) fn tag(class: ObjectClass) -> Option<&'static str> {
    match class {
        ObjectClass::Piano => Some(common::piano::FORMAT),
        ObjectClass::Sample => Some(common::sample::FORMAT),
        ObjectClass::Program => Some(electro5::program::FORMAT),
        ObjectClass::SetList => Some(electro5::song::FORMAT),
        ObjectClass::Live => Some(electro5::live::FORMAT),
        ObjectClass::Settings => Some(electro5::settings::FORMAT),
        ObjectClass::Unknown(_) => None,
    }
}

/// The noun that reads a tag's files, for steering a mismatch to the right command.
fn noun(format: &str) -> Option<&'static str> {
    match format {
        "ne5p" => Some("nord program"),
        "ne5t" => Some("nord setlist"),
        "ne5l" => Some("nord live"),
        "nsmp" => Some("nord sample"),
        _ => None,
    }
}

/// Refuse a file whose format tag belongs to another class's noun: summarising a set
/// list under `nord program` would mislabel everything it prints.
fn check(path: &Path, format: &str, class: ObjectClass) -> Result<(), String> {
    match tag(class) {
        Some(want) if want != format => {
            let steer = match noun(format) {
                Some(n) => format!(" — try `{n}`"),
                None => String::new(),
            };
            Err(format!(
                "{}: a {format} file, but this command reads {} ({want}){steer}",
                path.display(),
                class.label(),
            ))
        }
        _ => Ok(()),
    }
}

/// The header fields `envelope::unwrap` does not hand back: the schema version at 0x14
/// and the stored CRC-32 at 0x18. `unwrap` has already checked the length and checksum.
fn version_and_crc(file: &[u8]) -> (u32, u32) {
    let version = u32::from_le_bytes(file[0x14..0x18].try_into().unwrap());
    let crc = u32::from_le_bytes(file[0x18..0x1c].try_into().unwrap());
    (version, crc)
}

/// `get` on a file: print the summary, or with `--body` extract the wire body.
pub fn get(
    ui: &Ui,
    path: &Path,
    out: Option<PathBuf>,
    class: ObjectClass,
    body: bool,
) -> Result<(), String> {
    let file = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (format, _, wire_body) =
        nord_usb::envelope::unwrap(&file).map_err(|e| format!("{}: {e}", path.display()))?;
    check(path, &format, class)?;

    match (body, out) {
        (true, Some(out)) => {
            std::fs::write(&out, wire_body).map_err(|e| format!("{}: {e}", out.display()))?;
            ui.note(format!(
                "unwrapped the {format} body of {} -> {} ({} bytes)",
                path.display(),
                out.display(),
                wire_body.len(),
            ));
            Ok(())
        }
        (true, None) => Err("--body writes a file; give -o a path".into()),
        (false, Some(_)) => Err(format!(
            "{} is already a file; -o has nothing to save (--body extracts the wire body)",
            path.display()
        )),
        (false, None) => {
            let entity = nord_format::from_stream(&mut std::io::Cursor::new(&file))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            crate::summary::print(ui, &entity);
            Ok(())
        }
    }
}

/// `info` on a file: the CBIN header, which is exactly what the wire never transmits.
pub fn info(ui: &Ui, path: &Path, class: ObjectClass) -> Result<(), String> {
    let file = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (format, at, body) =
        nord_usb::envelope::unwrap(&file).map_err(|e| format!("{}: {e}", path.display()))?;
    check(path, &format, class)?;
    let (version, crc) = version_and_crc(&file);

    let row = |label: &str, value: String| {
        ui.out(format!("  {}{value}", ui.dim(format!("{label:<11}"))));
    };
    // Library files (samples) carry `0xffff:0xffff` where slot files keep bank/slot —
    // a library object has no slot until an instrument gives it one.
    if (at.bank, at.slot) == (0xffff, 0xffff) {
        row(
            "location:",
            format!("none {}", ui.dim("(a library file, not a slot save)")),
        );
    } else {
        row(
            "location:",
            format!(
                "{} {}",
                shown(at),
                ui.dim("(the slot the file was saved from)")
            ),
        );
    }
    row("name:", format!("none {}", ui.dim("(files store no name)")));
    row("format:", format);
    row("version:", version.to_string());
    row(
        "body:",
        format!(
            "{} bytes{}",
            crate::device::grouped(body.len() as u32),
            match crate::device::human_size(body.len() as u32) {
                Some(h) => format!("  {}", ui.dim(format!("({h})"))),
                None => String::new(),
            }
        ),
    );
    row("crc32:", format!("{crc:#010x}"));
    Ok(())
}

/// `deps` on a file: the library ids a program body stores.
///
/// Only ids — the names the wire's `DEPENDENCIES` reply attaches live on the
/// instrument, so the slot form of the verb is what resolves them.
pub fn deps(ui: &Ui, path: &Path, class: ObjectClass) -> Result<(), String> {
    let entity = nord_format::from_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let format = entity_tag(&entity);
    check(path, format, class)?;

    // The two bodies are byte-identical but sit in different slot spaces, so the ids
    // are pulled out per variant rather than through one schema reference.
    let (piano, sample) = match &entity {
        Entity::Program(Program::Electro5(p)) => {
            (p.schema.piano_panel.id, p.schema.sample_panel.id)
        }
        Entity::Live(Live::Electro5(l)) => (l.schema.piano_panel.id, l.schema.sample_panel.id),
        Entity::Song(_) => {
            return Err("a set list names program slots, not library objects; \
                 `nord setlist deps BANK:SLOT` asks the instrument, which resolves them"
                .into())
        }
        _ => {
            return Err(format!(
                "{}: a {format} file carries no dependency ids",
                path.display()
            ))
        }
    };

    let refs: Vec<(ObjectClass, u32)> =
        [(ObjectClass::Piano, piano), (ObjectClass::Sample, sample)]
            .into_iter()
            .filter(|&(_, id)| id != 0)
            .collect();

    if refs.is_empty() {
        ui.note(format!("{} references no library objects", path.display()));
        return Ok(());
    }
    ui.out(ui.dim(format!("{:<8} id", "class")));
    for (class, id) in &refs {
        ui.out(format!("{:<8} {id:08x}", class.label()));
    }
    ui.note(ui.dim("(ids only — the names live on the instrument; `deps BANK:SLOT` shows them)"));
    Ok(())
}

/// The format tag a decoded entity would carry on disk.
pub(crate) fn entity_tag(entity: &Entity) -> &'static str {
    match entity {
        Entity::Program(Program::Electro5(_)) => electro5::program::FORMAT,
        Entity::Live(Live::Electro5(_)) => electro5::live::FORMAT,
        Entity::Song(nord_format::Song::Electro5(_)) => electro5::song::FORMAT,
        Entity::Settings(nord_format::Settings::Electro5(_)) => electro5::settings::FORMAT,
        Entity::Piano(_) => common::piano::FORMAT,
        Entity::Sample(_) => common::sample::FORMAT,
        Entity::Bundle(_) => "zip",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_usb::wire::Location;

    /// A wrapped file must give back the version `wrap` stamped into it, and a CRC that
    /// tracks the body — which pins both reads to the right header offsets.
    #[test]
    fn the_header_fields_come_back_out_of_a_wrapped_file() {
        let at = Location::from_user(7, 4);
        let a = nord_usb::envelope::wrap("ne5p", at, 4, &[0u8; 8]).unwrap();
        let b = nord_usb::envelope::wrap("ne5p", at, 4, &[1u8; 8]).unwrap();
        let c = nord_usb::envelope::wrap("ne5t", at, 1, &[0u8; 8]).unwrap();
        assert_eq!(version_and_crc(&a).0, 4);
        assert_eq!(version_and_crc(&c).0, 1);
        assert_ne!(version_and_crc(&a).1, version_and_crc(&b).1);
    }

    /// The mismatch error must steer to the noun that does read the file.
    #[test]
    fn a_wrong_class_is_steered_to_the_right_noun() {
        let err = check(Path::new("x.ne5t"), "ne5t", ObjectClass::Program).unwrap_err();
        assert!(err.contains("nord setlist"), "{err}");
        assert!(check(Path::new("x.ne5t"), "ne5t", ObjectClass::SetList).is_ok());
        // A class with no known tag cannot be checked, so nothing is refused.
        assert!(check(Path::new("x.bin"), "abcd", ObjectClass::Unknown(9)).is_ok());
    }
}
