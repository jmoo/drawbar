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
use nord_usb::session::ReadWrite;
use nord_usb::transport::Transport;
use nord_usb::wire::{AllocationUnit, Bank, Dependency, ProgramInfo};
use nord_usb::{op, Error, Location, ObjectClass, Session};

use super::{DeviceCmd, DeviceEvent, Outgoing};
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
    /// The byte pipe failed, so there is nothing on the other end of it any more.
    Lost,
}

/// A device status is a reply; only transport failure means detachment.
fn hung_up(e: &Error) -> bool {
    matches!(e, Error::Transport(_))
}

/// Turn an error into the sentence for it, noting on the way whether the instrument is
/// still there. `at` is the slot the operation was aimed at, where it had one.
fn spoil(gone: &mut bool, at: Option<Location>) -> impl FnOnce(Error) -> String + '_ {
    move |e| {
        *gone |= hung_up(&e);
        match at {
            Some(at) => explain(e, at),
            None => e.to_string(),
        }
    }
}

/// Run one command to completion.
///
/// Emits exactly one [`DeviceEvent::Started`] and one [`DeviceEvent::Finished`], so the
/// UI's in-flight marker cannot be left set by an operation that failed halfway.
pub async fn run<T: Transport>(device: &mut Device<T>, cmd: DeviceCmd, emit: &Emit) -> Flow {
    if matches!(cmd, DeviceCmd::Disconnect) {
        return Flow::Released;
    }
    let what = cmd.label();
    emit.send(DeviceEvent::Started(what.clone()));

    let mut gone = false;
    let result = execute(device, cmd, emit, &mut gone).await;

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
    match gone {
        true => Flow::Lost,
        false => Flow::Continue,
    }
}

/// The command bodies. `Ok(Some(note))` is a line for the log; `Ok(None)` means the
/// command's own event already said everything.
async fn execute<T: Transport>(
    device: &mut Device<T>,
    cmd: DeviceCmd,
    emit: &Emit,
    gone: &mut bool,
) -> Result<Option<String>, String> {
    match cmd {
        // Handled by `run`; the transport is closed by the caller, which owns it.
        DeviceCmd::Disconnect => Ok(None),

        DeviceCmd::ScanBank {
            class,
            bank,
            slots: capacity,
        } => {
            let slots = scan_bank(device, class, bank, capacity)
                .await
                .map_err(spoil(gone, None))?;
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
                .map_err(spoil(gone, None))?;
            Ok(Some(format!(
                "{}: {} banks, {} items, {}, one session",
                class.label(),
                walked.banks,
                walked.items,
                walked.how,
            )))
        }

        DeviceCmd::SlotInfo { class, at } => {
            let info = match slot_info(device, class, at).await {
                Ok(info) => Some(info),
                // Status 1 is a vacant slot, not a failure.
                Err(Error::DeviceStatus(1)) => None,
                Err(e) => return Err(spoil(gone, Some(at))(e)),
            };
            emit.send(DeviceEvent::SlotInfo { class, at, info });
            Ok(None)
        }

        DeviceCmd::Deps { class, at } => {
            let deps = dependencies(device, class, at)
                .await
                .map_err(spoil(gone, Some(at)))?;
            let note = format!("{}: {} dependencies", shown(at), deps.len());
            emit.send(DeviceEvent::Deps { class, at, deps });
            Ok(Some(note))
        }

        DeviceCmd::Get {
            class,
            at,
            body,
            open,
        } => {
            let (info, bytes) = read_object(device, class, at, body)
                .await
                .map_err(spoil(gone, Some(at)))?;
            let note = format!(
                "read {:?} from {} ({} bytes)",
                info.name,
                shown(at),
                bytes.len()
            );
            emit.send(DeviceEvent::Got {
                name: entity_name(&info, body),
                origin: Origin::Device { class, at },
                bytes,
                open,
            });
            Ok(Some(note))
        }

        DeviceCmd::Put {
            id,
            class,
            at,
            name,
            bytes,
        } => {
            let note = put_one(device, class, at, &name, bytes, emit, gone)
                .await
                .map_err(spoil(gone, Some(at)))??;
            // Nothing is owed to the instrument until this session has closed.
            emit.send(DeviceEvent::Sent { id, class, at });
            Ok(Some(note))
        }

        DeviceCmd::SendAll { class, items } => send_all(device, class, items, emit, gone).await,

        DeviceCmd::Select { class, at } => {
            select(device, class, at)
                .await
                .map_err(spoil(gone, Some(at)))?;
            Ok(Some(format!("selected {} on the instrument", shown(at))))
        }

        DeviceCmd::Rename { class, at, name } => {
            rename(device, class, at, &name)
                .await
                .map_err(spoil(gone, Some(at)))?;
            Ok(Some(format!("renamed {} to {name:?}", shown(at))))
        }

        DeviceCmd::Move { class, from, to } => {
            move_object(device, class, from, to)
                .await
                .map_err(spoil(gone, Some(from)))?;
            Ok(Some(format!("moved {} -> {}", shown(from), shown(to))))
        }

        DeviceCmd::Duplicate { class, from, to } => {
            duplicate(device, class, from, to)
                .await
                .map_err(spoil(gone, Some(from)))?;
            Ok(Some(format!("duplicated {} -> {}", shown(from), shown(to))))
        }

        DeviceCmd::Delete { class, at } => {
            delete(device, class, at)
                .await
                .map_err(spoil(gone, Some(at)))?;
            Ok(Some(format!("deleted {}", shown(at))))
        }
    }
}

/// The unit this class allocates, from the instrument's own partition table.
async fn write_unit<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
) -> Result<AllocationUnit, Error> {
    device.geometry().await?.allocation_unit(class)
}

/// Write using the partition's reported allocation unit.
async fn store<T: Transport>(
    s: &mut Session<'_, T, ReadWrite>,
    unit: AllocationUnit,
    at: Location,
    file: &[u8],
    name: &str,
    timestamp: u32,
) -> Result<(), Error> {
    match unit.is_bytes() {
        true => op::write(s, at, file, name, timestamp).await,
        false => op::write_library(s, unit, at, file, name, timestamp).await,
    }
}

/// Replace a slot inside the caller's session.
/// ⚠️ An occupant is held in memory and restored or emitted as [`DeviceEvent::Rescued`].
async fn put<T: Transport>(
    s: &mut Session<'_, T, ReadWrite>,
    unit: AllocationUnit,
    at: Location,
    what: &str,
    bytes: Vec<u8>,
    emit: &Emit,
    gone: &mut bool,
) -> Result<Result<String, String>, Error> {
    match op::check_address(s, at).await {
        Ok(Some(why)) => return Ok(Err(format!("{}: {why}", shown(at)))),
        Ok(None) | Err(Error::DeviceStatus(_)) => {}
        Err(error) => return Err(error),
    }
    let class = s.class();
    let timestamp = unix_now()?;

    let existing = match op::info(s, at).await {
        Ok(info) => Some(info),
        Err(Error::DeviceStatus(1)) => None,
        Err(e) => return Ok(Err(spoil(gone, Some(at))(e))),
    };
    // Confirmed on hardware.
    // Library slots take their name from `BEGIN_WRITE`; buffer classes discard it.
    let write_name = slot_label(what)
        .or_else(|| existing.as_ref().map(|info| info.name.clone()))
        .unwrap_or_default();

    // Nothing is deleted until the backup is in hand.
    let backup = match &existing {
        Some(_) => match op::read_program(s, at).await {
            Ok(file) => Some(file),
            Err(e) => {
                return Ok(Err(format!(
                    "could not read {} back before replacing it, so it was left alone: {}",
                    shown(at),
                    spoil(gone, Some(at))(e)
                )))
            }
        },
        None => None,
    };

    // ⚠️ Deleting a class that overwrites in place has never been attempted on the
    // instrument, and its acceptance of a write at an occupied slot makes it needless.
    if backup.is_some() && !class.overwrites_in_place() {
        emit.send(DeviceEvent::Note(format!(
            "deleting {} to make room",
            shown(at)
        )));
        if let Err(e) = op::delete(s, at).await {
            return Ok(Err(format!(
                "deleting {}: {}",
                shown(at),
                spoil(gone, Some(at))(e)
            )));
        }
    }

    let written = store(s, unit, at, &bytes, &write_name, timestamp).await;

    Ok(match (written, backup) {
        (Ok(()), _) => Ok(wrote(class, at, what, &write_name)),
        (Err(e), None) => Err(spoil(gone, Some(at))(e)),
        // Restore the occupant before reporting the original error.
        (Err(e), Some(backup)) => {
            emit.send(DeviceEvent::OpFailed(format!(
                "the write failed and {}; putting the original back",
                aftermath(class, at)
            )));
            let restore_name = existing
                .as_ref()
                .map(|info| info.name.as_str())
                .unwrap_or_default();
            match store(s, unit, at, &backup, restore_name, timestamp).await {
                Ok(()) => Err(format!(
                    "{e} ({} was restored, and is unchanged)",
                    shown(at)
                )),
                Err(restore) => {
                    *gone |= hung_up(&restore);
                    let name = rescue_name(at, &backup);
                    emit.send(DeviceEvent::Rescued {
                        at,
                        name,
                        bytes: backup,
                    });
                    Err(format!(
                        "{e} (restoring failed as well: {restore}); {}, and its former \
                         contents are now in the local list as a rescued entity — \
                         put it back",
                        aftermath(class, at)
                    ))
                }
            }
        }
    })
}

/// Describe the slot state after a failed write.
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
/// Returns `None` rather than sending a blank name.
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

/// One put in a session of its own.
async fn put_one<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
    what: &str,
    bytes: Vec<u8>,
    emit: &Emit,
    gone: &mut bool,
) -> Result<Result<String, String>, Error> {
    let unit = write_unit(device, class).await?;
    device
        .destructive(class, async |s| {
            put(s, unit, at, what, bytes, emit, gone).await
        })
        .await
}

/// Send a batch until its first refusal; completed writes remain committed.
async fn send_all<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    items: Vec<Outgoing>,
    emit: &Emit,
    gone: &mut bool,
) -> Result<Option<String>, String> {
    let total = items.len();
    let mut done = 0;
    let outcome = batch(device, class, &items, total, &mut done, emit, gone).await;
    let refusal = outcome.map_err(spoil(gone, None))?;
    match refusal {
        None => Ok(Some(format!(
            "wrote {done} of {total} to {}",
            class.label()
        ))),
        Some(why) => Err(format!(
            "{why} — {done} of {total} were written; the rest are still waiting"
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
    emit: &Emit,
    gone: &mut bool,
) -> Result<Option<String>, Error> {
    let unit = write_unit(device, class).await?;
    device
        .destructive(class, async |s| {
            for item in items {
                emit.send(DeviceEvent::Note(format!(
                    "sending {:?} to {} ({} of {total})",
                    item.name,
                    shown(item.at),
                    *done + 1
                )));
                match put(s, unit, item.at, &item.name, item.bytes.clone(), emit, gone).await? {
                    Ok(note) => {
                        *done += 1;
                        emit.send(DeviceEvent::OpOk(note));
                        emit.send(DeviceEvent::Sent {
                            id: item.id,
                            class,
                            at: item.at,
                        });
                    }
                    Err(why) => return Ok(Some(why)),
                }
            }
            Ok(None)
        })
        .await
}

/// Status `1`, `3`, and `4` meanings are confirmed on hardware.
fn explain(e: Error, at: Location) -> String {
    match e {
        Error::DeviceStatus(1) => format!("{} is empty", shown(at)),
        Error::DeviceStatus(3) => format!("{} is out of range for this instrument", shown(at)),
        Error::DeviceStatus(4) => format!(
            "{} is occupied, and the instrument does not overwrite in place",
            shown(at)
        ),
        other => other.to_string(),
    }
}

async fn slot_info<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
) -> Result<ProgramInfo, Error> {
    device.read(class, async |s| op::info(s, at).await).await
}

/// A shorter per-frame limit for metadata walks; transfers keep the session default.
const SCAN_READ_LIMIT: Duration = Duration::from_secs(10);

/// Host safety limit for one complete class scan, not an instrument capacity.
const MOST_OCCUPIED: u32 = op::ENUMERATION_LIMIT as u32;

/// Scan the declared capacity, or scan to the device boundary when it reported none.
async fn scan_bank<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    bank: u32,
    slots: Option<u32>,
) -> Result<Vec<Option<ProgramInfo>>, Error> {
    if slots.is_some_and(|capacity| capacity > MOST_OCCUPIED) {
        return Err(Error::ScanLimit {
            bank: bank.saturating_sub(1),
            limit: MOST_OCCUPIED,
        });
    }
    device
        .read(class, async |s| {
            s.set_read_limit(SCAN_READ_LIMIT);
            match slots {
                Some(capacity) => walk_bank(s, bank, capacity).await,
                None => walk_open_bank(s, bank, MOST_OCCUPIED).await,
            }
        })
        .await
}

/// One bank a walk will read.
struct Planned {
    /// The bank number the panel labels it with.
    bank: NonZeroU32,
    /// Slots the device says it holds. `None` where it reported the unbounded sentinel.
    slots: Option<u32>,
}

/// What one class's walk did, for the line the activity log gets.
struct Walked {
    banks: u32,
    items: usize,
    /// Which of the two walks found the slots.
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
                Err(Error::DeviceStatus(1)) => emit.send(DeviceEvent::Focus { class, at: None }),
                Err(Error::DeviceStatus(_)) => {}
                Err(e) => return Err(e),
            }

            // Cursor enumeration wins for sparse or unbounded storage.
            let capacity = plan
                .iter()
                .try_fold(0u32, |sum, planned| sum.checked_add(planned.slots?));
            let sparse = capacity.is_none_or(|capacity| worth_the_cursor(held, capacity));
            let found = match sparse {
                true => occupied(s, &declared).await?,
                false => None,
            };

            let mut remaining = MOST_OCCUPIED;
            let mut scanned = Vec::with_capacity(plan.len());
            let how = match found {
                Some(found) => {
                    for planned in &plan {
                        let slots = shape(&found, planned, remaining)?;
                        remaining -= slots.len() as u32;
                        scanned.push((planned.bank.get(), slots));
                    }
                    "by cursor"
                }
                None => {
                    for planned in &plan {
                        let slots = match planned.slots {
                            Some(capacity) if capacity <= remaining => {
                                walk_bank(s, planned.bank.get(), capacity).await?
                            }
                            Some(_) => {
                                return Err(Error::ScanLimit {
                                    bank: planned.bank.get() - 1,
                                    limit: MOST_OCCUPIED,
                                })
                            }
                            None => walk_open_bank(s, planned.bank.get(), remaining).await?,
                        };
                        remaining -= slots.len() as u32;
                        scanned.push((planned.bank.get(), slots));
                    }
                    "slot by slot"
                }
            };

            let items = scanned
                .iter()
                .map(|(_, slots)| slots.iter().filter(|slot| slot.is_some()).count())
                .sum();
            let banks = scanned.len() as u32;
            for (bank, slots) in scanned {
                emit.send(DeviceEvent::BankScanned { class, bank, slots });
            }
            Ok(Walked { banks, items, how })
        })
        .await
}

/// The device's own banks, as a walk plan.
fn planned(declared: &[Bank]) -> Result<Vec<Planned>, Error> {
    let mut total = 0u32;
    let mut plan = Vec::with_capacity(declared.len());
    for bank in declared {
        let slots = bank.is_bounded().then_some(bank.slots);
        if let Some(slots) = slots {
            total = total.checked_add(slots).ok_or(Error::ScanLimit {
                bank: bank.index,
                limit: MOST_OCCUPIED,
            })?;
            if total > MOST_OCCUPIED {
                return Err(Error::ScanLimit {
                    bank: bank.index,
                    limit: MOST_OCCUPIED,
                });
            }
        }
        plan.push(Planned {
            bank: bank
                .index
                .checked_add(1)
                .and_then(NonZeroU32::new)
                .expect("a decoded bank index fits its panel number"),
            slots,
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
        return Err(Error::ScanLimit {
            bank: at.bank,
            limit: MOST_OCCUPIED,
        });
    }
    let mut out = Vec::with_capacity(found.len());
    for at in found {
        match op::info(s, at).await {
            Ok(info) => out.push((at, info)),
            // A cursor hit may be emptied before INFO; keep the rest of the scan.
            Err(Error::DeviceStatus(1)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Some(out))
}

/// Shape cursor hits to the declared capacity, or through an open bank's last item.
fn shape(
    found: &[(Location, ProgramInfo)],
    planned: &Planned,
    limit: u32,
) -> Result<Vec<Option<ProgramInfo>>, Error> {
    let bank = planned.bank.get() - 1;
    let mine: Vec<&(Location, ProgramInfo)> =
        found.iter().filter(|(at, _)| at.bank == bank).collect();
    let past = mine.iter().map(|(at, _)| at.slot + 1).max().unwrap_or(0);
    let len = planned.slots.unwrap_or(past).max(past);
    if len > limit {
        return Err(Error::ScanLimit {
            bank,
            limit: MOST_OCCUPIED,
        });
    }
    let mut slots = vec![None; len as usize];
    for (at, info) in mine {
        if let Some(cell) = slots.get_mut(at.slot as usize) {
            *cell = Some(info.clone());
        }
    }
    Ok(slots)
}

/// One bank's worth of `INFO`, inside a session the caller owns.
async fn walk_bank<T: Transport, C>(
    s: &mut Session<'_, T, C>,
    bank: u32,
    slots: u32,
) -> Result<Vec<Option<ProgramInfo>>, Error> {
    if slots == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for slot in 1..=slots {
        match op::info(s, Location::from_user(bank, slot)).await {
            Ok(info) => out.push(Some(info)),
            Err(Error::DeviceStatus(1)) => out.push(None),
            Err(Error::DeviceStatus(3)) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// One bank's worth of `INFO` where the device stated no capacity for it.
async fn walk_open_bank<T: Transport, C>(
    s: &mut Session<'_, T, C>,
    bank: u32,
    limit: u32,
) -> Result<Vec<Option<ProgramInfo>>, Error> {
    if limit == 0 {
        return Err(Error::ScanLimit {
            bank: bank.saturating_sub(1),
            limit: MOST_OCCUPIED,
        });
    }
    let mut out = Vec::new();
    for slot in 1..=limit {
        match op::info(s, Location::from_user(bank, slot)).await {
            Ok(info) => out.push(Some(info)),
            Err(Error::DeviceStatus(1)) => out.push(None),
            Err(Error::DeviceStatus(3)) => {
                while matches!(out.last(), Some(None)) {
                    out.pop();
                }
                return Ok(out);
            }
            Err(e) => return Err(e),
        }
    }
    Err(Error::ScanLimit {
        bank: bank.saturating_sub(1),
        limit: MOST_OCCUPIED,
    })
}

/// Read metadata plus either the wire body or a complete CBIN file.
async fn read_object<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
    body: bool,
) -> Result<(ProgramInfo, Vec<u8>), Error> {
    device
        .read(class, async |s| {
            let info = op::info(s, at).await?;
            let file = match body {
                true => op::read_body(s, at).await?,
                false => op::read_program(s, at).await?,
            };
            Ok((info, file))
        })
        .await
}

async fn dependencies<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
) -> Result<Vec<Dependency>, Error> {
    device
        .read(class, async |s| op::dependencies(s, at).await)
        .await
}

async fn select<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
) -> Result<(), Error> {
    device.read(class, async |s| op::select(s, at).await).await
}

async fn rename<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
    name: &str,
) -> Result<(), Error> {
    device
        .destructive(class, async |s| op::rename(s, at, name).await)
        .await
}

async fn move_object<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    from: Location,
    to: Location,
) -> Result<(), Error> {
    device
        .destructive(class, async |s| op::move_object(s, from, to).await)
        .await
}

async fn duplicate<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    from: Location,
    to: Location,
) -> Result<(), Error> {
    device
        .destructive(class, async |s| op::duplicate(s, from, to).await)
        .await
}

async fn delete<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    at: Location,
) -> Result<(), Error> {
    device
        .destructive(class, async |s| op::delete(s, at).await)
        .await
}

/// Unix seconds, for the timestamp word `BEGIN_WRITE` carries.
///
/// ⚠️ `SystemTime::now()` traps on `wasm32-unknown-unknown`, so the browser's own clock
/// is what the web build reads.
#[cfg(not(target_arch = "wasm32"))]
fn unix_now() -> Result<u32, Error> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| {
            Error::InvalidArgument(format!("system clock is before the Unix epoch: {e}"))
        })?;
    u32::try_from(elapsed.as_secs())
        .map_err(|_| Error::InvalidArgument("system time does not fit the device protocol".into()))
}

#[cfg(target_arch = "wasm32")]
fn unix_now() -> Result<u32, Error> {
    let seconds = js_sys::Date::now() / 1000.0;
    if !(0.0..=f64::from(u32::MAX)).contains(&seconds) {
        return Err(Error::InvalidArgument(
            "system time does not fit the device protocol".into(),
        ));
    }
    Ok(seconds as u32)
}

/// What the workspace calls an object read off the instrument.
///
/// Files store no name — it lives on the instrument — so a read is the one moment the
/// name and the bytes are together, and it goes into the entity's label here.
fn entity_name(info: &ProgramInfo, body: bool) -> String {
    let name = info.name.trim();
    let name = match name.is_empty() {
        true => "unnamed",
        false => name,
    };
    // A `--body` dump is a fragment of a file, not one; the suffix keeps it from being
    // handed back in as a whole object.
    match body {
        true => format!("{name}.body"),
        false => name.to_string(),
    }
}

/// Filename for a rescued slot: the location as the instrument labels it, and the
/// object's own format tag so it can be handed straight back to a put.
///
/// ⚠️ The tag is read out of the header rather than through `envelope::unwrap`, which
/// also verifies the checksum. These bytes are the last copy of the slot even if they
/// fail that check, so naming them must not depend on it.
fn rescue_name(at: Location, backup: &[u8]) -> String {
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

    /// The rescue entity is the last copy of a program that no longer exists on the
    /// instrument, so it has to be named something a person can act on.
    #[test]
    fn a_rescued_slot_is_named_for_its_location_and_format() {
        let mut file = vec![0u8; 45];
        file[0..4].copy_from_slice(b"CBIN");
        file[4..8].copy_from_slice(&1u32.to_le_bytes());
        file[8..12].copy_from_slice(b"ne5p");
        // Wire is zero-indexed, the instrument's labels are not.
        let at = Location { bank: 6, slot: 49 };
        assert_eq!(rescue_name(at, &file), "nord-rescued-7-50.ne5p");
    }

    /// Bytes that do not parse are still the only copy, so they still get a name.
    #[test]
    fn unparseable_bytes_still_get_rescued() {
        let at = Location { bank: 0, slot: 0 };
        assert_eq!(rescue_name(at, b"nonsense"), "nord-rescued-1-1.bin");
    }

    /// The device's name is the name, verbatim — spaces and all. Making it path-safe is
    /// the export dialog's business; a name sanitised here would go back to the
    /// instrument sanitised.
    #[test]
    fn a_read_keeps_the_slots_name_verbatim() {
        let info = ProgramInfo {
            location: Location { bank: 6, slot: 3 },
            body_len: 121,
            format: "ne5p".into(),
            version: 4,
            crc32: Some(0),
            name: "Africa Split".into(),
        };
        assert_eq!(entity_name(&info, false), "Africa Split");
        assert_eq!(entity_name(&info, true), "Africa Split.body");
    }

    /// The format tag the local list carries is a filename's business, not the panel's;
    /// everything else the operator typed goes over as it stands.
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
            "a tag and nothing"
        );
    }

    /// Nothing to send leaves the slot as the write left it. Blanking a name is not an
    /// improvement on the wrong one, and it is not what anybody asked for.
    #[test]
    fn a_name_with_nothing_in_it_is_not_sent() {
        for nothing in ["", "   ", "\t"] {
            assert_eq!(slot_label(nothing), None, "{nothing:?}");
        }
    }

    /// A name is UTF-8, and half a character is not a shorter name.
    #[test]
    fn a_long_name_is_cut_on_a_character_boundary() {
        let long = "é".repeat(200);
        let cut = slot_label(&long).expect("something is left");
        assert!(cut.len() <= 64, "{} bytes", cut.len());
        assert!(long.starts_with(&cut));
        assert_eq!(cut.chars().count(), 32, "whole characters only");
    }

    /// A slot with a blank name still has to produce a usable label.
    #[test]
    fn a_nameless_slot_still_gets_a_label() {
        let info = ProgramInfo {
            location: Location { bank: 0, slot: 0 },
            body_len: 121,
            format: "ne5p".into(),
            version: 4,
            crc32: None,
            name: "  ".into(),
        };
        assert_eq!(entity_name(&info, false), "unnamed");
    }

    /// A verbatim name goes over the wire verbatim: the round trip the operator sees is
    /// get "Big strings" → edit → send, and the slot must still read "Big strings".
    #[test]
    fn a_spaced_name_survives_to_the_write() {
        assert_eq!(slot_label("Big strings").as_deref(), Some("Big strings"));
    }
}

/// The write path driven against a stand-in device, which is the only way to see what a
/// put actually puts on the wire without an instrument on the end of it.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod wire_tests {
    use std::collections::VecDeque;
    use std::mem::size_of;
    use std::sync::mpsc::Receiver;

    use super::*;
    use nord_usb::wire::{cmd, ui, Message, Service};
    use nord_usb::Transport;

    /// A device that agrees to everything, and remembers what it was told.
    ///
    /// Enough of one to drive a whole operation: the framing rule is that a reply carries
    /// the request's command `+1` and leads with a status word, and nothing on the write
    /// path reads a reply's payload. The read paths do, so the commands a scan sends —
    /// `PARTITIONS`, `STATUS`, `BANKS`, `FOCUS`, `NEXT_SLOT`, `INFO` — are answered from
    /// the geometry and contents below rather than with the blanket reply.
    ///
    /// ⚠️ The progress strings are fire-and-forget — the code that sends them never reads
    /// a reply. Queueing one for those is how the stream desyncs, so they get none.
    struct Puppet {
        heard: Vec<Message>,
        replies: VecDeque<Vec<u8>>,
        /// The status `INFO` answers with where [`Puppet::filled`] is not keeping the
        /// contents. `1` is a vacant slot.
        info: u32,
        /// Every read fails, the way an unplugged device's does.
        deaf: bool,
        /// The banks this device actually has, as name and capacity. `INFO` answers
        /// out-of-range past them whether or not it will name them.
        banks: Vec<(&'static str, u32)>,
        /// Whether `BANKS` reports [`Puppet::banks`] or refuses to divide the class up.
        reports_geometry: bool,
        /// `BANKS` answers success with a reply too short to decode, which is neither a
        /// refusal nor a dead pipe.
        garbles_geometry: bool,
        /// Occupied addresses and their names. `None` leaves `INFO` answering
        /// [`Puppet::info`] for every slot and nothing to enumerate.
        filled: Option<Vec<(Location, &'static str)>>,
        /// Whether `NEXT_SLOT` works, or answers [`op::ENUMERATION_DISABLED`].
        enumerates: bool,
        /// What `FOCUS` reports. `None` answers "nothing loaded".
        focus: Option<Location>,
        refuses_first_write: bool,
    }

    /// The Electro 5's own division, which is what an unremarkable Puppet stands for.
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
            }
        }

        fn deaf() -> Puppet {
            Puppet {
                deaf: true,
                ..Puppet::new(1)
            }
        }

        /// A device with contents: the banks it divides them into, and what is in them.
        fn stocked(banks: &[(&'static str, u32)], filled: &[(Location, &'static str)]) -> Puppet {
            Puppet {
                banks: banks.to_vec(),
                filled: Some(filled.to_vec()),
                ..Puppet::new(1)
            }
        }

        /// An instrument that will not answer for its own geometry. It still has the
        /// banks — it just will not name them.
        fn mute_about_geometry(mut self) -> Puppet {
            self.reports_geometry = false;
            self
        }

        /// An instrument whose `BANKS` reply cannot be decoded — a success that yields an
        /// error, which is neither a refusal nor a dead pipe.
        fn garbling_geometry(mut self) -> Puppet {
            self.garbles_geometry = true;
            self
        }

        /// An instrument that will not enumerate its contents.
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

        /// The contents of one address, where this device is keeping any.
        fn holds(&self, at: Location) -> Option<&'static str> {
            self.filled
                .as_ref()?
                .iter()
                .find(|(held, _)| *held == at)
                .map(|(_, name)| *name)
        }

        /// A scan command's answer: its status and the payload behind it. `None` leaves
        /// the blanket reply to stand.
        ///
        /// ⚠️ One service only, and the guard is load-bearing: the two number their
        /// commands independently and they collide — `BANKS` and the UI's `GOODBYE` are
        /// both `0x02`, `PARTITIONS` and `HELLO` both `0x00`.
        fn answer(&self, msg: &Message) -> Option<(u32, Vec<u8>)> {
            if !matches!(msg.service, Service::Program) {
                return None;
            }
            let at = || Location {
                bank: u32::from_be_bytes(msg.args[0..4].try_into().unwrap()),
                slot: u32::from_be_bytes(msg.args[4..8].try_into().unwrap()),
            };
            match msg.command {
                cmd::BEGIN_WRITE
                    if self.refuses_first_write
                        && !self.heard.iter().any(|m| m.command == cmd::BEGIN_WRITE) =>
                {
                    Some((4, Vec::new()))
                }
                cmd::PARTITIONS => Some((0, partition_table())),
                // Five words, as the Electro 5 answers: `count, free, used, dirty,
                // spare`. A slot class parks nothing, so the last two are zero.
                cmd::STATUS => {
                    let count = self.filled.as_ref().map_or(0, Vec::len) as u32;
                    let total: u32 = self.banks.iter().map(|(_, slots)| slots).sum();
                    // One unit per item, so `Status::slots()` answers the bank capacity
                    // total rather than a coincidence of the division.
                    Some((0, words(&[count, total.saturating_sub(count), count, 0, 0])))
                }
                // A refusal, and the code is immaterial: what the walk keys on is that
                // the instrument answered rather than that the pipe failed.
                cmd::BANKS if !self.reports_geometry => Some((2, Vec::new())),
                // Success, and a body `Bank::decode_all` cannot read.
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
                    // Third word is the direction; the hardware refuses its absence
                    // (`0x11`) after a write, so the puppet insists on it too.
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
                // Enough of a body to be read back: the occupant of a slot has to be
                // recoverable before a replace will touch it, so a puppet that cannot
                // serve a read cannot exercise a replace at all.
                cmd::READ => {
                    let (offset, want) = (
                        u32::from_be_bytes(msg.args[8..12].try_into().unwrap()),
                        u32::from_be_bytes(msg.args[12..16].try_into().unwrap()),
                    );
                    let at = at();
                    let mut p = words(&[at.bank, at.slot, offset, want]);
                    p.resize(p.len() + want as usize, 0);
                    Some((0, p))
                }
                cmd::INFO => {
                    let at = at();
                    // Status 3 marks the address-space boundary for geometry-free walks.
                    let capacity = self.banks.get(at.bank as usize).map(|(_, slots)| *slots);
                    if capacity.is_none_or(|slots| at.slot >= slots) {
                        return Some((3, Vec::new()));
                    }
                    match &self.filled {
                        Some(_) => match self.holds(at) {
                            Some(name) => Some((0, info_payload(at, name))),
                            None => Some((1, Vec::new())),
                        },
                        None => match self.info {
                            0 => Some((0, info_payload(at, "something"))),
                            status => Some((status, Vec::new())),
                        },
                    }
                }
                _ => None,
            }
        }

        /// The slot commands it was sent, in order.
        ///
        /// ⚠️ One service only. The two number their commands independently, and they
        /// collide: `SESSION_CLOSE` and the UI's progress label are both `0x06`.
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

    /// Big-endian words, the way every argument list on this wire is laid out.
    fn words(of: &[u32]) -> Vec<u8> {
        of.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    /// A `PARTITIONS` reply covering every class code: `[u8 count]` then a
    /// `[u32 name_len][name][u32 allocation unit][25 further field bytes]` record per
    /// partition, whose position in the table is the code [`ObjectClass::to_raw`] uses.
    ///
    /// The unit is a net storage block in a library and `1` where the counters are
    /// byte-granular; nothing here reads the fields after it.
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

    /// An `INFO` reply body: the fixed words, then the length-prefixed name, then the
    /// `0xffffffff` that stands for "this device reported no checksum".
    fn info_payload(at: Location, name: &str) -> Vec<u8> {
        let mut p = words(&[at.bank, at.slot, 121]);
        p.extend_from_slice(b"ne5p");
        p.extend_from_slice(&words(&[4, u32::MAX, u32::MAX, name.len() as u32]));
        p.extend_from_slice(name.as_bytes());
        p.extend_from_slice(&u32::MAX.to_be_bytes());
        p
    }

    impl Transport for Puppet {
        async fn write(&mut self, buf: &[u8]) -> nord_usb::Result<()> {
            let msg = Message::decode(buf)?;
            let spoken = matches!(msg.service, Service::Ui)
                && matches!(msg.command, ui::LABEL | ui::PERCENT);
            // A scan reads its replies, so those are answered from the device's state;
            // everything else takes the blanket agreement.
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
        workspace.get(id).expect("just made").bytes.clone()
    }

    /// Run one command against `puppet`, which a [`Device`] owns for the length of it and
    /// hands back for the assertions.
    fn drive(puppet: &mut Puppet, cmd: DeviceCmd) -> (Flow, Receiver<DeviceEvent>) {
        let (tx, events) = std::sync::mpsc::channel();
        let emit = Emit::new(tx, egui::Context::default());
        let lent = std::mem::replace(puppet, Puppet::new(1));
        let mut device = Device::new(lent);
        let flow = nord_usb::block_on(run(&mut device, cmd, &emit));
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
                    bytes: a_program(),
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
    fn a_restore_puts_the_occupants_own_name_back() {
        let at = Location { bank: 0, slot: 3 };
        let mut device =
            Puppet::stocked(&[("Bank 1", 50)], &[(at, "Squabble B")]).refusing_the_first_write();
        drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at,
                name: "Africa-Split.ne5p".into(),
                bytes: a_program(),
            },
        );

        assert_eq!(written_names(&device), ["Africa-Split", "Squabble B"]);
    }

    #[test]
    fn a_put_into_a_buffer_class_never_deletes_the_slot() {
        let at = Location { bank: 0, slot: 2 };
        let put = |class| DeviceCmd::Put {
            id: 1,
            class,
            at,
            name: "Africa-Split.ne5p".into(),
            bytes: a_program(),
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
                bytes: a_program(),
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
                bytes: a_program(),
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
                bytes: a_program(),
            },
        );

        assert_eq!(written_name(&device), "Squabble B");
    }

    /// A batch names every slot it writes into, not just the first.
    #[test]
    fn every_item_of_a_batch_is_named() {
        let bytes = a_program();
        let item = |slot, name: &str| Outgoing {
            id: slot as u64,
            at: Location { bank: 6, slot },
            name: name.into(),
            bytes: bytes.clone(),
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
        // One geometry read and one destructive session around the pair, which is what
        // a batch is for.
        let opens = device
            .commands()
            .into_iter()
            .filter(|command| *command == cmd::SESSION_OPEN)
            .count();
        assert_eq!(opens, 2);
    }

    /// ⚠️ A device that stopped answering is not a device that said no. Only the first
    /// puts the app back into its unattached state.
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

    /// Two categories of unequal size — 80 addresses — holding two pianos between them.
    fn a_small_library() -> Puppet {
        Puppet::stocked(
            &[("Grand", 50), ("Upright", 30)],
            &[
                (Location { bank: 0, slot: 0 }, "Royal Grand 3D"),
                (Location { bank: 1, slot: 2 }, "Queen Upright"),
            ],
        )
    }

    /// What one bank came back as: its number, how many rows it has, and what is in them.
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

    /// Every bank a scan reported, in the order it reported them.
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

    /// Everything the operator was told went wrong, joined for one assertion.
    fn refused(events: Receiver<DeviceEvent>) -> String {
        events
            .try_iter()
            .filter_map(|event| match event {
                DeviceEvent::OpFailed(why) => Some(why),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn counted(device: &Puppet, command: u32) -> usize {
        device
            .commands()
            .into_iter()
            .filter(|held| *held == command)
            .count()
    }

    /// ⚠️ The whole point of the cursor walk: an `INFO` per *occupied* slot rather than
    /// per address. Eighty addresses holding two pianos is a handful of reads instead of
    /// eighty, and the banks that come out are the same shape either way.
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
        // One read per thing found; the declared banks say where to walk, so nothing is
        // spent discovering them.
        assert_eq!(counted(&device, cmd::INFO), 2, "not the 80 addresses");
    }

    /// ⚠️ An instrument can refuse to enumerate at all — see [`op::ENUMERATION_DISABLED`]
    /// for the conditions — so the walk has to fall back to asking about every address,
    /// and come back with the same banks.
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
        // Every address of both banks. The cursor walk spends no `INFO` before the
        // refusal: its first frame is the cursor request itself.
        assert_eq!(counted(&device, cmd::INFO), 80);
    }

    /// The device's own banks decide the shape, names and all. Told a category of 50 and
    /// one of 30, the walk must paint neither two banks of 50 nor one of 80.
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
        let mut device = Puppet::new(1);
        let (flow, events) = drive(
            &mut device,
            DeviceCmd::ScanBank {
                class: ObjectClass::Program,
                bank: 1,
                slots: Some(MOST_OCCUPIED + 1),
            },
        );

        assert!(flow == Flow::Continue);
        assert!(refused(events).contains("cannot be scanned completely"));
        assert_eq!(counted(&device, cmd::INFO), 0);
    }

    /// The instrument's own bank list is the whole plan, so a class it refuses to divide
    /// up is not walked at all: its refusal is the scan's error, and the operator is told
    /// rather than shown a folder nothing on the instrument agrees with.
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

    /// ⚠️ A bank list that cannot be decoded is a fault, not a refusal: it stops the whole
    /// geometry read, because a partly decoded division is one no walk should be planned
    /// from.
    #[test]
    fn a_bank_list_that_will_not_decode_stops_the_scan() {
        let mut device = Puppet::stocked(&[("Bank 1", 50)], &[]).garbling_geometry();
        let (flow, events) = drive(&mut device, scan(ObjectClass::Program));
        assert!(flow == Flow::Continue, "a bad reply is not a dead pipe");

        let why = refused(events);
        assert!(why.contains("truncated"), "{why}");
        assert_eq!(counted(&device, cmd::INFO), 0, "and nothing was walked");
    }

    /// ⚠️ A bank the device states no capacity for ends where its contents end. Bounding
    /// it by anything else reads a library of 60 back as some other number, with no error
    /// anywhere and the folder's own header saying 60.
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

    /// ⚠️ A factory instrument's program banks are full, so the commonest scan there is
    /// must not take the walk that costs two exchanges per occupied slot.
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
        assert!(counted(&sparse, cmd::NEXT_SLOT) > 0, "the cursor earned it");
        assert!(counted(&sparse, cmd::INFO) < 100);
    }

    #[test]
    fn a_malformed_preflight_stops_the_write() {
        let mut device = Puppet::stocked(&[("Bank 1", 50)], &[]).garbling_geometry();
        let (flow, _) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at: Location { bank: 0, slot: 3 },
                name: "Africa-Split.ne5p".into(),
                bytes: a_program(),
            },
        );
        assert!(flow == Flow::Continue, "not a disconnection");
        assert_eq!(counted(&device, cmd::WRITE_DATA), 0);
    }

    /// The slot the panel is on is read while the class's session is open, so the browser
    /// can mark it without a transaction of its own.
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

    /// ⚠️ An address the instrument does not have must be refused *before* the occupant
    /// of anything is deleted for it. The reason is the device's own — bank names and a
    /// count — rather than a status code arriving mid-transfer.
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
                bytes: a_program(),
            },
        );
        assert!(flow == Flow::Continue, "it said no, it did not go away");

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

    /// A destination the device does have is not refused, so the guard cannot become a
    /// wall in front of every write.
    #[test]
    fn a_write_to_a_real_address_still_goes() {
        let mut device = Puppet::stocked(&[("Bank 1", 50)], &[]);
        let (flow, _) = drive(
            &mut device,
            DeviceCmd::Put {
                id: 1,
                class: ObjectClass::Program,
                at: Location { bank: 0, slot: 3 },
                name: "Africa-Split.ne5p".into(),
                bytes: a_program(),
            },
        );
        assert!(flow == Flow::Continue);
        assert_eq!(counted(&device, cmd::WRITE_DATA), 1, "the bytes went");
    }

    /// A refusal keeps the instrument: it is attached, it understood, and it declined.
    #[test]
    fn a_refusal_is_not_a_disconnection() {
        // Status 3: the slot is outside this instrument's range.
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
