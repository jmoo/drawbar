//! `nord sample edit` — rename, retune and remap a sample instrument.
//!
//! The spelling mirrors `nord program edit`, but the fields come from
//! [`editors::SampleEditor`]'s accessors rather than a declarative panel: a
//! sample is mostly encoded audio, and only what the format crate can patch in
//! place is settable — the name, and each zone's root key and top note.

use std::path::PathBuf;

use clap::Args;
use nord_format::Entity;
use nord_usb::ObjectClass;

use crate::edit::{print_byte_diff, write_file};
use crate::editors::{self, SampleEditor};
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
    let sample = match &mut entity {
        Entity::Sample(nord_format::Sample::V2(sample)) => sample,
        // Editing needs the v2 zone/stroke accessors; the v3/v4 chain is read-only.
        Entity::Sample(nord_format::Sample::V3(_)) => {
            return Err(
                "this instrument is nsmp3/nsmp4 content; only v2 (.nsmp) can be edited".into(),
            )
        }
        Entity::SampleProject(_) => {
            return Err(
                "this is a Sample Editor project, not a sample instrument — try `nord edit`".into(),
            )
        }
        _ => return Err("sample edit only understands sample instruments (.nsmp)".into()),
    };

    let Some(changed) = editors::stage(ui, args.fields, &args.set, &mut SampleEditor(sample))?
    else {
        // `--fields` has listed them and is done.
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
            None,
            None,
        ),
    }
}
