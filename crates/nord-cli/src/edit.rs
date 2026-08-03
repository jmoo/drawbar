//! `nord program edit` and `nord live edit` — change fields inside a program body.
//!
//! Field paths and values come straight from `#[bitpanel]`, so `--fields` cannot go
//! stale and a field becomes settable by being declared. The live buffer is the program
//! body under another tag, so both nouns run this one command with the class fixed.
//!
//! A file and a slot are the same command. The slot form is a read-modify-write over
//! USB, so it obeys the rule every mutation obeys — describe the target, then refuse
//! without `--yes`. Editing a file in place takes the same guard; `-o` avoids it.

use std::path::Path;

use nord_format::common::bank;
use nord_format::electro5;
use nord_format::electro5::program::{Field, Schema};
use nord_format::panel::FieldError;
use nord_format::{Entity, Live, Program, Settings};
use nord_usb::ObjectClass;

use crate::slot::Target;
use crate::ui::Ui;
use crate::EditArgs;

/// The schema shapes `edit` drives: the program body in either slot space, and the
/// settings singleton. One vocabulary — `--set member.field=value` — over each.
trait Editable {
    fn fields(&self) -> Vec<Field>;
    fn set_field(&mut self, path: &str, value: &str) -> Result<(), FieldError>;
}

impl<L: bank::Location> Editable for Schema<L> {
    fn fields(&self) -> Vec<Field> {
        Schema::fields(self)
    }
    fn set_field(&mut self, path: &str, value: &str) -> Result<(), FieldError> {
        Schema::set_field(self, path, value)
    }
}

impl Editable for electro5::settings::Schema {
    fn fields(&self) -> Vec<Field> {
        electro5::settings::Schema::fields(self)
    }
    fn set_field(&mut self, path: &str, value: &str) -> Result<(), FieldError> {
        electro5::settings::Schema::set_field(self, path, value)
    }
}

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
        (Entity::Program(Program::Electro5(p)), ObjectClass::Program) => {
            stage(ui, &args, &mut p.schema)?
        }
        (Entity::Live(Live::Electro5(l)), ObjectClass::Live) => stage(ui, &args, &mut l.schema)?,
        (Entity::Settings(Settings::Electro5(s)), ObjectClass::Settings) => {
            stage(ui, &args, &mut s.schema)?
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
            ObjectClass::Program => {
                crate::device::send(ui, &edited, at, class, args.yes, "the edited program")
            }
            // ⚠️ `send` deletes the destination to make room, and whether the live
            // buffer or the settings singleton survives a delete/write of its class is
            // unconfirmed on hardware. Until it is, an edited slot of either stops at
            // a file.
            _ => Err(format!(
                "writing {} back over USB is unproven; give -o a path to save the edit \
                 as a .{} file",
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
    let entity = match class {
        ObjectClass::Program => Entity::Program(Program::Electro5(electro5::Program::new(
            (0, 0).try_into().map_err(|e| format!("{e}"))?,
        ))),
        ObjectClass::Live => Entity::Live(Live::Electro5(electro5::Live::new(
            (0, 0).try_into().map_err(|e| format!("{e}"))?,
        ))),
        ObjectClass::Settings => Entity::Settings(Settings::Electro5(electro5::Settings::new())),
        other => return Err(format!("edit does not exist for {}", other.label())),
    };
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// The target decoded, but not to what this noun edits.
fn mismatch(entity: &Entity, class: ObjectClass) -> String {
    let got = crate::file::entity_tag(entity);
    format!(
        "this command edits {} ({}); the target holds {got}{}",
        class.label(),
        crate::file::tag(class).unwrap_or("?"),
        steer(got),
    )
}

/// The `edit` that reads a tag's files — empty for a tag whose noun has none, so the
/// message never points at a command that does not exist.
fn steer(tag: &str) -> &'static str {
    match tag {
        "ne5p" => " — try `nord program edit`",
        "ne5l" => " — try `nord live edit`",
        "ne5s" => " — try `nord settings edit`",
        _ => "",
    }
}

/// List the fields (`--fields`, `None`) or apply every `--set`, returning how many
/// fields moved.
fn stage(ui: &Ui, args: &EditArgs, schema: &mut impl Editable) -> Result<Option<usize>, String> {
    if args.fields {
        list_fields(ui, schema);
        return Ok(None);
    }
    if args.set.is_empty() {
        return Err("nothing to do: pass --set PATH=VALUE, or --fields to see what exists".into());
    }

    // Every change lands before anything is written, so a bad path or an out-of-range
    // value cannot leave a half-edited program behind.
    let before = schema.fields();
    for assignment in &args.set {
        let (path, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected PATH=VALUE, got {assignment:?}"))?;
        schema
            .set_field(path.trim(), value)
            .map_err(|e| e.to_string())?;
    }
    warn_on_sticky_pairs(ui, &args.set);

    let after = schema.fields();
    Ok(Some(report_changes(ui, &before, &after)))
}

fn write_file(ui: &Ui, path: &Path, bytes: &[u8]) -> Result<(), String> {
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
fn report_changes(
    ui: &Ui,
    before: &[electro5::program::Field],
    after: &[electro5::program::Field],
) -> usize {
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

/// The bytes that moved.
///
/// The CRC moves with any body change; the row is annotated so it does not read as a
/// second unexplained edit.
fn print_byte_diff(ui: &Ui, before: &[u8], after: &[u8]) {
    if before.len() != after.len() {
        ui.warn(format!(
            "length changed: {} -> {} bytes",
            before.len(),
            after.len()
        ));
        return;
    }
    for (i, (b, a)) in before.iter().zip(after).enumerate() {
        if b == a {
            continue;
        }
        // The `ne5p` body checksum, stamped by `nord-format` during encode rather than
        // set by anyone.
        let note = if (0x18..0x1c).contains(&i) {
            "  (body crc32)"
        } else {
            ""
        };
        ui.out(ui.dim(format!("  byte {i:#06x}  {b:#04x} -> {a:#04x}{note}")));
    }
}

fn list_fields(ui: &Ui, schema: &impl Editable) {
    ui.out(format!(
        "{:<40} {:<12} {:<28} {}",
        "path", "bits", "value", "accepts"
    ));
    for f in schema.fields() {
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
            "{:<40} {:<12} {value:<28} {accepts}",
            f.path, f.spec.placement,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wrong-format target must steer to the noun whose `edit` reads it, and never
    /// to one that has no `edit` at all.
    #[test]
    fn a_mismatched_target_steers_to_the_noun_that_edits_it() {
        let live = Entity::Live(Live::Electro5(electro5::Live::new(
            (0, 0).try_into().unwrap(),
        )));
        let err = mismatch(&live, ObjectClass::Program);
        assert!(err.contains("nord live edit"), "{err}");

        let program = Entity::Program(Program::Electro5(electro5::Program::new(
            (0, 0).try_into().unwrap(),
        )));
        let err = mismatch(&program, ObjectClass::Live);
        assert!(err.contains("nord program edit"), "{err}");

        let settings = Entity::Settings(Settings::Electro5(electro5::Settings::new()));
        let err = mismatch(&settings, ObjectClass::Program);
        assert!(err.contains("nord settings edit"), "{err}");

        // Set lists and library content have no edit, so no steer may be invented.
        for tag in ["ne5t", "npno", "nsmp", "zip"] {
            assert_eq!(steer(tag), "", "{tag}");
        }
    }
}
