//! The instrument: what the app knows about the attached Nord, and the channel that
//! talks to it.
//!
//! egui is immediate-mode and single-threaded; USB operations are slow and async. So
//! the UI never touches a transport — it sends a [`DeviceCmd`] to a worker that owns
//! one, and reads [`DeviceEvent`]s back. **One operation is in flight at a time**,
//! which is also all the protocol allows: a transaction is not re-entrant.
//!
//! Two queues feed that one slot. What the user asked for goes in `pending` and is
//! always dispatched first; the background read of every bank of every class waits in
//! [`Scan`] behind it, so browsing the tree never makes a click wait on it.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Receiver;

use eframe::egui;
use nord_format::accept::{Acceptance, Family};
use nord_usb::wire::{AllocationUnit, Bank, Dependency, ProgramInfo, Status};
use nord_usb::{Location, ObjectClass};

use crate::log::Log;
use crate::queue::Queue;
use crate::strings::{folder, place, shown};
use crate::tabs::Tabs;
use crate::workspace::{LocalEntity, Origin, Workspace};

mod scan;
mod worker;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::Link;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web::Link;

pub use scan::{Progress, Scan};
pub use worker::{Emit, Flow};

/// One row of the instrument's own partition table: a class it has, under the device's
/// own name for it.
///
/// What the instrument declares, never what this app expects. A class this app has no
/// name for arrives as [`ObjectClass::Unknown`] and is shown under `name`.
pub struct Partition {
    pub class: ObjectClass,
    /// The device's own word: `Piano`, `Samp Lib`, `Program`, `Set List`, …
    pub name: String,
    /// Whether this is the `(Native)` view of a library — a second view of a pool the
    /// table already carries under its user partition, and not a folder of its own.
    pub native: bool,
    /// `None` where the partition reports no usable unit.
    pub unit: Option<AllocationUnit>,
}

/// What the UI asks the instrument to do.
#[derive(Clone)]
pub enum DeviceCmd {
    /// One class end to end — counters and every bank — inside a single session,
    /// streaming a [`DeviceEvent::BankScanned`] as each bank lands.
    ScanClass {
        class: ObjectClass,
    },
    /// One `INFO` per slot of a single bank. What a mutation owes: only the bank it
    /// touched can have changed.
    ScanBank {
        class: ObjectClass,
        bank: u32,
    },
    SlotInfo {
        class: ObjectClass,
        at: Location,
    },
    Deps {
        class: ObjectClass,
        at: Location,
    },
    Get {
        class: ObjectClass,
        at: Location,
        /// The wire body verbatim, rather than a whole CBIN file.
        body: bool,
        why: Purpose,
    },
    Put {
        /// The asset on this computer these bytes came from. It is what the
        /// [`DeviceEvent::Sent`] this raises names, so a lone put pays the same debt a
        /// batch does.
        id: u64,
        class: ObjectClass,
        at: Location,
        name: String,
        bytes: Vec<u8>,
    },
    /// Every queued object of one class, written inside a single session.
    ///
    /// Each item still runs the whole read-back / delete / write / restore flow a lone
    /// [`DeviceCmd::Put`] runs; what is shared is the session around them. A refusal
    /// stops the batch where it stands.
    SendAll {
        class: ObjectClass,
        items: Vec<Outgoing>,
    },
    Move {
        class: ObjectClass,
        from: Location,
        to: Location,
    },
    Duplicate {
        class: ObjectClass,
        from: Location,
        to: Location,
    },
    Delete {
        class: ObjectClass,
        at: Location,
    },
    Rename {
        class: ObjectClass,
        at: Location,
        name: String,
    },
    Select {
        class: ObjectClass,
        at: Location,
    },
    Disconnect,
}

/// What a read of a slot is for, which is what decides where its bytes go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// A **view** of the slot, opened in a tab rather than joining the local list,
    /// which is what a double-click asks for.
    View,
    /// A copy on this computer: a row in the list like any other.
    Copy,
    /// The occupant of a slot something is queued for, to be compared with what is
    /// waiting. ⚠️ These bytes never reach the workspace — a copy nobody asked for is a
    /// row nobody can account for.
    Compare,
}

/// One object waiting to go back to the instrument.
#[derive(Clone)]
pub struct Outgoing {
    pub id: u64,
    pub at: Location,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// How one operation is spoken about.
///
/// The activity log keeps the protocol line; the status strip gets sentences that name
/// places and things the way the panel does, and never a class number or a verb off the
/// wire.
#[derive(Clone)]
pub struct Words {
    pub doing: String,
    pub done: String,
    pub failed: String,
}

fn words(verbs: (&str, &str, &str), what: String) -> Words {
    Words {
        doing: format!("{} {what}…", verbs.0),
        done: format!("{} {what}", verbs.1),
        failed: format!("Could not {} {what}", verbs.2),
    }
}

const READING: (&str, &str, &str) = ("Reading", "Read", "read");
const COPYING: (&str, &str, &str) = ("Copying", "Copied", "copy");

impl DeviceCmd {
    /// What the log and the in-flight spinner call this operation.
    pub fn label(&self) -> String {
        match self {
            DeviceCmd::ScanClass { class } => format!("scan {}", class.label()),
            DeviceCmd::ScanBank { bank, .. } => format!("scan bank {bank}"),
            DeviceCmd::SlotInfo { at, .. } => format!("info {}", shown(*at)),
            DeviceCmd::Deps { at, .. } => format!("deps {}", shown(*at)),
            DeviceCmd::Get { at, body, why, .. } => match (body, why) {
                (_, Purpose::Compare) => format!("get {} (to compare)", shown(*at)),
                (true, _) => format!("get {} (raw body)", shown(*at)),
                (false, _) => format!("get {}", shown(*at)),
            },
            DeviceCmd::Put { at, name, .. } => format!("put {name} -> {}", shown(*at)),
            DeviceCmd::SendAll { class, items } => {
                format!("put {} objects -> {}", items.len(), class.label())
            }
            DeviceCmd::Move { from, to, .. } => {
                format!("move {} -> {}", shown(*from), shown(*to))
            }
            DeviceCmd::Duplicate { from, to, .. } => {
                format!("duplicate {} -> {}", shown(*from), shown(*to))
            }
            DeviceCmd::Delete { at, .. } => format!("delete {}", shown(*at)),
            DeviceCmd::Rename { at, name, .. } => format!("rename {} to {name:?}", shown(*at)),
            DeviceCmd::Select { at, .. } => format!("select {}", shown(*at)),
            DeviceCmd::Disconnect => "disconnect".into(),
        }
    }

    /// The plain-words sentences the status strip shows for this operation.
    pub fn words(&self) -> Words {
        match self {
            DeviceCmd::ScanClass { class } => words(READING, folder(*class).to_string()),
            DeviceCmd::ScanBank { class, bank, .. } => {
                words(READING, format!("{} — bank {bank}", folder(*class)))
            }
            DeviceCmd::SlotInfo { class, at } => words(READING, place(*class, *at)),
            DeviceCmd::Deps { class, at } => {
                words(READING, format!("what {} needs", place(*class, *at)))
            }
            DeviceCmd::Get {
                class,
                at,
                why: Purpose::Compare,
                ..
            } => words(READING, format!("what is in {}", place(*class, *at))),
            DeviceCmd::Get { class, at, .. } => {
                words(COPYING, format!("{} to this computer", place(*class, *at)))
            }
            DeviceCmd::Put {
                class, at, name, ..
            } => words(
                ("Sending", "Sent", "send"),
                format!("“{name}” to {}", place(*class, *at)),
            ),
            DeviceCmd::SendAll { class, items } => words(
                ("Sending", "Sent", "send"),
                match items.len() {
                    1 => format!("1 sound to {}", folder(*class)),
                    n => format!("{n} sounds to {}", folder(*class)),
                },
            ),
            DeviceCmd::Move { class, from, to } => words(
                ("Moving", "Moved", "move"),
                format!("{} to {}", place(*class, *from), place(*class, *to)),
            ),
            DeviceCmd::Duplicate { class, from, to } => words(
                COPYING,
                format!("{} to {}", place(*class, *from), place(*class, *to)),
            ),
            DeviceCmd::Delete { class, at } => {
                words(("Deleting", "Deleted", "delete"), place(*class, *at))
            }
            DeviceCmd::Rename { class, at, name } => words(
                ("Renaming", "Renamed", "rename"),
                format!("{} to “{name}”", place(*class, *at)),
            ),
            DeviceCmd::Select { class, at } => words(
                ("Loading", "Loaded", "load"),
                format!("{} on the instrument", place(*class, *at)),
            ),
            DeviceCmd::Disconnect => words(
                ("Releasing", "Released", "release"),
                "the instrument".into(),
            ),
        }
    }
}

/// What the worker reports back.
pub enum DeviceEvent {
    Connected(DeviceCard),
    ConnectFailed(String),
    Disconnected {
        /// The instrument went rather than being let go: the cable, or the transport
        /// under it, and not a refusal.
        lost: bool,
    },
    Started(String),
    Finished,
    /// The instrument's own partition table, in table order — the classes it has, which
    /// nothing above this can know before it arrives. Read once per connection.
    Partitions(Vec<Partition>),
    /// A class's own counters, read at the head of its walk.
    ClassStatus {
        class: ObjectClass,
        status: Status,
        /// Banks to expect, as the instrument's own bank list divides the class.
        banks: Option<u32>,
    },
    /// The device's own division of a class into banks, read at the head of its walk.
    Geometry {
        class: ObjectClass,
        banks: Vec<Bank>,
    },
    /// The slot the panel has loaded in a class; `None` when focus is supported but
    /// nothing is loaded. Never sent for a class that answers `0x15` (focus n/a).
    Focus {
        class: ObjectClass,
        at: Option<Location>,
    },
    BankScanned {
        class: ObjectClass,
        bank: u32,
        /// One entry per slot, `None` where the slot is vacant. Shorter than the bank
        /// asked for when the device said the class ends here.
        slots: Vec<Option<ProgramInfo>>,
    },
    SlotInfo {
        class: ObjectClass,
        at: Location,
        info: Option<ProgramInfo>,
    },
    Deps {
        class: ObjectClass,
        at: Location,
        deps: Vec<Dependency>,
    },
    Got {
        name: String,
        origin: Origin,
        bytes: Vec<u8>,
        why: Purpose,
    },
    /// A read found the slot empty, which is the instrument's answer rather than a
    /// fault. Only a read can settle a slot no walk has reached.
    Vacant {
        class: ObjectClass,
        at: Location,
        why: Purpose,
    },
    /// One object landed on the instrument, so it is no longer owed. Every write path
    /// raises one — a lone put as much as a batch.
    Sent {
        id: u64,
        class: ObjectClass,
        at: Location,
        /// The bytes the write carried, which is what the slot holds and what the asset
        /// is saved as from here on — see [`Workspace::landed`]. The asset may hold
        /// something else by now: a write takes as long as the instrument takes.
        bytes: Vec<u8>,
    },
    /// A slot's former contents, which a failed write and a failed restore left with
    /// nowhere else to go.
    Rescued {
        at: Location,
        name: String,
        bytes: Vec<u8>,
    },
    Note(String),
    OpOk(String),
    OpFailed(String),
    InstrumentChanged,
}

/// The attached instrument, from its USB descriptors — answerable before any
/// transaction is opened.
///
/// Everything from [`Identity`](nord_usb::transport::usb::Identity) down is read over
/// vendor control transfers on endpoint 0, which only the desktop transport issues, so
/// the browser build answers `None` for all of them rather than guessing.
#[derive(Clone)]
pub struct DeviceCard {
    pub product: String,
    pub manufacturer: Option<String>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: Option<String>,
    /// The vendor-specific interface this app claimed, by its descriptor number.
    pub interface: Option<u8>,
    /// Firmware version in hundredths: `204` is 2.04.
    pub firmware: Option<u16>,
    /// Reported at vendor request `0x05`. Plausibly a build number, unconfirmed.
    pub build: Option<u16>,
    /// Reported at vendor request `0x00`. Reads as a small constant; its meaning is not
    /// pinned down, so it is shown verbatim rather than under a name it might not have.
    pub kind: Option<u16>,
    /// Largest transfer the device will accept or produce, in bytes, framing included.
    pub max_transfer: Option<u32>,
}

#[derive(Default)]
pub enum Connection {
    #[default]
    Disconnected,
    Connecting,
    Connected(DeviceCard),
}

/// One slot's detail, as the last `info`/`deps` reported it.
#[derive(Default)]
pub struct Detail {
    pub at: Option<Location>,
    pub info: Option<ProgramInfo>,
    /// Whether the last `info` said the slot was empty, as opposed to never asked.
    pub asked: bool,
    pub deps: Option<Vec<Dependency>>,
}

/// The UI's cache of the instrument. Nothing here is authoritative — it is what the
/// device last said, which [`DeviceState::stale`] flags as possibly out of date.
#[derive(Default)]
pub struct DeviceState {
    pub connection: Connection,
    /// The operation currently running, if any. One at a time.
    pub in_flight: Option<Words>,
    pub inventory: Vec<Status>,
    /// The background read of every class.
    pub scan: Scan,
    /// The slot this app last asked the instrument to load, per class.
    ///
    /// ⚠️ Only what **this app** selected, and kept for the reselect a write owes. What
    /// the panel is actually on is [`DeviceState::focused`], which is a device answer
    /// rather than a record of our own commands.
    selected: HashMap<u32, Location>,
    /// The slot the panel had loaded when the class was last read, per class.
    focus: HashMap<u32, Option<Location>>,
    /// The device's own banks, per class: their names and their capacities.
    geometry: HashMap<u32, Vec<Bank>>,
    /// The instrument's own partition table, in table order. Empty until it is read,
    /// which is *not known* rather than *an instrument with no folders*.
    partitions: Vec<Partition>,
    banks: HashMap<(u32, u32), Vec<Option<ProgramInfo>>>,
    pub detail: Detail,
}

impl DeviceState {
    pub fn connected(&self) -> bool {
        matches!(self.connection, Connection::Connected(_))
    }

    pub fn product(&self) -> Option<&str> {
        self.card().map(|card| card.product.as_str())
    }

    /// What the descriptors and endpoint 0 said about the attached instrument.
    pub fn card(&self) -> Option<&DeviceCard> {
        match &self.connection {
            Connection::Connected(card) => Some(card),
            _ => None,
        }
    }

    /// The firmware version as the panel writes it — `2.04` — where the transport could
    /// ask for it.
    pub fn firmware(&self) -> Option<String> {
        // 204/100 = 2 and 204%100 = 04, and the panel reads 2.04.
        self.card()?
            .firmware
            .map(|held| format!("{}.{:02}", held / 100, held % 100))
    }

    /// What the instrument calls a bank, by the number the panel labels it with.
    ///
    /// For pianos these are the panel's categories — `Grand`, `Upright` — rather than
    /// numbers, which is the whole reason the browser shows them.
    pub fn bank_name(&self, class: ObjectClass, bank: u32) -> Option<&str> {
        let name = self
            .geometry
            .get(&class.to_raw())?
            .iter()
            .find(|held| held.index + 1 == bank)?
            .name
            .trim();
        (!name.is_empty()).then_some(name)
    }

    /// The slot the panel had loaded in a class when it was last read.
    ///
    /// ⚠️ Read once per walk, so a selection made on the panel afterwards is not in here
    /// until the class is read again.
    pub fn focused(&self, class: ObjectClass) -> Option<Location> {
        self.focus.get(&class.to_raw()).copied().flatten()
    }

    /// Whether the class answers focus reads at all; also `false` while the class has
    /// never been read.
    pub fn focus_applies(&self, class: ObjectClass) -> bool {
        self.focus.contains_key(&class.to_raw())
    }

    /// What the last dependency list called a library id.
    ///
    /// ⚠️ Only the wire carries these names — a program's file stores its piano and
    /// sample as bare ids. The cache holds one slot's list, so this answers for the slot
    /// that was last asked about and for no other; `None` means *not asked*, never
    /// *nameless*.
    pub fn dependency_name(
        &self,
        slot: Option<(ObjectClass, Location)>,
        class: ObjectClass,
        id: u32,
    ) -> Option<&str> {
        let (_, at) = slot?;
        if self.detail.at != Some(at) {
            return None;
        }
        self.detail
            .deps
            .as_ref()?
            .iter()
            .find(|dep| dep.class == class && dep.id == id)
            .map(|dep| dep.name.trim())
    }

    /// A scanned bank's slots, or `None` if it has not been scanned.
    pub fn bank(&self, class: ObjectClass, bank: u32) -> Option<&[Option<ProgramInfo>]> {
        self.banks.get(&(class.to_raw(), bank)).map(Vec::as_slice)
    }

    /// The classes the instrument declares, in its own table order.
    ///
    /// The whole of what the browser, the switcher and a resync walk. Empty until the
    /// partition table has been read.
    ///
    /// The `(Native)` rows are left out: each is a second view of a library the table
    /// already carries under its user partition, so listing one would show the same
    /// pool as a folder of its own.
    pub fn classes(&self) -> Vec<ObjectClass> {
        self.partitions
            .iter()
            .filter(|row| !row.native)
            .map(|row| row.class)
            .collect()
    }

    /// What the browser calls a class's folder: the panel's own word for a class this
    /// app names, and otherwise the name the instrument's partition table gave it.
    pub fn folder_name(&self, class: ObjectClass) -> &str {
        let ObjectClass::Unknown(_) = class else {
            return folder(class);
        };
        self.partition(class)
            .map(|row| row.name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| folder(class))
    }

    /// What one unit of whatever `STATUS` counts is worth for this class's partition.
    ///
    /// ⚠️ `None` is *not read yet*, never *byte-granular*: a slot-addressed partition
    /// reports a unit of 1, which is a unit like any other.
    pub fn allocation_unit(&self, class: ObjectClass) -> Option<AllocationUnit> {
        self.partition(class)?.unit
    }

    fn partition(&self, class: ObjectClass) -> Option<&Partition> {
        self.partitions
            .iter()
            .find(|row| !row.native && row.class == class)
    }

    /// How many banks a class declares.
    ///
    /// ⚠️ The device's own division of the class, read at the head of its walk — not
    /// what has been scanned, which [`DeviceState::banks_of`] answers. Zero until that
    /// geometry has arrived.
    pub fn banks(&self, class: ObjectClass) -> usize {
        self.geometry.get(&class.to_raw()).map_or(0, Vec::len)
    }

    /// The banks of a class that have been read, in order.
    pub fn banks_of(&self, class: ObjectClass) -> Vec<u32> {
        let mut banks: Vec<u32> = self
            .banks
            .keys()
            .filter(|(raw, _)| *raw == class.to_raw())
            .map(|(_, bank)| *bank)
            .collect();
        banks.sort_unstable();
        banks
    }

    /// What a slot holds, from the scan cache. `Some(None)` is a scanned empty slot.
    pub fn slot(&self, class: ObjectClass, at: Location) -> Option<Option<&ProgramInfo>> {
        let bank = self.bank(class, at.bank + 1)?;
        bank.get(at.slot as usize).map(Option::as_ref)
    }

    /// The format tags the scanned slots of a class report, in the order first seen.
    ///
    /// Every slot a walk reads names its own format, so this is what the folder is
    /// actually holding rather than what a model's folder is supposed to hold. An
    /// unscanned class answers with nothing, which is *not known*, never *empty*.
    pub fn formats_in(&self, class: ObjectClass) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for bank in self.banks_of(class) {
            for info in self.bank(class, bank).into_iter().flatten().flatten() {
                let format = info.format.trim();
                if !format.is_empty() && !seen.iter().any(|held| held == format) {
                    seen.push(format.to_string());
                }
            }
        }
        seen
    }

    /// Every slot of `class` a walk found vacant, in address order.
    pub fn free_slots(&self, class: ObjectClass) -> impl Iterator<Item = Location> + '_ {
        self.banks_of(class).into_iter().flat_map(move |bank| {
            self.bank(class, bank)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .filter(|(_, held)| held.is_none())
                .map(move |(slot, _)| Location::from_user(bank, slot as u32 + 1))
        })
    }

    /// The first slot of `class` known to be vacant and not among `taken` — where a
    /// duplicate lands when the user did not drag it anywhere, and where the next of a
    /// queued set goes.
    ///
    /// ⚠️ `taken` is what is already spoken for. Two writes handed one address are one
    /// write, so a set queued together walks down the free slots rather than piling onto
    /// the first of them.
    pub fn first_free(&self, class: ObjectClass, taken: &[Location]) -> Option<Location> {
        self.free_slots(class).find(|at| !taken.contains(at))
    }

    /// Drop one bank's cached names, because something just changed them.
    fn forget_bank(&mut self, class: ObjectClass, bank: u32) {
        self.banks.remove(&(class.to_raw(), bank));
    }

    fn forget_everything(&mut self) {
        self.banks.clear();
        self.focus.clear();
        self.geometry.clear();
        self.partitions.clear();
        self.inventory.clear();
        self.detail = Detail::default();
        self.scan.clear();
        self.selected.clear();
    }
}

/// How full a folder is, in the width its own heading has for it: `312/400`, or a bare
/// count for a class whose items differ in size and divide into no slots.
///
/// Slot counts only: the inventory also reports opaque block totals, and a class whose
/// items differ in size (pianos, samples) cannot be divided into slots at all.
pub fn occupancy(
    class: ObjectClass,
    inventory: &[Status],
    unit: Option<AllocationUnit>,
) -> Option<String> {
    let status = inventory.iter().find(|status| status.class == class)?;
    if let Some(slots) = status.slots() {
        return Some(format!("{}/{slots}", status.count));
    }
    // A library counts blocks, and one block is the partition's allocation unit of net
    // bytes. Until that unit has arrived the count is all there is to say.
    let Some(unit) = unit else {
        return Some(format!("{} items", status.count));
    };
    let bytes = |units: u64| units.saturating_mul(u64::from(unit.get()));
    Some(crate::room::measure_out_of(
        bytes(u64::from(status.used)),
        bytes(status.total()),
    ))
}

/// The allocation unit a partition reporting `bytes` per unit would hand back.
///
/// ⚠️ Built the one way there is to build one — out of a partition record — so a test
/// cannot invent a unit the wire could not carry.
#[cfg(test)]
pub fn pretend_allocation_unit(class: ObjectClass, bytes: u32) -> AllocationUnit {
    nord_usb::wire::Partition {
        index: class.to_raw(),
        name: String::new(),
        native: false,
        fields: bytes.to_be_bytes().to_vec(),
    }
    .allocation_unit()
    .expect("a partition reporting a unit of at least one")
}

/// The partition table an Electro 5 declares, in its own order, with the net bytes each
/// partition counts in units of. The `(Native)` rows are left out.
///
/// Confirmed on hardware.
#[cfg(test)]
pub const ELECTRO5: [(ObjectClass, &str, u32); 6] = [
    (ObjectClass::Piano, "Piano", 261_632),
    (ObjectClass::Sample, "Samp Lib", 131_064),
    (ObjectClass::Program, "Program", 1),
    (ObjectClass::SetList, "Set List", 1),
    (ObjectClass::Live, "Live", 1),
    (ObjectClass::Settings, "Settings", 1),
];

/// Whether the attached instrument takes an asset, and what is worth saying about it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Nothing is attached, so nothing can be said.
    Unattached,
    Takes,
    /// It should land, and this is what has not been checked.
    Warn(String),
    /// It is another instrument's, and this is why.
    Refuses(String),
}

impl Fit {
    /// Whether a write may be attempted. Only an outright refusal stops one.
    pub fn allowed(&self) -> bool {
        !matches!(self, Fit::Refuses(_))
    }

    /// The sentence, where there is one to show.
    pub fn why(&self) -> Option<&str> {
        match self {
            Fit::Warn(why) | Fit::Refuses(why) => Some(why),
            Fit::Unattached | Fit::Takes => None,
        }
    }
}

/// Whether the attached instrument takes this asset, from the acceptance table in
/// `nord-format` and, where that table says nothing, from what the folder is holding.
///
/// The asset's own folder decides the class: an asset is only ever written to the folder
/// its kind belongs in, so that is the one question worth asking.
pub fn fit(state: &DeviceState, entity: &LocalEntity) -> Fit {
    let Some(product) = state.product() else {
        return Fit::Unattached;
    };
    let Some(class) = crate::browser::Kind::of(entity.entity.as_ref()).home() else {
        return Fit::Takes;
    };
    let tag = entity.tag();
    let resident = || crate::browser::foreign_format(&tag, &state.formats_in(class));
    let unknown = || match resident() {
        Some(why) => Fit::Warn(why),
        None => Fit::Takes,
    };
    let (Some(slot), Some(family)) = (class.storage(), Family::from_product(product)) else {
        return unknown();
    };
    match family.accepts(slot, &tag) {
        Acceptance::Confirmed => Fit::Takes,
        Acceptance::Inferred => Fit::Warn(format!(
            "This is a {} file and the instrument is a {product}, but no file of this \
             kind has ever been written to one. Sending it is untried.",
            family.label()
        )),
        Acceptance::Refused => Fit::Refuses(match Family::of_tag(&tag) {
            Some(owner) => format!(
                "This is a {} file and the instrument is a {product}.",
                owner.label()
            ),
            // ⚠️ Unreachable while `accepts` refuses only a tag another family carries;
            // stated rather than unwrapped so a widened table cannot panic here.
            None => format!("A {tag} file is not one a {product} takes."),
        }),
        Acceptance::Unknown => unknown(),
    }
}

/// The slot on the attached instrument this asset stands on.
///
/// The slot it came off, while the instrument holds that slot — whatever it holds now.
/// An asset copied off 1:1 and changed here is still the copy of 1:1, and what the two
/// have made of each other since is [`crate::library::agrees`]'s question rather than
/// this one.
///
/// With no origin, or an origin the walk found vacant, the saved body is what matches:
/// the CRC-32 a type-1 container carries **is** the checksum a walk reports for a slot —
/// see the round trip in [`crate::workspace`] — so an asset and a slot are matched
/// without either body being hashed again, and [`among`] decides which of them where
/// several hold it. A class whose slots report no checksum is matched by [`named`]
/// instead. Saved rather than held now, which is the body
/// [`crate::library::keyboard_mark`] and the row's own sign are read against, so an
/// edit nothing has saved cannot make the three disagree about where this stands.
///
/// ⚠️ Takes the link the asset already carries as its own input, so running it again
/// over an unchanged cache answers the same thing. That is what lets an edit here keep
/// the asset pointing where it was matched.
pub fn link(state: &DeviceState, entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    // ⚠️ Before anything else: a foreign body whose CRC-32 happens to match a slot's
    // would otherwise be linked into a folder the instrument would refuse it from.
    if !fit(state, entity).allowed() {
        return None;
    }
    let class = home(entity)?;
    if let Some(origin) = entity
        .origin
        .slot()
        .filter(|(class, at)| state.slot(*class, *at).flatten().is_some())
    {
        return Some(origin);
    }
    let by_body = matchable(entity).and_then(|(folder, crc)| among(state, folder, crc, entity));
    match by_body {
        Some(at) => Some((class, at)),
        None => stands(state, entity).or_else(|| Some((class, named(state, class, entity)?))),
    }
}

/// How many further slots hold what this asset was saved as, beyond the one it is
/// linked to.
pub fn also_holding(state: &DeviceState, entity: &LocalEntity) -> usize {
    let Some((class, here)) = matchable(entity) else {
        return 0;
    };
    holding(state, class, here).count().saturating_sub(1)
}

/// The folder an asset belongs in.
///
/// A view is not on this computer until it is kept, and bytes that decode into nothing
/// belong in no folder.
fn home(entity: &LocalEntity) -> Option<ObjectClass> {
    entity
        .kept
        .then(|| crate::browser::Kind::of(entity.entity.as_ref()).home())
        .flatten()
}

/// That folder and the checksum a slot holding what this asset was saved as would
/// report — see [`crate::workspace::Baseline::crc32`].
fn matchable(entity: &LocalEntity) -> Option<(ObjectClass, u32)> {
    Some((home(entity)?, entity.saved.crc32?))
}

/// The slot a class whose slots report no checksum is matched to.
///
/// ⚠️ Settings, samples and pianos report no body checksum, so no body can be recognised
/// in one of their slots. Settings holds a single slot and that slot is the link; a
/// sample or a piano is matched to the slot the instrument gave this asset's own name.
/// A name is a label rather than a body, which [`crate::library::Where::Both`] says by
/// leaving the sign off until a compare read settles it.
fn named(state: &DeviceState, class: ObjectClass, entity: &LocalEntity) -> Option<Location> {
    match class {
        ObjectClass::Settings => {
            let mut slots = occupied(state, class);
            let (at, _) = slots.next()?;
            slots.next().is_none().then_some(at)
        }
        ObjectClass::Sample | ObjectClass::Piano => {
            let name = entity.name.trim();
            occupied(state, class)
                .find(|(_, info)| info.name.trim() == name)
                .map(|(at, _)| at)
        }
        _ => None,
    }
}

/// Every slot of `class` a walk found holding something, lowest address first.
fn occupied(
    state: &DeviceState,
    class: ObjectClass,
) -> impl Iterator<Item = (Location, &ProgramInfo)> + '_ {
    state.banks_of(class).into_iter().flat_map(move |bank| {
        state
            .bank(class, bank)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .filter_map(move |(slot, held)| {
                Some((Location::from_user(bank, slot as u32 + 1), held.as_ref()?))
            })
    })
}

/// Every slot of `class` whose scanned checksum is `crc`, lowest address first.
///
/// ⚠️ A class whose slots report no checksum matches nothing here. A name and a length
/// are not a body, and a library is where two different objects most readily share both.
fn holding(
    state: &DeviceState,
    class: ObjectClass,
    crc: u32,
) -> impl Iterator<Item = Location> + '_ {
    occupied(state, class)
        .filter(move |(_, info)| info.crc32 == Some(crc))
        .map(|(at, _)| at)
}

/// Which of the slots holding this asset's bytes it is matched to.
///
/// One body can sit in any number of slots, and the address the row shows is the one
/// the user has reason to expect: the slot it came off, or the slot it stands on — which
/// [`Workspace::landed`] set to the slot this app wrote it to. An asset standing nowhere
/// takes the lowest address holding it.
fn among(
    state: &DeviceState,
    class: ObjectClass,
    crc: u32,
    entity: &LocalEntity,
) -> Option<Location> {
    let held = |known: Option<(ObjectClass, Location)>| {
        known
            .filter(|(held, at)| {
                *held == class && holding(state, class, crc).any(|other| other == *at)
            })
            .map(|(_, at)| at)
    };
    held(entity.origin.slot())
        .or_else(|| held(entity.link))
        .or_else(|| holding(state, class, crc).next())
}

/// The link an asset keeps when no slot holds its bytes any more: an edit here moved
/// them, or [`Workspace::landed`] wrote them there, and where it stands is still where
/// it stands.
///
/// Whatever the slot holds now, and whether or not it reports a checksum of its own — a
/// slot this app has just written is holding what it was given.
///
/// ⚠️ Until a walk says otherwise. A slot found vacant holds nothing to point at. A bank
/// no walk has reached is silence rather than an answer, which is what a write must
/// outlive: [`Device::dispatch`] drops the bank it is about to change, and the read that
/// fills it in again lands long after the write does.
fn stands(state: &DeviceState, entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    let (class, at) = entity.link?;
    match state.slot(class, at) {
        Some(None) => None,
        Some(Some(_)) | None => Some((class, at)),
    }
}

/// Whether the browser offers to change a class at all.
///
/// ⚠️ A partition this app cannot name is listed and left alone: nothing here knows what
/// its slots hold or what a write into one would mean. Every class it can name takes a
/// write, a piano library included — that one is sent like a sample, and whether the
/// partition has room for it is [`crate::room::free_bytes`]'s question rather than this
/// one.
pub fn read_only(class: ObjectClass) -> bool {
    matches!(class, ObjectClass::Unknown(_))
}

/// Whether this app will write into a class at all.
pub fn sendable(class: ObjectClass) -> bool {
    !read_only(class)
}

/// What a write into this class disturbs beyond the slot it lands in, for the question
/// asked before it happens.
///
/// Confirmed on hardware.
///
/// A settings write makes the instrument reload the selected program, so panel state
/// the player has not stored is gone.
pub fn write_warning(class: ObjectClass) -> Option<&'static str> {
    match class {
        ObjectClass::Settings => Some(
            "Writing settings reloads the selected program on the instrument. \
             Panel changes that have not been stored will be lost.",
        ),
        _ => None,
    }
}

pub struct Device {
    pub state: DeviceState,
    events: Receiver<DeviceEvent>,
    /// A second handle on the worker's end of the channel, so a headless test can hand
    /// [`Device::poll`] the events an instrument would have reported.
    #[cfg(test)]
    from_worker: std::sync::mpsc::Sender<DeviceEvent>,
    link: Link,
    /// What the user asked for. Always dispatched ahead of the background scan.
    pending: VecDeque<DeviceCmd>,
    /// The class the running command is walking, so a scan that fails is taken off the
    /// queue rather than left looking like it is still going.
    reading: Option<ObjectClass>,
    /// The banks the running mutation touches, to be read again once it finishes.
    rescan: Vec<(ObjectClass, u32)>,
    /// The loaded slots the running command overwrites. A batch can touch one per
    /// class, so this is a list rather than a single slot.
    reselect: Vec<(ObjectClass, Location)>,
    /// The class the running command writes into, so a refusal can be put against the
    /// entry of the queue it stopped on.
    writing: Option<ObjectClass>,
    /// The list revision every link was last derived from. A link answers about both
    /// sides, so it is re-made when either has moved and not once a frame besides.
    linked: u64,
}

impl Device {
    pub fn new(ctx: egui::Context) -> Device {
        let (sender, events) = std::sync::mpsc::channel();
        Device {
            state: DeviceState::default(),
            events,
            #[cfg(test)]
            from_worker: sender.clone(),
            link: Link::new(ctx, sender),
            pending: VecDeque::new(),
            reading: None,
            rescan: Vec::new(),
            reselect: Vec::new(),
            writing: None,
            linked: 0,
        }
    }

    /// ⚠️ Must be reached from the frame the button was clicked in. On the web the
    /// device chooser needs the click's transient user activation, and awaiting
    /// anything first spends it.
    pub fn connect(&mut self, log: &mut Log) {
        if !matches!(self.state.connection, Connection::Disconnected) {
            return;
        }
        self.state.connection = Connection::Connecting;
        log.say("Looking for an instrument…");
        self.link.connect();
    }

    pub fn disconnect(&mut self, log: &mut Log) {
        if !self.state.connected() {
            return;
        }
        log.say("Releasing the instrument…");
        self.pending.clear();
        self.state.scan.clear();
        self.link.disconnect();
    }

    /// Queue one command the user asked for. It runs ahead of the background read, and
    /// after whatever is already in flight — the protocol runs one transaction at a
    /// time.
    pub fn send(&mut self, cmd: DeviceCmd, log: &mut Log) {
        if !self.state.connected() {
            log.trouble("No instrument is attached.");
            return;
        }
        self.pending.push_back(cmd);
    }

    /// Walk `class` again, in one session.
    ///
    /// The names already cached stay up until each bank's replacement arrives: a walk is
    /// dozens of reads long, and emptying the folder for the length of one is worse than
    /// showing names that are about to be confirmed. What a mutation touched is dropped
    /// outright — see [`Device::dispatch`].
    pub fn read_class(&mut self, class: ObjectClass) {
        self.state.scan.start(class);
    }

    /// Read the whole instrument again: every class it declares, and with each one its
    /// counters, its geometry and the slot the panel has loaded.
    ///
    /// One walk per class, which is the same thing attaching does — a class's counters,
    /// banks and focus are all read at the head of its own session, so there is nothing
    /// else to ask for.
    pub fn resync(&mut self) {
        for class in self.state.classes() {
            self.read_class(class);
        }
    }

    /// Start the next command if the instrument is free. Call once a frame, after the
    /// UI has had its say.
    pub fn pump(&mut self) {
        if !self.state.connected() || self.state.in_flight.is_some() {
            return;
        }
        if let Some(cmd) = self.pending.pop_front() {
            self.reading = None;
            return self.dispatch(cmd);
        }
        let Some(class) = self.state.scan.take() else {
            return;
        };
        self.reading = Some(class);
        self.dispatch(DeviceCmd::ScanClass { class });
    }

    fn dispatch(&mut self, cmd: DeviceCmd) {
        // Drop affected banks before a mutation so confirmations never quote stale names.
        self.rescan = match &cmd {
            DeviceCmd::Delete { class, at }
            | DeviceCmd::Rename { class, at, .. }
            | DeviceCmd::Put { class, at, .. } => vec![(*class, at.bank + 1)],
            DeviceCmd::Move { class, from, to } | DeviceCmd::Duplicate { class, from, to } => {
                vec![(*class, from.bank + 1), (*class, to.bank + 1)]
            }
            DeviceCmd::SendAll { class, items } => {
                let mut banks: Vec<(ObjectClass, u32)> = items
                    .iter()
                    .map(|item| (*class, item.at.bank + 1))
                    .collect();
                banks.sort_unstable_by_key(|(class, bank)| (class.to_raw(), *bank));
                banks.dedup();
                banks
            }
            _ => Vec::new(),
        };
        for (class, bank) in &self.rescan {
            self.state.forget_bank(*class, *bank);
        }
        // Confirmed on hardware: writing the loaded slot requires SELECT to reload it.
        let loaded = |state: &DeviceState, class: &ObjectClass, at: &Location| {
            state
                .selected
                .get(&class.to_raw())
                .filter(|held| *held == at)
                .map(|at| (*class, *at))
        };
        self.reselect = match &cmd {
            DeviceCmd::Put { class, at, .. } | DeviceCmd::Rename { class, at, .. } => {
                loaded(&self.state, class, at).into_iter().collect()
            }
            DeviceCmd::SendAll { class, items } => items
                .iter()
                .filter_map(|item| loaded(&self.state, class, &item.at))
                .collect(),
            _ => Vec::new(),
        };
        self.writing = match &cmd {
            DeviceCmd::Put { class, .. } | DeviceCmd::SendAll { class, .. } => Some(*class),
            _ => None,
        };
        if let DeviceCmd::Select { class, at } = &cmd {
            self.state.selected.insert(class.to_raw(), *at);
        }
        self.state.in_flight = Some(cmd.words());
        self.link.send(cmd);
    }

    /// Hand `poll` an event as though the worker had reported it.
    #[cfg(test)]
    pub fn pretend(&mut self, event: DeviceEvent) {
        let _ = self.from_worker.send(event);
    }

    /// What the user has asked the instrument for and it has not started yet.
    #[cfg(test)]
    pub fn queued(&self) -> &VecDeque<DeviceCmd> {
        &self.pending
    }

    /// Attach an instrument, as its descriptors would have. The product string is the
    /// one the recorded exchanges in `nord-usb` carry.
    #[cfg(test)]
    pub fn pretend_attached(&mut self) {
        self.pretend_attached_as("Nord Electro 5");
    }

    /// Attach an instrument reporting a product string of its own, for the rules that
    /// turn on which model it is.
    #[cfg(test)]
    pub fn pretend_attached_as(&mut self, product: &str) {
        self.state.connection = Connection::Connected(DeviceCard {
            build: Some(7),
            firmware: Some(204),
            interface: Some(3),
            kind: Some(1),
            manufacturer: Some("Clavia DMI AB".into()),
            max_transfer: Some(4096),
            product: product.to_string(),
            product_id: 0,
            serial: None,
            vendor_id: 0x0ffc,
        });
    }

    /// Fill in a bank as though a walk had answered, under the panel's own bank number —
    /// which is the number [`DeviceEvent::BankScanned`] carries.
    #[cfg(test)]
    pub fn pretend_scanned(&mut self, class: ObjectClass, bank: u32, names: &[&str]) {
        use nord_usb::wire::ProgramInfo;

        self.pretend_attached();
        let slots = names
            .iter()
            .enumerate()
            .map(|(slot, name)| {
                // An empty name is a vacant slot, which is a row like any other.
                (!name.is_empty()).then(|| ProgramInfo {
                    location: Location::from_user(bank, slot as u32 + 1),
                    body_len: 121,
                    format: "ne5p".into(),
                    version: 4,
                    crc32: None,
                    name: (*name).to_string(),
                })
            })
            .collect();
        self.state.banks.insert((class.to_raw(), bank), slots);
    }

    /// Give the instrument the partition table it would have declared: the class of each
    /// row, the device's own name for it, and the net bytes its counters are in units of.
    #[cfg(test)]
    pub fn pretend_partitions(&mut self, table: &[(ObjectClass, &str, u32)]) {
        self.state.partitions = table
            .iter()
            .map(|(class, name, bytes)| Partition {
                class: *class,
                name: (*name).to_string(),
                native: false,
                unit: Some(pretend_allocation_unit(*class, *bytes)),
            })
            .collect();
    }

    /// Fill in a bank as a walk of a class that checksums what it holds would have: a
    /// name and the body CRC-32 the device reports for each occupied slot.
    ///
    /// The counterpart of [`Device::pretend_scanned`], whose slots report no checksum.
    #[cfg(test)]
    pub fn pretend_bodies(&mut self, class: ObjectClass, bank: u32, slots: &[Option<(&str, u32)>]) {
        self.pretend_attached();
        let slots = slots
            .iter()
            .enumerate()
            .map(|(slot, held)| {
                held.map(|(name, crc)| ProgramInfo {
                    location: Location::from_user(bank, slot as u32 + 1),
                    body_len: 121,
                    format: "ne5p".into(),
                    version: 4,
                    crc32: Some(crc),
                    name: name.to_string(),
                })
            })
            .collect();
        self.state.banks.insert((class.to_raw(), bank), slots);
    }

    /// Give a class the banks the device would have reported, for a headless render.
    #[cfg(test)]
    pub fn pretend_geometry(&mut self, class: ObjectClass, banks: &[(&str, u32)]) {
        let banks = banks
            .iter()
            .enumerate()
            .map(|(index, (name, slots))| Bank {
                index: index as u32,
                name: (*name).to_string(),
                slots: *slots,
            })
            .collect();
        self.state.geometry.insert(class.to_raw(), banks);
    }

    /// Put the panel on a slot, as a walk's `FOCUS` read would have.
    #[cfg(test)]
    pub fn pretend_focused(&mut self, class: ObjectClass, at: Location) {
        self.state.focus.insert(class.to_raw(), Some(at));
    }

    /// Point every asset at the slot holding its bytes.
    ///
    /// A link is derived rather than stored, so it is re-made from the scan cache each
    /// time round: it compares a checksum the walk already reported with one the
    /// container already carries, and never touches a body.
    pub fn relink(&self, workspace: &mut Workspace) {
        let state = &self.state;
        workspace.relink(|entity| link(state, entity));
    }

    /// Drop everything the last instrument said, the links included.
    ///
    /// A link is a fact about an attached instrument. With none attached, or another one
    /// in its place, there is nothing for an asset to stand on until a walk says so —
    /// and an empty cache alone does not say that, or a write would unlink what it just
    /// wrote.
    fn forget(&mut self, workspace: &mut Workspace) {
        self.state.forget_everything();
        workspace.relink(|_| None);
        workspace.forget_writes();
    }

    /// Say in the log where a slot just read holds a body the asset standing on it was
    /// not saved as.
    ///
    /// Once per asset per read of its bank. Two checksums and an address are protocol
    /// detail: what the user reads is the row's own sign.
    fn disagreements(&self, class: ObjectClass, bank: u32, workspace: &Workspace, log: &mut Log) {
        for entity in workspace.listed() {
            let Some(at) = entity
                .link
                .filter(|(held, at)| *held == class && at.bank + 1 == bank)
                .map(|(_, at)| at)
            else {
                continue;
            };
            let (Some(here), Some(there)) = (
                entity.saved.crc32,
                self.state
                    .slot(class, at)
                    .flatten()
                    .and_then(|info| info.crc32),
            ) else {
                continue;
            };
            if here != there {
                log.info(format!(
                    "{} reports crc32 {there:#010x}; “{}” is saved as {here:#010x}",
                    place(class, at),
                    entity.name
                ));
            }
        }
    }

    /// Drain the worker's events into the cache, the local list and the tabs. Call once
    /// a frame.
    pub fn poll(
        &mut self,
        log: &mut Log,
        workspace: &mut Workspace,
        tabs: &mut Tabs,
        queue: &mut Queue,
    ) {
        let now = workspace.ctx().input(|input| input.time);
        let mut heard = false;
        while let Ok(event) = self.events.try_recv() {
            heard = true;
            match event {
                DeviceEvent::Connected(card) => {
                    log.info(format!(
                        "connected: {} ({:04x}:{:04x})",
                        card.product, card.vendor_id, card.product_id
                    ));
                    log.say(format!("{} is attached.", card.product));
                    self.state.connection = Connection::Connected(card);
                    self.forget(workspace);
                    self.pending.clear();
                }
                DeviceEvent::ConnectFailed(why) => {
                    log.error(why);
                    log.trouble("No instrument could be opened.");
                    self.state.connection = Connection::Disconnected;
                }
                // ⚠️ Disconnection must not clear local edits waiting for the instrument.
                DeviceEvent::Disconnected { lost } => {
                    match lost {
                        true => log.trouble("The instrument went away — reconnect when it's back."),
                        false => log.say("The instrument was released."),
                    }
                    self.state.connection = Connection::Disconnected;
                    self.state.in_flight = None;
                    self.forget(workspace);
                    self.pending.clear();
                    self.reading = None;
                    self.rescan.clear();
                    self.reselect.clear();
                    self.writing = None;
                }
                DeviceEvent::Started(what) => log.info(what),
                DeviceEvent::Finished => {
                    if let Some(class) = self.reading.take() {
                        self.state.scan.finished(class);
                        self.state.scan.heard(class, now);
                    }
                    // The panel is still playing what it read before the write, so it is
                    // asked to load the slot again. `select` is read-only.
                    for (class, at) in std::mem::take(&mut self.reselect) {
                        self.pending.push_back(DeviceCmd::Select { class, at });
                    }
                    for (class, bank) in std::mem::take(&mut self.rescan) {
                        self.pending.push_back(DeviceCmd::ScanBank { class, bank });
                    }
                    self.writing = None;
                    self.state.in_flight = None;
                }
                DeviceEvent::ClassStatus {
                    class,
                    status,
                    banks,
                } => {
                    self.state.inventory.retain(|held| held.class != class);
                    self.state.inventory.push(status);
                    self.state.scan.expect(class, banks);
                }
                // Which classes exist is the instrument's answer, so the walk of them
                // can only start here. Each class reads its counters, its banks and its
                // focus in one session.
                // ⚠️ What is waiting was checked against whatever was attached when it
                // was queued, and the queue outlives a disconnection.
                DeviceEvent::Partitions(partitions) => {
                    self.state.partitions = partitions;
                    crate::queue::refit(workspace, &self.state, queue, log);
                    self.resync();
                }
                DeviceEvent::Geometry { class, banks } => {
                    self.state.geometry.insert(class.to_raw(), banks);
                }
                DeviceEvent::Focus { class, at } => {
                    self.state.focus.insert(class.to_raw(), at);
                }
                DeviceEvent::BankScanned { class, bank, slots } => {
                    if !slots.is_empty() {
                        self.state.banks.insert((class.to_raw(), bank), slots);
                    }
                    self.state.scan.bank(class, bank);
                    self.state.scan.heard(class, now);
                    self.disagreements(class, bank, workspace, log);
                }
                DeviceEvent::SlotInfo { at, info, .. } => {
                    self.state.detail = Detail {
                        at: Some(at),
                        info,
                        asked: true,
                        deps: None,
                    };
                }
                DeviceEvent::Deps { at, deps, .. } => {
                    if self.state.detail.at != Some(at) {
                        self.state.detail = Detail {
                            at: Some(at),
                            ..Detail::default()
                        };
                    }
                    self.state.detail.deps = Some(deps);
                }
                // A view belongs to its tab, a copied read becomes a local entity, and
                // an occupant read for a diff belongs to the queue and to nothing else.
                DeviceEvent::Got {
                    name,
                    origin,
                    bytes,
                    why,
                } => match why {
                    Purpose::View => {
                        let id = workspace.view(name, origin, bytes, log);
                        tabs.open(id);
                    }
                    Purpose::Copy => {
                        workspace.ingest(name, origin, bytes, log);
                    }
                    Purpose::Compare => {
                        if let Some((class, at)) = origin.slot() {
                            queue.arrived(class, at, &name, &bytes, workspace);
                        }
                    }
                },
                // A slot that is not there to copy or open is a failure to the user, and
                // an answer to the queue: nothing is being replaced.
                DeviceEvent::Vacant { class, at, why } => match why {
                    Purpose::Compare => queue.vacant(class, at),
                    Purpose::Copy | Purpose::View => {
                        log.error(format!("{} holds nothing to read", shown(at)));
                        log.trouble(format!("{} is empty.", place(class, at)));
                    }
                },
                DeviceEvent::Rescued { at, name, bytes } => {
                    log.error(format!(
                        "{} could not be restored; its bytes are in the local list as {name}",
                        shown(at)
                    ));
                    log.trouble(format!(
                        "{} is empty — what was in it is on this computer as “{name}”.",
                        shown(at)
                    ));
                    workspace.ingest(name, Origin::Rescued { at }, bytes, log);
                }
                // It landed, so it is no longer owed; what landed is what it is saved as,
                // and the slot it landed in is where it stands. Only that object: the
                // rest of a batch is still waiting on its own write.
                DeviceEvent::Sent {
                    id,
                    class,
                    at,
                    bytes,
                } => {
                    queue.forget(id);
                    workspace.landed(id, class, at, bytes);
                }
                DeviceEvent::Note(text) => log.info(text),
                DeviceEvent::OpOk(text) => {
                    log.info(text);
                    if let Some(words) = &self.state.in_flight {
                        log.say(format!("{}.", words.done));
                    }
                }
                DeviceEvent::OpFailed(text) => {
                    if let Some(class) = self.writing {
                        queue.stumbled(class, &text);
                    }
                    log.error(text);
                    match &self.state.in_flight {
                        Some(words) => {
                            let failed = words.failed.clone();
                            log.trouble(format!("{failed}. The details are below."));
                        }
                        None => log.trouble("Something went wrong. The details are below."),
                    }
                }
                // External changes invalidate every cached name used by later dialogs.
                DeviceEvent::InstrumentChanged => {
                    log.warn("the instrument changed under us — every cached name is dropped");
                    log.say("Something changed on the instrument. Reading it again…");
                    self.resync();
                }
            }
        }
        // Both sides of a link move: the instrument said something, or an asset arrived,
        // changed, was kept or was reverted. An asset that arrives while an instrument
        // is attached stands wherever the cache already says it does, without waiting
        // for the next walk to report a bank again.
        if heard || self.linked != workspace.revision() {
            self.linked = workspace.revision();
            self.relink(workspace);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class this app has a name for, which is what the rules below are about.
    /// The instrument declares its own.
    fn named() -> impl Iterator<Item = ObjectClass> {
        crate::browser::Kind::ALL
            .into_iter()
            .filter_map(|kind| kind.home())
    }

    fn table() -> Vec<Partition> {
        vec![
            Partition {
                class: ObjectClass::Unknown(0),
                name: "Piano (Native)".into(),
                native: true,
                unit: None,
            },
            Partition {
                class: ObjectClass::Piano,
                name: "Piano".into(),
                native: false,
                unit: Some(pretend_allocation_unit(ObjectClass::Piano, 261_632)),
            },
            Partition {
                class: ObjectClass::Sample,
                name: "Samp Lib".into(),
                native: false,
                unit: Some(pretend_allocation_unit(ObjectClass::Sample, 131_064)),
            },
            Partition {
                class: ObjectClass::Unknown(9),
                name: "Rhythms".into(),
                native: false,
                unit: None,
            },
        ]
    }

    /// The classes the app walks are the instrument's own table, and a partition this
    /// app cannot name is one of them — under the device's word for it, and read only.
    ///
    /// ⚠️ A `(Native)` row is a second view of a library already in the table, so it is
    /// not a folder of its own.
    #[test]
    fn the_instrument_says_which_classes_it_has() {
        let ctx = egui::Context::default();
        let mut device = Device::new(ctx.clone());
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        assert!(device.state.classes().is_empty(), "nothing read yet");

        device.pretend(DeviceEvent::Partitions(table()));
        device.poll(&mut log, &mut workspace, &mut tabs, &mut Queue::default());
        assert_eq!(
            device.state.classes(),
            vec![
                ObjectClass::Piano,
                ObjectClass::Sample,
                ObjectClass::Unknown(9)
            ]
        );
        assert_eq!(device.state.folder_name(ObjectClass::Piano), "Pianos");
        assert_eq!(device.state.folder_name(ObjectClass::Unknown(9)), "Rhythms");
        assert!(read_only(ObjectClass::Unknown(9)));

        // Every class the table declared is walked, and nothing else is.
        for class in device.state.classes() {
            assert!(device.state.scan.progress(class).is_some(), "{class:?}");
        }
        assert!(device.state.scan.progress(ObjectClass::Program).is_none());

        device.pretend(DeviceEvent::Disconnected { lost: false });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut Queue::default());
        assert!(device.state.classes().is_empty());
    }

    /// The unit a count is measured in is the partition's own, so it arrives with the
    /// table and is forgotten when the instrument goes.
    #[test]
    fn a_count_is_measured_in_the_unit_its_partition_reports() {
        let ctx = egui::Context::default();
        let mut device = Device::new(ctx.clone());
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let class = ObjectClass::Sample;
        assert_eq!(
            device.state.allocation_unit(class),
            None,
            "nothing read yet"
        );

        device.pretend(DeviceEvent::Partitions(table()));
        device.poll(&mut log, &mut workspace, &mut tabs, &mut Queue::default());
        assert_eq!(
            device.state.allocation_unit(class).map(|unit| unit.get()),
            Some(131_064)
        );
        assert_eq!(
            device.state.allocation_unit(ObjectClass::Unknown(9)),
            None,
            "a partition that reported none"
        );

        device.pretend(DeviceEvent::Disconnected { lost: false });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut Queue::default());
        assert_eq!(device.state.allocation_unit(class), None);
    }

    /// ⚠️ A library counts blocks, not bytes, and one block is its partition's
    /// allocation unit. Until that unit has arrived the count is all the row can say —
    /// a block count read as bytes would be off by five orders of magnitude.
    #[test]
    fn a_library_reads_in_bytes_only_once_its_allocation_unit_has_arrived() {
        let class = ObjectClass::Sample;
        // The shape an Electro 5 answers with: 1 472 of the partition's 1 536 blocks
        // in use, each 131 064 net bytes.
        let inventory = [Status {
            class,
            count: 84,
            free: 60,
            used: 1472,
            dirty: 0,
            spare: 4,
        }];
        assert_eq!(
            occupancy(class, &inventory, None).as_deref(),
            Some("84 items"),
            "the count alone until the unit lands"
        );
        assert_eq!(
            occupancy(
                class,
                &inventory,
                Some(pretend_allocation_unit(class, 131_064))
            )
            .as_deref(),
            Some("184.0/192.0 MB")
        );

        // A slot-addressed class divides into slots, so it never reaches the unit at all.
        let programs = [Status {
            class: ObjectClass::Program,
            count: 128,
            free: 272 * 121,
            used: 128 * 121,
            dirty: 0,
            spare: 0,
        }];
        assert_eq!(
            occupancy(ObjectClass::Program, &programs, None).as_deref(),
            Some("128/400")
        );
    }

    /// ⚠️ A partition reads in the unit its own total deserves, and both figures in that
    /// one unit. The Live and Settings partitions hold less than a megabyte, and in
    /// megabytes each of them reads 0/0.
    #[test]
    fn a_partition_reads_in_the_unit_its_total_deserves() {
        let partition = |class, used: u32, free: u32| {
            [Status {
                class,
                count: 0,
                free,
                used,
                dirty: 0,
                spare: 0,
            }]
        };
        let byte = |class| Some(pretend_allocation_unit(class, 1));

        let live = ObjectClass::Live;
        assert_eq!(
            occupancy(live, &partition(live, 121, 379), byte(live)).as_deref(),
            Some("121/500 B")
        );

        // The part takes the whole's unit rather than its own: 500 bytes alone would
        // read in bytes, and would then look larger than the 24 kB it sits inside.
        let settings = ObjectClass::Settings;
        assert_eq!(
            occupancy(settings, &partition(settings, 500, 24_076), byte(settings)).as_deref(),
            Some("0.5/24.0 kB")
        );

        let samples = ObjectClass::Sample;
        assert_eq!(
            occupancy(
                samples,
                &partition(samples, 128, 128),
                Some(pretend_allocation_unit(samples, 1024 * 1024))
            )
            .as_deref(),
            Some("128.0/256.0 MB")
        );
    }

    /// Every folder this app can name takes a write — the buffer classes and the two
    /// libraries alike. A partition it cannot name is listed and left alone.
    #[test]
    fn only_a_class_with_no_name_is_read_only() {
        for class in named() {
            assert!(sendable(class), "{}", folder(class));
            assert!(!read_only(class), "{}", folder(class));
        }
        assert!(read_only(ObjectClass::Unknown(9)));
        assert!(!sendable(ObjectClass::Unknown(9)));
    }

    #[test]
    fn a_settings_write_warns_that_the_panel_reloads() {
        let why = write_warning(ObjectClass::Settings).expect("must warn");
        assert!(why.contains("reloads the selected program"), "{why}");
        for class in named().filter(|class| *class != ObjectClass::Settings) {
            assert!(write_warning(class).is_none(), "{}", folder(class));
        }
    }

    /// What landed is no longer owed, and nothing else is touched — a batch that stops
    /// halfway leaves the rest of the queue exactly as it was.
    #[test]
    fn a_sent_event_clears_the_object_it_names_and_no_other() {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();

        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let at = |slot| Location { bank: 6, slot };
        let landed = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(3),
            },
            bytes.clone(),
            &mut log,
        );
        let still_owed = workspace.ingest(
            "Squabble-B.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(4),
            },
            bytes,
            &mut log,
        );
        let mut queue = Queue::default();
        for (id, slot) in [(landed, 3), (still_owed, 4)] {
            crate::queue::enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                ObjectClass::Program,
                at(slot),
            );
        }

        device.pretend(DeviceEvent::Sent {
            id: landed,
            class: ObjectClass::Program,
            at: at(3),
            bytes: workspace.get(landed).unwrap().bytes.clone(),
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        assert!(!queue.holds(landed), "it was written");
        assert!(queue.holds(still_owed), "still waiting");
        assert_eq!(queue.ids(), vec![still_owed]);
    }

    /// One program on this computer, and the body checksum a walk would report for the
    /// slot holding it.
    fn program(workspace: &mut Workspace, log: &mut Log, origin: Origin) -> (u64, u32) {
        let made = workspace
            .create(crate::workspace::Fresh::Program, log)
            .unwrap();
        let bytes = workspace.get(made).unwrap().bytes.clone();
        workspace.remove(made, log);
        let id = workspace.ingest("Africa-Split.ne5p".into(), origin, bytes, log);
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32)
            .expect("every CBIN container has one");
        (id, crc)
    }

    /// An asset of another family's, on this computer.
    fn stage(workspace: &mut Workspace, log: &mut Log, origin: Origin) -> (u64, u32) {
        let made = workspace
            .create(crate::workspace::Fresh::Stage4Program, log)
            .unwrap();
        let bytes = workspace.get(made).unwrap().bytes.clone();
        workspace.remove(made, log);
        let id = workspace.ingest("Africa-Split.ns4p".into(), origin, bytes, log);
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32)
            .expect("every CBIN container has one");
        (id, crc)
    }

    /// The four answers the acceptance table gives, and the fifth for nothing attached.
    #[test]
    fn what_the_instrument_takes_is_decided_by_the_table_then_by_the_folder() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (own, _) = program(&mut workspace, &mut log, Origin::Fresh);
        let (other, _) = stage(&mut workspace, &mut log, Origin::Fresh);
        fn held(device: &Device, workspace: &Workspace, id: u64) -> Fit {
            fit(&device.state, workspace.get(id).expect("it is on the list"))
        }
        let held = |device: &Device, id| held(device, &workspace, id);

        assert_eq!(held(&device, own), Fit::Unattached);

        device.pretend_attached();
        assert_eq!(held(&device, own), Fit::Takes, "confirmed on hardware");
        match held(&device, other) {
            Fit::Refuses(why) => {
                assert!(why.contains("Stage 4"), "{why}");
                assert!(why.contains("Nord Electro 5"), "{why}");
            }
            answer => panic!("a Stage 4 program on an Electro 5: {answer:?}"),
        }

        device.pretend_attached_as("Nord Stage 4 88");
        match held(&device, other) {
            Fit::Warn(why) => assert!(why.contains("untried"), "{why}"),
            answer => panic!("a Stage 4 program on a Stage 4: {answer:?}"),
        }

        // A model with no row in the table falls back to what the folder is holding.
        device.pretend_attached_as("unnamed device");
        assert_eq!(held(&device, own), Fit::Takes);
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);
        device.pretend_attached_as("unnamed device");
        match held(&device, other) {
            Fit::Warn(why) => assert!(why.contains("ns4p"), "{why}"),
            answer => panic!("an unnameable instrument holding ne5p: {answer:?}"),
        }
    }

    /// ⚠️ A refused asset is not linked however well its checksum matches: a link is
    /// what the queue and the library treat as the slot holding these bytes.
    #[test]
    fn an_asset_the_instrument_refuses_is_linked_to_nothing() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (own, crc) = program(&mut workspace, &mut log, Origin::Fresh);
        let (other, _) = stage(&mut workspace, &mut log, Origin::Fresh);

        // Both slots report the Electro 5 program's checksum, so only the family
        // separates the two assets.
        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[Some(("Africa Split", crc)), Some(("Squabble B", crc))],
        );
        device.relink(&mut workspace);
        assert!(workspace.get(own).unwrap().link.is_some());
        assert_eq!(workspace.get(other).unwrap().link, None);
    }

    /// A link is a slot of the asset's **own** folder reporting the asset's own body.
    /// A folder that reports no checksum matches nothing: the name it holds is not
    /// evidence that the bytes under it are the same.
    #[test]
    fn a_link_is_a_slot_of_its_own_folder_reporting_its_own_body() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (id, crc) = program(&mut workspace, &mut log, Origin::Fresh);

        // The same checksum in another folder is another folder's business.
        device.pretend_bodies(ObjectClass::SetList, 1, &[Some(("Sunday", crc))]);
        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[None, Some(("Africa Split", crc))],
        );
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((ObjectClass::Program, Location { bank: 6, slot: 1 }))
        );

        // The same folder, read again and reporting no checksum for what it holds. The
        // name over those bytes is not evidence that they are these.
        device.pretend_scanned(ObjectClass::Program, 7, &["", "Africa Split"]);
        let (fresh, _) = program(&mut workspace, &mut log, Origin::Fresh);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(fresh).unwrap().link,
            None,
            "a folder reporting no checksum links nothing"
        );
    }

    /// A factory sound that also sits in a slot the user filled is in two places at
    /// once, and the address the row shows is the one the asset already stood on — a
    /// write of this app's put it there. Only an asset standing nowhere is matched to
    /// the lowest of them.
    #[test]
    fn a_link_keeps_the_slot_it_has_when_several_hold_the_bytes() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (id, crc) = program(&mut workspace, &mut log, Origin::File("Bells.ne5p".into()));
        let class = ObjectClass::Program;
        let (low, high) = (Location { bank: 6, slot: 0 }, Location { bank: 6, slot: 2 });
        device.pretend_bodies(
            class,
            7,
            &[
                Some(("Circling Bells", crc)),
                None,
                Some(("Circling Bells", crc)),
            ],
        );

        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((class, low)),
            "standing nowhere, it takes the lowest address holding it"
        );

        let sent = workspace.get(id).unwrap().bytes.clone();
        workspace.landed(id, class, high, sent);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((class, high)),
            "it stands where it was written, not where else the bytes are"
        );
    }

    /// A file opened against an instrument already walked stands on its slot in the
    /// frame it arrives in. Nothing is going to read that bank again on its account, so
    /// waiting for a walk to report is waiting for something that may never happen.
    #[test]
    fn an_asset_links_as_soon_as_it_arrives() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let mut queue = Queue::default();
        let class = ObjectClass::Program;
        let (first, crc) = program(&mut workspace, &mut log, Origin::Fresh);
        device.pretend_bodies(class, 7, &[None, Some(("Circling Bells", crc))]);
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        let at = workspace.get(first).unwrap().link;
        assert_eq!(at, Some((class, Location { bank: 6, slot: 1 })));

        // A second asset of the same bytes, with no event from the instrument between.
        let (second, _) = program(&mut workspace, &mut log, Origin::Fresh);
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        assert_eq!(
            workspace.get(second).unwrap().link,
            at,
            "it links off the cache rather than off the next walk"
        );
    }

    /// Bytes with no container carry no checksum, so nothing about them can be matched
    /// to a slot however much of the instrument has been read.
    #[test]
    fn an_asset_with_no_checksum_of_its_own_links_to_nothing() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (_, crc) = program(&mut workspace, &mut log, Origin::Fresh);
        let loose = workspace.ingest(
            "notes.txt".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            b"not a Nord file at all".to_vec(),
            &mut log,
        );
        assert!(workspace.get(loose).unwrap().container.is_none());

        device.pretend_bodies(ObjectClass::Program, 7, &[Some(("Africa Split", crc))]);
        device.relink(&mut workspace);
        assert_eq!(workspace.get(loose).unwrap().link, None);
    }

    /// Three slots hold one body: the asset that came off one of them keeps pointing at
    /// that one, and the asset that came off none takes the lowest address.
    #[test]
    fn a_link_prefers_the_slot_it_came_off_and_otherwise_the_lowest_address() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let at = |slot| Location { bank: 6, slot };
        let (fresh, crc) = program(&mut workspace, &mut log, Origin::Fresh);
        let (copied, _) = program(
            &mut workspace,
            &mut log,
            Origin::Device {
                class: ObjectClass::Program,
                at: at(2),
            },
        );

        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[
                Some(("Africa Split", crc)),
                Some(("Africa 2", crc)),
                Some(("Africa 3", crc)),
            ],
        );
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(fresh).unwrap().link,
            Some((ObjectClass::Program, at(0)))
        );
        assert_eq!(
            workspace.get(copied).unwrap().link,
            Some((ObjectClass::Program, at(2)))
        );
        assert_eq!(
            also_holding(&device.state, workspace.get(fresh).unwrap()),
            2,
            "the hover has the rest to count"
        );
    }

    /// The slot an asset came off is where it stands, whatever that slot holds now. A
    /// program copied from 1:1, changed here and saved, is still the copy of 1:1 — and
    /// the two having parted is what the library says about it, not a reason to point it
    /// somewhere else.
    #[test]
    fn the_slot_an_asset_came_off_is_its_link_while_the_instrument_holds_it() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let at = Location { bank: 0, slot: 0 };
        let class = ObjectClass::Program;
        let (id, crc) = program(&mut workspace, &mut log, Origin::Device { class, at });

        // Changed here before anything was attached, and saved.
        let bytes = workspace.get(id).unwrap().bytes.clone();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, &mut log);
        workspace.mark_saved(id);
        let now = workspace.get(id).unwrap().saved.crc32.unwrap();
        assert_ne!(now, crc, "the edit moved the body");
        device.relink(&mut workspace);
        assert_eq!(workspace.get(id).unwrap().link, None, "nothing is read yet");

        // Bank 1 is read: 1:1 holds what this asset used to be, and 1:2 holds what it is
        // now. The slot it came off is still the slot it came off.
        device.pretend_bodies(
            class,
            1,
            &[Some(("Africa Split", crc)), Some(("Squabble B", now))],
        );
        device.relink(&mut workspace);
        assert_eq!(workspace.get(id).unwrap().link, Some((class, at)));

        // A slot the walk found vacant holds nothing to stand on, so the body matches.
        device.pretend_bodies(class, 1, &[None, Some(("Squabble B", now))]);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((class, Location { bank: 0, slot: 1 })),
        );
    }

    /// A class whose slots report no checksum cannot be matched by body. Settings holds
    /// one slot and that slot is the link; a sample or a piano is matched to the slot
    /// carrying its own name, and to nothing at all where no slot does.
    #[test]
    fn a_class_reporting_no_checksum_links_by_its_singleton_or_by_name() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let settings = workspace
            .create(crate::workspace::Fresh::Settings, &mut log)
            .unwrap();

        device.pretend_scanned(ObjectClass::Settings, 1, &["Settings"]);
        device.pretend_scanned(ObjectClass::Sample, 1, &["Bass Clarinet", "Rhodes"]);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(settings).unwrap().link,
            Some((ObjectClass::Settings, Location { bank: 0, slot: 0 })),
        );

        // The name is the whole of the match, verbatim and case for case.
        let bytes = workspace.get(settings).unwrap().bytes.clone();
        let by_name = |workspace: &mut Workspace, name: &str, log: &mut Log| {
            let id = workspace.ingest(name.into(), Origin::Fresh, bytes.clone(), log);
            let at = super::named(
                &device.state,
                ObjectClass::Sample,
                workspace.get(id).unwrap(),
            );
            workspace.remove(id, log);
            at
        };
        let slot = |slot| Some(Location { bank: 0, slot });
        assert_eq!(by_name(&mut workspace, "Rhodes", &mut log), slot(1));
        assert_eq!(
            by_name(&mut workspace, " Bass Clarinet ", &mut log),
            slot(0)
        );
        assert_eq!(
            by_name(&mut workspace, "rhodes", &mut log),
            None,
            "case is part of a name"
        );
        assert_eq!(by_name(&mut workspace, "Wurlitzer", &mut log), None);

        // Two slots are not a singleton, so nothing about settings is decided by one.
        device.pretend_scanned(ObjectClass::Settings, 1, &["Settings", "Settings 2"]);
        let second = workspace.ingest("Settings".into(), Origin::Fresh, bytes, &mut log);
        device.relink(&mut workspace);
        assert_eq!(workspace.get(second).unwrap().link, None);
    }

    /// A link is derived from the scan cache, so a walk makes one and the instrument
    /// going takes it away. What is on this computer is untouched either way.
    #[test]
    fn a_link_arrives_with_a_walk_and_goes_when_the_instrument_does() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let mut queue = Queue::default();
        let (id, crc) = program(&mut workspace, &mut log, Origin::Fresh);
        let class = ObjectClass::Program;

        device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 7,
            slots: vec![Some(ProgramInfo {
                location: Location { bank: 6, slot: 0 },
                body_len: 121,
                format: "ne5p".into(),
                version: 4,
                crc32: Some(crc),
                name: "Africa Split".into(),
            })],
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((class, Location { bank: 6, slot: 0 }))
        );

        device.pretend(DeviceEvent::Disconnected { lost: true });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        assert_eq!(workspace.get(id).unwrap().link, None);
        assert!(workspace.get(id).is_some(), "the asset itself stays");
    }

    /// A write is the strongest evidence there is about where an asset stands: this app
    /// put the bytes there. What the read after it reports decides the sign the row
    /// wears, and cannot decide that the asset stands nowhere.
    #[test]
    fn a_send_lands_its_asset_on_the_slot_it_wrote() {
        /// What the read after the write reports for the slot.
        enum Rescan {
            /// Nothing read the bank again, which is where a write leaves it.
            Skipped,
            Same,
            Other,
            /// A slot that reports no checksum for what it holds.
            Silent,
        }

        let class = ObjectClass::Program;
        let at = Location { bank: 4, slot: 2 };
        let sent = |rescan: Rescan| {
            let ctx = egui::Context::default();
            let mut workspace = Workspace::new(ctx.clone());
            let mut device = Device::new(ctx);
            let mut log = Log::default();
            let mut tabs = Tabs::default();
            let mut queue = Queue::default();
            let origin = Origin::File("Africa-Split.ne5p".into());
            let (id, crc) = program(&mut workspace, &mut log, origin);
            device.pretend_attached();

            let sent = workspace.get(id).expect("it is on the list").bytes.clone();
            device.pretend(DeviceEvent::Sent {
                id,
                class,
                at,
                bytes: sent,
            });
            device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

            let reported = match rescan {
                Rescan::Skipped => None,
                Rescan::Same => Some(Some(crc)),
                Rescan::Other => Some(Some(crc ^ 1)),
                Rescan::Silent => Some(None),
            };
            if let Some(crc32) = reported {
                device.pretend(DeviceEvent::BankScanned {
                    class,
                    bank: at.bank + 1,
                    slots: vec![
                        None,
                        None,
                        Some(ProgramInfo {
                            location: at,
                            body_len: 121,
                            format: "ne5p".into(),
                            version: 4,
                            crc32,
                            name: "Africa Split".into(),
                        }),
                    ],
                });
                device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
            }
            // A second frame with nothing to report: the log is not a per-frame render.
            device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
            let entity = workspace.get(id).expect("it is on the list");
            (
                entity.link,
                crate::library::keyboard_mark(entity, &device.state, &queue),
                log.transcript(),
                crc,
            )
        };
        let (good, warn) = (crate::library::Mark::Agrees, crate::library::Mark::Differs);
        let there = Some((class, at));

        let (link, mark, said, _) = sent(Rescan::Same);
        assert_eq!(link, there);
        assert_eq!(mark, Some(good));
        assert!(!said.contains("crc32"), "the two agree: {said}");

        let (link, mark, said, crc) = sent(Rescan::Other);
        assert_eq!(
            link, there,
            "it is where it was written, holding what it holds"
        );
        assert_eq!(mark, Some(warn));
        assert!(
            said.contains(&format!(
                "Programs 5:3 reports crc32 {:#010x}; “Africa-Split.ne5p” is saved as \
                 {crc:#010x}",
                crc ^ 1
            )),
            "{said}"
        );
        assert_eq!(
            said.matches("reports crc32").count(),
            1,
            "once, on the read"
        );

        let (link, mark, _, _) = sent(Rescan::Silent);
        assert_eq!(link, there);
        assert_eq!(
            mark,
            Some(good),
            "this app wrote those bytes and the slot reports as many"
        );

        let (link, _, _, _) = sent(Rescan::Skipped);
        assert_eq!(link, there, "a bank nothing has read says neither way");
    }

    /// A write takes as long as the instrument takes, and an edit made while one is in
    /// flight is on this computer alone. What landed is what the asset is saved as; what
    /// it holds now is still unsaved, and closing the tab over it would be losing it.
    #[test]
    fn a_send_settles_the_baseline_on_the_bytes_it_carried_and_not_on_a_later_edit() {
        let class = ObjectClass::Program;
        let at = Location { bank: 4, slot: 2 };
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let mut queue = Queue::default();
        let (id, _) = program(&mut workspace, &mut log, Origin::Fresh);
        device.pretend_attached();
        crate::queue::enqueue(&workspace, &mut device, &mut queue, &mut log, id, class, at);

        // What the write carries is what the asset held when it went out.
        let sent = workspace.get(id).expect("it is on the list").bytes.clone();

        let (_, edited) = crate::fields::apply(&sent, &[("center_panel.gain".into(), "96".into())])
            .expect("the registry takes the set");
        workspace.replace_bytes(id, edited.clone(), &mut log);

        device.pretend(DeviceEvent::Sent {
            id,
            class,
            at,
            bytes: sent.clone(),
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        let entity = workspace.get(id).expect("it is on the list");
        assert_eq!(entity.saved.bytes, sent, "the instrument holds these");
        assert_eq!(entity.bytes, edited);
        assert!(entity.is_unsaved(), "the edit never went anywhere");
        assert_eq!(entity.link, Some((class, at)));
        assert!(!queue.holds(id), "it landed");
    }

    /// A library id resolves to a name only where the instrument has actually said so:
    /// for the slot that was asked about, for the class asked for, and for that id.
    /// Anything else is *not asked*, which is not the same as nameless.
    #[test]
    fn a_dependency_name_answers_only_for_what_was_asked() {
        let at = Location { bank: 6, slot: 3 };
        let elsewhere = Location { bank: 0, slot: 0 };
        let detail = Detail {
            at: Some(at),
            info: None,
            asked: true,
            deps: Some(vec![Dependency {
                flag: 0,
                class: ObjectClass::Piano,
                id: 0x0102_0304,
                name: "Royal Grand 3D ".into(),
                location: None,
            }]),
        };
        let state = DeviceState {
            detail,
            ..DeviceState::default()
        };

        let piano = |slot, id| {
            state
                .dependency_name(Some((ObjectClass::Program, slot)), ObjectClass::Piano, id)
                .map(str::to_string)
        };
        assert_eq!(piano(at, 0x0102_0304).as_deref(), Some("Royal Grand 3D"));
        assert_eq!(piano(elsewhere, 0x0102_0304), None, "another slot's list");
        assert_eq!(piano(at, 0x0999_0999), None, "an id it did not report");
        assert_eq!(
            state.dependency_name(
                Some((ObjectClass::Program, at)),
                ObjectClass::Sample,
                0x0102_0304
            ),
            None,
            "a piano is not a sample"
        );
        // Nothing to ask about: a document that never came off an instrument.
        assert_eq!(state.dependency_name(None, ObjectClass::Piano, 1), None);
    }

    /// The status strip names places and things; the protocol line keeps the verbs.
    #[test]
    fn the_status_sentences_never_quote_the_protocol() {
        let cmd = DeviceCmd::Put {
            id: 1,
            class: ObjectClass::Program,
            at: Location { bank: 6, slot: 3 },
            name: "Africa Split".into(),
            bytes: Vec::new(),
        };
        let words = cmd.words();
        assert_eq!(words.doing, "Sending “Africa Split” to Programs 7:4…");
        assert_eq!(words.done, "Sent “Africa Split” to Programs 7:4");
        assert_eq!(
            words.failed,
            "Could not send “Africa Split” to Programs 7:4"
        );
    }

    /// The folder name is what the panel calls the thing, never the class number.
    #[test]
    fn a_place_reads_as_a_folder_and_a_slot() {
        let at = Location { bank: 0, slot: 0 };
        assert_eq!(place(ObjectClass::SetList, at), "Set lists 1:1");
        assert_eq!(folder(ObjectClass::Unknown(9)), "Other");
    }
}
