//! The operations that talk to an attached instrument.
//!
//! Each one is parameterised by object class: the same code drives a program and a set
//! list, differing only in the class the session opened.
//!
//! Read-only queries (`status`, `get`, `info`, `deps`) and the non-destructive `select`
//! need no confirmation; the mutating actions (`put`, `move`, `delete`, `rename`,
//! `duplicate`) each describe what they will touch and then refuse to proceed without
//! `--yes`.

use std::path::PathBuf;

use nord_usb::op;
use nord_usb::transport::Transport;
use nord_usb::wire::{Location, Status};
use nord_usb::{op as usb_op, ObjectClass, Session};

use crate::slot::shown;
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
            let mut transport =
                nord_usb::transport::UsbTransport::open_first().map_err(|e| e.to_string())?;
            collect(&mut transport)?
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
        // The vendor-specific interface is the one this protocol rides. Without it the
        // device is a Clavia but not one this tool can drive, and saying so here saves a
        // confusing failure inside the first transaction.
        let vendor_iface = d.interfaces().any(|i| i.class() == 0xff);
        ui.out(format!(
            "  protocol:  {}",
            if vendor_iface {
                "vendor interface present"
            } else {
                "no vendor interface — this tool cannot drive it"
            }
        ));
    }
    Ok(())
}

fn collect<T: Transport>(transport: &mut T) -> Result<Vec<Status>, String> {
    nord_usb::block_on(op::inventory(transport)).map_err(|e| e.to_string())
}

fn print_table(ui: &Ui, report: &[Status]) {
    if report.is_empty() {
        ui.out("no classes answered");
        return;
    }
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
/// All three confirmed on hardware 2026-07-31: `0x1` from a vacant slot, `0x3` from
/// `9:1` and `8:51`, `0x4` from a write aimed at an occupied slot.
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

fn open_usb() -> Result<nord_usb::transport::UsbTransport, String> {
    nord_usb::transport::UsbTransport::open_first().map_err(|e| e.to_string())
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
    let (info, file) = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
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
    .map_err(|e| explain(e, at))?;

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
    send(ui, &file, at, class, confirmed, &path.display().to_string())
}

/// Run one mutating operation in its own session, committing either way — an abandoned
/// session leaves the instrument mid-transaction with its progress label still painted.
///
/// A macro rather than a function because the operation borrows the session it is handed,
/// and a closure returning a future that borrows its own argument cannot be written with
/// the higher-ranked bound that would need.
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
/// ⚠️ **An occupied destination is replaced, not overwritten.** The instrument answers
/// status 4 to a write aimed at a slot that already holds something, so this reads the
/// occupant, deletes it, writes, and puts the occupant back if the write fails — the slot
/// is genuinely empty in between, and the only copy of its contents is in this process.
pub fn send(
    ui: &Ui,
    file: &[u8],
    at: Location,
    class: ObjectClass,
    confirmed: bool,
    what: &str,
) -> Result<(), String> {
    let mut t = open_usb()?;

    // Name what is about to be destroyed before destroying it. An empty destination is
    // not a failure: status 1 means the slot is vacant, so there is nothing to report.
    let existing = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::info(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed)
    });

    let existing = match existing {
        Ok(info) => Some(info),
        Err(nord_usb::Error::DeviceStatus(1)) => None,
        Err(e) => return Err(explain(e, at)),
    };

    match &existing {
        Some(info) => {
            ui.note(format!(
                "about to {} {} (currently {:?}) with {what}",
                ui.danger("overwrite"),
                shown(at),
                info.name,
            ));
            // The operator is consenting to the slot being empty for a moment, not just
            // to a write, so the delete has to be part of the question.
            ui.note(format!(
                "  {} the instrument will not overwrite in place, so {} is deleted first. \
                 Its {} bytes are read back beforehand and put back if the write fails.",
                ui.danger("note:"),
                shown(at),
                info.body_len,
            ));
        }
        None => ui.note(format!("{} is empty; writing {what}", shown(at))),
    }
    ui.confirm(confirmed)?;

    // After consent, not before: for a piano this read is minutes long, and nobody should
    // sit through it only to be asked whether they meant it.
    let backup = match &existing {
        Some(_) => Some(
            nord_usb::block_on(async {
                let mut s = Session::open(&mut t, class).await?;
                let r = usb_op::read_program(&mut s, at).await;
                let closed = s.commit().await;
                finish(r, closed)
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

    if existing.is_some() {
        ui.note(format!("deleting {} to make room", shown(at)));
        one_shot!(&mut t, class, |s| usb_op::delete(&mut s, at))
            .map_err(|e| format!("deleting {}: {}", shown(at), explain(e, at)))?;
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);

    let written = one_shot!(&mut t, class, |s| usb_op::write_program(
        &mut s, at, file, timestamp
    ));

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
                "the write failed and {} is now empty; putting the original back",
                shown(at)
            ));
            match one_shot!(&mut t, class, |s| usb_op::write_program(
                &mut s, at, &backup, timestamp
            )) {
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
                    &backup,
                    &e.to_string(),
                    &restore.to_string(),
                )),
            }
        }
    }
}

/// Last resort: the write failed, the restore failed, and the slot's former contents
/// exist only in memory. Spill them next to the operator rather than exiting with them.
fn rescue(ui: &Ui, at: Location, backup: &[u8], write: &str, restore: &str) -> String {
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
                "{write} (restoring failed as well: {restore}) {} is empty; \
                 its former contents were saved to {} — put it back with `nord put`",
                shown(at),
                path.display(),
            )
        }
        Err(io) => format!(
            "{write} (restoring failed as well: {restore}) {} is EMPTY and its former \
             contents could not be saved either ({io}); {} bytes are lost",
            shown(at),
            backup.len(),
        ),
    }
}

/// Read one slot's name in a throwaway read-only session — used to show what a mutation
/// is about to affect before it happens.
fn peek(
    t: &mut nord_usb::transport::UsbTransport,
    class: ObjectClass,
    at: Location,
) -> Result<String, String> {
    nord_usb::block_on(async {
        let mut s = Session::open(t, class).await?;
        let r = usb_op::info(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed).map(|info| info.name)
    })
    .map_err(|e| explain(e, at))
}

/// Describe what currently occupies a *destination* slot, for the pre-flight line —
/// `move` and `duplicate` overwrite it, so it is the thing at risk.
///
/// Unlike [`peek`] this never fails: `INFO` errors on an empty destination, which is the
/// normal case here. A real transport fault surfaces on the operation itself a moment
/// later.
fn peek_dest(
    ui: &Ui,
    t: &mut nord_usb::transport::UsbTransport,
    class: ObjectClass,
    at: Location,
) -> String {
    match peek(t, class, at) {
        Ok(name) => format!("{} {name:?}", ui.danger("OVERWRITING")),
        Err(_) => "destination reads as empty".into(),
    }
}

/// Move an object from one slot to another. Destructive; requires confirmation.
pub fn move_object(
    ui: &Ui,
    from: Location,
    to: Location,
    class: ObjectClass,
    confirmed: bool,
) -> Result<(), String> {
    let mut t = open_usb()?;
    let name = peek(&mut t, class, from)?;
    let dest = peek_dest(ui, &mut t, class, to);
    ui.note(format!(
        "moving {:?} from {} to {} {} {}",
        name,
        shown(from),
        shown(to),
        ui.dash(),
        dest
    ));
    ui.confirm(confirmed)?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class)
            .await?
            .allow_destructive_writes();
        let r = usb_op::move_object(&mut s, from, to).await;
        r.and(s.commit().await)
    })
    .map_err(|e| e.to_string())?;
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
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class)
            .await?
            .allow_destructive_writes();
        let mut r = Ok(());
        for &at in slots {
            r = usb_op::delete(&mut s, at).await;
            if r.is_err() {
                break;
            }
        }
        r.and(s.commit().await)
    })
    .map_err(|e| e.to_string())?;
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
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class)
            .await?
            .allow_destructive_writes();
        let r = usb_op::rename(&mut s, at, &name).await;
        r.and(s.commit().await)
    })
    .map_err(|e| e.to_string())?;
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
    let dest = peek_dest(ui, &mut t, class, to);
    ui.note(format!(
        "duplicating {:?} from {} to {} {} {}",
        name,
        shown(from),
        shown(to),
        ui.dash(),
        dest
    ));
    ui.confirm(confirmed)?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class)
            .await?
            .allow_destructive_writes();
        let r = usb_op::duplicate(&mut s, from, to).await;
        r.and(s.commit().await)
    })
    .map_err(|e| e.to_string())?;
    ui.note(format!("duplicated {} -> {}", shown(from), shown(to)));
    Ok(())
}

/// Load an object live on the instrument (double-click in NSM). Non-destructive, so no
/// confirmation is needed.
pub fn select(ui: &Ui, at: Location, class: ObjectClass) -> Result<(), String> {
    let mut t = open_usb()?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::select(&mut s, at).await;
        let closed = s.commit().await;
        r.and(closed)
    })
    .map_err(|e| e.to_string())?;
    ui.note(format!("selected {} on the instrument", shown(at)));
    Ok(())
}

/// Thousands separators. A nine-digit byte count is otherwise counted by eye.
fn grouped(n: u32) -> String {
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
fn human_size(n: u32) -> Option<String> {
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
    let deps = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::dependencies(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed)
    })
    .map_err(|e| e.to_string())?;

    if deps.is_empty() {
        ui.note(format!("{} has no dependencies", shown(at)));
        return Ok(());
    }
    ui.out(ui.dim(format!("{:<8} {:<10} name", "class", "id")));
    for d in &deps {
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
    Ok(())
}

/// Report everything the instrument knows about one slot. Read-only.
///
/// This is `0x1e`: body length, format tag, version, name and CRC-32 — every field of the
/// CBIN header, which is never itself transmitted, plus the name, which no `.ne5p`/`.ne5t`
/// file stores at all.
pub fn slot_info(ui: &Ui, at: Location, class: ObjectClass) -> Result<(), String> {
    let mut t = open_usb()?;
    let info = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::info(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed)
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
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::read_program(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed)
    })
    .map_err(|e| explain(e, at))
}

/// Filename for a rescued slot: the location as the instrument labels it, and the
/// object's own format tag so the file can be handed straight back to `put`.
fn rescue_name(at: Location, backup: &[u8]) -> String {
    // Read the tag out of the header rather than through `envelope::unwrap`, which also
    // verifies the checksum. These bytes are the last copy of the slot even if they fail
    // that check, so naming them must not depend on it.
    let format = backup
        .get(8..12)
        .filter(|tag| tag.iter().all(|b| b.is_ascii_alphanumeric()))
        .map(|tag| String::from_utf8_lossy(tag).into_owned())
        .unwrap_or_else(|| "bin".to_string());
    format!("nord-rescued-{}-{}.{format}", at.bank + 1, at.slot + 1)
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
}
