//! The read-only verbs (`get`, `info`, `deps`) pointed at a file instead of a slot.
//!
//! Same verbs, no instrument: the object is the file's bytes. What a file does not
//! carry — the slot name, the names behind dependency ids — is reported as living on
//! the instrument rather than guessed at.

use std::path::{Path, PathBuf};

use nord_format::cbin::{Generation, Header};
use nord_format::formats::{ne5, npno, nsmp};
use nord_format::{Entity, Live, Program};
use nord_usb::ObjectClass;

use crate::slot::shown;
use crate::ui::Ui;

/// The format tag a class's files carry, or `None` for a class with no known tag.
pub(crate) fn tag(class: ObjectClass) -> Option<&'static str> {
    match class {
        ObjectClass::Piano => Some(npno::FORMAT),
        ObjectClass::Sample => Some(nsmp::FORMAT),
        ObjectClass::Program => Some(ne5::program::FORMAT),
        ObjectClass::SetList => Some(ne5::song::FORMAT),
        ObjectClass::Live => Some(ne5::live::FORMAT),
        ObjectClass::Settings => Some(ne5::settings::FORMAT),
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

/// The stored checksum, with the label its generation spells it under.
///
/// The one header fact the parsed [`Header`] does not carry: a type-1 file holds a
/// crc32 over the body at 0x18, a type-0 file a crc16 over the whole file in its last
/// two bytes, so the value is read from the bytes either way. `unwrap` verified it, so
/// this reports what it checked.
fn crc(header: &Header, bytes: &[u8]) -> (&'static str, String) {
    match header.generation {
        Generation::V0 => {
            let crc = u16::from_le_bytes(bytes[bytes.len() - 2..].try_into().unwrap());
            ("crc16:", format!("{crc:#06x}"))
        }
        Generation::V1 => {
            let crc = u32::from_le_bytes(bytes[0x18..0x1c].try_into().unwrap());
            ("crc32:", format!("{crc:#010x}"))
        }
    }
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
    let read = nord_usb::envelope::unwrap(&file).map_err(|e| format!("{}: {e}", path.display()))?;
    let format = nord_usb::envelope::tag(&read.header);
    check(path, &format, class)?;

    match (body, out) {
        (true, Some(out)) => {
            let wire_body = &read.body.0;
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
    let read = nord_usb::envelope::unwrap(&file).map_err(|e| format!("{}: {e}", path.display()))?;
    let format = nord_usb::envelope::tag(&read.header);
    check(path, &format, class)?;
    let at = nord_usb::envelope::location(&read.header);
    let body_len = read.body.0.len() as u32;
    let (crc_label, crc_value) = crc(&read.header, &file);

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
    row("version:", read.header.version.to_string());
    row(
        "body:",
        format!(
            "{} bytes{}",
            crate::device::grouped(body_len),
            match crate::device::human_size(body_len) {
                Some(h) => format!("  {}", ui.dim(format!("({h})"))),
                None => String::new(),
            }
        ),
    );
    row(crc_label, crc_value);
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
    // are pulled out per variant rather than through one reference to the body.
    let (piano, sample) = match &entity {
        Entity::Program(Program::Electro5(p)) => (p.piano_panel.id, p.sample_panel.id),
        Entity::Live(Live::Electro5(l)) => (l.piano_panel.id, l.sample_panel.id),
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
        Entity::Program(Program::Electro5(_)) => ne5::program::FORMAT,
        Entity::Live(Live::Electro5(_)) => ne5::live::FORMAT,
        Entity::Song(nord_format::Song::Electro5(_)) => ne5::song::FORMAT,
        Entity::Settings(nord_format::Settings::Electro5(_)) => ne5::settings::FORMAT,
        Entity::Piano(_) => npno::FORMAT,
        Entity::Sample(_) => nsmp::FORMAT,
        Entity::Bundle(_) => "zip",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_usb::wire::Location;

    /// The header a wrapped file gives back carries the version `wrap` stamped into it,
    /// and its CRC tracks the body — which pins both reads to the right header offsets.
    #[test]
    fn the_header_fields_come_back_out_of_a_wrapped_file() {
        let at = Location::from_user(7, 4);
        let a = nord_usb::envelope::wrap("ne5p", at, 4, &[0u8; 8]).unwrap();
        let b = nord_usb::envelope::wrap("ne5p", at, 4, &[1u8; 8]).unwrap();
        let c = nord_usb::envelope::wrap("ne5t", at, 1, &[0u8; 8]).unwrap();
        let header = |file: &[u8]| nord_usb::envelope::unwrap(file).unwrap().header;
        assert_eq!(header(&a).version, 4);
        assert_eq!(header(&c).version, 1);
        assert_eq!(crc(&header(&a), &a).0, "crc32:");
        assert_ne!(crc(&header(&a), &a).1, crc(&header(&b), &b).1);
    }

    /// A type-0 file has body bytes where the type-1 crc32 sits, so the checksum row
    /// has to follow the generation or it prints panel data as a checksum.
    #[test]
    fn a_type_0_file_reports_its_trailing_crc16() {
        let mut program = ne5::program::new((3, 7).try_into().unwrap());
        program.header.generation = Generation::V0;
        let mut bytes = Vec::new();
        program
            .write_to(&mut std::io::Cursor::new(&mut bytes))
            .unwrap();

        let header = nord_usb::envelope::unwrap(&bytes).unwrap().header;
        assert_eq!(header.version, 4);
        let (label, value) = crc(&header, &bytes);
        assert_eq!(label, "crc16:");
        let stored = u16::from_le_bytes(bytes[bytes.len() - 2..].try_into().unwrap());
        assert_eq!(value, format!("{stored:#06x}"));
        // 0x18 is the body's first byte here — the version echo, not a checksum.
        assert_eq!(u16::from_be_bytes([bytes[0x18], bytes[0x19]]), 4);
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
