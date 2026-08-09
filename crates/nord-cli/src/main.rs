//! `nord` — a thin CLI over [`nord_format`] and `nord_usb`, purely to dogfood them.
//!
//! Not a product; it exists to exercise the libraries and surface API friction.
//!
//! The nouns are the protocol's object classes: `nord program`, `nord sample`,
//! `nord setlist` and `nord live` are [`slot_action`] with the class fixed, `nord
//! settings` carries the one verb its singleton needs (`edit`), and the hidden
//! `nord raw --class N` is [`slot_action`] with the class given as a number.
//! `inspect`/`verify` dispatch on the CBIN format tag rather than on a class, so they sit
//! at the top level.
//!
//! ⚠️ `raw` is hidden but supported: it is the only way to reach a class with no noun of
//! its own, which today is pianos (1).

mod device;
mod edit;
mod file;
mod note;
mod sample;
mod slot;
mod summary;
mod ui;

use clap::{Args, Parser, Subcommand};
use nord_usb::ObjectClass;
use std::path::PathBuf;
use std::process::ExitCode;

use ui::{ColorChoice, Ui};

#[derive(Parser)]
#[command(name = "nord", about = "Inspect Clavia / Nord keyboard files", version)]
struct Cli {
    /// When to color output. `auto` means "stdout is a terminal"; `NO_COLOR` in the
    /// environment forces it off.
    #[arg(long, global = true, value_name = "WHEN", value_enum, default_value_t)]
    color: ColorChoice,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse Nord file(s) and print a summary of the decoded contents.
    Inspect {
        /// Files to read (.ne5p program, .ne5l live slot, .ne5t song, .ne5s
        /// settings, .npno piano, .nsmp sample, or a ZIP backup bundle).
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Dump the full `Debug` representation instead of the summary.
        #[arg(long)]
        raw: bool,
    },

    /// Re-encode file(s) and check the result is byte-identical to the input.
    ///
    /// Checks `nord-format`'s central invariant: decoded values are read-only views over
    /// a verbatim body, so a parse followed by a re-emit cannot drift.
    Verify {
        /// Files to round-trip. Bundles are archives, not re-emittable entities.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// The attached instrument itself: what is on the bus, and what it holds.
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },

    /// Programs on the instrument (object class 4). Slots are `BANK:SLOT`, as the
    /// instrument displays them; the read-only verbs and `edit` take a file instead.
    Program {
        #[command(subcommand)]
        action: ProgramAction,
    },

    /// Set lists on the instrument (object class 5). Same verbs as `nord program`.
    Setlist {
        #[command(subcommand)]
        action: SlotAction,
    },

    /// The live buffer — the panel as it stands (object class 6), in slots 1:1 to 1:3.
    Live {
        #[command(subcommand)]
        action: LiveAction,
    },

    /// The global settings singleton (object class 7): the System, MIDI and Sound
    /// menus, plus the panel state the instrument restores at power-up.
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },

    /// Sample instruments — the library on the instrument (object class 3), or
    /// `.nsmp` files.
    Sample {
        #[command(subcommand)]
        action: SampleAction,
    },

    /// The class-generic primitives, addressed by object-class number.
    ///
    /// Every typed noun above is this with the class fixed. Use it for a class with no
    /// noun — pianos are `--class 1`.
    #[command(hide = true)]
    Raw {
        /// Object class: 1 pianos, 3 samples, 4 programs, 5 set lists, 6 live.
        #[arg(long, global = true, value_name = "N", default_value_t = 4)]
        class: u32,

        #[command(subcommand)]
        action: SlotAction,
    },
}

#[derive(Subcommand)]
enum DeviceAction {
    /// Report what is stored on the instrument, per object class.
    ///
    /// Read-only: this sends one query per class and reads counters back. Nothing
    /// on the instrument is modified.
    Status {
        /// Replay a recorded exchange instead of opening a device. Useful for
        /// demos and for exercising the whole path without hardware.
        #[arg(long, value_name = "SCRIPT")]
        replay: Option<PathBuf>,

        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Identify the attached instrument, from its USB descriptors. Read-only, and opens
    /// no transaction — the first thing to run when nothing else answers.
    Info,
}

/// `nord program`: every class-generic verb, plus the one that only programs have.
#[derive(Subcommand)]
enum ProgramAction {
    #[command(flatten)]
    Slot(SlotAction),

    /// Change fields inside a program, in a file or in a slot.
    ///
    /// Field paths are `nord-format`'s own — `center_panel.transpose`,
    /// `effects_panel.fx1_rate`. `--fields` lists them.
    ///
    /// With no target the program is a fresh default one, so `--fields` needs nothing to
    /// read and `-o` writes a blank `.ne5p` to start from.
    Edit(EditArgs),
}

/// `nord sample`: every class-generic verb, plus the one that edits files.
#[derive(Subcommand)]
enum SampleAction {
    #[command(flatten)]
    Slot(SlotAction),

    /// Change fields inside a sample instrument, in a file or in a slot.
    ///
    /// A sample is mostly encoded audio; what is settable is what the format can
    /// patch in place — the name, and each zone's root key and top note. `--fields`
    /// lists them.
    Edit(sample::EditArgs),
}

/// `nord live`: the verbs that mean anything for the live buffer.
///
/// The live buffer is the panel as it stands, not a library — there is nothing to name,
/// nothing to delete, and `select` is what the *other* classes do to it. What is left is
/// the read-only subset, spelled exactly as [`SlotAction`] spells it, plus `edit`,
/// whose output stops at a file.
#[derive(Subcommand)]
enum LiveAction {
    #[command(flatten)]
    Slot(LiveSlotAction),

    /// Change fields inside a live slot, in a `.ne5l` file.
    ///
    /// The live buffer is the program body under another tag, so the fields are exactly
    /// `nord program edit`'s. A slot target is read off the instrument but never
    /// written back — the edit needs `-o`.
    Edit(EditArgs),
}

/// `nord settings`: the instrument holds exactly one of these, so there is nothing to
/// move, copy, name or delete, and `edit` is the whole verb set. The class-generic
/// verbs remain reachable as `raw --class 7`.
#[derive(Subcommand)]
enum SettingsAction {
    /// Change fields inside the global settings, in a `.ne5s` file.
    ///
    /// Fields are the menu settings plus the `startup_*` state the instrument restores
    /// at power-up; `--fields` lists them. A slot target is read off the instrument but
    /// never written back — the edit needs `-o`.
    Edit(EditArgs),
}

/// The [`SlotAction`] verbs the live buffer keeps, spelled identically.
#[derive(Subcommand)]
enum LiveSlotAction {
    /// Read a live slot off the instrument, or a `.ne5l` file. Read-only.
    ///
    /// Prints a summary by default; with `--out` writes the file instead.
    Get {
        /// Slot to read: 1:1, 1:2 or 1:3 — or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,

        /// Write the object to this file instead of printing a summary. With `--sweep`,
        /// the directory every capture lands in.
        #[arg(short, long, value_name = "FILE|DIR")]
        out: Option<PathBuf>,

        /// Save the wire body verbatim instead of wrapping it in a CBIN header.
        /// Needs `--out`.
        #[arg(long)]
        body: bool,

        /// Read the slot over and over, once per prompt, into the `--out` directory.
        ///
        /// The live slot is the panel itself, so this captures a change-one-knob corpus
        /// without saving a program between steps.
        #[arg(long, requires = "out")]
        sweep: bool,
    },

    /// Report everything the instrument knows about a live slot, or a `.ne5l` file's
    /// header. Read-only.
    Info {
        /// Slot to describe: 1:1, 1:2 or 1:3 — or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },

    /// List the piano/sample library objects the live panel depends on. Read-only.
    Deps {
        /// Slot to inspect: 1:1, 1:2 or 1:3 — or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },
}

/// The verb vocabulary, identical for every object class.
#[derive(Subcommand)]
enum SlotAction {
    /// Read an object off the instrument, or from a file. Read-only.
    ///
    /// Prints a summary by default; with `--out` writes the file instead.
    Get {
        /// Slot to read, e.g. 7:4, or a file to read with no instrument attached.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,

        /// Write the object to this file instead of printing a summary. With `--sweep`,
        /// the directory every capture lands in.
        #[arg(short, long, value_name = "FILE|DIR")]
        out: Option<PathBuf>,

        /// Save the wire body verbatim instead of wrapping it in a CBIN header. For
        /// classes whose header layout is not yet known, where wrapping it would
        /// fabricate a wrong file. On a file, strips the header instead. Needs `--out`.
        #[arg(long)]
        body: bool,

        /// Read the slot over and over, once per prompt, into the `--out` directory.
        ///
        /// Change one thing on the instrument, say what you changed, and that capture is
        /// filed under your answer; repeat until a blank line. For building the
        /// one-field-at-a-time corpus that pins down where a field lives.
        #[arg(long, requires = "out")]
        sweep: bool,
    },

    /// Write a file into a slot, OVERWRITING it. Requires --yes.
    Put {
        /// The file to send.
        file: PathBuf,

        /// Destination slot, e.g. 7:4.
        #[arg(value_name = "BANK:SLOT")]
        at: String,

        /// Confirm the overwrite. Without this the command stops after reporting what
        /// currently occupies the slot.
        #[arg(long)]
        yes: bool,
    },

    /// Move an object between slots. OVERWRITES the destination. Requires --yes.
    Move {
        /// Source slot, e.g. 8:13.
        #[arg(value_name = "FROM")]
        from: String,

        /// Destination slot, e.g. 7:16.
        #[arg(value_name = "TO")]
        to: String,

        #[arg(long)]
        yes: bool,
    },

    /// Rename the object in a slot. Requires --yes.
    Rename {
        /// Slot to rename, e.g. 6:13.
        #[arg(value_name = "BANK:SLOT")]
        at: String,

        /// The new name.
        name: String,

        #[arg(long)]
        yes: bool,
    },

    /// Duplicate an object into another slot (device-internal deep copy). Requires --yes.
    Duplicate {
        /// Source slot, e.g. 7:2.
        #[arg(value_name = "FROM")]
        from: String,

        /// Destination slot, e.g. 7:3.
        #[arg(value_name = "TO")]
        to: String,

        #[arg(long)]
        yes: bool,
    },

    /// Delete one or more slots. Requires --yes.
    Delete {
        /// Slots to delete, e.g. 7:50 (repeatable).
        #[arg(value_name = "BANK:SLOT", required = true)]
        slots: Vec<String>,

        #[arg(long)]
        yes: bool,
    },

    /// Load an object live on the instrument (double-click in NSM). Non-destructive.
    Select {
        /// Slot to load, e.g. 2:12.
        #[arg(value_name = "BANK:SLOT")]
        at: String,
    },

    /// Report everything the instrument knows about one slot, or a file's header.
    /// Read-only.
    ///
    /// Shows the fields the CBIN header carries but the wire never transmits — format
    /// tag, version, CRC-32 — plus the slot name, which no file stores at all.
    Info {
        /// Slot to describe, e.g. 7:4, or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },

    /// List the piano/sample library objects an entity depends on. Read-only.
    ///
    /// A file yields the stored ids alone; the slot form asks the instrument, which
    /// attaches the names.
    Deps {
        /// Slot to inspect, e.g. 7:3, or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },
}

#[derive(Args)]
pub struct EditArgs {
    /// A file (`.ne5p` under `nord program`, `.ne5l` under `nord live`, `.ne5s` under
    /// `nord settings`), or a slot on the instrument (`7:4`). A slot makes this a
    /// read-modify-write over USB, so it is a mutation and obeys `--yes`. Omit it to
    /// start from a fresh default, which then needs `-o`.
    #[arg(
        value_name = "FILE|BANK:SLOT",
        required_unless_present_any = ["fields", "out"],
    )]
    pub target: Option<String>,

    /// `path=value`, repeatable. Paths are `nord-format`'s field names.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub set: Vec<String>,

    /// Report what would change — including which bytes — and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// List every settable field with its placement and current value, then exit. With
    /// no target, lists the fields of a fresh default.
    #[arg(long)]
    pub fields: bool,

    /// Write the edit here instead of over the input file.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Confirm the write. Editing a slot, or a file in place, needs it.
    #[arg(long)]
    pub yes: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = Ui::new(cli.color);

    let result = match cli.command {
        Command::Inspect { files, raw } => inspect(&ui, &files, raw),
        Command::Verify { files } => verify(&ui, &files),
        Command::Device { action } => match action {
            DeviceAction::Status { replay, json } => {
                let source = match replay {
                    Some(path) => device::Source::Replay(path),
                    None => device::Source::Usb,
                };
                device::status(&ui, source, json)
            }
            DeviceAction::Info => device::info(&ui),
        },
        Command::Program { action } => match action {
            ProgramAction::Slot(action) => slot_action(&ui, action, ObjectClass::Program),
            ProgramAction::Edit(args) => edit::run(&ui, args, ObjectClass::Program),
        },
        Command::Sample { action } => match action {
            SampleAction::Slot(action) => slot_action(&ui, action, ObjectClass::Sample),
            SampleAction::Edit(args) => sample::run(&ui, args),
        },
        Command::Setlist { action } => slot_action(&ui, action, ObjectClass::SetList),
        Command::Live { action } => match action {
            LiveAction::Slot(action) => slot_action(&ui, action.into(), ObjectClass::Live),
            LiveAction::Edit(args) => edit::run(&ui, args, ObjectClass::Live),
        },
        Command::Settings { action } => match action {
            SettingsAction::Edit(args) => edit::run(&ui, args, ObjectClass::Settings),
        },
        Command::Raw { class, action } => slot_action(&ui, action, ObjectClass::from_raw(class)),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            ui.note(format!("{}: {e}", ui.danger("error")));
            ExitCode::FAILURE
        }
    }
}

impl From<LiveSlotAction> for SlotAction {
    fn from(action: LiveSlotAction) -> SlotAction {
        match action {
            LiveSlotAction::Get {
                at,
                out,
                body,
                sweep,
            } => SlotAction::Get {
                at,
                out,
                body,
                sweep,
            },
            LiveSlotAction::Info { at } => SlotAction::Info { at },
            LiveSlotAction::Deps { at } => SlotAction::Deps { at },
        }
    }
}

/// Dispatch one verb against a fixed object class, whichever noun asked for it.
///
/// The read-only verbs take a file as well as a slot ([`slot::Target`]); the rest name
/// device storage, which no file stands in for.
fn slot_action(ui: &Ui, action: SlotAction, class: ObjectClass) -> Result<(), String> {
    match action {
        SlotAction::Get {
            at,
            out,
            body,
            sweep,
        } => match slot::target(&at)? {
            slot::Target::File(path) if sweep => Err(format!(
                "--sweep re-reads the instrument as the panel changes; {} has only one state",
                path.display()
            )),
            slot::Target::File(path) => file::get(ui, &path, out, class, body),
            slot::Target::Slot(at) => match (sweep, out) {
                (true, Some(dir)) => device::sweep(ui, at, dir, class, body),
                (true, None) => Err("--sweep fills a directory; give -o a path".into()),
                (false, out) => device::get(ui, at, out, class, body),
            },
        },
        SlotAction::Put { file, at, yes } => device::put(ui, file, slot::parse(&at)?, class, yes),
        SlotAction::Move { from, to, yes } => {
            device::move_object(ui, slot::parse(&from)?, slot::parse(&to)?, class, yes)
        }
        SlotAction::Rename { at, name, yes } => {
            device::rename(ui, slot::parse(&at)?, name, class, yes)
        }
        SlotAction::Duplicate { from, to, yes } => {
            device::duplicate(ui, slot::parse(&from)?, slot::parse(&to)?, class, yes)
        }
        SlotAction::Delete { slots, yes } => {
            device::delete(ui, &slot::parse_all(&slots)?, class, yes)
        }
        SlotAction::Select { at } => device::select(ui, slot::parse(&at)?, class),
        SlotAction::Info { at } => match slot::target(&at)? {
            slot::Target::File(path) => file::info(ui, &path, class),
            slot::Target::Slot(at) => device::slot_info(ui, at, class),
        },
        SlotAction::Deps { at } => match slot::target(&at)? {
            slot::Target::File(path) => file::deps(ui, &path, class),
            slot::Target::Slot(at) => device::deps(ui, at, class),
        },
    }
}

fn inspect(ui: &Ui, files: &[PathBuf], raw: bool) -> Result<(), String> {
    let mut failed = 0usize;
    for (i, path) in files.iter().enumerate() {
        if i > 0 {
            ui.out("");
        }
        ui.out(path.display());
        match nord_format::from_path(path) {
            Ok(entity) if raw => ui.out(format!("{entity:#?}")),
            Ok(entity) => summary::print(ui, &entity),
            Err(e) => {
                ui.note(format!("  error: {e}"));
                failed += 1;
            }
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} file(s) did not parse", files.len())),
    }
}

/// Parse each file and re-emit it, checking the bytes come back identical.
///
/// Reports the offset of the first difference, which in a bit-packed format is usually
/// enough to name the field on its own.
fn verify(ui: &Ui, files: &[PathBuf]) -> Result<(), String> {
    let mut failed = 0usize;
    for path in files {
        let original = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                ui.out(format!("error  {} ({e})", path.display()));
                failed += 1;
                continue;
            }
        };
        let reencoded =
            nord_format::from_path(path).and_then(|entity| nord_format::to_bytes(&entity));
        match reencoded {
            Ok(bytes) if bytes == original => {
                ui.out(format!(
                    "ok     {} ({} bytes)",
                    path.display(),
                    original.len()
                ));
            }
            Ok(bytes) => {
                failed += 1;
                let at = bytes
                    .iter()
                    .zip(&original)
                    .position(|(a, b)| a != b)
                    .map(|i| format!("{i:#x}"))
                    .unwrap_or_else(|| "the end (length differs)".to_string());
                ui.out(format!(
                    "DIFFER {} (in {} bytes, out {}; first difference at {at})",
                    path.display(),
                    original.len(),
                    bytes.len(),
                ));
            }
            Err(e) => {
                failed += 1;
                ui.out(format!("error  {} ({e})", path.display()));
            }
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} file(s) did not round-trip", files.len())),
    }
}
