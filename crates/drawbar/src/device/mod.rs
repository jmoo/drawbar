//! The instrument: what the app knows about the attached Nord, and the channel that
//! talks to it. The UI never touches a transport. It sends a [`DeviceCmd`] to a worker that
//! owns one and reads [`DeviceEvent`]s back, one operation at a time, since a transaction
//! is not re-entrant. What the user asks for waits in `pending` and runs ahead of the
//! background walk in [`Scan`].

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui;
use nord_format::accept::{Acceptance, Family};
use nord_usb::op::Received;
use nord_usb::wire::{AllocationUnit, Bank, Dependency, ProgramInfo, Status};
use nord_usb::{Location, ObjectClass};

use crate::link::{Held, Keyboard, Print, Spot, Standing, There};
use crate::log::Log;
use crate::ondisk::OnDisk;
use crate::queue::Queue;
use crate::strings::{counted, folder, place, shown};
use crate::tabs::Tabs;
use crate::workspace::{LocalEntity, Moved, Origin, Workspace};

#[cfg(all(test, unix))]
mod hardware;
#[cfg(all(test, unix))]
mod links_on_hardware;
mod scan;
mod scratch;
#[cfg(not(target_arch = "wasm32"))]
pub use scratch::shelf;
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
use scratch::Scratch;
pub use worker::{Emit, Flow};

/// The number the panel labels a zero-indexed bank with.
///
/// `None` for an index of `u32::MAX`, which is refused instead of wrapping onto another
/// bank's cache.
pub(crate) fn user_bank(index: u32) -> Option<u32> {
    index.checked_add(1)
}

/// One row of the instrument's partition table: a class it has, under the device's name
/// for it. A class this app has no name for arrives as [`ObjectClass::Unknown`] and is
/// shown under `name`.
pub struct Partition {
    pub class: ObjectClass,
    /// The device's own word: `Piano`, `Samp Lib`, `Program`, `Set List`, …
    pub name: String,
    /// Whether this is the `(Native)` view of a library: a second view of a pool the table
    /// already lists under its user partition, not a separate folder.
    pub native: bool,
    /// `None` where the partition reports no usable unit.
    pub unit: Option<AllocationUnit>,
}

/// What the UI asks the instrument to do.
#[derive(Clone)]
pub enum DeviceCmd {
    /// One class end to end (counters and every bank) inside a single session, streaming
    /// a [`DeviceEvent::BankScanned`] as each bank arrives.
    ScanClass {
        class: ObjectClass,
    },
    /// One `INFO` per slot of a single bank. A mutation rescans only the bank it touched,
    /// since no other can have changed.
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
        why: Purpose,
    },
    /// Copy every one of `slots` to this computer inside a single session. An object too
    /// large to hold streams to a file, reported as [`DeviceEvent::Fetched`]; the rest
    /// come back as [`DeviceEvent::Got`]. A failure stops the batch there.
    CopyAll {
        class: ObjectClass,
        slots: Vec<Location>,
    },
    /// Walk set lists and programs to everything a bundle of them holds, list each one's
    /// dependencies, report those slots and `also` in [`DeviceEvent::Gathered`], and copy
    /// each once to this computer, one session per class.
    Gather {
        roots: Vec<(ObjectClass, Location)>,
        also: Vec<(ObjectClass, Location)>,
        /// Which export this is for, echoed in [`DeviceEvent::Gathered`].
        request: u64,
    },
    Put {
        /// The asset on this computer this came from. The [`DeviceEvent::Sent`] this
        /// raises names it, so a lone put settles the queue as a batch does.
        id: u64,
        class: ObjectClass,
        at: Location,
        name: String,
        payload: Payload,
        expect: Expect,
    },
    /// Every queued object of one class, written inside a single session.
    ///
    /// Each item runs the same read-back, delete, write and restore flow as a lone
    /// [`DeviceCmd::Put`]; only the session is shared, and a restore runs in one of its
    /// own after it. A refusal stops the batch there; an item whose slot is not as it
    /// expects is skipped.
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
    /// Select again whichever of `written` the panel is on when this runs, so it plays
    /// what was just written there.
    ///
    /// ⚠️ The panel's position is read when this runs. Selecting without reading would move
    /// a panel turned since the last walk and discard the edits made there.
    Reload {
        class: ObjectClass,
        written: Vec<Location>,
    },
    Disconnect,
}

/// What a read of a slot is for, which decides where its bytes go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// A view of the slot, opened in a tab without joining the local list. A double-click
    /// asks for this.
    View,
    /// A copy on this computer: a row in the list like any other.
    Copy,
    /// The occupant of a slot something is queued for, to be compared with what is
    /// waiting.
    ///
    /// ⚠️ These bytes never reach the workspace: a copy nobody asked for would be a row
    /// nobody can account for.
    Compare,
    /// The slot's content, to be saved as the file linked to it. See [`crate::bring`].
    Pull,
    /// The slot's content, for the question of which of it and its file is current.
    Choose,
}

/// One object waiting to go back to the instrument.
#[derive(Clone)]
pub struct Outgoing {
    pub id: u64,
    pub at: Location,
    pub name: String,
    pub payload: Payload,
    pub expect: Expect,
}

/// What a write expects its slot to hold, as the user was shown it when they sent it.
/// The worker reads the slot just before the write and writes nothing over anything
/// else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Expect {
    Vacant,
    /// ⚠️ `crc` is `None` where the class or the read that found the occupant reports no
    /// checksum; the occupant is then known by its name and body length.
    Holds {
        crc: Option<u32>,
        len: u32,
        name: String,
    },
}

impl Expect {
    /// What a read reporting `info` found, `None` for an empty slot.
    pub fn of(info: Option<&ProgramInfo>) -> Expect {
        match info {
            None => Expect::Vacant,
            Some(info) => Expect::Holds {
                crc: info.crc32,
                len: info.body_len,
                name: info.name.trim().to_string(),
            },
        }
    }

    /// Whether a slot found holding `now` holds what this expects: the same checksum and
    /// length, or where either side has no checksum, the same name and length.
    pub fn matches(&self, now: &Expect) -> bool {
        match (self, now) {
            (Expect::Vacant, Expect::Vacant) => true,
            (
                Expect::Holds { crc, len, name },
                Expect::Holds {
                    crc: c,
                    len: l,
                    name: n,
                },
            ) => {
                let same = match (crc, c) {
                    (Some(crc), Some(now)) => crc == now,
                    _ => name == n,
                };
                len == l && same
            }
            (Expect::Vacant, Expect::Holds { .. }) | (Expect::Holds { .. }, Expect::Vacant) => {
                false
            }
        }
    }
}

/// What a write sends: the bytes an asset holds, or the file it rests in, which the
/// write reads a transfer chunk at a time and never whole.
#[derive(Clone)]
pub enum Payload {
    Bytes(Vec<u8>),
    File {
        file: Arc<OnDisk>,
        /// What a slot holding the file reports, from the check that cleared it to be
        /// sent.
        print: Print,
    },
}

impl Payload {
    /// What sending `entity` writes, or why the instrument must not be sent it. One resting
    /// in its file under an edit is sent once the edit is saved there.
    pub fn of(entity: &LocalEntity) -> Result<Payload, String> {
        entity.sendable()?;
        match (entity.rests(), &entity.container) {
            (None, _) => Ok(Payload::Bytes(entity.bytes.to_vec())),
            (Some(_), _) if entity.is_unsaved() => {
                Err("it holds an edit not yet saved into its file".into())
            }
            (Some(file), Some(container)) => Ok(Payload::File {
                file: file.clone(),
                print: container.print().ok_or("its body is too long for a slot")?,
            }),
            (Some(_), None) => Err("its checksum has not been checked".into()),
        }
    }

    /// What a slot holding what this writes reports.
    pub fn print(&self) -> Option<Print> {
        match self {
            Payload::Bytes(bytes) => crate::workspace::print_of(bytes),
            Payload::File { print, .. } => Some(*print),
        }
    }
}

/// How the status strip describes one operation.
///
/// The activity log keeps the protocol line; the status strip gets sentences that name
/// places and things as the panel does, with no class numbers or wire verbs.
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
            DeviceCmd::Get { at, why, .. } => match why {
                Purpose::Compare | Purpose::Choose => format!("get {} (to compare)", shown(*at)),
                Purpose::Pull => format!("get {} (to bring in)", shown(*at)),
                Purpose::Copy | Purpose::View => format!("get {}", shown(*at)),
            },
            DeviceCmd::CopyAll { class, slots } => {
                format!("get {} objects <- {}", slots.len(), class.label())
            }
            DeviceCmd::Gather { roots, also, .. } => {
                format!("gather a bundle of {} objects", roots.len() + also.len())
            }
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
            DeviceCmd::Reload { class, .. } => format!("reload the {} panel", class.label()),
            DeviceCmd::Disconnect => "disconnect".into(),
        }
    }

    /// What telemetry calls this operation. `None` for a release, which asks nothing of
    /// the instrument.
    pub fn metric(&self) -> Option<crate::telemetry::Op> {
        let (name, class, asked) = match self {
            DeviceCmd::ScanClass { class } => ("scan-class", class, false),
            DeviceCmd::ScanBank { class, .. } => ("scan-bank", class, false),
            DeviceCmd::SlotInfo { class, .. } => ("info", class, false),
            DeviceCmd::Deps { class, .. } => ("deps", class, false),
            DeviceCmd::Get { class, why, .. } => (
                "get",
                class,
                !matches!(why, Purpose::Compare | Purpose::Choose),
            ),
            DeviceCmd::Put { class, .. } => ("put", class, true),
            DeviceCmd::SendAll { class, .. } => ("send-all", class, true),
            DeviceCmd::Move { class, .. } => ("move", class, true),
            DeviceCmd::Duplicate { class, .. } => ("duplicate", class, true),
            DeviceCmd::Delete { class, .. } => ("delete", class, true),
            DeviceCmd::Rename { class, .. } => ("rename", class, true),
            DeviceCmd::Select { class, .. } => ("select", class, false),
            DeviceCmd::Reload { class, .. } => ("reload", class, false),
            DeviceCmd::CopyAll { class, .. } => ("copy-all", class, true),
            // A bundle's closure crosses classes, so the row names none.
            DeviceCmd::Gather { .. } => {
                return Some(crate::telemetry::Op {
                    name: "gather",
                    class: None,
                    asked: true,
                })
            }
            DeviceCmd::Disconnect => return None,
        };
        Some(crate::telemetry::Op {
            name,
            class: Some(*class),
            asked,
        })
    }

    /// The plain sentences the status strip shows for this operation.
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
                why: Purpose::Compare | Purpose::Choose,
            } => words(READING, format!("what is in {}", place(*class, *at))),
            DeviceCmd::Get {
                class,
                at,
                why: Purpose::Pull,
            } => words(
                ("Bringing in", "Brought in", "bring in"),
                place(*class, *at),
            ),
            DeviceCmd::Get { class, at, .. } => {
                words(COPYING, format!("{} to this computer", place(*class, *at)))
            }
            DeviceCmd::Gather { roots, also, .. } => words(
                COPYING,
                format!(
                    "{} and what they play to this computer",
                    counted(roots.len() + also.len(), "sound", "sounds")
                ),
            ),
            DeviceCmd::CopyAll { class, slots } => words(
                COPYING,
                format!(
                    "{} from {} to this computer",
                    counted(slots.len(), "sound", "sounds"),
                    folder(*class)
                ),
            ),
            DeviceCmd::Put {
                class, at, name, ..
            } => words(
                ("Sending", "Sent", "send"),
                format!("“{name}” to {}", place(*class, *at)),
            ),
            DeviceCmd::SendAll { class, items } => words(
                ("Sending", "Sent", "send"),
                format!(
                    "{} to {}",
                    counted(items.len(), "sound", "sounds"),
                    folder(*class)
                ),
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
            DeviceCmd::Reload { .. } => words(
                ("Reloading", "Reloaded", "reload"),
                "what the panel plays".into(),
            ),
            DeviceCmd::Disconnect => words(
                ("Releasing", "Released", "release"),
                "the instrument".into(),
            ),
        }
    }
}

/// An object read off the instrument into a file of drawbar's own, to be copied into the
/// library, after which the file goes.
#[derive(Debug, PartialEq, Eq)]
pub struct Fetched {
    pub class: ObjectClass,
    pub at: Location,
    /// The name its slot reports.
    pub name: String,
    /// Its format's tag, which names its file's extension.
    pub tag: String,
    pub file: crate::store::Outside,
    pub len: u64,
    /// What its slot reported, as the read took it.
    pub print: Print,
}

/// What the worker reports back.
pub enum DeviceEvent {
    Connected(DeviceCard),
    ConnectFailed(String),
    Disconnected {
        /// The connection was lost to a cable or transport failure, not released on
        /// request.
        lost: bool,
    },
    Started(String),
    Finished,
    /// The instrument's partition table, in table order: the classes it has, which nothing
    /// else knows before it arrives. Read once per connection.
    Partitions(Vec<Partition>),
    /// A class's counters, read at the start of its walk and with each bank rescan.
    ClassStatus {
        class: ObjectClass,
        status: Status,
        /// Banks a starting walk will deliver, as the instrument's own bank list divides
        /// the class; `None` from a rescan, which starts no walk.
        banks: Option<u32>,
    },
    /// The device's division of a class into banks, read at the start of its walk.
    Geometry {
        class: ObjectClass,
        banks: Vec<Bank>,
    },
    /// The slot the panel has loaded in a class, from a `FOCUS` read or from the select
    /// that put it there; `None` when focus is supported but nothing is loaded. A class
    /// whose `FOCUS` answers `0x15` (focus n/a) sends none from a read.
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
    /// An object of a [`DeviceCmd::CopyAll`] too large to hold, read into a file.
    Fetched(Fetched),
    /// The slots a [`DeviceCmd::Gather`] found and is copying, and each piano or sample
    /// it needed and found in no one slot.
    Gathered {
        request: u64,
        slots: Vec<(ObjectClass, Location)>,
        unfound: Vec<String>,
    },
    /// A read found the slot empty. This is the instrument's answer, not a fault, and it
    /// settles a slot no walk has reached.
    Vacant {
        class: ObjectClass,
        at: Location,
        why: Purpose,
    },
    /// One object landed on the instrument, so it is no longer owed. Every write path
    /// raises one, a lone put as well as a batch.
    Sent {
        id: u64,
        class: ObjectClass,
        at: Location,
        /// The name the write was given, which names the slot without its format tag.
        name: String,
        /// What the write carried: what the slot now holds and what the asset is saved as
        /// from here on (see [`Workspace::landed`] and [`Workspace::landed_file`]). The
        /// asset may hold something else by now, since a write takes as long as the
        /// instrument takes.
        sent: Payload,
    },
    /// A write found its slot holding something other than it expected, and wrote
    /// nothing. `holds` is what the slot holds now, `None` where it is empty.
    Changed {
        id: u64,
        class: ObjectClass,
        at: Location,
        holds: Option<ProgramInfo>,
    },
    /// A slot's former contents, which a failed write and a failed restore, or a delete
    /// that may have landed, left in the file they were read into.
    Kept {
        at: Location,
        /// Where the file is, in words a person can follow to it.
        place: String,
    },
    /// The occupant of a piano or sample slot something is queued for, read through its
    /// checksum and not kept, since the instrument reports no checksum for those slots.
    Summed {
        class: ObjectClass,
        at: Location,
        received: Received,
    },
    Note(String),
    OpOk(String),
    OpFailed(String),
    InstrumentChanged,
}

/// The attached instrument, from its USB descriptors, which answer before any transaction
/// opens.
///
/// `firmware`, `build`, `kind` and `max_transfer` come from
/// [`Identity`](nord_usb::transport::usb::Identity), which only the desktop transport
/// reads, so the browser build leaves them `None`.
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
    /// Reported at vendor request `0x00`. It reads as a small constant of unknown meaning,
    /// so it is shown as a raw number.
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
    /// The slot it answers for, by class and address.
    ///
    /// ⚠️ Both are needed: every class uses the same bank and slot numbers (a program and
    /// a sample can both sit at 1:1), so only the class tells the two answers apart.
    pub at: Option<(ObjectClass, Location)>,
    /// What the last `info` reported, shaped as [`DeviceState::slot`] shapes a scanned
    /// slot: `Some(None)` is a slot answered empty, and `None` is one never asked about.
    pub info: Option<Option<ProgramInfo>>,
    pub deps: Option<Vec<Dependency>>,
}

/// The UI's cache of the instrument. Nothing here is authoritative: it is what the device
/// last said, and [`Scan::read_at`] says when.
#[derive(Default)]
pub struct DeviceState {
    pub connection: Connection,
    /// The operation currently running, if any. One at a time.
    pub in_flight: Option<Words>,
    pub inventory: Vec<Status>,
    /// The background read of every class.
    pub scan: Scan,
    /// The slot the panel has loaded in a class, as the instrument last reported it.
    focus: HashMap<ObjectClass, Option<Location>>,
    /// The device's banks per class, with their names and capacities.
    geometry: HashMap<ObjectClass, Vec<Bank>>,
    /// The instrument's partition table, in table order. Empty until it is read, which
    /// means unknown, not an instrument with no folders.
    partitions: Vec<Partition>,
    banks: HashMap<(ObjectClass, u32), Vec<Option<ProgramInfo>>>,
    /// What each set list or program the instrument has listed dependencies for needs,
    /// by class and slot. Cleared when the instrument reports a change or disconnects.
    needs: HashMap<(ObjectClass, u32, u32), Vec<nord_format::bundle::Key>>,
    pub detail: Detail,
    /// The keyboard the card last attached names. See [`DeviceState::keyboard`].
    keyboard: Option<Keyboard>,
}

impl DeviceCard {
    /// What tells this instrument from others: its serial number, or its product name
    /// where it reports none.
    pub fn instrument(&self) -> String {
        self.serial.clone().unwrap_or_else(|| self.product.clone())
    }
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

    /// The keyboard attached, as a library knows one.
    pub fn keyboard(&self) -> Option<&Keyboard> {
        self.keyboard.as_ref().filter(|_| self.connected())
    }

    fn attach(&mut self, card: DeviceCard) {
        self.keyboard = Some(Keyboard {
            model: card.product.clone(),
            serial: card.serial.clone(),
        });
        self.connection = Connection::Connected(card);
    }

    /// The firmware version as the panel writes it (`2.04`), where the transport could ask
    /// for it.
    pub fn firmware(&self) -> Option<String> {
        // 204/100 = 2 and 204%100 = 04, and the panel reads 2.04.
        self.card()?
            .firmware
            .map(|held| format!("{}.{:02}", held / 100, held % 100))
    }

    /// What the instrument calls a bank, by the number the panel labels it with.
    ///
    /// For pianos these are the panel's categories, such as `Grand` and `Upright`.
    pub fn bank_name(&self, class: ObjectClass, bank: u32) -> Option<&str> {
        let name = self
            .geometry
            .get(&class)?
            .iter()
            .find(|held| user_bank(held.index) == Some(bank))?
            .name
            .trim();
        (!name.is_empty()).then_some(name)
    }

    /// The slot the panel has loaded in a class, as the instrument last reported it: at the
    /// start of a walk, and after a select or reload this app asked for.
    ///
    /// ⚠️ A selection made on the panel itself is not in here until one of those.
    pub fn focused(&self, class: ObjectClass) -> Option<Location> {
        self.focus.get(&class).copied().flatten()
    }

    /// Whether the panel loads slots of this class, as far as its reads have said.
    fn has_focus(&self, class: ObjectClass) -> bool {
        self.focus.contains_key(&class)
    }

    /// Record what a slot said it depends on. The list belongs to that slot.
    fn depends(&mut self, class: ObjectClass, at: Location, deps: Vec<Dependency>) {
        if self.detail.at != Some((class, at)) {
            self.detail = Detail {
                at: Some((class, at)),
                ..Detail::default()
            };
        }
        let key = (class, at.bank, at.slot);
        self.needs.insert(key, nord_usb::bundle::needs(&deps));
        self.detail.deps = Some(deps);
    }

    /// What the set list or program in a slot needs, as the instrument last listed it.
    pub fn needs_of(
        &self,
        class: ObjectClass,
        at: Location,
    ) -> Option<&[nord_format::bundle::Key]> {
        let key = (class, at.bank, at.slot);
        self.needs.get(&key).map(Vec::as_slice)
    }

    /// The Pianos bank a program's Type dial picks by its stored value, which counts the
    /// banks from 0 in the instrument's order. Confirmed on hardware.
    pub fn pianos_of(&self, category: u8) -> Option<&[Option<ProgramInfo>]> {
        self.bank(ObjectClass::Piano, u32::from(category) + 1)
    }

    /// The piano a program's Type and Model dials pick, by the name the scan read there:
    /// the Model dial counts the Type's bank in slot order. Confirmed on hardware.
    pub fn piano_at(&self, category: u8, model: usize) -> Option<&str> {
        let piano = self.pianos_of(category)?.get(model)?.as_ref()?;
        Some(piano.name.trim())
    }

    /// Whether every bank the class declares has been read, so a slot missing from the
    /// cache is missing from the instrument.
    pub fn read_whole(&self, class: ObjectClass) -> bool {
        self.geometry.get(&class).is_some_and(|banks| {
            !banks.is_empty()
                && banks.iter().all(|bank| {
                    user_bank(bank.index).is_some_and(|n| self.banks.contains_key(&(class, n)))
                })
        })
    }

    /// A scanned bank's slots, or `None` if it has not been scanned.
    pub fn bank(&self, class: ObjectClass, bank: u32) -> Option<&[Option<ProgramInfo>]> {
        self.banks.get(&(class, bank)).map(Vec::as_slice)
    }

    /// The classes the instrument declares, in its own table order.
    ///
    /// These are the classes the browser, the switcher and a resync walk. Empty until the
    /// partition table has been read. The `(Native)` rows are left out: each is a second
    /// view of a library the table already lists under its user partition.
    pub fn classes(&self) -> Vec<ObjectClass> {
        self.partitions
            .iter()
            .filter(|row| !row.native)
            .map(|row| row.class)
            .collect()
    }

    /// What the browser calls a class's folder: the panel's word for a class this app
    /// names, and otherwise the name the instrument's partition table gave it.
    pub fn folder_name(&self, class: ObjectClass) -> &str {
        let ObjectClass::Unknown(_) = class else {
            return folder(class);
        };
        self.partition(class)
            .map(|row| row.name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| folder(class))
    }

    /// The size of one unit of what `STATUS` counts, for this class's partition.
    ///
    /// ⚠️ `None` means not read yet, not byte-granular: a slot-addressed partition reports
    /// a unit of 1 like any other unit.
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
    /// ⚠️ The device's division of the class, read at the start of its walk, not the banks
    /// scanned so far ([`DeviceState::banks_of`]). Zero until that geometry arrives.
    pub fn banks(&self, class: ObjectClass) -> usize {
        self.geometry.get(&class).map_or(0, Vec::len)
    }

    /// The banks of a class that have been read, in order.
    pub fn banks_of(&self, class: ObjectClass) -> Vec<u32> {
        let mut banks: Vec<u32> = self
            .banks
            .keys()
            .filter(|(raw, _)| *raw == class)
            .map(|(_, bank)| *bank)
            .collect();
        banks.sort_unstable();
        banks
    }

    /// What a slot holds, from the scan cache. `Some(None)` is a scanned empty slot.
    pub fn slot(&self, class: ObjectClass, at: Location) -> Option<Option<&ProgramInfo>> {
        let bank = self.bank(class, user_bank(at.bank)?)?;
        bank.get(at.slot as usize).map(Option::as_ref)
    }

    /// The format tags the scanned slots of a class report, in the order first seen.
    ///
    /// This is what the folder holds, read from each slot's own format tag. An unscanned
    /// class returns nothing, which means unknown, not empty.
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

    /// A scanned bank's slots, each with the address it answers to, in address order. A
    /// bank no walk has read has none.
    ///
    /// ⚠️ Converts the scan cache's numbering (banks as the panel counts them, slots from
    /// one) into the zero-indexed [`Location`] everything else uses.
    pub fn slots_of(
        &self,
        class: ObjectClass,
        bank: u32,
    ) -> impl Iterator<Item = (Location, &Option<ProgramInfo>)> + '_ {
        self.bank(class, bank)
            .unwrap_or_default()
            .iter()
            .zip(1u32..)
            .map(move |(held, slot)| (Location::from_user(bank, slot), held))
    }

    /// Every scanned slot of `class`, in address order.
    fn all_slots(
        &self,
        class: ObjectClass,
    ) -> impl Iterator<Item = (Location, &Option<ProgramInfo>)> + '_ {
        self.banks_of(class)
            .into_iter()
            .flat_map(move |bank| self.slots_of(class, bank))
    }

    /// Every slot of `class` a walk found vacant, in address order.
    pub fn free_slots(&self, class: ObjectClass) -> impl Iterator<Item = Location> + '_ {
        self.all_slots(class)
            .filter(|(_, held)| held.is_none())
            .map(|(at, _)| at)
    }

    /// The first slot of `class` known to be vacant and not in `taken`: where a duplicate
    /// lands when the user did not drag it anywhere, and where the next of a queued set
    /// goes.
    ///
    /// ⚠️ `taken` holds addresses already claimed. Two writes to one address leave one
    /// object, so a set queued together takes successive free slots instead of all
    /// landing on the first.
    pub fn first_free(&self, class: ObjectClass, taken: &[Location]) -> Option<Location> {
        self.free_slots(class).find(|at| !taken.contains(at))
    }

    /// Drop one bank's cached names, because something just changed them.
    fn forget_bank(&mut self, class: ObjectClass, bank: u32) {
        self.banks.remove(&(class, bank));
    }

    fn forget_everything(&mut self) {
        self.banks.clear();
        self.focus.clear();
        self.geometry.clear();
        self.partitions.clear();
        self.inventory.clear();
        self.needs.clear();
        self.detail = Detail::default();
        self.scan.clear();
    }

    /// Whether a stored link names a slot this keyboard can have: a class this build
    /// knows and the keyboard declares, a name that is not blank, and an address inside
    /// the class's banks. Unknown until the partition table and the class's banks arrive.
    fn names_a_slot(&self, held: &Held) -> bool {
        let class = held.class();
        let declared =
            self.partitions.is_empty() || self.partitions.iter().any(|row| row.class == class);
        let fits = match &held.spot {
            Spot::Named(name) => !name.trim().is_empty(),
            Spot::At { bank, slot } => self
                .geometry
                .get(&class)
                .is_none_or(|banks| banks.iter().any(|it| it.index == *bank && *slot < it.slots)),
        };
        !matches!(class, ObjectClass::Unknown(_)) && declared && fits
    }
}

/// How full a folder is, short enough for its heading: `312/400` for a class with slots.
/// A library (pianos, samples) counts blocks of its partition's allocation unit, shown as
/// bytes used out of the total once the unit is known and as an item count before.
pub fn occupancy(
    class: ObjectClass,
    inventory: &[Status],
    unit: Option<AllocationUnit>,
) -> Option<String> {
    let status = inventory.iter().find(|status| status.class == class)?;
    if let Some(slots) = status.slots() {
        return Some(format!("{}/{slots}", status.count));
    }
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
/// Built from a partition record, the only way to build one, so a test cannot invent a
/// unit the wire could not carry.
#[cfg(test)]
pub fn pretend_allocation_unit(bytes: u32) -> AllocationUnit {
    nord_usb::wire::Partition {
        index: 0,
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
/// The asset's home folder decides the class, because an asset is only written to the
/// folder its kind belongs in.
pub fn fit(state: &DeviceState, entity: &LocalEntity) -> Fit {
    let Some(product) = state.product() else {
        return Fit::Unattached;
    };
    let Some(class) = crate::browser::Kind::of(entity).home() else {
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
            // ⚠️ Unreachable while `accepts` refuses only tags another family carries;
            // handled so a widened table cannot panic here.
            None => format!("A {tag} file is not one a {product} takes."),
        }),
        Acceptance::Unknown if !family.written() => Fit::Warn(format!(
            "Nothing has ever been written to a {product}. Sending to it is untried."
        )),
        Acceptance::Unknown => unknown(),
    }
}

/// How `class` keys its slot at `at`: by the address, or by the name a walk found there.
pub(crate) fn spot_at(state: &DeviceState, class: ObjectClass, at: Location) -> Option<Spot> {
    match class.is_library() {
        true => Some(Spot::of(class, at, &state.slot(class, at).flatten()?.name)),
        false => Some(Spot::at(at)),
    }
}

/// Link the copy `id` of the slot `at` of `class`, read as `print`, to that slot on the
/// attached keyboard, unless another asset is linked there already. A slot no walk has
/// named goes by the name a write of the copy gives it.
pub fn link_copy(
    state: &DeviceState,
    workspace: &mut Workspace,
    id: u64,
    class: ObjectClass,
    at: Location,
    print: Print,
) {
    let (Some(keyboard), Some(entity)) = (state.keyboard(), workspace.get(id)) else {
        return;
    };
    let scanned = state
        .slot(class, at)
        .flatten()
        .map(|info| info.name.clone());
    let name = scanned
        .or_else(|| slot_label(&entity.name))
        .unwrap_or_default();
    let Some(held) = Held::new(class, at, &name, print) else {
        return;
    };
    if workspace.linked_at(keyboard, class, &held.spot).is_none() {
        workspace.link(id, keyboard, held);
    }
}

/// Every piano and sample slot a walk found, by class and name, the lowest address of
/// each name.
type ByName<'s> = HashMap<(ObjectClass, &'s str), (Location, &'s ProgramInfo)>;

fn by_name(state: &DeviceState) -> ByName<'_> {
    let mut by_name = ByName::new();
    for class in [ObjectClass::Piano, ObjectClass::Sample] {
        for (at, info) in occupied(state, class) {
            by_name
                .entry((class, info.name.trim()))
                .or_insert((at, info));
        }
    }
    by_name
}

/// Link each asset with no link on the attached keyboard to the slot it names, where that
/// slot holds exactly its saved body and nothing else is linked there, and log them in
/// one line. Other slots holding the same body do not matter.
///
/// It names the slot its header stores, then the slot it came off or was rescued from,
/// and the first of them holding its body is the one. A piano or sample names the slot
/// of its name, which must hold a body of its length. A new asset, and one the user keeps
/// off the keyboard, names none.
fn link_found(state: &DeviceState, workspace: &mut Workspace, log: &mut Log) {
    let Some(keyboard) = state.keyboard() else {
        return;
    };
    let by_name = by_name(state);
    let found: Vec<(u64, Held)> = workspace
        .listed()
        .filter_map(|entity| Some((entity.id, found(state, &by_name, keyboard, entity)?)))
        .collect();
    let mut claims: HashMap<(ObjectClass, &Spot), usize> = HashMap::new();
    let linked = workspace.entities().iter();
    let linked = linked.filter_map(|entity| entity.held_on(keyboard));
    for held in linked.chain(found.iter().map(|(_, held)| held)) {
        *claims.entry((held.class(), &held.spot)).or_default() += 1;
    }
    let found: Vec<(u64, Held)> = found
        .iter()
        .filter(|(_, held)| claims[&(held.class(), &held.spot)] == 1)
        .cloned()
        .collect();
    if found.is_empty() {
        return;
    }
    let linked: Vec<u64> = found.iter().map(|(id, _)| *id).collect();
    for (id, held) in found {
        workspace.link(id, keyboard, held);
    }
    log.say(format!(
        "{} linked to {}.",
        counted(linked.len(), "file", "files"),
        match linked.len() {
            1 => "the slot that holds it",
            _ => "the slots that hold them",
        }
    ));
    log.offer(crate::log::Undo::Unlink {
        keyboard: keyboard.clone(),
        ids: linked,
    });
}

/// The slot [`link_found`] links an asset to, and what the two agree on.
fn found(
    state: &DeviceState,
    by_name: &ByName<'_>,
    keyboard: &Keyboard,
    entity: &LocalEntity,
) -> Option<Held> {
    if matches!(entity.origin, Origin::Fresh) {
        return None;
    }
    let class = home(entity)?;
    let print = entity.saved_print()?;
    if entity.kept_off(keyboard) || held_here(state, entity).is_some() {
        return None;
    }
    let (at, info) = match class.is_library() {
        true => {
            let name = slot_label(&entity.name)?;
            let &(at, info) = by_name.get(&(class, name.as_str()))?;
            (info.body_len == print.len).then_some((at, info))?
        }
        false => named_slots(entity, class).find_map(|at| {
            let info = state.slot(class, at).flatten()?;
            (info.crc32 == Some(print.crc)).then_some((at, info))
        })?,
    };
    let held = Held::new(class, at, &info.name, print)?;
    fit(state, entity).allowed().then_some(held)
}

/// Settle slots two assets are linked to, as two libraries that each linked one before
/// they synced leave them: of the files linked to a slot a walk has read, one keeps its
/// link and the others are unlinked, in one log line whose Undo links them back.
///
/// Every computer keeps the same one: the file saved as what the slot holds, else the one
/// whose link knew what it holds, else the one whose link has the slot's name, else the
/// greatest link.
fn settle_claims(state: &DeviceState, workspace: &mut Workspace, log: &mut Log) {
    let Some(keyboard) = state.keyboard().cloned() else {
        return;
    };
    let mut slots: std::collections::BTreeMap<(u32, &Spot), Vec<(&LocalEntity, &Held)>> =
        Default::default();
    for entity in workspace.listed() {
        if let Some(held) = held_here(state, entity) {
            let code = crate::store::keys::class_code(held.class()).unwrap_or_default();
            let claims = slots.entry((code, &held.spot)).or_default();
            claims.push((entity, held));
        }
    }
    let unlinked: Vec<(u64, Held)> = slots
        .values()
        .filter(|claims| claims.len() > 1)
        .filter_map(|claims| Some((kept(state, claims)?, claims)))
        .flat_map(|(kept, claims)| {
            let others = claims.iter().filter(move |(entity, _)| entity.id != kept);
            others.map(|(entity, held)| (entity.id, (*held).clone()))
        })
        .collect();
    if unlinked.is_empty() {
        return;
    }
    for (id, _) in &unlinked {
        workspace.unlink(*id, &keyboard);
    }
    log.say(format!(
        "{} unlinked from a slot another file is linked to.",
        counted(unlinked.len(), "file", "files")
    ));
    log.offer(crate::log::Undo::Reclaim {
        keyboard,
        links: unlinked,
    });
}

/// Which of the files linked to one slot [`settle_claims`] keeps. `None` while no walk has
/// read the slot.
fn kept(state: &DeviceState, claims: &[(&LocalEntity, &Held)]) -> Option<u64> {
    let (at, there) = there(state, claims.first()?.1);
    if there == There::Unread {
        return None;
    }
    let named = at
        .and_then(|at| state.slot(claims[0].1.class(), at).flatten())
        .map(|info| info.name.trim());
    let rank = |&&(entity, held): &&(&LocalEntity, &Held)| {
        let same = crate::link::standing(held, entity.saved_print(), there) == Standing::Same;
        let knew = matches!(there, There::Holds { crc: Some(crc), .. } if held.knew(crc));
        let named = named.is_some() && held.name() == named;
        (same, knew, named, held.clone(), entity.path.clone())
    };
    claims.iter().max_by_key(rank).map(|(entity, _)| entity.id)
}

/// The slots of `class` an asset names: the address its header stores, then the slot it
/// came off or was rescued from.
pub fn named_slots(entity: &LocalEntity, class: ObjectClass) -> impl Iterator<Item = Location> {
    let stored = entity
        .container
        .as_ref()
        .map(|container| container.header.slot())
        .map(|(bank, slot)| Location {
            bank: bank.into(),
            slot: slot.into(),
        });
    let came_off = match entity.origin {
        Origin::Device { class: from, at } if from == class => Some(at),
        Origin::Rescued { at } => Some(at),
        Origin::Device { .. } | Origin::File(_) | Origin::Fresh => None,
    };
    stored.into_iter().chain(came_off)
}

/// Record the name each linked slot of a bank just read has, where it holds what its file
/// and it last agreed on under another name than the link recorded: the keyboard altered
/// the name it was sent, or the panel renamed the slot, which keeps its checksum. A slot two
/// files claim is left to [`settle_claims`].
fn follow_names(state: &DeviceState, workspace: &mut Workspace, class: ObjectClass, bank: u32) {
    let Some(keyboard) = state.keyboard() else {
        return;
    };
    let claimed = |held: &Held| {
        let others = workspace
            .listed()
            .filter_map(|entity| held_here(state, entity));
        others
            .filter(|other| other.class() == class && other.spot == held.spot)
            .count()
            > 1
    };
    let renamed: Vec<(u64, Held)> = workspace
        .listed()
        .filter_map(|entity| {
            let held = held_here(state, entity).filter(|held| held.class() == class)?;
            let at = held
                .location()
                .filter(|at| user_bank(at.bank) == Some(bank))?;
            let info = state.slot(class, at).flatten()?;
            let name = info.name.trim();
            let agrees = info.crc32 == Some(held.base.crc);
            let renamed = agrees && !name.is_empty() && held.name() != Some(name);
            (renamed && !claimed(held)).then(|| (entity.id, held.agreed(held.base, name)))
        })
        .collect();
    for (id, held) in renamed {
        workspace.link(id, keyboard, held);
    }
}

/// Link a view just kept to the slot it shows, as it was read.
pub fn link_kept(state: &DeviceState, workspace: &mut Workspace, id: u64) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    if let (Some((class, at)), Some(print)) = (entity.origin.slot(), entity.saved_print()) {
        link_copy(state, workspace, id, class, at, print);
    }
}

/// Link `id` to the slot a send of it reached under `name` as `print`, in place of any
/// other asset.
fn link_sent(
    state: &DeviceState,
    workspace: &mut Workspace,
    id: u64,
    (class, at, name): (ObjectClass, Location, &str),
    print: Print,
) {
    let (Some(keyboard), Some(entity)) = (state.keyboard(), workspace.get(id)) else {
        return;
    };
    let name = slot_label(name).unwrap_or_default();
    let Some(fresh) = Held::new(class, at, &name, print) else {
        return;
    };
    let held = match entity.held_on(keyboard) {
        Some(held) if held.class() == class && held.spot == fresh.spot => held.agreed(print, &name),
        _ => fresh,
    };
    workspace.link(id, keyboard, held);
}

/// Where the slot `held` names is, and what a walk says it holds there. A slot kept by
/// its name has no address until a walk finds it, and is vacant only once every bank of
/// its class has been read.
pub fn there(state: &DeviceState, held: &Held) -> (Option<Location>, There) {
    let class = held.class();
    let holds = |info: &ProgramInfo| There::Holds {
        crc: info.crc32,
        len: info.body_len,
    };
    let found = match &held.spot {
        Spot::At { bank, slot } => {
            let at = Location {
                bank: *bank,
                slot: *slot,
            };
            return match state.slot(class, at) {
                None => (Some(at), There::Unread),
                Some(None) => (Some(at), There::Vacant),
                Some(Some(info)) => (Some(at), holds(info)),
            };
        }
        Spot::Named(name) => named(state, class, name).next(),
    };
    match found {
        Some((at, info)) => (Some(at), holds(info)),
        None if state.read_whole(class) => (None, There::Vacant),
        None => (None, There::Unread),
    }
}

/// An asset's link on the attached keyboard, unless it names no slot that keyboard can
/// have, which counts as no link.
fn held_here<'e>(state: &DeviceState, entity: &'e LocalEntity) -> Option<&'e Held> {
    let card = state.card()?;
    let link = entity
        .links
        .iter()
        .find(|link| link.keyboard.model == card.product && link.keyboard.serial == card.serial)?;
    link.held.as_ref().filter(|held| state.names_a_slot(held))
}

/// An asset's place on a keyboard, and how the asset compares with its slot there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Linked {
    pub class: ObjectClass,
    /// The slot's address, once known: a slot kept by its name has none until a walk
    /// finds it.
    pub at: Option<Location>,
    pub there: There,
    pub standing: Standing,
}

impl Linked {
    /// Whether its address says nothing yet of what the slot holds.
    pub fn faint(&self) -> bool {
        self.at.is_none() || self.standing == Standing::Unread
    }
}

/// How an asset compares with the slot it is linked to on the attached keyboard. `None`
/// while nothing is attached, or the asset has no link there.
pub fn standing_of(state: &DeviceState, entity: &LocalEntity) -> Option<Linked> {
    let held = held_here(state, entity)?;
    let (at, there) = there(state, held);
    Some(Linked {
        class: held.class(),
        at,
        there,
        standing: crate::link::standing(held, entity.saved_print(), there),
    })
}

/// Where an asset is on a keyboard: [`standing_of`] while one is attached, and else the
/// address of its one link, unread.
pub fn place_of(state: &DeviceState, entity: &LocalEntity) -> Option<Linked> {
    if state.connected() {
        return standing_of(state, entity);
    }
    let mut held = entity.links.iter().filter_map(|link| link.held.as_ref());
    let (Some(held), None) = (held.next(), held.next()) else {
        return None;
    };
    Some(Linked {
        class: held.class(),
        at: Some(held.location()?),
        there: There::Unread,
        standing: Standing::Unread,
    })
}

/// The slot an asset stands for on the attached keyboard: the one a kept asset is linked
/// to, where a walk has found it, or the one a view shows.
pub fn slot_of(state: &DeviceState, entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    if !entity.kept {
        return entity.origin.slot();
    }
    let linked = standing_of(state, entity)?;
    Some((linked.class, linked.at?))
}

/// The slot an asset would be written back to ([`slot_of`]), when this app writes to that
/// class.
pub fn owed(state: &DeviceState, entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    slot_of(state, entity).filter(|(class, _)| !read_only(*class))
}

/// The asset that stands for the slot `at` of `class`: the one linked to it, or a view of
/// it.
pub fn standing_on<'w>(
    state: &DeviceState,
    workspace: &'w Workspace,
    class: ObjectClass,
    at: Location,
) -> Option<&'w LocalEntity> {
    let linked = state.keyboard().zip(spot_at(state, class, at));
    let linked = linked.and_then(|(keyboard, spot)| workspace.linked_at(keyboard, class, &spot));
    match linked.and_then(|id| workspace.get(id)) {
        Some(entity) => Some(entity),
        None => workspace
            .entities()
            .iter()
            .find(|entity| !entity.kept && entity.origin.slot() == Some((class, at))),
    }
}

/// A slot of the attached keyboard an asset with no link there may be linked to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Offer {
    class: ObjectClass,
    at: Location,
    /// Whether the slot holds exactly what the asset was saved as.
    holds: bool,
}

/// [`link_offers`], each with whether it holds what the asset was saved as.
fn offers(state: &DeviceState, workspace: &Workspace, entity: &LocalEntity) -> Vec<Offer> {
    let (Some(keyboard), Some(class), Some(print)) =
        (state.keyboard(), home(entity), entity.saved_print())
    else {
        return Vec::new();
    };
    if held_here(state, entity).is_some() {
        return Vec::new();
    }
    let label = slot_label(&entity.name).filter(|_| !matches!(entity.origin, Origin::Fresh));
    let free = |at: &Location| {
        let spot = spot_at(state, class, *at);
        spot.is_none_or(|spot| workspace.linked_at(keyboard, class, &spot).is_none())
    };
    let offer = |at: Location| {
        let info = state.slot(class, at).flatten()?;
        let holds = info.crc32 == Some(print.crc);
        let named = label.as_deref() == Some(info.name.trim());
        (holds || named).then_some(Offer { class, at, holds })
    };
    let named = named_slots(entity, class).filter(free).find_map(offer);
    let holding = occupied(state, class)
        .filter(|(at, info)| info.crc32 == Some(print.crc) && named.is_none_or(|it| it.at != *at))
        .map(|(at, _)| at)
        .filter(free)
        .map(|at| Offer {
            class,
            at,
            holds: true,
        });
    named.into_iter().chain(holding).collect()
}

/// The slots of the attached keyboard an asset with no link there may be linked to: the
/// ones holding exactly what it was saved as, and first the slot its header or origin
/// names where that slot holds it or, for an asset that is not new, a sound under its
/// name. Only those no other asset is linked to. Only a hint: nothing is linked, merged or
/// sent by it.
pub fn link_offers(
    state: &DeviceState,
    workspace: &Workspace,
    entity: &LocalEntity,
) -> Vec<(ObjectClass, Location)> {
    let offers = offers(state, workspace, entity).into_iter();
    offers.map(|offer| (offer.class, offer.at)).collect()
}

/// The slots of [`link_offers`] holding exactly what an asset was saved as.
pub fn also_at(
    state: &DeviceState,
    workspace: &Workspace,
    entity: &LocalEntity,
) -> Vec<(ObjectClass, Location)> {
    let offers = offers(state, workspace, entity).into_iter();
    let holding = offers.filter(|offer| offer.holds);
    holding.map(|offer| (offer.class, offer.at)).collect()
}

/// The folder an asset belongs in.
///
/// A view is not on this computer until it is kept, and bytes that decode into nothing
/// belong in no folder.
fn home(entity: &LocalEntity) -> Option<ObjectClass> {
    entity
        .kept
        .then(|| crate::browser::Kind::of(entity).home())
        .flatten()
}

/// The name a write gives its slot: an asset's name without its format suffix, and
/// `None` for a blank name.
pub(crate) fn slot_label(name: &str) -> Option<String> {
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

/// The slot of a library, other than `at`, already holding the name a write of this
/// asset into `at` would give.
///
/// ⚠️ The instrument refuses that write with [`nord_usb::op::NAME_TAKEN`], whatever the
/// slot.
///
/// Slot classes take any number of objects of one name.
pub fn name_taken(
    state: &DeviceState,
    class: ObjectClass,
    at: Location,
    entity: &LocalEntity,
) -> Option<Location> {
    if !class.is_library() {
        return None;
    }
    let name = slot_label(&entity.name)?;
    let taken = named(state, class, &name).find(|(held, _)| *held != at);
    taken.map(|(held, _)| held)
}

/// Every slot of `class` a walk found holding something, lowest address first.
pub(crate) fn occupied(
    state: &DeviceState,
    class: ObjectClass,
) -> impl Iterator<Item = (Location, &ProgramInfo)> + '_ {
    state
        .all_slots(class)
        .filter_map(|(at, held)| Some((at, held.as_ref()?)))
}

/// Every slot of `class` a walk found holding something named `name`, lowest address
/// first.
fn named<'s>(
    state: &'s DeviceState,
    class: ObjectClass,
    name: &'s str,
) -> impl Iterator<Item = (Location, &'s ProgramInfo)> + 's {
    occupied(state, class).filter(move |(_, info)| info.name.trim() == name)
}

/// Whether the browser offers to change a class at all.
///
/// ⚠️ A partition this app cannot name is listed and left alone: nothing here knows what
/// a write into it would mean. Every class it can name takes a write, piano libraries
/// included; [`crate::room::free_bytes`] answers whether the partition has room.
pub fn read_only(class: ObjectClass) -> bool {
    matches!(class, ObjectClass::Unknown(_))
}

pub fn loadable(state: &DeviceState, class: ObjectClass, at: Location) -> bool {
    state.connected() && !read_only(class) && state.slot(class, at).flatten().is_some()
}

/// What a write into this class disturbs beyond the slot it lands in, for the question
/// asked before it happens.
///
/// A settings write makes the Electro 5 reload the selected program, so panel state the
/// player has not stored is lost. Confirmed on hardware. Another model is assumed to do
/// the same.
pub fn write_warning(class: ObjectClass) -> Option<&'static str> {
    match class {
        ObjectClass::Settings => Some(
            "Writing settings reloads the selected program on the instrument. \
             Panel changes that have not been stored will be lost.",
        ),
        _ => None,
    }
}

/// Why nothing can be connected, in a browser with no WebUSB.
pub const NO_USB: &str = "This browser can't reach an instrument over USB. Open drawbar in \
     Chrome, Edge or Brave to connect.";

pub struct Device {
    pub state: DeviceState,
    /// Whether this build can reach an instrument at all. Settled once: a browser does
    /// not gain or lose WebUSB while the page is open.
    usb: bool,
    events: Receiver<DeviceEvent>,
    /// A second handle on the worker's end of the channel, so a headless test can hand
    /// [`Device::poll`] the events an instrument would have reported.
    #[cfg(test)]
    from_worker: std::sync::mpsc::Sender<DeviceEvent>,
    link: Link,
    /// What the user asked for, each with the links it moves once it succeeds, taken
    /// when it was asked for: an earlier command in the queue may drop the names a later
    /// one's are keyed by. Always dispatched ahead of the background scan.
    pending: VecDeque<(DeviceCmd, Option<Respot>)>,
    /// What the command in flight leaves to do when it finishes or fails.
    running: Option<Running>,
    /// The list revision [`link_found`] last looked at. It looks again when the list or
    /// the instrument changes, and not every frame.
    list_seen: u64,
    /// Objects read into files, waiting to be copied into the library.
    fetched: Vec<Fetched>,
    /// The slots the last [`DeviceCmd::Gather`] found, until taken.
    gathered: Option<(u64, Vec<(ObjectClass, Location)>)>,
    /// Whether a command failed, a copied slot was empty, or the instrument went, since
    /// the last [`Device::take_failed`].
    failed: bool,
    /// The slots whose former occupants writes left on this computer since the last
    /// [`Device::take_kept`].
    kept: Vec<Location>,
    /// Counts what the instrument has reported and the links that moved since.
    revision: u64,
    /// What drawbar asks, and has been told, about linked files while this keyboard is
    /// attached.
    pub asks: crate::bring::Asks,
}

/// The follow-up owed by the command in flight, set when it is dispatched.
struct Running {
    /// The class the command is walking, so a scan that fails stops showing as running.
    reading: Option<ObjectClass>,
    /// The banks the command mutates, to be read again once it finishes.
    rescan: Vec<(ObjectClass, u32)>,
    /// The [`DeviceCmd::Reload`] to send once the write has landed.
    reload: Option<DeviceCmd>,
    /// The class the command writes into, so a refusal can be recorded against the queue
    /// entry it stopped on.
    writing: Option<ObjectClass>,
    /// A rescan or reload the app queued after a change. Its success stays off the status
    /// line, which keeps saying what the change did.
    follow_up: bool,
    /// The links the command moves once it succeeds. See [`respot`].
    respot: Option<Respot>,
}

/// Slots of one class whose links move. See [`Workspace::respot`].
type Respot = (ObjectClass, Vec<Moved>);

/// The links a slot operation moves once it succeeds: a move swaps two addresses, a
/// delete unlinks, and a rename gives the slot its new name, which keys a piano or sample.
fn respot(state: &DeviceState, cmd: &DeviceCmd) -> Option<Respot> {
    match cmd {
        DeviceCmd::Move { class, from, to } if !class.is_library() => {
            let (from, to) = (Spot::at(*from), Spot::at(*to));
            Some((
                *class,
                vec![
                    (from.clone(), Some(to.clone()), None),
                    (to, Some(from), None),
                ],
            ))
        }
        DeviceCmd::Delete { class, at } => {
            Some((*class, vec![(spot_at(state, *class, *at)?, None, None)]))
        }
        DeviceCmd::Rename { class, at, name } => {
            let (from, to) = (spot_at(state, *class, *at)?, Spot::of(*class, *at, name));
            Some((*class, vec![(from, Some(to), Some(name.clone()))]))
        }
        _ => None,
    }
}

impl Device {
    pub fn new(ctx: egui::Context) -> Device {
        let (sender, events) = std::sync::mpsc::channel();
        Device {
            state: DeviceState::default(),
            usb: Link::available(),
            events,
            #[cfg(test)]
            from_worker: sender.clone(),
            link: Link::new(ctx, sender, Scratch::default()),
            pending: VecDeque::new(),
            running: None,
            list_seen: 0,
            fetched: Vec::new(),
            gathered: None,
            failed: false,
            kept: Vec::new(),
            revision: 0,
            asks: crate::bring::Asks::default(),
        }
    }

    /// Changes whenever what the instrument said, or a link derived from it, may have.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether an instrument can be reached from here at all. Anything that offers to
    /// connect asks this first, and is grayed out with [`NO_USB`] on hover when it cannot.
    pub fn usb(&self) -> bool {
        self.usb
    }

    /// ⚠️ Must be reached from the frame the button was clicked in. On the web the
    /// device chooser needs the click's transient user activation, and awaiting
    /// anything first spends it.
    pub fn connect(&mut self, log: &mut Log) {
        if !self.usb {
            log.trouble(NO_USB);
            return;
        }
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

    /// Release the instrument on quit, and wait for the worker to close its session.
    ///
    /// ⚠️ Otherwise closing the window mid-command cuts a transaction, and the instrument
    /// holds an open session until it times out. The wait is bounded so an instrument
    /// that has stopped answering cannot hold the window open.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn release(&mut self) {
        self.link.disconnect();
        self.link.join(std::time::Duration::from_secs(2));
    }

    /// Keep the occupant of a slot a write replaces, when it is too large to hold, in
    /// `dir`: this install's own state for the open library, or `None` where the library
    /// cannot be written, for a `rescued` folder of drawbar's own data.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn keep_occupants_in(&self, dir: Option<std::path::PathBuf>) {
        self.link.scratch().keep_in(dir);
    }

    /// Keep the occupant of a slot a write replaces in `dir` of the browser's storage:
    /// the open library's own state for this install, or with `None`, the browser's
    /// library's.
    #[cfg(target_arch = "wasm32")]
    pub fn keep_occupants_in(&self, dir: Option<toshokan::RelPath>) {
        self.link.scratch().keep_in(dir);
    }

    /// Queue one command the user asked for. It runs ahead of the background read, and
    /// after whatever is already in flight, since the protocol runs one transaction at a
    /// time.
    pub fn send(&mut self, cmd: DeviceCmd, log: &mut Log) {
        if !self.state.connected() {
            log.trouble("No instrument is attached.");
            return;
        }
        let respot = respot(&self.state, &cmd);
        self.pending.push_back((cmd, respot));
    }

    /// Whether anything failed since the last call, once.
    pub fn take_failed(&mut self) -> bool {
        std::mem::take(&mut self.failed)
    }

    /// The slots whose former occupants writes left on this computer since the last call.
    pub fn take_kept(&mut self) -> Vec<Location> {
        std::mem::take(&mut self.kept)
    }

    /// The slots the last gather found, with its request, once.
    pub fn take_gathered(&mut self) -> Option<(u64, Vec<(ObjectClass, Location)>)> {
        self.gathered.take()
    }

    /// The objects read into files since the last call, to be copied into the library.
    pub fn take_fetched(&mut self) -> Vec<Fetched> {
        std::mem::take(&mut self.fetched)
    }

    /// Walk `class` again, in one session.
    ///
    /// Cached names stay until each bank's replacement arrives. Banks a mutation touches
    /// are dropped at once (see `Device::dispatch`).
    pub fn read_class(&mut self, class: ObjectClass) {
        self.state.scan.start(class);
    }

    /// Read the whole instrument again, one walk per class as attaching does. Each walk
    /// reads its class's counters, geometry and loaded slot at the start of its session.
    pub fn resync(&mut self) {
        for class in self.state.classes() {
            self.read_class(class);
        }
    }

    /// Start the next command if the instrument is free. Call once a frame, after the
    /// UI has queued its commands.
    pub fn pump(&mut self) {
        if !self.state.connected() || self.state.in_flight.is_some() {
            return;
        }
        if let Some((cmd, respot)) = self.pending.pop_front() {
            return self.dispatch(cmd, respot);
        }
        let Some(class) = self.state.scan.take() else {
            return;
        };
        self.dispatch(DeviceCmd::ScanClass { class }, None);
    }

    fn dispatch(&mut self, cmd: DeviceCmd, respot: Option<Respot>) {
        // Drop affected banks before a mutation so confirmations never quote stale names.
        let touched = match &cmd {
            DeviceCmd::Delete { class, at }
            | DeviceCmd::Rename { class, at, .. }
            | DeviceCmd::Put { class, at, .. } => Some((*class, vec![*at])),
            DeviceCmd::Move { class, from, to } | DeviceCmd::Duplicate { class, from, to } => {
                Some((*class, vec![*from, *to]))
            }
            DeviceCmd::SendAll { class, items } => {
                Some((*class, items.iter().map(|item| item.at).collect()))
            }
            _ => None,
        };
        let rescan: Vec<(ObjectClass, u32)> = touched
            .as_ref()
            .map(|(class, touched)| {
                let mut banks: Vec<u32> =
                    touched.iter().filter_map(|at| user_bank(at.bank)).collect();
                banks.sort_unstable();
                banks.dedup();
                banks.into_iter().map(|bank| (*class, bank)).collect()
            })
            .unwrap_or_default();
        for (class, bank) in &rescan {
            self.state.forget_bank(*class, *bank);
            self.revision += 1;
        }
        if let Some((class, slots)) = &touched {
            let slot_touched = |(held, bank, slot): &(ObjectClass, u32, u32)| {
                held == class && slots.iter().any(|at| at.bank == *bank && at.slot == *slot)
            };
            self.state.needs.retain(|key, _| !slot_touched(key));
        }
        // Writing the Electro 5's loaded slot requires SELECT to reload it. Confirmed on
        // hardware. Another model is assumed alike.
        let written = match &cmd {
            DeviceCmd::Put { class, at, .. } | DeviceCmd::Rename { class, at, .. } => {
                Some((*class, vec![*at]))
            }
            DeviceCmd::SendAll { class, items } => {
                Some((*class, items.iter().map(|item| item.at).collect()))
            }
            _ => None,
        };
        let reload = written
            .filter(|(class, _)| self.state.has_focus(*class))
            .map(|(class, written)| DeviceCmd::Reload { class, written });
        self.running = Some(Running {
            reading: match &cmd {
                DeviceCmd::ScanClass { class } => Some(*class),
                _ => None,
            },
            rescan,
            reload,
            writing: match &cmd {
                DeviceCmd::Put { class, .. } | DeviceCmd::SendAll { class, .. } => Some(*class),
                _ => None,
            },
            follow_up: matches!(cmd, DeviceCmd::ScanBank { .. } | DeviceCmd::Reload { .. }),
            respot,
        });
        self.state.in_flight = Some(cmd.words());
        self.link.send(cmd);
    }

    /// Stand in for a browser with no WebUSB.
    #[cfg(test)]
    pub fn pretend_no_usb(&mut self) {
        self.usb = false;
    }

    /// Hand `poll` an event as though the worker had reported it.
    #[cfg(test)]
    pub fn pretend(&mut self, event: DeviceEvent) {
        let _ = self.from_worker.send(event);
    }

    /// What the user has asked the instrument for and it has not started yet.
    #[cfg(test)]
    pub fn queued(&self) -> VecDeque<&DeviceCmd> {
        self.pending.iter().map(|(cmd, _)| cmd).collect()
    }

    /// Attach an instrument, as its descriptors would have. The product string matches
    /// the recorded exchanges in `nord-usb`.
    #[cfg(test)]
    pub fn pretend_attached(&mut self) {
        self.pretend_attached_as("Nord Electro 5");
    }

    /// Attach an instrument with the given product string, for rules that depend on the
    /// model.
    #[cfg(test)]
    pub fn pretend_attached_as(&mut self, product: &str) {
        self.state.attach(DeviceCard {
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

    /// Attach the card attached now again, reporting `serial`.
    #[cfg(test)]
    pub fn pretend_serial(&mut self, serial: &str) {
        let card = self.state.card().expect("attached").clone();
        let serial = Some(serial.to_string());
        self.state.attach(DeviceCard { serial, ..card });
    }

    /// Fill in a bank as though a walk had answered, under the panel's bank number, which
    /// is the number [`DeviceEvent::BankScanned`] carries.
    #[cfg(test)]
    pub fn pretend_scanned(&mut self, class: ObjectClass, bank: u32, names: &[&str]) {
        use nord_usb::wire::ProgramInfo;

        self.pretend_attached();
        let slots = names
            .iter()
            .enumerate()
            .map(|(slot, name)| {
                // An empty name makes a vacant slot.
                (!name.is_empty()).then(|| ProgramInfo {
                    location: Location::from_user(bank, slot as u32 + 1),
                    body_len: 121,
                    format: "ne5p".into(),
                    version: 4,
                    crc32: None,
                    modified: None,
                    name: (*name).to_string(),
                })
            })
            .collect();
        self.state.banks.insert((class, bank), slots);
    }

    /// Give the instrument the partition table it would have declared: the class of each
    /// row, the device's name for it, and the net bytes its counters are in units of.
    #[cfg(test)]
    pub fn pretend_partitions(&mut self, table: &[(ObjectClass, &str, u32)]) {
        self.state.partitions = table
            .iter()
            .map(|(class, name, bytes)| Partition {
                class: *class,
                name: (*name).to_string(),
                native: false,
                unit: Some(pretend_allocation_unit(*bytes)),
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
                    modified: None,
                    name: name.to_string(),
                })
            })
            .collect();
        self.state.banks.insert((class, bank), slots);
    }

    /// Fill in a bank as a walk of a piano or sample library would have: a name and a
    /// body length for each occupied slot, and no checksum, since those slots report none.
    #[cfg(test)]
    pub fn pretend_lengths(
        &mut self,
        class: ObjectClass,
        bank: u32,
        slots: &[Option<(&str, u32)>],
    ) {
        self.pretend_attached();
        let slots = slots
            .iter()
            .enumerate()
            .map(|(slot, held)| {
                held.map(|(name, body_len)| ProgramInfo {
                    location: Location::from_user(bank, slot as u32 + 1),
                    body_len,
                    format: "npno".into(),
                    version: 540,
                    crc32: None,
                    modified: None,
                    name: name.to_string(),
                })
            })
            .collect();
        self.state.banks.insert((class, bank), slots);
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
        self.state.geometry.insert(class, banks);
    }

    /// Answer for a slot's dependencies, as a `DEPENDENCIES` read would have.
    #[cfg(test)]
    pub fn pretend_deps(&mut self, class: ObjectClass, at: Location, deps: Vec<Dependency>) {
        self.state.depends(class, at, deps);
    }

    /// Put the panel on a slot, as a walk's `FOCUS` read would have.
    #[cfg(test)]
    pub fn pretend_focused(&mut self, class: ObjectClass, at: Location) {
        self.state.focus.insert(class, Some(at));
    }

    /// Drop everything the last instrument said, and whatever its command in flight was
    /// owed.
    fn forget(&mut self) {
        self.state.forget_everything();
        self.running = None;
        self.asks = crate::bring::Asks::default();
        self.revision += 1;
    }

    /// Whether a write to the instrument is running or waiting to run.
    pub fn sending(&self) -> bool {
        let writes =
            |cmd: &DeviceCmd| matches!(cmd, DeviceCmd::Put { .. } | DeviceCmd::SendAll { .. });
        let running = self.running.as_ref();
        running.is_some_and(|running| running.writing.is_some())
            || self.pending.iter().any(|(cmd, _)| writes(cmd))
    }

    /// Log each slot just read whose body differs from what the asset linked to it was
    /// saved as.
    ///
    /// Once per asset per read of its bank, at info level: the checksums are protocol
    /// detail, and the user sees the row's mark.
    fn disagreements(&self, class: ObjectClass, bank: u32, workspace: &Workspace, log: &mut Log) {
        for entity in workspace.listed() {
            let Some(linked) = standing_of(&self.state, entity) else {
                continue;
            };
            let Some(at) = linked
                .at
                .filter(|at| linked.class == class && user_bank(at.bank) == Some(bank))
            else {
                continue;
            };
            let (
                Some(here),
                There::Holds {
                    crc: Some(there), ..
                },
            ) = (entity.saved.crc32(), linked.there)
            else {
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
                    crate::telemetry::attached(Some(&card));
                    log.info(format!(
                        "connected: {} ({:04x}:{:04x})",
                        card.product, card.vendor_id, card.product_id
                    ));
                    log.say(format!("{} is attached.", card.product));
                    self.state.attach(card);
                    self.forget();
                    self.pending.clear();
                }
                DeviceEvent::ConnectFailed(why) => {
                    crate::telemetry::fault("usb", "connect-failed");
                    log.error(why);
                    log.trouble("No instrument could be opened.");
                    self.state.connection = Connection::Disconnected;
                }
                // ⚠️ Disconnection must not clear local edits waiting for the instrument.
                DeviceEvent::Disconnected { lost } => {
                    if lost {
                        crate::telemetry::fault("usb", "lost");
                    }
                    crate::telemetry::attached(None);
                    self.failed = true;
                    match lost {
                        true => log.trouble("The instrument went away. Reconnect when it's back."),
                        false => log.say("The instrument was released."),
                    }
                    self.state.connection = Connection::Disconnected;
                    self.state.in_flight = None;
                    self.forget();
                    self.pending.clear();
                }
                DeviceEvent::Started(what) => log.info(what),
                DeviceEvent::Finished => {
                    if let Some(running) = self.running.take() {
                        if let Some(class) = running.reading {
                            self.state.scan.finished(class);
                            self.state.scan.heard(class, now);
                        }
                        // The panel is still playing what it read before the write.
                        self.pending.extend(running.reload.map(|cmd| (cmd, None)));
                        self.pending.extend(
                            running
                                .rescan
                                .into_iter()
                                .map(|(class, bank)| (DeviceCmd::ScanBank { class, bank }, None)),
                        );
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
                    if banks.is_some() {
                        self.state.scan.expect(class, banks);
                    }
                }
                // Only the partition table says which classes exist, so the walk starts
                // here.
                // ⚠️ The queue outlives a disconnection, and its entries were checked
                // against whatever was attached when they were queued.
                DeviceEvent::Partitions(partitions) => {
                    self.state.partitions = partitions;
                    crate::queue::refit(workspace, &self.state, queue, log);
                    crate::queue::reattach(workspace, self, queue, log);
                    self.resync();
                }
                DeviceEvent::Geometry { class, banks } => {
                    self.state.geometry.insert(class, banks);
                }
                DeviceEvent::Focus { class, at } => {
                    self.state.focus.insert(class, at);
                }
                // An empty bank is still an answer: dropping it would leave its old names
                // in place.
                DeviceEvent::BankScanned { class, bank, slots } => {
                    self.state.banks.insert((class, bank), slots);
                    self.state.scan.bank(class, bank);
                    self.state.scan.heard(class, now);
                    self.disagreements(class, bank, workspace, log);
                    follow_names(&self.state, workspace, class, bank);
                    crate::queue::refresh(workspace, self, queue, log);
                }
                DeviceEvent::SlotInfo { class, at, info } => {
                    self.state.detail = Detail {
                        at: Some((class, at)),
                        info: Some(info),
                        deps: None,
                    };
                }
                DeviceEvent::Deps { class, at, deps } => self.state.depends(class, at, deps),
                // A view belongs to its tab, a copied read becomes a local entity, and
                // an occupant read for a diff goes only to the queue.
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
                        let print = crate::workspace::print_of(&bytes);
                        let slot = origin.slot();
                        let id = workspace.ingest(name, origin, bytes, log);
                        if let (Some((class, at)), Some(print)) = (slot, print) {
                            link_copy(&self.state, workspace, id, class, at, print);
                        }
                    }
                    Purpose::Compare => {
                        if let Some((class, at)) = origin.slot() {
                            queue.arrived(class, at, &name, &bytes, workspace);
                        }
                    }
                    Purpose::Pull => {
                        if let Some((class, at)) = origin.slot() {
                            crate::bring::landed(self, workspace, log, class, at, bytes);
                        }
                    }
                    Purpose::Choose => {
                        if let Some((class, at)) = origin.slot() {
                            self.asks.read(class, at, bytes, workspace);
                        }
                    }
                },
                DeviceEvent::Fetched(fetched) => self.fetched.push(fetched),
                DeviceEvent::Gathered {
                    request,
                    slots,
                    unfound,
                } => {
                    for what in unfound {
                        log.trouble(format!(
                            "The bundle leaves out the {what}: no one slot holds it."
                        ));
                    }
                    self.gathered = Some((request, slots));
                }
                // An empty slot is a failure to a user who asked to copy or open it, and
                // an answer to the queue: nothing is being replaced.
                DeviceEvent::Vacant { class, at, why } => match why {
                    Purpose::Compare => queue.vacant(class, at),
                    Purpose::Pull | Purpose::Choose => self.asks.vacant(class, at),
                    Purpose::Copy | Purpose::View => {
                        self.failed = true;
                        log.error(format!("{} holds nothing to read", shown(at)));
                        log.trouble(format!("{} is empty.", place(class, at)));
                    }
                },
                DeviceEvent::Kept { at, place } => {
                    log.error(format!(
                        "{} may have lost what it held; its bytes are kept at {place}",
                        shown(at)
                    ));
                    self.kept.push(at);
                }
                DeviceEvent::Summed {
                    class,
                    at,
                    received,
                } => queue.summed(class, at, &received, workspace),
                // This covers that object only; the rest of a batch still waits on its
                // own write.
                DeviceEvent::Sent {
                    id,
                    class,
                    at,
                    name,
                    sent,
                } => {
                    queue.forget(id);
                    let print = sent.print();
                    match sent {
                        Payload::Bytes(bytes) => workspace.landed(id, bytes),
                        Payload::File { file, print } => workspace.landed_file(id, file, print.crc),
                    }
                    if let Some(print) = print {
                        link_sent(&self.state, workspace, id, (class, at, &name), print);
                    }
                }
                DeviceEvent::Changed {
                    id,
                    class,
                    at,
                    holds,
                } => {
                    let reload = self.running.as_mut().and_then(|run| run.reload.as_mut());
                    if let Some(DeviceCmd::Reload { written, .. }) = reload {
                        written.retain(|it| *it != at);
                    }
                    let slot = (id, class, at);
                    crate::queue::changed_under(workspace, self, queue, log, slot, holds.as_ref());
                    let name = workspace.get(id).map_or("", |entity| entity.name.as_str());
                    log.trouble(format!(
                        "“{name}” was not sent: {} changed on the keyboard.",
                        place(class, at)
                    ));
                }
                DeviceEvent::Note(text) => log.info(text),
                DeviceEvent::OpOk(text) => {
                    log.info(text);
                    let respot = self.running.as_mut().and_then(|run| run.respot.take());
                    if let (Some((class, moves)), Some(keyboard)) = (respot, self.state.keyboard())
                    {
                        workspace.respot(keyboard, class, &moves);
                        crate::queue::unqueue_moved(workspace, queue, log, &[(class, moves)]);
                    }
                    let follow_up = self.running.as_ref().is_some_and(|run| run.follow_up);
                    if let Some(words) = self.state.in_flight.as_ref().filter(|_| !follow_up) {
                        log.say(format!("{}.", words.done));
                    }
                }
                // A refused write left its slot as it was, or empty after a rescue, so
                // the panel has nothing new to play there.
                DeviceEvent::OpFailed(text) => {
                    self.failed = true;
                    if let Some(running) = self.running.as_mut() {
                        running.reload = None;
                        if let Some(class) = running.writing {
                            queue.stumbled(class, &text);
                        }
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
                    log.warn("the instrument changed; every cached name is dropped");
                    self.state.needs.clear();
                    log.say("Something changed on the instrument. Reading it again…");
                    self.resync();
                }
            }
        }
        // An asset that arrives while an instrument is attached is linked against what
        // the cache already shows, without waiting for the next walk.
        if heard || self.list_seen != workspace.revision() {
            let before = workspace.revision();
            settle_claims(&self.state, workspace, log);
            link_found(&self.state, workspace, log);
            crate::bring::follow(self, workspace, log);
            crate::bring::queue_ahead(self, workspace, queue, log);
            if heard || workspace.revision() != before {
                self.revision += 1;
            }
            self.list_seen = workspace.revision();
        }
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

    /// Every class this app has a name for. The instrument declares its own.
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
                unit: Some(pretend_allocation_unit(261_632)),
            },
            Partition {
                class: ObjectClass::Sample,
                name: "Samp Lib".into(),
                native: false,
                unit: Some(pretend_allocation_unit(131_064)),
            },
            Partition {
                class: ObjectClass::Unknown(9),
                name: "Rhythms".into(),
                native: false,
                unit: None,
            },
        ]
    }

    /// A partition this app cannot name is included under the device's name for it and is
    /// read-only. A `(Native)` row is left out.
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

        for class in device.state.classes() {
            assert!(device.state.scan.progress(class).is_some(), "{class:?}");
        }
        assert!(device.state.scan.progress(ObjectClass::Program).is_none());

        device.pretend(DeviceEvent::Disconnected { lost: false });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut Queue::default());
        assert!(device.state.classes().is_empty());
    }

    /// A write or delete rescans its bank, and the rescan's counters replace the folder's
    /// total without restarting its walk.
    #[test]
    fn a_rescan_moves_the_folder_total_and_leaves_the_walk_alone() {
        let ctx = egui::Context::default();
        let mut device = Device::new(ctx.clone());
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let class = ObjectClass::Program;
        let counted = |count| Status {
            class,
            count,
            free: 400 - count,
            used: count,
            dirty: 0,
            spare: 0,
        };

        device.pretend(DeviceEvent::ClassStatus {
            class,
            status: counted(396),
            banks: Some(8),
        });
        device.pretend(DeviceEvent::ClassStatus {
            class,
            status: counted(397),
            banks: None,
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut Queue::default());

        assert_eq!(
            occupancy(class, &device.state.inventory, None).as_deref(),
            Some("397/400")
        );
        assert_eq!(
            device
                .state
                .scan
                .progress(class)
                .and_then(|walk| walk.total),
            Some(8)
        );
    }

    #[test]
    fn connecting_without_usb_says_so_and_never_starts_looking() {
        let mut device = Device::new(egui::Context::default());
        let mut log = Log::default();
        device.pretend_no_usb();

        device.connect(&mut log);
        assert!(
            matches!(device.state.connection, Connection::Disconnected),
            "a connection that cannot start must not be left looking"
        );
        assert_eq!(log.status(), (crate::log::Level::Error, NO_USB));
    }

    /// The unit a count is measured in is the partition's own, so it arrives with the
    /// table and is forgotten when the instrument disconnects.
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

    /// A library counts blocks, not bytes, and one block is its partition's allocation
    /// unit. Until the unit arrives the row shows only the item count; a block count read
    /// as bytes would be off by five orders of magnitude.
    #[test]
    fn a_library_reads_in_bytes_only_once_its_allocation_unit_has_arrived() {
        let class = ObjectClass::Sample;
        // The shape an Electro 5 answers with: 1,472 of the partition's 1,536 blocks
        // in use, each 131,064 net bytes.
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
            occupancy(class, &inventory, Some(pretend_allocation_unit(131_064))).as_deref(),
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

    /// The Live and Settings partitions hold less than a megabyte and would read 0/0 in
    /// megabytes.
    #[test]
    fn a_partition_reads_in_the_unit_its_total_calls_for() {
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
        let byte = Some(pretend_allocation_unit(1));

        let live = ObjectClass::Live;
        assert_eq!(
            occupancy(live, &partition(live, 121, 379), byte).as_deref(),
            Some("121/500 B")
        );

        // The used figure takes the total's unit: 500 bytes alone would read in bytes
        // and look larger than the 24 kB it sits inside.
        let settings = ObjectClass::Settings;
        assert_eq!(
            occupancy(settings, &partition(settings, 500, 24_076), byte).as_deref(),
            Some("0.5/24.0 kB")
        );

        let samples = ObjectClass::Sample;
        assert_eq!(
            occupancy(
                samples,
                &partition(samples, 128, 128),
                Some(pretend_allocation_unit(1024 * 1024))
            )
            .as_deref(),
            Some("128.0/256.0 MB")
        );
    }

    /// A folder emptied on the instrument must not keep showing its old names, and must
    /// not look as if it was never read.
    #[test]
    fn a_bank_that_scans_as_empty_replaces_what_it_held() {
        let ctx = egui::Context::default();
        let mut device = Device::new(ctx.clone());
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let class = ObjectClass::Sample;

        device.pretend_scanned(class, 1, &["Marimba"]);
        assert_eq!(device.state.bank(class, 1).map(<[_]>::len), Some(1));

        device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 1,
            slots: Vec::new(),
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut Queue::default());
        let slots = device.state.bank(class, 1).expect("the bank was read");
        assert!(slots.is_empty(), "read and empty, not never read");
    }

    /// A bank index of `u32::MAX` has no panel number, and is refused instead of wrapping
    /// onto another bank's cache.
    #[test]
    fn a_bank_index_with_no_panel_number_is_refused() {
        let ctx = egui::Context::default();
        let mut device = Device::new(ctx);
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 1, &["Africa Split"]);
        device.state.geometry.insert(
            class,
            vec![Bank {
                index: u32::MAX,
                name: "Nowhere".into(),
                slots: 1,
            }],
        );

        assert!(device
            .state
            .slot(class, Location { bank: 0, slot: 0 })
            .is_some());
        assert!(device
            .state
            .slot(
                class,
                Location {
                    bank: u32::MAX,
                    slot: 0
                }
            )
            .is_none());
        assert_eq!(device.state.bank_name(class, 0), None);
    }

    #[test]
    fn a_settings_write_warns_that_the_panel_reloads() {
        let why = write_warning(ObjectClass::Settings).expect("must warn");
        assert!(why.contains("reloads the selected program"), "{why}");
        for class in named().filter(|class| *class != ObjectClass::Settings) {
            assert!(write_warning(class).is_none(), "{}", folder(class));
        }
    }

    /// What landed is no longer owed, and nothing else is touched: a batch that stops
    /// halfway leaves the rest of the queue as it was.
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
            let bytes = workspace.get(id).unwrap().bytes.to_vec();
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
            name: "Sent.ne5p".into(),
            sent: Payload::Bytes(workspace.get(landed).unwrap().bytes.to_vec()),
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        assert!(!queue.holds(landed), "it was written");
        assert!(queue.holds(still_owed), "still waiting");
        assert_eq!(queue.ids(), vec![still_owed]);
    }

    /// A file that lands after its asset was saved as something else leaves the asset
    /// saved as the file: what the slot holds. The edit saved meanwhile is still on this
    /// computer, now unsaved against the file.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_file_that_lands_after_another_save_is_what_its_asset_is_saved_as() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        device.pretend_attached();
        let (mut log, mut tabs, mut queue) = (Log::default(), Tabs::default(), Queue::default());
        let dir = crate::testing::Temp::new();
        let bytes = crate::testing::piano(4);
        let file = crate::testing::on_disk(&dir, "Upright.npno", &bytes);
        let id = crate::testing::rest(&mut workspace, "Upright.npno", file.clone());
        workspace.settle_files(&mut log);
        let crc32 = workspace.get(id).unwrap().saved.crc32().unwrap();
        let sent = Payload::of(workspace.get(id).unwrap()).unwrap();

        // The last byte of its audio, changed, under a checksum that agrees.
        let mut edited = bytes.clone();
        let last = edited.len() - 1;
        edited[last] ^= 1;
        let v1 = nord_format::cbin::Generation::V1;
        let body = nord_format::crc::crc32(&edited[v1.body_start() as usize..]);
        let stored = v1.checksum_range(edited.len()).unwrap();
        edited[stored].copy_from_slice(&body.to_le_bytes());
        workspace.replace_bytes(id, edited.clone(), &mut log);
        workspace.mark_saved(id);
        assert!(!workspace.get(id).unwrap().is_unsaved());

        let at = Location::from_user(1, 1);
        device.pretend(DeviceEvent::Sent {
            id,
            class: ObjectClass::Piano,
            at,
            name: "Upright.npno".into(),
            sent,
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        let entity = workspace.get(id).unwrap();
        assert_eq!(entity.bytes, edited, "the edit is kept");
        assert!(entity.is_unsaved(), "and is not what the slot holds");
        assert!(entity
            .saved
            .file()
            .is_some_and(|held| Arc::ptr_eq(held, &file)));
        assert_eq!(entity.saved.crc32(), Some(crc32));
        let keyboard = device.state.keyboard().cloned().unwrap();
        let held = entity.held_on(&keyboard).map(|held| held.spot.clone());
        assert_eq!(held, Some(Spot::Named("Upright".into())));
        assert_eq!(file.take_reads(), [], "nothing read the file");
    }

    /// One program on this computer, and the body checksum a walk would report for the
    /// slot holding it.
    fn program(workspace: &mut Workspace, log: &mut Log, origin: Origin) -> (u64, u32) {
        let made = workspace
            .create(crate::workspace::Fresh::Program, log)
            .unwrap();
        let bytes = workspace.get(made).unwrap().bytes.to_vec();
        workspace.remove(made, log);
        let id = workspace.ingest("Africa-Split.ne5p".into(), origin, bytes, log);
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one");
        (id, crc)
    }

    /// An asset of another family, on this computer.
    fn stage(workspace: &mut Workspace, log: &mut Log, origin: Origin) -> (u64, u32) {
        let made = workspace
            .create(crate::workspace::Fresh::Stage4Program, log)
            .unwrap();
        let bytes = workspace.get(made).unwrap().bytes.to_vec();
        workspace.remove(made, log);
        let id = workspace.ingest("Africa-Split.ns4p".into(), origin, bytes, log);
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32())
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
        let shared = workspace.ingest(
            "Pad.nsmp".into(),
            Origin::Fresh,
            include_bytes!("../../../nord-format/tests/fixtures/demo/drawbar-pad.nsmp").to_vec(),
            &mut log,
        );
        fn held(device: &Device, workspace: &Workspace, id: u64) -> Fit {
            fit(&device.state, workspace.get(id).expect("it is on the list"))
        }
        let held = |device: &Device, id| held(device, &workspace, id);

        assert_eq!(held(&device, own), Fit::Unattached);

        device.pretend_attached();
        assert_eq!(held(&device, own), Fit::Takes, "confirmed on hardware");
        assert_eq!(held(&device, shared), Fit::Takes, "confirmed on hardware");
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
        match held(&device, shared) {
            Fit::Warn(why) => assert!(why.contains("untried"), "{why}"),
            answer => panic!("a sample on a Stage 4, which nothing was written to: {answer:?}"),
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

    /// A write is the strongest evidence of where an asset stands, because this app put
    /// the bytes there. The read after it decides the row's sign but cannot unlink the
    /// asset.
    #[test]
    fn a_send_links_its_file_to_the_slot_and_a_later_walk_says_how_they_compare() {
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

            let sent = workspace.get(id).expect("it is on the list").bytes.to_vec();
            device.pretend(DeviceEvent::Sent {
                id,
                class,
                at,
                name: "Sent.ne5p".into(),
                sent: Payload::Bytes(sent),
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
                            modified: None,
                            name: "Africa Split".into(),
                        }),
                    ],
                });
                device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
            }
            // A second frame with nothing to report: the log is not a per-frame render.
            device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
            let entity = workspace.get(id).expect("it is on the list");
            let linked = standing_of(&device.state, entity).expect("linked where it landed");
            assert_eq!(linked.at, Some(at));
            (linked.standing, log.transcript(), crc)
        };

        let (standing, said, _) = sent(Rescan::Same);
        assert_eq!(standing, Standing::Same);
        assert!(!said.contains("crc32"), "the two agree: {said}");

        let (standing, said, crc) = sent(Rescan::Other);
        assert_eq!(standing, Standing::Theirs, "the keyboard changed it since");
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

        let (standing, _, _) = sent(Rescan::Silent);
        assert_eq!(
            standing,
            Standing::Same,
            "a slot of the length sent that reports no checksum holds what was sent"
        );

        let (standing, _, _) = sent(Rescan::Skipped);
        assert_eq!(
            standing,
            Standing::Unread,
            "a bank nothing has read says neither way"
        );
    }

    /// A write takes as long as the instrument takes, and an edit made while one is in
    /// flight is on this computer alone. What landed is what the asset is saved as; what
    /// it holds now is still unsaved, and closing the tab would lose it.
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
        let sent = workspace.get(id).expect("it is on the list").bytes.to_vec();

        let (_, edited) = crate::fields::apply(&sent, &[("center_panel.gain".into(), "96".into())])
            .expect("the registry takes the set");
        workspace.replace_bytes(id, edited.clone(), &mut log);

        device.pretend(DeviceEvent::Sent {
            id,
            class,
            at,
            name: "Sent.ne5p".into(),
            sent: Payload::Bytes(sent.clone()),
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        let entity = workspace.get(id).expect("it is on the list");
        assert_eq!(*entity.saved.bytes(), sent, "the instrument holds these");
        assert_eq!(entity.bytes, edited);
        assert!(entity.is_unsaved(), "the edit was never sent");
        let keyboard = device.state.keyboard().cloned().unwrap();
        let held = entity.held_on(&keyboard).map(|held| held.location());
        assert_eq!(held, Some(Some(at)));
        assert!(!queue.holds(id), "it landed");
    }

    /// A write that lands owes the panel a reload of the slots it wrote, which the
    /// worker checks against where the panel is when it runs. A refused write owes
    /// nothing, and neither does a class whose panel loads no slots.
    #[test]
    fn a_write_that_lands_owes_the_panel_a_reload_and_a_refused_one_does_not() {
        let class = ObjectClass::Program;
        let at = Location { bank: 4, slot: 2 };

        let owed = |focus: bool, refused: Option<DeviceEvent>| {
            let ctx = egui::Context::default();
            let mut workspace = Workspace::new(ctx.clone());
            let mut device = Device::new(ctx);
            let mut log = Log::default();
            device.pretend_attached();
            if focus {
                device.pretend_focused(class, Location { bank: 0, slot: 0 });
            }
            device.send(
                DeviceCmd::Put {
                    id: 1,
                    class,
                    at,
                    name: "Africa Split".into(),
                    payload: Payload::Bytes(vec![0; 4]),
                    expect: Expect::Vacant,
                },
                &mut log,
            );
            device.pump();
            if let Some(refused) = refused {
                device.pretend(refused);
            }
            device.pretend(DeviceEvent::Finished);
            device.poll(
                &mut log,
                &mut workspace,
                &mut Tabs::default(),
                &mut Queue::default(),
            );
            device.queued().into_iter().find_map(|cmd| match cmd {
                DeviceCmd::Reload { class, written } => Some((*class, written.clone())),
                _ => None,
            })
        };

        let failed = || Some(DeviceEvent::OpFailed("put: refused".into()));
        let changed = || {
            let (id, holds) = (1, None);
            Some(DeviceEvent::Changed {
                id,
                class,
                at,
                holds,
            })
        };
        assert_eq!(owed(true, None), Some((class, vec![at])));
        assert_eq!(
            owed(true, failed()),
            None,
            "the refused write changed nothing"
        );
        assert_eq!(
            owed(true, changed()),
            Some((class, vec![])),
            "the slot that changed on the keyboard keeps what the panel plays"
        );
        assert_eq!(owed(false, None), None, "this panel loads no programs");
    }

    /// A program written back changes what it needs, so the cached answer does not
    /// outlive the write; a write elsewhere leaves it.
    #[test]
    fn a_write_forgets_what_its_slot_needed() {
        let program = ObjectClass::Program;
        let (written, beside) = (Location { bank: 6, slot: 0 }, Location { bank: 6, slot: 1 });
        let plays = |id| {
            vec![Dependency {
                flag: 1,
                class: ObjectClass::Piano,
                id,
                name: format!("Grand {id}"),
                location: None,
            }]
        };
        let after_a_put = |class, at| {
            let mut device = Device::new(egui::Context::default());
            device.pretend_attached();
            device.pretend_deps(program, written, plays(1));
            device.pretend_deps(program, beside, plays(2));
            let put = DeviceCmd::Put {
                id: 1,
                class,
                at,
                name: "Africa Split".into(),
                payload: Payload::Bytes(vec![0; 4]),
                expect: Expect::Vacant,
            };
            device.send(put, &mut Log::default());
            device.pump();
            device.state
        };

        let state = after_a_put(program, written);
        assert_eq!(state.needs_of(program, written), None);
        assert_eq!(
            state.needs_of(program, beside),
            Some(&[nord_format::bundle::Key::Piano(2)][..])
        );
        let state = after_a_put(ObjectClass::Piano, Location { bank: 0, slot: 0 });
        assert!(state.needs_of(program, written).is_some());
    }

    #[test]
    fn a_move_within_one_bank_reads_that_bank_again_once() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        device.pretend_attached();
        let class = ObjectClass::Program;
        device.send(
            DeviceCmd::Move {
                class,
                from: Location { bank: 4, slot: 2 },
                to: Location { bank: 4, slot: 7 },
            },
            &mut log,
        );
        device.pump();
        device.pretend(DeviceEvent::Finished);
        device.poll(
            &mut log,
            &mut workspace,
            &mut Tabs::default(),
            &mut Queue::default(),
        );
        let rescans: Vec<(ObjectClass, u32)> = device
            .queued()
            .into_iter()
            .filter_map(|cmd| match cmd {
                DeviceCmd::ScanBank { class, bank } => Some((*class, *bank)),
                _ => None,
            })
            .collect();
        assert_eq!(rescans, vec![(class, 5)]);
    }

    /// The read of the banks a duplicate touched runs after it, and the status line
    /// still says where the copy went.
    #[test]
    fn the_reread_after_a_duplicate_keeps_where_the_copy_went() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        device.pretend_attached();
        let finish = |device: &mut Device, log: &mut Log, workspace: &mut Workspace| {
            device.pump();
            device.pretend(DeviceEvent::OpOk("done".into()));
            device.pretend(DeviceEvent::Finished);
            device.poll(log, workspace, &mut Tabs::default(), &mut Queue::default());
        };
        device.send(
            DeviceCmd::Duplicate {
                class: ObjectClass::Program,
                from: Location::from_user(7, 15),
                to: Location::from_user(5, 46),
            },
            &mut log,
        );
        finish(&mut device, &mut log, &mut workspace);
        let copied = "Copied Programs 7:15 to Programs 5:46.";
        assert_eq!(log.status().1, copied);

        while !device.queued().is_empty() {
            finish(&mut device, &mut log, &mut workspace);
        }
        assert_eq!(log.status().1, copied);
    }

    /// The status strip names places and things; the protocol line keeps the verbs.
    #[test]
    fn the_status_sentences_never_quote_the_protocol() {
        let cmd = DeviceCmd::Put {
            id: 1,
            class: ObjectClass::Program,
            at: Location { bank: 6, slot: 3 },
            name: "Africa Split".into(),
            payload: Payload::Bytes(Vec::new()),
            expect: Expect::Vacant,
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

    fn linked_at(class: ObjectClass, at: Location, name: &str, len: u32) -> Held {
        Held::new(class, at, name, crate::link::Print { crc: 7, len }).unwrap()
    }

    #[test]
    fn a_slot_kept_by_its_address_is_unread_vacant_or_what_a_walk_reported() {
        let mut device = Device::new(egui::Context::default());
        let program = |slot| linked_at(ObjectClass::Program, Location { bank: 0, slot }, "", 121);
        let at = |slot| Some(Location { bank: 0, slot });
        assert_eq!(there(&device.state, &program(0)), (at(0), There::Unread));
        device.pretend_bodies(ObjectClass::Program, 1, &[Some(("Tine", 9)), None]);
        let holds = There::Holds {
            crc: Some(9),
            len: 121,
        };
        assert_eq!(there(&device.state, &program(0)), (at(0), holds));
        assert_eq!(there(&device.state, &program(1)), (at(1), There::Vacant));
    }

    #[test]
    fn a_slot_kept_by_its_name_is_found_anywhere_and_vacant_once_every_bank_is_read() {
        let mut device = Device::new(egui::Context::default());
        let piano =
            |name: &str| linked_at(ObjectClass::Piano, Location { bank: 0, slot: 0 }, name, 900);
        device.pretend_geometry(ObjectClass::Piano, &[("Grand", 4), ("Upright", 4)]);
        device.pretend_lengths(ObjectClass::Piano, 2, &[None, Some((" Yamaha ", 900))]);
        let found = (
            Some(Location { bank: 1, slot: 1 }),
            There::Holds {
                crc: None,
                len: 900,
            },
        );
        assert_eq!(there(&device.state, &piano("Yamaha")), found);
        let absent = there(&device.state, &piano("Steinway"));
        assert_eq!(absent, (None, There::Unread), "a bank is unread");
        device.pretend_lengths(ObjectClass::Piano, 1, &[None]);
        assert_eq!(
            there(&device.state, &piano("Steinway")),
            (None, There::Vacant)
        );
    }

    #[test]
    fn an_asset_stands_against_its_slot_only_on_the_keyboard_it_is_linked_on() {
        let ctx = egui::Context::default();
        let mut device = Device::new(ctx.clone());
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let program = crate::workspace::Fresh::Program.bytes().unwrap();
        let id = workspace.ingest("A.ne5p".into(), Origin::Fresh, program, &mut log);
        workspace.settle_files(&mut log);
        let print = workspace.get(id).unwrap().saved_print().expect("read");
        let held = Held::new(
            ObjectClass::Program,
            Location { bank: 0, slot: 0 },
            "",
            print,
        )
        .unwrap();
        let elsewhere = Keyboard {
            model: "Nord Electro 5".into(),
            serial: Some("B2".into()),
        };
        workspace.link(id, &elsewhere, held.clone());
        let standing = |device: &Device, workspace: &Workspace| {
            standing_of(&device.state, workspace.get(id).unwrap())
        };
        assert_eq!(standing(&device, &workspace), None, "nothing attached");

        device.pretend_bodies(ObjectClass::Program, 1, &[Some(("A", print.crc))]);
        assert_eq!(standing(&device, &workspace), None, "linked elsewhere");
        let here = device.state.keyboard().cloned().expect("attached");
        workspace.link(id, &here, held);
        let linked = Linked {
            class: ObjectClass::Program,
            at: Some(Location { bank: 0, slot: 0 }),
            there: There::Holds {
                crc: Some(print.crc),
                len: 121,
            },
            standing: Standing::Same,
        };
        assert_eq!(standing(&device, &workspace), Some(linked));
    }

    fn heard(bench: &mut crate::testing::Bench) {
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
    }

    fn body() -> Vec<u8> {
        crate::workspace::Fresh::Program.bytes().unwrap()
    }

    fn gained(gain: &str) -> Vec<u8> {
        let gain = [("center_panel.gain".into(), gain.into())];
        crate::fields::apply(&body(), &gain).unwrap().1
    }

    fn print(bytes: &[u8]) -> Print {
        crate::workspace::print_of(bytes).unwrap()
    }

    fn held_at(bench: &crate::testing::Bench, id: u64) -> Option<Spot> {
        let keyboard = bench.device.state.keyboard().cloned().expect("attached");
        let entity = bench.workspace.get(id).unwrap();
        entity.held_on(&keyboard).map(|held| held.spot.clone())
    }

    fn at(slot: u32) -> Location {
        Location::from_user(1, slot)
    }

    fn file(bench: &mut crate::testing::Bench, name: &str, bytes: Vec<u8>) -> u64 {
        let origin = Origin::File(name.into());
        let id = bench
            .workspace
            .ingest(name.into(), origin, bytes, &mut bench.log);
        bench.workspace.settle_files(&mut bench.log);
        id
    }

    #[test]
    fn a_copy_is_linked_to_its_slot_and_a_second_copy_of_it_is_not() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench
            .device
            .pretend_bodies(class, 1, &[Some(("Tine", print(&body()).crc))]);
        for _ in 0..2 {
            bench.device.pretend(DeviceEvent::Got {
                name: "Tine.ne5p".into(),
                origin: Origin::Device { class, at: at(1) },
                bytes: body(),
                why: Purpose::Copy,
            });
        }
        heard(&mut bench);
        let ids: Vec<u64> = bench.workspace.listed().map(|entity| entity.id).collect();
        let [first, second] = ids.as_slice() else {
            panic!("{} copies", ids.len())
        };
        assert_eq!(held_at(&bench, *first), Some(Spot::at(at(1))));
        assert_eq!(
            held_at(&bench, *second),
            None,
            "the slot is the first copy's"
        );
    }

    #[test]
    fn a_send_links_the_file_to_the_slot_it_reached_in_place_of_any_other() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_attached();
        let first = file(&mut bench, "A.ne5p", body());
        let second = file(&mut bench, "B.ne5p", gained("40"));
        let sent = |bench: &mut crate::testing::Bench, id, slot, bytes| {
            bench.device.pretend(DeviceEvent::Sent {
                id,
                class,
                at: at(slot),
                name: "Sent.ne5p".into(),
                sent: Payload::Bytes(bytes),
            });
            heard(bench);
        };
        sent(&mut bench, first, 1, body());
        assert_eq!(held_at(&bench, first), Some(Spot::at(at(1))));
        sent(&mut bench, second, 1, gained("40"));
        assert_eq!(
            held_at(&bench, first),
            None,
            "the slot holds the second now"
        );
        assert_eq!(held_at(&bench, second), Some(Spot::at(at(1))));
        sent(&mut bench, second, 2, gained("40"));
        assert_eq!(
            held_at(&bench, second),
            Some(Spot::at(at(2))),
            "one link per keyboard"
        );

        bench
            .workspace
            .replace_bytes(second, gained("41"), &mut bench.log);
        bench.workspace.mark_saved(second);
        sent(&mut bench, second, 2, gained("41"));
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let held = bench
            .workspace
            .get(second)
            .unwrap()
            .held_on(&keyboard)
            .unwrap();
        assert_eq!(held.base, print(&gained("41")));
        assert!(
            held.knew(print(&gained("40")).crc),
            "the slot's earlier version"
        );
    }

    fn name_held(bench: &crate::testing::Bench, id: u64) -> Option<String> {
        let keyboard = bench.device.state.keyboard().cloned().expect("attached");
        let held = bench.workspace.get(id)?.held_on(&keyboard)?;
        held.name().map(str::to_string)
    }

    #[test]
    fn a_send_records_the_name_its_write_gave_the_slot() {
        let mut bench = crate::testing::Bench::new();
        bench.device.pretend_attached();
        let id = file(&mut bench, "Clav.ne5p", body());
        let sent = |bench: &mut crate::testing::Bench, name: &str, bytes| {
            bench.device.pretend(DeviceEvent::Sent {
                id,
                class: ObjectClass::Program,
                at: at(1),
                name: name.into(),
                sent: Payload::Bytes(bytes),
            });
            heard(bench);
        };
        sent(&mut bench, "Tine.ne5p", body());
        assert_eq!(name_held(&bench, id).as_deref(), Some("Tine"));
        bench
            .workspace
            .replace_bytes(id, gained("41"), &mut bench.log);
        bench.workspace.mark_saved(id);
        sent(&mut bench, "Wurl.ne5p", gained("41"));
        assert_eq!(
            name_held(&bench, id).as_deref(),
            Some("Wurl"),
            "agreed again"
        );
    }

    #[test]
    fn a_move_drawbar_makes_swaps_the_links_a_rename_renames_and_a_delete_unlinks() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench
            .device
            .pretend_bodies(class, 1, &[Some(("A", 1)), Some(("B", 2))]);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let [first, second] = ["A.ne5p", "B.ne5p"].map(|name| file(&mut bench, name, body()));
        for (id, slot, name) in [(first, 1, "A"), (second, 2, "B")] {
            let held = Held::new(class, at(slot), name, print(&body())).unwrap();
            bench.workspace.link(id, &keyboard, held);
        }
        let done = |bench: &mut crate::testing::Bench, act| {
            bench.act(vec![act]);
            bench.device.pump();
            bench.device.pretend(DeviceEvent::OpOk("done".into()));
            heard(bench);
        };
        let waiting = file(&mut bench, "C.ne5p", gained("40"));
        crate::queue::enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            waiting,
            class,
            at(1),
        );
        bench.device.pending.clear();
        let swap = crate::browser::Act::Rearrange {
            class,
            from: at(1),
            to: at(2),
        };
        done(&mut bench, swap);
        assert_eq!(held_at(&bench, first), Some(Spot::at(at(2))));
        assert_eq!(held_at(&bench, second), Some(Spot::at(at(1))));
        assert!(
            !bench.queue.holds(waiting),
            "its slot holds another sound now"
        );
        let said = "“C” is no longer waiting to send.";
        assert!(bench.log.iter().any(|entry| entry.text == said));
        bench.act(vec![crate::browser::Act::SendAll]);
        let sends = bench.device.queued().into_iter();
        let mut sends = sends.filter(|cmd| matches!(cmd, DeviceCmd::SendAll { .. }));
        assert!(sends.next().is_none(), "nothing is written to either slot");
        assert_eq!(name_held(&bench, first).as_deref(), Some("A"), "it travels");

        bench.device.state.in_flight = None;
        let name = "Bright".to_string();
        done(
            &mut bench,
            crate::browser::Act::RenameSlot {
                class,
                at: at(1),
                name,
            },
        );
        assert_eq!(held_at(&bench, second), Some(Spot::at(at(1))));
        assert_eq!(name_held(&bench, second).as_deref(), Some("Bright"));

        bench.device.state.in_flight = None;
        done(
            &mut bench,
            crate::browser::Act::DeleteSlot { class, at: at(2) },
        );
        assert_eq!(held_at(&bench, first), None);
        assert_eq!(held_at(&bench, second), Some(Spot::at(at(1))));
    }

    #[test]
    fn deleting_several_pianos_at_once_unlinks_each_file() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Piano;
        let names = ["Tine", "Clav"];
        bench
            .device
            .pretend_lengths(class, 1, &[Some((names[0], 10)), Some((names[1], 10))]);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let ids = names.map(|name| {
            let id = file(&mut bench, &format!("{name}.npno"), body());
            let held = Held::new(
                class,
                Location { bank: 0, slot: 0 },
                name,
                Print { crc: 1, len: 10 },
            );
            let held = held.unwrap();
            bench.workspace.link(id, &keyboard, held);
            id
        });
        bench.act(vec![
            crate::browser::Act::DeleteSlot { class, at: at(1) },
            crate::browser::Act::DeleteSlot { class, at: at(2) },
        ]);
        for _ in names {
            bench.device.pump();
            bench.device.pretend(DeviceEvent::OpOk("deleted".into()));
            bench.device.pretend(DeviceEvent::Finished);
            heard(&mut bench);
        }
        for id in ids {
            assert_eq!(held_at(&bench, id), None, "file {id}");
        }
    }

    #[test]
    fn a_sent_piano_is_linked_by_the_name_its_write_gave_the_slot() {
        let mut bench = crate::testing::Bench::new();
        bench.device.pretend_attached();
        let id = file(&mut bench, "Tine.npno", body());
        bench.workspace.rename(id, "Clav.npno".into());
        bench.device.pretend(DeviceEvent::Sent {
            id,
            class: ObjectClass::Piano,
            at: at(1),
            name: "Tine.npno".into(),
            sent: Payload::Bytes(body()),
        });
        heard(&mut bench);
        assert_eq!(held_at(&bench, id), Some(Spot::Named("Tine".into())));
    }

    #[test]
    fn a_file_is_linked_on_its_own_to_the_slot_it_names_holding_its_body() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        let crc = print(&body()).crc;
        bench.device.pretend_geometry(class, &[("A", 3)]);
        bench
            .device
            .pretend_bodies(class, 1, &[None, Some(("Tine", crc)), None]);
        let origin = Origin::Device { class, at: at(2) };
        let copied =
            bench
                .workspace
                .ingest("Tine.ne5p".into(), origin.clone(), body(), &mut bench.log);
        let new = bench
            .workspace
            .ingest("New.ne5p".into(), Origin::Fresh, body(), &mut bench.log);
        let kept_off =
            bench
                .workspace
                .ingest("Off.ne5p".into(), origin, gained("40"), &mut bench.log);
        bench.workspace.settle_files(&mut bench.log);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        bench.workspace.keep_off(kept_off, &keyboard);
        heard(&mut bench);
        assert_eq!(held_at(&bench, copied), Some(Spot::at(at(2))));
        assert_eq!(
            held_at(&bench, new),
            None,
            "a new file is linked only by a send"
        );
        assert_eq!(held_at(&bench, kept_off), None);

        bench.workspace.unlink(copied, &keyboard);
        bench
            .device
            .pretend_bodies(class, 1, &[None, Some(("Tine", crc)), Some(("Tine", crc))]);
        heard(&mut bench);
        assert_eq!(
            held_at(&bench, copied),
            Some(Spot::at(at(2))),
            "another slot holding its body does not matter"
        );

        bench.workspace.unlink(copied, &keyboard);
        let origin = Origin::Device { class, at: at(2) };
        let twin = bench
            .workspace
            .ingest("Twin.ne5p".into(), origin, body(), &mut bench.log);
        bench.workspace.settle_files(&mut bench.log);
        heard(&mut bench);
        assert_eq!(
            (held_at(&bench, copied), held_at(&bench, twin)),
            (None, None),
            "two files naming one slot link neither"
        );
    }

    #[test]
    fn link_to_offers_the_named_slot_first_even_edited_and_every_free_slot_holding_it() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        let (mine, other) = (gained("40"), body());
        let slots = [
            None,
            Some(("A", print(&other).crc)),
            Some(("Z", print(&mine).crc)),
            Some(("X", print(&mine).crc)),
        ];
        bench.device.pretend_bodies(class, 1, &slots);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let taken = file(&mut bench, "X.ne5p", mine.clone());
        let held = Held::new(class, at(4), "", print(&mine)).unwrap();
        bench.workspace.link(taken, &keyboard, held);
        let origin = Origin::Device { class, at: at(2) };
        let id = bench
            .workspace
            .ingest("A.ne5p".into(), origin, mine, &mut bench.log);
        bench.workspace.settle_files(&mut bench.log);
        let entity = bench.workspace.get(id).unwrap();
        let offers = link_offers(&bench.device.state, &bench.workspace, entity);
        assert_eq!(offers, [(class, at(2)), (class, at(3))]);

        bench.act(vec![crate::browser::Act::Link {
            id,
            class,
            at: at(2),
        }]);
        let entity = bench.workspace.get(id).unwrap();
        let linked = standing_of(&bench.device.state, entity).unwrap();
        assert!(linked.standing.needs_attention(), "{:?}", linked.standing);
    }

    #[test]
    fn a_new_file_is_offered_only_the_slots_holding_its_bytes_whatever_its_header_names() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        let fresh = crate::workspace::Fresh::Program.bytes().unwrap();
        let slots = [
            Some(("untitled", print(&gained("40")).crc)),
            Some(("B", print(&fresh).crc)),
        ];
        bench.device.pretend_bodies(class, 1, &slots);
        let id = bench
            .workspace
            .create(crate::workspace::Fresh::Program, &mut bench.log)
            .unwrap();
        let entity = bench.workspace.get(id).unwrap();
        assert_eq!(
            named_slots(entity, class).next(),
            Some(at(1)),
            "a New program's header names 1:1"
        );

        let state = &bench.device.state;
        let offers = link_offers(state, &bench.workspace, entity);
        assert_eq!(offers, [(class, at(2))]);
        assert_eq!(also_at(state, &bench.workspace, entity), [(class, at(2))]);
    }

    #[test]
    fn also_at_names_only_the_slots_holding_the_files_bytes_and_link_to_its_named_slot_too() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        let (mine, other) = (gained("40"), body());
        let slots = [
            Some(("A", print(&other).crc)),
            Some(("Z", print(&mine).crc)),
        ];
        bench.device.pretend_bodies(class, 1, &slots);
        let origin = Origin::Device { class, at: at(1) };
        let id = bench
            .workspace
            .ingest("A.ne5p".into(), origin, mine, &mut bench.log);
        bench.workspace.settle_files(&mut bench.log);
        let entity = bench.workspace.get(id).unwrap();

        let state = &bench.device.state;
        let offers = link_offers(state, &bench.workspace, entity);
        assert_eq!(offers, [(class, at(1)), (class, at(2))]);
        assert_eq!(also_at(state, &bench.workspace, entity), [(class, at(2))]);
    }

    #[test]
    fn a_stored_link_to_no_slot_this_keyboard_has_counts_as_none() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_partitions(&ELECTRO5);
        bench.device.pretend_geometry(class, &[("A", 2)]);
        bench
            .device
            .pretend_bodies(class, 1, &[Some(("", 1)), None]);
        let id = file(&mut bench, "A.ne5p", body());
        let stored = [
            r#"{"class":999,"spot":{"at":{"bank":0,"slot":0}},"base":{"crc":1,"len":121}}"#,
            r#"{"class":4,"spot":{"named":""},"base":{"crc":1,"len":121}}"#,
            r#"{"class":4,"spot":{"at":{"bank":0,"slot":9}},"base":{"crc":1,"len":121}}"#,
            r#"{"class":4,"spot":{"at":{"bank":5,"slot":0}},"base":{"crc":1,"len":121}}"#,
        ];
        for held in stored {
            let json = format!(r#"{{"keyboard":{{"model":"Nord Electro 5"}},"held":{held}}}"#);
            let link: crate::link::Link = serde_json::from_str(&json).unwrap();
            bench.workspace.follow_links(id, vec![link]);
            let entity = bench.workspace.get(id).unwrap();
            assert_eq!(standing_of(&bench.device.state, entity), None, "{held}");
        }
    }

    /// Files `A` holding `body()` and `B` holding `gained("40")`, linked to slot 1 of bank 1
    /// as `links` says, as two libraries that synced leave them, with the slot holding
    /// `slot` under the name `A`.
    fn claimed(
        bench: &mut crate::testing::Bench,
        slot: Option<u32>,
        links: [Held; 2],
    ) -> (u64, u64) {
        let class = ObjectClass::Program;
        bench
            .device
            .pretend_bodies(class, 1, &[slot.map(|crc| ("A", crc))]);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let [first, second] = [("A.ne5p", body()), ("B.ne5p", gained("40"))]
            .map(|(name, bytes)| file(bench, name, bytes));
        for (id, held) in [first, second].into_iter().zip(links) {
            let link = crate::link::Link {
                keyboard: keyboard.clone(),
                held: Some(held),
            };
            bench.workspace.follow_links(id, vec![link]);
        }
        heard(bench);
        (first, second)
    }

    fn agreed(name: &str, base: &[u8]) -> Held {
        Held::new(ObjectClass::Program, at(1), name, print(base)).unwrap()
    }

    /// Which of `A` and `B`, claiming one slot as `links` says, keeps it.
    fn keeps(slot: &[u8], links: [Held; 2]) -> [Option<Spot>; 2] {
        let mut bench = crate::testing::Bench::new();
        let (first, second) = claimed(&mut bench, Some(print(slot).crc), links);
        [held_at(&bench, first), held_at(&bench, second)]
    }

    #[test]
    fn of_two_files_linked_to_one_slot_every_computer_keeps_the_same_one() {
        let (linked, off) = (Some(Spot::at(at(1))), None);
        let both = || [agreed("", &body()), agreed("", &body())];
        assert_eq!(keeps(&gained("40"), both()), [off.clone(), linked.clone()]);
        assert_eq!(keeps(&body(), both()), [linked.clone(), off.clone()]);

        let other = gained("42");
        let knows = keeps(&other, [agreed("", &other), agreed("", &gained("41"))]);
        assert_eq!(
            knows,
            [linked.clone(), off.clone()],
            "the slot holds A's base"
        );
        let neither = gained("43");
        let named = keeps(&neither, [agreed("", &body()), agreed("A", &body())]);
        assert_eq!(named, [off.clone(), linked.clone()], "the slot is named A");

        let lesser = agreed("", &gained("44"));
        let greater = agreed("", &gained("45"));
        let (lesser, greater) = match lesser < greater {
            true => (lesser, greater),
            false => (greater, lesser),
        };
        let ordered = keeps(&neither, [lesser.clone(), greater.clone()]);
        assert_eq!(ordered, [off.clone(), linked.clone()]);
        assert_eq!(keeps(&neither, [greater, lesser]), [linked, off]);
    }

    #[test]
    fn a_slot_two_files_claim_is_settled_once_read_and_undo_links_the_other_back() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_attached();
        bench.device.pretend_geometry(class, &[("A", 1), ("B", 1)]);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let slot = Location::from_user(2, 1);
        let held = Held::new(class, slot, "", print(&body())).unwrap();
        let [first, second] = [("A.ne5p", body()), ("B.ne5p", gained("40"))]
            .map(|(name, bytes)| file(&mut bench, name, bytes));
        for id in [first, second] {
            let link = crate::link::Link {
                keyboard: keyboard.clone(),
                held: Some(held.clone()),
            };
            bench.workspace.follow_links(id, vec![link]);
        }
        heard(&mut bench);
        let linked =
            |bench: &crate::testing::Bench| [first, second].map(|id| held_at(bench, id).is_some());
        assert_eq!(linked(&bench), [true, true], "bank 2 is unread");

        let info = ProgramInfo {
            location: slot,
            body_len: 121,
            format: "ne5p".into(),
            version: 4,
            crc32: Some(print(&body()).crc),
            modified: None,
            name: "A".into(),
        };
        bench.device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 2,
            slots: vec![Some(info)],
        });
        heard(&mut bench);
        assert_eq!(linked(&bench), [true, false]);
        assert!(bench
            .log
            .press("1 file unlinked from a slot another file is linked to."));
        bench.log.undo_pressed(
            &mut bench.workspace,
            &mut bench.queue,
            &mut bench.device.asks,
        );
        heard(&mut bench);
        assert_eq!(linked(&bench), [false, true], "the other one, in its place");
    }

    #[test]
    fn a_link_takes_the_name_its_slot_holds_its_version_under_and_keeps_its_own_otherwise() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_attached();
        bench.device.pretend_geometry(class, &[("A", 2)]);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let [same, edited] = ["A.ne5p", "B.ne5p"].map(|name| file(&mut bench, name, body()));
        for (id, slot) in [(same, 1), (edited, 2)] {
            let held = Held::new(class, at(slot), "Tine", print(&body())).unwrap();
            bench.workspace.link(id, &keyboard, held);
        }
        let info = |slot, crc| ProgramInfo {
            location: at(slot),
            body_len: 121,
            format: "ne5p".into(),
            version: 4,
            crc32: Some(crc),
            modified: None,
            name: "Tine Mk".into(),
        };
        let slots = vec![
            Some(info(1, print(&body()).crc)),
            Some(info(2, print(&gained("40")).crc)),
        ];
        bench.device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 1,
            slots,
        });
        heard(&mut bench);
        let name = |id| {
            let entity = bench.workspace.get(id).unwrap();
            entity
                .held_on(&keyboard)
                .unwrap()
                .name()
                .map(str::to_string)
        };
        assert_eq!(name(same).as_deref(), Some("Tine Mk"));
        assert_eq!(
            name(edited).as_deref(),
            Some("Tine"),
            "its slot holds another version"
        );
    }

    #[test]
    fn a_duplicate_has_no_link_and_sending_it_elsewhere_leaves_the_original_where_it_was() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_attached();
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let original = file(&mut bench, "A.ne5p", body());
        let held = Held::new(class, at(1), "", print(&body())).unwrap();
        bench.workspace.link(original, &keyboard, held);
        let copy = bench.workspace.duplicate(original, &mut bench.log).unwrap();
        assert_eq!(held_at(&bench, copy), None);

        bench.device.pretend(DeviceEvent::Sent {
            id: copy,
            class,
            at: at(2),
            name: "Sent.ne5p".into(),
            sent: Payload::Bytes(body()),
        });
        heard(&mut bench);
        assert_eq!(held_at(&bench, copy), Some(Spot::at(at(2))));
        assert_eq!(held_at(&bench, original), Some(Spot::at(at(1))));
    }

    #[test]
    fn another_keyboard_sees_no_link_and_is_sent_nothing_queued_for_this_one() {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_scanned(class, 1, &["", ""]);
        let id = file(&mut bench, "A.ne5p", body());
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let held = Held::new(class, at(1), "", print(&body())).unwrap();
        bench.workspace.link(id, &keyboard, held);
        crate::queue::enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            id,
            class,
            at(1),
        );
        bench.device.pending.clear();

        bench.device.pretend_serial("B2");
        let entity = bench.workspace.get(id).unwrap();
        assert_eq!(standing_of(&bench.device.state, entity), None);
        crate::queue::refit(
            &bench.workspace,
            &bench.device.state,
            &mut bench.queue,
            &mut bench.log,
        );
        let why = bench.queue.entry(id).and_then(|held| held.failure.clone());
        assert_eq!(why.as_deref(), Some("It was queued for another keyboard."));
        bench.act(vec![crate::browser::Act::SendAll]);
        assert!(
            !bench
                .device
                .queued()
                .into_iter()
                .any(|cmd| matches!(cmd, DeviceCmd::SendAll { .. })),
            "nothing is written"
        );
        assert!(bench.queue.holds(id), "it keeps its place");

        let other = bench.device.state.keyboard().cloned().unwrap();
        bench
            .device
            .pretend_bodies(class, 1, &[Some(("A", print(&body()).crc)), None]);
        bench.device.pretend_serial("B2");
        let held = Held::new(class, at(1), "", print(&body())).unwrap();
        bench.workspace.link(id, &other, held);
        let gain = [("center_panel.gain".into(), "40".into())];
        let edited = crate::fields::apply(&body(), &gain).unwrap().1;
        bench.workspace.replace_bytes(id, edited, &mut bench.log);
        bench.act(vec![crate::browser::Act::SaveDoc(id)]);
        crate::queue::refit(
            &bench.workspace,
            &bench.device.state,
            &mut bench.queue,
            &mut bench.log,
        );
        let entry = bench.queue.entry(id).unwrap();
        assert_eq!(
            (entry.keyboard.as_ref(), entry.failure.as_deref()),
            (Some(&other), None),
            "saving queues it for the keyboard attached now"
        );
    }
}
