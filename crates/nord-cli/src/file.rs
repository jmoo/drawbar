//! The read-only verbs (`get`, `info`, `deps`) on a file instead of a slot, and the
//! helpers shared by every verb that checks a list of targets.
//!
//! The object is the file's bytes, and no instrument is needed. What a file does not
//! hold (the slot name, the names behind dependency ids) is reported as held by the
//! instrument, never guessed.

use std::path::{Path, PathBuf};

use nord_format::accept::Family;
use nord_format::cbin::{self, Generation, Info};
use nord_format::{Entity, Live, Program};
use nord_usb::ObjectClass;

use crate::slot::shown;
use crate::ui::Ui;

/// The format tag a class's files carry, or `None` for a class with no known tag.
pub(crate) fn tag(class: ObjectClass) -> Option<&'static str> {
    Family::Electro5.tag(class.storage()?)
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
/// The inverse of [`tag`], so a format is only ever steered to the command that reads
/// it.
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

/// The stored checksum, with its generation's label.
fn crc(info: &Info) -> (&'static str, String) {
    match info.header.generation {
        Generation::V0 => ("crc16:", format!("{:#06x}", info.stored_checksum)),
        Generation::V1 => ("crc32:", format!("{:#010x}", info.stored_checksum)),
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
            "{} is already a file; -o has nothing to save (--body extracts the body without \
             its CBIN header)",
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

/// `info` on a file: the CBIN header, which USB transfers leave out.
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
    let container = cbin::inspect(&mut std::io::Cursor::new(&file))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let (crc_label, crc_value) = crc(&container);

    let row = |label: &str, value: String| {
        ui.out(format!("  {}{value}", ui.dim(format!("{label:<11}"))));
    };
    // Library files (samples) hold `0xffff:0xffff` where slot files hold bank and slot,
    // because a library object has no slot until an instrument gives it one.
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
/// Only ids: the names come from the instrument's `DEPENDENCIES` reply, so only the
/// slot form of the verb resolves them.
pub fn deps(ui: &Ui, path: &Path, class: ObjectClass) -> Result<(), String> {
    let entity = nord_format::from_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let format = entity.identity().format;
    check(path, format, class)?;

    // The two bodies are byte-identical but live in different slot spaces, so each
    // variant is matched on its own.
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
    ui.note(ui.dim("(ids only: the instrument holds the names, and `deps BANK:SLOT` shows them)"));
    Ok(())
}

/// Where two encodings of one object first differ.
///
/// A round trip that comes back different has a field the model does not account for,
/// and in a bit-packed body the offset usually identifies it.
///
/// ⚠️ Only called for two byte strings that differ, so when one is a prefix of the
/// other, they differ only in length.
pub(crate) fn first_difference(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(at) => format!("{at:#x}"),
        None => "the end (the lengths differ)".to_string(),
    }
}

/// Check each target, print its verdict line, and fail with a count of those that did
/// not pass.
///
/// The three `verify` verbs check different things and word their verdicts differently,
/// but share this contract: a run that loses a target says how many of how many failed,
/// and exits with a failure status a script can read.
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

/// Run each item under its heading, a blank line apart, report each failure beneath
/// its heading, and fail with a count of the items that did not run.
///
/// The reporting verbs print what each item holds instead of one verdict line, as
/// [`check_each`] does, but a run that loses an item still says how many failed.
pub(crate) fn report_each<T>(
    ui: &Ui,
    items: &[T],
    what: &str,
    heading: impl Fn(&T) -> String,
    mut run: impl FnMut(&T) -> Result<(), String>,
) -> Result<(), String> {
    let mut failed = 0usize;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            ui.out("");
        }
        ui.out(ui.bold(heading(item)));
        if let Err(e) = run(item) {
            failed += 1;
            ui.note(format!("  {} {e}", ui.danger("error")));
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} {what}", items.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::ne5;
    use nord_usb::wire::Location;

    fn inspect(file: &[u8]) -> Info {
        cbin::inspect(&mut std::io::Cursor::new(file)).unwrap()
    }

    /// A wrapped file reports a crc32 that tracks its body.
    #[test]
    fn a_wrapped_files_crc32_tracks_its_body() {
        let at = Location::from_user(7, 4);
        let a = nord_usb::envelope::wrap("ne5p", at, 4, &[0u8; 8]).unwrap();
        let b = nord_usb::envelope::wrap("ne5p", at, 4, &[1u8; 8]).unwrap();
        assert_eq!(crc(&inspect(&a)).0, "crc32:");
        assert_ne!(crc(&inspect(&a)).1, crc(&inspect(&b)).1);
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
        let (label, value) = crc(&inspect(&bytes));
        assert_eq!(label, "crc16:");
        let stored = u16::from_le_bytes(bytes[bytes.len() - 2..].try_into().unwrap());
        assert_eq!(value, format!("{stored:#06x}"));
        // 0x18 is the body's first byte here: the version echo, not a checksum.
        assert_eq!(u16::from_be_bytes([bytes[0x18], bytes[0x19]]), 4);
    }

    /// The offset must be the first byte that differs, and strings that differ only in
    /// length have no offset to give.
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

    #[test]
    fn a_report_of_many_items_counts_what_failed() {
        let ui = Ui::piped();
        let heading = |item: &&str| item.to_string();
        let run = |item: &&str| match *item {
            "bad" => Err("unreadable".to_string()),
            _ => Ok(()),
        };
        let what = "file(s) did not read";
        assert!(report_each(&ui, &["a", "b"], what, heading, run).is_ok());
        assert_eq!(
            report_each(&ui, &["bad", "a", "bad"], what, heading, run).unwrap_err(),
            "2 of 3 file(s) did not read"
        );
    }

    #[test]
    fn a_wrong_class_is_steered_to_the_right_noun() {
        let err = check(Path::new("x.ne5t"), "ne5t", ObjectClass::Program).unwrap_err();
        assert!(err.contains("nord setlist"), "{err}");
        assert!(check(Path::new("x.ne5t"), "ne5t", ObjectClass::SetList).is_ok());
        // A class with no known tag cannot be checked, so nothing is refused.
        assert!(check(Path::new("x.bin"), "abcd", ObjectClass::Unknown(9)).is_ok());
    }
}
