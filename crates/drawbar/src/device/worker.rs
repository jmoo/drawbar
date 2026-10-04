//! The operations, run against whichever transport the target supplies.
//!
//! Everything here is generic over [`Transport`], so the browser and the desktop run
//! the same code and only the spawn glue is cfg'd.
//!
//! ⚠️ Every operation attempts explicit session cleanup, including after an error.
//! [`Device::read`] and [`Device::destructive`] own that contract here.

use std::num::NonZeroU32;
use std::sync::mpsc::Sender;
use std::time::Duration;

use eframe::egui;
use nord_usb::device::Device;
use nord_usb::envelope;
use nord_usb::error::ErrKind;
use nord_usb::session::ReadWrite;
use nord_usb::transport::Transport;
use nord_usb::wire::{AllocationUnit, Bank, ProgramInfo};
use nord_usb::{op, Error, Location, ObjectClass, Session};

use super::scratch::{Kept, Scratch};
use super::{DeviceCmd, DeviceEvent, Fetched, Outgoing, Partition, Payload, Purpose};
use crate::strings::shown;
use crate::workspace::Origin;

/// The event channel back to the UI thread, with the repaint that makes an event
/// visible before the next input arrives.
#[derive(Clone)]
pub struct Emit {
    tx: Sender<DeviceEvent>,
    ctx: egui::Context,
}

impl Emit {
    pub fn new(tx: Sender<DeviceEvent>, ctx: egui::Context) -> Emit {
        Emit { tx, ctx }
    }

    pub fn send(&self, event: DeviceEvent) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

/// Whether the worker keeps its transport after this command, and why it does not.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// The operator asked for it back.
    Released,
    /// The transport failed, so nothing is on the other end anymore.
    Lost,
}

/// A device status is a reply; only transport failure means detachment.
fn hung_up(e: &Error) -> bool {
    matches!(e, Error::Transport(_))
}

/// Report the classes the instrument declares, from its own partition table.
///
/// The first thing a connection does, because nothing can ask for a class before it
/// knows the instrument has one. The table is read once and kept, and later operations
/// answer from it.
pub async fn announce<T: Transport>(device: &mut Device<T>, emit: &Emit) -> Flow {
    let started = crate::telemetry::now();
    let rows = match device.geometry().await {
        Ok(geometry) => geometry
            .entries()
            .map(|(partition, _)| Partition {
                class: ObjectClass::from_raw(partition.index),
                name: partition.name.clone(),
                native: partition.native,
                unit: partition.allocation_unit().ok(),
            })
            .collect(),
        Err(e) => {
            let lost = hung_up(&e);
            let partitions = crate::telemetry::Op {
                name: "partitions",
                class: None,
                asked: false,
            };
            let took = crate::telemetry::now() - started;
            crate::telemetry::op(partitions, Some(e.expect_kind().to_string()), took);
            emit.send(DeviceEvent::OpFailed(format!("partitions: {e}")));
            return match lost {
                true => Flow::Lost,
                false => Flow::Continue,
            };
        }
    };
    emit.send(DeviceEvent::Partitions(rows));
    Flow::Continue
}

/// What one command's failures said, gathered as they are turned into sentences.
#[derive(Default)]
struct Fault {
    /// The instrument is no longer there.
    gone: bool,
    /// The first failure's kind. `None` when the command refused on its own account.
    kind: Option<ErrKind>,
}

impl Fault {
    fn saw(&mut self, e: &Error) {
        self.gone |= hung_up(e);
        self.named(e);
    }

    /// Record `e`'s kind without deciding whether the instrument is gone; the caller
    /// leaves that to the attempt that follows.
    fn named(&mut self, e: &Error) {
        self.kind.get_or_insert(e.expect_kind());
    }
}

/// Turn an error into the sentence for it, noting on the way whether the instrument is
/// still there. `at` is the slot the operation was aimed at, where it had one.
fn spoil(fault: &mut Fault, at: Option<Location>) -> impl FnOnce(Error) -> String + '_ {
    move |e| {
        fault.saw(&e);
        match at {
            Some(at) => explain(e, at),
            None => e.to_string(),
        }
    }
}

/// Run one command to completion.
///
/// Emits exactly one [`DeviceEvent::Started`] and one [`DeviceEvent::Finished`], so the
/// UI's in-flight marker cannot be left set by an operation that failed halfway, and at
/// most one [`DeviceEvent::OpOk`] or [`DeviceEvent::OpFailed`]: each is the outcome of one
/// command, and a second would be recorded against another entry of the send queue.
/// Steps within a command report through [`DeviceEvent::Note`].
pub async fn run<T: Transport>(
    device: &mut Device<T>,
    cmd: DeviceCmd,
    scratch: &Scratch,
    emit: &Emit,
) -> Flow {
    if matches!(cmd, DeviceCmd::Disconnect) {
        return Flow::Released;
    }
    let what = cmd.label();
    emit.send(DeviceEvent::Started(what.clone()));

    let metric = cmd.metric();
    let started = crate::telemetry::now();
    let mut fault = Fault::default();
    let result = execute(device, cmd, scratch, emit, &mut fault).await;
    if let Some(metric) = metric {
        let outcome = match &result {
            Ok(_) => None,
            Err(_) => Some(
                fault
                    .kind
                    .map_or("refused".to_string(), |kind| kind.to_string()),
            ),
        };
        crate::telemetry::op(metric, outcome, crate::telemetry::now() - started);
    }

    // State read during this command may already be stale, even when it failed.
    if device.take_changed() {
        emit.send(DeviceEvent::InstrumentChanged);
    }
    match result {
        Ok(Some(note)) => emit.send(DeviceEvent::OpOk(note)),
        Ok(None) => {}
        Err(e) => emit.send(DeviceEvent::OpFailed(format!("{what}: {e}"))),
    }
    emit.send(DeviceEvent::Finished);
    match fault.gone {
        true => Flow::Lost,
        false => Flow::Continue,
    }
}

/// The command bodies. `Ok(Some(note))` is a line for the log; `Ok(None)` means the
/// command's own event already said everything.
async fn execute<T: Transport>(
    device: &mut Device<T>,
    cmd: DeviceCmd,
    scratch: &Scratch,
    emit: &Emit,
    fault: &mut Fault,
) -> Result<Option<String>, String> {
    match cmd {
        // Handled by `run`; the transport is closed by the caller, which owns it.
        DeviceCmd::Disconnect => Ok(None),

        DeviceCmd::ScanBank { class, bank } => {
            let slots = scan_bank(device, class, bank)
                .await
                .map_err(spoil(fault, None))?;
            let filled = slots.iter().filter(|s| s.is_some()).count();
            let note = format!(
                "bank {bank}: {filled} of {} slots hold something",
                slots.len()
            );
            emit.send(DeviceEvent::BankScanned { class, bank, slots });
            Ok(Some(note))
        }

        DeviceCmd::ScanClass { class } => {
            let walked = scan_class(device, class, emit)
                .await
                .map_err(spoil(fault, None))?;
            Ok(Some(format!(
                "{}: {} banks, {} items, {}, one session",
                class.label(),
                walked.banks,
                walked.items,
                walked.how,
            )))
        }

        DeviceCmd::SlotInfo { class, at } => {
            let info = match device.read(class, async |s| op::info(s, at).await).await {
                Ok(info) => Some(info),
                // Status 1 is a vacant slot, not a failure.
                Err(Error::DeviceStatus(op::VACANT)) => None,
                Err(e) => return Err(spoil(fault, Some(at))(e)),
            };
            emit.send(DeviceEvent::SlotInfo { class, at, info });
            Ok(None)
        }

        DeviceCmd::Deps { class, at } => {
            let deps = device
                .read(class, async |s| op::dependencies(s, at).await)
                .await
                .map_err(spoil(fault, Some(at)))?;
            let note = format!("{}: {} dependencies", shown(at), deps.len());
            emit.send(DeviceEvent::Deps { class, at, deps });
            Ok(Some(note))
        }

        // The instrument reports no checksum for a piano or sample slot, so its occupant
        // is read through a CRC-32, and nothing of it is held.
        DeviceCmd::Get {
            class,
            at,
            why: Purpose::Compare,
        } if class.is_library() => {
            let read = device
                .read(class, async |s| {
                    op::read_into(s, at, &mut std::io::sink()).await
                })
                .await;
            let received = match read {
                Ok(received) => received,
                Err(Error::DeviceStatus(op::VACANT)) => {
                    emit.send(DeviceEvent::Vacant {
                        class,
                        at,
                        why: Purpose::Compare,
                    });
                    return Ok(None);
                }
                Err(e) => return Err(spoil(fault, Some(at))(e)),
            };
            let note = format!(
                "read {:?} from {} through its checksum ({} bytes)",
                received.info.name,
                shown(at),
                received.info.body_len
            );
            emit.send(DeviceEvent::Summed {
                class,
                at,
                received,
            });
            Ok(Some(note))
        }

        DeviceCmd::Get { class, at, why } => {
            let (info, bytes) = match read_object(device, class, at).await {
                Ok(read) => read,
                // Status 1 is a vacant slot, not a failure.
                Err(Error::DeviceStatus(op::VACANT)) => {
                    emit.send(DeviceEvent::Vacant { class, at, why });
                    return Ok(None);
                }
                Err(e) => return Err(spoil(fault, Some(at))(e)),
            };
            let note = format!(
                "read {:?} from {} ({} bytes)",
                info.name,
                shown(at),
                bytes.len()
            );
            emit.send(DeviceEvent::Got {
                name: entity_name(&info),
                origin: Origin::Device { class, at },
                bytes,
                why,
            });
            Ok(Some(note))
        }

        DeviceCmd::CopyAll { class, slots } => {
            copy_all(device, class, &slots, scratch, emit, gone).await
        }

        DeviceCmd::Gather { roots } => gather(device, &roots, scratch, emit, gone).await,

        DeviceCmd::Put {
            id,
            class,
            at,
            name,
            payload,
        } => {
            let note = put_one(device, class, at, &name, &payload, scratch, emit, fault)
                .await
                .map_err(spoil(fault, Some(at)))??;
            // Reported as sent only once its session has closed.
            emit.send(DeviceEvent::Sent {
                id,
                class,
                at,
                sent: payload,
            });
            Ok(Some(note))
        }

        DeviceCmd::SendAll { class, items } => {
            send_all(device, class, items, scratch, emit, fault).await
        }

        DeviceCmd::Select { class, at } => {
            device
                .read(class, async |s| op::select(s, at).await)
                .await
                .map_err(spoil(fault, Some(at)))?;
            // `select` puts the panel on the slot, so this is the answer a `FOCUS` read
            // would give, without walking the class.
            emit.send(DeviceEvent::Focus {
                class,
                at: Some(at),
            });
            Ok(Some(format!("selected {} on the instrument", shown(at))))
        }

        DeviceCmd::Reload { class, written } => {
            let focus = reload(device, class, &written)
                .await
                .map_err(spoil(fault, None))?;
            emit.send(DeviceEvent::Focus { class, at: focus });
            Ok(focus
                .filter(|at| written.contains(at))
                .map(|at| format!("selected {} again to play what was written", shown(at))))
        }

        DeviceCmd::Rename { class, at, name } => {
            device
                .destructive(class, async |s| op::rename(s, at, &name).await)
                .await
                .map_err(spoil(fault, Some(at)))?;
            Ok(Some(format!("renamed {} to {name:?}", shown(at))))
        }

        DeviceCmd::Move { class, from, to } => {
            device
                .destructive(class, async |s| op::move_object(s, from, to).await)
                .await
                .map_err(spoil(fault, Some(from)))?;
            Ok(Some(format!("moved {} -> {}", shown(from), shown(to))))
        }

        DeviceCmd::Duplicate { class, from, to } => {
            device
                .destructive(class, async |s| op::duplicate(s, from, to).await)
                .await
                .map_err(spoil(fault, Some(from)))?;
            Ok(Some(format!("duplicated {} -> {}", shown(from), shown(to))))
        }

        DeviceCmd::Delete { class, at } => {
            device
                .destructive(class, async |s| op::delete(s, at).await)
                .await
                .map_err(spoil(fault, Some(at)))?;
            Ok(Some(format!("deleted {}", shown(at))))
        }
    }
}

/// Occupants whose body is at most this many bytes, such as programs, set lists and the
/// smaller samples, are held in memory while a write replaces them. A larger one, a piano
/// or most samples, waits in a file ([`Scratch`]).
const HELD_OCCUPANT: u32 = 1 << 20;

/// What a replace keeps of a slot's occupant until the new object has landed.
enum Backup {
    Held(Vec<u8>),
    Kept(Kept),
}

/// Read the occupant `info` describes so it can be put back: into memory when it is small,
/// and otherwise into a new file from `scratch`, a transfer chunk at a time. A read that
/// fails leaves no file behind.
async fn back_up<T: Transport>(
    s: &mut Session<'_, T, ReadWrite>,
    scratch: &Scratch,
    info: &ProgramInfo,
    fault: &mut Fault,
) -> Result<Backup, String> {
    let at = info.location;
    if info.body_len <= HELD_OCCUPANT {
        return op::read_program(s, at)
            .await
            .map(Backup::Held)
            .map_err(spoil(fault, Some(at)));
    }
    let mut kept = scratch
        .create(&envelope::rescue_name_for(at, &info.format))
        .await
        .map_err(|e| format!("there was nowhere on this computer to keep it: {e}"))?;
    let read = match op::read_into(s, at, &mut kept).await {
        Ok(_) => kept.close().await.map_err(Error::Io),
        Err(e) => Err(e),
    };
    match read {
        Ok(()) => Ok(Backup::Kept(kept)),
        Err(e) => {
            let _ = kept.remove().await;
            Err(spoil(fault, Some(at))(e))
        }
    }
}

/// Put a backup back into its slot.
async fn restore<T: Transport>(
    s: &mut Session<'_, T, ReadWrite>,
    unit: AllocationUnit,
    at: Location,
    backup: &Backup,
    name: &str,
    timestamp: u32,
) -> Result<(), Error> {
    match backup {
        Backup::Held(bytes) => op::write(s, unit, at, bytes, name, timestamp).await,
        Backup::Kept(kept) => {
            let mut file = kept.source().await?;
            op::write_from(s, unit, at, &mut file, name, timestamp).await
        }
    }
}

/// Let a backup go once the slot holds what it should. A file that cannot be deleted is
/// said, and left.
async fn discard(backup: Option<Backup>, emit: &Emit) {
    if let Some(Backup::Kept(kept)) = backup {
        let place = kept.place();
        if let Err(e) = kept.remove().await {
            emit.send(DeviceEvent::Note(format!(
                "{place} is no longer needed, and could not be deleted: {e}"
            )));
        }
    }
}

/// Hand a backup on when the slot may have lost what it held: bytes to the local list, a
/// file left where it is. Returns where it went, in words for the failure.
fn rescue(at: Location, backup: Backup, emit: &Emit) -> String {
    match backup {
        Backup::Held(bytes) => {
            let name = envelope::rescue_name(at, &bytes);
            emit.send(DeviceEvent::Rescued { at, name, bytes });
            "its former contents are in the local list as a rescued entity. Put it back".into()
        }
        Backup::Kept(kept) => {
            let place = kept.place();
            emit.send(DeviceEvent::Kept {
                at,
                place: place.clone(),
            });
            format!("its former contents are kept at {place}. Put them back")
        }
    }
}

/// Why a [`put`] stopped short of writing.
enum Stop {
    Said(String),
    /// The write failed after the occupant was read back, so the occupant is to be put
    /// back ([`put_back`]).
    Undo(Undo),
}

/// An occupant to put back into the slot a write into it failed.
struct Undo {
    at: Location,
    backup: Backup,
    /// The occupant's own name, which the slot takes back.
    name: String,
    timestamp: u32,
    /// Why the write failed.
    why: String,
}

/// What a stop says, leaving an occupant still to be put back in `undo`.
fn stopped(stop: Stop, undo: &mut Option<Undo>) -> String {
    match stop {
        Stop::Said(why) => why,
        Stop::Undo(held) => {
            let why = held.why.clone();
            *undo = Some(held);
            why
        }
    }
}

/// Replace a slot inside the caller's session. The caller checks the address against
/// the geometry first; this sends frames.
///
/// ⚠️ The occupant is read back first, into memory or into a file from `scratch`
/// ([`HELD_OCCUPANT`]). If the write then fails, it is handed back in [`Stop::Undo`], to be
/// put back once this session has closed.
#[allow(clippy::too_many_arguments)]
async fn put<T: Transport>(
    s: &mut Session<'_, T, ReadWrite>,
    unit: AllocationUnit,
    at: Location,
    what: &str,
    payload: &Payload,
    scratch: &Scratch,
    emit: &Emit,
    fault: &mut Fault,
) -> Result<Result<String, Stop>, Error> {
    let class = s.class();
    let timestamp = unix_now()?;

    let existing = match op::info(s, at).await {
        Ok(info) => Some(info),
        Err(Error::DeviceStatus(op::VACANT)) => None,
        Err(e) => return Ok(Err(Stop::Said(spoil(fault, Some(at))(e)))),
    };
    // Confirmed on hardware.
    // Library slots take their name from `BEGIN_WRITE`; buffer classes discard it.
    let write_name = slot_label(what)
        .or_else(|| existing.as_ref().map(|info| info.name.clone()))
        .unwrap_or_default();

    // Nothing is deleted until the backup is in hand.
    let backup = match &existing {
        Some(info) => match back_up(s, scratch, info, fault).await {
            Ok(backup) => Some(backup),
            Err(why) => {
                return Ok(Err(Stop::Said(format!(
                    "could not read {} back before replacing it, so it was left alone: {why}",
                    shown(at),
                ))))
            }
        },
        None => None,
    };

    if let (Some(_), false) = (&backup, class.overwrites_in_place()) {
        emit.send(DeviceEvent::Note(format!(
            "deleting {} to make room",
            shown(at)
        )));
        if let Err(e) = op::delete(s, at).await {
            let refused = matches!(e, Error::DeviceStatus(_));
            let cause = spoil(fault, Some(at))(e);
            // ⚠️ A status is the instrument declining before the delete landed, so the
            // occupant is still there. Any other failure may have come after it.
            return Ok(Err(Stop::Said(match (refused, backup) {
                (false, Some(backup)) => format!(
                    "{} may have been deleted: {cause}; {}",
                    shown(at),
                    rescue(at, backup, emit)
                ),
                (true, backup) => {
                    discard(backup, emit).await;
                    format!("deleting {}: {cause}", shown(at))
                }
                (false, None) => format!("deleting {}: {cause}", shown(at)),
            })));
        }
    }

    let written = match payload {
        Payload::Bytes(bytes) => op::write(s, unit, at, bytes, &write_name, timestamp).await,
        Payload::File { file, .. } => {
            op::write_from(s, unit, at, &mut &**file, &write_name, timestamp).await
        }
    };

    Ok(match (written, backup) {
        (Ok(()), backup) => {
            discard(backup, emit).await;
            Ok(wrote(class, at, what, &write_name))
        }
        (Err(Error::Io(e)), None) => Err(Stop::Said(unreadable(what, &e))),
        (Err(e), None) => Err(Stop::Said(spoil(fault, Some(at))(e))),
        (Err(e), Some(backup)) => {
            fault.named(&e);
            Err(Stop::Undo(Undo {
                at,
                backup,
                name: existing.map(|info| info.name).unwrap_or_default(),
                timestamp,
                why: match e {
                    Error::Io(e) => unreadable(what, &e),
                    e => e.to_string(),
                },
            }))
        }
    })
}

/// Put an occupant back into the slot a write into it failed, and say how the failure
/// ended.
///
/// ⚠️ Called once the failed write's session has closed, never inside it. A write left
/// unfinished is dropped when its session closes, but a second write in that session makes
/// the instrument keep the unfinished one, as an object of its own in another slot.
///
/// Confirmed on hardware.
async fn put_back<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    unit: AllocationUnit,
    undo: Undo,
    emit: &Emit,
    fault: &mut Fault,
) -> String {
    let Undo {
        at,
        backup,
        name,
        timestamp,
        why,
    } = undo;
    emit.send(DeviceEvent::Note(format!(
        "the write failed and {}; putting the original back",
        aftermath(class, at)
    )));
    let restored = device
        .destructive(class, async |s| {
            restore(s, unit, at, &backup, &name, timestamp).await
        })
        .await;
    match restored {
        Ok(()) => {
            discard(Some(backup), emit).await;
            format!("{why} ({} was restored, and is unchanged)", shown(at))
        }
        Err(restore) => {
            fault.saw(&restore);
            format!(
                "{why} (restoring failed as well: {restore}); {}, and {}.",
                aftermath(class, at),
                rescue(at, backup, emit)
            )
        }
    }
}

/// Why a write stopped when the file it was sending stopped reading partway.
fn unreadable(what: &str, e: &std::io::Error) -> String {
    format!(
        "“{what}” could not be read from its file while it was sent ({e}). The file may \
         have changed or moved on disk"
    )
}

fn aftermath(class: ObjectClass, at: Location) -> String {
    match class.overwrites_in_place() {
        true => format!("{} may hold a partly written body", shown(at)),
        false => format!("{} is empty", shown(at)),
    }
}

/// Report a name only for classes that store one.
fn wrote(class: ObjectClass, at: Location, what: &str, name: &str) -> String {
    let wrote = format!("wrote {what} -> {}", shown(at));
    match class.names_its_slots() && !name.is_empty() {
        true => format!("{wrote}, named {name:?}"),
        false => wrote,
    }
}

/// Strip the format suffix from a local label, preserving the operator's text.
/// Returns `None` for a blank name.
fn slot_label(name: &str) -> Option<String> {
    // Application bound; the instrument's maximum is unknown.
    const LONGEST: usize = 64;

    let mut label = name.trim();
    if let Some((stem, tag)) = label.rsplit_once('.') {
        // A format tag, not a name that happens to hold a dot: `Bass 2.0` keeps its `0`.
        let is_tag = (2..=5).contains(&tag.len())
            && tag.chars().all(|c| c.is_ascii_alphanumeric())
            && tag.chars().any(|c| c.is_ascii_alphabetic());
        if is_tag && !stem.trim().is_empty() {
            label = stem;
        }
    }
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    // A truncated UTF-8 name must still end on a character boundary.
    let end = (0..=LONGEST.min(label.len()))
        .rev()
        .find(|end| label.is_char_boundary(*end))?;
    Some(label[..end].trim_end().to_string())
}

#[allow(clippy::too_many_arguments)]
async fn put_one<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
    what: &str,
    payload: &Payload,
    scratch: &Scratch,
    emit: &Emit,
    fault: &mut Fault,
) -> Result<Result<String, String>, Error> {
    let geometry = device.geometry().await?;
    if let Some(why) = geometry.check_address(class, at)? {
        return Ok(Err(format!("{}: {why}", shown(at))));
    }
    let unit = geometry.allocation_unit(class)?;
    // Outside the session, so an occupant to put back survives a failed close.
    let mut undo = None;
    let outcome = device
        .destructive(class, async |s| {
            let put = put(s, unit, at, what, payload, scratch, emit, fault).await?;
            Ok(put.map_err(|stop| stopped(stop, &mut undo)))
        })
        .await;
    match undo {
        Some(undo) => Ok(Err(put_back(device, class, unit, undo, emit, fault).await)),
        None => outcome,
    }
}

/// Send a batch until its first refusal; completed writes remain committed.
async fn send_all<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    items: Vec<Outgoing>,
    scratch: &Scratch,
    emit: &Emit,
    fault: &mut Fault,
) -> Result<Option<String>, String> {
    let total = items.len();
    let mut done = 0;
    let outcome = batch(
        device, class, &items, total, &mut done, scratch, emit, fault,
    )
    .await;
    let refusal = outcome.map_err(spoil(fault, None))?;
    match refusal {
        None => Ok(Some(format!(
            "wrote {done} of {total} to {}",
            class.label()
        ))),
        Some(why) => Err(format!(
            "{why}. {done} of {total} were written; the rest are still waiting"
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn batch<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    items: &[Outgoing],
    total: usize,
    done: &mut usize,
    scratch: &Scratch,
    emit: &Emit,
    fault: &mut Fault,
) -> Result<Option<String>, Error> {
    let geometry = device.geometry().await?;
    let unit = geometry.allocation_unit(class)?;
    let mut refusals = Vec::with_capacity(items.len());
    for item in items {
        refusals.push(geometry.check_address(class, item.at)?);
    }
    // Outside the session, so an occupant to put back survives a failed close.
    let mut undo = None;
    let stopped_at = device
        .destructive(class, async |s| {
            for (item, refused) in items.iter().zip(&refusals) {
                emit.send(DeviceEvent::Note(format!(
                    "sending {:?} to {} ({} of {total})",
                    item.name,
                    shown(item.at),
                    *done + 1
                )));
                if let Some(why) = refused {
                    return Ok(Some(format!("{}: {why}", shown(item.at))));
                }
                let put = put(
                    s,
                    unit,
                    item.at,
                    &item.name,
                    &item.payload,
                    scratch,
                    emit,
                    fault,
                );
                match put.await? {
                    Ok(note) => {
                        *done += 1;
                        emit.send(DeviceEvent::Note(note));
                        emit.send(DeviceEvent::Sent {
                            id: item.id,
                            class,
                            at: item.at,
                            sent: item.payload.clone(),
                        });
                    }
                    Err(stop) => return Ok(Some(stopped(stop, &mut undo))),
                }
            }
            Ok(None)
        })
        .await;
    match undo {
        Some(undo) => Ok(Some(put_back(device, class, unit, undo, emit, fault).await)),
        None => stopped_at,
    }
}

/// The sentence for an error, in terms of the slot the operation was aimed at.
// Confirmed on hardware.
fn explain(e: Error, at: Location) -> String {
    match e {
        Error::DeviceStatus(op::VACANT) => format!("{} is empty", shown(at)),
        Error::DeviceStatus(op::OUT_OF_RANGE) => {
            format!("{} is out of range for this instrument", shown(at))
        }
        Error::DeviceStatus(op::OCCUPIED) => format!(
            "{} is occupied, and the instrument does not overwrite in place",
            shown(at)
        ),
        other => other.to_string(),
    }
}

/// A shorter per-frame limit for metadata walks; transfers keep the session default.
const SCAN_READ_LIMIT: Duration = Duration::from_secs(10);

/// Host safety limit for one complete class scan, not an instrument capacity.
const MOST_OCCUPIED: u32 = op::ENUMERATION_LIMIT as u32;

/// Scan the slots the instrument declares for this bank, or up to the device boundary
/// where it declared the unbounded sentinel.
///
/// The capacity comes from the [`Device`]'s geometry, not the UI's cache: a rescan after
/// a mutation can run before the class has been walked, and walking a bounded bank as if
/// it were open costs one `INFO` per address up to the host limit.
async fn scan_bank<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    bank: u32,
) -> Result<Vec<Option<ProgramInfo>>, Error> {
    let declared = device
        .geometry()
        .await?
        .banks(class)?
        .iter()
        .find(|held| super::user_bank(held.index) == Some(bank))
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "the instrument declares no bank {bank} in {}",
                class.label()
            ))
        })?;
    let extent = Extent::of(declared);
    if matches!(extent, Extent::Known(slots) if slots > MOST_OCCUPIED) {
        return Err(scan_limit(bank.saturating_sub(1)));
    }
    device
        .read(class, async |s| {
            s.set_read_limit(SCAN_READ_LIMIT);
            walk(s, bank, extent, MOST_OCCUPIED).await
        })
        .await
}

/// `bank` is the zero-based index, as the error reports it.
fn scan_limit(bank: u32) -> Error {
    Error::ScanLimit {
        bank,
        limit: MOST_OCCUPIED,
    }
}

/// How many slots a bank holds, as its instrument declares it.
#[derive(Clone, Copy)]
enum Extent {
    Known(u32),
    /// The unbounded sentinel: the bank ends where the instrument refuses a slot.
    Open,
}

impl Extent {
    fn of(bank: &Bank) -> Extent {
        match bank.is_bounded() {
            true => Extent::Known(bank.slots),
            false => Extent::Open,
        }
    }
}

/// One bank a walk will read.
struct Planned {
    /// The bank number the panel labels it with.
    bank: NonZeroU32,
    extent: Extent,
}

struct Walked {
    banks: u32,
    items: usize,
    how: &'static str,
}

/// Scan only the banks and capacities declared by the instrument.
async fn scan_class<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    emit: &Emit,
) -> Result<Walked, Error> {
    let declared = device.geometry().await?.banks(class)?.to_vec();
    let plan = planned(&declared)?;

    device
        .read(class, async |s| {
            // This also bounds the closing exchanges.
            s.set_read_limit(SCAN_READ_LIMIT);

            let status = op::status(s).await?;
            let held = status.count;
            emit.send(DeviceEvent::Geometry {
                class,
                banks: declared.clone(),
            });
            emit.send(DeviceEvent::ClassStatus {
                class,
                status,
                banks: Some(plan.len() as u32),
            });

            // Status 1 means supported but empty; other refusals mean no focus applies.
            match op::focus(s).await {
                Ok(at) => emit.send(DeviceEvent::Focus {
                    class,
                    at: Some(at),
                }),
                Err(Error::DeviceStatus(op::VACANT)) => {
                    emit.send(DeviceEvent::Focus { class, at: None })
                }
                Err(Error::DeviceStatus(_)) => {}
                Err(e) => return Err(e),
            }

            // Cursor enumeration wins for sparse or unbounded storage.
            let capacity = plan
                .iter()
                .try_fold(0u32, |sum, planned| match planned.extent {
                    Extent::Known(slots) => sum.checked_add(slots),
                    Extent::Open => None,
                });
            let sparse = capacity.is_none_or(|capacity| worth_the_cursor(held, capacity));
            let found = match sparse {
                true => occupied(s, &declared).await?,
                false => None,
            };

            let mut remaining = MOST_OCCUPIED;
            let mut items = 0;
            let how = match &found {
                Some(_) => "by cursor",
                None => "slot by slot",
            };
            for planned in &plan {
                let slots = match &found {
                    Some(found) => shape(found, planned, remaining)?,
                    None => walk(s, planned.bank.get(), planned.extent, remaining).await?,
                };
                let taken = u32::try_from(slots.len()).unwrap_or(u32::MAX);
                remaining = remaining
                    .checked_sub(taken)
                    .ok_or_else(|| scan_limit(planned.bank.get() - 1))?;
                items += slots.iter().filter(|slot| slot.is_some()).count();
                emit.send(DeviceEvent::BankScanned {
                    class,
                    bank: planned.bank.get(),
                    slots,
                });
            }
            Ok(Walked {
                banks: plan.len() as u32,
                items,
                how,
            })
        })
        .await
}

fn planned(declared: &[Bank]) -> Result<Vec<Planned>, Error> {
    let mut total = 0u32;
    let mut plan = Vec::with_capacity(declared.len());
    for bank in declared {
        let extent = Extent::of(bank);
        if let Extent::Known(slots) = extent {
            total = total
                .checked_add(slots)
                .ok_or_else(|| scan_limit(bank.index))?;
            if total > MOST_OCCUPIED {
                return Err(scan_limit(bank.index));
            }
        }
        plan.push(Planned {
            bank: bank
                .index
                .checked_add(1)
                .and_then(NonZeroU32::new)
                .expect("a decoded bank index fits its panel number"),
            extent,
        });
    }
    Ok(plan)
}

/// Use cursor enumeration below half capacity, where its two exchanges per item win.
fn worth_the_cursor(held: u32, capacity: u32) -> bool {
    capacity > 0 && held.saturating_mul(2) < capacity
}

/// Return `None` when the device refuses cursor enumeration.
async fn occupied<T: Transport, C>(
    s: &mut Session<'_, T, C>,
    banks: &[Bank],
) -> Result<Option<Vec<(Location, ProgramInfo)>>, Error> {
    let found = match op::occupied_slots(s, banks).await {
        Ok(found) => found,
        Err(Error::DeviceStatus(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    if let Some(at) = found.iter().find(|at| at.slot >= MOST_OCCUPIED) {
        return Err(scan_limit(at.bank));
    }
    let mut out = Vec::with_capacity(found.len());
    for at in found {
        match op::info(s, at).await {
            Ok(info) => out.push((at, info)),
            // A cursor hit may be emptied before INFO; keep the rest of the scan.
            Err(Error::DeviceStatus(op::VACANT)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Some(out))
}

/// Shape cursor hits to the declared capacity, or through an open bank's last item.
///
/// A hit outside a bounded bank is [`Error::Enumeration`]: the instrument answered about
/// a slot it says it does not have, and widening the bank to fit would report slots no
/// later read could reach.
fn shape(
    found: &[(Location, ProgramInfo)],
    planned: &Planned,
    limit: u32,
) -> Result<Vec<Option<ProgramInfo>>, Error> {
    let bank = planned.bank.get() - 1;
    let mine: Vec<&(Location, ProgramInfo)> =
        found.iter().filter(|(at, _)| at.bank == bank).collect();
    let len = match planned.extent {
        Extent::Known(slots) => {
            if let Some((answered, _)) = mine.iter().find(|(at, _)| at.slot >= slots) {
                return Err(Error::Enumeration {
                    bank,
                    answered: *answered,
                    slots,
                });
            }
            slots
        }
        Extent::Open => mine.iter().map(|(at, _)| at.slot + 1).max().unwrap_or(0),
    };
    if len > limit {
        return Err(scan_limit(bank));
    }
    let mut slots = vec![None; len as usize];
    for (at, info) in mine {
        if let Some(cell) = slots.get_mut(at.slot as usize) {
            *cell = Some(info.clone());
        }
    }
    Ok(slots)
}

/// Read a bank slot by slot, taking at most `limit` slots.
///
/// A bank of known extent that refuses one of its own slots is [`Error::Enumeration`].
/// An open bank ends at its first refused slot, less the empty slots before it, and one
/// that has not ended within `limit` is [`Error::ScanLimit`].
async fn walk<T: Transport, C>(
    s: &mut Session<'_, T, C>,
    bank: u32,
    extent: Extent,
    limit: u32,
) -> Result<Vec<Option<ProgramInfo>>, Error> {
    let last = match extent {
        Extent::Known(slots) if slots > limit => return Err(scan_limit(bank.saturating_sub(1))),
        Extent::Known(slots) => slots,
        Extent::Open => limit,
    };
    let mut out = Vec::new();
    for slot in 1..=last {
        let at = Location::from_user(bank, slot);
        match (op::info(s, at).await, extent) {
            (Ok(info), _) => out.push(Some(info)),
            (Err(Error::DeviceStatus(op::VACANT)), _) => out.push(None),
            (Err(Error::DeviceStatus(op::OUT_OF_RANGE)), Extent::Known(slots)) => {
                return Err(Error::Enumeration {
                    bank: at.bank,
                    answered: at,
                    slots,
                })
            }
            (Err(Error::DeviceStatus(op::OUT_OF_RANGE)), Extent::Open) => {
                while matches!(out.last(), Some(None)) {
                    out.pop();
                }
                return Ok(out);
            }
            (Err(e), _) => return Err(e),
        }
    }
    match extent {
        Extent::Known(_) => Ok(out),
        Extent::Open => Err(scan_limit(bank.saturating_sub(1))),
    }
}

/// Copy `slots` of `class` to this computer in one session: each small object as bytes,
/// and each larger one into a file from `scratch`, a transfer chunk at a time.
async fn copy_all<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    slots: &[Location],
    scratch: &Scratch,
    emit: &Emit,
    gone: &mut bool,
) -> Result<Option<String>, String> {
    let mut copied = 0;
    let read = device
        .read(class, async |s| {
            for &at in slots {
                let info = match op::info(s, at).await {
                    Ok(info) => info,
                    Err(Error::DeviceStatus(op::VACANT)) => {
                        let why = Purpose::Copy;
                        emit.send(DeviceEvent::Vacant { class, at, why });
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let name = entity_name(&info);
                if info.body_len <= HELD_OCCUPANT {
                    let bytes = op::read_program(s, at).await?;
                    let origin = Origin::Device { class, at };
                    let why = Purpose::Copy;
                    emit.send(DeviceEvent::Got {
                        name,
                        origin,
                        bytes,
                        why,
                    });
                } else {
                    let file = fetch(s, class, at, &info, scratch).await?;
                    emit.send(DeviceEvent::Fetched(file));
                }
                copied += 1;
            }
            Ok(())
        })
        .await;
    read.map_err(spoil(gone, None))?;
    Ok(Some(format!(
        "copied {copied} of {} from {} in one session",
        slots.len(),
        class.label()
    )))
}

/// Walk `roots` to everything a bundle of them holds, then copy it all to this computer.
async fn gather<T: Transport>(
    device: &mut Device<T>,
    roots: &[(ObjectClass, Location)],
    scratch: &Scratch,
    emit: &Emit,
    gone: &mut bool,
) -> Result<Option<String>, String> {
    let closure = nord_usb::bundle::closure(device, roots)
        .await
        .map_err(spoil(gone, None))?;
    for (class, at, deps) in &closure.objects {
        let (class, at, deps) = (*class, *at, deps.clone());
        emit.send(DeviceEvent::Deps { class, at, deps });
    }
    let slots: Vec<(ObjectClass, Location)> = closure.slots().collect();
    let unfound = closure
        .unfound
        .iter()
        .map(|row| format!("{} “{}”", row.class.label(), row.name.trim_end()))
        .collect();
    emit.send(DeviceEvent::Gathered {
        slots: slots.clone(),
        unfound,
    });
    for class in [
        ObjectClass::SetList,
        ObjectClass::Program,
        ObjectClass::Piano,
        ObjectClass::Sample,
    ] {
        let of: Vec<Location> = slots
            .iter()
            .filter(|(held, _)| *held == class)
            .map(|(_, at)| *at)
            .collect();
        if !of.is_empty() {
            copy_all(device, class, &of, scratch, emit, gone).await?;
        }
    }
    Ok(Some(format!(
        "gathered {} objects for a bundle",
        slots.len()
    )))
}

/// Read the object `info` describes into a new file from `scratch`. A read that fails
/// leaves no file behind.
async fn fetch<T: Transport, C>(
    s: &mut Session<'_, T, C>,
    class: ObjectClass,
    at: Location,
    info: &ProgramInfo,
    scratch: &Scratch,
) -> Result<Fetched, Error> {
    let extension = info.format.trim_end_matches('\0');
    let name = format!("fetched-{}-{}.{extension}", at.bank, at.slot);
    let mut kept = scratch.create(&name).await.map_err(Error::Io)?;
    let read = match op::read_into(s, at, &mut kept).await {
        Ok(_) => kept.close().await.map_err(Error::Io),
        Err(e) => Err(e),
    };
    let file = match read {
        Ok(()) => kept.outside().await.map_err(Error::Io),
        Err(e) => Err(e),
    };
    match file {
        Ok(file) => Ok(Fetched {
            class,
            at,
            name: entity_name(info),
            tag: extension.to_string(),
            len: u64::from(info.body_len) + nord_format::cbin::Generation::V1.body_start(),
            file,
        }),
        Err(e) => {
            let _ = kept.remove().await;
            Err(e)
        }
    }
}

/// Read a slot's metadata and a complete CBIN file of what it holds.
async fn read_object<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
) -> Result<(ProgramInfo, Vec<u8>), Error> {
    device
        .read(class, async |s| {
            let info = op::info(s, at).await?;
            let file = op::read_program(s, at).await?;
            Ok((info, file))
        })
        .await
}

/// Select the panel's slot again where it is one of `written`, and say where the
/// panel is.
async fn reload<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    written: &[Location],
) -> Result<Option<Location>, Error> {
    device
        .read(class, async |s| {
            // Status 1 means supported but empty.
            let focus = match op::focus(s).await {
                Ok(at) => Some(at),
                Err(Error::DeviceStatus(op::VACANT)) => None,
                Err(e) => return Err(e),
            };
            if let Some(at) = focus.filter(|at| written.contains(at)) {
                op::select(s, at).await?;
            }
            Ok(focus)
        })
        .await
}

/// Unix seconds, for the timestamp word `BEGIN_WRITE` carries.
fn unix_now() -> Result<u32, Error> {
    crate::work::unix_seconds().ok_or_else(|| {
        Error::InvalidArgument("system time does not fit the device protocol".into())
    })
}

/// Name a fetched entity after the name its slot reports.
fn entity_name(info: &ProgramInfo) -> String {
    let name = info.name.trim();
    match name.is_empty() {
        true => "unnamed".to_string(),
        false => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_is_named_what_this_computer_calls_the_object() {
        let label = |name: &str| slot_label(name);
        assert_eq!(label("Africa-Split.ne5p").as_deref(), Some("Africa-Split"));
        assert_eq!(label("Squabble B.ne5t").as_deref(), Some("Squabble B"));
        assert_eq!(label("  Rotary Fast  ").as_deref(), Some("Rotary Fast"));
        // A dot that is not a tag: a name is allowed to hold one.
        assert_eq!(label("Bass 2.0").as_deref(), Some("Bass 2.0"));
        assert_eq!(label("Mr. Hammond").as_deref(), Some("Mr. Hammond"));
        assert_eq!(
            label(".ne5p").as_deref(),
            Some(".ne5p"),
            "a bare tag is kept as the name"
        );
    }

    #[test]
    fn a_name_with_nothing_in_it_is_not_sent() {
        for nothing in ["", "   ", "\t"] {
            assert_eq!(slot_label(nothing), None, "{nothing:?}");
        }
    }

    #[test]
    fn a_long_name_is_cut_on_a_character_boundary() {
        let long = "é".repeat(200);
        let cut = slot_label(&long).expect("something is left");
        assert!(cut.len() <= 64, "{} bytes", cut.len());
        assert!(long.starts_with(&cut));
        assert_eq!(cut.chars().count(), 32, "whole characters only");
    }

    #[test]
    fn a_nameless_slot_still_gets_a_label() {
        let named = |name: &str| {
            entity_name(&ProgramInfo {
                location: Location { bank: 0, slot: 0 },
                body_len: 121,
                format: "ne5p".into(),
                version: 4,
                crc32: None,
                modified: None,
                name: name.into(),
            })
        };
        assert_eq!(named("  "), "unnamed");
        assert_eq!(named(" Africa Split "), "Africa Split");
    }

    /// A cursor hit past a bounded bank's capacity is refused: widening the bank would
    /// report slots the instrument says it does not have. An unbounded bank has no
    /// capacity to contradict, so it is shaped through its last item.
    #[test]
    fn a_cursor_hit_past_a_declared_capacity_is_refused() {
        let at = Location { bank: 0, slot: 7 };
        let found = [(
            at,
            ProgramInfo {
                location: at,
                body_len: 121,
                format: "ne5p".into(),
                version: 4,
                crc32: None,
                modified: None,
                name: "Africa Split".into(),
            },
        )];
        let planned = |extent| Planned {
            bank: NonZeroU32::new(1).expect("bank 1"),
            extent,
        };

        match shape(&found, &planned(Extent::Known(4)), MOST_OCCUPIED) {
            Err(Error::Enumeration {
                bank,
                answered,
                slots,
            }) => assert_eq!((bank, answered, slots), (0, at, 4)),
            other => panic!(
                "a hit at 1:8 in a bank of four: {:?}",
                other.map(|slots| slots.len())
            ),
        }
        let open =
            shape(&found, &planned(Extent::Open), MOST_OCCUPIED).expect("an open bank takes it");
        assert_eq!(open.len(), 8, "through the last item and no further");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod wire_tests {
    use std::collections::VecDeque;
    use std::mem::size_of;
    use std::sync::mpsc::Receiver;

    use super::*;
    use crate::device::Purpose;
    use crate::ondisk::OnDisk;
    use nord_usb::wire::{cmd, ui, Message, Service};
    use nord_usb::Transport;
    use std::sync::Arc;

    /// Minimal instrument state for exercising complete worker commands.
    struct Puppet {
        heard: Vec<Message>,
        replies: VecDeque<Vec<u8>>,
        info: u32,
        deaf: bool,
        banks: Vec<(&'static str, u32)>,
        reports_geometry: bool,
        garbles_geometry: bool,
        filled: Option<Vec<(Location, &'static str)>>,
        enumerates: bool,
        focus: Option<Location>,
        refuses_first_write: bool,
        refuses_every_write: bool,
        hangs_up_on_delete: bool,
        /// Whether the body bytes a write carries are kept in [`Puppet::heard`]. A
        /// forgetful one keeps only their checksum and length.
        keeps_data: bool,
        data: nord_format::crc::Crc32Stream<'static>,
        data_len: u64,
        /// What every occupied slot reports holding: its body's length and format tag.
        /// A read of it answers [`occupant_byte`]s.
        occupant: (u32, &'static str),
    }

    /// The Electro 5's bank division, which a default Puppet uses.
    const EIGHT_BANKS: [(&str, u32); 8] = [
        ("Bank 1", 50),
        ("Bank 2", 50),
        ("Bank 3", 50),
        ("Bank 4", 50),
        ("Bank 5", 50),
        ("Bank 6", 50),
        ("Bank 7", 50),
        ("Bank 8", 50),
    ];

    impl Puppet {
        fn new(info: u32) -> Puppet {
            Puppet {
                heard: Vec::new(),
                replies: VecDeque::new(),
                info,
                deaf: false,
                banks: EIGHT_BANKS.to_vec(),
                reports_geometry: true,
                garbles_geometry: false,
                filled: None,
                enumerates: true,
                focus: None,
                refuses_first_write: false,
                refuses_every_write: false,
                hangs_up_on_delete: false,
                keeps_data: true,
                data: nord_format::crc::Crc32Stream::new(),
                data_len: 0,
                occupant: (121, "ne5p"),
            }
        }

        /// Every occupied slot holds a body of `len` bytes, as a file tagged `format`.
        fn holding(mut self, len: u32, format: &'static str) -> Puppet {
            self.occupant = (len, format);
            self
        }

        /// Whether `msg` is a chunk of a write the instrument does not answer: every chunk
        /// but the one that ends the body `BEGIN_WRITE` announced.
        fn unacknowledged(&self, msg: &Message) -> bool {
            let word = |args: &[u8], at: usize| {
                u32::from_be_bytes(args[at..at + 4].try_into().expect("four bytes"))
            };
            let program =
                |command| msg.command == command && matches!(msg.service, Service::Program);
            if !program(cmd::WRITE_DATA) {
                return false;
            }
            let announced = self
                .heard
                .iter()
                .rev()
                .find(|heard| {
                    heard.command == cmd::BEGIN_WRITE && matches!(heard.service, Service::Program)
                })
                .map(|begin| word(&begin.args, 8));
            let end = word(&msg.args, 8) + word(&msg.args, 12);
            announced.is_some_and(|len| end < len)
        }

        /// Keep only the checksum and length of what each write carries, so a write
        /// as large as a library holds nothing of it.
        fn forgetful(mut self) -> Puppet {
            self.keeps_data = false;
            self
        }

        fn deaf() -> Puppet {
            Puppet {
                deaf: true,
                ..Puppet::new(1)
            }
        }

        fn stocked(banks: &[(&'static str, u32)], filled: &[(Location, &'static str)]) -> Puppet {
            Puppet {
                banks: banks.to_vec(),
                filled: Some(filled.to_vec()),
                ..Puppet::new(1)
            }
        }

        fn mute_about_geometry(mut self) -> Puppet {
            self.reports_geometry = false;
            self
        }

        fn garbling_geometry(mut self) -> Puppet {
            self.garbles_geometry = true;
            self
        }

        fn no_enumeration(mut self) -> Puppet {
            self.enumerates = false;
            self
        }

        fn focused_on(mut self, at: Location) -> Puppet {
            self.focus = Some(at);
            self
        }

        fn refusing_the_first_write(mut self) -> Puppet {
            self.refuses_first_write = true;
            self
        }

        /// Refuses the restore as well, which leaves the occupant nowhere to go but the
        /// local list.
        fn refusing_every_write(mut self) -> Puppet {
            self.refuses_every_write = true;
            self
        }

        /// Stops answering once it hears a delete, which leaves the delete's fate unknown.
        fn hanging_up_on_delete(mut self) -> Puppet {
            self.hangs_up_on_delete = true;
            self
        }

        fn holds(&self, at: Location) -> Option<&'static str> {
            self.filled
                .as_ref()?
                .iter()
                .find(|(held, _)| *held == at)
                .map(|(_, name)| *name)
        }

        /// Program and UI services reuse command numbers, so only answer Program frames.
        fn answer(&self, msg: &Message) -> Option<(u32, Vec<u8>)> {
            if !matches!(msg.service, Service::Program) {
                return None;
            }
            let at = || Location {
                bank: u32::from_be_bytes(msg.args[0..4].try_into().unwrap()),
                slot: u32::from_be_bytes(msg.args[4..8].try_into().unwrap()),
            };
            match msg.command {
                cmd::BEGIN_WRITE if self.refuses_every_write => Some((4, Vec::new())),
                cmd::BEGIN_WRITE
                    if self.refuses_first_write
                        && !self.heard.iter().any(|m| m.command == cmd::BEGIN_WRITE) =>
                {
                    Some((4, Vec::new()))
                }
                cmd::PARTITIONS => Some((0, partition_table())),
                // Five words in the order `nord_usb::wire::Status` decodes them. Its doc
                // describes this shape and the zero `dirty` and `spare` that classes
                // outside the libraries report.
                cmd::STATUS => {
                    let count = self.filled.as_ref().map_or(0, Vec::len) as u32;
                    let total: u32 = self.banks.iter().map(|(_, slots)| slots).sum();
                    // One unit per item, so `Status::slots()` returns the total bank
                    // capacity.
                    Some((0, words(&[count, total.saturating_sub(count), count, 0, 0])))
                }
                cmd::BANKS if !self.reports_geometry => Some((2, Vec::new())),
                cmd::BANKS if self.garbles_geometry => Some((0, vec![0xff, 0xff])),
                cmd::BANKS => {
                    let mut p = msg.args[0..4].to_vec();
                    p.push(self.banks.len() as u8);
                    for (name, slots) in &self.banks {
                        p.extend_from_slice(&(name.len() as u32).to_be_bytes());
                        p.extend_from_slice(name.as_bytes());
                        p.extend_from_slice(&slots.to_be_bytes());
                    }
                    Some((0, p))
                }
                cmd::FOCUS => match self.focus {
                    Some(at) => Some((0, words(&[at.bank, at.slot]))),
                    None => Some((1, Vec::new())),
                },
                cmd::NEXT_SLOT if !self.enumerates => Some((op::ENUMERATION_DISABLED, Vec::new())),
                cmd::NEXT_SLOT => {
                    let from = at();
                    // Third word is the direction, which `op::next_occupied` always
                    // sends because the instrument answers its absence with
                    // `op::ENUMERATION_DISABLED`.
                    let Some(dir) = msg.args.get(8..12) else {
                        return Some((op::ENUMERATION_DISABLED, Vec::new()));
                    };
                    let backward = u32::from_be_bytes(dir.try_into().unwrap()) == 1;
                    let in_bank = self
                        .filled
                        .as_ref()
                        .into_iter()
                        .flatten()
                        .filter_map(|(held, _)| (held.bank == from.bank).then_some(held.slot));
                    let hit = if backward {
                        in_bank
                            .filter(|s| from.slot == op::SLOT_BOUNDARY || *s < from.slot)
                            .max()
                    } else {
                        in_bank
                            .filter(|s| from.slot == op::SLOT_BOUNDARY || *s > from.slot)
                            .min()
                    };
                    match hit {
                        Some(slot) => Some((0, words(&[from.bank, slot]))),
                        None => Some((1, words(&[from.bank, op::SLOT_BOUNDARY]))),
                    }
                }
                cmd::READ => {
                    let (offset, want) = (
                        u32::from_be_bytes(msg.args[8..12].try_into().unwrap()),
                        u32::from_be_bytes(msg.args[12..16].try_into().unwrap()),
                    );
                    let at = at();
                    let mut p = words(&[at.bank, at.slot, offset, want]);
                    p.extend((offset..offset + want).map(occupant_byte));
                    Some((0, p))
                }
                cmd::INFO => {
                    let at = at();
                    // Confirmed on hardware.
                    // Status 3 marks the address-space boundary for geometry-free walks.
                    let capacity = self.banks.get(at.bank as usize).map(|(_, slots)| *slots);
                    if capacity.is_none_or(|slots| at.slot >= slots) {
                        return Some((3, Vec::new()));
                    }
                    match &self.filled {
                        Some(_) => match self.holds(at) {
                            Some(name) => Some((0, info_payload(at, name, self.occupant))),
                            None => Some((1, Vec::new())),
                        },
                        None => match self.info {
                            0 => Some((0, info_payload(at, "something", self.occupant))),
                            status => Some((status, Vec::new())),
                        },
                    }
                }
                _ => None,
            }
        }

        /// Program and UI services reuse command numbers, so return Program frames only.
        fn commands(&self) -> Vec<u32> {
            self.heard
                .iter()
                .filter(|msg| matches!(msg.service, Service::Program))
                .map(|msg| msg.command)
                .collect()
        }

        fn first(&self, command: u32) -> Option<&Message> {
            self.heard.iter().find(|msg| msg.command == command)
        }
    }

    fn words(of: &[u32]) -> Vec<u8> {
        of.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    /// Encode all class partitions with their reported allocation units.
    fn partition_table() -> Vec<u8> {
        const COUNT: u32 = 8;
        const UNREAD_FIELDS: usize = 25;

        let mut p = vec![COUNT as u8];
        for index in 0..COUNT {
            let name = format!("Partition {index}");
            p.extend_from_slice(&(name.len() as u32).to_be_bytes());
            p.extend_from_slice(name.as_bytes());
            let unit: u32 = match ObjectClass::from_raw(index).is_library() {
                true => 131_064,
                false => 1,
            };
            p.extend_from_slice(&unit.to_be_bytes());
            p.resize(p.len() + UNREAD_FIELDS, 0);
        }
        p
    }

    /// The byte at `offset` of every occupant's body: every chunk differs, so a chunk put
    /// back at the wrong offset is caught.
    fn occupant_byte(offset: u32) -> u8 {
        (offset.wrapping_mul(2_654_435_761) >> 24) as u8
    }

    /// An occupant's body of `len` bytes, a chunk at a time, as its CRC-32.
    fn occupant_crc(len: u32) -> u32 {
        let mut crc = nord_format::crc::Crc32Stream::new();
        let mut chunk = Vec::with_capacity(1 << 16);
        for start in (0..len).step_by(1 << 16) {
            chunk.clear();
            chunk.extend((start..len.min(start + (1 << 16))).map(occupant_byte));
            crc.update(&chunk);
        }
        crc.value()
    }

    /// The schema version every occupant reports.
    const OCCUPANT_VERSION: u32 = 4;

    fn info_payload(at: Location, name: &str, (len, format): (u32, &str)) -> Vec<u8> {
        let mut p = words(&[at.bank, at.slot, len]);
        p.extend_from_slice(format.as_bytes());
        p.extend_from_slice(&words(&[
            OCCUPANT_VERSION,
            u32::MAX,
            u32::MAX,
            name.len() as u32,
        ]));
        p.extend_from_slice(name.as_bytes());
        p.extend_from_slice(&u32::MAX.to_be_bytes());
        p
    }

    impl Transport for Puppet {
        async fn write(&mut self, buf: &[u8]) -> nord_usb::Result<()> {
            let msg = Message::decode(buf)?;
            let spoken = matches!(msg.service, Service::Ui)
                && matches!(msg.command, ui::LABEL | ui::PERCENT)
                || self.unacknowledged(&msg);
            let (status, payload) = match self.answer(&msg) {
                Some(answered) => answered,
                None => (0, vec![0; 32]),
            };
            if !spoken {
                let mut args = status.to_be_bytes().to_vec();
                args.extend_from_slice(&payload);
                self.replies.push_back(
                    Message::new(msg.service, msg.subsystem, msg.command + 1, args).encode(),
                );
            }
            self.deaf |= self.hangs_up_on_delete
                && matches!(msg.service, Service::Program)
                && msg.command == cmd::DELETE;
            let mut msg = msg;
            if msg.command == cmd::WRITE_DATA && matches!(msg.service, Service::Program) {
                // The address, the offset and the length, then the chunk.
                let chunk = &msg.args[16..];
                self.data.update(chunk);
                self.data_len += chunk.len() as u64;
                if !self.keeps_data {
                    msg.args.truncate(16);
                }
            }
            self.heard.push(msg);
            Ok(())
        }

        async fn read(&mut self, _max: usize) -> nord_usb::Result<Vec<u8>> {
            if self.deaf {
                return Err(Error::Transport("the device stopped answering".into()));
            }
            self.replies
                .pop_front()
                .ok_or_else(|| Error::Transport("nothing to read".into()))
        }
    }

    fn a_program() -> Vec<u8> {
        let ctx = egui::Context::default();
        let mut workspace = crate::workspace::Workspace::new(ctx);
        let mut log = crate::log::Log::default();
        let id = workspace
            .create(crate::workspace::Fresh::Program, &mut log)
            .expect("a fresh default");
        workspace.get(id).expect("just made").bytes.to_vec()
    }

    fn drive(puppet: &mut Puppet, cmd: DeviceCmd) -> (Flow, Receiver<DeviceEvent>) {
        drive_keeping(puppet, cmd, &Scratch::default())
    }

    /// [`drive`], keeping an occupant too large to hold in `scratch`.
    fn drive_keeping(
        puppet: &mut Puppet,
        cmd: DeviceCmd,
        scratch: &Scratch,
    ) -> (Flow, Receiver<DeviceEvent>) {
        let (tx, events) = std::sync::mpsc::channel();
        let emit = Emit::new(tx, egui::Context::default());
        let lent = std::mem::replace(puppet, Puppet::new(1));
        let mut device = Device::new(lent);
        let flow = nord_usb::block_on(run(&mut device, cmd, scratch, &emit));
        *puppet = device.into_transport();
        (flow, events)
    }

    fn written_names(device: &Puppet) -> Vec<String> {
        device
            .heard
            .iter()
            .filter(|msg| msg.command == cmd::BEGIN_WRITE)
            .map(|msg| {
                let name_arg = &msg.args[6 * size_of::<u32>()..];
                let (len, name) = name_arg.split_at(size_of::<u32>());
                let len = u32::from_be_bytes(len.try_into().expect("four bytes")) as usize;
                assert_eq!(name.len(), len, "the name is length-prefixed");
                String::from_utf8(name.to_vec()).expect("a name is UTF-8")
            })
            .collect()
    }

    fn written_name(device: &Puppet) -> String {
        let mut names = written_names(device);
        assert_eq!(names.len(), 1, "one write");
        names.remove(0)
    }

    /// A read of a slot the instrument reports empty is an answer, not a fault: the
    /// queue needs to hear that the slot is empty to stop waiting on it.
    #[test]
    fn a_read_of_an_empty_slot_is_forwarded_as_vacant() {
        let at = Location { bank: 0, slot: 3 };
        let mut device = Puppet::stocked(&[("Bank 1", 50)], &[]);
        let (_, events) = drive(
            &mut device,
            DeviceCmd::Get {
                class: ObjectClass::Program,
                at,
                why: Purpose::Compare,
            },
        );

        let said: Vec<DeviceEvent> = events.try_iter().collect();
        assert!(
            said.iter().any(|event| matches!(
                event,
                DeviceEvent::Vacant {
                    at: empty,
                    why: Purpose::Compare,
                    ..
                } if *empty == at
            )),
            "the empty slot was reported"
        );
        assert!(
            !said
                .iter()
                .any(|event| matches!(event, DeviceEvent::OpFailed(_) | DeviceEvent::Got { .. })),
            "and neither failed nor handed anything back"
        );
    }

    #[test]
    fn a_put_names_the_slot_in_the_write_itself() {
        let at = Location { bank: 6, slot: 3 };
        for class in [ObjectClass::Program, ObjectClass::Sample] {
            let mut device = Puppet::new(1);
            let (flow, _) = drive(
                &mut device,
                DeviceCmd::Put {
                    id: 1,
                    class,
                    at,
                    name: "Africa-Split.ne5p".into(),
                    payload: Payload::Bytes(a_program()),
                },
            );

            assert!(flow == Flow::Continue, "the instrument is still there");
            assert_eq!(written_name(&device), "Africa-Split", "{}", class.label());
            assert_eq!(
                counted(&device, cmd::RENAME),
                0,
                "no follow-up rename for {}",
                class.label()
            );
        }
    }

    #[test]
    fn a_select_reports_where_it_left_the_panel() {
        let at = Location { bank: 6, slot: 3 };
        let mut device = Puppet::new(1);
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::Select {
                class: ObjectClass::Program,
                at,
            },
        );

        assert!(flow == Flow::Continue, "the instrument is still there");
        assert!(
            events.try_iter().any(|event| matches!(
                event,
                DeviceEvent::Focus {
                    class: ObjectClass::Program,
                    at: Some(loaded)
                } if loaded == at
            )),
            "the select said nothing about the panel"
        );
    }

    /// A reload selects the written slot only where the panel is still on it. A panel
    /// turned elsewhere since the last walk keeps its slot and the edits made there.
    #[test]
    fn a_reload_selects_only_the_written_slot_the_panel_is_on() {
        let written = Location { bank: 6, slot: 3 };
        let turned = Location { bank: 2, slot: 1 };

        let reloaded = |panel: Option<Location>| {
            let mut device = Puppet::new(1);
            if let Some(panel) = panel {
                device = device.focused_on(panel);
            }
            let (flow, events) = drive(
                &mut device,
                DeviceCmd::Reload {
                    class: ObjectClass::Program,
                    written: vec![Location { bank: 0, slot: 0 }, written],
                },
            );
            assert!(flow == Flow::Continue, "the instrument is still there");
            let focus = events.try_iter().find_map(|event| match event {
                DeviceEvent::Focus { at, .. } => Some(at),
                _ => None,
            });
            let selected = device.first(cmd::SELECT).map(|msg| Location {
                bank: u32::from_be_bytes(msg.args[0..4].try_into().unwrap()),
                slot: u32::from_be_bytes(msg.args[4..8].try_into().unwrap()),
            });
            (selected, focus)
        };

        assert_eq!(
            reloaded(Some(written)),
            (Some(written), Some(Some(written)))
        );
        assert_eq!(
            reloaded(Some(turned)),
            (None, Some(Some(turned))),
            "the panel was turned to another slot"
        );
        assert_eq!(
            reloaded(None),
            (None, Some(None)),
            "the panel has nothing loaded"
        );
    }

    #[test]
    fn a_restore_puts_the_occupants_own_name_back() {
        let at = Location { bank: 0, slot: 3 };
        let mut device =
            Puppet::stocked(&[("Bank 1", 50)], &[(at, "Squabble B")]).refusing_the_first_write();
        let (_, events) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at,
                name: "Africa-Split.ne5p".into(),
                payload: Payload::Bytes(a_program()),
            },
        );

        assert_eq!(written_names(&device), ["Africa-Split", "Squabble B"]);
        let said: Vec<DeviceEvent> = events.try_iter().collect();
        assert_eq!(
            failures(&said).len(),
            1,
            "one refusal is one failure: {:?}",
            failures(&said)
        );
    }

    /// A write that fails and cannot be put back is still one failure, and the occupant
    /// it displaced reaches the local list once, under the name its bytes are filed as.
    #[test]
    fn an_occupant_that_cannot_be_restored_is_rescued_once() {
        let at = Location { bank: 0, slot: 3 };
        let mut device =
            Puppet::stocked(&[("Bank 1", 50)], &[(at, "Squabble B")]).refusing_every_write();
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at,
                name: "Africa-Split.ne5p".into(),
                payload: Payload::Bytes(a_program()),
            },
        );
        assert!(flow == Flow::Continue, "a refusal is not a disconnection");

        let said: Vec<DeviceEvent> = events.try_iter().collect();
        let rescued: Vec<&str> = said
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Rescued { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(rescued, ["nord-rescued-1-4.ne5p"]);
        assert_eq!(
            failures(&said).len(),
            1,
            "one refusal is one failure: {:?}",
            failures(&said)
        );
    }

    #[test]
    fn an_occupant_whose_delete_went_unanswered_is_rescued() {
        let at = Location { bank: 0, slot: 3 };
        let mut device =
            Puppet::stocked(&[("Bank 1", 50)], &[(at, "Squabble B")]).hanging_up_on_delete();
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at,
                name: "Africa-Split.ne5p".into(),
                payload: Payload::Bytes(a_program()),
            },
        );
        assert!(flow == Flow::Lost, "the instrument stopped answering");

        let said: Vec<DeviceEvent> = events.try_iter().collect();
        let rescued: Vec<&str> = said
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Rescued { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(rescued, ["nord-rescued-1-4.ne5p"]);
        let failed = failures(&said);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].contains("may have been deleted"), "{}", failed[0]);
        assert_eq!(counted(&device, cmd::BEGIN_WRITE), 0, "nothing was written");
    }

    /// Every item logs its own line, but the batch reports success once.
    #[test]
    fn a_batch_succeeds_once_however_many_items_it_carries() {
        let bytes = a_program();
        let item = |slot, name: &str| Outgoing {
            id: slot as u64,
            at: Location { bank: 6, slot },
            name: name.into(),
            payload: Payload::Bytes(bytes.clone()),
        };
        let mut device = Puppet::new(1);
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::SendAll {
                class: ObjectClass::Program,
                items: vec![item(3, "Africa-Split.ne5p"), item(4, "Squabble-B.ne5p")],
            },
        );
        assert!(flow == Flow::Continue);

        let said: Vec<DeviceEvent> = events.try_iter().collect();
        let landed: Vec<&str> = said
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::OpOk(note) => Some(note.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(landed.len(), 1, "{landed:?}");
        assert!(landed[0].contains("wrote 2 of 2"), "{}", landed[0]);
        let sent = said
            .iter()
            .filter(|event| matches!(event, DeviceEvent::Sent { .. }))
            .count();
        assert_eq!(sent, 2, "each item was reported sent");
    }

    #[test]
    fn a_put_into_a_buffer_class_never_deletes_the_slot() {
        let at = Location { bank: 0, slot: 2 };
        let put = |class| DeviceCmd::Put {
            id: 1,
            class,
            at,
            name: "Africa-Split.ne5p".into(),
            payload: Payload::Bytes(a_program()),
        };

        let mut live = Puppet::stocked(&[("Live", 3)], &[(at, "Live 3")]);
        let (flow, _) = drive(&mut live, put(ObjectClass::Live));
        assert!(flow == Flow::Continue, "the instrument is still there");
        assert_eq!(counted(&live, cmd::DELETE), 0, "nothing was emptied");
        assert_eq!(counted(&live, cmd::BEGIN_WRITE), 1, "and the bytes went");

        let mut program = Puppet::stocked(&[("Bank 1", 50)], &[(at, "Africa")]);
        drive(&mut program, put(ObjectClass::Program));
        assert_eq!(
            counted(&program, cmd::DELETE),
            1,
            "a class that refuses an occupied slot still makes room"
        );
    }

    #[test]
    fn a_class_that_stores_no_name_is_not_reported_as_named() {
        let at = Location { bank: 0, slot: 2 };
        let mut device = Puppet::stocked(&[("Live", 3)], &[(at, "Live 3")]);
        let (flow, _) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Live,
                at,
                name: "Africa-Split.ne5l".into(),
                payload: Payload::Bytes(a_program()),
            },
        );
        assert!(flow == Flow::Continue);
        assert!(device.first(cmd::WRITE_DATA).is_some(), "the bytes went");
        let told = wrote(ObjectClass::Live, at, "Africa-Split.ne5l", "Africa-Split");
        assert!(!told.contains("named"), "{told}");
    }

    #[test]
    fn a_nameless_asset_still_gets_its_bytes_written() {
        let mut device = Puppet::new(1);
        let (flow, _) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
                name: "   ".into(),
                payload: Payload::Bytes(a_program()),
            },
        );
        assert!(flow == Flow::Continue);
        assert!(device.first(cmd::WRITE_DATA).is_some(), "the bytes went");
        assert_eq!(written_name(&device), "", "nothing to name it");
    }

    #[test]
    fn a_nameless_asset_leaves_the_slots_name_alone() {
        let at = Location { bank: 0, slot: 3 };
        let mut device = Puppet::stocked(&[("Bank 1", 50)], &[(at, "Squabble B")]);
        drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at,
                name: "   ".into(),
                payload: Payload::Bytes(a_program()),
            },
        );

        assert_eq!(written_name(&device), "Squabble B");
    }

    #[test]
    fn every_item_of_a_batch_is_named() {
        let bytes = a_program();
        let item = |slot, name: &str| Outgoing {
            id: slot as u64,
            at: Location { bank: 6, slot },
            name: name.into(),
            payload: Payload::Bytes(bytes.clone()),
        };
        let mut device = Puppet::new(1);
        let (flow, _) = drive(
            &mut device,
            DeviceCmd::SendAll {
                class: ObjectClass::Program,
                items: vec![item(3, "Africa-Split.ne5p"), item(4, "Squabble-B.ne5p")],
            },
        );
        assert!(flow == Flow::Continue);
        assert_eq!(
            written_names(&device),
            ["Africa-Split", "Squabble-B"],
            "one name per item"
        );
        // One session for the geometry read and one destructive session around the pair.
        let opens = device
            .commands()
            .into_iter()
            .filter(|command| *command == cmd::SESSION_OPEN)
            .count();
        assert_eq!(opens, 2);
    }

    #[test]
    fn a_command_reports_its_first_failure_s_kind_and_any_hang_up() {
        let mut fault = Fault::default();
        fault.saw(&Error::DeviceStatus(op::OCCUPIED));
        fault.saw(&Error::Transport("cable".into()));
        assert_eq!(fault.kind, Some(ErrKind::DeviceStatus(op::OCCUPIED)));
        assert!(fault.gone);
    }

    #[test]
    fn a_failure_the_instrument_answered_after_is_not_a_hang_up() {
        let mut fault = Fault::default();
        fault.named(&Error::Transport("the write timed out".into()));
        assert_eq!(fault.kind, Some(ErrKind::Transport));
        assert!(!fault.gone);
    }

    #[test]
    fn a_transport_that_fails_is_the_instrument_going_away() {
        let (flow, _) = drive(
            &mut Puppet::deaf(),
            DeviceCmd::SlotInfo {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
        );
        assert!(flow == Flow::Lost);
    }

    fn a_small_library() -> Puppet {
        Puppet::stocked(
            &[("Grand", 50), ("Upright", 30)],
            &[
                (Location { bank: 0, slot: 0 }, "Royal Grand 3D"),
                (Location { bank: 1, slot: 2 }, "Queen Upright"),
            ],
        )
    }

    fn holdings(bank: &(u32, Vec<Option<String>>)) -> (u32, usize, Vec<(usize, &str)>) {
        let held = bank
            .1
            .iter()
            .enumerate()
            .filter_map(|(slot, name)| Some((slot, name.as_deref()?)))
            .collect();
        (bank.0, bank.1.len(), held)
    }

    fn scan(class: ObjectClass) -> DeviceCmd {
        DeviceCmd::ScanClass { class }
    }

    fn scanned(events: Receiver<DeviceEvent>) -> Vec<(u32, Vec<Option<String>>)> {
        events
            .try_iter()
            .filter_map(|event| match event {
                DeviceEvent::BankScanned { bank, slots, .. } => Some((
                    bank,
                    slots
                        .into_iter()
                        .map(|slot| slot.map(|info| info.name))
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// The failures reported for one command. `run` emits one; a second means a step
    /// inside the command reported its own, which the send queue would record against
    /// the next waiting entry.
    fn failures(said: &[DeviceEvent]) -> Vec<&str> {
        said.iter()
            .filter_map(|event| match event {
                DeviceEvent::OpFailed(why) => Some(why.as_str()),
                _ => None,
            })
            .collect()
    }

    fn refused(events: Receiver<DeviceEvent>) -> String {
        let said: Vec<DeviceEvent> = events.try_iter().collect();
        failures(&said).join(" | ")
    }

    fn counted(device: &Puppet, command: u32) -> usize {
        device
            .commands()
            .into_iter()
            .filter(|held| *held == command)
            .count()
    }

    #[test]
    fn a_scan_publishes_a_bank_before_reading_the_next_one() {
        struct ObserveProgress {
            puppet: Puppet,
            events: Receiver<DeviceEvent>,
            first_bank_arrived: bool,
        }

        impl Transport for ObserveProgress {
            async fn write(&mut self, buf: &[u8]) -> nord_usb::Result<()> {
                let msg = Message::decode(buf)?;
                if msg.service == Service::Program
                    && msg.command == cmd::INFO
                    && msg.args[..4] == 1u32.to_be_bytes()
                {
                    self.first_bank_arrived = self.events.try_iter().any(|event| {
                        matches!(event, DeviceEvent::BankScanned { class: ObjectClass::Program, bank: 1, slots }
                            if slots.first().and_then(Option::as_ref).is_some_and(|info| info.name == "First"))
                    });
                }
                self.puppet.write(buf).await
            }

            async fn read(&mut self, max: usize) -> nord_usb::Result<Vec<u8>> {
                self.puppet.read(max).await
            }
        }

        let (tx, events) = std::sync::mpsc::channel();
        let emit = Emit::new(tx, egui::Context::default());
        let puppet = Puppet::stocked(
            &[("Bank 1", 1), ("Bank 2", 1)],
            &[
                (Location { bank: 0, slot: 0 }, "First"),
                (Location { bank: 1, slot: 0 }, "Second"),
            ],
        );
        let mut device = Device::new(ObserveProgress {
            puppet,
            events,
            first_bank_arrived: false,
        });
        let flow = nord_usb::block_on(run(
            &mut device,
            scan(ObjectClass::Program),
            &Scratch::default(),
            &emit,
        ));
        assert!(flow == Flow::Continue);
        assert!(
            device.transport().first_bank_arrived,
            "the first bank's names and progress were withheld until the second bank finished"
        );
    }

    #[test]
    fn a_scan_asks_only_about_the_slots_that_hold_something() {
        let mut device = a_small_library();
        let (flow, events) = drive(&mut device, scan(ObjectClass::Piano));
        assert!(flow == Flow::Continue);

        let banks = scanned(events);
        assert_eq!(
            banks.iter().map(holdings).collect::<Vec<_>>(),
            vec![
                (1, 50, vec![(0, "Royal Grand 3D")]),
                (2, 30, vec![(2, "Queen Upright")]),
            ]
        );
        assert!(counted(&device, cmd::NEXT_SLOT) > 0, "the cursor was used");
        assert_eq!(counted(&device, cmd::INFO), 2, "not the 80 addresses");
    }

    #[test]
    fn a_device_that_refuses_to_enumerate_is_walked_slot_by_slot() {
        let mut device = a_small_library().no_enumeration();
        let (flow, events) = drive(&mut device, scan(ObjectClass::Piano));
        assert!(flow == Flow::Continue, "a refusal is not a disconnection");

        let banks = scanned(events);
        assert_eq!(
            banks.iter().map(holdings).collect::<Vec<_>>(),
            vec![
                (1, 50, vec![(0, "Royal Grand 3D")]),
                (2, 30, vec![(2, "Queen Upright")]),
            ],
            "the same folder, found the long way"
        );
        assert!(counted(&device, cmd::NEXT_SLOT) > 0, "it was tried");
        assert_eq!(counted(&device, cmd::INFO), 80);
    }

    /// The partition table says which classes exist. Every row is announced, including
    /// classes this app has no name for, so an unnamed folder is still listed.
    #[test]
    fn a_connection_announces_the_classes_the_instrument_declares() {
        let mut puppet = Puppet::stocked(&[("Bank 1", 50)], &[]);
        let (tx, events) = std::sync::mpsc::channel();
        let emit = Emit::new(tx, egui::Context::default());
        let lent = std::mem::replace(&mut puppet, Puppet::new(1));
        let mut device = Device::new(lent);

        let flow = nord_usb::block_on(announce(&mut device, &emit));
        assert!(flow == Flow::Continue);

        let announced: Vec<Vec<Partition>> = events
            .try_iter()
            .filter_map(|event| match event {
                DeviceEvent::Partitions(rows) => Some(rows),
                _ => None,
            })
            .collect();
        let [rows] = announced.as_slice() else {
            panic!("one announcement per connection, not {}", announced.len());
        };
        assert_eq!(
            rows.iter().map(|row| row.class).collect::<Vec<_>>(),
            (0..8).map(ObjectClass::from_raw).collect::<Vec<_>>(),
            "the table's index is the class code"
        );
        assert_eq!(rows[4].name, "Partition 4", "the device's name");
        // The libraries count blocks of net bytes; every other partition counts bytes.
        assert_eq!(
            rows.iter()
                .map(|row| row.unit.map(|unit| unit.get()))
                .collect::<Vec<_>>(),
            [1, 131_064, 1, 131_064, 1, 1, 1, 1].map(Some),
            "each partition's own unit"
        );
    }

    #[test]
    fn the_devices_own_geometry_shapes_the_scan() {
        let mut device = a_small_library();
        let (_, events) = drive(&mut device, scan(ObjectClass::Piano));

        let mut named = Vec::new();
        let mut widths = Vec::new();
        for event in events.try_iter() {
            match event {
                DeviceEvent::Geometry { banks, .. } => {
                    named = banks
                        .into_iter()
                        .map(|bank| (bank.name, bank.slots))
                        .collect()
                }
                DeviceEvent::BankScanned { slots, .. } => widths.push(slots.len()),
                _ => {}
            }
        }
        assert_eq!(
            named,
            vec![("Grand".to_string(), 50), ("Upright".to_string(), 30)],
            "the categories reach the browser by name"
        );
        assert_eq!(widths, vec![50, 30], "and its capacities");
    }

    #[test]
    fn an_oversized_geometry_is_refused_before_slot_reads() {
        let mut device = Puppet::stocked(&[("Bank 1", MOST_OCCUPIED + 1)], &[]);
        let (flow, events) = drive(&mut device, scan(ObjectClass::Program));

        assert!(flow == Flow::Continue);
        assert!(refused(events).contains("cannot be scanned completely"));
        assert_eq!(counted(&device, cmd::INFO), 0);
    }

    #[test]
    fn an_oversized_bank_scan_is_refused_before_slot_reads() {
        let mut device = Puppet::stocked(&[("Bank 1", MOST_OCCUPIED + 1)], &[]);
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::ScanBank {
                class: ObjectClass::Program,
                bank: 1,
            },
        );

        assert!(flow == Flow::Continue);
        assert!(refused(events).contains("cannot be scanned completely"));
        assert_eq!(counted(&device, cmd::INFO), 0);
    }

    #[test]
    fn a_rescan_takes_the_banks_capacity_from_the_instrument() {
        let at = Location { bank: 0, slot: 1 };
        let mut device = Puppet::stocked(&[("Bank 1", 4)], &[(at, "Africa Split")]);
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::ScanBank {
                class: ObjectClass::Program,
                bank: 1,
            },
        );

        assert!(flow == Flow::Continue);
        assert_eq!(
            scanned(events),
            vec![(1, vec![None, Some("Africa Split".to_string()), None, None])],
            "the declared four slots, not a walk to the host limit"
        );
        assert_eq!(counted(&device, cmd::INFO), 4);
    }

    #[test]
    fn a_bank_ending_before_its_declared_capacity_is_not_reported() {
        let mut device = Puppet::new(3);
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::ScanBank {
                class: ObjectClass::Program,
                bank: 1,
            },
        );

        assert!(flow == Flow::Continue);
        let events: Vec<DeviceEvent> = events.try_iter().collect();
        assert!(events.iter().any(
            |event| matches!(event, DeviceEvent::OpFailed(why) if why.contains("became inconsistent"))
        ));
        assert!(!events
            .iter()
            .any(|event| matches!(event, DeviceEvent::BankScanned { .. })));
    }

    #[test]
    fn a_class_whose_banks_are_refused_is_not_scanned() {
        let mut device = Puppet::stocked(
            &[("Bank 1", 50)],
            &[(Location { bank: 0, slot: 1 }, "Africa Split")],
        )
        .mute_about_geometry();
        let (flow, events) = drive(&mut device, scan(ObjectClass::Program));
        assert!(flow == Flow::Continue, "a refusal is not a disconnection");

        let why = refused(events);
        assert!(
            why.contains("status 0x2"),
            "the device's own refusal: {why}"
        );
        assert_eq!(counted(&device, cmd::INFO), 0, "and nothing was walked");
    }

    #[test]
    fn a_bank_list_that_will_not_decode_stops_the_scan() {
        let mut device = Puppet::stocked(&[("Bank 1", 50)], &[]).garbling_geometry();
        let (flow, events) = drive(&mut device, scan(ObjectClass::Program));
        assert!(flow == Flow::Continue, "a bad reply is not a dead pipe");

        let why = refused(events);
        assert!(why.contains("truncated"), "{why}");
        assert_eq!(counted(&device, cmd::INFO), 0, "and nothing was walked");
    }

    #[test]
    fn an_unbounded_bank_is_read_to_its_last_item() {
        let filled: Vec<(Location, &'static str)> = (0..60)
            .map(|slot| (Location { bank: 0, slot }, "Marimba"))
            .collect();
        let mut device = Puppet::stocked(&[("Samp Lib", Bank::UNBOUNDED)], &filled);
        let (flow, events) = drive(&mut device, scan(ObjectClass::Sample));
        assert!(flow == Flow::Continue);

        let banks = scanned(events);
        assert_eq!(banks.len(), 1);
        assert_eq!(banks[0].1.len(), 60, "all of them");
        assert!(banks[0].1.iter().all(Option::is_some));
    }

    #[test]
    fn an_unbounded_bank_without_an_end_does_not_report_a_partial_scan() {
        let at = Location { bank: 0, slot: 40 };
        let mut library =
            Puppet::stocked(&[("Samp Lib", Bank::UNBOUNDED)], &[(at, "Marimba")]).no_enumeration();
        let (flow, events) = drive(&mut library, scan(ObjectClass::Sample));

        assert!(flow == Flow::Continue);
        assert!(refused(events).contains("cannot be scanned completely"));
        assert_eq!(counted(&library, cmd::INFO), MOST_OCCUPIED as usize);
    }

    #[test]
    fn a_full_class_is_read_slot_by_slot_and_a_sparse_one_by_cursor() {
        let full: Vec<(Location, &'static str)> = (0..2)
            .flat_map(|bank| (0..50).map(move |slot| (Location { bank, slot }, "Africa Split")))
            .collect();
        let banks = [("Bank 1", 50), ("Bank 2", 50)];

        let mut dense = Puppet::stocked(&banks, &full);
        drive(&mut dense, scan(ObjectClass::Program));
        assert_eq!(counted(&dense, cmd::NEXT_SLOT), 0, "the cursor was skipped");
        assert_eq!(
            counted(&dense, cmd::INFO),
            100,
            "one per address, and no more"
        );

        let mut sparse = Puppet::stocked(&banks, &full[..2]);
        drive(&mut sparse, scan(ObjectClass::Program));
        assert!(counted(&sparse, cmd::NEXT_SLOT) > 0, "the cursor was used");
        assert!(counted(&sparse, cmd::INFO) < 100);
    }

    #[test]
    fn a_geometry_that_cannot_be_read_stops_the_write_before_any_frame_of_it() {
        let mut device = Puppet::stocked(&[("Bank 1", 50)], &[]).garbling_geometry();
        let (flow, _) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at: Location { bank: 0, slot: 3 },
                name: "Africa-Split.ne5p".into(),
                payload: Payload::Bytes(a_program()),
            },
        );
        assert!(flow == Flow::Continue, "not a disconnection");
        assert_eq!(counted(&device, cmd::BEGIN_WRITE), 0);
        assert_eq!(counted(&device, cmd::WRITE_DATA), 0);
    }

    #[test]
    fn a_scan_reports_the_slot_the_panel_has_loaded() {
        let panel = Location { bank: 1, slot: 2 };
        let mut device = a_small_library().focused_on(panel);
        let (_, events) = drive(&mut device, scan(ObjectClass::Piano));

        let focused: Vec<Location> = events
            .try_iter()
            .filter_map(|event| match event {
                DeviceEvent::Focus { at, .. } => at,
                _ => None,
            })
            .collect();
        assert_eq!(focused, vec![panel]);
    }

    #[test]
    fn a_write_past_the_end_is_refused_before_anything_is_deleted() {
        let mut device = a_small_library();
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
                name: "Africa-Split.ne5p".into(),
                payload: Payload::Bytes(a_program()),
            },
        );
        assert!(flow == Flow::Continue, "a refusal is not a disconnection");

        let why = refused(events);
        assert!(why.contains("bank 7 does not exist"), "{why}");
        assert!(
            why.contains("Grand, Upright"),
            "in the panel's own words: {why}"
        );

        assert_eq!(counted(&device, cmd::DELETE), 0, "nothing was emptied");
        assert_eq!(
            counted(&device, cmd::BEGIN_WRITE),
            0,
            "and nothing was sent"
        );
    }

    /// A piano library resting in a file of its own under a fresh folder, with its bytes.
    fn resting_piano(blocks: u16) -> (crate::testing::Temp, Arc<OnDisk>, Vec<u8>) {
        let dir = crate::testing::Temp::new();
        let bytes = crate::testing::piano(blocks);
        let file = crate::testing::on_disk(&dir, "Upright.npno", &bytes);
        (dir, file, bytes)
    }

    /// A write of `payload` to bank 1 slot 1 of the piano library.
    fn put_piano(payload: Payload) -> DeviceCmd {
        DeviceCmd::Put {
            id: 1,
            class: ObjectClass::Piano,
            at: Location { bank: 0, slot: 0 },
            name: "Upright.npno".into(),
            payload,
        }
    }

    /// The wire body of `file`, and its CRC-32: what the frames of a write carry, and the
    /// checksum a slot holding it reports.
    fn wire_body(file: &[u8]) -> (std::ops::Range<usize>, u32) {
        let info = nord_format::cbin::inspect(&mut std::io::Cursor::new(file)).unwrap();
        let start = info.header.generation.body_start() as usize;
        let body = start..start + info.body_len as usize;
        let crc = nord_format::crc::crc32(&file[body.clone()]);
        (body, crc)
    }

    /// Every frame sent, encoded, with the timestamp `BEGIN_WRITE` carries cleared, since
    /// two writes a second apart differ there and nowhere else.
    fn frames(device: &Puppet) -> Vec<Vec<u8>> {
        device
            .heard
            .iter()
            .map(|msg| {
                let mut msg = msg.clone();
                if msg.command == cmd::BEGIN_WRITE && matches!(msg.service, Service::Program) {
                    msg.args[16..20].fill(0);
                }
                msg.encode()
            })
            .collect()
    }

    /// A piano library of a vendor's size is sent from its file a transfer chunk at a
    /// time. No read of it and no allocation comes near its size, and the frames carry its
    /// body.
    #[test]
    fn a_resting_piano_is_sent_without_ever_being_held_whole() {
        let (_dir, file, bytes) = resting_piano(u16::MAX);
        let len = bytes.len();
        assert!(len > 200_000_000, "{len} bytes is a vendor's size");
        let (body, crc32) = wire_body(&bytes);
        drop(bytes);

        let mut device = Puppet::stocked(&[("Bank 1", 4000)], &[]).forgetful();
        let payload = Payload::File {
            file: file.clone(),
            crc32,
        };
        let ((flow, events), largest) =
            crate::testing::largest_allocation(|| drive(&mut device, put_piano(payload)));

        assert!(flow == Flow::Continue);
        let said: Vec<DeviceEvent> = events.try_iter().collect();
        assert_eq!(failures(&said), Vec::<&str>::new());
        assert!(
            said.iter()
                .any(|event| matches!(event, DeviceEvent::Sent { .. })),
            "it landed"
        );
        assert!(
            largest < len / 100,
            "the largest allocation was {largest} bytes, of a {len}-byte file"
        );
        let reads = file.take_reads();
        let widest = reads.iter().map(|read| read.end - read.start).max();
        assert!(
            widest.is_some_and(|widest| widest <= 64 << 10),
            "the widest read was {widest:?} bytes"
        );
        assert_eq!(
            device.data_len,
            body.len() as u64,
            "the frames carry the body"
        );
        assert_eq!(device.data.value(), crc32, "and the body is the file's");
    }

    /// Sent from its file or from memory, one file makes the same exchange, frame for
    /// frame.
    #[test]
    fn a_file_sent_from_disk_makes_the_exchange_its_bytes_make() {
        let (_dir, file, bytes) = resting_piano(40);
        assert!(bytes.len() > 3 * 32720, "a body of several chunks");
        let at = Location { bank: 0, slot: 0 };
        let mut sent = Vec::new();
        for payload in [
            Payload::File { file, crc32: 0 },
            Payload::Bytes(bytes.clone()),
        ] {
            let mut device = Puppet::stocked(&[("Bank 1", 400)], &[(at, "Grand")]);
            let (flow, events) = drive(&mut device, put_piano(payload));
            assert!(flow == Flow::Continue);
            let said: Vec<DeviceEvent> = events.try_iter().collect();
            assert_eq!(failures(&said), Vec::<&str>::new());
            sent.push(frames(&device));
        }
        let (from_disk, from_memory) = (&sent[0], &sent[1]);
        assert_eq!(from_disk.len(), from_memory.len(), "frames sent");
        let differs = from_disk.iter().zip(from_memory).position(|(a, b)| a != b);
        assert_eq!(differs, None, "the first frame that differs");
    }

    /// An occupant goes back into its slot from a session of its own once a write stops
    /// partway, alone or in a batch: an instrument keeps an unfinished write as an object
    /// of its own when another write follows it in the same session.
    #[test]
    fn a_write_that_stops_partway_closes_its_session_before_the_occupant_goes_back() {
        let (_dir, file, _) = resting_piano(40);
        let payload = || Payload::File {
            file: file.clone(),
            crc32: 0,
        };
        drive(
            &mut Puppet::stocked(&[("Bank 1", 400)], &[]),
            put_piano(payload()),
        );
        let reads = file.take_reads().len();
        let at = Location { bank: 0, slot: 0 };
        let batch = DeviceCmd::SendAll {
            class: ObjectClass::Piano,
            items: vec![Outgoing {
                id: 1,
                at,
                name: "Upright.npno".into(),
                payload: payload(),
            }],
        };
        for send in [put_piano(payload()), batch] {
            let label = send.label();
            file.vanish_after(reads - 2);
            let mut device = Puppet::stocked(&[("Bank 1", 400)], &[(at, "Grand")]);
            drive(&mut device, send);

            let commands = device.commands();
            let begun: Vec<usize> = (0..commands.len())
                .filter(|&i| commands[i] == cmd::BEGIN_WRITE)
                .collect();
            let [write, restore] = begun[..] else {
                panic!(
                    "{label}: {} writes begun, not the write and its restore",
                    begun.len()
                )
            };
            let between = &commands[write..restore];
            assert!(
                between.contains(&cmd::SESSION_CLOSE) && between.contains(&cmd::SESSION_OPEN),
                "{label}: the restore began in the session of the write that stopped: \
                 {between:#04x?}"
            );
        }
    }

    /// A file that stops reading partway through its transfer fails the send as a refused
    /// write does: the occupant is put back, the instrument stays attached, nothing is
    /// reported sent, and the failure names the file.
    #[test]
    fn a_file_that_vanishes_mid_send_puts_the_occupant_back_and_says_so() {
        let (_dir, file, _) = resting_piano(40);
        let payload = || Payload::File {
            file: file.clone(),
            crc32: 0,
        };
        drive(
            &mut Puppet::stocked(&[("Bank 1", 400)], &[]),
            put_piano(payload()),
        );
        let reads = file.take_reads().len();
        // The last two reads are the transfer's last two chunks.
        file.vanish_after(reads - 2);

        let at = Location { bank: 0, slot: 0 };
        let mut device = Puppet::stocked(&[("Bank 1", 400)], &[(at, "Grand")]);
        let (flow, events) = drive(&mut device, put_piano(payload()));

        assert!(flow == Flow::Continue, "a file is not the instrument");
        assert_eq!(
            written_names(&device),
            ["Upright", "Grand"],
            "sent, then restored"
        );
        assert!(
            device.data_len > 0,
            "it failed partway through the transfer"
        );
        let said: Vec<DeviceEvent> = events.try_iter().collect();
        assert!(!said
            .iter()
            .any(|event| matches!(event, DeviceEvent::Sent { .. })));
        let failed = failures(&said);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(
            failed[0].contains("“Upright.npno” could not be read from its file")
                && failed[0].contains("1:1 was restored"),
            "{}",
            failed[0]
        );
    }

    /// An occupant's body of `len` bytes, held whole, for a test that compares against it.
    fn occupant_body(len: u32) -> Vec<u8> {
        (0..len).map(occupant_byte).collect()
    }

    /// The file an occupant of `len` bytes at `at` is kept as: the one its read rebuilds.
    fn occupant_file(at: Location, len: u32) -> Vec<u8> {
        envelope::wrap("npno", at, OCCUPANT_VERSION, &occupant_body(len)).unwrap()
    }

    /// Where a test keeps occupants: a folder of its own.
    fn scratch_in(dir: &crate::testing::Temp) -> Scratch {
        let scratch = Scratch::default();
        scratch.keep_in(Some(dir.0.clone()));
        scratch
    }

    /// A piano slot holding an occupant of a vendor's size is read into a file, not into
    /// memory, and a refused write puts it back from that file, after which the file is
    /// gone. No allocation comes near the occupant's size.
    #[test]
    fn a_large_occupant_waits_in_a_file_and_is_put_back_from_it_in_bounded_memory() {
        let len: u32 = 48 << 20;
        let crc = occupant_crc(len);
        let at = Location { bank: 0, slot: 0 };
        let dir = crate::testing::Temp::new();
        let scratch = scratch_in(&dir);
        let mut device = Puppet::stocked(&[("Bank 1", 400)], &[(at, "Grand")])
            .holding(len, "npno")
            .refusing_the_first_write()
            .forgetful();
        let payload = Payload::Bytes(crate::testing::piano(2));

        let ((flow, events), largest) = crate::testing::largest_allocation(|| {
            drive_keeping(&mut device, put_piano(payload), &scratch)
        });

        assert!(flow == Flow::Continue);
        assert!(
            largest < len as usize / 100,
            "the largest allocation was {largest} bytes, of a {len}-byte occupant"
        );
        assert_eq!(
            written_names(&device),
            ["Upright", "Grand"],
            "refused, then restored"
        );
        assert_eq!(
            device.data_len,
            u64::from(len),
            "the restore carried a body"
        );
        assert_eq!(device.data.value(), crc, "and the body is the occupant's");
        let said: Vec<DeviceEvent> = events.try_iter().collect();
        let failed = failures(&said);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].contains("1:1 was restored"), "{}", failed[0]);
        assert_eq!(dir.names(""), Vec::<String>::new(), "the file is let go");
    }

    /// Put back from its file, an occupant makes the exchange its bytes make when they are
    /// written from memory, frame for frame.
    #[test]
    fn a_large_occupant_put_back_from_its_file_makes_the_exchange_its_bytes_make() {
        let len: u32 = 3 << 20;
        let at = Location { bank: 0, slot: 0 };
        let dir = crate::testing::Temp::new();
        let mut restored = Puppet::stocked(&[("Bank 1", 400)], &[(at, "Grand")])
            .holding(len, "npno")
            .refusing_the_first_write();
        drive_keeping(
            &mut restored,
            put_piano(Payload::Bytes(crate::testing::piano(2))),
            &scratch_in(&dir),
        );
        let mut written = Puppet::stocked(&[("Bank 1", 400)], &[]);
        drive(
            &mut written,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Piano,
                at,
                name: "Grand.npno".into(),
                payload: Payload::Bytes(occupant_file(at, len)),
            },
        );

        // From the reservation the write makes, through the session's close.
        let from_status = |device: &Puppet| {
            let frames = frames(device);
            let status = device
                .heard
                .iter()
                .rposition(|msg| {
                    msg.command == cmd::STATUS && matches!(msg.service, Service::Program)
                })
                .expect("a library write reserves space");
            frames[status..].to_vec()
        };
        let (from_file, from_memory) = (from_status(&restored), from_status(&written));
        assert!(from_file.len() > 100, "a body of many chunks");
        assert_eq!(from_file.len(), from_memory.len(), "frames sent");
        let differs = from_file.iter().zip(&from_memory).position(|(a, b)| a != b);
        assert_eq!(differs, None, "the first frame that differs");
    }

    /// A large occupant that cannot be put back stays in the file it was read into, which
    /// is whole, and the failure says where it is.
    #[test]
    fn a_large_occupant_that_cannot_be_put_back_stays_in_its_file() {
        let len: u32 = 2 << 20;
        let at = Location { bank: 0, slot: 0 };
        let dir = crate::testing::Temp::new();
        let mut device = Puppet::stocked(&[("Bank 1", 400)], &[(at, "Grand")])
            .holding(len, "npno")
            .refusing_every_write();
        let (flow, events) = drive_keeping(
            &mut device,
            put_piano(Payload::Bytes(crate::testing::piano(2))),
            &scratch_in(&dir),
        );
        assert!(flow == Flow::Continue, "a refusal is not a disconnection");

        let kept = dir.at("nord-rescued-1-1.npno");
        assert_eq!(dir.names(""), ["nord-rescued-1-1.npno"]);
        assert!(
            dir.read("nord-rescued-1-1.npno") == occupant_file(at, len),
            "the kept file is the occupant's"
        );
        let said: Vec<DeviceEvent> = events.try_iter().collect();
        let places: Vec<&str> = said
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Kept { place, .. } => Some(place.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(places, [kept.display().to_string()]);
        let failed = failures(&said);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(
            failed[0].contains(&format!("kept at {}", kept.display())),
            "{}",
            failed[0]
        );
    }

    /// A piano slot something is queued for is read through its checksum: the queue hears
    /// the body's length and CRC-32, nothing of the body is held, and nothing is kept.
    #[test]
    fn a_piano_slot_is_compared_through_its_checksum_in_bounded_memory() {
        let len: u32 = 48 << 20;
        let crc = occupant_crc(len);
        let at = Location { bank: 0, slot: 0 };
        let mut device = Puppet::stocked(&[("Bank 1", 400)], &[(at, "Grand")]).holding(len, "npno");

        let ((flow, events), largest) = crate::testing::largest_allocation(|| {
            drive(
                &mut device,
                DeviceCmd::Get {
                    class: ObjectClass::Piano,
                    at,
                    why: Purpose::Compare,
                },
            )
        });

        assert!(flow == Flow::Continue);
        assert!(
            largest < len as usize / 100,
            "the largest allocation was {largest} bytes, of a {len}-byte occupant"
        );
        let said: Vec<DeviceEvent> = events.try_iter().collect();
        let summed: Vec<(u32, u32)> = said
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Summed { received, .. } => {
                    Some((received.info.body_len, received.body_crc32))
                }
                _ => None,
            })
            .collect();
        assert_eq!(summed, [(len, crc)]);
        assert!(
            !said
                .iter()
                .any(|event| matches!(event, DeviceEvent::Got { .. })),
            "no bytes reach the queue"
        );
    }

    #[test]
    fn a_refusal_is_not_a_disconnection() {
        let (flow, _) = drive(
            &mut Puppet::new(3),
            DeviceCmd::SlotInfo {
                class: ObjectClass::Program,
                at: Location { bank: 30, slot: 3 },
            },
        );
        assert!(flow == Flow::Continue);
    }
}
