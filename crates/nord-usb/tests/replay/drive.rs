//! The intent table: one row per verb, from the declared intent to the call that
//! produces those frames.
//!
//! The vocabulary is the CLI's — `<class> <verb> <args…>`, slots spelled `BANK:SLOT` as
//! the panel labels them — plus the primitives the CLI performs on its own where no
//! command names them (`check-address`, `read`, `read-body`). A new operation is a row
//! here, never a test of its own.
//!
//! An intent that does not parse fails its trial whatever the script declared:
//! [`Error::InvalidArgument`] is deliberately outside the `expect` vocabulary, so a
//! misspelled verb cannot be swallowed by a script that expected a failure.

use std::path::{Path, PathBuf};

use nord_usb::device::Geometry;
use nord_usb::transport::ReplayTransport;
use nord_usb::wire::{AllocationUnit, Bank, ObjectClass};
use nord_usb::{envelope, op, Error, Location, Result, Session};

/// Bytes an intent produced, and the file it named to compare them against.
pub struct Produced {
    pub bytes: Vec<u8>,
    pub expected: PathBuf,
}

/// Run one transaction and close it, whatever the operation did — an abandoned session
/// leaves the instrument mid-transaction, and the closing exchanges are part of what
/// every script pins.
macro_rules! session {
    ($t:expr, $class:expr, |$s:ident| $body:expr) => {{
        let mut $s = Session::open($t, $class).await?;
        let r = $body.await;
        finish(r, $s.commit().await)
    }};
}

/// The same, escalated to a session that may mutate the device.
macro_rules! rw_session {
    ($t:expr, $class:expr, |$s:ident| $body:expr) => {{
        let mut $s = Session::open($t, $class).await?.allow_destructive_writes();
        let r = $body.await;
        finish(r, $s.commit().await)
    }};
}

/// Drive one section's intent through the transport its frames came from.
///
/// `dir` is the script's own directory: a file an intent names travels beside it.
/// `geometry` is the script's own, once one of its sections has read it.
pub async fn drive(
    t: &mut ReplayTransport,
    geometry: &mut Option<Geometry>,
    class: Option<ObjectClass>,
    verb: &str,
    args: &[String],
    dir: &Path,
) -> Result<Option<Produced>> {
    match verb {
        "status" if class.is_none() => op::inventory(t).await.map(|_| None),
        "recover" => op::recover(t).await.map(|_| None),
        "geometry" => {
            let read = session!(t, ObjectClass::Program, |s| Geometry::read(&mut s))?;
            *geometry = Some(read);
            Ok(None)
        }
        "get" | "read" | "get-body" | "read-body" => {
            drive_read(t, need_class(class)?, verb, args, dir).await
        }
        "put" | "move" | "duplicate" | "rename" | "delete" => {
            drive_write(t, geometry, need_class(class)?, verb, args, dir).await
        }
        _ => drive_query(t, geometry, class, verb, args).await,
    }
}

/// The banks a walk is bounded by, and the unit a library `put` reserves in: from the
/// script's own `device geometry` section, else from the committed recording of that
/// exchange, replayed offline.
///
/// A recording made before either was geometry-bounded has no geometry section to read.
/// The tables are static configuration — confirmed on hardware — so the fixture stands
/// in for the instrument those recordings were taken from.
async fn declared_banks(geometry: &Option<Geometry>, class: ObjectClass) -> Result<Vec<Bank>> {
    match geometry {
        Some(read) => read.banks(class).map(<[Bank]>::to_vec),
        None => committed_geometry()
            .await?
            .banks(class)
            .map(<[Bank]>::to_vec),
    }
}

async fn declared_unit(geometry: &Option<Geometry>, class: ObjectClass) -> Result<AllocationUnit> {
    match geometry {
        Some(read) => read.allocation_unit(class),
        None => committed_geometry().await?.allocation_unit(class),
    }
}

/// The committed recording of `device geometry`, replayed on a transport of its own.
async fn committed_geometry() -> Result<Geometry> {
    let mut t = ReplayTransport::new(crate::scripts::fixture("device/geometry.script").steps());
    session!(&mut t, ObjectClass::Program, |s| Geometry::read(&mut s))
}

async fn drive_query(
    t: &mut ReplayTransport,
    geometry: &mut Option<Geometry>,
    class: Option<ObjectClass>,
    verb: &str,
    args: &[String],
) -> Result<Option<Produced>> {
    match verb {
        "status" => session!(t, need_class(class)?, |s| op::status(&mut s)).map(|_| None),
        "focus" => session!(t, need_class(class)?, |s| async {
            // Whatever the panel has loaded is then named, as the CLI names it. An
            // empty focused slot answers status 1, which is not a fault to report.
            let at = op::focus(&mut s).await?;
            match op::info(&mut s, at).await {
                Ok(_) | Err(Error::DeviceStatus(1)) => Ok(()),
                Err(e) => Err(e),
            }
        })
        .map(|()| None),
        "info" => {
            let at = slot(args, 0)?;
            session!(t, need_class(class)?, |s| op::info(&mut s, at)).map(|_| None)
        }
        "deps" => {
            let at = slot(args, 0)?;
            session!(t, need_class(class)?, |s| op::dependencies(&mut s, at)).map(|_| None)
        }
        "check-address" => {
            let at = slot(args, 0)?;
            session!(t, need_class(class)?, |s| op::check_address(&mut s, at)).map(|_| None)
        }
        "select" => {
            let at = slot(args, 0)?;
            session!(t, need_class(class)?, |s| op::select(&mut s, at)).map(|_| None)
        }
        "walk" => {
            if !args.is_empty() {
                return Err(bad("walk takes no arguments; the device's banks bound it"));
            }
            let class = need_class(class)?;
            let banks = declared_banks(geometry, class).await?;
            session!(t, class, |s| async {
                for at in op::occupied_slots(&mut s, &banks).await? {
                    // The cursor's starting address may be empty; status 1 remains in step.
                    match op::info(&mut s, at).await {
                        Ok(_) | Err(Error::DeviceStatus(1)) => {}
                        Err(e) => return Err(e),
                    }
                }
                Ok(())
            })
            .map(|()| None)
        }
        other => Err(bad(format!("unknown verb {other:?}"))),
    }
}

async fn drive_read(
    t: &mut ReplayTransport,
    class: ObjectClass,
    verb: &str,
    args: &[String],
    dir: &Path,
) -> Result<Option<Produced>> {
    let at = slot(args, 0)?;
    let named = args.get(1).map(|file| dir.join(file));
    let (name_first, whole) = (verb.starts_with("get"), !verb.ends_with("body"));
    let bytes = session!(t, class, |s| async {
        if name_first {
            op::info(&mut s, at).await?;
        }
        match whole {
            true => op::read_program(&mut s, at).await,
            false => op::read_body(&mut s, at).await,
        }
    })?;
    Ok(named.map(|expected| Produced { bytes, expected }))
}

async fn drive_write(
    t: &mut ReplayTransport,
    geometry: &mut Option<Geometry>,
    class: ObjectClass,
    verb: &str,
    args: &[String],
    dir: &Path,
) -> Result<Option<Produced>> {
    match verb {
        "put" => {
            let file = std::fs::read(dir.join(text(args, 0)?))?;
            let (at, name, stamp) = (
                slot(args, 1)?,
                text(args, 2)?,
                number(args.get(3).map_or("", String::as_str))?,
            );
            if !class.is_library() {
                return rw_session!(t, class, |s| op::write(&mut s, at, &file, name, stamp))
                    .map(|()| None);
            }
            let unit = declared_unit(geometry, class).await?;
            let blocks = unit.blocks_for(envelope::unwrap(&file)?.body.0.len())?;
            rw_session!(t, class, |s| async {
                op::reserve(&mut s, blocks).await?;
                op::write(&mut s, at, &file, name, stamp).await
            })
            .map(|()| None)
        }
        "move" => {
            let (from, to) = (slot(args, 0)?, slot(args, 1)?);
            rw_session!(t, class, |s| op::move_object(&mut s, from, to)).map(|()| None)
        }
        "duplicate" => {
            let (from, to) = (slot(args, 0)?, slot(args, 1)?);
            rw_session!(t, class, |s| op::duplicate(&mut s, from, to)).map(|()| None)
        }
        "rename" => {
            let (at, name) = (slot(args, 0)?, text(args, 1)?);
            rw_session!(t, class, |s| op::rename(&mut s, at, name)).map(|()| None)
        }
        "delete" => {
            if args.is_empty() {
                return Err(bad("delete names at least one slot"));
            }
            let slots: Vec<Location> = (0..args.len())
                .map(|i| slot(args, i))
                .collect::<Result<_>>()?;
            rw_session!(t, class, |s| async {
                for at in &slots {
                    op::delete(&mut s, *at).await?;
                }
                Ok(())
            })
            .map(|()| None)
        }
        _ => unreachable!("write verbs are dispatched by drive"),
    }
}

/// The object class an intent names, or `None` for the device-wide verbs.
pub fn class_of(token: &str) -> Result<Option<ObjectClass>> {
    Ok(Some(match token {
        "device" => return Ok(None),
        "piano" => ObjectClass::Piano,
        "sample" => ObjectClass::Sample,
        "program" => ObjectClass::Program,
        "setlist" => ObjectClass::SetList,
        "live" => ObjectClass::Live,
        "settings" => ObjectClass::Settings,
        // The classes with no noun of their own, reached as `nord raw --class N`.
        other => match other.strip_prefix("class-").and_then(|n| n.parse().ok()) {
            Some(raw) => ObjectClass::from_raw(raw),
            None => return Err(bad(format!("unknown object class {other:?}"))),
        },
    }))
}

/// Split an intent into its words, keeping a `"quoted string"` whole — a name is one
/// argument however many spaces it holds.
pub fn words(intent: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut chars = intent.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut word = String::new();
        if c == '"' {
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some(c) => word.push(c),
                    None => return Err(bad("unterminated quote")),
                }
            }
        } else {
            word.push(c);
            while chars.peek().is_some_and(|c| !c.is_whitespace()) {
                word.push(chars.next().expect("peeked"));
            }
        }
        out.push(word);
    }
    Ok(out)
}

fn need_class(class: Option<ObjectClass>) -> Result<ObjectClass> {
    class.ok_or_else(|| bad("this verb needs an object class, not `device`"))
}

/// `BANK:SLOT`, the way the instrument labels a location and the CLI parses one.
fn slot(args: &[String], i: usize) -> Result<Location> {
    let s = text(args, i)?;
    let (b, l) = s
        .split_once([':', '-'])
        .ok_or_else(|| bad(format!("expected BANK:SLOT, got {s:?}")))?;
    let (bank, slot) = (number(b)?, number(l)?);
    if bank == 0 || slot == 0 {
        return Err(bad(
            "banks and slots are numbered from 1, as shown on the instrument",
        ));
    }
    Ok(Location::from_user(bank, slot))
}

/// `0x`-prefixed hex or decimal.
fn number(s: &str) -> Result<u32> {
    match s.trim().strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => s.trim().parse(),
    }
    .map_err(|e| bad(format!("{s:?}: {e}")))
}

fn text(args: &[String], i: usize) -> Result<&str> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| bad(format!("missing argument {}", i + 1)))
}

fn bad(what: impl std::fmt::Display) -> Error {
    Error::InvalidArgument(what.to_string())
}

/// The operation's result, or the close's failure when the operation itself passed.
fn finish<T>(r: Result<T>, closed: Result<()>) -> Result<T> {
    match (r, closed) {
        (Err(e), _) | (Ok(_), Err(e)) => Err(e),
        (Ok(v), Ok(())) => Ok(v),
    }
}
