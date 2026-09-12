//! What the browser asks for, and the running of it.
//!
//! Rendering answers with [`Act`]s rather than acting, so a row can be drawn while the
//! thing it stands for is about to change. [`apply`] is where they meet the workspace,
//! the device and the tabs.

use nord_usb::{Location, ObjectClass};

use super::drag::{Item, Kind};
use super::Browser;
use crate::device::{
    fit, sendable, write_warning, Device, DeviceCmd, DeviceState, Fit, Outgoing, Purpose,
};
use crate::filter::Narrow;
use crate::log::Log;
use crate::newproject::Making;
use crate::queue::{enqueue, retarget, Occupancy, Queue};
use crate::shell::{Dock, Page, Shell};
use crate::strings::place;
use crate::tabs::{Spot, Tabs};
use crate::workspace::{Fresh, LocalEntity, Workspace};

/// What the browser asks the rest of the app to do.
pub enum Act {
    Connect,
    Disconnect,
    OpenFiles,
    New(Fresh),
    /// Pick the WAVs a new project or instrument is laid out from. What they make is
    /// made once the dialog has each file's root key — see [`crate::newproject`].
    NewFromWavs(Making),
    /// Read the whole instrument again — every class, its geometry and its focus.
    Resync,
    ReadAgain(ObjectClass),
    Open(Item),
    /// A view of a slot becomes an asset on this computer.
    Keep(u64),
    NewFolder,
    RemoveFolder(u64),
    /// Put a tag on every one of these assets. A view is kept first.
    Tag {
        ids: Vec<u64>,
        tag: u64,
    },
    /// Take a tag off every one of these assets.
    Untag {
        ids: Vec<u64>,
        tag: u64,
    },
    /// A tag with nothing on it yet, its name waiting to be typed.
    NewTag(String),
    RenameTag {
        id: u64,
        name: String,
    },
    RemoveTag(u64),
    /// What is picked, under a new tag, and nothing more.
    SaveAsGig,
    /// Put an asset in a folder, or out of the one it is in.
    File {
        id: u64,
        folder: Option<u64>,
    },
    /// Queue every one of these assets for the slot it came off. One with none is
    /// skipped, and the log says how many were.
    SendChecked(Vec<u64>),
    Copy {
        class: ObjectClass,
        at: Location,
    },
    LoadOnInstrument {
        class: ObjectClass,
        at: Location,
    },
    /// Queue a local asset for a slot, asking first where the write itself has a
    /// warning to carry.
    Send {
        id: u64,
        class: ObjectClass,
        at: Location,
    },
    /// Send something already waiting to another slot instead.
    Retarget {
        id: u64,
        class: ObjectClass,
        at: Location,
    },
    /// Stop waiting to send this one. Nothing is deleted.
    Unqueue(u64),
    /// Stop waiting to send any of it. A queue is a plan, so emptying it asks nothing.
    ClearQueue,
    /// Write everything in the queue, grouped by folder. Already agreed to.
    SendAll,
    /// Queue every asset the instrument no longer agrees with, each for its own slot.
    QueueChanged,
    /// Put the "send everything waiting" question, which `SendAll` is the answer to.
    AskSendAll,
    /// The same as a Send, already agreed to. Nothing asks twice.
    Replace {
        id: u64,
        class: ObjectClass,
        at: Location,
    },
    Rearrange {
        class: ObjectClass,
        from: Location,
        to: Location,
    },
    RenameLocal {
        id: u64,
        name: String,
    },
    RenameFolder {
        id: u64,
        name: String,
    },
    RenameSlot {
        class: ObjectClass,
        at: Location,
        name: String,
    },
    DuplicateLocal(u64),
    DuplicateSlot {
        class: ObjectClass,
        from: Location,
        to: Location,
    },
    DeleteSlot {
        class: ObjectClass,
        at: Location,
    },
    Remove(u64),
    /// Hand the open document's bytes to the user as a file.
    Export(u64),
    /// ⌘S over the open document: what saving means for whichever kind of document it
    /// is — see [`save_doc`].
    SaveDoc(u64),
    /// The write back to a slot a [`Act::SaveDoc`] over a view asks about, agreed to.
    WriteBack(u64),
    /// Back to the bytes it was last saved as.
    Revert(u64),
    /// Bring a view of the centre forward.
    ShowTab(Spot),
    /// The keyboard tab, switched to one class.
    ShowClass(ObjectClass),
    /// Turn one of the library's filters on or off.
    Narrow(Narrow),
    /// Shut whatever the centre is on.
    CloseTab,
    ToggleDock(Dock),
    /// Open the bottom dock on one of its pages.
    ShowPage(Page),
    /// The whole activity log onto the clipboard.
    CopyLog,
    /// Ask the window to close. Never reached on the web, where the tab is the window.
    Quit,
    /// Nothing happened, and this is why.
    Refused(String),
}

/// What can be asked of everything checked at once, in the order both the library's
/// footer and a checked row's menu offer it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bulk {
    Queue,
    Copy,
    Export,
    Tag,
    Delete,
}

impl Bulk {
    pub const ALL: [Bulk; 5] = [
        Bulk::Queue,
        Bulk::Copy,
        Bulk::Export,
        Bulk::Tag,
        Bulk::Delete,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Bulk::Queue => "Queue for sending",
            Bulk::Copy => "Copy to this computer",
            Bulk::Export => "Export…",
            Bulk::Tag => "Tag…",
            Bulk::Delete => "Delete…",
        }
    }

    /// Why the control is dead, which is what a hover over it says.
    pub fn nothing(self) -> &'static str {
        match self {
            Bulk::Queue | Bulk::Export | Bulk::Tag => "nothing checked is on this computer",
            Bulk::Copy => "nothing checked is on the instrument",
            Bulk::Delete => "nothing is checked",
        }
    }
}

/// What the attached instrument makes of a checked set: how many of it it would take,
/// and the sentence it refuses the rest with.
///
/// ⚠️ Only what is on this computer is counted. A slot is on the instrument by having
/// got there, and nothing here would write it back.
pub struct Fits {
    pub takes: usize,
    pub of: usize,
    /// Why the ones it will not take are refused — the first refusal's own words, which
    /// is the one a hover over a dead control shows.
    pub why: Option<String>,
}

impl Fits {
    /// What the Queue control is labelled with: its own words, or the count once the
    /// instrument has refused some of the set.
    pub fn label(&self) -> String {
        match self.takes < self.of {
            true => format!("Queue {} of {}", self.takes, self.of),
            false => Bulk::Queue.label().to_string(),
        }
    }
}

/// What the instrument makes of a checked set, from [`fit`] over each of its locals.
pub fn fits(checked: &[Item], workspace: &Workspace, state: &DeviceState) -> Fits {
    let mut held = Fits {
        takes: 0,
        of: 0,
        why: None,
    };
    let locals = checked
        .iter()
        .copied()
        .filter_map(Item::local)
        .filter_map(|id| workspace.get(id));
    for entity in locals {
        held.of += 1;
        let verdict = fit(state, entity);
        match verdict.allowed() {
            true => held.takes += 1,
            false => held.why = held.why.or_else(|| verdict.why().map(str::to_string)),
        }
    }
    held
}

/// What one of those asks for over everything checked.
///
/// [`Bulk::Tag`] answers with the ids a tag would hang on rather than with acts: which
/// tag is picked from a menu of its own, and only then is there an act.
pub fn bulk(action: Bulk, checked: &[Item]) -> Vec<Act> {
    match action {
        Bulk::Queue => match checked
            .iter()
            .copied()
            .filter_map(Item::local)
            .collect::<Vec<_>>()
        {
            ids if ids.is_empty() => Vec::new(),
            ids => vec![Act::SendChecked(ids)],
        },
        Bulk::Copy => checked
            .iter()
            .filter_map(|item| match item {
                Item::Slot { class, at } => Some(Act::Copy {
                    class: *class,
                    at: *at,
                }),
                _ => None,
            })
            .collect(),
        Bulk::Export => checked
            .iter()
            .copied()
            .filter_map(Item::local)
            .map(Act::Export)
            .collect(),
        Bulk::Tag => Vec::new(),
        Bulk::Delete => checked
            .iter()
            .filter_map(|item| match item {
                Item::Local(id) => Some(Act::Remove(*id)),
                Item::Slot { class, at } => Some(Act::DeleteSlot {
                    class: *class,
                    at: *at,
                }),
                Item::Folder(_) | Item::Tag(_) => None,
            })
            .collect(),
    }
}

/// Whether running this act puts something in the send queue or moves what is in it.
fn enqueues(act: &Act) -> bool {
    matches!(
        act,
        Act::Send { .. }
            | Act::Replace { .. }
            | Act::Retarget { .. }
            | Act::SendChecked(_)
            | Act::QueueChanged
    )
}

/// Run what the browser asked for.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    browser: &mut Browser,
    shell: &mut Shell,
    acts: Vec<Act>,
    workspace: &mut Workspace,
    device: &mut Device,
    tabs: &mut Tabs,
    queue: &mut Queue,
    log: &mut Log,
) {
    for act in acts {
        // Taken here rather than by each caller: a drop, a menu and a footer all reach
        // the queue through an act, and a plan made out of sight is a plan nobody
        // reviews.
        if enqueues(&act) {
            shell.show_page(Page::Queue);
        }
        match act {
            Act::Connect => device.connect(log),
            Act::Disconnect => device.disconnect(log),
            Act::OpenFiles => workspace.open_dialog(),
            Act::New(kind) => {
                if let Some(id) = workspace.create(kind, log) {
                    tabs.open(id);
                }
            }
            Act::NewFromWavs(making) => workspace.pick_wavs(making),
            Act::Resync => {
                device.resync();
                log.say("Reading the instrument again…");
            }
            Act::ReadAgain(class) => device.read_class(class),
            Act::Keep(id) => workspace.keep(id, log),
            Act::NewFolder => {
                let id = browser.folders.make();
                // ⚠️ Edit the unique name chosen by `make`, not its generic seed.
                let name = browser.folders.name_of(id).unwrap_or_default().to_string();
                browser.start_rename(Item::Folder(id), &name);
            }
            Act::RemoveFolder(id) => {
                // ⚠️ A removed row cannot close its rename state; a reused id would inherit it.
                browser.forget_rename(Item::Folder(id));
                browser.folders.remove(id);
            }
            Act::File { id, folder } => browser.folders.file(id, folder),
            Act::Tag { ids, tag } => tag_all(browser, workspace, log, &ids, tag),
            Act::Untag { ids, tag } => {
                for id in ids {
                    browser.tags.set(id, tag, false);
                }
            }
            Act::NewTag(wanted) => {
                let id = browser.tags.make(&wanted);
                // ⚠️ Edit the unique name chosen by `make`, not the generic seed it
                // started from: two tags of one name are one row twice.
                let name = browser.tags.name_of(id).unwrap_or_default().to_string();
                browser.start_rename(Item::Tag(id), &name);
            }
            Act::RenameTag { id, name } => browser.tags.rename(id, name),
            Act::RemoveTag(id) => {
                // ⚠️ A removed row cannot close its rename state; a reused id would inherit it.
                browser.forget_rename(Item::Tag(id));
                browser.tags.remove(id);
                // ⚠️ And a tag nobody can see must stop narrowing the library from nowhere.
                shell.filter.forget_tag(id);
            }
            Act::SaveAsGig => {
                let ids = browser.selection.locals();
                match ids.is_empty() {
                    true => {
                        log.say("Nothing on this computer is picked, so there is no gig to save.")
                    }
                    false => {
                        let tag = browser.tags.make("New gig");
                        tag_all(browser, workspace, log, &ids, tag);
                        let name = browser.tags.name_of(tag).unwrap_or_default().to_string();
                        browser.start_rename(Item::Tag(tag), &name);
                    }
                }
            }
            Act::SendChecked(ids) => queue_all(workspace, device, queue, log, &ids),
            Act::Open(Item::Folder(_) | Item::Tag(_)) => {}
            Act::Open(Item::Local(id)) => tabs.open(id),
            // ⚠️ One view per slot prevents divergent copies queued back to one address.
            Act::Open(Item::Slot { class, at }) => match workspace.view_of(class, at) {
                Some(id) => tabs.open(id),
                None => device.send(
                    DeviceCmd::Get {
                        class,
                        at,
                        body: false,
                        why: Purpose::View,
                    },
                    log,
                ),
            },
            Act::Copy { class, at } => device.send(
                DeviceCmd::Get {
                    class,
                    at,
                    body: false,
                    why: Purpose::Copy,
                },
                log,
            ),
            Act::LoadOnInstrument { class, at } => {
                device.send(DeviceCmd::Select { class, at }, log)
            }
            Act::Send { id, class, at } => {
                send(browser, workspace, device, queue, log, id, class, at, true)
            }
            Act::Replace { id, class, at } => {
                send(browser, workspace, device, queue, log, id, class, at, false)
            }
            Act::Retarget { id, class, at } => {
                retarget(workspace, device, queue, log, id, class, at)
            }
            Act::Unqueue(id) => queue.forget(id),
            Act::ClearQueue => queue.clear(),
            Act::SendAll => send_batch(queue, workspace, device, log),
            Act::QueueChanged => crate::queue::queue_changed(workspace, device, queue, log),
            Act::AskSendAll => {
                let title = match queue.len() {
                    1 => "Send 1 sound to the instrument?".to_string(),
                    n => format!("Send {n} sounds to the instrument?"),
                };
                browser.ask_send(workspace, device, queue, title, Act::SendAll);
            }
            Act::Rearrange { class, from, to } => {
                device.send(DeviceCmd::Move { class, from, to }, log)
            }
            Act::RenameLocal { id, name } => {
                workspace.rename(id, name.clone());
                log.say(format!("Renamed it “{name}”."));
            }
            Act::RenameFolder { id, name } => browser.folders.rename(id, name),
            Act::RenameSlot { class, at, name } => {
                device.send(DeviceCmd::Rename { class, at, name }, log)
            }
            Act::DuplicateLocal(id) => {
                workspace.duplicate(id, log);
            }
            Act::DuplicateSlot { class, from, to } => {
                device.send(DeviceCmd::Duplicate { class, from, to }, log)
            }
            Act::DeleteSlot { class, at } => device.send(DeviceCmd::Delete { class, at }, log),
            Act::Remove(id) => {
                tabs.close(Spot::Document(id));
                queue.forget(id);
                browser.folders.forget(id);
                browser.tags.forget(id);
                workspace.remove(id, log);
            }
            Act::Export(id) => workspace.export(id),
            Act::SaveDoc(id) => save_doc(browser, workspace, device, queue, log, id, true),
            Act::WriteBack(id) => save_doc(browser, workspace, device, queue, log, id, false),
            Act::Revert(id) => workspace.revert(id, log),
            Act::ShowTab(spot) => tabs.show(spot),
            Act::ShowClass(class) => {
                tabs.show(Spot::Keyboard);
                tabs.keyboard_on(class);
            }
            Act::Narrow(narrow) => shell.filter.narrow(narrow),
            Act::CloseTab => {
                if let Some(spot) = tabs.showing() {
                    tabs.close(spot);
                }
            }
            Act::ToggleDock(dock) => shell.toggle(dock),
            Act::ShowPage(page) => shell.show_page(page),
            Act::CopyLog => workspace.ctx().copy_text(log.transcript()),
            Act::Quit => workspace
                .ctx()
                .send_viewport_cmd(eframe::egui::ViewportCommand::Close),
            Act::Refused(why) => log.say(why),
        }
    }
}

/// Put a tag on every one of these assets.
///
/// ⚠️ Membership is by workspace id and a view has none that survives a session — the
/// store skips it and nothing lists it, so the tag would go with the tab. A view is
/// kept first, the way [`Act::Keep`] keeps one, and the log says that is what happened.
fn tag_all(browser: &mut Browser, workspace: &mut Workspace, log: &mut Log, ids: &[u64], tag: u64) {
    let views: Vec<u64> = ids
        .iter()
        .copied()
        .filter(|id| workspace.is_view(*id))
        .collect();
    for id in &views {
        workspace.keep(*id, log);
    }
    if !views.is_empty() {
        log.say(match views.len() {
            1 => {
                "A tag needs somewhere to hang, so it was kept on this computer first.".to_string()
            }
            n => format!(
                "A tag needs somewhere to hang, so {n} views were kept on this computer first."
            ),
        });
    }
    for id in ids {
        browser.tags.set(*id, tag, true);
    }
}

/// Drain the queue, one command per folder.
///
/// The one write path there is: same refusal, same grouping, same per-item flow. What
/// is written leaves the queue when its [`crate::device::DeviceEvent::Sent`] lands, so a
/// batch that stops halfway leaves the rest of the queue where it was.
fn send_batch(queue: &mut Queue, workspace: &Workspace, device: &mut Device, log: &mut Log) {
    // ⚠️ The instrument attached now need not be the one each entry was queued against:
    // the queue survives a disconnection, and the next instrument is asked afresh.
    crate::queue::refit(workspace, &device.state, queue, log);
    let batch = grouped(queue, workspace);
    // Validate the whole batch before the first delete-then-write.
    for item in batch.iter().flat_map(|(_, items)| items) {
        if let Err(e) = nord_usb::envelope::unwrap(&item.bytes) {
            log.error(format!("{}: {e}", item.name));
            log.trouble(format!(
                "“{}” is not a file the instrument takes, so nothing was sent.",
                item.name
            ));
            return;
        }
    }
    for (class, items) in batch {
        device.send(DeviceCmd::SendAll { class, items }, log);
    }
}

/// What is waiting, gathered per folder in the order the queue holds it.
///
/// A session belongs to a folder, so a folder is the unit a batch is cut into. An entry
/// carrying a refusal is left out of it and stays in the queue.
fn grouped(queue: &Queue, workspace: &Workspace) -> Vec<(ObjectClass, Vec<Outgoing>)> {
    let mut by_class: Vec<(ObjectClass, Vec<Outgoing>)> = Vec::new();
    for held in queue.entries().iter().filter(|held| held.failure.is_none()) {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        let item = Outgoing {
            id: entity.id,
            at: held.at,
            name: entity.name.clone(),
            bytes: entity.bytes.clone(),
        };
        match by_class.iter_mut().find(|(class, _)| *class == held.class) {
            Some((_, items)) => items.push(item),
            None => by_class.push((held.class, vec![item])),
        }
    }
    by_class
}

/// Warn when an outgoing tag differs from every scanned resident tag.
/// An unreadable tag or unscanned folder yields no warning; this never refuses a write.
pub fn foreign_format(outgoing: &str, resident: &[String]) -> Option<String> {
    let outgoing = outgoing.trim();
    // ⚠️ `?` means unreadable, not a format known to differ from the instrument.
    let readable = !outgoing.is_empty() && outgoing.chars().all(|c| c.is_ascii_alphanumeric());
    if !readable || resident.is_empty() {
        return None;
    }
    if resident
        .iter()
        .any(|held| held.trim().eq_ignore_ascii_case(outgoing))
    {
        return None;
    }
    let held: Vec<&str> = resident.iter().map(|held| held.trim()).collect();
    Some(format!(
        "⚠️ This file is {outgoing}; everything read in that folder is {}. Sending it \
         replaces what is there.",
        held.join(" or "),
    ))
}

/// Everything worth reading before a write into `class` lands: what the attached
/// instrument makes of the asset's format, and what the class itself disturbs beyond
/// the slot.
pub(super) fn write_warnings(
    state: &DeviceState,
    class: ObjectClass,
    entity: &LocalEntity,
) -> impl Iterator<Item = String> {
    [
        fit(state, entity).why().map(str::to_string),
        write_warning(class).map(str::to_string),
    ]
    .into_iter()
    .flatten()
}

/// The same set as one note, for the dialog that asks about a single slot.
fn write_note(state: &DeviceState, class: ObjectClass, entity: &LocalEntity) -> Option<String> {
    let note: Vec<String> = write_warnings(state, class, entity).collect();
    (!note.is_empty()).then(|| note.join("\n\n"))
}

/// Where an asset would be written back to, if anywhere: the slot holding its bytes,
/// then the slot it came off, and only where this app will write into that class at all.
pub(super) fn owed(entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    let (class, at) = entity.spot()?;
    crate::device::sendable(class).then_some((class, at))
}

/// Where queueing an asset for sending would put it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Bound {
    At(ObjectClass, Location),
    /// Its folder is on the instrument, and no slot of it has been read and found free.
    Full(ObjectClass),
    /// The attached instrument does not take this format, and this is why.
    Refused(String),
    /// Nothing the instrument declares takes this kind, or this app will not write into
    /// the folder that would.
    Nowhere,
}

/// The slot an asset is owed to, and otherwise the first free slot of its own folder
/// that nothing is already waiting for.
///
/// ⚠️ Free means read and found empty. A folder no walk has reached offers nothing, for
/// the reason [`crate::queue::Occupancy`] gives.
pub(super) fn bound_for(entity: &LocalEntity, state: &DeviceState, queue: &Queue) -> Bound {
    if let Fit::Refuses(why) = fit(state, entity) {
        return Bound::Refused(why);
    }
    if let Some((class, at)) = owed(entity) {
        return Bound::At(class, at);
    }
    let home = Kind::of(entity.entity.as_ref()).home();
    let Some(class) = home.filter(|class| sendable(*class) && state.classes().contains(class))
    else {
        return Bound::Nowhere;
    };
    match state.first_free(class, &queue.waiting_in(class)) {
        Some(at) => Bound::At(class, at),
        None => Bound::Full(class),
    }
}

/// Queue a set of assets, each for wherever it is bound, and say what would not go.
///
/// Destinations are handed out one at a time and each entry joins the queue before the
/// next is placed, so a set of unlinked assets walks down the free slots in address
/// order rather than piling onto the first of them.
///
/// A folder with no room refuses by name: what a set of them comes to is a fact about
/// the instrument, and dropping the entry without a word would read as a bug.
fn queue_all(
    workspace: &Workspace,
    device: &mut Device,
    queue: &mut Queue,
    log: &mut Log,
    ids: &[u64],
) {
    let mut nowhere = 0;
    let mut asked = 0;
    let mut fits = 0;
    for id in ids.iter().copied() {
        let Some(entity) = workspace.get(id) else {
            continue;
        };
        let name = entity.name.clone();
        asked += 1;
        match bound_for(entity, &device.state, queue) {
            Bound::At(class, at) => {
                fits += 1;
                enqueue(workspace, device, queue, log, id, class, at);
            }
            Bound::Full(class) => {
                fits += 1;
                log.say(format!(
                    "“{name}” was not queued: no slot of {} is both read and still free.",
                    device.state.folder_name(class)
                ));
            }
            Bound::Refused(why) => log.say(format!("“{name}” was not queued. {why}")),
            Bound::Nowhere => {
                fits += 1;
                nowhere += 1;
            }
        }
    }
    if let Some(said) = device
        .state
        .product()
        .and_then(|product| crate::strings::fitting(fits, asked, product))
    {
        log.say(format!("{said}."));
    }
    if nowhere > 0 {
        log.say(match nowhere {
            1 => "1 of them belongs in no folder the instrument has, so it is waiting for \
                  nowhere."
                .to_string(),
            n => format!(
                "{n} of them belong in no folder the instrument has, so they are waiting for \
                 nowhere."
            ),
        });
    }
}

/// What ⌘S means for whichever kind of document this is.
///
/// A **view** is the instrument's own copy looked at in place, so saving it is the write
/// itself: the put runs at once, and the baseline moves when the instrument says the
/// bytes landed. Anything **on this computer** is already kept, so saving it settles its
/// baseline — and, where it stands for a slot, queues it for that slot.
///
/// `ask` is false once the question a write carries has been answered.
#[allow(clippy::too_many_arguments)]
fn save_doc(
    browser: &mut Browser,
    workspace: &mut Workspace,
    device: &mut Device,
    queue: &mut Queue,
    log: &mut Log,
    id: u64,
    ask: bool,
) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    if entity.kept {
        let name = entity.name.clone();
        let spot = owed(entity);
        workspace.mark_saved(id);
        let waiting = queue.entry(id).map(|held| (held.class, held.at));
        match spot {
            // Already waiting for that slot: the plan is what it was, so what this
            // gesture did is settle the bytes.
            Some(spot) if waiting == Some(spot) => {
                log.say(format!("“{name}” is saved on this computer."))
            }
            Some((class, at)) => enqueue(workspace, device, queue, log, id, class, at),
            None => log.say(format!("“{name}” is saved on this computer.")),
        }
        return;
    }
    let Some((class, at)) = owed(entity) else {
        return log.say(format!(
            "“{}” came off nowhere this app writes to, so there is nothing to save it \
             into.",
            entity.name
        ));
    };
    // Refused before the write: bytes that are not what they claim to be must never
    // reach a delete-then-write.
    if let Err(e) = nord_usb::envelope::unwrap(&entity.bytes) {
        log.error(format!("{}: {e}", entity.name));
        return log.trouble(format!(
            "“{}” is not a file the instrument takes.",
            entity.name
        ));
    }
    if let Some(note) = write_note(&device.state, class, entity).filter(|_| ask) {
        return browser.ask_write(&entity.name, place(class, at), note, Act::WriteBack(id));
    }
    device.send(
        DeviceCmd::Put {
            id,
            class,
            at,
            name: entity.name.clone(),
            bytes: entity.bytes.clone(),
        },
        log,
    );
}

/// Queue a local asset for a slot.
///
/// `ask` is false once the question has been answered, which is what keeps the answer
/// from raising it again.
///
/// ⚠️ The question is now only about what a **write** carries — a foreign format, a
/// settings write reloading the panel — and not about the slot being taken. Queueing is
/// reversible and the queue shows the occupant, so an occupied slot no longer earns a
/// modal; the one question before anything is actually written is [`Act::AskSendAll`].
#[allow(clippy::too_many_arguments)]
fn send(
    browser: &mut Browser,
    workspace: &Workspace,
    device: &mut Device,
    queue: &mut Queue,
    log: &mut Log,
    id: u64,
    class: ObjectClass,
    at: Location,
    ask: bool,
) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    // Refused before anything is queued: bytes that are not what they claim to be must
    // never reach a delete-then-write.
    if let Err(e) = nord_usb::envelope::unwrap(&entity.bytes) {
        log.error(format!("{}: {e}", entity.name));
        log.trouble(format!(
            "“{}” is not a file the instrument takes.",
            entity.name
        ));
        return;
    }
    let note = write_note(&device.state, class, entity);
    // The same evidence the entry it is about to become carries: a bank nothing has read
    // holds nothing to name.
    let holds = Occupancy::of(&device.state, class, at);
    match (ask, note, holds.occupant()) {
        (true, Some(note), Some(occupant)) => browser.ask_replace(
            &occupant.name,
            &entity.name,
            place(class, at),
            Some(note),
            Act::Replace { id, class, at },
        ),
        _ => enqueue(workspace, device, queue, log, id, class, at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::bench::bench;
    use crate::strings::folder;
    use crate::workspace::Origin;

    fn at(slot: u32) -> Location {
        Location { bank: 6, slot }
    }

    /// One program's bytes, with nothing left in the list to show for them.
    fn program(workspace: &mut Workspace, log: &mut Log) -> Vec<u8> {
        let id = workspace.create(Fresh::Program, log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.clone();
        workspace.remove(id, log);
        bytes
    }

    /// A fixture checked set: two assets on this computer, one of them off a slot, and
    /// two slots on the instrument.
    fn checked() -> Vec<Item> {
        vec![
            Item::Local(1),
            Item::Local(2),
            Item::Slot {
                class: ObjectClass::Program,
                at: at(0),
            },
            Item::Slot {
                class: ObjectClass::SetList,
                at: at(3),
            },
        ]
    }

    /// ⌘S over a **view** is the write itself: the instrument's own copy, looked at in
    /// place, goes back to the slot it came off at once. Nothing joins the queue, which
    /// is for what this computer is owed to send.
    #[test]
    fn saving_a_view_writes_it_back_to_its_slot_and_queues_nothing() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        device.pretend_attached();
        let id = workspace.view(
            "Africa Split.ne5p".to_string(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(3),
            },
            bytes,
            &mut log,
        );
        let held = workspace.get(id).unwrap().bytes.clone();
        let (_, edited) =
            crate::fields::apply(&held, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, &mut log);

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::SaveDoc(id)],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        assert!(queue.is_empty(), "a write back is not a debt");
        let put = device.queued().front().expect("a put was asked for");
        assert!(
            matches!(put, DeviceCmd::Put { id: sent, class, at: to, .. }
                if *sent == id && *class == ObjectClass::Program && *to == at(3)),
            "{}",
            put.label()
        );
        // The instrument has not answered yet, so the baseline has not moved.
        assert!(workspace.get(id).unwrap().is_unsaved());
        device.pretend(crate::device::DeviceEvent::Sent {
            id,
            class: ObjectClass::Program,
            at: at(3),
            bytes: workspace.get(id).unwrap().bytes.clone(),
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        assert!(!workspace.get(id).unwrap().is_unsaved());
    }

    /// ⌘S over something **on this computer** keeps what is here — it is already kept —
    /// and, where it stands for a slot, asks for that slot.
    #[test]
    fn saving_a_kept_asset_settles_its_baseline_and_queues_it_where_it_stands() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[None, None, None, Some(("held", 7))],
        );
        let linked = workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(3),
            },
            bytes.clone(),
            &mut log,
        );
        let alone = workspace.ingest(
            "Squabble B.ne5p".to_string(),
            Origin::File("Squabble B.ne5p".into()),
            bytes,
            &mut log,
        );
        for id in [linked, alone] {
            let held = workspace.get(id).unwrap().bytes.clone();
            let (_, edited) =
                crate::fields::apply(&held, &[("center_panel.gain".into(), "96".into())]).unwrap();
            workspace.replace_bytes(id, edited, &mut log);
        }

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::SaveDoc(linked), Act::SaveDoc(alone)],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        assert!(!workspace.get(linked).unwrap().is_unsaved());
        assert!(!workspace.get(alone).unwrap().is_unsaved());
        assert_eq!(
            queue.ids(),
            vec![linked],
            "only the one that stands for a slot"
        );
        assert_eq!(queue.entry(linked).map(|held| held.at), Some(at(3)));

        // Saving it again asks the instrument nothing further: it is already waiting for
        // that slot, and a save still says what it did.
        let reads = device.queued().len();
        log.clear();
        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::SaveDoc(linked)],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        assert_eq!(queue.ids(), vec![linked]);
        assert_eq!(device.queued().len(), reads, "the slot was not read again");
        assert!(
            log.transcript()
                .contains("“Africa Split.ne5p” is saved on this computer"),
            "{}",
            log.transcript()
        );
    }

    /// Each of the things offered over a checked set asks only about the half of it that
    /// half is about: queueing and exporting reach this computer's, copying reaches the
    /// instrument's, and deleting reaches all of it.
    #[test]
    fn each_action_over_a_checked_set_asks_only_about_the_rows_it_is_for() {
        let checked = checked();

        let queued = bulk(Bulk::Queue, &checked);
        assert!(
            matches!(queued.as_slice(), [Act::SendChecked(ids)] if *ids == vec![1, 2]),
            "one queueing, over this computer's rows"
        );
        assert_eq!(bulk(Bulk::Copy, &checked).len(), 2, "one per slot");
        assert!(bulk(Bulk::Copy, &checked)
            .iter()
            .all(|act| matches!(act, Act::Copy { .. })));
        assert!(
            matches!(
                bulk(Bulk::Export, &checked).as_slice(),
                [Act::Export(1), Act::Export(2)]
            ),
            "one export per asset on this computer"
        );
        assert!(
            bulk(Bulk::Tag, &checked).is_empty(),
            "a tag is picked first"
        );
        assert!(matches!(
            bulk(Bulk::Delete, &checked).as_slice(),
            [
                Act::Remove(1),
                Act::Remove(2),
                Act::DeleteSlot { .. },
                Act::DeleteSlot { .. }
            ]
        ));
    }

    /// A control the checked set gives nothing to do is a control that is offered dead,
    /// which is what an empty answer says.
    #[test]
    fn an_action_with_nothing_to_act_on_asks_for_nothing() {
        let slots = vec![Item::Slot {
            class: ObjectClass::Program,
            at: at(0),
        }];
        let locals = vec![Item::Local(1)];
        assert!(bulk(Bulk::Queue, &slots).is_empty());
        assert!(bulk(Bulk::Export, &slots).is_empty());
        assert!(bulk(Bulk::Copy, &locals).is_empty());
        assert!(bulk(Bulk::Delete, &[]).is_empty());
    }

    /// Queueing a checked set puts each of them where it is bound: the slot it is owed
    /// to, the first free slot of its own folder for one that came off none, and nowhere
    /// at all for bytes that belong in no folder the instrument has.
    #[test]
    fn queueing_a_checked_set_puts_each_of_them_where_it_is_bound() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        device.pretend_partitions(&crate::device::ELECTRO5);
        // 7:1 is taken by the asset that came off it; 7:2 is the first free slot.
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", ""]);
        let owed = workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(0),
            },
            bytes.clone(),
            &mut log,
        );
        let opened = workspace.ingest(
            "Squabble B.ne5p".to_string(),
            Origin::File("Squabble B.ne5p".into()),
            bytes,
            &mut log,
        );
        let nowhere = workspace.ingest(
            "notes.txt".to_string(),
            Origin::Fresh,
            b"not a Nord file at all".to_vec(),
            &mut log,
        );

        apply(
            &mut browser,
            &mut Shell::default(),
            bulk(
                Bulk::Queue,
                &[Item::Local(owed), Item::Local(opened), Item::Local(nowhere)],
            ),
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        assert_eq!(queue.ids(), vec![owed, opened]);
        assert_eq!(queue.entry(owed).map(|held| held.at), Some(at(0)));
        assert_eq!(
            queue.entry(opened).map(|held| held.at),
            Some(at(1)),
            "the first slot read and found free"
        );
        assert!(
            log.transcript()
                .contains("1 of them belongs in no folder the instrument has"),
            "{}",
            log.transcript()
        );
    }

    /// A set of assets that came off no slot lands on distinct free slots in address
    /// order — the gaps a walk found, skipping what is already waiting — and the one
    /// past the last free slot is refused by folder rather than dropped.
    #[test]
    fn a_queued_set_walks_down_the_free_slots_and_names_the_folder_that_runs_out() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_partitions(&crate::device::ELECTRO5);
        // Four gaps among the residents, and one of them is already spoken for.
        device.pretend_scanned(class, 7, &["", "Africa Split", "", "", "Squabble B", ""]);

        let bytes = program(&mut workspace, &mut log);
        let ids: Vec<u64> = (0..5)
            .map(|n| {
                workspace.ingest(
                    format!("sound {n}.ne5p"),
                    Origin::File(format!("sound {n}.ne5p")),
                    bytes.clone(),
                    &mut log,
                )
            })
            .collect();
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            ids[0],
            class,
            at(2),
        );

        apply(
            &mut browser,
            &mut Shell::default(),
            bulk(
                Bulk::Queue,
                &ids[1..]
                    .iter()
                    .copied()
                    .map(Item::Local)
                    .collect::<Vec<_>>(),
            ),
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        let landed: Vec<(u64, u32)> = queue
            .entries()
            .iter()
            .map(|held| (held.id, held.at.slot))
            .collect();
        assert_eq!(
            landed,
            vec![(ids[0], 2), (ids[1], 0), (ids[2], 3), (ids[3], 5)],
            "each takes the next free slot, and 7:3 was already waiting"
        );
        assert!(!queue.holds(ids[4]), "the free slots ran out before it");
        let said = log.transcript();
        assert!(
            said.contains("“sound 4.ne5p” was not queued: no slot of Programs"),
            "{said}"
        );
    }

    /// A mixed set queues what the instrument takes, leaves out what it does not, and
    /// says how much of the set that was.
    #[test]
    fn queueing_a_mixed_set_queues_only_what_the_instrument_takes() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.pretend_scanned(class, 7, &["", "", ""]);

        let bytes = program(&mut workspace, &mut log);
        let mine = workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::File("Africa Split.ne5p".into()),
            bytes,
            &mut log,
        );
        let stage = workspace.create(Fresh::Stage4Program, &mut log).unwrap();

        apply(
            &mut browser,
            &mut Shell::default(),
            bulk(Bulk::Queue, &[Item::Local(mine), Item::Local(stage)]),
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        assert_eq!(
            queue.ids(),
            vec![mine],
            "only the Electro 5's own is waiting"
        );
        let said = log.transcript();
        assert!(said.contains("1 of 2 fit the Nord Electro 5"), "{said}");
        assert!(said.contains("Stage 4"), "{said}");

        // And the control offering it says as much before it is clicked: the count in
        // its label, and the instrument's own refusal for the one it would leave out.
        let checked = [Item::Local(mine), Item::Local(stage)];
        let held = fits(&checked, &workspace, &device.state);
        assert_eq!(held.label(), "Queue 1 of 2");
        assert!(held
            .why
            .as_deref()
            .is_some_and(|why| why.contains("Stage 4")));

        // Nothing it takes: dead, and the hover is the instrument's reason rather than
        // the checked set's.
        let refused = fits(&[Item::Local(stage)], &workspace, &device.state);
        assert_eq!(refused.takes, 0);
        assert_eq!(refused.label(), "Queue 0 of 1");
        assert!(refused.why.is_some());

        // Everything it takes: its own words, and no reason to show.
        let taken = fits(&[Item::Local(mine)], &workspace, &device.state);
        assert_eq!(taken.label(), Bulk::Queue.label());
        assert_eq!(taken.why, None);
    }

    /// ⚠️ A refused asset never gets an entry, however it was aimed: the queue is what a
    /// send walks, so a foreign body in it would reach a delete-then-write.
    #[test]
    fn a_slot_named_outright_still_refuses_what_the_instrument_does_not_take() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &[""]);
        let stage = workspace.create(Fresh::Stage4Program, &mut log).unwrap();

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::Send {
                id: stage,
                class,
                at: at(0),
            }],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        assert!(queue.is_empty(), "nothing is waiting");
        let said = log.transcript();
        assert!(said.contains("cannot go to"), "{said}");
    }

    /// A row already waiting, dropped on another slot, is re-targeted rather than
    /// duplicated: the drop means the same Send it means from any other row, and one
    /// asset has one entry wherever it was dragged from.
    #[test]
    fn dropping_something_already_waiting_onto_a_slot_moves_its_entry() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        let class = ObjectClass::Program;
        let id = workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::Device { class, at: at(0) },
            bytes,
            &mut log,
        );
        let mut send = |acts, device: &mut Device, queue: &mut Queue| {
            apply(
                &mut browser,
                &mut Shell::default(),
                acts,
                &mut workspace,
                device,
                &mut tabs,
                queue,
                &mut log,
            );
        };
        send(vec![Act::SendChecked(vec![id])], &mut device, &mut queue);
        assert_eq!(queue.entry(id).map(|held| held.at), Some(at(0)));

        // What the queue row carries, landing where the keyboard's cells are.
        let carried = crate::browser::Held {
            what: Item::Local(id),
            kind: crate::browser::Kind::Program,
            filed: None,
            fits: true,
        };
        let onto = crate::browser::Onto::Slot { class, at: at(3) };
        assert_eq!(
            crate::browser::landing(&carried, onto),
            crate::browser::Landing::Send
        );

        send(
            vec![Act::Send {
                id,
                class,
                at: at(3),
            }],
            &mut device,
            &mut queue,
        );
        assert_eq!(queue.ids(), vec![id], "one entry, moved");
        assert_eq!(queue.entry(id).map(|held| held.at), Some(at(3)));
    }

    /// One entry leaves the queue on its own, and the whole queue empties — and neither
    /// takes anything off this computer with it.
    #[test]
    fn an_entry_leaves_the_queue_alone_and_the_queue_empties_without_deleting_anything() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.pretend_scanned(class, 7, &["", "", ""]);
        let bytes = program(&mut workspace, &mut log);
        let ids: Vec<u64> = (0..3)
            .map(|n| {
                workspace.ingest(
                    format!("sound {n}.ne5p"),
                    Origin::File(format!("sound {n}.ne5p")),
                    bytes.clone(),
                    &mut log,
                )
            })
            .collect();

        let mut run = |acts, queue: &mut Queue, workspace: &mut Workspace| {
            apply(
                &mut browser,
                &mut Shell::default(),
                acts,
                workspace,
                &mut device,
                &mut tabs,
                queue,
                &mut log,
            )
        };
        run(
            bulk(
                Bulk::Queue,
                &ids.iter().copied().map(Item::Local).collect::<Vec<_>>(),
            ),
            &mut queue,
            &mut workspace,
        );
        assert_eq!(queue.ids(), ids);

        run(vec![Act::Unqueue(ids[1])], &mut queue, &mut workspace);
        assert_eq!(queue.ids(), vec![ids[0], ids[2]]);

        run(vec![Act::ClearQueue], &mut queue, &mut workspace);
        assert!(queue.is_empty());
        assert_eq!(
            workspace.listed().count(),
            3,
            "a queue is a plan, so emptying it deletes nothing"
        );
    }

    /// Queueing brings the plan into view: whatever put something in the queue leaves
    /// the bottom dock open on the queue page, however shut it was.
    #[test]
    fn queueing_opens_the_dock_on_the_queue_page() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.pretend_scanned(class, 7, &[""]);
        let bytes = program(&mut workspace, &mut log);
        let id = workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::File("Africa Split.ne5p".into()),
            bytes,
            &mut log,
        );

        let mut shell = Shell {
            dock_open: false,
            page: Page::Log,
            ..Shell::default()
        };
        apply(
            &mut browser,
            &mut shell,
            vec![Act::Send {
                id,
                class,
                at: at(0),
            }],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        assert!(queue.holds(id));
        assert!(shell.dock_open);
        assert_eq!(shell.page, Page::Queue);
    }

    /// A folder every read slot of which is taken refuses the entry by name. Dropping it
    /// without a word would read as a bug, and putting it somewhere occupied would be
    /// this app choosing what to overwrite.
    #[test]
    fn queueing_into_a_folder_with_no_free_slot_refuses_and_names_it() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "Squabble B"]);
        let id = workspace.ingest(
            "Jazzy Click B.ne5p".to_string(),
            Origin::File("Jazzy Click B.ne5p".into()),
            bytes,
            &mut log,
        );

        apply(
            &mut browser,
            &mut Shell::default(),
            bulk(Bulk::Queue, &[Item::Local(id)]),
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        assert!(queue.is_empty());
        assert!(
            log.transcript()
                .contains("no slot of Programs is both read and still free"),
            "{}",
            log.transcript()
        );
    }

    /// A batch is one command per folder, because a session belongs to a folder, and it
    /// goes out in the order the queue holds it.
    #[test]
    fn a_batch_is_grouped_into_one_command_per_folder() {
        let (_browser, mut workspace, mut device, _tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        for (class, slot) in [
            (ObjectClass::Program, 0),
            (ObjectClass::Program, 1),
            (ObjectClass::SetList, 0),
        ] {
            let id = workspace.ingest(
                format!("{}.ne5p", place(class, at(slot))),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                bytes.clone(),
                &mut log,
            );
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                at(slot),
            );
        }

        let grouped = grouped(&queue, &workspace);
        assert_eq!(grouped.len(), 2, "one command per folder");
        let programs = grouped
            .iter()
            .find(|(class, _)| *class == ObjectClass::Program)
            .expect("programs are queued");
        assert_eq!(programs.1.len(), 2);
        let slots: Vec<u32> = programs.1.iter().map(|item| item.at.slot).collect();
        assert_eq!(slots, vec![0, 1], "in the order the queue holds them");
    }

    /// A send queues; the write happens when the queue is drained, and it names the
    /// asset each item came from so the debt it pays is that one's.
    #[test]
    fn a_send_queues_and_the_drain_names_the_asset_it_writes() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", ""]);
        let at = Location { bank: 6, slot: 1 };
        let bytes = program(&mut workspace, &mut log);
        let id = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            bytes,
            &mut log,
        );

        let mut act = |acts, device: &mut Device, queue: &mut Queue| {
            apply(
                &mut browser,
                &mut Shell::default(),
                acts,
                &mut workspace,
                device,
                &mut tabs,
                queue,
                &mut log,
            )
        };
        // The slot is empty in the scan and a program carries no write warning, so
        // nothing is asked.
        act(
            vec![Act::Send {
                id,
                class: ObjectClass::Program,
                at,
            }],
            &mut device,
            &mut queue,
        );
        assert_eq!(queue.ids(), vec![id], "queued rather than written");
        assert!(device.queued().is_empty(), "and nothing has been asked for");

        act(vec![Act::SendAll], &mut device, &mut queue);
        match device.queued().front().expect("a batch was queued") {
            DeviceCmd::SendAll { class, items } => {
                assert_eq!(*class, ObjectClass::Program);
                assert_eq!(
                    items.iter().map(|item| item.id).collect::<Vec<_>>(),
                    vec![id]
                );
            }
            other => panic!("{}", other.label()),
        }
        assert_eq!(queue.ids(), vec![id], "still owed until the write lands");
    }

    /// The question before a write says what is known about each slot it is about to
    /// land in, and a bank nothing has read is not an empty one.
    #[test]
    fn the_send_question_says_what_is_known_about_each_slot() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        // Bank 7 was walked: 7:1 holds something and 7:2 is empty. Nothing has read 8.
        device.pretend_scanned(class, 7, &["Africa Split", ""]);
        let bytes = program(&mut workspace, &mut log);
        for (bank, slot) in [(6, 0), (6, 1), (7, 0)] {
            let id = workspace.ingest(
                format!("sound {bank}-{slot}"),
                Origin::Fresh,
                bytes.clone(),
                &mut log,
            );
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                Location { bank, slot },
            );
        }

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::AskSendAll],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        let note = browser
            .ask
            .as_ref()
            .and_then(|ask| ask.note.clone())
            .expect("a question was raised");
        for said in [
            "Programs 7:1 holds “Africa Split”",
            "Programs 7:2 is empty",
            "Programs 8:1 has not been read yet",
        ] {
            assert!(note.contains(said), "{note}");
        }
    }

    /// The queue outlives the instrument it was built against. What the one attached now
    /// refuses is not written, keeps its place with the reason against it, and does not
    /// stop the rest of the batch.
    #[test]
    fn a_send_re_checks_every_entry_against_the_instrument_attached_now() {
        use crate::device::DeviceEvent;

        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B"]);
        let theirs = program(&mut workspace, &mut log);
        let electro = workspace.ingest("Africa-Split.ne5p".into(), Origin::Fresh, theirs, &mut log);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            electro,
            class,
            at(0),
        );

        // Another instrument in its place, and something it does take waiting with it.
        device.pretend(DeviceEvent::Disconnected { lost: true });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        device.pretend_attached_as("Nord Stage 4");
        let made = workspace
            .create(Fresh::Stage4Program, &mut log)
            .expect("a Stage 4 program");
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            made,
            class,
            at(1),
        );
        assert_eq!(queue.ids(), vec![electro, made]);

        // The instrument declares itself, and the queue page says so before anyone has
        // pressed Send.
        device.pretend(DeviceEvent::Partitions(vec![crate::device::Partition {
            class,
            name: "Program".into(),
            native: false,
            unit: None,
        }]));
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        assert!(queue
            .entry(electro)
            .is_some_and(|held| held.failure.is_some()));

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::SendAll],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        let batch = device
            .queued()
            .iter()
            .find(|cmd| matches!(cmd, DeviceCmd::SendAll { .. }))
            .expect("the rest of the batch still goes");
        let DeviceCmd::SendAll { items, .. } = batch else {
            unreachable!()
        };
        assert_eq!(
            items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![made],
            "only what this instrument takes is written"
        );
        assert_eq!(queue.ids(), vec![electro, made], "both are still waiting");
        let refused = queue.entry(electro).expect("it kept its place");
        assert!(
            refused
                .failure
                .as_deref()
                .is_some_and(|why| why.contains("Nord Stage 4")),
            "{:?}",
            refused.failure
        );
        assert!(
            log.transcript().contains("Africa-Split.ne5p” cannot go to"),
            "{}",
            log.transcript()
        );
    }

    /// The queue goes out in the order it was built, one command per folder, and what
    /// a stopped batch did not write is still waiting with the reason against it.
    #[test]
    fn a_batch_that_stops_leaves_the_rest_of_the_queue_waiting() {
        use crate::device::DeviceEvent;

        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B", "Bass Manual"]);
        let bytes = program(&mut workspace, &mut log);
        let ids: Vec<u64> = (0..3)
            .map(|slot| {
                let id = workspace.ingest(
                    format!("sound {slot}"),
                    Origin::Device {
                        class,
                        at: at(slot),
                    },
                    bytes.clone(),
                    &mut log,
                );
                enqueue(
                    &workspace,
                    &mut device,
                    &mut queue,
                    &mut log,
                    id,
                    class,
                    at(slot),
                );
                id
            })
            .collect();

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::SendAll],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        // Three reads of what is there, and then the one batch that writes them.
        let batch = device
            .queued()
            .iter()
            .find(|cmd| matches!(cmd, DeviceCmd::SendAll { .. }))
            .expect("one command for the folder");
        let DeviceCmd::SendAll { items, .. } = batch else {
            unreachable!()
        };
        assert_eq!(
            items.iter().map(|item| item.id).collect::<Vec<_>>(),
            ids,
            "in the order the queue holds them"
        );
        assert_eq!(
            device
                .queued()
                .iter()
                .filter(|cmd| matches!(cmd, DeviceCmd::SendAll { .. }))
                .count(),
            1,
        );

        // The reads of what is in those slots go out first; each one finishing lets the
        // next command start, so the batch is what the instrument is doing when it
        // refuses.
        let waiting = |device: &Device| {
            device
                .queued()
                .iter()
                .any(|cmd| matches!(cmd, DeviceCmd::SendAll { .. }))
        };
        while waiting(&device) {
            device.pump();
            if waiting(&device) {
                device.pretend(DeviceEvent::Finished);
                device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
            }
        }

        // The instrument takes the first and refuses the second.
        device.pretend(DeviceEvent::Sent {
            id: ids[0],
            class,
            at: at(0),
            bytes: workspace.get(ids[0]).unwrap().bytes.clone(),
        });
        device.pretend(DeviceEvent::OpFailed(
            "Programs 7:2 is occupied, and the instrument does not overwrite in place".into(),
        ));
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        assert_eq!(queue.ids(), ids[1..], "what was not written is still owed");
        let stopped = queue.entry(ids[1]).expect("the one it stopped on");
        assert!(
            stopped
                .failure
                .as_deref()
                .is_some_and(|why| why.contains("7:2")),
            "{:?}",
            stopped.failure
        );
        assert!(queue.entry(ids[2]).unwrap().failure.is_none());
        assert!(log.transcript().contains("7:2"), "the log names the slot");
    }

    /// Sending a folder queues everything in it that came off a slot, and nothing that
    /// has nowhere to go back to.
    #[test]
    fn a_folder_queues_only_what_can_go_back_to_a_slot() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        let folder = browser.folders.make();
        for (class, slot) in [
            (ObjectClass::Program, 0),
            (ObjectClass::SetList, 0),
            // The live buffer takes a write like any other slot.
            (ObjectClass::Live, 0),
            // A piano is installed by the instrument, so it must not reach the queue.
            (ObjectClass::Piano, 0),
        ] {
            let id = workspace.ingest(
                format!("{}.ne5p", place(class, at(slot))),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                bytes.clone(),
                &mut log,
            );
            browser.folders.file(id, Some(folder));
        }
        // Never off an instrument, and nothing is attached to offer it a free slot.
        let fresh = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.folders.file(fresh, Some(folder));

        let members: Vec<Item> = browser
            .folders
            .members(folder, &workspace)
            .iter()
            .map(|entity| Item::Local(entity.id))
            .collect();
        apply(
            &mut browser,
            &mut Shell::default(),
            bulk(Bulk::Queue, &members),
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        let classes: Vec<ObjectClass> = queue.entries().iter().map(|held| held.class).collect();
        assert_eq!(
            classes,
            vec![
                ObjectClass::Program,
                ObjectClass::SetList,
                ObjectClass::Live
            ]
        );
        assert!(!queue.holds(fresh), "it is bound for nowhere");
    }

    /// A double-click on a slot opens a view: a tab and a document, and no new row in
    /// the list. Keeping it is what puts it there.
    #[test]
    fn opening_a_slot_does_not_put_it_on_this_computer() {
        use crate::device::DeviceEvent;
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        let at = Location { bank: 6, slot: 3 };
        let origin = Origin::Device {
            class: ObjectClass::Program,
            at,
        };

        device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin,
            bytes,
            why: Purpose::View,
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        let id = tabs.active().expect("a view opens in a tab");
        assert!(workspace.is_view(id));
        assert_eq!(workspace.listed().count(), 0, "nothing joined the list");
        // It is still a working copy in every other way: it knows the slot it came off,
        // so Send back works from it.
        assert_eq!(
            workspace.get(id).unwrap().origin.slot(),
            Some((ObjectClass::Program, at))
        );

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::Keep(id)],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        assert!(!workspace.is_view(id));
        assert_eq!(workspace.listed().count(), 1);
    }

    /// ⚠️ The editor opens on the name the folder actually has. Prefilling it with the
    /// name `make` starts from, beside a folder already called that, is one Enter away
    /// from two folders of one name — which is what `make` picked a different one to
    /// avoid.
    #[test]
    fn a_new_folder_opens_its_editor_on_the_name_it_was_given() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let mut new_folder = |browser: &mut Browser| {
            apply(
                browser,
                &mut Shell::default(),
                vec![Act::NewFolder],
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut queue,
                &mut log,
            );
            let rename = browser.rename.as_ref().expect("the editor is armed");
            let Item::Folder(id) = rename.what else {
                panic!("it is armed on the folder");
            };
            (id, rename.text.clone())
        };

        let (first, typed) = new_folder(&mut browser);
        assert_eq!(typed, "New folder");
        let (second, typed) = new_folder(&mut browser);
        assert_eq!(typed, "New folder 2", "the name it actually has");
        assert_eq!(browser.folders.name_of(second), Some(typed.as_str()));
        assert_ne!(first, second);
    }

    /// A folder that goes while its name is being typed takes the editor with it: no row
    /// will be drawn to close it, and the next folder to take its id would inherit it.
    #[test]
    fn removing_a_folder_mid_rename_takes_the_editor_with_it() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let mut act = |browser: &mut Browser, act| {
            apply(
                browser,
                &mut Shell::default(),
                vec![act],
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut queue,
                &mut log,
            )
        };
        act(&mut browser, Act::NewFolder);
        let Some(Item::Folder(id)) = browser.rename.as_ref().map(|r| r.what) else {
            panic!("a new folder arms its editor");
        };

        act(&mut browser, Act::RemoveFolder(id));
        assert!(browser.rename.is_none(), "the editor went with it");
        assert!(browser.selection.sole().is_none());

        // And the id `make` hands out again is a folder with no editor waiting on it.
        act(&mut browser, Act::NewFolder);
        let Some(Item::Folder(again)) = browser.rename.as_ref().map(|r| r.what) else {
            panic!("the new one arms its own");
        };
        assert_eq!(again, id, "the id came back round");
        assert_eq!(
            browser.rename.as_ref().map(|r| r.text.as_str()),
            Some("New folder")
        );
    }

    /// ⚠️ One view per slot. A second read of a slot already being viewed would be two
    /// working copies of one place — edited apart, both owed back to it, and both queued
    /// into one batch, where the last written wins.
    #[test]
    fn opening_a_slot_that_is_already_open_activates_its_tab() {
        use crate::device::DeviceEvent;
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = program(&mut workspace, &mut log);
        let class = ObjectClass::Program;
        let at = Location { bank: 6, slot: 3 };
        device.pretend_scanned(class, 7, &["", "", "", "Africa Split"]);

        device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin: Origin::Device { class, at },
            bytes,
            why: Purpose::View,
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        let first = tabs.active().expect("a view opened");

        // Another double-click on the same slot.
        tabs.close(Spot::Document(first));
        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::Open(Item::Slot { class, at })],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        assert!(device.queued().is_empty(), "nothing was read again");
        assert_eq!(tabs.active(), Some(first), "its own tab came forward");
        assert_eq!(workspace.entities().len(), 1, "and there is one copy");

        // A slot with no view open is read, as it must be.
        let elsewhere = Location { bank: 6, slot: 4 };
        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::Open(Item::Slot {
                class,
                at: elsewhere,
            })],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        assert_eq!(device.queued().len(), 1);
    }

    /// ⚠️ A file the instrument turns out not to want costs the occupant of the slot —
    /// and the New menu makes another model's program one click away. It warns; it does
    /// not refuse, because nothing here has watched an instrument refuse one.
    #[test]
    fn a_file_of_another_model_is_warned_about_and_not_refused() {
        let held =
            |tags: &[&str]| -> Vec<String> { tags.iter().map(|tag| tag.to_string()).collect() };
        let warning = foreign_format("ns4p", &held(&["ne5p"])).expect("a Stage 4 file here");
        assert!(
            warning.contains("ns4p") && warning.contains("ne5p"),
            "{warning}"
        );
        assert!(warning.contains("replaces"), "{warning}");

        // What the folder is already holding raises nothing, whitespace and case included.
        assert_eq!(foreign_format("ne5p", &held(&["ne5p"])), None);
        assert_eq!(foreign_format(" ne5p ", &held(&["NE5P "])), None);
        assert_eq!(foreign_format("ne5p", &held(&["ne5p", "ne5l"])), None);

        // Not known is not the same as does not match: an unscanned folder says nothing,
        // and neither does a file whose own tag could not be read.
        assert_eq!(foreign_format("ns4p", &[]), None);
        assert_eq!(
            foreign_format("?", &held(&["ne5p"])),
            None,
            "no tag to judge"
        );
        assert_eq!(foreign_format("", &held(&["ne5p"])), None);
    }

    /// One button for the whole column, and it asks for every folder.
    #[test]
    fn a_sync_reads_every_folder_again() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::Resync],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        for class in device.state.classes() {
            let progress = device.state.scan.progress(class);
            assert!(
                progress.is_some_and(|progress| progress.running),
                "{}",
                folder(class)
            );
        }
    }
}
