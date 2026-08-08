//! `nord sample edit` — rename, retune and remap a sample instrument.
//!
//! The spelling mirrors `nord program edit`, but the fields come from [`Sample`]'s
//! accessors rather than a declarative panel: a sample is mostly encoded audio, and
//! only what the format crate can patch in place is settable — the name, and each
//! zone's root key and top note.

use std::path::PathBuf;

use clap::Args;
use nord_format::cbin::Cbin;
use nord_format::formats::nsmp::Sample;
use nord_format::Entity;
use nord_usb::ObjectClass;

use crate::edit::{print_byte_diff, write_file};
use crate::note;
use crate::slot::Target;
use crate::ui::Ui;

#[derive(Args)]
pub struct EditArgs {
    /// A `.nsmp` file, or a slot on the instrument (`1:14`). A slot makes this a
    /// read-modify-write over USB, so it is a mutation and obeys `--yes`.
    #[arg(value_name = "FILE|BANK:SLOT")]
    pub target: String,

    /// `path=value`, repeatable: `name=NAME`, `zone1.root_key=NOTE`,
    /// `zone1.top_note=NOTE`. Notes are names (`C4`, `F#3`) or numbers (0-127).
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub set: Vec<String>,

    /// Report what would change — including which bytes — and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// List every settable field with its current value, then exit.
    #[arg(long)]
    pub fields: bool,

    /// Write the edited sample here instead of over the input file.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Confirm the write. Editing a slot, or a file in place, needs it.
    #[arg(long)]
    pub yes: bool,
}

pub fn run(ui: &Ui, args: EditArgs) -> Result<(), String> {
    let target = crate::slot::target(&args.target)?;

    let original = match &target {
        Target::File(path) => {
            std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Target::Slot(at) => crate::device::fetch(*at, ObjectClass::Sample)?,
    };

    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| e.to_string())?;
    let Entity::Sample(sample) = &mut entity else {
        return Err("sample edit only understands sample instruments (.nsmp)".into());
    };

    if args.fields {
        return list_fields(ui, sample);
    }
    if args.set.is_empty() {
        return Err("nothing to do: pass --set PATH=VALUE, or --fields to see what exists".into());
    }

    // Every change lands before anything is written, so a bad path or an out-of-range
    // value cannot leave a half-edited sample behind.
    let before = snapshot(sample)?;
    for assignment in &args.set {
        let (path, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected PATH=VALUE, got {assignment:?}"))?;
        apply(sample, path.trim(), value.trim())?;
    }
    let after = snapshot(sample)?;

    let mut changed = 0;
    for ((path, b), (_, a)) in before.iter().zip(&after) {
        if b != a {
            changed += 1;
            ui.out(format!("{path:<20} {b} -> {}", ui.bold(a)));
        }
    }
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
            ObjectClass::Sample,
            args.yes,
            "the edited sample",
        ),
    }
}

/// Every settable field with its display value, in path order.
fn snapshot(sample: &Cbin<Sample>) -> Result<Vec<(String, String)>, String> {
    let mut out = vec![(
        "name".to_string(),
        sample.name().map_err(|e| e.to_string())?,
    )];
    let zones = sample.zones().map_err(|e| e.to_string())?;
    let strokes = sample.strokes().map_err(|e| e.to_string())?;
    for (i, (zone, stroke)) in zones.iter().zip(&strokes).enumerate() {
        let n = i + 1;
        out.push((format!("zone{n}.root_key"), note::name(stroke.root_key)));
        out.push((format!("zone{n}.top_note"), note::name(zone.top_note)));
    }
    Ok(out)
}

fn apply(sample: &mut Cbin<Sample>, path: &str, value: &str) -> Result<(), String> {
    if path == "name" {
        return sample.set_name(value).map_err(|e| e.to_string());
    }
    let unknown = || format!("unknown field {path:?}; --fields lists what exists");
    let (zone, field) = path.split_once('.').ok_or_else(unknown)?;
    let index = zone
        .strip_prefix("zone")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .ok_or_else(unknown)?;
    // Checked here so the message speaks the CLI's 1-based numbering, not the
    // format crate's 0-based one.
    let zones = sample.zones().map_err(|e| e.to_string())?.len();
    if index > zones {
        return Err(format!("no zone {index}: the instrument has {zones}"));
    }
    let value = note::parse(value)?;
    match field {
        "root_key" => sample.set_root_key(index - 1, value),
        "top_note" => sample.set_zone_top_note(index - 1, value),
        _ => return Err(unknown()),
    }
    .map_err(|e| e.to_string())
}

fn list_fields(ui: &Ui, sample: &Cbin<Sample>) -> Result<(), String> {
    ui.out(format!("{:<20} {:<16} {}", "path", "value", "accepts"));
    for (path, value) in snapshot(sample)? {
        let accepts = if path == "name" {
            format!("up to {} bytes", nord_format::formats::nsmp::MAX_NAME_LEN)
        } else {
            "a note name (C4, F#3) or 0-127".to_string()
        };
        ui.out(format!("{path:<20} {value:<16} {accepts}"));
    }
    Ok(())
}
