//! The device side of the vendor protocol, as a state machine.
//!
//! Frames go in with [`EmuDevice::receive`] and replies come out of
//! [`EmuDevice::take_response`]. Nothing here does I/O; [`crate::EmuTransport`] is the
//! part that pretends to be a byte pipe.
//!
//! Replies are **queued, not returned**, because the host does not always read one
//! before writing again: NSM sends its progress percentage between `WRITE_DATA` and the
//! read of that command's reply.

use std::collections::VecDeque;

use nord_usb::error::Result;
use nord_usb::wire::{cmd, ui, Location, Message, ObjectClass, Service};

use crate::store::{status, Focus, Object, Partition};

/// Subsystem the file-transfer service is addressed with.
///
/// Behaves like a **protocol version**, not an address: 8, 9 and 10 are interchangeable,
/// 11 and 0 are dropped, and 1 stalls the bulk endpoints. Confirmed on hardware.
pub const SUBSYSTEM: u32 = 10;

/// The window of subsystem values the device answers on. 8, 9 and 10 are interchangeable
/// — which is the argument that the field is a version and not an address — and anything
/// outside it is dropped or worse.
const SUBSYSTEM_WINDOW: std::ops::RangeInclusive<u32> = 8..=10;

/// ⚠️ Subsystem 1 on the file-transfer service stalls the bulk endpoints. Never sweep
/// this field downward: the recovery frame cannot be written either, so only a power
/// cycle clears it.
const STALLING_SUBSYSTEM: u32 = 1;

/// ⚠️ The other frame observed to stall the endpoints: command `0x09` on the UI service.
const UI_STALL: u32 = 0x09;

/// Statuses for shapes the corpus and the hardware notes do not pin down.
///
/// Every field here is a **guess with a defensible neighbour** — the status a
/// closely-related refusal is known to use — so that the emulator answers something
/// rather than inventing protocol. Change one to match an instrument that differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unmodeled {
    /// `DELETE` of a slot that holds nothing. NSM never sends it.
    pub delete_empty: u32,
    /// `RENAME` of a slot that holds nothing.
    pub rename_empty: u32,
    /// `SELECT` of a slot that holds nothing.
    pub select_empty: u32,
    /// `MOVE` or `COPY` whose **source** holds nothing.
    pub source_empty: u32,
    /// A transfer command out of step — `WRITE_DATA` with no `BEGIN_WRITE`, a `READ`
    /// range past the end of the body.
    pub bad_state: u32,
    /// A class-scoped command with no session open. NSM always opens one first.
    pub no_session: u32,
    /// What an unrecognised command draws. `None` — the default — is silence, which is
    /// what several probed codes did on hardware; nothing distinguishes "unimplemented"
    /// from "ignored" at the protocol level.
    pub unknown_command: Option<u32>,
}

impl Default for Unmodeled {
    fn default() -> Self {
        Self {
            delete_empty: status::EMPTY,
            rename_empty: status::EMPTY,
            select_empty: status::EMPTY,
            source_empty: status::EMPTY,
            bad_state: status::BAD_ARGUMENTS,
            no_session: status::STALE_SESSION,
            unknown_command: None,
        }
    }
}

/// A transfer opened by `BEGIN_READ` or `BEGIN_WRITE` and closed by `END_TRANSFER`.
#[derive(Debug)]
enum Transfer {
    /// A read holds no state: `READ` is a range request that carries its own address, so
    /// the open transfer only says that `END_TRANSFER` has nothing to commit.
    Read,
    Write {
        class: u32,
        at: Location,
        object: Object,
        /// Body length `BEGIN_WRITE` declared. The body is assembled into it by offset.
        len: u32,
    },
}

/// An emulated instrument.
///
/// Defaults to a Nord Electro 5's partition table and bank geometry with empty storage;
/// [`Self::partition_mut`] fills the slots.
pub struct EmuDevice {
    partitions: Vec<Partition>,
    /// Open UI sessions. One is healthy; two or more is the wedge that makes every slot
    /// read empty — see [`Self::ui_wedged`].
    ui_depth: u32,
    session: Option<u32>,
    transfer: Option<Transfer>,
    outbox: VecDeque<Vec<u8>>,
    stopped: bool,
    stalled: bool,
    cursor: bool,
    poison_cursor: bool,
    /// Set by any mutation, so a caller can see that a walk would now be refused on
    /// hardware even when [`Self::poison_cursor_on_mutation`] is off.
    mutated: bool,
    unmodeled: Unmodeled,
}

impl Default for EmuDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl EmuDevice {
    /// An Electro 5 with nothing stored in it.
    pub fn new() -> Self {
        Self::with_partitions(crate::store::electro5())
    }

    /// An instrument with a partition table of the caller's own.
    pub fn with_partitions(partitions: Vec<Partition>) -> Self {
        Self {
            partitions,
            ui_depth: 0,
            session: None,
            transfer: None,
            outbox: VecDeque::new(),
            stopped: false,
            stalled: false,
            cursor: true,
            poison_cursor: false,
            mutated: false,
            unmodeled: Unmodeled::default(),
        }
    }

    pub fn partitions(&self) -> &[Partition] {
        &self.partitions
    }

    /// The partition a class code addresses. The class code **is** the partition index.
    pub fn partition(&self, class: ObjectClass) -> Option<&Partition> {
        self.partitions.get(class.to_raw() as usize)
    }

    pub fn partition_mut(&mut self, class: ObjectClass) -> Option<&mut Partition> {
        self.partitions.get_mut(class.to_raw() as usize)
    }

    /// Put an object in a slot, as if the panel had stored it there.
    ///
    /// Panics if the class has no partition — a test that addresses one it did not
    /// configure is asking the wrong question of the emulator.
    pub fn insert(&mut self, class: ObjectClass, at: Location, object: Object) {
        self.partition_mut(class)
            .unwrap_or_else(|| panic!("no partition for {class:?}"))
            .insert(at, object);
    }

    pub fn get(&self, class: ObjectClass, at: Location) -> Option<&Object> {
        self.partition(class).and_then(|p| p.get(at))
    }

    pub fn unmodeled(&mut self) -> &mut Unmodeled {
        &mut self.unmodeled
    }

    /// Queue the unsolicited `CHANGED` notification a front-panel STORE produces.
    ///
    /// It arrives in place of whatever reply the host reads for next, which is exactly
    /// how it desynced a real session. Observed on hardware; absent from every capture.
    pub fn queue_changed(&mut self) {
        let frame = Message::new(Service::Program, SUBSYSTEM, cmd::CHANGED, Vec::new()).encode();
        self.outbox.push_back(frame);
    }

    /// Start out holding a class session an earlier run abandoned, so the next
    /// `SESSION_OPEN` is refused with `0x12` until a bare `SESSION_CLOSE` clears it.
    /// Confirmed on hardware.
    pub fn hold_session(&mut self, class: ObjectClass) {
        self.session = Some(class.to_raw());
    }

    /// Start out holding a UI session an earlier run opened and never closed.
    ///
    /// While more than one is open every slot in every class reads **empty** — a wrong
    /// answer that looks like a right one. A session's own `GOODBYE` balances only its
    /// own `HELLO`, so the state survives reopening and is cured only by a `GOODBYE`
    /// with no `HELLO` of its own. The symptom, its persistence and its cure are
    /// confirmed on hardware; the depth count is this crate's model of them, not a
    /// known device implementation.
    pub fn abandon_ui_session(&mut self) {
        self.ui_depth += 1;
    }

    /// Whether the device is telling every slot it is empty **right now**.
    ///
    /// ⚠️ True only while the extra session is open, which between transactions it is
    /// not — a caller outside a session is asking about the symptom rather than the
    /// fault.
    pub fn ui_wedged(&self) -> bool {
        self.ui_depth >= 2
    }

    /// Stop accepting writes, as the bulk endpoints do when handed a frame the device
    /// cannot handle. The instrument keeps playing and endpoint 0 keeps answering; only
    /// a power cycle recovers. Confirmed on hardware.
    pub fn stall_endpoints(&mut self) {
        self.stalled = true;
    }

    pub fn endpoints_stalled(&self) -> bool {
        self.stalled
    }

    /// Whether the device has stopped answering altogether — what probing `0x7e` did.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Whether anything has mutated storage since this device was built.
    ///
    /// On hardware that is what poisons the enumeration cursor until a power cycle.
    pub fn mutated(&self) -> bool {
        self.mutated
    }

    /// Kill the enumeration cursor on the first mutation, as the instrument does.
    ///
    /// Off by default: on hardware the degradation is partial before it is total and
    /// takes a different class down each time, so a faithful default would be
    /// nondeterministic. On, every class refuses `NEXT_SLOT` with `0x11` from the first
    /// mutation onward.
    pub fn poison_cursor_on_mutation(&mut self, yes: bool) {
        self.poison_cursor = yes;
    }

    /// Whether a reply is waiting to be read.
    pub fn has_response(&self) -> bool {
        !self.outbox.is_empty()
    }

    /// The next queued reply, if the device has anything to say.
    pub fn take_response(&mut self) -> Option<Vec<u8>> {
        self.outbox.pop_front()
    }

    /// Feed the device one host frame, queueing whatever it answers.
    ///
    /// Errors on a frame that does not decode — a bad CRC or a length field that
    /// disagrees with the buffer. A real device drops those silently; here the host is
    /// in the same process and a malformed frame is a bug worth surfacing.
    pub fn receive(&mut self, frame: &[u8]) -> Result<()> {
        let req = Message::decode(frame)?;
        if self.stopped {
            return Ok(());
        }
        if let Some(reply) = self.dispatch(&req) {
            self.outbox.push_back(reply);
        }
        Ok(())
    }

    fn dispatch(&mut self, req: &Message) -> Option<Vec<u8>> {
        match req.service {
            Service::Ui if req.subsystem == ui::SUBSYSTEM => self.ui(req),
            // ⚠️ Two unrelated codes in two services produce the identical stall, so this
            // is a general response to a frame the device cannot handle rather than a
            // property of either — which means any unprobed code can do it, and the ones
            // that merely answered were luck. Confirmed on hardware.
            Service::Program if req.subsystem == STALLING_SUBSYSTEM => {
                self.stall_endpoints();
                None
            }
            Service::Program if SUBSYSTEM_WINDOW.contains(&req.subsystem) => self.program(req),
            // A frame carrying a subsystem outside the supported window is dropped
            // without a reply. Confirmed on hardware for 0 and 11.
            _ => None,
        }
    }

    fn ui(&mut self, req: &Message) -> Option<Vec<u8>> {
        match req.command {
            UI_STALL => {
                self.stall_endpoints();
                None
            }
            ui::HELLO => {
                self.ui_depth += 1;
                Some(ok(Service::Ui, req.command, Vec::new()))
            }
            ui::GOODBYE => {
                self.ui_depth = self.ui_depth.saturating_sub(1);
                Some(ok(Service::Ui, req.command, Vec::new()))
            }
            // The progress strings are fire-and-forget: the device paints them and never
            // answers. Confirmed across the capture corpus.
            ui::LABEL | ui::PERCENT => None,
            _ => self
                .unmodeled
                .unknown_command
                .map(|s| refuse(Service::Ui, req.command, s)),
        }
    }

    fn program(&mut self, req: &Message) -> Option<Vec<u8>> {
        let args = words(req.payload());
        let arg = |i: usize| args.get(i).copied();
        let at = |i: usize| Location {
            bank: args.get(i).copied().unwrap_or_default(),
            slot: args.get(i + 1).copied().unwrap_or_default(),
        };
        let command = req.command;
        let reply = |args: Vec<u8>| Some(ok(Service::Program, command, args));
        let no = |s: u32| Some(refuse(Service::Program, command, s));

        match command {
            cmd::PARTITIONS => reply(self.partition_table()),

            cmd::BANKS => match arg(0).and_then(|i| self.partitions.get(i as usize)) {
                // With no argument the device answers "wrong arguments" rather than
                // defaulting to a partition. Confirmed on hardware.
                None => no(status::BAD_ARGUMENTS),
                Some(p) => {
                    let mut out = arg(0).unwrap().to_be_bytes().to_vec();
                    out.push(p.banks.len() as u8);
                    for bank in &p.banks {
                        out.extend_from_slice(&(bank.name.len() as u32).to_be_bytes());
                        out.extend_from_slice(bank.name.as_bytes());
                        out.extend_from_slice(&bank.slots.to_be_bytes());
                    }
                    reply(out)
                }
            },

            cmd::SESSION_OPEN => match self.session {
                // A session an earlier run left open is refused, not replaced.
                Some(_) => no(status::STALE_SESSION),
                None => {
                    let class = arg(0)?;
                    self.session = Some(class);
                    reply(class.to_be_bytes().to_vec())
                }
            },

            cmd::SESSION_CLOSE => {
                // Answers success whether or not anything was open — which is what makes
                // it usable bare, as the cure for a session the device still holds.
                self.session = None;
                self.transfer = None;
                reply(Vec::new())
            }

            cmd::STATUS => {
                let class = arg(0)?;
                let Some(p) = self.partitions.get(class as usize) else {
                    return no(status::BAD_ARGUMENTS);
                };
                let (count, free, used) = p.counters();
                let mut out = Vec::new();
                for w in [count, free, used, p.extra_counters[0], p.extra_counters[1]] {
                    out.extend_from_slice(&w.to_be_bytes());
                }
                reply(out)
            }

            cmd::INFO => match self.locate(at(0)) {
                Err(s) => no(s),
                Ok((class, at)) => {
                    let p = &self.partitions[class as usize];
                    reply(info_record(p, at, p.get(at).unwrap()))
                }
            },

            cmd::DEPENDENCIES => match self.locate(at(0)) {
                Err(s) => no(s),
                Ok((class, at)) => reply(self.dependency_list(class, at)),
            },

            cmd::NEXT_SLOT => {
                let Some(class) = self.session else {
                    return no(self.unmodeled.no_session);
                };
                if !self.cursor {
                    return no(status::ENUMERATION_DISABLED);
                }
                let asked = at(0);
                let next = self.partitions.get(class as usize).and_then(|p| {
                    // Inferred from the wedge's observed effect on every slot query: with
                    // nothing findable the walk ends immediately. The cursor's own
                    // behaviour under the wedge is not separately measured.
                    match self.ui_wedged() {
                        true => None,
                        false => p.next_occupied(asked),
                    }
                });
                let mut out = asked.bank.to_be_bytes().to_vec();
                match next {
                    Some(found) => {
                        out.extend_from_slice(&found.slot.to_be_bytes());
                        reply(out)
                    }
                    // End of the bank: the status says so and the payload echoes the
                    // position asked about. Confirmed in USB captures.
                    None => {
                        out.extend_from_slice(&asked.slot.to_be_bytes());
                        Some(frame(Service::Program, command, status::EMPTY, out))
                    }
                }
            }

            cmd::FOCUS => {
                let Some(class) = self.session else {
                    return no(self.unmodeled.no_session);
                };
                match self.partitions.get(class as usize).map(|p| p.focus) {
                    Some(Focus::At(at)) => {
                        let mut out = at.bank.to_be_bytes().to_vec();
                        out.extend_from_slice(&at.slot.to_be_bytes());
                        reply(out)
                    }
                    Some(Focus::Nothing) => no(status::EMPTY),
                    Some(Focus::NotApplicable) => no(status::NO_FOCUS),
                    None => no(status::BAD_ARGUMENTS),
                }
            }

            cmd::SELECT => match self.locate(at(0)) {
                Err(status::EMPTY) => no(self.unmodeled.select_empty),
                Err(s) => no(s),
                Ok((class, at)) => {
                    self.partitions[class as usize].focus = Focus::At(at);
                    reply(address(at))
                }
            },

            cmd::DELETE => match self.locate(at(0)) {
                Err(status::EMPTY) => no(self.unmodeled.delete_empty),
                Err(s) => no(s),
                Ok((class, at)) => {
                    self.partitions[class as usize].remove(at);
                    // ⚠️ No reference fix-up, unlike `MOVE`: a set list pointing at the
                    // slot keeps pointing at it and its row starts reading as dangling.
                    // Confirmed on hardware.
                    self.mutate();
                    reply(address(at))
                }
            },

            cmd::RENAME => match self.locate(at(0)) {
                Err(status::EMPTY) => no(self.unmodeled.rename_empty),
                Err(s) => no(s),
                Ok((class, at)) => {
                    let len = arg(2).unwrap_or_default() as usize;
                    let name = req.payload().get(12..12 + len).unwrap_or_default();
                    self.partitions[class as usize].get_mut(at).unwrap().name =
                        String::from_utf8_lossy(name).into_owned();
                    self.mutate();
                    // The reply carries the address only — the name is not echoed back.
                    // Confirmed in USB captures.
                    reply(address(at))
                }
            },

            cmd::MOVE => match (self.locate(at(0)), self.addressable(at(2))) {
                (Err(status::EMPTY), _) => no(self.unmodeled.source_empty),
                (Err(s), _) => no(s),
                (_, Err(s)) => no(s),
                (Ok((class, from)), Ok(to)) => {
                    let p = &mut self.partitions[class as usize];
                    // An occupied destination is **swapped**, not overwritten: its
                    // occupant ends up in the source slot byte-identical. Confirmed on
                    // hardware, and the one behaviour here that must never be described
                    // as an overwrite.
                    let moved = p.remove(from).unwrap();
                    if let Some(displaced) = p.remove(to) {
                        p.insert(from, displaced);
                    }
                    p.insert(to, moved);
                    self.relocate_references(class, from, to);
                    self.mutate();
                    let mut out = address(from);
                    out.extend_from_slice(&address(to));
                    reply(out)
                }
            },

            cmd::COPY => match (self.locate(at(0)), self.addressable(at(2))) {
                (Err(status::EMPTY), _) => no(self.unmodeled.source_empty),
                (Err(s), _) => no(s),
                (_, Err(s)) => no(s),
                (Ok((class, from)), Ok(to)) => {
                    // A deep copy the device performs internally, and it **overwrites**
                    // an occupied destination — the `0x4` precondition belongs to the
                    // write path alone. Confirmed on hardware.
                    let copy = self.partitions[class as usize].get(from).unwrap().clone();
                    self.partitions[class as usize].insert(to, copy);
                    self.mutate();
                    let mut out = address(from);
                    out.extend_from_slice(&address(to));
                    reply(out)
                }
            },

            cmd::BEGIN_READ => match self.locate(at(0)) {
                Err(s) => no(s),
                Ok((_, at)) => {
                    self.transfer = Some(Transfer::Read);
                    reply(address(at))
                }
            },

            cmd::READ => {
                let (Some(offset), Some(want)) = (arg(2), arg(3)) else {
                    return no(status::BAD_ARGUMENTS);
                };
                match self.locate(at(0)) {
                    Err(s) => no(s),
                    Ok((class, at)) => {
                        let body = &self.partitions[class as usize].get(at).unwrap().body;
                        let end = offset as usize + want as usize;
                        let Some(chunk) = body.get(offset as usize..end) else {
                            return no(self.unmodeled.bad_state);
                        };
                        let mut out = address(at);
                        out.extend_from_slice(&offset.to_be_bytes());
                        out.extend_from_slice(&want.to_be_bytes());
                        out.extend_from_slice(chunk);
                        reply(out)
                    }
                }
            }

            cmd::BEGIN_WRITE => {
                let to = at(0);
                let class = match self.addressable(to) {
                    Ok(_) => self.session.unwrap(),
                    Err(s) => return no(s),
                };
                let Some(len) = arg(2) else {
                    return no(status::BAD_ARGUMENTS);
                };
                if self.partitions[class as usize].get(to).is_some() {
                    // ⚠️ The device will not overwrite in place. Replacing a slot is
                    // delete-then-write, and the window in between belongs to the caller.
                    // Confirmed on hardware.
                    return no(status::OCCUPIED);
                }
                let tag = req.payload().get(12..16).unwrap_or(&[0; 4]);
                // The tail is a length-prefixed name, and a write that sends `1, '0'`
                // leaves the slot called `"0"`. Confirmed in USB captures, both in the
                // request and in the `INFO` read that followed it.
                let name_len = arg(6).unwrap_or_default() as usize;
                let name = req.payload().get(28..28 + name_len).unwrap_or_default();
                let object = Object::new(
                    &String::from_utf8_lossy(name),
                    &tag.try_into().unwrap_or([0; 4]),
                    // `BEGIN_WRITE` carries no version and the device reports one per
                    // format tag, so the class supplies it.
                    self.partitions[class as usize].write_version,
                    Vec::new(),
                );
                self.transfer = Some(Transfer::Write {
                    class,
                    at: to,
                    object,
                    len,
                });
                reply(address(to))
            }

            cmd::WRITE_DATA => {
                let bad = self.unmodeled.bad_state;
                let (Some(offset), Some(len)) = (arg(2), arg(3)) else {
                    return no(status::BAD_ARGUMENTS);
                };
                let body = req.payload().get(16..).unwrap_or_default().to_vec();
                if body.len() != len as usize {
                    return no(bad);
                }
                let Some(Transfer::Write { at, object, .. }) = self.transfer.as_mut() else {
                    return no(bad);
                };
                let at = *at;
                let end = offset as usize + body.len();
                if object.body.len() < end {
                    object.body.resize(end, 0);
                }
                object.body[offset as usize..end].copy_from_slice(&body);
                // The reply carries the address only, not the offset or the body.
                // Confirmed in USB captures.
                reply(address(at))
            }

            cmd::END_TRANSFER => {
                let at = at(0);
                let bad = self.unmodeled.bad_state;
                match self.transfer.take() {
                    Some(Transfer::Write {
                        class,
                        at: to,
                        object,
                        len,
                    }) => {
                        if object.body.len() != len as usize {
                            return no(bad);
                        }
                        self.partitions[class as usize].insert(to, object);
                        self.mutate();
                        reply(address(to))
                    }
                    _ => reply(address(at)),
                }
            }

            // ⚠️ Modelled because it is worth recognising, never because it is worth
            // sending: on hardware this paints "Deleting..." and a full progress bar,
            // answers nothing at all, and leaves the session impossible to close.
            cmd::DO_NOT_SEND_DELETING => {
                self.stopped = true;
                None
            }

            _ => self
                .unmodeled
                .unknown_command
                .map(|s| refuse(Service::Program, command, s)),
        }
    }

    /// The class the open session is scoped to.
    fn session_class(&self) -> std::result::Result<u32, u32> {
        self.session.ok_or(self.unmodeled.no_session)
    }

    /// Resolve a slot-addressed command's address against the session's class, answering
    /// with the status the device would refuse it with.
    fn locate(&self, at: Location) -> std::result::Result<(u32, Location), u32> {
        let class = self.session_class()?;
        let p = self
            .partitions
            .get(class as usize)
            .ok_or(status::OUT_OF_RANGE)?;
        if !p.addressable(at) {
            return Err(status::OUT_OF_RANGE);
        }
        // An abandoned UI session makes every slot in every class read empty. Nothing
        // distinguishes the lie from the truth at the protocol level, which is exactly
        // what makes it worth emulating.
        if self.ui_wedged() || p.get(at).is_none() {
            return Err(status::EMPTY);
        }
        Ok((class, at))
    }

    /// Like [`Self::locate`] but for a destination, which need not hold anything.
    fn addressable(&self, at: Location) -> std::result::Result<Location, u32> {
        let class = self.session_class()?;
        match self.partitions.get(class as usize) {
            Some(p) if p.addressable(at) => Ok(at),
            _ => Err(status::OUT_OF_RANGE),
        }
    }

    fn mutate(&mut self) {
        self.mutated = true;
        if self.poison_cursor {
            self.cursor = false;
        }
    }

    /// Rewrite every stored reference to a moved object, in every class.
    ///
    /// The instrument maintains referential integrity itself: moving a program updates
    /// every set list that points at it, so a move cannot leave a dangling reference.
    /// Confirmed on hardware. The swap's other half is rewritten symmetrically, which is
    /// inferred from the same mechanism rather than separately measured.
    ///
    /// Not modelled: the rewrite migrates the referring object's own schema version and
    /// changes its checksum, which on hardware is an irreversible side effect of a move.
    fn relocate_references(&mut self, class: u32, from: Location, to: Location) {
        for p in &mut self.partitions {
            for object in p.objects_mut() {
                for row in &mut object.dependencies {
                    if row.class != class {
                        continue;
                    }
                    match row.location {
                        Some(l) if l == from => row.location = Some(to),
                        Some(l) if l == to => row.location = Some(from),
                        _ => {}
                    }
                }
            }
        }
    }

    fn partition_table(&self) -> Vec<u8> {
        let mut out = vec![self.partitions.len() as u8];
        for p in &self.partitions {
            out.extend_from_slice(&(p.name.len() as u32).to_be_bytes());
            out.extend_from_slice(p.name.as_bytes());
            out.extend_from_slice(&p.fields);
        }
        out
    }

    fn dependency_list(&self, class: u32, at: Location) -> Vec<u8> {
        let object = self.partitions[class as usize].get(at).unwrap();
        let mut out = address(at);
        out.extend_from_slice(&(object.dependencies.len() as u32).to_be_bytes());
        for row in &object.dependencies {
            let mut row = row.clone();
            // The dangling marker is resolved at read time, not stored: the referring
            // object still records the address and the device checks existence on every
            // read. Confirmed on hardware by deleting a referenced program and reading
            // the set list back — one byte changed, and changed back on restore.
            if let Some(l) = row.location {
                let exists = self
                    .partitions
                    .get(row.class as usize)
                    .is_some_and(|p| p.get(l).is_some());
                row.missing = u32::from(!exists);
            }
            row.encode(&mut out);
        }
        out
    }
}

/// What `INFO` reports about one slot.
fn info_record(p: &Partition, at: Location, o: &Object) -> Vec<u8> {
    let mut out = address(at);
    out.extend_from_slice(&(o.body.len() as u32).to_be_bytes());
    out.extend_from_slice(&o.format);
    out.extend_from_slice(&o.version.to_be_bytes());
    for w in o.words_before_name {
        out.extend_from_slice(&w.to_be_bytes());
    }
    out.extend_from_slice(&(o.name.len() as u32).to_be_bytes());
    out.extend_from_slice(o.name.as_bytes());
    for w in o.words_after_name {
        out.extend_from_slice(&w.to_be_bytes());
    }
    // The classes that do not checksum their content report `0xffffffff` rather than a
    // value, which the host normalizes away.
    let crc = match p.checksummed {
        true => nord_usb::envelope::crc32(&o.body),
        false => u32::MAX,
    };
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

fn address(at: Location) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    at.write_to(&mut out);
    out
}

fn words(p: &[u8]) -> Vec<u32> {
    p.chunks_exact(4)
        .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
        .collect()
}

/// A reply: the request's command `+ 1`, a status word, then the arguments.
fn frame(service: Service, command: u32, status: u32, args: Vec<u8>) -> Vec<u8> {
    let mut all = status.to_be_bytes().to_vec();
    all.extend_from_slice(&args);
    let subsystem = match service {
        Service::Ui => ui::SUBSYSTEM,
        _ => SUBSYSTEM,
    };
    Message::new(service, subsystem, command + 1, all).encode()
}

fn ok(service: Service, command: u32, args: Vec<u8>) -> Vec<u8> {
    frame(service, command, status::OK, args)
}

/// A refusal carries the status word and nothing else.
///
/// ⚠️ A real refusal's payload is **not** zeroed — repeating one failing call returned
/// different leftover device memory each time. Nothing may decode a payload whose status
/// is non-zero, so the emulator sends none rather than manufacturing plausible garbage.
fn refuse(service: Service, command: u32, status: u32) -> Vec<u8> {
    frame(service, command, status, Vec::new())
}
