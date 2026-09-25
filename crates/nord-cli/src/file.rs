//! The read-only verbs (`get`, `info`, `deps`) pointed at a file instead of a slot,
//! and what every verb that checks a list of them shares.
//!
//! Same verbs, no instrument: the object is the file's bytes. What a file does not
//! carry — the slot name, the names behind dependency ids — is reported as living on
//! the instrument rather than guessed at.

use std::ops::Range;
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

/// Every class a noun addresses, so a tag reads back to the command that takes it.
const NAMED: [ObjectClass; 6] = [
    ObjectClass::Piano,
    ObjectClass::Sample,
    ObjectClass::Program,
    ObjectClass::SetList,
    ObjectClass::Live,
    ObjectClass::Settings,
];

/// The noun that reads a tag's files, for steering a mismatch to the right command.
///
/// [`tag`] read backwards: a class that names its files steers to its own noun, so a
/// format cannot be claimed by a command that does not read it.
pub(crate) fn noun(format: &str) -> Option<String> {
    NAMED
        .into_iter()
        .find(|&class| tag(class) == Some(format))
        .map(crate::slot::noun)
}

/// Refuse a file whose format tag belongs to another class's noun: summarizing a set
/// list under `nord program` would mislabel everything it prints.
fn check(path: &Path, format: &str, class: ObjectClass) -> Result<(), String> {
    match tag(class) {
        Some(want) if want != format => {
            let steer = match noun(format) {
                Some(n) => format!(" — try `nord {n}`"),
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

/// Where a CBIN file of this generation keeps its checksum.
///
/// ⚠️ A type-0 file holds body data at `0x18`, where a type-1 file holds its crc32, so
/// the range follows the generation: read at the wrong one, a program's panel bytes
/// report as a checksum and a real edit annotates as bookkeeping.
///
/// This belongs in `cbin::Generation`, beside the rest of the layout it describes.
pub(crate) fn checksum_range(generation: Generation, len: usize) -> Option<Range<usize>> {
    match generation {
        Generation::V0 => len.checked_sub(2).map(|at| at..len),
        Generation::V1 => (len >= 0x1c).then_some(0x18..0x1c),
    }
}

/// The stored checksum, with the label its generation spells it under.
///
/// The one header fact the parsed [`Header`] does not carry: the value lives in the
/// bytes, at [`checksum_range`]. `unwrap` verified it, so this reports what it checked.
fn crc(header: &Header, bytes: &[u8]) -> (&'static str, String) {
    let stored = checksum_range(header.generation, bytes.len()).and_then(|at| bytes.get(at));
    match (header.generation, stored) {
        (Generation::V0, Some(b)) => (
            "crc16:",
            format!("{:#06x}", u16::from_le_bytes(b.try_into().unwrap())),
        ),
        (Generation::V1, Some(b)) => (
            "crc32:",
            format!("{:#010x}", u32::from_le_bytes(b.try_into().unwrap())),
        ),
        // The header parsed, so the file is longer than either range; a file too short
        // to hold one says so rather than reporting a number it did not read.
        _ => ("crc:", "not in these bytes".to_string()),
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
            crate::edit::replace_file(&out, wire_body)?;
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
    let body_len = u32::try_from(read.body.0.len()).map_err(|_| {
        format!(
            "{}: body length does not fit the file format",
            path.display()
        )
    })?;
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
    row(
        "name:",
        format!(
            "none {}",
            ui.dim("(the CBIN header has no name; inspect reads the body)")
        ),
    );
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
        Entity::Program(Program::Electro5(p)) => (p.piano_panel.id.id(), p.sample_panel.id.id()),
        Entity::Live(Live::Electro5(l)) => (l.piano_panel.id.id(), l.sample_panel.id.id()),
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
        ui.out(format!(
            "{:<8} {}",
            class.label(),
            crate::summary::dep_id(*id)
        ));
    }
    ui.note(ui.dim("(ids only — the names live on the instrument; `deps BANK:SLOT` shows them)"));
    Ok(())
}

/// The format tag a decoded entity would carry on disk.
pub(crate) fn entity_tag(entity: &Entity) -> &'static str {
    entity.identity().format
}

/// Where two renderings of one object first disagree.
///
/// A round trip that comes back different is a field the model does not account for,
/// and in a bit-packed body the offset usually names that field on its own.
///
/// ⚠️ Only reached for two byte strings that differ, so one that is a prefix of the
/// other differs in its length and nowhere else.
pub(crate) fn first_difference(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(at) => format!("{at:#x}"),
        None => "the end (the lengths differ)".to_string(),
    }
}

/// Check each target, print its verdict line, and fail with how many did not pass.
///
/// The three `verify` verbs differ in what they check and in how a verdict reads; that
/// a run which lost a target says so, and says how many of how many, is one contract —
/// and the exit status is what a script reads.
pub(crate) fn check_each<T>(
    ui: &Ui,
    targets: &[T],
    what: &str,
    mut check: impl FnMut(&T) -> Result<String, String>,
) -> Result<(), String> {
    let mut failed = 0usize;
    for target in targets {
        match check(target) {
            Ok(line) => ui.out(line),
            Err(line) => {
                failed += 1;
                ui.out(line);
            }
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} {what}", targets.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_usb::wire::Location;

    /// A wrapped file reports a crc32 that tracks its body.
    #[test]
    fn a_wrapped_files_crc32_tracks_its_body() {
        let at = Location::from_user(7, 4);
        let a = nord_usb::envelope::wrap("ne5p", at, 4, &[0u8; 8]).unwrap();
        let b = nord_usb::envelope::wrap("ne5p", at, 4, &[1u8; 8]).unwrap();
        let header = |file: &[u8]| nord_usb::envelope::unwrap(file).unwrap().header;
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

    /// The offset is what names the field a round trip lost, so it has to be the first
    /// byte that moved; bytes that only ran out have no offset to give.
    #[test]
    fn a_round_trip_that_differs_says_where_it_first_did() {
        assert_eq!(first_difference(b"abcd", b"abed"), "0x2");
        assert_eq!(first_difference(b"Xbcd", b"abcd"), "0x0");
        let ran_out = "the end (the lengths differ)";
        assert_eq!(first_difference(b"abcd", b"abcde"), ran_out);
        assert_eq!(first_difference(b"", b"a"), ran_out);
    }

    /// A run that lost some of its targets exits with that count, so a script can tell
    /// it from one that checked out.
    #[test]
    fn a_check_of_many_targets_counts_what_failed() {
        let ui = Ui::piped();
        let verdict = |target: &&str| match *target {
            "bad" => Err("DIFFER bad".to_string()),
            ok => Ok(format!("ok {ok}")),
        };
        let what = "file(s) did not round-trip";
        assert!(check_each(&ui, &["a", "b"], what, verdict).is_ok());
        assert_eq!(
            check_each(&ui, &["a", "bad", "bad"], what, verdict).unwrap_err(),
            "2 of 3 file(s) did not round-trip"
        );
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
