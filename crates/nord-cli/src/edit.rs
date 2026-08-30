//! `nord program edit`, `nord live edit`, `nord setlist edit` and `nord
//! settings edit` — change fields inside an Electro 5 body.
//!
//! Field paths and values come straight from `#[bitpanel]`, so `--fields`
//! cannot go stale and a field becomes settable by being declared. The live
//! buffer is the program body under another tag, so both nouns run this one
//! command with the class fixed. The set list has no registry — its four slots
//! are set through [`editors::SongEditor`], in the same `--set` vocabulary.
//!
//! A file and a slot are the same command. The slot form is a read-modify-write
//! over USB, so it obeys the rule every mutation obeys — describe the target,
//! then refuse without `--yes`. Editing a file in place takes the same guard;
//! `-o` avoids it.

use std::path::Path;

use nord_format::fields::{ControlKind, Field, Registry, Unit};
use nord_format::formats::ne5;
use nord_format::{Entity, Live, Program, Settings, Song};
use nord_usb::ObjectClass;

use crate::editors;
use crate::slot::Target;
use crate::ui::Ui;
use crate::EditArgs;

pub fn run(ui: &Ui, args: EditArgs, class: ObjectClass) -> Result<(), String> {
    // No target is `--fields` or `-o` with nothing to read: a fresh default object.
    let target = args
        .target
        .as_deref()
        .map(crate::slot::target)
        .transpose()?;

    let original = match &target {
        Some(Target::File(path)) => {
            std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Some(Target::Slot(at)) => crate::device::fetch(*at, class)?,
        None => fresh(class)?,
    };

    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| e.to_string())?;
    let staged = match (&mut entity, class) {
        (Entity::Program(Program::Electro5(p)), ObjectClass::Program) => stage(ui, &args, p)?,
        (Entity::Live(Live::Electro5(l)), ObjectClass::Live) => stage(ui, &args, l)?,
        (Entity::Settings(Settings::Electro5(s)), ObjectClass::Settings) => stage(ui, &args, s)?,
        (Entity::Song(Song::Electro5(s)), ObjectClass::SetList) => {
            editors::stage(ui, args.fields, &args.set, &mut editors::SongEditor(s))?
        }
        _ => return Err(mismatch(&entity, class)),
    };
    // `--fields` has listed them and is done.
    let Some(changed) = staged else {
        return Ok(());
    };
    if changed == 0 {
        ui.note("no field changed; writing nothing");
        return Ok(());
    }

    let edited = nord_format::to_bytes(&entity).map_err(|e| e.to_string())?;
    print_byte_diff(ui, &original, &edited);

    if args.dry_run {
        ui.note("--dry-run: nothing written");
        return Ok(());
    }

    match (target, args.out) {
        // An explicit destination is the unambiguous case, whatever the source was.
        (_, Some(out)) => write_file(ui, &out, &edited),
        (Some(Target::File(path)), None) => {
            ui.note(format!(
                "about to {} {} in place",
                ui.danger("overwrite"),
                path.display()
            ));
            ui.confirm(args.yes)?;
            write_file(ui, &path, &edited)
        }
        (Some(Target::Slot(at)), None) => match class {
            ObjectClass::Program => crate::device::send(
                ui,
                &edited,
                at,
                class,
                args.yes,
                "the edited program",
                None,
                None,
            ),
            ObjectClass::SetList => crate::device::send(
                ui,
                &edited,
                at,
                class,
                args.yes,
                "the edited set list",
                None,
                None,
            ),
            // ⚠️ These classes take a write in place; a delete of either is untried.
            _ => Err(format!(
                "writing {} back over USB deletes first, which is untried for this \
                 class; give -o a path to save the edit as a .{} file",
                class.label(),
                crate::file::tag(class).unwrap_or("bin"),
            )),
        },
        (None, None) => {
            Err("editing a fresh default needs -o: there is nothing to write back to".into())
        }
    }
}

/// The bytes of a fresh default object: what a target-less `--fields` lists and a
/// target-less `-o` starts from.
fn fresh(class: ObjectClass) -> Result<Vec<u8>, String> {
    let first = |e| format!("{e}");
    let entity = match class {
        ObjectClass::Program => Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().map_err(first)?,
        ))),
        ObjectClass::Live => Entity::Live(Live::Electro5(ne5::live::new(
            (0, 0).try_into().map_err(first)?,
        ))),
        ObjectClass::Settings => Entity::Settings(Settings::Electro5(ne5::settings::new())),
        ObjectClass::SetList => Entity::Song(Song::Electro5(ne5::song::new(
            (0, 0).try_into().map_err(first)?,
            ne5::song::DEFAULT_VERSION,
            [(0, 0).try_into().map_err(first)?; 4],
        ))),
        other => return Err(format!("edit does not exist for {}", other.label())),
    };
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// The target decoded, but not to what this noun edits.
fn mismatch(entity: &Entity, class: ObjectClass) -> String {
    format!(
        "this command edits {} ({}); the target holds {}{}",
        class.label(),
        crate::file::tag(class).unwrap_or("?"),
        crate::file::entity_tag(entity),
        steer(entity),
    )
}

/// The `edit` that reads this entity's files — empty for something nothing
/// edits, so the message never points at a command that does not exist.
fn steer(entity: &Entity) -> &'static str {
    match crate::file::entity_tag(entity) {
        "ne5p" => " — try `nord program edit`",
        "ne5l" => " — try `nord live edit`",
        "ne5s" => " — try `nord settings edit`",
        "ne5t" => " — try `nord setlist edit`",
        "nsmp" => " — try `nord sample edit`",
        // Everything else editable — the Stage bodies, the Sample Editor
        // project — has no noun of its own and lives under the file verb.
        _ if crate::file_edit::editable(entity) => " — try `nord edit`",
        _ => "",
    }
}

/// List the fields (`--fields`, `None`) or apply every `--set`, returning how many
/// fields moved.
pub(crate) fn stage(
    ui: &Ui,
    args: &EditArgs,
    file: &mut dyn Registry,
) -> Result<Option<usize>, String> {
    if args.fields {
        if !args.set.is_empty() {
            return Err("--fields lists and writes nothing; drop it to apply --set".into());
        }
        list_fields(ui, file);
        return Ok(None);
    }
    if args.set.is_empty() {
        return Err("nothing to do: pass --set PATH=VALUE, or --fields to see what exists".into());
    }

    // Every change lands before anything is written, so a bad path or an out-of-range
    // value cannot leave a half-edited program behind.
    let before = file.fields();
    for assignment in &args.set {
        let (path, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected PATH=VALUE, got {assignment:?}"))?;
        file.set_field(path.trim(), value)
            .map_err(|e| e.to_string())?;
    }
    warn_on_sticky_pairs(ui, &args.set);

    let after = file.fields();
    Ok(Some(report_changes(ui, &before, &after)))
}

pub(crate) fn write_file(ui: &Ui, path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    ui.note(format!("wrote {} ({} bytes)", path.display(), bytes.len()));
    Ok(())
}

/// ⚠️ Fields that do nothing without a companion. The pairing is a fact about the
/// instrument, not something the declaration carries.
///
/// Transpose: the stored value is ignored while `transpose_enabled` is clear, the
/// instrument never clears that bit once set, and an untouched program holds `+1` rather
/// than `0`. So `--set center_panel.transpose=0` alone leaves a program the panel still
/// calls transposed. Warn rather than refuse — setting one half deliberately is
/// legitimate.
const STICKY_PAIRS: [(&str, &str); 1] =
    [("center_panel.transpose", "center_panel.transpose_enabled")];

fn warn_on_sticky_pairs(ui: &Ui, sets: &[String]) {
    let paths: Vec<&str> = sets
        .iter()
        .filter_map(|s| s.split_once('=').map(|(p, _)| p.trim()))
        .collect();
    for (field, companion) in STICKY_PAIRS {
        if paths.contains(&field) && !paths.contains(&companion) {
            ui.warn(format!(
                "{field} was set but {companion} was not; the instrument reads the pair, not \
                 either half alone",
            ));
        }
    }
}

/// Echo every field whose value moved, before and after.
///
/// Display lives on the value, so this prints exactly what `nord inspect` would.
fn report_changes(ui: &Ui, before: &[Field], after: &[Field]) -> usize {
    let mut changed = 0;
    for (b, a) in before.iter().zip(after) {
        if b.display == a.display {
            continue;
        }
        changed += 1;
        ui.out(format!(
            "{:<40} {} -> {}",
            a.path,
            b.display,
            ui.bold(&a.display),
        ));
    }
    changed
}

/// Where a CBIN file keeps its checksum and what to call it, or `None` for bytes that
/// are not a CBIN file.
///
/// The two generations put it in different places, and a type-0 file's `0x18` is body
/// data — annotating it as the type-1 crc32 would label a real edit as bookkeeping.
fn checksum_bytes(file: &[u8]) -> Option<(std::ops::Range<usize>, &'static str)> {
    if file.len() < 8 || &file[0..4] != nord_format::cbin::MAGIC {
        return None;
    }
    match u32::from_le_bytes(file[4..8].try_into().unwrap()) {
        0 => Some((file.len() - 2..file.len(), "  (file crc16)")),
        1 => Some((0x18..0x1c, "  (body crc32)")),
        _ => None,
    }
}

/// The bytes that moved.
///
/// The CRC moves with any body change; the row is annotated so it does not read as a
/// second unexplained edit.
pub(crate) fn print_byte_diff(ui: &Ui, before: &[u8], after: &[u8]) {
    if before.len() != after.len() {
        ui.warn(format!(
            "length changed: {} -> {} bytes",
            before.len(),
            after.len()
        ));
        return;
    }
    let checksum = checksum_bytes(after);
    for (i, (b, a)) in before.iter().zip(after).enumerate() {
        if b == a {
            continue;
        }
        // The CBIN checksum, stamped by `nord-format` during encode rather than set by
        // anyone.
        let note = match &checksum {
            Some((at, label)) if at.contains(&i) => *label,
            _ => "",
        };
        ui.out(ui.dim(format!("  byte {i:#06x}  {b:#04x} -> {a:#04x}{note}")));
    }
}

/// The panel control a field is, in the short form the listing has room for.
fn control(kind: ControlKind) -> String {
    let unit = |u: Unit| match u {
        Unit::Panel10 => "0-10",
        Unit::Decibels => "dB",
        Unit::Milliseconds => "ms",
        Unit::Hertz => "Hz",
        Unit::Bpm => "bpm",
        Unit::ClockDivision => "clock",
        Unit::Semitones => "semi",
        Unit::Octaves => "oct",
        Unit::Pan => "pan",
        Unit::None => "",
    };
    match kind {
        ControlKind::Toggle => "toggle".to_string(),
        ControlKind::Selector => "selector".to_string(),
        ControlKind::Knob(u) => format!("knob {}", unit(u)),
        ControlKind::Bipolar(u) => format!("bipolar {}", unit(u)),
        ControlKind::Shift(u) => format!("shift {}", unit(u)),
        ControlKind::Drawbar => "drawbar".to_string(),
        ControlKind::Morph => "morph".to_string(),
        ControlKind::Pattern => "pattern".to_string(),
        ControlKind::Reference => "library ref".to_string(),
        ControlKind::Number => "number".to_string(),
    }
}

fn list_fields(ui: &Ui, file: &dyn Registry) {
    ui.out(format!(
        "{:<40} {:<12} {:<14} {:<28} {}",
        "path", "bits", "control", "value", "accepts"
    ));
    for f in file.fields() {
        // A field too wide to enumerate lists no values; its stored bits are the
        // spelling, and the current one is already in the value column.
        let accepts = match (f.spec.legal)() {
            v if v.is_empty() => "stored bits, decimal or 0x…".to_string(),
            v if v.len() > 12 => format!("{} .. {}", v.first().unwrap(), v.last().unwrap()),
            v => v.join(", "),
        };
        let value = if f.value == f.display {
            f.value.clone()
        } else {
            format!("{} {}", f.value, ui.dim(&f.display))
        };
        ui.out(format!(
            "{:<40} {:<12} {:<14} {value:<28} {accepts}",
            f.path,
            f.spec.placement,
            ui.dim(control(f.spec.control)),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wrong-format target must steer to the noun whose `edit` reads it — and for a
    /// format with no noun, to the file verb — never to a command that does not exist.
    #[test]
    fn a_mismatched_target_steers_to_the_command_that_edits_it() {
        let live = Entity::Live(Live::Electro5(ne5::live::new((0, 0).try_into().unwrap())));
        let err = mismatch(&live, ObjectClass::Program);
        assert!(err.contains("nord live edit"), "{err}");

        let program = Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().unwrap(),
        )));
        let err = mismatch(&program, ObjectClass::Live);
        assert!(err.contains("nord program edit"), "{err}");

        let settings = Entity::Settings(Settings::Electro5(ne5::settings::new()));
        let err = mismatch(&settings, ObjectClass::Program);
        assert!(err.contains("nord settings edit"), "{err}");

        let song = Entity::Song(Song::Electro5(ne5::song::new(
            (0, 0).try_into().unwrap(),
            ne5::song::DEFAULT_VERSION,
            [(0, 0).try_into().unwrap(); 4],
        )));
        let err = mismatch(&song, ObjectClass::Program);
        assert!(err.contains("nord setlist edit"), "{err}");

        // A registry body with no noun of its own goes to the file verb.
        let stage = nord_format::from_stream(&mut std::io::Cursor::new(
            crate::file_edit::tests::stage3_program(),
        ))
        .unwrap();
        let err = mismatch(&stage, ObjectClass::Program);
        assert!(err.contains("nord edit"), "{err}");

        // A piano library has no edit anywhere, so no steer may be invented.
        assert_eq!(
            steer(&nord_format::from_stream(&mut std::io::Cursor::new(pipe_library())).unwrap()),
            "",
        );
    }

    /// The smallest container-verified stub: enough bytes to decode, nothing to edit.
    fn pipe_library() -> Vec<u8> {
        let file = nord_format::cbin::Cbin {
            header: nord_format::cbin::Header::new("npip", (0xffff, 0xffff), 1),
            body: nord_format::cbin::RawBody(vec![0x5a; 16]),
        };
        let mut out = std::io::Cursor::new(Vec::new());
        file.write_to(&mut out).unwrap();
        out.into_inner()
    }
}
