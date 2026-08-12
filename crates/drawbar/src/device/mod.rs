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
use nord_usb::wire::{Bank, Dependency, ProgramInfo, Status};
use nord_usb::{Location, ObjectClass};

use crate::log::Log;
use crate::strings::{folder, place, shown};
use crate::tabs::Tabs;
use crate::workspace::{Origin, Workspace};

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

/// The folders the browser shows, in the order it shows them.
pub const BROWSED: [ObjectClass; 6] = [
    ObjectClass::Program,
    ObjectClass::SetList,
    ObjectClass::Sample,
    ObjectClass::Piano,
    ObjectClass::Live,
    ObjectClass::Settings,
];

/// What the UI asks the instrument to do.
#[derive(Clone)]
pub enum DeviceCmd {
    /// One class end to end — counters and every bank — inside a single session,
    /// streaming a [`DeviceEvent::BankScanned`] as each bank lands.
    ScanClass {
        class: ObjectClass,
        /// Slots to ask each bank for, where the device will not say.
        slots: u32,
        /// Ceiling on the walk, for a class whose size neither the geometry nor the
        /// counters can divide.
        banks: u32,
    },
    /// One `INFO` per slot of a single bank. What a mutation owes: only the bank it
    /// touched can have changed.
    ScanBank {
        class: ObjectClass,
        bank: u32,
        slots: u32,
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
        /// The copy opens in a tab as a **view** of the slot rather than joining the
        /// local list, which is what a double-click asks for. Copying to this computer
        /// is the same read with this off.
        open: bool,
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
    pub log: String,
    pub doing: String,
    pub done: String,
    pub failed: String,
}

fn words(verbs: (&str, &str, &str), what: String, log: String) -> Words {
    Words {
        log,
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
            DeviceCmd::ScanClass { class, .. } => format!("scan {}", class.label()),
            DeviceCmd::ScanBank { bank, .. } => format!("scan bank {bank}"),
            DeviceCmd::SlotInfo { at, .. } => format!("info {}", shown(*at)),
            DeviceCmd::Deps { at, .. } => format!("deps {}", shown(*at)),
            DeviceCmd::Get { at, body, .. } => match body {
                true => format!("get {} (raw body)", shown(*at)),
                false => format!("get {}", shown(*at)),
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
        let log = self.label();
        match self {
            DeviceCmd::ScanClass { class, .. } => words(READING, folder(*class).to_string(), log),
            DeviceCmd::ScanBank { class, bank, .. } => {
                words(READING, format!("{} — bank {bank}", folder(*class)), log)
            }
            DeviceCmd::SlotInfo { class, at } => words(READING, place(*class, *at), log),
            DeviceCmd::Deps { class, at } => {
                words(READING, format!("what {} needs", place(*class, *at)), log)
            }
            DeviceCmd::Get { class, at, .. } => words(
                COPYING,
                format!("{} to this computer", place(*class, *at)),
                log,
            ),
            DeviceCmd::Put {
                class, at, name, ..
            } => words(
                ("Sending", "Sent", "send"),
                format!("“{name}” to {}", place(*class, *at)),
                log,
            ),
            DeviceCmd::SendAll { class, items } => words(
                ("Sending", "Sent", "send"),
                match items.len() {
                    1 => format!("1 sound to {}", folder(*class)),
                    n => format!("{n} sounds to {}", folder(*class)),
                },
                log,
            ),
            DeviceCmd::Move { class, from, to } => words(
                ("Moving", "Moved", "move"),
                format!("{} to {}", place(*class, *from), place(*class, *to)),
                log,
            ),
            DeviceCmd::Duplicate { class, from, to } => words(
                COPYING,
                format!("{} to {}", place(*class, *from), place(*class, *to)),
                log,
            ),
            DeviceCmd::Delete { class, at } => {
                words(("Deleting", "Deleted", "delete"), place(*class, *at), log)
            }
            DeviceCmd::Rename { class, at, name } => words(
                ("Renaming", "Renamed", "rename"),
                format!("{} to “{name}”", place(*class, *at)),
                log,
            ),
            DeviceCmd::Select { class, at } => words(
                ("Loading", "Loaded", "load"),
                format!("{} on the instrument", place(*class, *at)),
                log,
            ),
            DeviceCmd::Disconnect => words(
                ("Releasing", "Released", "release"),
                "the instrument".into(),
                log,
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
    /// A class's own counters, read at the head of its walk.
    ClassStatus {
        class: ObjectClass,
        status: Status,
        /// Banks to expect: the device's own count where it gave one, otherwise what the
        /// counters divide into.
        banks: Option<u32>,
    },
    /// The device's own division of a class into banks, read at the head of its walk.
    Geometry {
        class: ObjectClass,
        banks: Vec<Bank>,
    },
    /// The slot the instrument's panel has loaded in a class.
    Focus {
        class: ObjectClass,
        at: Location,
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
        /// The copy is a view of the slot rather than a new row on this computer.
        open: bool,
    },
    /// One object landed on the instrument, so it is no longer owed. Every write path
    /// raises one — a lone put as much as a batch.
    Sent {
        id: u64,
        class: ObjectClass,
        at: Location,
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
    focus: HashMap<u32, Location>,
    /// The device's own banks, per class: their names and their capacities.
    geometry: HashMap<u32, Vec<Bank>>,
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

    /// How many slots the device says a bank holds. `None` where it did not say, or
    /// where it answered the sentinel the unbounded library views carry.
    pub fn slots_in(&self, class: ObjectClass, bank: u32) -> Option<u32> {
        self.geometry
            .get(&class.to_raw())?
            .iter()
            .find(|held| held.index + 1 == bank)
            .filter(|held| held.is_bounded())
            .map(|held| held.slots)
    }

    /// The slot the panel had loaded in a class when it was last read.
    ///
    /// ⚠️ Read once per walk, so a selection made on the panel afterwards is not in here
    /// until the class is read again.
    pub fn focused(&self, class: ObjectClass) -> Option<Location> {
        self.focus.get(&class.to_raw()).copied()
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

    /// The first slot of `class` known to be vacant — where a duplicate lands when the
    /// user did not drag it anywhere.
    pub fn first_free(&self, class: ObjectClass) -> Option<Location> {
        self.banks_of(class).into_iter().find_map(|bank| {
            let slots = self.bank(class, bank)?;
            let slot = slots.iter().position(Option::is_none)?;
            Some(Location::from_user(bank, slot as u32 + 1))
        })
    }

    /// Drop one bank's cached names, because something just changed them.
    fn forget_bank(&mut self, class: ObjectClass, bank: u32) {
        self.banks.remove(&(class.to_raw(), bank));
    }

    fn forget_everything(&mut self) {
        self.banks.clear();
        self.focus.clear();
        self.geometry.clear();
        self.inventory.clear();
        self.detail = Detail::default();
        self.scan.clear();
        self.selected.clear();
    }
}

/// ⚠️ A ceiling on a walk whose end the device alone decides, used only where the
/// device would not report its own geometry. The cap is here so an instrument that
/// never answers "out of range" cannot walk forever.
pub const MAX_BANKS: u32 = 32;

/// Slots per bank, per class, where the device will not say.
///
/// ⚠️ These are the Electro 5's divisions — 50 programs and 50 set lists to a bank,
/// three live slots, one settings singleton — inferred from the corpus and the panel's
/// own labels, not read off the device. The walk asks the device for its real banks
/// first and only falls back here; either way it is not held to the number, because the
/// device answers status 3 past the end of its slot space.
pub fn slots_per_bank(class: ObjectClass) -> u32 {
    match class {
        ObjectClass::Live => 3,
        ObjectClass::Settings => 1,
        _ => 50,
    }
}

/// How full a folder is, in the width its own heading has for it: `312/400`, or a bare
/// count for a class whose items differ in size and divide into no slots.
///
/// Slot counts only: the inventory also reports opaque block totals, and a class whose
/// items differ in size (pianos, samples) cannot be divided into slots at all.
pub fn occupancy(class: ObjectClass, inventory: &[Status]) -> Option<String> {
    let status = inventory.iter().find(|status| status.class == class)?;
    Some(match status.slots() {
        Some(slots) => format!("{}/{slots}", status.count),
        None => format!("{} items", status.count),
    })
}

/// Whether the browser offers to change a class at all.
///
/// ⚠️ A piano is a multi-megabyte library that the instrument builds its own index
/// over; the browser lists what is installed and offers nothing that would move it.
pub fn read_only(class: ObjectClass) -> bool {
    matches!(class, ObjectClass::Piano)
}

/// Whether this app will write into a class at all.
pub fn sendable(class: ObjectClass) -> bool {
    put_refusal(class).is_none() && !read_only(class)
}

/// Whether a class accepts a write.
///
/// ⚠️ A write is a delete followed by a write, and whether the live buffer or the
/// settings singleton survives a delete of its own class is unconfirmed on hardware.
/// Until it is, an edit of either stops at a file.
pub fn put_refusal(class: ObjectClass) -> Option<String> {
    match class {
        ObjectClass::Live | ObjectClass::Settings => Some(format!(
            "writing {} back over USB is unproven on hardware; save the object as a \
             file instead",
            class.label(),
        )),
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

    /// Read the whole instrument again: every class, and with each one its counters, its
    /// geometry and the slot the panel has loaded.
    ///
    /// One walk per class, which is the same thing attaching does — a class's counters,
    /// banks and focus are all read at the head of its own session, so there is nothing
    /// else to ask for.
    pub fn resync(&mut self) {
        for class in BROWSED {
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
        self.dispatch(DeviceCmd::ScanClass {
            class,
            slots: slots_per_bank(class),
            banks: MAX_BANKS,
        });
    }

    fn dispatch(&mut self, cmd: DeviceCmd) {
        // A mutation makes the names the browser is showing wrong, so they are dropped
        // now rather than quietly kept — a stale name is what a confirmation would go on
        // to quote back at the operator. Only the banks it touches can have changed, so
        // only those are dropped and read again.
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
                banks.dedup();
                banks
            }
            _ => Vec::new(),
        };
        for (class, bank) in &self.rescan {
            self.state.forget_bank(*class, *bank);
        }
        // Writing into the slot the instrument has loaded leaves the panel playing the
        // buffer it read before the write; only a fresh SELECT reloads it. Confirmed on
        // hardware.
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

    /// Fill in a bank as though the instrument had answered, for a headless render.
    #[cfg(test)]
    pub fn pretend_scanned(&mut self, class: ObjectClass, bank: u32, names: &[&str]) {
        use nord_usb::wire::ProgramInfo;

        self.state.connection = Connection::Connected(DeviceCard {
            build: Some(7),
            firmware: Some(204),
            interface: Some(3),
            kind: Some(1),
            manufacturer: Some("Clavia DMI AB".into()),
            max_transfer: Some(4096),
            product: "Electro 5".into(),
            product_id: 0,
            serial: None,
            vendor_id: 0x0ffc,
        });
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
        self.state.focus.insert(class.to_raw(), at);
    }

    /// Drain the worker's events into the cache, the local list and the tabs. Call once
    /// a frame.
    pub fn poll(&mut self, log: &mut Log, workspace: &mut Workspace, tabs: &mut Tabs) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                DeviceEvent::Connected(card) => {
                    log.info(format!(
                        "connected: {} ({:04x}:{:04x})",
                        card.product, card.vendor_id, card.product_id
                    ));
                    log.say(format!("{} is attached.", card.product));
                    self.state.connection = Connection::Connected(card);
                    self.state.forget_everything();
                    self.pending.clear();
                    // One walk per class, each its own session. Nothing is asked ahead
                    // of them: a class's counters are read at the head of its own walk.
                    self.resync();
                }
                DeviceEvent::ConnectFailed(why) => {
                    log.error(why);
                    log.trouble("No instrument could be opened.");
                    self.state.connection = Connection::Disconnected;
                }
                // ⚠️ Nothing on this computer is touched. What was waiting to be sent is
                // still waiting: the instrument coming back is the point of keeping it,
                // and a badge cleared here would be an edit silently given up on.
                DeviceEvent::Disconnected { lost } => {
                    match lost {
                        true => log.trouble("The instrument went away — reconnect when it's back."),
                        false => log.say("The instrument was released."),
                    }
                    self.state.connection = Connection::Disconnected;
                    self.state.in_flight = None;
                    self.state.forget_everything();
                    self.pending.clear();
                    self.reading = None;
                    self.rescan.clear();
                    self.reselect.clear();
                }
                DeviceEvent::Started(what) => log.info(what),
                DeviceEvent::Finished => {
                    if let Some(class) = self.reading.take() {
                        self.state.scan.finished(class);
                    }
                    // The panel is still playing what it read before the write, so it is
                    // asked to load the slot again. `select` is read-only.
                    for (class, at) in std::mem::take(&mut self.reselect) {
                        self.pending.push_back(DeviceCmd::Select { class, at });
                    }
                    for (class, bank) in std::mem::take(&mut self.rescan) {
                        let slots = self
                            .state
                            .slots_in(class, bank)
                            .unwrap_or_else(|| slots_per_bank(class));
                        self.pending
                            .push_back(DeviceCmd::ScanBank { class, bank, slots });
                    }
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
                DeviceEvent::Got {
                    name,
                    origin,
                    bytes,
                    open,
                } => {
                    // A double-click opens a view: a working copy with a tab and a
                    // document, which nothing lists and which goes when the tab does.
                    // Copying is the same read, kept.
                    let id = match open {
                        true => workspace.view(name, origin, bytes, log),
                        false => workspace.ingest(name, origin, bytes, log),
                    };
                    if open {
                        tabs.open(id, workspace);
                    }
                }
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
                // It landed, so it is no longer owed. Only that object: the rest of a
                // batch is still waiting on its own write.
                DeviceEvent::Sent { id, .. } => workspace.mark_pending(id, false),
                DeviceEvent::Note(text) => log.info(text),
                DeviceEvent::OpOk(text) => {
                    log.info(text);
                    if let Some(words) = &self.state.in_flight {
                        log.say(format!("{}.", words.done));
                    }
                }
                DeviceEvent::OpFailed(text) => {
                    log.error(text);
                    match &self.state.in_flight {
                        Some(words) => {
                            let failed = words.failed.clone();
                            log.trouble(format!("{failed}. The details are below."));
                        }
                        None => log.trouble("Something went wrong. The details are below."),
                    }
                }
                // Something changed outside our session, so every cached name may now be
                // wrong. They are dropped and read again rather than flagged: a name the
                // user is about to drag is one a dialog would go on to quote back.
                DeviceEvent::InstrumentChanged => {
                    log.warn("the instrument changed under us — every cached name is dropped");
                    log.say("Something changed on the instrument. Reading it again…");
                    self.resync();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two classes whose write path is unproven must refuse by name, and the rest
    /// must not.
    #[test]
    fn only_live_and_settings_refuse_a_put() {
        for class in [ObjectClass::Live, ObjectClass::Settings] {
            let why = put_refusal(class).expect("must refuse");
            assert!(why.contains("unproven on hardware"), "{why}");
        }
        for class in ObjectClass::INVENTORY {
            assert!(put_refusal(class).is_none(), "{}", class.label());
        }
    }

    /// Pianos are listed and never altered.
    #[test]
    fn only_pianos_are_read_only() {
        assert!(read_only(ObjectClass::Piano));
        for class in BROWSED.iter().filter(|c| **c != ObjectClass::Piano) {
            assert!(!read_only(*class), "{}", folder(*class));
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
        workspace.mark_pending(landed, true);
        workspace.mark_pending(still_owed, true);

        device.pretend(DeviceEvent::Sent {
            id: landed,
            class: ObjectClass::Program,
            at: at(3),
        });
        device.poll(&mut log, &mut workspace, &mut tabs);

        assert!(!workspace.get(landed).unwrap().pending, "it was written");
        assert!(workspace.get(still_owed).unwrap().pending, "still waiting");
        let owed: Vec<u64> = workspace.pending().iter().map(|e| e.id).collect();
        assert_eq!(owed, vec![still_owed]);
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
        assert_eq!(words.log, "put Africa Split -> 7:4");
    }

    /// The folder name is what the panel calls the thing, never the class number.
    #[test]
    fn a_place_reads_as_a_folder_and_a_slot() {
        let at = Location { bank: 0, slot: 0 };
        assert_eq!(place(ObjectClass::SetList, at), "Set lists 1:1");
        assert_eq!(folder(ObjectClass::Unknown(9)), "Other");
    }
}
