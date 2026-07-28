//! `nord device` and `nord program` — talk to an attached instrument over USB.
//!
//! `nord program` is a program-scoped facade over the same operations: no `--class`,
//! `BANK:SLOT` slots, and `get` defaults to a readable summary instead of a file. The
//! generic `nord device` forms remain for the other object classes.
//!
//! Read-only queries (`status`, `read`, `deps`) and the non-destructive `select` need
//! no confirmation; the mutating actions (`write`, `move`, `delete`, `rename`,
//! `duplicate`) each describe what they will touch and then refuse to proceed without
//! `--yes`. `status` is the gentlest end-to-end check: it sends one query per object
//! class and reads counters back, changing nothing on the instrument.

use std::path::PathBuf;

use nord_usb::op;
use nord_usb::transport::Transport;
use nord_usb::wire::{Location, Status};
use nord_usb::{op as usb_op, ObjectClass, Session};

/// Where to get the exchange from.
pub enum Source {
    /// A real instrument over USB.
    Usb,
    /// A recorded exchange. Lets the whole path be demonstrated with no hardware —
    /// and is how this command is exercised under Wine, qemu and in CI.
    Replay(PathBuf),
}

pub fn run(source: Source, json: bool) -> Result<(), String> {
    let report = match source {
        Source::Usb => {
            let mut transport =
                nord_usb::transport::UsbTransport::open_first().map_err(|e| e.to_string())?;
            collect(&mut transport)?
        }
        Source::Replay(path) => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let mut transport = nord_usb::ReplayTransport::from_script(&text)
                .map_err(|e| e.to_string())?
                .lenient();
            collect(&mut transport)?
        }
    };

    if json {
        print_json(&report);
    } else {
        print_table(&report);
    }
    Ok(())
}

fn collect<T: Transport>(transport: &mut T) -> Result<Vec<Status>, String> {
    nord_usb::block_on(op::inventory(transport)).map_err(|e| e.to_string())
}

fn print_table(report: &[Status]) {
    if report.is_empty() {
        println!("no classes answered");
        return;
    }
    println!("{:<10} {:>20} {:>7}  {}", "class", "used", "full", "of");
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
                (format!("{} / {} blocks", s.used, s.total()), format!("{} items", s.count))
            }
        };
        println!("{:<10} {:>20} {:>6.1}%  {}", s.class.label(), used, s.used_percent(), of);
    }
    if any_variable {
        // Only worth saying for the classes where the number is genuinely opaque.
        println!("\n(blocks are a device-internal unit, not bytes)");
    }
}

fn print_json(report: &[Status]) {
    println!("[");
    for (i, s) in report.iter().enumerate() {
        let comma = if i + 1 == report.len() { "" } else { "," };
        println!(
            "  {{\"class\": \"{}\", \"code\": {}, \"items\": {}, \"used\": {}, \"free\": {}, \"capacity\": {}}}{comma}",
            s.class.label(),
            s.class.to_raw(),
            s.count,
            s.used,
            s.free,
            s.total(),
        );
    }
    println!("]");
}

/// Combine an operation's result with its session close, keeping the operation's error
/// when both fail — a close failing is usually a *consequence* of the op failing, and
/// the original error is the informative one.
///
/// **Always call the close.** An abandoned session leaves the instrument mid-transaction,
/// and a read that has already sent its `"Uploading..."` progress label leaves that label
/// on the display with **no way out but a power cycle** — the closing exchanges are what
/// clear it. Reading an empty slot hit exactly this: `INFO` returns status `0x1`, the
/// `?` skipped the commit, and the instrument was stranded.
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
/// Status `0x1` from a slot-addressed read means the slot is empty — measured across
/// five different vacant slots, while every occupied one answers normally.
fn explain(e: nord_usb::Error, at: Location) -> String {
    match e {
        nord_usb::Error::DeviceStatus(1) => {
            format!("{} is empty", shown(at))
        }
        other => other.to_string(),
    }
}

/// Parse a slot the way the instrument displays it: `8:14` is bank 8, slot 14.
///
/// `:` is the canonical separator — it is what the Electro 5's own display and Nord
/// Sound Manager use. `-` is also accepted because the older `nord device` subcommands
/// documented it, and silently rejecting a form this CLI itself once told people to use
/// would be gratuitous.
pub fn parse_location(s: &str) -> Result<Location, String> {
    let (b, l) = s
        .split_once([':', '-'])
        .ok_or_else(|| format!("expected BANK:SLOT (e.g. 7:4), got {s:?}"))?;
    let bank: u32 = b.trim().parse().map_err(|_| format!("bad bank {b:?}"))?;
    let slot: u32 = l.trim().parse().map_err(|_| format!("bad slot {l:?}"))?;
    if bank == 0 || slot == 0 {
        return Err("banks and slots are numbered from 1, as shown on the instrument".into());
    }
    Ok(Location::from_user(bank, slot))
}

fn open_usb() -> Result<nord_usb::transport::UsbTransport, String> {
    nord_usb::transport::UsbTransport::open_first().map_err(|e| e.to_string())
}

/// Read one entity off the instrument. Read-only.
///
/// `class` selects the object type (4 programs, 5 set lists, …). `raw` writes the wire
/// body verbatim instead of wrapping it in a CBIN header — essential for formats whose
/// header layout is not yet known, where wrapping would fabricate a wrong file.
pub fn read(at: Location, out: Option<PathBuf>, class: u32, raw: bool) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    let (info, file) = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = async {
            let info = usb_op::info(&mut s, at).await?;
            let file = if raw {
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
    .map_err(|e| e.to_string())?;
    eprintln!("  class={:?} format={:?} body_len={}", class, info.format, info.body_len);

    let path = out.unwrap_or_else(|| {
        // Default to the slot name, which is what the corpus convention keys on.
        let stem = if info.name.is_empty() { "entity".into() } else { info.name.clone() };
        let ext = if raw { "body".to_string() } else { info.format.clone() };
        PathBuf::from(format!("{stem}.{ext}"))
    });
    std::fs::write(&path, &file).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("read {} ({} bytes) -> {}", info.name, file.len(), path.display());
    Ok(())
}

/// Write a `.ne5p` into a slot, overwriting it.
pub fn write(path: PathBuf, at: Location, confirmed: bool) -> Result<(), String> {
    let file = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Fail before touching the device if the file is not what it claims to be.
    nord_usb::envelope::unwrap(&file).map_err(|e| e.to_string())?;

    let mut t = open_usb()?;

    // Show what is about to be destroyed before destroying it. Reading the slot
    // first costs one round trip and turns a silent overwrite into an informed one.
    let existing = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await?;
        let r = usb_op::info(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed)
    })
    .map_err(|e| e.to_string())?;

    eprintln!(
        "about to overwrite bank {} slot {} (currently {:?}) with {}",
        existing.location.bank + 1,
        existing.location.slot + 1,
        existing.name,
        path.display()
    );
    if !confirmed {
        return Err("refusing to write without --yes (back up first: `nord device read`)".into());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);

    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await?.allow_destructive_writes();
        let r = usb_op::write_program(&mut s, at, &file, timestamp).await;
        // Close the transaction either way; leaving it half-open is worse than the
        // original error.
        let closed = s.commit().await;
        r.and(closed)
    })
    .map_err(|e| e.to_string())?;

    eprintln!("wrote {} -> bank {} slot {}", path.display(), at.bank + 1, at.slot + 1);
    Ok(())
}

/// One-indexed `bank N slot M`, matching the instrument's own labels.
fn shown(at: Location) -> String {
    format!("bank {} slot {}", at.bank + 1, at.slot + 1)
}

/// Read one slot's name/format in a throwaway read-only session — used to show what a
/// mutation is about to affect before it happens.
fn peek(t: &mut nord_usb::transport::UsbTransport, class: ObjectClass, at: Location) -> Result<String, String> {
    nord_usb::block_on(async {
        let mut s = Session::open(t, class).await?;
        let r = usb_op::info(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed).map(|info| info.name)
    })
    .map_err(|e| e.to_string())
}

/// Describe what currently occupies a *destination* slot, for the pre-flight line.
///
/// `move` and `duplicate` overwrite their destination, so naming only the source hides
/// the thing actually at risk. Unlike [`peek`] this never fails: an empty destination is
/// the normal case and makes `INFO` error, and refusing to move into a free slot because
/// it is free would be absurd. A real transport fault surfaces on the operation itself a
/// moment later.
fn peek_dest(t: &mut nord_usb::transport::UsbTransport, class: ObjectClass, at: Location) -> String {
    match peek(t, class, at) {
        Ok(name) => format!("OVERWRITING {name:?}"),
        Err(_) => "destination reads as empty".into(),
    }
}

/// Refuse a destructive op unless `--yes` was given, after describing what it touches.
fn require_yes(confirmed: bool) -> Result<(), String> {
    if confirmed {
        Ok(())
    } else {
        Err("refusing to modify the device without --yes (back up first: `nord device read`)".into())
    }
}

/// Move an object from one slot to another. Destructive; requires `--yes`.
pub fn move_object(from: Location, to: Location, class: u32, confirmed: bool) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    let name = peek(&mut t, class, from)?;
    let dest = peek_dest(&mut t, class, to);
    eprintln!("moving {:?} from {} to {} — {}", name, shown(from), shown(to), dest);
    require_yes(confirmed)?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?.allow_destructive_writes();
        let r = usb_op::move_object(&mut s, from, to).await;
        r.and(s.commit().await)
    })
    .map_err(|e| e.to_string())?;
    eprintln!("moved {} -> {}", shown(from), shown(to));
    Ok(())
}

/// Delete one or more slots. Destructive; requires `--yes`. All items run in one
/// session, exactly as NSM batches a multi-delete.
pub fn delete(slots: &[Location], class: u32, confirmed: bool) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    for &at in slots {
        let name = peek(&mut t, class, at)?;
        eprintln!("deleting {:?} at {}", name, shown(at));
    }
    require_yes(confirmed)?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?.allow_destructive_writes();
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
    eprintln!("deleted {} item(s)", slots.len());
    Ok(())
}

/// Rename the object in a slot. Destructive; requires `--yes`.
pub fn rename(at: Location, name: String, class: u32, confirmed: bool) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    let old = peek(&mut t, class, at)?;
    eprintln!("renaming {} from {:?} to {:?}", shown(at), old, name);
    require_yes(confirmed)?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?.allow_destructive_writes();
        let r = usb_op::rename(&mut s, at, &name).await;
        r.and(s.commit().await)
    })
    .map_err(|e| e.to_string())?;
    eprintln!("renamed {} -> {:?}", shown(at), name);
    Ok(())
}

/// Duplicate an object into another slot (a device-internal deep copy). Destructive;
/// requires `--yes`.
pub fn duplicate(from: Location, to: Location, class: u32, confirmed: bool) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    let name = peek(&mut t, class, from)?;
    let dest = peek_dest(&mut t, class, to);
    eprintln!("duplicating {:?} from {} to {} — {}", name, shown(from), shown(to), dest);
    require_yes(confirmed)?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?.allow_destructive_writes();
        let r = usb_op::duplicate(&mut s, from, to).await;
        r.and(s.commit().await)
    })
    .map_err(|e| e.to_string())?;
    eprintln!("duplicated {} -> {}", shown(from), shown(to));
    Ok(())
}

/// Load an object live on the instrument (double-click in NSM). Non-destructive, so no
/// confirmation is needed.
pub fn select(at: Location, class: u32) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::select(&mut s, at).await;
        let closed = s.commit().await;
        r.and(closed)
    })
    .map_err(|e| e.to_string())?;
    eprintln!("selected {} on the instrument", shown(at));
    Ok(())
}

/// List the piano/sample library objects an entity depends on. Read-only.
pub fn deps(at: Location, class: u32) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    let deps = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::dependencies(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed)
    })
    .map_err(|e| e.to_string())?;

    if deps.is_empty() {
        println!("{} has no dependencies", shown(at));
        return Ok(());
    }
    println!("{:<8} {:<10} name", "class", "id");
    for d in &deps {
        let loc = d.location.map(shown).unwrap_or_default();
        println!("{:<8} {:08x}   {} {}", d.class.label(), d.id, d.name, loc);
    }
    Ok(())
}

/// Report everything the instrument knows about one slot. Read-only.
///
/// This is `0x1e`, the richest single response on the wire: it carries the body length,
/// format tag, version, name and CRC-32 — i.e. every field of the CBIN header, which is
/// never itself transmitted, plus the name, which no `.ne5p`/`.ne5t` file stores at all.
/// So this is the one command that shows what a `nord device read` would reconstruct,
/// without reading the body.
pub fn info(at: Location, class: u32) -> Result<(), String> {
    let class = ObjectClass::from_raw(class);
    let mut t = open_usb()?;
    let info = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, class).await?;
        let r = usb_op::info(&mut s, at).await;
        let closed = s.commit().await;
        finish(r, closed)
    })
    .map_err(|e| explain(e, at))?;

    println!("  location:  {}", shown(info.location));
    println!("  name:      {:?}", info.name);
    println!("  format:    {}", info.format);
    println!("  version:   {}", info.version);
    println!("  body:      {} bytes", info.body_len);
    match info.crc32 {
        // Library content (pianos, samples) reports 0xffffffff: no checksum is kept for
        // objects this large.
        Some(crc) => println!("  crc32:     {crc:#010x}"),
        None => println!("  crc32:     none (not checksummed for this class)"),
    }
    Ok(())
}

/// Read a program and either print a readable summary or save the `.ne5p`. Read-only.
///
/// Summary is the default because that is what you usually want when poking at an
/// instrument: `nord program get 7:4` should tell you what is in 7:4, not leave a file
/// in the working directory. `--out` is the deliberate act.
pub fn program_get(at: Location, out: Option<PathBuf>) -> Result<(), String> {
    let mut t = open_usb()?;
    let (info, file) = nord_usb::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await?;
        let r = async {
            let info = usb_op::info(&mut s, at).await?;
            let file = usb_op::read_program(&mut s, at).await?;
            Ok::<_, nord_usb::Error>((info, file))
        }
        .await;
        let closed = s.commit().await;
        finish(r, closed)
    })
    .map_err(|e| explain(e, at))?;

    if let Some(path) = out {
        std::fs::write(&path, &file).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!("read {:?} from {} -> {}", info.name, shown(at), path.display());
        return Ok(());
    }

    // Parse the bytes we just built rather than reporting the wire fields directly:
    // that exercises the same path `nord inspect` uses, so a decode regression shows up
    // here too instead of only in file-land.
    let entity = nord_format::from_stream(&mut std::io::Cursor::new(&file))
        .map_err(|e| format!("{} decoded off the device but did not parse: {e}", shown(at)))?;

    println!("{} — {:?}  ({}, version {})", shown(at), info.name, info.format, info.version);
    crate::print_summary(&entity);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_location;

    /// `:` is the canonical separator; `-` stays accepted for the older `nord device`
    /// spellings. Both must land on the same zero-indexed wire location.
    #[test]
    fn both_slot_separators_parse_to_the_same_place() {
        let colon = parse_location("7:4").unwrap();
        let dash = parse_location("7-4").unwrap();
        assert_eq!(colon, dash);
        // The UI is one-indexed, the wire is zero-indexed.
        assert_eq!((colon.bank, colon.slot), (6, 3));
    }

    #[test]
    fn whitespace_around_the_numbers_is_tolerated() {
        assert_eq!(parse_location(" 8 : 14 ").unwrap(), parse_location("8:14").unwrap());
    }

    /// Zero is the giveaway that someone passed a wire index instead of a panel label.
    #[test]
    fn zero_is_rejected_because_the_panel_counts_from_one() {
        for bad in ["0:1", "1:0", "0:0"] {
            let err = parse_location(bad).unwrap_err();
            assert!(err.contains("numbered from 1"), "{bad}: {err}");
        }
    }

    #[test]
    fn malformed_slots_say_what_was_expected() {
        assert!(parse_location("74").unwrap_err().contains("BANK:SLOT"));
        assert!(parse_location("7:x").unwrap_err().contains("bad slot"));
        assert!(parse_location("x:4").unwrap_err().contains("bad bank"));
    }
}
