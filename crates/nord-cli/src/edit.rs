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

use std::path::{Path, PathBuf};

use nord_format::cbin::Generation;
use nord_format::fields::{ControlKind, Field, Registry, Unit};
use nord_format::formats::ne5;
use nord_format::{Entity, Live, Program, Settings, Song};
use nord_usb::ObjectClass;

use crate::editors::{self, Fields, Row, PATH_WIDTH};
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

    let named = |what: String| match &target {
        Some(Target::File(path)) => format!("{}: {what}", path.display()),
        Some(Target::Slot(at)) => format!("{}: {what}", crate::slot::addr(*at)),
        None => format!("a fresh {}: {what}", crate::slot::noun(class)),
    };
    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| named(e.to_string()))?;
    let what = match (&entity, class) {
        (Entity::Program(Program::Electro5(_)), ObjectClass::Program) => "the edited program",
        (Entity::Live(Live::Electro5(_)), ObjectClass::Live) => "the edited live slot",
        (Entity::Settings(Settings::Electro5(_)), ObjectClass::Settings) => "the edited settings",
        (Entity::Song(Song::Electro5(_)), ObjectClass::SetList) => "the edited set list",
        _ => return Err(mismatch(&mut entity, class)),
    };
    let staged = editors::stage(
        ui,
        args.common.fields,
        &args.common.set,
        editor_for(&mut entity)?.as_mut(),
    )?;
    let Some(changed) = staged else {
        return Ok(());
    };
    if changed == 0 {
        ui.note("no field changed; writing nothing");
        return Ok(());
    }

    let edited = nord_format::to_bytes(&entity).map_err(|e| e.to_string())?;
    print_byte_diff(ui, &original, &edited);

    if args.common.dry_run {
        ui.note("--dry-run: nothing written");
        return Ok(());
    }

    match (target, args.common.out) {
        (Some(Target::File(path)), out) => write_edit(ui, &path, out, args.common.yes, &edited),
        // An explicit destination is the unambiguous case, whatever the source was.
        (_, Some(out)) => write_file(ui, &out, &edited),
        // The slot keeps whatever it is already called, so the write carries no name.
        (Some(Target::Slot(at)), None) => {
            crate::device::send(ui, &edited, at, class, args.common.yes, what, None, None)
        }
        (None, None) => {
            Err("editing a fresh default needs -o: there is nothing to write back to".into())
        }
    }
}

/// The flags an `edit` takes wherever it is reached from: the nouns, the file verb,
/// and the accessor-backed editors under both.
#[derive(clap::Args)]
pub struct SetArgs {
    /// `path=value`, repeatable. Paths are what `--fields` lists.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub set: Vec<String>,

    /// Report what would change — including which bytes — and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// List every settable field with its current value, then exit.
    #[arg(long)]
    pub fields: bool,

    /// Write the edit here instead of over the input file.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Confirm the write. Editing a slot, or a file in place, needs it.
    #[arg(long)]
    pub yes: bool,
}

/// What sets this entity's fields, or `None` where nothing in it is settable.
///
/// The one dispatch: [`editable`], the noun edits and the file verb all read it, so a
/// body cannot be editable under one and not the other.
pub(crate) fn editor(entity: &mut Entity) -> Option<Box<dyn Fields + '_>> {
    if entity.registry().is_some() {
        return entity
            .registry_mut()
            .map(|r| Box::new(Registered(r)) as Box<dyn Fields>);
    }
    match entity {
        Entity::Song(Song::Electro5(song)) => Some(Box::new(editors::SongEditor(song))),
        Entity::Sample(sample) => Some(Box::new(editors::SampleEditor(sample))),
        Entity::SampleProject(project) => Some(Box::new(editors::ProjectEditor(project))),
        _ => None,
    }
}

/// Whether anything in this entity is settable.
pub(crate) fn editable(entity: &mut Entity) -> bool {
    editor(entity).is_some()
}

/// [`editor`], with the refusal a caller with nothing to edit has to print.
pub(crate) fn editor_for(entity: &mut Entity) -> Result<Box<dyn Fields + '_>, String> {
    let id = entity.identity();
    editor(entity).ok_or_else(|| {
        format!(
            "nothing in a {} ({}) is settable yet; `nord inspect` still reads it",
            id.kind, id.format,
        )
    })
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
        ObjectClass::SetList => Entity::Song(Song::Electro5(
            ne5::song::new(
                (0, 0).try_into().map_err(first)?,
                ne5::song::DEFAULT_VERSION,
                [(0, 0).try_into().map_err(first)?; 4],
            )
            .map_err(|e| e.to_string())?,
        )),
        other => return Err(format!("edit does not exist for {}", other.label())),
    };
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// The target decoded, but not to what this noun edits.
pub(crate) fn mismatch(entity: &mut Entity, class: ObjectClass) -> String {
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
fn steer(entity: &mut Entity) -> String {
    match crate::file::noun(crate::file::entity_tag(entity)) {
        Some(noun) => format!(" — try `nord {noun} edit`"),
        // Everything else editable — the Stage bodies, the Sample Editor
        // project — has no noun of its own and lives under the file verb.
        None if editable(entity) => " — try `nord edit`".to_string(),
        None => String::new(),
    }
}

/// The generated registry as one more set of [`Fields`], so a declared field and an
/// accessor-backed one are staged, listed and reported by the same code.
pub(crate) struct Registered<'a>(pub &'a mut dyn Registry);

impl Registered<'_> {
    fn row(f: &Field) -> Row {
        // A field too wide to enumerate lists no values; its stored bits are the
        // spelling, and the current one is already in the value column.
        let accepts = match (f.spec.legal)() {
            v if v.is_empty() => "stored bits, decimal or 0x…".to_string(),
            v if v.len() > 12 => format!("{} .. {}", v.first().unwrap(), v.last().unwrap()),
            v => v.join(", "),
        };
        Row {
            path: f.path.clone(),
            value: f.value.clone(),
            accepts,
        }
    }
}

impl Fields for Registered<'_> {
    fn rows(&self) -> Result<Vec<Row>, String> {
        Ok(self.0.fields().iter().map(Registered::row).collect())
    }

    fn set(&mut self, path: &str, value: &str) -> Result<(), String> {
        self.0.set_field(path, value).map_err(|e| e.to_string())
    }

    fn list(&self, ui: &Ui) -> Result<(), String> {
        ui.out(format!(
            "{:<PATH_WIDTH$} {:<12} {:<14} {:<28} {}",
            "path", "bits", "control", "value", "accepts"
        ));
        for f in self.0.fields() {
            let row = Registered::row(&f);
            let value = if f.value == f.display {
                row.value
            } else {
                format!("{} {}", f.value, ui.dim(&f.display))
            };
            ui.out(format!(
                "{:<PATH_WIDTH$} {:<12} {:<14} {value:<28} {}",
                row.path,
                f.spec.placement,
                ui.dim(control(f.spec.control)),
                row.accepts,
            ));
        }
        Ok(())
    }
}

/// Write an edit of `path`: to `out`, or over `path` itself.
///
/// ⚠️ `-o` naming the file being edited is an in-place overwrite however it is
/// spelled, so it takes the in-place guard rather than the unguarded write.
pub(crate) fn write_edit(
    ui: &Ui,
    path: &Path,
    out: Option<PathBuf>,
    yes: bool,
    bytes: &[u8],
) -> Result<(), String> {
    match out {
        Some(out) if !same_file(path, &out) => write_file(ui, &out, bytes),
        _ => {
            ui.note(format!(
                "about to {} {} in place",
                ui.danger("overwrite"),
                path.display()
            ));
            ui.confirm(yes)?;
            write_file(ui, path, bytes)
        }
    }
}

/// Whether two paths name one file on disk, with links and `..` resolved.
///
/// Only a path that exists canonicalizes, which is the answer wanted here: a
/// destination that is not there yet cannot be the file being edited.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Now, as the 32-bit Unix seconds count the wire protocol and the Sample Editor's
/// `m_modifyDate` both stamp a write with.
pub(crate) fn unix_seconds_now() -> Result<u32, String> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock is before the Unix epoch: {e}"))?;
    u32::try_from(elapsed.as_secs())
        .map_err(|_| "the current time does not fit a 32-bit Unix timestamp".to_string())
}

pub(crate) fn write_file(ui: &Ui, path: &Path, bytes: &[u8]) -> Result<(), String> {
    replace_file(path, bytes)?;
    ui.note(format!("wrote {} ({} bytes)", path.display(), bytes.len()));
    Ok(())
}

/// Put `bytes` at `path`, leaving whatever was there untouched if that cannot be done.
///
/// Every write this CLI makes goes through here, so a destination under a directory that
/// does not exist yet is created rather than refused, wherever the bytes came from.
///
/// ⚠️ The bytes land in a sibling file that is then renamed over the target, because an
/// edit reads its own destination: a write that truncates first and fails part way
/// through leaves neither the original nor the edit. The temporary is a sibling so the
/// rename stays inside one filesystem, where it replaces the target in one step.
///
/// ⚠️ A rename replaces the name it is given, so a destination that exists is resolved
/// first: editing through a symlink rewrites the file it points at and leaves the link,
/// and the replacement carries the permissions the target already had rather than the
/// umask default a new file would get.
pub(crate) fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = target
        .file_name()
        .ok_or_else(|| format!("{}: not a file to write", path.display()))?;
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut temp = name.to_os_string();
    temp.push(format!(".nord{}.tmp", std::process::id()));
    let temp = target.with_file_name(temp);
    let existing = std::fs::metadata(&target).ok().map(|m| m.permissions());

    let failed = |e: std::io::Error| {
        let _ = std::fs::remove_file(&temp);
        format!("{}: {e}", path.display())
    };
    std::fs::write(&temp, bytes).map_err(failed)?;
    if let Some(permissions) = existing {
        std::fs::set_permissions(&temp, permissions).map_err(failed)?;
    }
    std::fs::rename(&temp, &target).map_err(failed)
}

/// ⚠️ Fields that do nothing without a companion. The pairing is a fact about the
/// instrument, not something the declaration carries.
///
/// Transpose: neither half answers on its own, and the enable is sticky once set — see
/// `ne5::program::center::CenterPanel::transpose_enabled`, which carries the evidence.
/// So `--set center_panel.transpose=0` alone leaves a program the panel still calls
/// transposed. Warn rather than refuse — setting one half deliberately is legitimate.
const STICKY_PAIRS: [(&str, &str); 1] =
    [("center_panel.transpose", "center_panel.transpose_enabled")];

pub(crate) fn warn_on_sticky_pairs(ui: &Ui, sets: &[String]) {
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

/// Where a CBIN file keeps its checksum and what to call it, or `None` for bytes that
/// are not a CBIN file.
///
/// The generation comes from the header parser rather than a second reading of the
/// word at `0x04`, and the range from [`crate::file::checksum_range`], so the CLI holds
/// one account of where a checksum sits.
fn checksum_bytes(file: &[u8]) -> Option<(std::ops::Range<usize>, &'static str)> {
    let header = nord_usb::envelope::unwrap(file).ok()?.header;
    let at = crate::file::checksum_range(header.generation, file.len())?;
    Some(match header.generation {
        Generation::V0 => (at, "  (file crc16)"),
        Generation::V1 => (at, "  (body crc32)"),
    })
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
        // A whole register in one field reads as its bar count; a single bar reads as
        // where in the register the declaration places it.
        ControlKind::Drawbar { bars, rank, .. } => match (bars, rank) {
            (1, Some(rank)) => format!("drawbar {rank}"),
            (1, None) => "drawbar".to_string(),
            (bars, _) => format!("{bars} drawbars"),
        },
        ControlKind::Morph { .. } => "morph".to_string(),
        ControlKind::Pattern { steps, .. } => format!("pattern {steps}"),
        ControlKind::Reference(library) => format!("{} ref", library.label()),
        ControlKind::Number => "number".to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A wrong-format target must steer to the noun whose `edit` reads it — and for a
    /// format with no noun, to the file verb — never to a command that does not exist.
    #[test]
    fn a_mismatched_target_steers_to_the_command_that_edits_it() {
        let mut live = Entity::Live(Live::Electro5(ne5::live::new((0, 0).try_into().unwrap())));
        let err = mismatch(&mut live, ObjectClass::Program);
        assert!(err.contains("nord live edit"), "{err}");

        let mut program = Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().unwrap(),
        )));
        let err = mismatch(&mut program, ObjectClass::Live);
        assert!(err.contains("nord program edit"), "{err}");

        let mut settings = Entity::Settings(Settings::Electro5(ne5::settings::new()));
        let err = mismatch(&mut settings, ObjectClass::Program);
        assert!(err.contains("nord settings edit"), "{err}");

        let mut song = Entity::Song(Song::Electro5(
            ne5::song::new(
                (0, 0).try_into().unwrap(),
                ne5::song::DEFAULT_VERSION,
                [(0, 0).try_into().unwrap(); 4],
            )
            .unwrap(),
        ));
        let err = mismatch(&mut song, ObjectClass::Program);
        assert!(err.contains("nord setlist edit"), "{err}");

        // A registry body with no noun of its own goes to the file verb.
        let mut stage = nord_format::from_stream(&mut std::io::Cursor::new(
            crate::file_edit::tests::stage3_program(),
        ))
        .unwrap();
        let err = mismatch(&mut stage, ObjectClass::Program);
        assert!(err.contains("nord edit"), "{err}");

        // A piano library has no edit anywhere, so no steer may be invented.
        let mut pipe = nord_format::from_stream(&mut std::io::Cursor::new(pipe_library())).unwrap();
        assert_eq!(steer(&mut pipe), "");
    }

    /// Every editable shape answers, and the stubs say no — the one dispatch the
    /// file verb, the noun edits and the steers all rest on.
    #[test]
    fn editable_knows_every_shape() {
        let mut stage3 = nord_format::from_stream(&mut std::io::Cursor::new(
            crate::file_edit::tests::stage3_program(),
        ))
        .unwrap();
        assert!(editable(&mut stage3));

        let mut song = Entity::Song(Song::Electro5(
            ne5::song::new(
                (0, 0).try_into().unwrap(),
                ne5::song::DEFAULT_VERSION,
                [(0, 0).try_into().unwrap(); 4],
            )
            .unwrap(),
        ));
        assert!(editable(&mut song));

        let mut project = Entity::SampleProject(
            nord_format::formats::nsmpproj::Project::new(
                "X",
                &[nord_format::formats::nsmpproj::NewZone {
                    path: "x.wav".into(),
                    sample_rate: 44100,
                    frames: 44100,
                    root_key: 60,
                }],
                0,
            )
            .unwrap(),
        );
        assert!(editable(&mut project));

        let mut stub = nord_format::from_stream(&mut std::io::Cursor::new(pipe_library())).unwrap();
        assert!(!editable(&mut stub));
    }

    pub(crate) fn scratch(what: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nord-{what}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An edit's destination is usually its own source, so a write that cannot
    /// finish has to leave that file as it was rather than truncated — and must not
    /// leave its own temporary behind either.
    #[test]
    fn a_write_that_cannot_finish_leaves_its_directory_as_it_was() {
        let dir = scratch("write-fails");
        let blocked = dir.join("out.ne5p");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("held"), b"kept").unwrap();

        assert!(replace_file(&blocked, b"edited").is_err());
        assert_eq!(
            std::fs::read(blocked.join("held")).unwrap(),
            b"kept".to_vec()
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    /// A destination under a directory that is not there yet is made, not refused:
    /// every verb that writes reaches this, and they used to disagree about it.
    #[test]
    fn a_write_makes_the_directory_its_destination_names() {
        let dir = scratch("write-makes-dirs");
        let nested = dir.join("wavs").join("zone1.wav");
        replace_file(&nested, b"edited").unwrap();
        assert_eq!(std::fs::read(&nested).unwrap(), b"edited".to_vec());
    }

    #[test]
    fn a_write_over_an_existing_file_replaces_the_whole_of_it() {
        let dir = scratch("write-replaces");
        let path = dir.join("out.ne5p");
        std::fs::write(&path, b"the longer file that was here before").unwrap();
        replace_file(&path, b"edited").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"edited".to_vec());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_write_through_a_symlink_edits_the_file_it_points_at_and_keeps_the_link() {
        let dir = scratch("write-symlink");
        let target = dir.join("preset.ne5p");
        std::fs::write(&target, b"original").unwrap();
        let link = dir.join("linked.ne5p");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        replace_file(&link, b"edited").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"edited".to_vec());
        let kind = std::fs::symlink_metadata(&link).unwrap().file_type();
        assert!(kind.is_symlink(), "{} is no longer a link", link.display());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_write_over_an_existing_file_keeps_its_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("write-keeps-mode");
        let path = dir.join("private.ne5p");
        std::fs::write(&path, b"original").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        replace_file(&path, b"edited").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} came back as {mode:o}", path.display());
    }

    #[cfg(unix)]
    #[test]
    fn a_write_into_a_directory_that_refuses_it_leaves_the_original_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("write-refused");
        let held = dir.join("held");
        std::fs::create_dir(&held).unwrap();
        let target = held.join("preset.ne5p");
        std::fs::write(&target, b"original").unwrap();
        let link = dir.join("linked.ne5p");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o500)).unwrap();

        let err = replace_file(&link, b"edited").unwrap_err();

        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(err.contains("linked.ne5p"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"original".to_vec());
        assert_eq!(std::fs::read_dir(&held).unwrap().count(), 1);
    }

    /// `-o` pointing back at the input is an overwrite of the file being edited, so
    /// it has to meet the guard that spelling it with no `-o` meets.
    #[test]
    fn an_output_that_is_the_input_takes_the_in_place_guard() {
        let dir = scratch("edit-in-place");
        let path = dir.join("p.ne5p");
        let original = fresh(ObjectClass::Program).unwrap();
        std::fs::write(&path, &original).unwrap();
        let spelled = dir.join(".").join("p.ne5p");

        let args = EditArgs {
            target: Some(path.display().to_string()),
            common: SetArgs {
                set: vec!["center_panel.gain=64".into()],
                dry_run: false,
                fields: false,
                out: Some(spelled),
                yes: false,
            },
        };
        let err = run(&Ui::piped(), args, ObjectClass::Program).unwrap_err();
        assert!(err.contains("--yes"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    /// A target that does not decode says which target it was, as every other file
    /// error in the CLI does.
    #[test]
    fn a_target_that_does_not_decode_is_named_in_the_error() {
        let dir = scratch("edit-undecodable");
        let path = dir.join("junk.ne5p");
        std::fs::write(&path, b"not a CBIN file at all").unwrap();

        let args = EditArgs {
            target: Some(path.display().to_string()),
            common: SetArgs {
                set: vec!["center_panel.gain=64".into()],
                dry_run: false,
                fields: false,
                out: None,
                yes: false,
            },
        };
        let err = run(&Ui::piped(), args, ObjectClass::Program).unwrap_err();
        assert!(err.contains("junk.ne5p"), "{err}");
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

    /// The control column is this listing's reading of the registry's vocabulary, and
    /// what a kind carries has to reach the reader — a bar's place in the register, a
    /// pattern's length, which library resolves an id.
    #[test]
    fn a_control_reads_as_what_the_field_registry_says_it_is() {
        use nord_format::bits::Packed;
        use nord_format::components::{ArpPattern, Drawbar, Level, PianoRef, SampleRef};
        use nord_format::fields::Library;

        let kind = |c: ControlKind| control(c);
        assert_eq!(kind(<Level as Packed>::CONTROL), "knob 0-10");
        assert_eq!(kind(<Drawbar as Packed>::CONTROL), "drawbar");
        assert_eq!(kind(<Drawbar as Packed>::CONTROL.ranked(3)), "drawbar 3");
        assert_eq!(kind(<ArpPattern as Packed>::CONTROL), "pattern 16");
        assert_eq!(kind(<PianoRef as Packed>::CONTROL), "piano ref");
        assert_eq!(kind(<SampleRef as Packed>::CONTROL), "sample ref");
        assert_eq!(
            kind(ControlKind::Reference(Library::SetList)),
            "set list ref"
        );
        // A whole register in one field counts its bars rather than naming a position.
        assert_eq!(
            kind(<nord_format::formats::ne5::Drawbars as Packed>::CONTROL),
            "9 drawbars"
        );
    }
}
