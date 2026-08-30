//! The operations that talk to an attached instrument.
//!
//! Each one is parameterised by object class: the same code drives a program and a set
//! list, differing only in the class the session opened.
//!
//! Read-only queries (`status`, `get`, `info`, `deps`) and the non-destructive `select`
//! need no confirmation; the mutating actions (`put`, `move`, `delete`, `rename`,
//! `duplicate`) each describe what they will touch and then refuse to proceed without
//! `--yes`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nord_usb::op;
use nord_usb::transport::Transport;
use nord_usb::wire::{Location, ProgramInfo, Status};
use nord_usb::{op as usb_op, ObjectClass, Session};

use crate::slot::{addr, noun, shown};
use crate::ui::Ui;

/// Where to get the exchange from.
pub enum Source {
    /// A real instrument over USB.
    Usb,
    /// A recorded exchange. Lets the whole path be demonstrated with no hardware —
    /// and is how this command is exercised under Wine, qemu and in CI.
    Replay(PathBuf),
}

pub fn status(ui: &Ui, source: Source, json: bool) -> Result<(), String> {
    let report = match source {
        Source::Usb => {
            let mut transport = open_usb()?;
            transact(&mut transport, "device status", |t| {
                nord_usb::block_on(op::inventory(t))
            })
            .map_err(|e| e.to_string())?
        }
        Source::Replay(path) => {
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut transport = nord_usb::ReplayTransport::from_script(&text)
                .map_err(|e| e.to_string())?
                .lenient();
            collect(&mut transport)?
        }
    };

    // An empty report means no class answered, not that every class is empty.
    if report.is_empty() {
        return Err(
            "no object class answered — either the instrument is not in a usable \
             session state (a power cycle clears it), or the connection failed. \
             `nord device info` shows what is on the bus."
                .into(),
        );
    }

    if json {
        print_json(ui, &report);
    } else {
        print_table(ui, &report);
    }
    Ok(())
}

/// Report the attached instrument itself: what is on the bus, not what is stored on it.
///
/// Answered entirely from the USB descriptors, so it works before any transaction is
/// opened and is the right first thing to run when nothing else responds.
pub fn info(ui: &Ui) -> Result<(), String> {
    let devices = nord_usb::transport::usb::list().map_err(|e| e.to_string())?;
    if devices.is_empty() {
        return Err("no Clavia device found".into());
    }
    // Remembered rather than returned on the spot: the descriptors below are worth
    // printing for every device even when one of them will not answer.
    let mut unreachable = None;
    for (i, d) in devices.iter().enumerate() {
        if i > 0 {
            ui.out("");
        }
        ui.out(format!(
            "  product:   {}",
            d.product_string().unwrap_or("(none reported)")
        ));
        ui.out(format!(
            "  vendor:    {} ({:#06x})",
            d.manufacturer_string().unwrap_or("(none reported)"),
            d.vendor_id(),
        ));
        ui.out(format!("  product id: {:#06x}", d.product_id()));
        ui.out(format!(
            "  serial:    {}",
            d.serial_number().unwrap_or("(none reported)")
        ));
        // The protocol requires the vendor-specific interface, not USB-MIDI alone.
        let vendor_iface = d.interfaces().any(|i| i.class() == 0xff);
        ui.out(format!(
            "  protocol:  {}",
            if vendor_iface {
                "vendor interface present"
            } else {
                "no vendor interface — this tool cannot drive it"
            }
        ));

        // Endpoint 0, so this needs no transaction and still answers on an instrument
        // that has stopped serving the bulk protocol.
        if vendor_iface {
            match nord_usb::transport::UsbTransport::open(d).and_then(|t| t.identity()) {
                Ok(id) => {
                    ui.out(format!(
                        "  firmware:  {}",
                        crate::summary::version_label(u32::from(id.firmware))
                    ));
                    ui.out(format!("  build:     {}", id.build));
                    ui.out(format!("  max xfer:  {} bytes", id.max_transfer));
                }
                Err(e) => {
                    ui.out(format!("  firmware:  {}", ui.dim(e.to_string())));
                    unreachable.get_or_insert_with(|| e.to_string());
                }
            }
        }
    }
    // This is what a caller runs to find out whether the instrument can be reached at
    // all, so the exit code has to carry that answer and not just the printed lines.
    match unreachable {
        Some(e) => Err(format!("could not identify the instrument: {e}")),
        None => Ok(()),
    }
}

fn collect<T: Transport>(transport: &mut T) -> Result<Vec<Status>, String> {
    nord_usb::block_on(op::inventory(transport)).map_err(|e| e.to_string())
}

fn print_table(ui: &Ui, report: &[Status]) {
    ui.out(ui.dim(format!(
        "{:<10} {:>20} {:>7}  {}",
        "class", "used", "full", "of"
    )));
    let mut any_variable = false;
    for s in report {
        // Fixed-size classes are far clearer as slots than as raw blocks: programs
        // report 400, which is exactly the instrument's 8 banks x 50.
        let (used, of) = match s.slots() {
            Some(slots) => (
                format!("{} / {} slots", s.count, slots),
                format!("{} blocks each", s.blocks_per_item().unwrap_or(0)),
            ),
            None => {
                any_variable = true;
                (
                    format!("{} / {} blocks", s.used, s.total()),
                    format!("{} items", s.count),
                )
            }
        };
        ui.out(format!(
            "{:<10} {:>20} {:>6.1}%  {}",
            s.class.label(),
            used,
            s.used_percent(),
            ui.dim(of),
        ));
    }
    if any_variable {
        ui.note("");
        ui.note("(blocks are a device-internal unit, not bytes)");
    }
}

fn print_json(ui: &Ui, report: &[Status]) {
    ui.out("[");
    for (i, s) in report.iter().enumerate() {
        let comma = if i + 1 == report.len() { "" } else { "," };
        ui.out(format!(
            "  {{\"class\": \"{}\", \"code\": {}, \"items\": {}, \"used\": {}, \"free\": {}, \"capacity\": {}}}{comma}",
            s.class.label(),
            s.class.to_raw(),
            s.count,
            s.used,
            s.free,
            s.total(),
        ));
    }
    ui.out("]");
}

/// Combine an operation's result with its session close, keeping the operation's error
/// when both fail — a close failing is usually a *consequence* of the op failing, and
/// the original error is the informative one.
///
/// ⚠️ **Always call the close, including on the error path.** An abandoned session leaves
/// the instrument mid-transaction, and a read that has already sent its `"Uploading..."`
/// progress label leaves that label on the display with **no way out but a power cycle**
/// — the closing exchanges are what clear it. A `?` between opening a session and
/// committing it is how that happens.
fn finish<T>(
    result: Result<T, nord_usb::Error>,
    closed: Result<(), nord_usb::Error>,
) -> Result<T, nord_usb::Error> {
    match result {
        Ok(v) => closed.map(|()| v),
        Err(e) => Err(e),
    }
}

/// Turn the device's bare status code into something actionable.
///
/// All three confirmed on hardware: `0x1` from a vacant slot, `0x3` from an address
/// past the instrument's geometry, `0x4` from a write aimed at an occupied slot.
fn explain(e: nord_usb::Error, at: Location) -> String {
    match e {
        nord_usb::Error::DeviceStatus(1) => {
            format!("{} is empty", shown(at))
        }
        nord_usb::Error::DeviceStatus(3) => {
            format!("{} is out of range for this instrument", shown(at))
        }
        nord_usb::Error::DeviceStatus(4) => {
            format!(
                "{} is occupied, and the instrument does not overwrite in place",
                shown(at)
            )
        }
        other => other.to_string(),
    }
}

/// [`explain`] for a verb with a source and a destination: an empty slot can only be
/// the source, an occupied one only the destination.
fn explain_pair(e: nord_usb::Error, from: Location, to: Location) -> String {
    match e {
        nord_usb::Error::DeviceStatus(1) => explain(e, from),
        nord_usb::Error::DeviceStatus(4) => explain(e, to),
        nord_usb::Error::DeviceStatus(3) => format!(
            "{} or {} is out of range for this instrument",
            shown(from),
            shown(to)
        ),
        other => other.to_string(),
    }
}

/// Turn a refusal from the enumeration walk into something actionable.
///
/// No slot to name here — the failing command is the walk itself.
fn explain_walk(e: nord_usb::Error) -> String {
    match e {
        nord_usb::Error::DeviceStatus(usb_op::ENUMERATION_DISABLED) => {
            "the instrument refused the enumeration request as malformed (status 0x11) \
             — it refuses a cursor request without the direction word after any write \
             since power-up. nord sends the full form, so this should not happen; \
             per-slot `info` still works in the meantime"
                .into()
        }
        other => other.to_string(),
    }
}

/// Where `--record` writes, for the one transport this process opens.
static RECORDING: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Set once, from the parsed global flag, before any command runs.
pub fn set_recording(path: Option<PathBuf>) {
    let _ = RECORDING.set(path);
}

/// Run one transaction on the instrument, recording what it was for and — if it failed —
/// what it produced.
///
/// A recording is replayable only if the script says both, and they cannot be
/// written at the same moment: the intent goes ahead of the frames, the outcome is only
/// known once they are on disk. A success writes nothing, because a section that says
/// nothing expects `ok`.
fn transact<T>(
    t: &mut nord_usb::transport::UsbTransport,
    intent: impl std::fmt::Display,
    run: impl FnOnce(&mut nord_usb::transport::UsbTransport) -> nord_usb::Result<T>,
) -> nord_usb::Result<T> {
    t.mark_intent(&intent.to_string());
    let outcome = run(t);
    if let Err(e) = &outcome {
        t.mark_expect(e);
    }
    outcome
}

fn open_usb() -> Result<nord_usb::transport::UsbTransport, String> {
    let t = nord_usb::transport::UsbTransport::open_first().map_err(|e| e.to_string())?;
    match RECORDING.get().and_then(Option::as_deref) {
        Some(path) => t.recording_to(path).map_err(|e| e.to_string()),
        None => Ok(t),
    }
}

/// One read in its own session: the slot's metadata, then its bytes.
///
/// `body` returns the wire body verbatim; otherwise the bytes are a whole CBIN file.
fn read_object(
    t: &mut nord_usb::transport::UsbTransport,
    at: Location,
    class: ObjectClass,
    body: bool,
) -> Result<(ProgramInfo, Vec<u8>), String> {
    let verb = if body { "get-body" } else { "get" };
    transact(t, format!("{} {verb} {}", noun(class), addr(at)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = async {
                let info = usb_op::info(&mut s, at).await?;
                let file = if body {
                    usb_op::read_body(&mut s, at).await?
                } else {
                    usb_op::read_program(&mut s, at).await?
                };
                Ok::<_, nord_usb::Error>((info, file))
            }
            .await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    })
    .map_err(|e| explain(e, at))
}

/// Read one object off the instrument. Read-only.
///
/// With `out` set, writes the file; otherwise decodes and prints a summary.
///
/// `body` writes the wire body verbatim instead of wrapping it in a CBIN header, for
/// classes whose header layout is not yet known and where wrapping would fabricate a
/// wrong file.
pub fn get(
    ui: &Ui,
    at: Location,
    out: Option<PathBuf>,
    class: ObjectClass,
    body: bool,
) -> Result<(), String> {
    // Before the transport opens: a piano read is minutes long, and finding out at the
    // end that there was nowhere to put it is the worst possible time.
    if body && out.is_none() {
        return Err("--body writes a file; give -o a path".into());
    }
    let mut t = open_usb()?;
    let (info, file) = read_object(&mut t, at, class, body)?;

    if let Some(path) = out {
        std::fs::write(&path, &file).map_err(|e| format!("{}: {e}", path.display()))?;
        ui.note(format!(
            "read {:?} ({} bytes) from {} -> {}",
            info.name,
            file.len(),
            shown(at),
            path.display(),
        ));
        return Ok(());
    }

    // Parse the bytes just built rather than reporting the wire fields directly, so this
    // runs the same decode path `nord inspect` does.
    let entity = nord_format::from_stream(&mut std::io::Cursor::new(&file)).map_err(|e| {
        format!(
            "{} decoded off the device but did not parse: {e}",
            shown(at)
        )
    })?;

    ui.out(format!(
        "{} {} {:?}  ({}, version {})",
        shown(at),
        ui.dash(),
        info.name,
        info.format,
        info.version
    ));
    crate::summary::print(ui, &entity);
    Ok(())
}

/// Read the same slot once per change the operator makes on the panel, filing each
/// capture under what they say changed. Read-only.
///
/// ⚠️ **The read happens after the answer, not before.** The answer is the operator
/// saying the instrument is now in the state to capture; reading first would file every
/// capture under the change that comes next.
///
/// A failed read is reported and the sweep continues — one fumbled step should not cost
/// the session, and nothing already written is at risk.
///
/// ⚠️ The prompt sits *between* sessions, never inside one. Ctrl-C there ends the process
/// outright ([`Ui::ask`]), and an interrupt taken mid-session would leave the instrument
/// holding its progress label with no way out but a power cycle.
pub fn sweep(
    ui: &Ui,
    at: Location,
    dir: PathBuf,
    class: ObjectClass,
    body: bool,
) -> Result<(), String> {
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut t = open_usb()?;
    ui.note(format!(
        "sweeping {} ({}) into {}",
        shown(at),
        class.label(),
        dir.display()
    ));
    ui.note("change one thing on the instrument, then say what it was");
    ui.note("each prompt reopens with your last answer, editable; clear it to finish");

    let mut captured = 0usize;
    // Reuse the last answer because adjacent captures usually differ by one value.
    let mut previous = String::new();
    while let Some(label) = ui.ask("what changed", &previous)? {
        previous = label.clone();
        let stem = match stem(&label) {
            Ok(s) => s,
            Err(e) => {
                ui.warn(e);
                continue;
            }
        };
        // Refuse duplicates before the slow device read.
        if taken(&dir, &stem)? {
            ui.warn(format!(
                "{stem:?} is already captured; give this one another name"
            ));
            continue;
        }

        let (info, file) = match read_object(&mut t, at, class, body) {
            Ok(read) => read,
            Err(e) => {
                ui.warn(e);
                continue;
            }
        };
        // The extension says what the bytes are: a wrapped file carries the device's own
        // format tag, a `--body` dump is a fragment and no format at all.
        let path = dir.join(match body {
            true => format!("{stem}.bin"),
            false => format!("{stem}.{}", info.format),
        });
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        output
            .write_all(&file)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        captured += 1;
        ui.note(format!("  {} ({} bytes)", path.display(), file.len()));
    }

    ui.note(format!("captured {captured} file(s) in {}", dir.display()));
    Ok(())
}

/// Turn what the operator typed into a filename stem.
///
/// The answer is prose — `split point C4`, `vol 5 -> 6` — and in the corpus directory it
/// is the only record of what the bytes mean, so it stays readable: whitespace runs
/// become one `-`, and only what a path cannot carry is dropped.
fn stem(label: &str) -> Result<String, String> {
    // Defer separators so runs collapse and trailing punctuation disappears.
    let mut owed = false;
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        match c {
            '-' => owed = !out.is_empty(),
            _ if c.is_whitespace() => owed = !out.is_empty(),
            // The corpus must remain readable on Windows.
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => owed = !out.is_empty(),
            _ if c.is_control() => {}
            _ => {
                if std::mem::take(&mut owed) {
                    out.push('-');
                }
                out.push(c);
            }
        }
    }
    // Trim shell-option prefixes and dots that are hidden or invalid across platforms.
    let out = out.trim_matches(['.', '-']);
    if out.is_empty() {
        return Err(format!("{label:?} leaves nothing usable as a filename"));
    }
    let device = out.split('.').next().unwrap_or(out).to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || device
            .strip_prefix("COM")
            .or_else(|| device.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"));
    if reserved {
        return Err(format!("{label:?} is a reserved filename on Windows"));
    }
    Ok(out.to_string())
}

/// Whether a capture under this name already exists, whatever extension it took.
fn taken(dir: &Path, stem: &str) -> Result<bool, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let same = Path::new(&entry.file_name())
            .file_stem()
            .is_some_and(|held| held.to_string_lossy().eq_ignore_ascii_case(stem));
        if same {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Write a file into a slot, overwriting it.
pub fn put(
    ui: &Ui,
    path: PathBuf,
    at: Location,
    class: ObjectClass,
    confirmed: bool,
) -> Result<(), String> {
    let file = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Fail before touching the device if the file is not what it claims to be.
    nord_usb::envelope::unwrap(&file).map_err(|e| e.to_string())?;
    // The write carries the slot's name and the file supplies none: the stem is it.
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty() && !s.starts_with('.'))
        .ok_or_else(|| {
            format!(
                "{}: the slot takes its name from the file's stem, and this file has none",
                path.display()
            )
        })?;
    // The file's own modification time, as NSM sends.
    let stamp = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| u32::try_from(d.as_secs()))
        .transpose()
        .map_err(|_| {
            format!(
                "{}: modification time does not fit the protocol",
                path.display()
            )
        })?;

    send(
        ui,
        &file,
        at,
        class,
        confirmed,
        &path.display().to_string(),
        Some(&stem),
        stamp,
    )
}

/// Run one mutating operation in its own session, committing either way — an abandoned
/// session leaves the instrument mid-transaction with its progress label still painted.
macro_rules! one_shot {
    ($t:expr, $class:expr, |$s:ident| $body:expr) => {
        nord_usb::block_on(async {
            let mut $s = Session::open($t, $class).await?.allow_destructive_writes();
            let r = $body.await;
            let closed = $s.commit().await;
            r.and(closed)
        })
    };
}

/// Send an already-validated file into a slot, describing the target first. Shared with
/// `edit`, which arrives with bytes rather than a path.
///
/// ⚠️ **On most classes an occupied destination is replaced, not overwritten.** The
/// instrument answers status 4 to a write aimed at a slot that already holds something,
/// so this reads the occupant, deletes it, writes, and puts the occupant back if the
/// write fails — the slot is genuinely empty in between, and the only copy of its
/// contents is in this process. The classes that
/// [overwrite in place](ObjectClass::overwrites_in_place) skip the delete and keep the
/// same backup-and-restore guard.
#[allow(clippy::too_many_arguments)]
pub fn send(
    ui: &Ui,
    file: &[u8],
    at: Location,
    class: ObjectClass,
    confirmed: bool,
    what: &str,
    // `None` keeps whatever the slot is already called.
    name: Option<&str>,
    // `None` means now; the device can refuse a timestamp it considers future.
    stamp: Option<u32>,
) -> Result<(), String> {
    let mut t = open_usb()?;

    // Check geometry before the transfer starts.
    let bad = transact(
        &mut t,
        format!("{} check-address {}", noun(class), addr(at)),
        |t| {
            nord_usb::block_on(async {
                let mut s = Session::open(t, class).await?;
                let r = usb_op::check_address(&mut s, at).await;
                let closed = s.commit().await;
                finish(r, closed)
            })
        },
    )
    .map_err(|e| explain(e, at))?;
    if let Some(reason) = bad {
        return Err(format!("{}: {reason}", shown(at)));
    }

    let timestamp = match stamp {
        Some(stamp) => stamp,
        None => {
            let elapsed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| format!("system clock is before the Unix epoch: {e}"))?;
            u32::try_from(elapsed.as_secs())
                .map_err(|_| "system time does not fit the device protocol".to_string())?
        }
    };

    // Name what is about to be destroyed before destroying it. An empty destination is
    // not a failure: status 1 means the slot is vacant, so there is nothing to report.
    let existing = transact(&mut t, format!("{} info {}", noun(class), addr(at)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = usb_op::info(&mut s, at).await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    });

    let existing = match existing {
        Ok(info) => Some(info),
        Err(nord_usb::Error::DeviceStatus(1)) => None,
        Err(e) => return Err(explain(e, at)),
    };

    let in_place = class.overwrites_in_place();
    match &existing {
        Some(info) => {
            ui.note(format!(
                "about to {} {} (currently {:?}) with {what}",
                ui.danger("overwrite"),
                shown(at),
                info.name,
            ));
            let room = match in_place {
                true => format!(
                    "the instrument overwrites {} in place, so nothing is deleted.",
                    shown(at)
                ),
                // The operator is consenting to the slot being empty for a moment, not
                // just to a write, so the delete has to be part of the question.
                false => format!(
                    "{} the instrument will not overwrite in place, so {} is deleted first.",
                    ui.danger("note:"),
                    shown(at),
                ),
            };
            ui.note(format!(
                "  {room} Its {} bytes are read back beforehand and put back if the \
                 write fails.",
                info.body_len,
            ));
        }
        None => ui.note(format!("{} is empty; writing {what}", shown(at))),
    }
    // The write carries the slot's name, so say it up front — and say when the device
    // will drop it, rather than reporting a naming that never happens.
    match name.filter(|n| !n.is_empty()) {
        Some(name) if class.names_its_slots() => {
            ui.note(format!("the slot will be named {name:?}"))
        }
        Some(_) => ui.note(format!(
            "{} keeps its fixed name: this class stores none, so the device discards \
             the write's name argument",
            shown(at),
        )),
        None => {}
    }
    if class == ObjectClass::Settings {
        // Confirmed on hardware.
        ui.warn(
            "a settings write reloads the selected program: panel state that has not \
             been stored is lost, so re-select and re-apply afterwards",
        );
    }
    ui.confirm(confirmed)?;

    // After consent, not before: for a piano this read is minutes long, and nobody should
    // sit through it only to be asked whether they meant it.
    let backup = match &existing {
        Some(_) => Some(
            transact(&mut t, format!("{} read {}", noun(class), addr(at)), |t| {
                nord_usb::block_on(async {
                    let mut s = Session::open(t, class).await?;
                    let r = usb_op::read_program(&mut s, at).await;
                    let closed = s.commit().await;
                    finish(r, closed)
                })
            })
            // Nothing is deleted until the backup is in hand.
            .map_err(|e| {
                format!(
                    "could not read {} back before replacing it, so it was left alone: {}",
                    shown(at),
                    explain(e, at)
                )
            })?,
        ),
        None => None,
    };

    if existing.is_some() && !in_place {
        ui.note(format!("deleting {} to make room", shown(at)));
        transact(
            &mut t,
            format!("{} delete {}", noun(class), addr(at)),
            |t| one_shot!(t, class, |s| usb_op::delete(&mut s, at)),
        )
        .map_err(|e| format!("deleting {}: {}", shown(at), explain(e, at)))?;
    }

    // What the slot ends up called: the caller's choice, else the occupant's name.
    let write_name = name
        .map(str::to_string)
        .or_else(|| existing.as_ref().map(|i| i.name.clone()))
        .unwrap_or_default();

    let written = if fail_after_delete() {
        // The backup is in hand and the write never happens: exactly the state the
        // restore path exists for, reached without needing a real transport failure.
        Err(nord_usb::Error::Transport(
            "NORD_FAIL_AFTER_DELETE was set, so the write was not attempted".into(),
        ))
    } else {
        transact(
            &mut t,
            put_intent(class, what, at, &write_name, timestamp),
            |t| {
                one_shot!(t, class, |s| usb_op::write(
                    &mut s,
                    at,
                    file,
                    &write_name,
                    timestamp
                ))
            },
        )
    };

    match (written, backup) {
        (Ok(()), _) => {
            ui.note(format!("wrote {what} -> {}", shown(at)));
            Ok(())
        }
        (Err(e), None) => Err(e.to_string()),
        // Getting the occupant back matters more than reporting the original error, which
        // is carried along and reported once the slot is whole again.
        (Err(e), Some(backup)) => {
            ui.warn(format!(
                "the write failed and {}; putting the original back",
                aftermath(class, at)
            ));
            // Restoring puts back what the slot was called, not what the caller wanted
            // the replacement named.
            let restore_name = existing
                .as_ref()
                .map(|i| i.name.clone())
                .unwrap_or_else(|| write_name.clone());
            let restore = transact(
                &mut t,
                put_intent(
                    class,
                    &rescue_name(at, &backup),
                    at,
                    &restore_name,
                    timestamp,
                ),
                |t| {
                    one_shot!(t, class, |s| usb_op::write(
                        &mut s,
                        at,
                        &backup,
                        &restore_name,
                        timestamp
                    ))
                },
            );
            match restore {
                Ok(()) => {
                    ui.note(format!("restored {}", shown(at)));
                    Err(format!(
                        "{e} ({} was restored, and is unchanged)",
                        shown(at)
                    ))
                }
                Err(restore) => Err(rescue(
                    ui,
                    at,
                    class,
                    &backup,
                    &e.to_string(),
                    &restore.to_string(),
                )),
            }
        }
    }
}

/// Whether to skip the write and report a failure, so the restore and rescue paths can
/// be exercised against a real instrument. Test tool: a genuine transport failure at this
/// exact point is otherwise only reachable by pulling the cable mid-operation.
///
/// ⚠️ It leaves the slot deleted, so point it at a scratch slot with a copy on disk.
#[cfg(feature = "fault-injection")]
fn fail_after_delete() -> bool {
    std::env::var_os("NORD_FAIL_AFTER_DELETE").is_some()
}

#[cfg(not(feature = "fault-injection"))]
fn fail_after_delete() -> bool {
    false
}

/// Last resort: the write failed, the restore failed, and the slot's former contents
/// exist only in memory. Spill them next to the operator rather than exiting with them.
fn rescue(
    ui: &Ui,
    at: Location,
    class: ObjectClass,
    backup: &[u8],
    write: &str,
    restore: &str,
) -> String {
    let path = std::env::current_dir()
        .unwrap_or_default()
        .join(rescue_name(at, backup));
    match std::fs::write(&path, backup) {
        Ok(()) => {
            ui.warn(format!(
                "restore failed too; wrote the original to {}",
                path.display()
            ));
            format!(
                "{write} (restoring failed as well: {restore}) {}; \
                 its former contents were saved to {} — put it back with `nord put`",
                aftermath(class, at),
                path.display(),
            )
        }
        Err(io) => format!(
            "{write} (restoring failed as well: {restore}) {} and its former \
             contents could not be saved either ({io}); {} bytes are lost",
            aftermath(class, at),
            backup.len(),
        ),
    }
}

/// What a failed write left in the slot, for the line that reports it.
///
/// The two write paths fail differently: the delete-first composition leaves the slot
/// genuinely empty, while a class that overwrites in place leaves whatever the
/// interrupted write put there. Naming the wrong one tells the operator to take the
/// wrong next step.
fn aftermath(class: ObjectClass, at: Location) -> String {
    match class.overwrites_in_place() {
        true => format!("{} may hold a partly written body", shown(at)),
        false => format!("{} is empty", shown(at)),
    }
}

/// Read one slot's name in a throwaway read-only session — used to show what a mutation
/// is about to affect before it happens.
fn peek(
    t: &mut nord_usb::transport::UsbTransport,
    class: ObjectClass,
    at: Location,
) -> Result<String, String> {
    transact(t, format!("{} info {}", noun(class), addr(at)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = usb_op::info(&mut s, at).await;
            let closed = s.commit().await;
            finish(r, closed).map(|info| info.name)
        })
    })
    .map_err(|e| explain(e, at))
}

/// What the operation does to whatever occupies the destination slot.
enum DestFate {
    /// Replaced, and lost.
    Overwritten,
    /// Exchanged with the source slot's contents. Nothing is lost.
    Swapped,
}

/// Describe what currently occupies a *destination* slot, for the pre-flight line.
///
/// ⚠️ The two fates need different words, and saying "overwriting" for a swap is worse
/// than saying nothing: it invites the reader to delete the destination first to protect
/// it, which destroys the very thing the swap would have preserved.
///
/// Unlike [`peek`] this never fails: `INFO` errors on an empty destination, which is the
/// normal case here. A real transport fault surfaces on the operation itself a moment
/// later.
fn peek_dest(
    ui: &Ui,
    t: &mut nord_usb::transport::UsbTransport,
    class: ObjectClass,
    at: Location,
    fate: DestFate,
) -> String {
    match (peek(t, class, at), fate) {
        (Ok(name), DestFate::Overwritten) => format!("{} {name:?}", ui.danger("OVERWRITING")),
        (Ok(name), DestFate::Swapped) => format!("{} {name:?}", ui.bold("SWAPPING WITH")),
        (Err(_), _) => "destination reads as empty".into(),
    }
}

/// Move an object from one slot to another. Requires confirmation: it changes both slots,
/// though an occupied destination is swapped rather than destroyed.
pub fn move_object(
    ui: &Ui,
    from: Location,
    to: Location,
    class: ObjectClass,
    confirmed: bool,
) -> Result<(), String> {
    let mut t = open_usb()?;
    let name = peek(&mut t, class, from)?;
    let dest = peek_dest(ui, &mut t, class, to, DestFate::Swapped);
    ui.note(format!(
        "moving {:?} from {} to {} {} {}",
        name,
        shown(from),
        shown(to),
        ui.dash(),
        dest
    ));
    ui.confirm(confirmed)?;
    transact(
        &mut t,
        format!("{} move {} {}", noun(class), addr(from), addr(to)),
        |t| one_shot!(t, class, |s| usb_op::move_object(&mut s, from, to)),
    )
    .map_err(|e| explain_pair(e, from, to))?;
    ui.note(format!("moved {} -> {}", shown(from), shown(to)));
    Ok(())
}

/// Delete one or more slots. Destructive; requires confirmation. All items run in one
/// session, exactly as NSM batches a multi-delete.
pub fn delete(
    ui: &Ui,
    slots: &[Location],
    class: ObjectClass,
    confirmed: bool,
) -> Result<(), String> {
    let mut t = open_usb()?;
    for &at in slots {
        let name = peek(&mut t, class, at)?;
        ui.note(format!(
            "{} {:?} at {}",
            ui.danger("deleting"),
            name,
            shown(at)
        ));
    }
    ui.confirm(confirmed)?;
    let addresses: Vec<String> = slots.iter().map(|&at| addr(at)).collect();
    // Each delete lands on the instrument as it is sent: a failure part-way leaves the
    // earlier ones gone, and the report has to say which.
    let mut done = 0;
    // Which slot the failure was about — carried out of the transaction separately, so
    // the recorder still sees a plain protocol error to write down.
    let mut failed = slots[0];
    let outcome = transact(
        &mut t,
        format!("{} delete {}", noun(class), addresses.join(" ")),
        |t| {
            nord_usb::block_on(async {
                let mut s = Session::open(t, class)
                    .await
                    .map_err(|e| (slots[0], e))?
                    .allow_destructive_writes();
                let mut r = Ok(());
                for &at in slots {
                    r = usb_op::delete(&mut s, at).await.map_err(|e| (at, e));
                    if r.is_err() {
                        break;
                    }
                    done += 1;
                }
                match (r, s.commit().await) {
                    (Err(e), _) => Err(e),
                    (Ok(()), Err(e)) => Err((slots[slots.len() - 1], e)),
                    (Ok(()), Ok(())) => Ok(()),
                }
            })
            .map_err(|(at, e)| {
                failed = at;
                e
            })
        },
    );
    if let Err(e) = outcome {
        let at = failed;
        let gone: Vec<String> = slots[..done].iter().map(|&at| shown(at)).collect();
        return Err(match done {
            0 => format!("deleting {}: {}", shown(at), explain(e, at)),
            _ => format!(
                "deleting {}: {} — {} already deleted ({}); {} left alone",
                shown(at),
                explain(e, at),
                done,
                gone.join(", "),
                slots.len() - done - 1
            ),
        });
    }
    ui.note(format!("deleted {} item(s)", slots.len()));
    Ok(())
}

/// Rename the object in a slot. Destructive; requires confirmation.
pub fn rename(
    ui: &Ui,
    at: Location,
    name: String,
    class: ObjectClass,
    confirmed: bool,
) -> Result<(), String> {
    let mut t = open_usb()?;
    let old = peek(&mut t, class, at)?;
    ui.note(format!(
        "renaming {} from {:?} to {:?}",
        shown(at),
        old,
        name
    ));
    ui.confirm(confirmed)?;
    transact(
        &mut t,
        format!("{} rename {} {name:?}", noun(class), addr(at)),
        |t| one_shot!(t, class, |s| usb_op::rename(&mut s, at, &name)),
    )
    .map_err(|e| explain(e, at))?;
    ui.note(format!("renamed {} -> {:?}", shown(at), name));
    Ok(())
}

/// Duplicate an object into another slot (a device-internal deep copy). Destructive;
/// requires confirmation.
pub fn duplicate(
    ui: &Ui,
    from: Location,
    to: Location,
    class: ObjectClass,
    confirmed: bool,
) -> Result<(), String> {
    let mut t = open_usb()?;
    let name = peek(&mut t, class, from)?;
    let dest = peek_dest(ui, &mut t, class, to, DestFate::Overwritten);
    ui.note(format!(
        "duplicating {:?} from {} to {} {} {}",
        name,
        shown(from),
        shown(to),
        ui.dash(),
        dest
    ));
    ui.confirm(confirmed)?;
    transact(
        &mut t,
        format!("{} duplicate {} {}", noun(class), addr(from), addr(to)),
        |t| one_shot!(t, class, |s| usb_op::duplicate(&mut s, from, to)),
    )
    .map_err(|e| explain_pair(e, from, to))?;
    ui.note(format!("duplicated {} -> {}", shown(from), shown(to)));
    Ok(())
}

/// Load an object live on the instrument (double-click in NSM). Non-destructive, so no
/// confirmation is needed.
pub fn select(ui: &Ui, at: Location, class: ObjectClass) -> Result<(), String> {
    let mut t = open_usb()?;
    transact(
        &mut t,
        format!("{} select {}", noun(class), addr(at)),
        |t| {
            nord_usb::block_on(async {
                let mut s = Session::open(t, class).await?;
                let r = usb_op::select(&mut s, at).await;
                let closed = s.commit().await;
                r.and(closed)
            })
        },
    )
    .map_err(|e| explain(e, at))?;
    ui.note(format!("selected {} on the instrument", shown(at)));
    Ok(())
}

/// Thousands separators. A nine-digit byte count is otherwise counted by eye.
pub(crate) fn grouped(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Rounded binary size, or `None` below a kibibyte where the byte count already reads.
pub(crate) fn human_size(n: u32) -> Option<String> {
    const UNITS: [&str; 3] = ["KiB", "MiB", "GiB"];
    if n < 1024 {
        return None;
    }
    let mut value = n as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    Some(format!("{value:.1} {}", UNITS[unit]))
}

/// List the piano/sample library objects an entity depends on. Read-only.
pub fn deps(ui: &Ui, at: Location, class: ObjectClass) -> Result<(), String> {
    let mut t = open_usb()?;
    let deps = transact(&mut t, format!("{} deps {}", noun(class), addr(at)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = usb_op::dependencies(&mut s, at).await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    })
    .map_err(|e| explain(e, at))?;

    // Unrouted sections still report rows, but they are not live dependencies.
    let (live, idle): (Vec<_>, Vec<_>) = deps.iter().partition(|d| d.flag == 1);
    // Null ids describe unassigned routed sections, not library objects.
    let (live, unassigned): (Vec<_>, Vec<_>) = live.into_iter().partition(|d| d.is_required());

    if live.is_empty() {
        ui.note(format!("{} depends on nothing", shown(at)));
    } else {
        ui.out(ui.dim(format!("{:<8} {:<10} name", "class", "id")));
        for d in &live {
            // Library objects report no slot, so most rows carry no location at all.
            let loc = match d.location.map(shown) {
                Some(at) => format!("  {}", ui.dim(at)),
                None => String::new(),
            };
            ui.out(format!(
                "{:<8} {:08x}   {}{loc}",
                d.class.label(),
                d.id,
                d.name.trim_end(),
            ));
        }
    }

    if !unassigned.is_empty() {
        let which: Vec<String> = unassigned
            .iter()
            .map(|d| d.class.label().to_string())
            .collect();
        ui.note("");
        ui.note(format!("routed but nothing assigned: {}", which.join(", ")));
    }

    if !idle.is_empty() {
        ui.note("");
        ui.note(format!(
            "{} further row(s) reported but not in use — the section is not routed to a \
             keyboard part, so the instrument names an object this object does not depend on:",
            idle.len()
        ));
        for d in &idle {
            let named = if d.name.trim_end().is_empty() {
                "(no name)".to_string()
            } else {
                d.name.trim_end().to_string()
            };
            ui.note(format!("  {} {:08x} {}", d.class.label(), d.id, named));
        }
    }
    Ok(())
}

/// Release anything an interrupted run left open on the instrument.
pub fn recover(ui: &Ui) -> Result<(), String> {
    let mut t = open_usb()?;
    transact(&mut t, "device recover", |t| {
        nord_usb::block_on(usb_op::recover(t))
    })
    .map_err(|e| e.to_string())?;
    ui.note("released any session the instrument was still holding");
    ui.note("if slots were reading as empty, re-check them now");
    Ok(())
}

/// Report the instrument's storage layout, from the device's own tables. Read-only.
pub fn geometry(ui: &Ui) -> Result<(), String> {
    let mut t = open_usb()?;
    let rows = transact(&mut t, "device geometry", |t| {
        nord_usb::block_on(async {
            // Any class opens a session; the partition table is device-wide.
            let mut s = Session::open(t, ObjectClass::Program).await?;
            let r = async {
                let parts = usb_op::partitions(&mut s).await?;
                let mut rows = Vec::new();
                for p in parts {
                    let banks = usb_op::banks(&mut s, p.index).await?;
                    rows.push((p, banks));
                }
                Ok(rows)
            }
            .await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    })
    .map_err(|e| e.to_string())?;

    ui.out(ui.dim(format!(
        "{:<4} {:<18} {:>6} {:>7}  banks",
        "code", "partition", "banks", "slots"
    )));
    for (p, banks) in &rows {
        // The sentinel is not a capacity and must not be summed into one.
        let bounded: Vec<&nord_usb::wire::Bank> = banks.iter().filter(|b| b.is_bounded()).collect();
        let slots = if bounded.len() == banks.len() {
            bounded.iter().map(|b| b.slots).sum::<u32>().to_string()
        } else {
            "—".to_string()
        };
        let names: Vec<&str> = banks.iter().map(|b| b.name.as_str()).collect();
        ui.out(format!(
            "{:<4} {:<18} {:>6} {:>7}  {}",
            p.index,
            p.name,
            banks.len(),
            slots,
            ui.dim(names.join(", ")),
        ));
    }
    ui.note("");
    ui.note("the partition index is the object class number; (Native) partitions are a");
    ui.note("second view of the same library, so their capacity is a sentinel, not a size");
    Ok(())
}

/// Deliberately abandon an open session, wedging the instrument. Test tool.
///
/// Behind the `wedge` feature: it breaks the attached instrument on purpose.
///
/// Reproduces the half-open `HELLO` on purpose: opens a transaction and drops it without
/// the closing exchanges. The instrument then answers "empty" for every slot in every
/// class, which survives reopening.
///
/// Exists so recovery can be tested against a *known* wedge rather than one arrived at by
/// accident. Nothing stored is harmed — but until it is cleared, every reading taken from
/// the instrument is a lie, which is worse than an error.
#[cfg(feature = "wedge")]
pub fn wedge(ui: &Ui, class: ObjectClass, yes: bool) -> Result<(), String> {
    if !yes {
        return Err("refusing to wedge the instrument without --yes; \
             clear it afterwards with `nord device recover`"
            .into());
    }
    let mut t = open_usb()?;
    nord_usb::block_on(async {
        let s = Session::open(&mut t, class).await?;
        s.abort();
        Ok::<(), nord_usb::Error>(())
    })
    .map_err(|e| e.to_string())?;

    ui.note("session abandoned with no GOODBYE — the instrument is now wedged");
    ui.note("every slot will read as empty, and read *successfully*, until you run");
    ui.note("`nord device recover`");
    Ok(())
}

/// Sweep vendor control requests on endpoint 0. Reverse-engineering tool.
///
/// Read-only, and outside the bulk protocol: no session is opened, so nothing here can
/// desync or wedge one. A request the device does not implement stalls the endpoint,
/// which arrives as an error and is reported as a dash rather than as data.
#[allow(clippy::too_many_arguments)]
pub fn controls(
    ui: &Ui,
    from: u8,
    to: u8,
    len: usize,
    interface: bool,
    value: u16,
    index: u16,
) -> Result<(), String> {
    if from > to {
        return Err(format!(
            "--from {from:#04x} is above --to {to:#04x}; nothing to sweep"
        ));
    }
    let t = open_usb()?;
    let recipient = if interface {
        nord_usb::transport::usb::Recipient::Interface
    } else {
        nord_usb::transport::usb::Recipient::Device
    };

    ui.out(ui.dim(format!("{:<9} {:>5}  {}", "bRequest", "bytes", "response")));
    let mut answered = 0;
    for request in from..=to {
        let got = t.vendor_control_in(
            recipient,
            request,
            value,
            index,
            len,
            std::time::Duration::from_millis(500),
        );
        match got {
            Ok(data) if data.is_empty() => {
                answered += 1;
                ui.out(format!(
                    "{request:#04x} ({request:>3}) {:>5}  (accepted, no data)",
                    0
                ));
            }
            Ok(data) => {
                answered += 1;
                let hex: Vec<String> = data.iter().take(24).map(|b| format!("{b:02x}")).collect();
                let text: String = data
                    .iter()
                    .map(|&b| {
                        if (0x20..0x7f).contains(&b) {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                ui.out(format!(
                    "{request:#04x} ({request:>3}) {:>5}  {}",
                    data.len(),
                    hex.join(" ")
                ));
                ui.out(format!("{:>16}  {}", "", ui.dim(text)));
            }
            // The overwhelmingly common case while sweeping: not implemented.
            Err(_) => ui.out(ui.dim(format!("{request:#04x} ({request:>3})     -  —"))),
        }
    }
    ui.note("");
    ui.note(format!(
        "{answered} of {} request(s) answered",
        u16::from(to) - u16::from(from) + 1
    ));
    Ok(())
}

/// Report which object the panel has loaded in this class. Read-only.
pub fn focus(ui: &Ui, class: ObjectClass) -> Result<(), String> {
    let mut t = open_usb()?;
    let (at, info) = transact(&mut t, format!("{} focus", noun(class)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = async {
                let at = usb_op::focus(&mut s).await?;
                // An empty focused slot is possible and is not an error to report as one.
                let info = match usb_op::info(&mut s, at).await {
                    Ok(i) => Some(i),
                    Err(nord_usb::Error::DeviceStatus(1)) => None,
                    Err(e) => return Err(e),
                };
                Ok((at, info))
            }
            .await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    })
    .map_err(|e| e.to_string())?;

    match info {
        Some(info) => ui.out(format!("{}  {:?}", addr(at), info.name)),
        None => ui.out(format!("{}  (empty)", addr(at))),
    }
    Ok(())
}

/// List every occupied slot in a class, with each object's name. Read-only.
///
/// One session: the cursor walk and every `info` share it, so a library of a few hundred
/// items is a few hundred exchanges rather than a few hundred sessions.
pub fn list(ui: &Ui, class: ObjectClass, cap: usize) -> Result<(), String> {
    let mut t = open_usb()?;
    // The guard is part of what the walk sends, so it belongs in the intent whenever it
    // is not the default the sweep assumes.
    let bound = if cap == 1024 {
        String::new()
    } else {
        format!(" {cap}")
    };
    let rows = transact(&mut t, format!("{} walk{bound}", noun(class)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = async {
                let mut rows = Vec::new();
                for at in usb_op::occupied_slots(&mut s, cap).await? {
                    // The cursor may return an empty starting address; status 1 is harmless.
                    match usb_op::info(&mut s, at).await {
                        Ok(info) => rows.push((at, info)),
                        Err(nord_usb::Error::DeviceStatus(1)) => {}
                        Err(e) => return Err(e),
                    }
                }
                Ok(rows)
            }
            .await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    })
    .map_err(explain_walk)?;

    if rows.is_empty() {
        ui.note(format!("no {} on the instrument", class.label()));
        return Ok(());
    }

    ui.out(ui.dim(format!(
        "{:<8} {:<6} {:>9}  name",
        "slot", "format", "bytes"
    )));
    for (at, info) in &rows {
        ui.out(format!(
            "{:<8} {:<6} {:>9}  {}",
            addr(*at),
            info.format,
            info.body_len,
            info.name.trim_end(),
        ));
    }
    ui.note("");
    ui.note(format!("{} {}", rows.len(), class.label()));
    Ok(())
}

/// Send a raw command code and print the reply verbatim. Reverse-engineering tool.
///
/// Interprets nothing: an unknown command's status word and payload are the finding, so
/// both are printed as they arrived. A device that ignores the command is reported as a
/// timeout rather than hanging the caller.
#[allow(clippy::too_many_arguments)]
pub fn probe(
    ui: &Ui,
    class: ObjectClass,
    op: u32,
    args: &[u32],
    wait: u64,
    yes: bool,
    bare: bool,
    service: u32,
    subsystem: u32,
) -> Result<(), String> {
    let mut words = Vec::with_capacity(args.len() * 4);
    for a in args {
        words.extend_from_slice(&a.to_be_bytes());
    }

    // Known wedges: no reply, and a power cycle to recover. A price, not a
    // prohibition, so `--yes` proceeds informed.
    if op == nord_usb::wire::cmd::NOTIFY_READ_WEDGE {
        ui.note(format!(
            "{op:#04x} is known to wedge the instrument (no reply, session lost, \
             power cycle to recover); nothing stored has ever been harmed by it"
        ));
    }

    // The one code that destroys data rather than costing a power cycle. The session's
    // class is what aims it, so the warning names the target it is currently pointed at.
    if op == nord_usb::wire::cmd::ERASE_ALL {
        ui.note(format!(
            "{op:#04x} is reported to erase an ENTIRE PARTITION — as aimed, all of {}. \
             Unlike the wedges this does not cost a power cycle, it costs the data; \
             restoring a library means a backup and a long upload",
            class.label()
        ));
    }

    // The general form of that warning. A code above the answering range is not a
    // spare slot: one of them starts erasing and cannot be talked out of it.
    if op > nord_usb::wire::cmd::HIGHEST_ANSWERING {
        ui.note(format!(
            "{op:#04x} is above {:#04x}, the highest command this instrument has been \
             seen to answer; codes up there are unexplored and at least one is \
             destructive",
            nord_usb::wire::cmd::HIGHEST_ANSWERING
        ));
    }

    ui.note(format!(
        "probing command {op:#04x} on {} with {} argument word(s)",
        class.label(),
        args.len()
    ));
    if !yes {
        return Err("refusing to probe without --yes".into());
    }

    if op == u32::MAX {
        return Err(format!(
            "{op:#x} has no `op + 1` reply code; the command space ends one below it"
        ));
    }

    let svc = nord_usb::Service::from_raw(service);
    let mut t = open_usb()?;

    // Bare probes bypass session machinery that may itself be refusing commands.
    if bare {
        let reply = nord_usb::block_on(async {
            let req = nord_usb::Message::new(svc, subsystem, op, words.clone());
            t.write(&req.encode()).await?;
            match t
                .read_timeout(
                    nord_usb::transport::READ_BUFFER,
                    std::time::Duration::from_secs(wait),
                )
                .await?
            {
                Some(raw) => nord_usb::Message::decode_probe(&raw).map(Some),
                None => Ok(None),
            }
        })
        .map_err(|e: nord_usb::Error| e.to_string())?;

        match reply {
            Some(reply) => report_reply(ui, &reply, op),
            None => ui.out(format!("no reply within {wait}s")),
        }
        return Ok(());
    }

    let (reply, changed, close_failed) = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = s
            .probe(
                svc,
                subsystem,
                op,
                &words,
                std::time::Duration::from_secs(wait),
            )
            .await;
        let changed = s.instrument_changed();
        let closed = s
            .commit_with_read_limit(std::time::Duration::from_secs(wait))
            .await;
        // Preserve a probe reply even if that unknown command made the close fail.
        r.map(|reply| (reply, changed, closed.err()))
    })
    .map_err(|e| e.to_string())?;

    if changed {
        ui.note("the instrument reported a change during this session");
    }
    if let Some(e) = close_failed {
        ui.note(format!("the session would not close afterwards: {e}"));
    }

    let Some(reply) = reply else {
        ui.out(format!(
            "no reply within {wait}s — the device ignored command {op:#04x}"
        ));
        return Ok(());
    };

    report_reply(ui, &reply, op);
    Ok(())
}

/// Print a probed reply verbatim: echoed command, status, and a hex/ASCII payload dump.
///
/// Interprets nothing. On an unknown command the status is the finding, and the payload
/// of a non-zero status is uninitialised device memory rather than data — so it is shown
/// as bytes and never decoded.
fn report_reply(ui: &Ui, reply: &nord_usb::Message, op: u32) {
    // `command` is the device's own echo, not an assumption: an unknown code may not
    // answer with `op + 1`, and which code it does answer with is part of the finding.
    ui.out(format!(
        "reply command {:#04x}{}",
        reply.command,
        if reply.command == op + 1 {
            String::new()
        } else {
            format!(" (expected {:#04x} by the +1 rule)", op + 1)
        }
    ));
    match reply.status() {
        Some(0) => ui.out("status  0 (ok)".to_string()),
        Some(code) => ui.out(format!("status  {code} ({code:#x}) — not success")),
        None => ui.out("status  absent — reply too short to carry one".to_string()),
    }

    let payload = reply.payload();
    ui.out(format!("payload {} bytes", payload.len()));
    for (i, chunk) in payload.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        ui.out(format!("  {:04x}  {:<47}  {ascii}", i * 16, hex.join(" ")));
    }
}

/// Report everything the instrument knows about one slot. Read-only.
///
/// This is `0x1e`: body length, format tag, version, name and CRC-32 — every field of the
/// CBIN header, which is never itself transmitted, plus the name, which no `.ne5p`/`.ne5t`
/// file stores at all.
pub fn slot_info(ui: &Ui, at: Location, class: ObjectClass) -> Result<(), String> {
    let mut t = open_usb()?;
    let info = transact(&mut t, format!("{} info {}", noun(class), addr(at)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = usb_op::info(&mut s, at).await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    })
    .map_err(|e| explain(e, at))?;

    let row = |label: &str, value: String| {
        ui.out(format!("  {}{value}", ui.dim(format!("{label:<11}"))));
    };
    row("location:", shown(info.location));
    row("name:", format!("{:?}", info.name));
    row("format:", info.format.clone());
    row("version:", info.version.to_string());
    row(
        "body:",
        format!(
            "{} bytes{}",
            grouped(info.body_len),
            // A piano is nine digits of bytes; the rounded size is what tells you it is
            // a 200MB object rather than a 20MB one.
            match human_size(info.body_len) {
                Some(h) => format!("  {}", ui.dim(format!("({h})"))),
                None => String::new(),
            }
        ),
    );
    match info.crc32 {
        // Library content (pianos, samples) reports 0xffffffff: no checksum is kept for
        // objects this large.
        Some(crc) => row("crc32:", format!("{crc:#010x}")),
        None => row(
            "crc32:",
            format!("none {}", ui.dim("(not checksummed for this class)")),
        ),
    }
    Ok(())
}

/// Read one object's bytes with no printing, for `edit`'s read-modify-write.
pub fn fetch(at: Location, class: ObjectClass) -> Result<Vec<u8>, String> {
    let mut t = open_usb()?;
    transact(&mut t, format!("{} read {}", noun(class), addr(at)), |t| {
        nord_usb::block_on(async {
            let mut s = Session::open(t, class).await?;
            let r = usb_op::read_program(&mut s, at).await;
            let closed = s.commit().await;
            finish(r, closed)
        })
    })
    .map_err(|e| explain(e, at))
}

/// The intent line for a write: the file beside the script, the slot, and the two
/// `BEGIN_WRITE` arguments the file itself does not carry — the name the slot ends up
/// with, and the timestamp the device stores.
fn put_intent(class: ObjectClass, what: &str, at: Location, name: &str, stamp: u32) -> String {
    let file = Path::new(what)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| what.to_string());
    format!("{} put {file} {} {name:?} {stamp}", noun(class), addr(at))
}

/// Filename for a rescued slot: the location as the instrument labels it, and the
/// object's own format tag so the file can be handed straight back to `put`.
fn rescue_name(at: Location, backup: &[u8]) -> String {
    // The rescue name must survive a bad checksum in the slot's last remaining copy.
    let format = backup
        .get(8..12)
        .filter(|tag| tag.iter().all(|b| b.is_ascii_alphanumeric()))
        .map(|tag| String::from_utf8_lossy(tag).into_owned())
        .unwrap_or_else(|| "bin".to_string());
    format!(
        "nord-rescued-{}-{}.{format}",
        at.user_bank(),
        at.user_slot()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rescue file is the last copy of a program that no longer exists on the
    /// instrument, so it has to be named something a person can act on.
    #[test]
    fn a_rescued_slot_is_named_for_its_location_and_format() {
        // A minimal CBIN: magic, header type, tag. The checksum is deliberately left
        // wrong — naming must not depend on the backup being intact.
        let mut file = vec![0u8; 45];
        file[0..4].copy_from_slice(b"CBIN");
        file[4..8].copy_from_slice(&1u32.to_le_bytes());
        file[8..12].copy_from_slice(b"ne5p");
        let at = Location { bank: 6, slot: 49 };
        // Wire is zero-indexed, the instrument's labels are not.
        assert_eq!(rescue_name(at, &file), "nord-rescued-7-50.ne5p");
    }

    /// The report after a failed write is the operator's only account of the slot, and
    /// the two write paths leave it in different states: the delete-first composition
    /// empties the slot, an in-place class leaves whatever the interrupted write put
    /// there. Saying "empty" for the second invites a restore that was never needed.
    #[test]
    fn a_failed_write_says_what_it_left_in_the_slot() {
        let at = Location { bank: 0, slot: 1 };
        assert_eq!(
            aftermath(ObjectClass::Program, at),
            "bank 1 slot 2 is empty"
        );
        assert_eq!(
            aftermath(ObjectClass::Live, at),
            "bank 1 slot 2 may hold a partly written body"
        );
        assert_eq!(
            aftermath(ObjectClass::Settings, at),
            "bank 1 slot 2 may hold a partly written body"
        );
    }

    /// A set list must not land with a program's extension.
    #[test]
    fn the_format_tag_comes_from_the_bytes() {
        let mut file = vec![0u8; 45];
        file[8..12].copy_from_slice(b"ne5t");
        let at = Location { bank: 0, slot: 3 };
        assert_eq!(rescue_name(at, &file), "nord-rescued-1-4.ne5t");
    }

    /// Bytes that do not parse are still the only copy, so they must still get a name.
    #[test]
    fn unparseable_bytes_still_get_rescued() {
        let at = Location { bank: 0, slot: 0 };
        assert_eq!(rescue_name(at, b"nonsense"), "nord-rescued-1-1.bin");
    }

    /// The answer is the only description the corpus will ever have of these bytes, so it
    /// survives into the filename rather than being reduced to something opaque.
    #[test]
    fn a_swept_capture_keeps_the_words_it_was_described_with() {
        assert_eq!(stem("split point C4").unwrap(), "split-point-C4");
        assert_eq!(stem("  transpose +1  ").unwrap(), "transpose-+1");
        assert_eq!(stem("organ vol 5 -> 6").unwrap(), "organ-vol-5-6");
    }

    /// The stem is joined to the output directory, so nothing in it may climb out.
    #[test]
    fn a_swept_name_cannot_leave_the_output_directory() {
        assert_eq!(stem("../../etc/passwd").unwrap(), "etc-passwd");
        assert_eq!(stem("rotary:fast").unwrap(), "rotary-fast");
        assert_eq!(stem(".hidden").unwrap(), "hidden");
    }

    /// Rejected, not silently turned into some default — an unnamed capture in a sweep is
    /// indistinguishable from the ones around it.
    #[test]
    fn an_answer_with_no_filename_in_it_is_refused() {
        for bad in ["...", "/", "  ", "?*", "-", "CON", "lpt1.txt"] {
            assert!(stem(bad).is_err(), "{bad:?}");
        }
    }
}
