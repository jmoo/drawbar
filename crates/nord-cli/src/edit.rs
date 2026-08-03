//! `nord program edit` — change fields inside a program.
//!
//! Field paths and values come straight from `#[bitpanel]`, so `--fields` cannot go
//! stale and a field becomes settable by being declared.
//!
//! A file and a slot are the same command. The slot form is a read-modify-write over
//! USB, so it obeys the rule every mutation obeys — describe the target, then refuse
//! without `--yes`. Editing a file in place takes the same guard; `-o` avoids it.

use std::path::{Path, PathBuf};

use nord_format::electro5;
use nord_format::{Entity, Program};
use nord_usb::wire::Location;
use nord_usb::ObjectClass;

use crate::ui::Ui;
use crate::EditArgs;

/// Where the program came from, and therefore where it goes back to.
enum Target {
    File(PathBuf),
    Slot(Location),
    /// `--fields` with nothing to read: list what a fresh program offers.
    Default,
}

/// Decide whether an argument names a file or a slot.
///
/// A path that exists wins, so a file called `7:4` is still a file. Otherwise anything
/// that parses as `BANK:SLOT` is one — which is what makes `nord program edit 7:4` work
/// without a flag saying which kind of thing was meant.
fn resolve(target: Option<String>) -> Result<Target, String> {
    let Some(target) = target else {
        return Ok(Target::Default);
    };
    let path = PathBuf::from(&target);
    if path.exists() {
        return Ok(Target::File(path));
    }
    match crate::slot::parse(&target) {
        Ok(at) => Ok(Target::Slot(at)),
        // Neither reading works, so the error has to name both.
        Err(e) => Err(format!("{target}: no such file, and not a slot ({e})")),
    }
}

pub fn run(ui: &Ui, args: EditArgs) -> Result<(), String> {
    let target = resolve(args.target)?;

    let original = match &target {
        Target::File(path) => {
            std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Target::Slot(at) => crate::device::fetch(*at, ObjectClass::Program)?,
        Target::Default => {
            let entity = Entity::Program(Program::Electro5(electro5::Program::new(
                (0, 0).try_into().map_err(|e| format!("{e}"))?,
            )));
            nord_format::to_bytes(&entity).map_err(|e| e.to_string())?
        }
    };

    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| e.to_string())?;
    let Entity::Program(Program::Electro5(program)) = &mut entity else {
        return Err("edit only understands Electro 5 programs (.ne5p)".into());
    };

    if args.fields {
        list_fields(ui, &program.schema);
        return Ok(());
    }
    if args.set.is_empty() {
        return Err("nothing to do: pass --set PATH=VALUE, or --fields to see what exists".into());
    }

    // Every change lands before anything is written, so a bad path or an out-of-range
    // value cannot leave a half-edited program behind.
    let before = program.schema.fields();
    for assignment in &args.set {
        let (path, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected PATH=VALUE, got {assignment:?}"))?;
        program
            .schema
            .set_field(path.trim(), value)
            .map_err(|e| e.to_string())?;
    }
    warn_on_sticky_pairs(ui, &args.set);

    let after = program.schema.fields();
    let changed = report_changes(ui, &before, &after);
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
        (Target::File(path), None) => {
            ui.note(format!(
                "about to {} {} in place",
                ui.danger("overwrite"),
                path.display()
            ));
            ui.confirm(args.yes)?;
            write_file(ui, &path, &edited)
        }
        (Target::Slot(at), None) => crate::device::send(
            ui,
            &edited,
            at,
            ObjectClass::Program,
            args.yes,
            "the edited program",
        ),
        (Target::Default, None) => {
            Err("editing a default program needs -o: there is nothing to write back to".into())
        }
    }
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

fn list_fields(ui: &Ui, schema: &electro5::program::Schema<electro5::program::Location>) {
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
