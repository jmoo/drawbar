//! What the browser asks for, and [`apply`], which runs it against the workspace, the
//! device and the tabs.

use nord_usb::{Location, ObjectClass};

use super::drag::{Item, Kind};
use super::Browser;
use crate::device::{
    fit, read_only, write_warning, Device, DeviceCmd, DeviceState, Fetched, Fit, Outgoing, Payload,
    Purpose,
};
use crate::filter::Narrow;
use crate::folders::{Clash, Folders, Occupant};
use crate::log::Log;
use crate::newproject::Making;
use crate::queue::{enqueue, retarget, Occupancy, Queue, Queued};
use crate::shell::{Dock, Shell};
use crate::store::{names, outside_len, CopyOf, LibPath, Outside, Part};
use crate::strings::place;
use crate::tabs::{Spot, Tabs};
use crate::workspace::{Fresh, LocalEntity, Origin, Unbundled, VerifyState, Workspace};

/// What [`Act::LoadOnInstrument`] is called wherever it is offered.
pub const LOAD_ON_INSTRUMENT: &str = "Load on instrument";

/// What the browser asks the rest of the app to do.
#[derive(PartialEq, Eq, Debug)]
pub enum Act {
    Connect,
    Disconnect,
    OpenFiles,
    /// Fetch the demo sounds; see [`crate::demo`].
    FetchDemos,
    /// File the demo sounds a fetch brought back.
    Demos(Vec<(String, Vec<u8>)>),
    New(Fresh),
    /// Pick the WAVs a new project or instrument is built from. It is built once the
    /// dialog has each file's root key; see [`crate::newproject`].
    NewFromWavs(Making),
    /// Read the whole instrument again: every class, its geometry and its focus.
    Resync,
    ReadAgain(ObjectClass),
    Open(Item),
    /// A view of a slot becomes an asset on this computer.
    Keep(u64),
    /// Take a file from outside the library into its top level, under its own name.
    Import {
        name: String,
        bytes: Vec<u8>,
    },
    /// Copy a file from outside the library into the folder `dir`, as `name`.
    Take {
        from: Outside,
        dir: LibPath,
        name: String,
    },
    /// Unpack a bundle whose directory has been read into a new folder.
    Unpack(Unbundled),
    /// Copy a file from outside the library over the file of the asset `id`, which keeps
    /// its id, folder and tags.
    TakeOver {
        id: u64,
        from: Outside,
    },
    /// Put bytes over an existing asset, which keeps its id, folder and tags. `gone` is an
    /// asset the overwrite came from, one renamed or moved onto the name, removed once the
    /// bytes are saved.
    Overwrite {
        id: u64,
        bytes: Vec<u8>,
        gone: Option<u64>,
    },
    /// Copy the file the asset `from` rests in, with any edit held of it, over the file
    /// of the asset `id`, which keeps its id, folder and tags, and remove `from` once
    /// the copy has landed: the overwrite of an asset renamed or moved onto a name, where
    /// it rests in its file.
    CopyOver {
        id: u64,
        from: u64,
    },
    /// Move an asset into a folder, or the top level for `None`, under `name`.
    MoveAs {
        id: u64,
        folder: Option<u64>,
        name: String,
    },
    /// Keep an unsaved edit as a new asset beside its file, and take the file as it is on
    /// disk.
    KeepBoth(u64),
    /// Let go of an index row whose file is gone, with the tags it kept.
    Forget(u64),
    NewFolder,
    NewFolderIn(u64),
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
    /// A new tag on these assets, with its rename editor open and in view.
    NewTag(Vec<u64>),
    RenameTag {
        id: u64,
        name: String,
    },
    RemoveTag(u64),
    /// Put an asset in a folder, or out of the one it is in.
    File {
        id: u64,
        folder: Option<u64>,
    },
    /// Queue each of these assets for the slot `bound_for` gives it. The log says which
    /// were not queued, and why.
    SendChecked(Vec<u64>),
    Copy {
        class: ObjectClass,
        at: Location,
    },
    /// Copy these slots of one class to this computer, in one session.
    CopyAll {
        class: ObjectClass,
        slots: Vec<Location>,
    },
    /// File an object the instrument read into a file, copying it into the library.
    Arrive(Fetched),
    /// Write these assets, and what they need, as one bundle, once these slots have been
    /// copied to this computer and the assets `reading` names have been read.
    ExportBundle {
        ids: Vec<u64>,
        slots: Vec<(ObjectClass, Location)>,
        reading: Vec<u64>,
    },
    LoadOnInstrument {
        class: ObjectClass,
        at: Location,
    },
    /// Queue a local asset for a slot, asking first where the write both carries a
    /// warning and replaces an occupant.
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
    /// Empty the queue. Nothing is deleted, so it asks nothing.
    ClearQueue,
    /// Write everything in the queue, grouped by folder. Already confirmed.
    SendAll,
    /// Queue every asset [`crate::queue::changed`] finds, each for its own slot.
    QueueChanged,
    /// Ask before sending everything waiting: the review of the queue, whose Send all
    /// runs `SendAll`.
    AskSendAll,
    /// A Send already confirmed, so it does not ask again.
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
    /// ⌘S on the open document; `save_doc` says what saving means for each kind.
    SaveDoc(u64),
    /// The write back to a slot that [`Act::SaveDoc`] on a view asked about, confirmed.
    WriteBack(u64),
    /// Revert to the bytes it was last saved as.
    Revert(u64),
    /// Bring one of the center's tabs forward.
    ShowTab(Spot),
    /// Show the keyboard tab, switched to one class.
    ShowClass(ObjectClass),
    /// Turn one of the library's filters on or off.
    Narrow(Narrow),
    /// Show the browser with this row's branches open, for a rename that types in it.
    Reveal(Item),
    /// Close the tab the center is showing.
    CloseTab,
    ToggleDock(Dock),
    /// Open the review of what is waiting to be sent.
    ReviewQueue,
    /// Show or hide the activity popover.
    ToggleLog,
    /// Open the activity popover on the problems alone.
    ShowProblems,
    /// Copy the whole activity log to the clipboard.
    CopyLog,
    /// Ask the window to close. Never reached on the web, where the tab is the window.
    Quit,
    /// Pick a folder to open as the library.
    PickLibrary,
    /// Open this library in place of the one open now. The app runs it, not [`apply`],
    /// once every piano plan over the open library's assets is laid out.
    OpenLibrary(crate::store::Root),
    /// [`Act::OpenLibrary`], confirmed though the library open now loses what it cannot
    /// keep.
    OpenLibraryDiscarding(crate::store::Root),
    /// Do this with a slot's former occupant an interrupted write left. The app runs
    /// it, not [`apply`], since it reaches the library's files.
    Rescue(crate::store::Rescue, Rescuing),
    /// Delete the `copies` working copies a library with no index keeps, and open it
    /// again without them: asked again first unless `confirmed`. The app runs the
    /// confirmed one, not [`apply`], since it reaches the library's files.
    DropUnindexed {
        copies: usize,
        confirmed: bool,
    },
    /// Set aside the index of a library that does not read it, and open it again without
    /// it: asked again first unless `confirmed`. The app runs the confirmed one, not
    /// [`apply`], since it reaches the library's files.
    SetAside {
        confirmed: bool,
    },
    /// Nothing happened, and this is why.
    Refused(String),
}

/// What to do with a slot's former occupant an interrupted write left on this computer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rescuing {
    /// Move it into the library's top level as an asset.
    Keep,
    /// Show its folder in the system's file manager, and ask again.
    Show,
    /// Ask before it is deleted.
    Confirm,
    Discard,
}

impl Act {
    /// The assets whose bytes the act carries out of the app: saved into their files,
    /// exported, queued, or written to the instrument.
    pub fn carries(&self, queue: &Queue) -> Vec<u64> {
        match self {
            Act::SaveDoc(id)
            | Act::WriteBack(id)
            | Act::Export(id)
            | Act::Send { id, .. }
            | Act::Retarget { id, .. }
            | Act::Replace { id, .. } => vec![*id],
            Act::SendChecked(ids) => ids.clone(),
            Act::ExportBundle { ids, .. } => ids.clone(),
            Act::SendAll => will_write(queue).map(|held| held.id).collect(),
            _ => Vec::new(),
        }
    }

    /// The assets on this computer whose contents the act works from. It waits until
    /// each has been read, and decodes it first where that is still to be done.
    pub fn reads(&self) -> Vec<u64> {
        match self {
            Act::Open(item) => item.local().into_iter().collect(),
            Act::Overwrite { id, .. } | Act::MoveAs { id, .. } | Act::File { id, .. } => {
                vec![*id]
            }
            Act::Keep(id)
            | Act::KeepBoth(id)
            | Act::Send { id, .. }
            | Act::Retarget { id, .. }
            | Act::Replace { id, .. }
            | Act::DuplicateLocal(id)
            | Act::Export(id)
            | Act::SaveDoc(id)
            | Act::WriteBack(id)
            | Act::Revert(id) => vec![*id],
            Act::SendChecked(ids) => ids.clone(),
            Act::ExportBundle { ids, reading, .. } => [ids.as_slice(), reading].concat(),
            Act::Connect
            | Act::Disconnect
            | Act::OpenFiles
            | Act::FetchDemos
            | Act::Demos(_)
            | Act::New(_)
            | Act::NewFromWavs(_)
            | Act::Resync
            | Act::ReadAgain(_)
            | Act::Import { .. }
            | Act::Take { .. }
            | Act::Unpack(_)
            | Act::TakeOver { .. }
            | Act::CopyOver { .. }
            | Act::Forget(_)
            | Act::NewFolder
            | Act::NewFolderIn(_)
            | Act::RemoveFolder(_)
            | Act::Tag { .. }
            | Act::Untag { .. }
            | Act::NewTag(_)
            | Act::RenameTag { .. }
            | Act::RemoveTag(_)
            | Act::Copy { .. }
            | Act::CopyAll { .. }
            | Act::Arrive(_)
            | Act::LoadOnInstrument { .. }
            | Act::Unqueue(_)
            | Act::ClearQueue
            | Act::SendAll
            | Act::QueueChanged
            | Act::AskSendAll
            | Act::Rearrange { .. }
            | Act::RenameLocal { .. }
            | Act::RenameFolder { .. }
            | Act::RenameSlot { .. }
            | Act::DuplicateSlot { .. }
            | Act::DeleteSlot { .. }
            | Act::Remove(_)
            | Act::ShowTab(_)
            | Act::ShowClass(_)
            | Act::Narrow(_)
            | Act::Reveal(_)
            | Act::CloseTab
            | Act::ToggleDock(_)
            | Act::ReviewQueue
            | Act::ToggleLog
            | Act::ShowProblems
            | Act::CopyLog
            | Act::Quit
            | Act::PickLibrary
            | Act::OpenLibrary(_)
            | Act::OpenLibraryDiscarding(_)
            | Act::Rescue(..)
            | Act::DropUnindexed { .. }
            | Act::SetAside { .. }
            | Act::Refused(_) => Vec::new(),
        }
    }
}

/// The bulk actions on the checked set, in the order the Selection card and a checked
/// row's menu offer them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bulk {
    Queue,
    Copy,
    Export,
    /// Write the checked set, and what it needs, as one bundle.
    Bundle,
    Tag,
    Delete,
}

impl Bulk {
    pub const ALL: [Bulk; 6] = [
        Bulk::Queue,
        Bulk::Copy,
        Bulk::Export,
        Bulk::Bundle,
        Bulk::Tag,
        Bulk::Delete,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Bulk::Queue => "Queue for sending",
            Bulk::Copy => "Copy to this computer",
            Bulk::Export => "Export…",
            Bulk::Bundle => "Export as bundle…",
            Bulk::Tag => "Tag…",
            Bulk::Delete => "Delete…",
        }
    }

    /// Why the control is disabled, shown on hover.
    pub fn nothing(self) -> &'static str {
        match self {
            Bulk::Queue | Bulk::Export | Bulk::Tag => "nothing checked is on this computer",
            Bulk::Copy => "nothing checked is on the instrument",
            Bulk::Bundle => "nothing is checked",
            Bulk::Delete => "nothing is checked",
        }
    }
}

/// How much of a checked set the attached instrument would take, and why it refuses the
/// rest.
///
/// ⚠️ Only what is on this computer is counted. A slot is already on the instrument, and
/// nothing here would write it back.
pub struct Fits {
    pub takes: usize,
    pub of: usize,
    /// The first refusal's reason, which the hover over a disabled control shows.
    pub why: Option<String>,
}

impl Fits {
    /// The Queue control's label: the usual one, or a count once the instrument refuses
    /// some of the set.
    pub fn label(&self) -> String {
        match self.takes < self.of {
            true => format!("Queue {} of {}", self.takes, self.of),
            false => Bulk::Queue.label().to_string(),
        }
    }
}

/// How much of a checked set the instrument takes, from [`fit`] on each local asset.
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
        let verdict = match Kind::of(entity).home() {
            Some(_) => fit(state, entity),
            None => Fit::Refuses(format!(
                "“{}” belongs in no folder the instrument has.",
                entity.name
            )),
        };
        match verdict.allowed() {
            true => held.takes += 1,
            false => held.why = held.why.or_else(|| verdict.why().map(str::to_string)),
        }
    }
    held
}

/// Whether a bundle would carry anything checked: a slot holding something, or an asset
/// of a kind [`crate::bundle::carries`].
pub fn bundled(checked: &[Item], workspace: &Workspace, state: &DeviceState) -> bool {
    checked.iter().any(|item| match *item {
        Item::Local(id) => workspace
            .get(id)
            .is_some_and(|entity| crate::bundle::carries(Kind::of(entity))),
        Item::Slot { class, at } => state.slot(class, at).flatten().is_some(),
        Item::Folder(_) | Item::Tag(_) => false,
    })
}

/// The acts one bulk action runs over the checked set.
///
/// [`Bulk::Tag`] returns none: the tag is picked from its own menu, which makes the act.
pub fn bulk(action: Bulk, checked: &[Item], state: &DeviceState) -> Vec<Act> {
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
        Bulk::Copy => {
            let mut by_class: Vec<(ObjectClass, Vec<Location>)> = Vec::new();
            for item in checked {
                // A slot the scan found empty holds nothing to ask the instrument for.
                let Item::Slot { class, at } = *item else {
                    continue;
                };
                if state.slot(class, at).flatten().is_none() {
                    continue;
                }
                match by_class.iter_mut().find(|(held, _)| *held == class) {
                    Some((_, slots)) => slots.push(at),
                    None => by_class.push((class, vec![at])),
                }
            }
            by_class
                .into_iter()
                .map(|(class, slots)| Act::CopyAll { class, slots })
                .collect()
        }
        Bulk::Export => checked
            .iter()
            .copied()
            .filter_map(Item::local)
            .map(Act::Export)
            .collect(),
        Bulk::Bundle => {
            let ids: Vec<u64> = checked.iter().copied().filter_map(Item::local).collect();
            let slots: Vec<(ObjectClass, Location)> = checked
                .iter()
                .filter_map(|item| match *item {
                    Item::Slot { class, at } => {
                        state.slot(class, at).flatten().map(|_| (class, at))
                    }
                    _ => None,
                })
                .collect();
            match ids.is_empty() && slots.is_empty() {
                true => Vec::new(),
                false => vec![Act::ExportBundle {
                    ids,
                    slots,
                    reading: Vec::new(),
                }],
            }
        }
        Bulk::Tag => Vec::new(),
        Bulk::Delete => checked
            .iter()
            .filter_map(|item| match item {
                Item::Local(id) => Some(Act::Remove(*id)),
                // A slot the scan found empty holds nothing to delete, and asking would
                // cost a round trip that can only fail.
                Item::Slot { class, at } => {
                    state.slot(*class, *at).flatten().map(|_| Act::DeleteSlot {
                        class: *class,
                        at: *at,
                    })
                }
                Item::Folder(_) | Item::Tag(_) => None,
            })
            .collect(),
    }
}

/// Whether an act may run now.
enum Ready {
    Now,
    /// An asset it reads is still being read, or a folder it removes still being listed.
    Later,
    /// An asset it reads could not be read, and this says so.
    Never(String),
}

/// Whether every asset `act` reads has been read, and everything in a folder it removes
/// has been listed. One not read yet is asked for, and so is the listing of a folder, and
/// the act waits for them.
fn ready(act: &Act, workspace: &mut Workspace, folders: &mut Folders) -> Ready {
    if let Some(dir) = removing(act, folders).filter(|dir| !folders.listed_whole(dir)) {
        folders.walk(&dir);
        return Ready::Later;
    }
    let mut later = false;
    for id in act.reads() {
        let Some(entity) = workspace.get(id).filter(|entity| entity.unread()) else {
            continue;
        };
        if let VerifyState::NotRead(why) = &entity.verify() {
            return Ready::Never(format!("“{}” could not be read: {why}.", entity.name));
        }
        workspace.hurry(id);
        later = true;
    }
    match later {
        true => Ready::Later,
        false => Ready::Now,
    }
}

/// The folder an act removes, if it removes one.
fn removing(act: &Act, folders: &Folders) -> Option<LibPath> {
    match act {
        Act::RemoveFolder(id) => folders.path_of(*id).cloned(),
        _ => None,
    }
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
    let mut held = std::mem::take(&mut browser.held);
    held.extend(acts);
    for act in held {
        match ready(&act, workspace, &mut browser.folders) {
            Ready::Now => {}
            Ready::Later => {
                browser.held.push(act);
                continue;
            }
            Ready::Never(why) => {
                log.trouble(why);
                continue;
            }
        }
        workspace.read_now(act.reads(), log);
        match act {
            Act::Connect => device.connect(log),
            Act::Disconnect => device.disconnect(log),
            Act::OpenFiles => workspace.open_dialog(),
            Act::FetchDemos => {
                workspace.fetch_demos();
                log.say("Fetching the demo sounds…");
            }
            Act::Demos(files) => {
                crate::demo::file(files, workspace, &mut browser.folders, log);
            }
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
            Act::Import { name, bytes } => import(browser, workspace, log, name, bytes),
            Act::Take { from, dir, name } => take(browser, workspace, log, from, dir, name),
            Act::Unpack(read) => unpack(browser, workspace, log, read),
            Act::TakeOver { id, from } => take_over(workspace, log, id, from),
            Act::Overwrite { id, bytes, gone } => {
                if let Some(gone) = workspace.save_over(id, bytes, gone, log) {
                    remove(browser, workspace, tabs, queue, log, gone);
                }
                if let Some(entity) = workspace.get(id) {
                    log.say(format!("Replaced “{}”.", entity.name));
                }
            }
            Act::CopyOver { id, from } => copy_over(workspace, log, id, from),
            Act::MoveAs { id, folder, name } => match browser.folders.dir(folder) {
                Some(dir) => put(browser, workspace, log, id, dir, name),
                None => log.say("That folder is gone, so nothing moved."),
            },
            Act::KeepBoth(id) => keep_both(browser, workspace, log, id),
            Act::Forget(id) => {
                browser.folders.forget(id);
                browser.tags.forget(id);
            }
            Act::NewFolder => new_folder(browser, workspace, None),
            Act::NewFolderIn(parent) => new_folder(browser, workspace, Some(parent)),
            Act::RemoveFolder(id) => remove_folder(browser, workspace, log, id),
            Act::File { id, folder } => {
                let (Some(dir), Some(entity)) = (browser.folders.dir(folder), workspace.get(id))
                else {
                    continue;
                };
                let name = match &entity.path {
                    Some(path) => path.leaf().to_string(),
                    None => crate::workspace::library_filename(entity),
                };
                put(browser, workspace, log, id, dir, name);
            }
            Act::Tag { ids, tag } => tag_all(browser, workspace, log, &ids, tag),
            Act::Untag { ids, tag } => {
                for id in ids {
                    browser.tags.set(id, tag, false);
                }
            }
            Act::NewTag(ids) => match browser.tags.make("New tag") {
                // ⚠️ Edit the unique name chosen by `make`, not its generic seed: two
                // tags with one name would show as one row twice.
                Some(id) => {
                    tag_all(browser, workspace, log, &ids, id);
                    let name = browser.tags.name_of(id).unwrap_or_default().to_string();
                    browser.start_rename(Item::Tag(id), &name);
                    browser.reveal(Item::Tag(id), workspace);
                    shell.browser_open = true;
                }
                None => log.trouble("The tag list is full, so there is no new tag."),
            },
            Act::RenameTag { id, name } => browser.tags.rename(id, name),
            Act::RemoveTag(id) => {
                // ⚠️ A removed row cannot close its rename state; a reused id would inherit it.
                browser.forget_rename(Item::Tag(id));
                browser.tags.remove(id);
                // ⚠️ A removed tag must also stop filtering the library.
                shell.filter.forget_tag(id);
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
                        why: Purpose::View,
                    },
                    log,
                ),
            },
            Act::Copy { class, at } => device.send(
                DeviceCmd::Get {
                    class,
                    at,
                    why: Purpose::Copy,
                },
                log,
            ),
            Act::CopyAll { class, slots } => device.send(DeviceCmd::CopyAll { class, slots }, log),
            Act::Arrive(fetched) => arrive(browser, workspace, fetched),
            Act::ExportBundle { ids, slots, .. } => {
                export_bundle(browser, workspace, device, log, ids, slots)
            }
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
            Act::Unqueue(id) => {
                queue.forget(id);
                if let Some(entity) = workspace.get(id) {
                    log.say(format!(
                        "“{}” is no longer waiting to be sent.",
                        entity.name
                    ));
                }
            }
            Act::ClearQueue => {
                queue.clear();
                log.say("The send queue is empty. Nothing was sent.");
            }
            Act::SendAll => send_batch(queue, workspace, device, log),
            Act::QueueChanged => crate::queue::queue_changed(workspace, device, queue, log),
            // ⚠️ The queue always belongs to an attached instrument; with none there is
            // nothing to review.
            Act::AskSendAll | Act::ReviewQueue => shell.review_open = device.state.connected(),
            Act::Rearrange { class, from, to } => {
                device.send(DeviceCmd::Move { class, from, to }, log)
            }
            Act::RenameLocal { id, name } => rename(browser, workspace, log, id, name),
            Act::RenameFolder { id, name } => rename_folder(browser, workspace, log, id, name),
            Act::RenameSlot { class, at, name } => {
                device.send(DeviceCmd::Rename { class, at, name }, log)
            }
            Act::DuplicateLocal(id) => duplicate(browser, workspace, log, id),
            Act::DuplicateSlot { class, from, to } => {
                device.send(DeviceCmd::Duplicate { class, from, to }, log)
            }
            Act::DeleteSlot { class, at } => {
                browser.forget_rename(Item::Slot { class, at });
                device.send(DeviceCmd::Delete { class, at }, log)
            }
            Act::Remove(id) => remove(browser, workspace, tabs, queue, log, id),
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
            Act::Reveal(item) => {
                browser.reveal(item, workspace);
                shell.browser_open = true;
            }
            Act::CloseTab => {
                tabs.close(tabs.showing());
            }
            Act::ToggleDock(dock) => shell.toggle(dock),
            Act::ToggleLog => {
                shell.log_open = !shell.log_open;
                shell.log_problems = false;
            }
            Act::ShowProblems => {
                shell.log_open = !shell.log_open || !shell.log_problems;
                shell.log_problems = true;
            }
            Act::CopyLog => workspace.ctx().copy_text(log.transcript()),
            Act::Quit => workspace
                .ctx()
                .send_viewport_cmd(eframe::egui::ViewportCommand::Close),
            Act::Refused(why) => log.say(why),
            Act::DropUnindexed {
                copies,
                confirmed: false,
            } => browser.ask_drop_unindexed(copies),
            Act::SetAside { confirmed: false } => browser.ask_set_aside(),
            // The app takes these before the browser's acts run.
            Act::PickLibrary
            | Act::OpenLibrary(_)
            | Act::OpenLibraryDiscarding(_)
            | Act::Rescue(..)
            | Act::DropUnindexed {
                confirmed: true, ..
            }
            | Act::SetAside { confirmed: true } => {}
        }
    }
}

/// Take an asset off this computer, and its file with it.
fn remove(
    browser: &mut Browser,
    workspace: &mut Workspace,
    tabs: &mut Tabs,
    queue: &mut Queue,
    log: &mut Log,
    id: u64,
) {
    tabs.close(Spot::Document(id));
    queue.forget(id);
    // ⚠️ A removed row cannot close its rename state; a reused id would inherit it.
    browser.forget_rename(Item::Local(id));
    browser.tags.forget(id);
    browser.folders.missing.remove(&id);
    workspace.remove(id, log);
}

/// How a folder is named in a sentence.
fn spoken(dir: &LibPath) -> String {
    match dir.is_root() {
        true => "the top level of the library".to_string(),
        false => format!("“{dir}”"),
    }
}

/// Put an asset at `name` in `dir`, or ask what to do about what is already there.
///
/// ⚠️ A name the user typed is refused, never made portable behind their back, and an
/// existing entry is never replaced without asking.
fn put(
    browser: &mut Browser,
    workspace: &mut Workspace,
    log: &mut Log,
    id: u64,
    dir: LibPath,
    name: String,
) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    if let Some(why) = names::refusal(&name) {
        return log.trouble(format!("“{name}” cannot be a file's name: {why}."));
    }
    if entity.path.as_ref() == Some(&dir.join(&name)) {
        return;
    }
    match browser
        .folders
        .clash(&dir, &name, workspace, Some(Occupant::Asset(id)))
    {
        Clash::Free => {
            workspace.place(id, dir.join(&name));
            log.say(format!("“{name}” is in {}.", spoken(&dir)));
        }
        Clash::Ambiguous(held) => log.trouble(ambiguous(&held, &dir)),
        Clash::Taken(occupant) => {
            let folder = browser.folders.id_of(&dir);
            let free = browser.folders.free(&dir, &name, workspace);
            let both = vec![Act::MoveAs {
                id,
                folder,
                name: free.clone(),
            }];
            // One resting in its file is copied over by the library, never held whole.
            let over = match entity.rests() {
                Some(_) => overwritable(occupant, workspace)
                    .map(|held| vec![Act::CopyOver { id: held, from: id }]),
                None => match entity.whole() {
                    Ok(bytes) => over(occupant, workspace, bytes.into_owned(), Some(id)),
                    Err(e) => {
                        log.error(format!("{}: {e}", entity.name));
                        None
                    }
                },
            };
            browser.ask_clash(&name, &dir, over, both, &free);
        }
    }
}

/// The overwrite a clash offers, or `None` when the entry there may not be overwritten:
/// a folder, a row whose file is gone, or an asset holding the only copy of something.
fn over(
    occupant: Occupant,
    workspace: &Workspace,
    bytes: Vec<u8>,
    gone: Option<u64>,
) -> Option<Vec<Act>> {
    let id = overwritable(occupant, workspace)?;
    Some(vec![Act::Overwrite { id, bytes, gone }])
}

/// The asset a clash may overwrite: not a folder, a row whose file is gone, or an asset
/// holding the only copy of something.
fn overwritable(occupant: Occupant, workspace: &Workspace) -> Option<u64> {
    let Occupant::Asset(held) = occupant else {
        return None;
    };
    // The queue is not consulted: an asset waiting to be sent can take new bytes, and
    // the queue diffs them again.
    (!workspace.get(held)?.is_unsaved()).then_some(held)
}

fn ambiguous(held: &[String], dir: &LibPath) -> String {
    let named: Vec<String> = held.iter().map(|name| format!("“{name}”")).collect();
    format!(
        "{} in {} differ only by case and would collide on macOS or Windows. Rename one \
         of them first.",
        named.join(" and "),
        spoken(dir)
    )
}

/// A file opened from outside the library, into its top level.
fn import(
    browser: &mut Browser,
    workspace: &mut Workspace,
    log: &mut Log,
    name: String,
    bytes: Vec<u8>,
) {
    if crate::bundle::is_bundle(&name) {
        return log.trouble(format!(
            "“{name}” was not imported: a bundle unpacks into the library, which cannot \
             take files now."
        ));
    }
    if let Some(why) = names::refusal(&name) {
        return log.trouble(format!(
            "“{name}” was not taken onto this computer: {why}. Rename it and open it again."
        ));
    }
    let root = LibPath::root();
    match browser.folders.clash(&root, &name, workspace, None) {
        Clash::Free => {
            let id = workspace.ingest(name.clone(), Origin::File(name.clone()), bytes, log);
            workspace.place(id, root.join(&name));
        }
        Clash::Ambiguous(held) => log.trouble(ambiguous(&held, &root)),
        Clash::Taken(occupant) => {
            let free = browser.folders.free(&root, &name, workspace);
            let both = vec![Act::Import {
                name: free.clone(),
                bytes: bytes.clone(),
            }];
            let over = over(occupant, workspace, bytes, None);
            browser.ask_clash(&name, &root, over, both, &free);
        }
    }
}

/// A file from outside the library, copied into the folder `dir` as `name`, where it is
/// then read as any file of the library's own.
fn take(
    browser: &mut Browser,
    workspace: &mut Workspace,
    log: &mut Log,
    from: Outside,
    dir: LibPath,
    name: String,
) {
    if crate::bundle::is_bundle(&name) {
        return workspace.unbundle(from, dir, name);
    }
    if let Some(why) = names::refusal(&name) {
        return log.trouble(format!(
            "“{name}” was not taken onto this computer: {why}. Rename it and open it again."
        ));
    }
    let Some(len) = outside_len(&from) else {
        return log.trouble(format!("“{name}” could not be read."));
    };
    match browser.folders.clash(&dir, &name, workspace, None) {
        Clash::Free => {
            let origin = Origin::File(name.clone());
            workspace.arrive(dir.join(&name), origin, CopyOf::Outside(from), len);
        }
        Clash::Ambiguous(held) => log.trouble(ambiguous(&held, &dir)),
        Clash::Taken(occupant) => {
            let free = browser.folders.free(&dir, &name, workspace);
            let over = overwritable(occupant, workspace).map(|id| {
                vec![Act::TakeOver {
                    id,
                    from: from.clone(),
                }]
            });
            let both = vec![Act::Take {
                from,
                dir: dir.clone(),
                name: free.clone(),
            }];
            browser.ask_clash(&name, &dir, over, both, &free);
        }
    }
}

/// A bundle's members, each copied out of it into a new folder in `dir` named after the
/// bundle. The folder is flat: the archive's own folders are left behind.
fn unpack(browser: &mut Browser, workspace: &mut Workspace, log: &mut Log, read: Unbundled) {
    let Unbundled {
        from,
        dir,
        name,
        members,
    } = read;
    let members = match members {
        Ok(members) => members,
        Err(why) => return log.trouble(format!("“{name}” was not imported: {why}.")),
    };
    let stem = name
        .rsplit_once('.')
        .map_or(name.as_str(), |(stem, _)| stem);
    let wanted = browser
        .folders
        .free(&dir, &names::portable(stem), workspace);
    if browser
        .folders
        .named_or_made(&dir, &wanted, workspace)
        .is_none()
    {
        return log.trouble(format!("“{name}” was not imported: “{wanted}” is taken."));
    }
    let folder = dir.join(&wanted);
    for member in &members {
        let leaf = browser
            .folders
            .free(&folder, &names::portable(member.leaf()), workspace);
        let len = member.bytes.end - member.bytes.start;
        let from = CopyOf::Part(Part {
            from: from.clone(),
            bytes: member.bytes.clone(),
            crc32: member.crc32,
        });
        workspace.arrive(folder.join(&leaf), Origin::File(leaf.clone()), from, len);
    }
    log.say(format!(
        "Imported the {} files of “{name}” into “{wanted}”.",
        members.len()
    ));
}

/// An object read off the instrument into a file, copied into the top level of the
/// library under its slot's name, numbered past any name taken there. The file goes once
/// the copy has answered.
fn arrive(browser: &Browser, workspace: &mut Workspace, fetched: Fetched) {
    let root = LibPath::root();
    let wanted = names::portable(&format!("{}.{}", fetched.name, fetched.tag));
    let name = browser.folders.free(&root, &wanted, workspace);
    let origin = Origin::Device {
        class: fetched.class,
        at: fetched.at,
    };
    workspace.arrive_fetched(root.join(&name), origin, fetched.file, fetched.len);
}

/// Write `ids` and what they need as one bundle. Slots are copied to this computer first,
/// in one session per class, and the bundle waits for them; see
/// [`Workspace::bundle_after`].
fn export_bundle(
    browser: &mut Browser,
    workspace: &mut Workspace,
    device: &mut Device,
    log: &mut Log,
    ids: Vec<u64>,
    slots: Vec<(ObjectClass, Location)>,
) {
    if !slots.is_empty() {
        log.say(format!(
            "Copying {} and what they play from the instrument for the bundle…",
            crate::strings::counted(slots.len(), "sound", "sounds")
        ));
        let (roots, also) = slots
            .into_iter()
            .partition(|(class, _)| matches!(class, ObjectClass::SetList | ObjectClass::Program));
        if !device.state.connected() {
            return log.trouble("No instrument is attached.");
        }
        let request = workspace.bundle_after(ids);
        return device.send(
            DeviceCmd::Gather {
                roots,
                also,
                request,
            },
            log,
        );
    }
    match crate::bundle::lay_out(&ids, workspace, &device.state) {
        Ok(crate::bundle::Laid::Read(reading)) => browser.held.push(Act::ExportBundle {
            ids,
            slots: Vec::new(),
            reading,
        }),
        Ok(crate::bundle::Laid::Ready(export)) => {
            for why in &export.left_out {
                log.trouble(format!("Left out of the bundle: {why}."));
            }
            for (path, need) in &export.plan.unmet {
                log.trouble(format!(
                    "{path} needs {}, which is not on this computer.",
                    crate::bundle::needed(*need)
                ));
            }
            workspace.export_bundle(export);
        }
        Err(why) => log.trouble(format!("No bundle was written: {why}.")),
    }
}

/// A file from outside the library, copied over the file of the asset `id`.
fn take_over(workspace: &mut Workspace, log: &mut Log, id: u64, from: Outside) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    let name = entity.name.clone();
    match outside_len(&from) {
        Some(len) => workspace.arrive_over(id, CopyOf::Outside(from), len),
        None => log.trouble(format!("The file to put over “{name}” could not be read.")),
    }
}

/// Rename an asset. A view's name is the slot's, and only a kept asset has a file.
fn rename(browser: &mut Browser, workspace: &mut Workspace, log: &mut Log, id: u64, name: String) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    let Some(path) = entity.path.clone().filter(|_| entity.kept) else {
        workspace.rename(id, name.clone());
        return log.say(format!("Renamed it “{name}”."));
    };
    // Every file carries its extension, so a name typed without one keeps the old one.
    let name = match crate::strings::carries_tag(&name) {
        true => name,
        false => crate::strings::tagged(&entity.name, &name),
    };
    put(browser, workspace, log, id, path.parent(), name);
}

/// Copy the file the asset `from` rests in over the file of the asset `id`. `from` goes
/// once the copy has landed, and stays where it did not.
fn copy_over(workspace: &mut Workspace, log: &mut Log, id: u64, from: u64) {
    if !unapplied(workspace, log, from) {
        workspace.move_over(id, from);
    }
}

/// Refuse to copy an asset holding an edit that does not apply to the file it rests in,
/// saying so: no file holds what the edit makes of it.
fn unapplied(workspace: &Workspace, log: &mut Log, id: u64) -> bool {
    let (Some(entity), Some(why)) = (workspace.get(id), workspace.unapplied(id)) else {
        return false;
    };
    log.error(format!("{}: {why}", entity.name));
    log.trouble(format!(
        "“{}” was not copied: its edit does not apply to its file as it is now. Revert it, \
         or edit it again.",
        entity.name
    ));
    true
}

fn duplicate(browser: &mut Browser, workspace: &mut Workspace, log: &mut Log, id: u64) {
    let dir = workspace
        .get(id)
        .and_then(|entity| entity.path.as_ref())
        .map(LibPath::parent);
    if unapplied(workspace, log, id) {
        return;
    }
    // One resting in its file is copied by the library, never held whole.
    if let (Some(dir), Some(copy)) = (dir.clone(), workspace.copy_of(id)) {
        let Some(source) = workspace.get(id) else {
            return;
        };
        let named = crate::strings::display_name(&source.name);
        let wanted = names::portable(&crate::strings::tagged(
            &source.name,
            &format!("{named} copy"),
        ));
        let name = browser.folders.free(&dir, &wanted, workspace);
        let (origin, len) = (source.origin.clone(), source.size());
        workspace.arrive(dir.join(&name), origin, copy, len);
        return;
    }
    let Some(copy) = workspace.duplicate(id, log) else {
        return;
    };
    if let (Some(dir), Some(entity)) = (dir, workspace.get(copy)) {
        let wanted = crate::workspace::library_filename(entity);
        let name = browser.folders.free(&dir, &wanted, workspace);
        workspace.place(copy, dir.join(&name));
    }
}

/// Keep an edit as a new asset beside the file it was an edit of, named after it, and
/// take that file as it is on disk.
fn keep_both(browser: &mut Browser, workspace: &mut Workspace, log: &mut Log, id: u64) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    let dir = entity
        .path
        .as_ref()
        .map_or_else(LibPath::root, LibPath::parent);
    if unapplied(workspace, log, id) {
        return;
    }
    let name = browser.folders.free(&dir, &entity.name, workspace);
    // One resting in its file is copied by the library, with the edit held of it.
    if let Some(copy) = workspace.copy_of(id) {
        let (origin, len) = (entity.origin.clone(), entity.size());
        workspace.arrive(dir.join(&name), origin, copy, len);
        return workspace.revert(id, log);
    }
    let mine = match entity.whole() {
        Ok(mine) => mine.into_owned(),
        Err(e) => {
            log.error(format!("{}: {e}", entity.name));
            return log.trouble(format!("“{}” could not be read.", entity.name));
        }
    };
    let origin = entity.origin.clone();
    let copy = workspace.ingest(name.clone(), origin, mine, log);
    workspace.place(copy, dir.join(&name));
    workspace.revert(id, log);
}

fn new_folder(browser: &mut Browser, workspace: &Workspace, parent: Option<u64>) {
    let Some(dir) = browser.folders.dir(parent) else {
        return;
    };
    let id = browser.folders.make(&dir, workspace);
    if let Some(parent) = parent {
        browser.open.insert(super::tree::Branch::Folder(parent));
    }
    // ⚠️ Edit the unique name chosen by `make`, not its generic seed.
    let name = browser.folders.name_of(id).unwrap_or_default().to_string();
    browser.start_rename(Item::Folder(id), &name);
}

fn rename_folder(
    browser: &mut Browser,
    workspace: &mut Workspace,
    log: &mut Log,
    id: u64,
    name: String,
) {
    let Some(from) = browser.folders.path_of(id).cloned() else {
        return;
    };
    if let Some(why) = names::refusal(&name) {
        return log.trouble(format!("“{name}” cannot be a folder's name: {why}."));
    }
    let dir = from.parent();
    match browser
        .folders
        .clash(&dir, &name, workspace, Some(Occupant::Folder(id)))
    {
        Clash::Free => browser.folders.relocate(id, dir.join(&name), workspace),
        Clash::Ambiguous(held) => log.trouble(ambiguous(&held, &dir)),
        Clash::Taken(_) => log.trouble(format!(
            "“{name}” is already in {}, so the folder kept its name.",
            spoken(&dir)
        )),
    }
}

/// Remove a folder, moving what was in it up a level. Nothing is deleted, and nothing
/// moves unless everything can.
fn remove_folder(browser: &mut Browser, workspace: &mut Workspace, log: &mut Log, id: u64) {
    let Some(path) = browser.folders.path_of(id).cloned() else {
        return;
    };
    if browser.folders.holds_strangers(&path) {
        return log.trouble(format!(
            "“{}” was not removed: it holds files drawbar does not hold.",
            path.leaf()
        ));
    }
    let up = path.parent();
    let assets: Vec<(u64, String)> = browser
        .folders
        .members(Some(id), workspace)
        .iter()
        .filter_map(|entity| Some((entity.id, entity.path.as_ref()?.leaf().to_string())))
        .collect();
    let folders: Vec<(u64, String)> = browser
        .folders
        .children(Some(id))
        .into_iter()
        .filter_map(|child| Some((child, browser.folders.name_of(child)?.to_string())))
        .collect();
    let blocked = assets
        .iter()
        .chain(&folders)
        .map(|(_, name)| name)
        .find(|name| browser.folders.clash(&up, name, workspace, None) != Clash::Free);
    if let Some(name) = blocked {
        return log.trouble(format!(
            "The folder was not removed: “{name}” is already in {}.",
            spoken(&up)
        ));
    }
    for (asset, name) in assets {
        workspace.place(asset, up.join(&name));
    }
    for (child, name) in folders {
        browser.folders.relocate(child, up.join(&name), workspace);
    }
    // ⚠️ A removed row cannot close its rename state; a reused id would inherit it.
    browser.forget_rename(Item::Folder(id));
    browser.folders.remove(id);
}

/// Put a tag on every one of these assets.
///
/// ⚠️ Membership is by workspace id, and a view's id does not survive the session: the
/// store skips views and nothing lists them, so the tag would be lost with the tab. A
/// view is kept first, as [`Act::Keep`] does, and the log says so.
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
            1 => "Kept a copy on this computer to preserve its tag.".to_string(),
            n => format!("Kept copies of {n} views on this computer to preserve their tags."),
        });
    }
    for id in ids {
        browser.tags.set(*id, tag, true);
    }
}

/// Drain the queue, one command per folder.
///
/// Every queued write goes through here, with the same refusal, grouping and per-item
/// flow. An entry leaves the queue when its [`crate::device::DeviceEvent::Sent`] arrives,
/// so a batch that stops halfway leaves the rest of the queue in place.
fn send_batch(queue: &mut Queue, workspace: &Workspace, device: &mut Device, log: &mut Log) {
    // ⚠️ The instrument attached now may not be the one each entry was queued against:
    // the queue survives a disconnection, so every entry is checked again.
    crate::queue::refit(workspace, &device.state, queue, log);
    // The whole batch is checked before the first delete-then-write.
    let batch = match grouped(queue, workspace) {
        Ok(batch) => batch,
        Err((name, e)) => {
            log.error(format!("{name}: {e}"));
            return log.trouble(format!(
                "Nothing was sent: “{name}” cannot be sent. The details are below."
            ));
        }
    };
    for (class, items) in batch {
        device.send(DeviceCmd::SendAll { class, items }, log);
    }
}

/// One folder's writes, in queue order.
type Batch = (ObjectClass, Vec<Outgoing>);

/// What will be written, grouped by folder in queue order. A session belongs to a
/// folder, so a batch is split by folder. An asset the instrument must not be sent stops
/// the batch, named with why. An asset resting in its file is sent from it, unread here.
fn grouped(queue: &Queue, workspace: &Workspace) -> Result<Vec<Batch>, (String, String)> {
    let mut by_class: Vec<Batch> = Vec::new();
    for held in will_write(queue) {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        let item = Outgoing {
            id: entity.id,
            at: held.at,
            name: entity.name.clone(),
            payload: Payload::of(entity).map_err(|e| (entity.name.clone(), e))?,
        };
        match by_class.iter_mut().find(|(class, _)| *class == held.class) {
            Some((_, items)) => items.push(item),
            None => by_class.push((held.class, vec![item])),
        }
    }
    Ok(by_class)
}

/// What a batch would write: every waiting entry the attached instrument has not
/// refused. A refused entry stays in the queue, so nothing that counts or names the write
/// may include it.
pub(super) fn will_write(queue: &Queue) -> impl Iterator<Item = &Queued> {
    queue.entries().iter().filter(|held| held.failure.is_none())
}

/// What a batch should be warned about, each warning once, in queue order.
pub fn send_warnings(queue: &Queue, workspace: &Workspace, state: &DeviceState) -> Vec<String> {
    let mut warnings: Vec<String> = Vec::new();
    for held in will_write(queue) {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        for warning in write_warnings(state, held.class, entity) {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
    }
    warnings
}

/// Warn when an outgoing format tag differs from every format tag read in the folder.
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

/// The warnings for a write into `class`: what the attached instrument makes of the
/// asset's format, and what a write to the class disturbs beyond the slot.
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

/// The same warnings as one note, for the dialog about a single slot.
fn write_note(state: &DeviceState, class: ObjectClass, entity: &LocalEntity) -> Option<String> {
    let note: Vec<String> = write_warnings(state, class, entity).collect();
    (!note.is_empty()).then(|| note.join("\n\n"))
}

/// The slot an asset would be written back to: its [`LocalEntity::spot`], when this app
/// writes to that class.
pub(super) fn owed(entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    let (class, at) = entity.spot()?;
    (!read_only(class)).then_some((class, at))
}

/// Where queueing an asset for sending would put it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Bound {
    At(ObjectClass, Location),
    /// Its folder is on the instrument, but no slot there has been read and found empty.
    Full(ObjectClass),
    /// The attached instrument does not take this format, and this is why.
    Refused(String),
    /// Nothing the instrument declares takes this kind, or this app will not write into
    /// the folder that would.
    Nowhere,
}

/// The first slot of a folder that a scan found empty and nothing in the queue is waiting
/// for: where a duplicate of a slot lands, and where an unlinked asset is queued.
///
/// ⚠️ Two writes to one address would leave only one.
pub(super) fn spare_slot(
    state: &DeviceState,
    class: ObjectClass,
    queue: &Queue,
) -> Option<Location> {
    state.first_free(class, &queue.waiting_in(class))
}

/// The slot an asset belongs to ([`owed`]), or else the first free slot of its folder
/// that nothing in the queue is waiting for.
///
/// ⚠️ Free means read and found empty. A folder no scan has reached offers no slot, for
/// the reason [`crate::queue::Occupancy`] gives.
pub(super) fn bound_for(entity: &LocalEntity, state: &DeviceState, queue: &Queue) -> Bound {
    if let Fit::Refuses(why) = fit(state, entity) {
        return Bound::Refused(why);
    }
    if let Some((class, at)) = owed(entity) {
        return Bound::At(class, at);
    }
    let home = Kind::of(entity).home();
    let Some(class) = home.filter(|class| !read_only(*class) && state.classes().contains(class))
    else {
        return Bound::Nowhere;
    };
    match spare_slot(state, class, queue) {
        Some(at) => Bound::At(class, at),
        None => Bound::Full(class),
    }
}

/// Queue a set of assets, each for the slot [`bound_for`] gives, and say what was not
/// queued.
///
/// Each entry joins the queue before the next is placed, so unlinked assets fill the
/// free slots in address order instead of all landing on the first.
///
/// A folder with no free slot is named in the log; dropping the entry silently would
/// look like a bug.
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
            1 => "1 of them belongs in no folder the instrument has, so it was not queued."
                .to_string(),
            n => format!(
                "{n} of them belong in no folder the instrument has, so they were not queued."
            ),
        });
    }
}

/// What ⌘S does for each kind of document.
///
/// A view is the instrument's own copy, opened in place, so saving it writes it back at
/// once. Its baseline moves when the instrument confirms the write. Anything on this
/// computer is already kept, so saving it moves its baseline and, if it belongs to a
/// slot, queues it for that slot.
///
/// `ask` is false once the user has confirmed the write's warnings.
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
            // Already waiting for that slot, so the save only moved the baseline.
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
            "“{}” did not come from a slot this app writes to, so there is nowhere to \
             save it.",
            entity.name
        ));
    };
    // Refused before the write.
    let payload = match Payload::of(entity) {
        Ok(payload) => payload,
        Err(e) => {
            log.error(format!("{}: {e}", entity.name));
            return log.trouble(format!(
                "“{}” is not a file the instrument takes.",
                entity.name
            ));
        }
    };
    if let Some(note) = write_note(&device.state, class, entity).filter(|_| ask) {
        return browser.ask_write(&entity.name, place(class, at), note, Act::WriteBack(id));
    }
    device.send(
        DeviceCmd::Put {
            id,
            class,
            at,
            name: entity.name.clone(),
            payload,
        },
        log,
    );
}

/// Queue a local asset for a slot.
///
/// `ask` is false once the user has answered, so the answer does not ask again.
///
/// ⚠️ It asks only when the write both carries a warning (a foreign format, or a settings
/// write that reloads the panel) and replaces an occupant. Every other warning waits for
/// [`Act::AskSendAll`], the confirmation before anything is written.
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
    // Refused before anything is queued.
    if let Err(e) = entity.sendable() {
        log.error(format!("{}: {e}", entity.name));
        log.trouble(format!(
            "“{}” is not a file the instrument takes.",
            entity.name
        ));
        return;
    }
    let note = write_note(&device.state, class, entity);
    // The same evidence the queue entry will carry: an unread bank has no occupant to
    // name.
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
    use crate::strings::folder;
    use crate::testing::Bench;
    use crate::workspace::Origin;

    fn at(slot: u32) -> Location {
        Location { bank: 6, slot }
    }

    /// A checked set: two assets on this computer and two slots on the instrument.
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

    /// ⌘S on a view writes it back to its slot at once. Nothing joins the queue, which
    /// holds only what this computer has to send.
    #[test]
    fn saving_a_view_writes_it_back_to_its_slot_and_queues_nothing() {
        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        bench.device.pretend_attached();
        let id = bench.workspace.view(
            "Africa Split.ne5p".to_string(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(3),
            },
            bytes,
            &mut bench.log,
        );
        let held = bench.workspace.get(id).unwrap().bytes.to_vec();
        let (_, edited) =
            crate::fields::apply(&held, &[("center_panel.gain".into(), "96".into())]).unwrap();
        bench.workspace.replace_bytes(id, edited, &mut bench.log);

        bench.act(vec![Act::SaveDoc(id)]);

        assert!(
            bench.queue.is_empty(),
            "a write back does not join the queue"
        );
        let put = bench.device.queued().front().expect("a put was asked for");
        assert!(
            matches!(put, DeviceCmd::Put { id: sent, class, at: to, .. }
                if *sent == id && *class == ObjectClass::Program && *to == at(3)),
            "{}",
            put.label()
        );
        // The instrument has not answered yet, so the baseline has not moved.
        assert!(bench.workspace.get(id).unwrap().is_unsaved());
        bench.device.pretend(crate::device::DeviceEvent::Sent {
            id,
            class: ObjectClass::Program,
            at: at(3),
            sent: crate::device::Payload::Bytes(bench.workspace.get(id).unwrap().bytes.to_vec()),
        });
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
        assert!(!bench.workspace.get(id).unwrap().is_unsaved());
    }

    /// A piano resting in its file is queued and batched without being read: the batch
    /// carries the file, and once it lands the asset stands on the slot with the
    /// checksum of the file, saved as the file, which nothing holds whole.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_resting_piano_is_sent_from_its_file_and_stands_on_the_slot_it_landed_in() {
        use std::sync::Arc;

        let mut bench = Bench::new();
        let dir = crate::testing::Temp::new();
        let file = crate::testing::on_disk(&dir, "Upright.npno", &crate::testing::piano(40));
        let id = crate::testing::rest(&mut bench.workspace, "Upright.npno", file.clone());
        bench.workspace.settle_files(&mut bench.log);
        let crc32 = bench.workspace.get(id).unwrap().saved.crc32();
        assert!(crc32.is_some(), "its check answered");
        file.take_reads();

        let class = ObjectClass::Piano;
        let at = Location::from_user(1, 1);
        bench.device.pretend_scanned(class, 1, &[""]);
        bench.act(vec![Act::Send { id, class, at }, Act::SendAll]);

        let Some(DeviceCmd::SendAll { items, .. }) = bench.device.queued().back() else {
            panic!("a batch was asked for")
        };
        let [item] = items.as_slice() else {
            panic!("{} items", items.len())
        };
        let Payload::File {
            file: sent,
            crc32: said,
        } = &item.payload
        else {
            panic!("the batch carries bytes in place of the file")
        };
        assert!(Arc::ptr_eq(sent, &file), "the file it rests in");
        assert_eq!(Some(*said), crc32);
        assert_eq!(file.take_reads(), [], "nothing read it");
        let sent = item.payload.clone();

        // Dispatching the write drops what the walk said of the bank it changes.
        bench.device.pump();
        bench.device.pretend(crate::device::DeviceEvent::Sent {
            id,
            class,
            at,
            sent,
        });
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );

        assert!(bench.queue.is_empty(), "it is no longer owed");
        let entity = bench.workspace.get(id).unwrap();
        assert_eq!(entity.link, Some((class, at)));
        assert!(entity.wrote.is_some_and(|wrote| Some(wrote.crc32) == crc32));
        assert!(entity.rests().is_some_and(|held| Arc::ptr_eq(held, &file)));
        assert!(!entity.is_unsaved());
        assert_eq!(entity.held_whole(), 0, "nothing holds it whole");
        assert_eq!(file.take_reads(), [], "and nothing read it");
    }

    #[test]
    fn saving_a_kept_asset_settles_its_baseline_and_queues_it_where_it_stands() {
        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        bench.device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[None, None, None, Some(("held", 7))],
        );
        let linked = bench.workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(3),
            },
            bytes.clone(),
            &mut bench.log,
        );
        let alone = bench.workspace.ingest(
            "Squabble B.ne5p".to_string(),
            Origin::File("Squabble B.ne5p".into()),
            bytes,
            &mut bench.log,
        );
        for id in [linked, alone] {
            let held = bench.workspace.get(id).unwrap().bytes.to_vec();
            let (_, edited) =
                crate::fields::apply(&held, &[("center_panel.gain".into(), "96".into())]).unwrap();
            bench.workspace.replace_bytes(id, edited, &mut bench.log);
        }

        bench.act(vec![Act::SaveDoc(linked), Act::SaveDoc(alone)]);

        assert!(!bench.workspace.get(linked).unwrap().is_unsaved());
        assert!(!bench.workspace.get(alone).unwrap().is_unsaved());
        assert_eq!(
            bench.queue.ids(),
            vec![linked],
            "only the one that belongs to a slot"
        );
        assert_eq!(bench.queue.entry(linked).map(|held| held.at), Some(at(3)));

        // Saving it again reads nothing from the instrument, because it is already
        // waiting for that slot, and the log still says what the save did.
        let reads = bench.device.queued().len();
        bench.log.clear();
        bench.act(vec![Act::SaveDoc(linked)]);
        assert_eq!(bench.queue.ids(), vec![linked]);
        assert_eq!(
            bench.device.queued().len(),
            reads,
            "the slot was not read again"
        );
        assert!(
            bench
                .log
                .transcript()
                .contains("“Africa Split.ne5p” is saved on this computer"),
            "{}",
            bench.log.transcript()
        );
    }

    /// Queueing and exporting act on this computer's rows, copying on the instrument's,
    /// and deleting on all of them.
    #[test]
    fn each_action_over_a_checked_set_asks_only_about_the_rows_it_is_for() {
        let Bench { mut device, .. } = Bench::new();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);
        device.pretend_scanned(ObjectClass::SetList, 7, &["", "", "", "Sunday"]);
        let state = &device.state;
        let checked = checked();

        let queued = bulk(Bulk::Queue, &checked, state);
        assert!(
            matches!(queued.as_slice(), [Act::SendChecked(ids)] if *ids == vec![1, 2]),
            "one queue act, for this computer's rows"
        );
        assert!(
            matches!(
                bulk(Bulk::Copy, &checked, state).as_slice(),
                [
                    Act::CopyAll { class: ObjectClass::Program, slots: programs },
                    Act::CopyAll { class: ObjectClass::SetList, slots: set_lists },
                ] if programs.len() == 1 && set_lists.len() == 1
            ),
            "one copy per class, of its slots"
        );
        assert!(
            matches!(
                bulk(Bulk::Export, &checked, state).as_slice(),
                [Act::Export(1), Act::Export(2)]
            ),
            "one export per asset on this computer"
        );
        assert!(
            bulk(Bulk::Tag, &checked, state).is_empty(),
            "a tag is picked first"
        );
        assert!(matches!(
            bulk(Bulk::Delete, &checked, state).as_slice(),
            [
                Act::Remove(1),
                Act::Remove(2),
                Act::DeleteSlot { .. },
                Act::DeleteSlot { .. }
            ]
        ));
    }

    /// An empty answer disables the control.
    ///
    /// ⚠️ A slot the scan found empty has nothing to act on: asking the instrument for it
    /// costs a round trip that can only fail.
    #[test]
    fn an_action_with_nothing_to_act_on_asks_for_nothing() {
        let Bench { mut device, .. } = Bench::new();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", ""]);
        let state = &device.state;
        let slots = vec![Item::Slot {
            class: ObjectClass::Program,
            at: at(0),
        }];
        let vacant = vec![Item::Slot {
            class: ObjectClass::Program,
            at: at(1),
        }];
        let locals = vec![Item::Local(1)];
        assert!(bulk(Bulk::Queue, &slots, state).is_empty());
        assert!(bulk(Bulk::Export, &slots, state).is_empty());
        assert!(bulk(Bulk::Copy, &locals, state).is_empty());
        assert!(bulk(Bulk::Delete, &[], state).is_empty());
        assert!(
            bulk(Bulk::Copy, &vacant, state).is_empty(),
            "7:2 was read and found empty"
        );
        assert!(
            bulk(Bulk::Delete, &vacant, state).is_empty(),
            "and deleting what is not there deletes nothing"
        );
    }

    /// Each asset goes to the slot it came from, or to the first free slot of its folder
    /// if it came from none. Bytes that belong in no folder the instrument has are not
    /// queued.
    #[test]
    fn queueing_a_checked_set_puts_each_of_them_where_it_is_bound() {
        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        // 7:1 is taken by the asset that came off it; 7:2 is the first free slot.
        bench
            .device
            .pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", ""]);
        let owed = bench.workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(0),
            },
            bytes.clone(),
            &mut bench.log,
        );
        let opened = bench.workspace.ingest(
            "Squabble B.ne5p".to_string(),
            Origin::File("Squabble B.ne5p".into()),
            bytes,
            &mut bench.log,
        );
        let nowhere = bench.workspace.ingest(
            "mystery.dat".to_string(),
            Origin::Fresh,
            vec![0x00, 0xff, 0x01, 0xfe],
            &mut bench.log,
        );

        bench.act(bulk(
            Bulk::Queue,
            &[Item::Local(owed), Item::Local(opened), Item::Local(nowhere)],
            &bench.device.state,
        ));

        assert_eq!(bench.queue.ids(), vec![owed, opened]);
        assert_eq!(bench.queue.entry(owed).map(|held| held.at), Some(at(0)));
        assert_eq!(
            bench.queue.entry(opened).map(|held| held.at),
            Some(at(1)),
            "the first slot read and found free"
        );
        assert!(
            bench
                .log
                .transcript()
                .contains("1 of them belongs in no folder the instrument has"),
            "{}",
            bench.log.transcript()
        );
    }

    /// Assets that came from no slot take distinct free slots in address order, skipping
    /// slots the queue already holds. The asset past the last free slot is refused, and
    /// the log names its folder.
    #[test]
    fn a_queued_set_walks_down_the_free_slots_and_names_the_folder_that_runs_out() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        // Four empty slots, one of them already taken by the queue.
        bench
            .device
            .pretend_scanned(class, 7, &["", "Africa Split", "", "", "Squabble B", ""]);

        let bytes = Fresh::Program.bytes().unwrap();
        let ids: Vec<u64> = (0..5)
            .map(|n| {
                bench.workspace.ingest(
                    format!("sound {n}.ne5p"),
                    Origin::File(format!("sound {n}.ne5p")),
                    bytes.clone(),
                    &mut bench.log,
                )
            })
            .collect();
        enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            ids[0],
            class,
            at(2),
        );

        bench.act(bulk(
            Bulk::Queue,
            &ids[1..]
                .iter()
                .copied()
                .map(Item::Local)
                .collect::<Vec<_>>(),
            &bench.device.state,
        ));

        let landed: Vec<(u64, u32)> = bench
            .queue
            .entries()
            .iter()
            .map(|held| (held.id, held.at.slot))
            .collect();
        assert_eq!(
            landed,
            vec![(ids[0], 2), (ids[1], 0), (ids[2], 3), (ids[3], 5)],
            "each takes the next free slot, and the queue already held 7:3"
        );
        assert!(
            !bench.queue.holds(ids[4]),
            "the free slots ran out before it"
        );
        let said = bench.log.transcript();
        assert!(
            said.contains("“sound 4.ne5p” was not queued: no slot of Programs"),
            "{said}"
        );
    }

    /// The log says how much of the set fit.
    #[test]
    fn queueing_a_mixed_set_queues_only_what_the_instrument_takes() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench.device.pretend_scanned(class, 7, &["", "", ""]);

        let bytes = Fresh::Program.bytes().unwrap();
        let mine = bench.workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::File("Africa Split.ne5p".into()),
            bytes,
            &mut bench.log,
        );
        let stage = bench
            .workspace
            .create(Fresh::Stage4Program, &mut bench.log)
            .unwrap();

        bench.act(bulk(
            Bulk::Queue,
            &[Item::Local(mine), Item::Local(stage)],
            &bench.device.state,
        ));

        assert_eq!(
            bench.queue.ids(),
            vec![mine],
            "only the Electro 5's own is waiting"
        );
        let said = bench.log.transcript();
        assert!(said.contains("1 of 2 fit the Nord Electro 5"), "{said}");
        assert!(said.contains("Stage 4"), "{said}");

        // The control says the same before it is clicked: the count in its label, and the
        // instrument's refusal for the one it leaves out.
        let checked = [Item::Local(mine), Item::Local(stage)];
        let held = fits(&checked, &bench.workspace, &bench.device.state);
        assert_eq!(held.label(), "Queue 1 of 2");
        assert!(held
            .why
            .as_deref()
            .is_some_and(|why| why.contains("Stage 4")));

        // Nothing it takes: the control is disabled, and the hover gives the instrument's
        // reason.
        let refused = fits(&[Item::Local(stage)], &bench.workspace, &bench.device.state);
        assert_eq!(refused.takes, 0);
        assert_eq!(refused.label(), "Queue 0 of 1");
        assert!(refused.why.is_some());

        // Everything it takes: the usual label, and no reason.
        let taken = fits(&[Item::Local(mine)], &bench.workspace, &bench.device.state);
        assert_eq!(taken.label(), Bulk::Queue.label());
        assert_eq!(taken.why, None);
    }

    /// ⚠️ A refused asset never gets a queue entry, however it was aimed: a send writes
    /// the queue, so a foreign file in it would reach a delete-then-write.
    #[test]
    fn a_slot_named_outright_still_refuses_what_the_instrument_does_not_take() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_scanned(class, 7, &[""]);
        let stage = bench
            .workspace
            .create(Fresh::Stage4Program, &mut bench.log)
            .unwrap();

        bench.act(vec![Act::Send {
            id: stage,
            class,
            at: at(0),
        }]);

        assert!(bench.queue.is_empty(), "nothing is waiting");
        let said = bench.log.transcript();
        assert!(said.contains("cannot go to"), "{said}");
    }

    /// A drop means the same Send from any row, and one asset has one queue entry
    /// wherever it was dragged from.
    #[test]
    fn dropping_something_already_waiting_onto_a_slot_moves_its_entry() {
        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        let class = ObjectClass::Program;
        let id = bench.workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::Device { class, at: at(0) },
            bytes,
            &mut bench.log,
        );
        bench.act(vec![Act::SendChecked(vec![id])]);
        assert_eq!(bench.queue.entry(id).map(|held| held.at), Some(at(0)));

        // What a queue row carries, dropped on a keyboard cell.
        let carried = crate::browser::Held {
            what: Item::Local(id),
            kind: crate::browser::Kind::Program,
            filed: None,
            fits: true,
        };
        let onto = crate::browser::Onto::Slot { class, at: at(3) };
        assert_eq!(
            crate::browser::landing(&carried, onto),
            Ok(Act::Send {
                id,
                class,
                at: at(3)
            })
        );

        bench.act(vec![Act::Send {
            id,
            class,
            at: at(3),
        }]);
        assert_eq!(bench.queue.ids(), vec![id], "one entry, moved");
        assert_eq!(bench.queue.entry(id).map(|held| held.at), Some(at(3)));
    }

    /// Nothing refuses a note by name. It has no object class, so `bound_for` returns
    /// `Nowhere`, as for any other kind with no folder.
    #[test]
    fn a_note_is_never_queued_because_it_belongs_in_no_folder() {
        let mut bench = Bench::new();
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench
            .device
            .pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", ""]);
        let note = bench.workspace.ingest(
            "Set 1.txt".to_string(),
            Origin::Fresh,
            b"Set 1\n".to_vec(),
            &mut bench.log,
        );
        let program = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();

        bench.act(vec![Act::SendChecked(vec![note, program])]);

        assert_eq!(
            bench.queue.ids(),
            vec![program],
            "the program takes a free slot and the note is not queued"
        );
        assert_eq!(
            bound_for(
                bench.workspace.get(note).unwrap(),
                &bench.device.state,
                &bench.queue
            ),
            Bound::Nowhere
        );
    }

    /// Neither removing one entry nor clearing the queue deletes anything from this
    /// computer, and the status line stops saying that something is waiting.
    #[test]
    fn an_entry_leaves_the_queue_alone_and_the_queue_empties_without_deleting_anything() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench.device.pretend_scanned(class, 7, &["", "", ""]);
        let bytes = Fresh::Program.bytes().unwrap();
        let ids: Vec<u64> = (0..3)
            .map(|n| {
                bench.workspace.ingest(
                    format!("sound {n}.ne5p"),
                    Origin::File(format!("sound {n}.ne5p")),
                    bytes.clone(),
                    &mut bench.log,
                )
            })
            .collect();

        let queueing = bulk(
            Bulk::Queue,
            &ids.iter().copied().map(Item::Local).collect::<Vec<_>>(),
            &bench.device.state,
        );
        bench.act(queueing);
        assert_eq!(bench.queue.ids(), ids);

        bench.act(vec![Act::Unqueue(ids[1])]);
        assert_eq!(bench.queue.ids(), vec![ids[0], ids[2]]);
        assert_eq!(
            bench.log.status().1,
            "“sound 1.ne5p” is no longer waiting to be sent."
        );

        bench.act(vec![Act::ClearQueue]);
        assert!(bench.queue.is_empty());
        assert_eq!(
            bench.log.status().1,
            "The send queue is empty. Nothing was sent."
        );
        assert_eq!(
            bench.workspace.listed().count(),
            3,
            "emptying the queue deletes nothing"
        );
    }

    /// Queueing says where the asset is going on the status line and leaves the window as
    /// it was: the review opens only when asked for.
    #[test]
    fn queueing_says_where_it_goes_without_opening_the_review() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench.device.pretend_scanned(class, 7, &[""]);
        let bytes = Fresh::Program.bytes().unwrap();
        let id = bench.workspace.ingest(
            "Africa Split.ne5p".to_string(),
            Origin::File("Africa Split.ne5p".into()),
            bytes,
            &mut bench.log,
        );

        bench.act(vec![Act::Send {
            id,
            class,
            at: at(0),
        }]);

        assert!(bench.queue.holds(id));
        assert!(!bench.shell.review_open);
        let (_, said) = bench.log.status();
        assert!(said.contains("waiting to be sent to Programs"), "{said}");

        bench.act(vec![Act::AskSendAll]);
        assert!(bench.shell.review_open, "asking to send all is the review");
    }

    /// A session belongs to a folder, so a batch is one command per folder, in queue
    /// order.
    #[test]
    fn a_batch_is_grouped_into_one_command_per_folder() {
        let Bench {
            mut workspace,
            mut device,
            mut queue,
            mut log,
            ..
        } = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
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

        let grouped = grouped(&queue, &workspace).expect("every asset reads");
        assert_eq!(grouped.len(), 2, "one command per folder");
        let programs = grouped
            .iter()
            .find(|(class, _)| *class == ObjectClass::Program)
            .expect("programs are queued");
        assert_eq!(programs.1.len(), 2);
        let slots: Vec<u32> = programs.1.iter().map(|item| item.at.slot).collect();
        assert_eq!(slots, vec![0, 1], "in the order the queue holds them");
    }

    /// A send queues; the write happens when the queue is drained, and each item names
    /// the asset it came from, so the right queue entry is cleared.
    #[test]
    fn a_send_queues_and_the_drain_names_the_asset_it_writes() {
        let mut bench = Bench::new();
        bench
            .device
            .pretend_scanned(ObjectClass::Program, 7, &["Africa Split", ""]);
        let at = Location { bank: 6, slot: 1 };
        let bytes = Fresh::Program.bytes().unwrap();
        let id = bench.workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            bytes,
            &mut bench.log,
        );

        bench.act(vec![Act::Send {
            id,
            class: ObjectClass::Program,
            at,
        }]);
        assert_eq!(
            bench.queue.ids(),
            vec![id],
            "queued, and nothing written yet"
        );
        assert!(
            bench.device.queued().is_empty(),
            "an empty slot carrying no warning asks nothing"
        );

        bench.act(vec![Act::SendAll]);
        match bench.device.queued().front().expect("a batch was queued") {
            DeviceCmd::SendAll { class, items } => {
                assert_eq!(*class, ObjectClass::Program);
                assert_eq!(
                    items.iter().map(|item| item.id).collect::<Vec<_>>(),
                    vec![id]
                );
            }
            other => panic!("{}", other.label()),
        }
        assert_eq!(
            bench.queue.ids(),
            vec![id],
            "still queued until the write lands"
        );
    }

    /// The confirmation says what is known about each destination slot, and an unread
    /// bank is not reported as empty. With no warnings, nothing comes before the
    /// destinations.
    #[test]
    fn the_review_says_what_is_known_about_each_slot() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        // Bank 7 was scanned: 7:1 holds something and 7:2 is empty. Bank 8 was not read.
        bench
            .device
            .pretend_scanned(class, 7, &["Africa Split", ""]);
        let bytes = Fresh::Program.bytes().unwrap();
        for (bank, slot) in [(6, 0), (6, 1), (7, 0)] {
            let id = bench.workspace.ingest(
                format!("sound {bank}-{slot}"),
                Origin::Fresh,
                bytes.clone(),
                &mut bench.log,
            );
            enqueue(
                &bench.workspace,
                &mut bench.device,
                &mut bench.queue,
                &mut bench.log,
                id,
                class,
                Location { bank, slot },
            );
        }

        bench.act(vec![Act::AskSendAll]);
        assert!(bench.shell.review_open);

        let mut said = Vec::new();
        for id in bench.queue.ids() {
            bench.queue.picked = Some(id);
            said.extend(reviewed(&mut bench));
        }
        for slot in [
            "→ Programs 7:1 holds “Africa Split”",
            "→ Programs 7:2 is empty",
            "→ Programs 8:1 has not been read yet",
        ] {
            assert!(
                said.iter().any(|word| word.starts_with(slot)),
                "{slot}: {said:?}"
            );
        }
    }

    /// ⚠️ The review counts what the batch would write. An entry the attached instrument
    /// has refused stays in the queue and is not written, so counting it would promise a
    /// write that will not happen.
    #[test]
    fn the_review_counts_only_what_the_batch_would_write() {
        use crate::device::DeviceEvent;

        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench
            .device
            .pretend_scanned(class, 7, &["Africa Split", "Squabble B"]);
        let theirs = Fresh::Program.bytes().unwrap();
        let electro = bench.workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Fresh,
            theirs,
            &mut bench.log,
        );
        enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            electro,
            class,
            at(0),
        );

        // Another instrument in its place, which refuses the one already waiting.
        bench
            .device
            .pretend(DeviceEvent::Disconnected { lost: true });
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
        bench.device.pretend_attached_as("Nord Stage 4");
        let made = bench
            .workspace
            .create(Fresh::Stage4Program, &mut bench.log)
            .expect("a Stage 4 program");
        enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            made,
            class,
            at(1),
        );
        bench
            .device
            .pretend(DeviceEvent::Partitions(vec![crate::device::Partition {
                class,
                name: "Program".into(),
                native: false,
                unit: None,
            }]));
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
        assert_eq!(
            bench.queue.ids(),
            vec![electro, made],
            "both are still waiting"
        );

        bench.act(vec![Act::AskSendAll]);

        let said = reviewed(&mut bench);
        for word in ["1 write", "1 cannot go", "Send all 1"] {
            assert!(said.iter().any(|it| it == word), "{word}: {said:?}");
        }
    }

    /// Every string the review paints, after a frame to settle its layout.
    fn reviewed(bench: &mut Bench) -> Vec<String> {
        let mut said = Vec::new();
        for _ in 0..2 {
            let input = crate::testing::screen(eframe::egui::vec2(1280.0, 800.0), Vec::new());
            let output = crate::testing::run(&bench.ctx, input, |ctx| {
                crate::queue::review(
                    ctx,
                    &mut bench.queue,
                    &bench.workspace,
                    &bench.device.state,
                    &mut Vec::new(),
                );
            });
            said = crate::testing::words(&output);
        }
        said
    }

    /// The queue outlives the instrument it was built for. An entry the attached
    /// instrument refuses is not written, stays in the queue with the reason, and does
    /// not stop the rest of the batch.
    #[test]
    fn a_send_re_checks_every_entry_against_the_instrument_attached_now() {
        use crate::device::DeviceEvent;

        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench
            .device
            .pretend_scanned(class, 7, &["Africa Split", "Squabble B"]);
        let theirs = Fresh::Program.bytes().unwrap();
        let electro = bench.workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Fresh,
            theirs,
            &mut bench.log,
        );
        enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            electro,
            class,
            at(0),
        );

        // Another instrument in its place, and something it does take waiting with it.
        bench
            .device
            .pretend(DeviceEvent::Disconnected { lost: true });
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
        bench.device.pretend_attached_as("Nord Stage 4");
        let made = bench
            .workspace
            .create(Fresh::Stage4Program, &mut bench.log)
            .expect("a Stage 4 program");
        enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            made,
            class,
            at(1),
        );
        assert_eq!(bench.queue.ids(), vec![electro, made]);

        // The instrument reports its partitions, and the queue marks the refusal before
        // Send is pressed.
        bench
            .device
            .pretend(DeviceEvent::Partitions(vec![crate::device::Partition {
                class,
                name: "Program".into(),
                native: false,
                unit: None,
            }]));
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
        assert!(bench
            .queue
            .entry(electro)
            .is_some_and(|held| held.failure.is_some()));

        bench.act(vec![Act::SendAll]);

        let batch = bench
            .device
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
        assert_eq!(
            bench.queue.ids(),
            vec![electro, made],
            "both are still waiting"
        );
        let refused = bench.queue.entry(electro).expect("it kept its place");
        assert!(
            refused
                .failure
                .as_deref()
                .is_some_and(|why| why.contains("Nord Stage 4")),
            "{:?}",
            refused.failure
        );
        assert!(
            bench
                .log
                .transcript()
                .contains("Africa-Split.ne5p” cannot go to"),
            "{}",
            bench.log.transcript()
        );
    }

    /// What a stopped batch did not write stays in the queue, with the reason on the
    /// entry it stopped at.
    #[test]
    fn a_batch_that_stops_leaves_the_rest_of_the_queue_waiting() {
        use crate::device::DeviceEvent;

        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench
            .device
            .pretend_scanned(class, 7, &["Africa Split", "Squabble B", "Bass Manual"]);
        let bytes = Fresh::Program.bytes().unwrap();
        let ids: Vec<u64> = (0..3)
            .map(|slot| {
                let id = bench.workspace.ingest(
                    format!("sound {slot}"),
                    Origin::Device {
                        class,
                        at: at(slot),
                    },
                    bytes.clone(),
                    &mut bench.log,
                );
                enqueue(
                    &bench.workspace,
                    &mut bench.device,
                    &mut bench.queue,
                    &mut bench.log,
                    id,
                    class,
                    at(slot),
                );
                id
            })
            .collect();

        bench.act(vec![Act::SendAll]);
        let batch = bench
            .device
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
            bench
                .device
                .queued()
                .iter()
                .filter(|cmd| matches!(cmd, DeviceCmd::SendAll { .. }))
                .count(),
            1,
        );

        // ⚠️ One command runs at a time, so the slot reads must finish before the batch
        // runs.
        let waiting = |device: &Device| {
            device
                .queued()
                .iter()
                .any(|cmd| matches!(cmd, DeviceCmd::SendAll { .. }))
        };
        while waiting(&bench.device) {
            bench.device.pump();
            if waiting(&bench.device) {
                bench.device.pretend(DeviceEvent::Finished);
                bench.device.poll(
                    &mut bench.log,
                    &mut bench.workspace,
                    &mut bench.tabs,
                    &mut bench.queue,
                );
            }
        }

        // The instrument takes the first and refuses the second.
        bench.device.pretend(DeviceEvent::Sent {
            id: ids[0],
            class,
            at: at(0),
            sent: crate::device::Payload::Bytes(
                bench.workspace.get(ids[0]).unwrap().bytes.to_vec(),
            ),
        });
        bench.device.pretend(DeviceEvent::OpFailed(
            "Programs 7:2 is occupied, and the instrument does not overwrite in place".into(),
        ));
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );

        assert_eq!(
            bench.queue.ids(),
            ids[1..],
            "what was not written is still queued"
        );
        let stopped = bench.queue.entry(ids[1]).expect("the one it stopped on");
        assert!(
            stopped
                .failure
                .as_deref()
                .is_some_and(|why| why.contains("7:2")),
            "{:?}",
            stopped.failure
        );
        assert!(bench.queue.entry(ids[2]).unwrap().failure.is_none());
        assert!(
            bench.log.transcript().contains("7:2"),
            "the log names the slot"
        );
    }

    /// Queueing a folder's members queues everything that came from a slot. With nothing
    /// attached, a new program has no free slot to go to.
    #[test]
    fn a_folder_queues_only_what_can_go_back_to_a_slot() {
        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        let folder = bench
            .browser
            .folders
            .make(&crate::store::LibPath::root(), &bench.workspace);
        for (class, slot) in [
            (ObjectClass::Program, 0),
            (ObjectClass::SetList, 0),
            // The live buffer and the piano library take a write like any other slot.
            (ObjectClass::Live, 0),
            (ObjectClass::Piano, 0),
        ] {
            let id = bench.workspace.ingest(
                format!("{}.ne5p", place(class, at(slot))),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                bytes.clone(),
                &mut bench.log,
            );
            bench
                .browser
                .folders
                .file(&mut bench.workspace, id, Some(folder));
        }
        // Never on an instrument, and nothing is attached to offer it a free slot.
        let fresh = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        bench
            .browser
            .folders
            .file(&mut bench.workspace, fresh, Some(folder));

        // In the order a selection of them iterates.
        let members: std::collections::BTreeSet<Item> = bench
            .browser
            .folders
            .members(Some(folder), &bench.workspace)
            .iter()
            .map(|entity| Item::Local(entity.id))
            .collect();
        let members: Vec<Item> = members.into_iter().collect();
        bench.act(bulk(Bulk::Queue, &members, &bench.device.state));
        let classes: Vec<ObjectClass> = bench
            .queue
            .entries()
            .iter()
            .map(|held| held.class)
            .collect();
        assert_eq!(
            classes,
            vec![
                ObjectClass::Program,
                ObjectClass::SetList,
                ObjectClass::Live,
                ObjectClass::Piano
            ]
        );
        assert!(!bench.queue.holds(fresh), "it has no slot to go to");
    }

    /// A double-click on a slot opens a view: a tab and a document, with no new row in
    /// the list. Keeping the view adds the row.
    #[test]
    fn opening_a_slot_does_not_put_it_on_this_computer() {
        use crate::device::DeviceEvent;
        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        let at = Location { bank: 6, slot: 3 };
        let origin = Origin::Device {
            class: ObjectClass::Program,
            at,
        };

        bench.device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin,
            bytes,
            why: Purpose::View,
        });
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );

        let id = bench.tabs.active().expect("a view opens in a tab");
        assert!(bench.workspace.is_view(id));
        assert_eq!(
            bench.workspace.listed().count(),
            0,
            "nothing joined the list"
        );
        // It still knows the slot it came from, so it can be sent back.
        assert_eq!(
            bench.workspace.get(id).unwrap().origin.slot(),
            Some((ObjectClass::Program, at))
        );

        bench.act(vec![Act::Keep(id)]);
        assert!(!bench.workspace.is_view(id));
        assert_eq!(bench.workspace.listed().count(), 1);
    }

    /// ⚠️ The editor opens on the folder's actual name. Prefilled with the generic name
    /// `make` starts from, one Enter beside a folder already called that would make two
    /// folders with one name, which `make` avoided.
    #[test]
    fn a_new_folder_opens_its_editor_on_the_name_it_was_given() {
        let mut bench = Bench::new();
        let mut new_folder = || {
            bench.act(vec![Act::NewFolder]);
            let rename = bench.browser.rename.as_ref().expect("the editor is open");
            let Item::Folder(id) = rename.what else {
                panic!("it is open on the folder");
            };
            (id, rename.text.clone())
        };

        let (first, typed) = new_folder();
        assert_eq!(typed, "New folder");
        let (second, typed) = new_folder();
        assert_eq!(typed, "New folder 2", "the name it actually has");
        assert_eq!(bench.browser.folders.name_of(second), Some(typed.as_str()));
        assert_ne!(first, second);
    }

    /// Removing a folder during its rename closes the editor: no row will be drawn to
    /// close it, and the next folder to reuse the id would inherit it.
    #[test]
    fn removing_a_folder_mid_rename_takes_the_editor_with_it() {
        let mut bench = Bench::new();
        bench.act(vec![Act::NewFolder]);
        let Some(Item::Folder(id)) = bench.browser.rename.as_ref().map(|r| r.what) else {
            panic!("a new folder opens its editor");
        };

        bench.act(vec![Act::RemoveFolder(id)]);
        assert!(bench.browser.rename.is_none(), "the editor went with it");
        assert!(bench.browser.selection.sole().is_none());

        // The id `make` hands out again has no editor left open on it.
        bench.act(vec![Act::NewFolder]);
        let Some(Item::Folder(again)) = bench.browser.rename.as_ref().map(|r| r.what) else {
            panic!("the new one opens its own");
        };
        assert_eq!(again, id, "the id was reused");
        assert_eq!(
            bench.browser.rename.as_ref().map(|r| r.text.as_str()),
            Some("New folder")
        );
    }

    /// Removing an asset during its rename closes the editor and drops it from the
    /// selection: no row will be drawn to close either, and the next asset to reuse the
    /// id would inherit both.
    #[test]
    fn removing_an_asset_mid_rename_takes_the_editor_with_it() {
        let mut bench = Bench::new();
        let id = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        bench.browser.start_rename(Item::Local(id), "Africa Split");
        assert!(bench.browser.selection.holds(Item::Local(id)));

        bench.act(vec![Act::Remove(id)]);

        assert!(bench.browser.rename.is_none(), "the editor went with it");
        assert!(
            !bench.browser.selection.holds(Item::Local(id)),
            "and the selection holds no removed row"
        );
    }

    /// ⚠️ One view per slot. A second read of a slot already viewed would make two
    /// working copies of one place, edited separately and both queued back to it in one
    /// batch, where the last written wins.
    #[test]
    fn opening_a_slot_that_is_already_open_activates_its_tab() {
        use crate::device::DeviceEvent;
        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        let class = ObjectClass::Program;
        let at = Location { bank: 6, slot: 3 };
        bench
            .device
            .pretend_scanned(class, 7, &["", "", "", "Africa Split"]);

        bench.device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin: Origin::Device { class, at },
            bytes,
            why: Purpose::View,
        });
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
        let first = bench.tabs.active().expect("a view opened");

        bench.tabs.close(Spot::Document(first));
        bench.act(vec![Act::Open(Item::Slot { class, at })]);
        assert!(bench.device.queued().is_empty(), "nothing was read again");
        assert_eq!(bench.tabs.active(), Some(first), "its own tab came forward");
        assert_eq!(bench.workspace.entities().len(), 1, "and there is one copy");

        let elsewhere = Location { bank: 6, slot: 4 };
        bench.act(vec![Act::Open(Item::Slot {
            class,
            at: elsewhere,
        })]);
        assert_eq!(
            bench.device.queued().len(),
            1,
            "a slot with no open view is read"
        );
    }

    /// ⚠️ Sending a file the instrument does not want costs the slot's occupant, and the
    /// New menu makes another model's program one click away. This warns and does not
    /// refuse, because no instrument has been seen to refuse such a file.
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

        // An unscanned folder raises nothing, and neither does a file whose own format
        // tag could not be read.
        assert_eq!(foreign_format("ns4p", &[]), None);
        assert_eq!(
            foreign_format("?", &held(&["ne5p"])),
            None,
            "no tag to judge"
        );
        assert_eq!(foreign_format("", &held(&["ne5p"])), None);
    }

    #[test]
    fn a_sync_reads_every_folder_again() {
        let mut bench = Bench::new();
        bench
            .device
            .pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        bench.act(vec![Act::Resync]);
        for class in bench.device.state.classes() {
            let progress = bench.device.state.scan.progress(class);
            assert!(
                progress.is_some_and(|progress| progress.running),
                "{}",
                folder(class)
            );
        }
    }

    /// A bench whose new programs have files in the top level of the library.
    fn placed(bench: &mut Bench, count: usize) -> Vec<u64> {
        let ids = (0..count)
            .map(|_| {
                bench
                    .workspace
                    .create(Fresh::Program, &mut bench.log)
                    .unwrap()
            })
            .collect();
        crate::folders::place_new(&mut bench.workspace, &bench.browser.folders);
        ids
    }

    fn name(bench: &Bench, id: u64) -> &str {
        &bench.workspace.get(id).expect("held").name
    }

    #[test]
    fn a_name_windows_keeps_for_a_device_is_refused_at_rename() {
        let mut bench = Bench::new();
        let [id] = placed(&mut bench, 1)[..] else {
            unreachable!()
        };
        bench.act(vec![Act::RenameLocal {
            id,
            name: "CON".into(),
        }]);
        assert_eq!(name(&bench, id), "untitled.ne5p", "the name stays");
        let said = bench.log.status().1;
        assert!(
            said.contains("“CON.ne5p”") && said.contains("device"),
            "{said}"
        );
    }

    #[test]
    fn a_rename_onto_a_taken_name_asks_and_an_overwrite_keeps_the_other_id() {
        let mut bench = Bench::new();
        let [kept, renamed] = placed(&mut bench, 2)[..] else {
            unreachable!()
        };
        let incoming = with_gain(&bench.workspace.get(renamed).unwrap().bytes);
        bench
            .workspace
            .replace_bytes(renamed, incoming.clone(), &mut bench.log);
        bench.workspace.mark_saved(renamed);

        bench.act(vec![Act::RenameLocal {
            id: renamed,
            name: "UNTITLED.ne5p".into(),
        }]);
        let (title, answers) = bench.browser.asking().expect("a question");
        assert_eq!(
            title,
            "“UNTITLED.ne5p” is already in the top level of the library"
        );
        assert_eq!(answers, ["Cancel", "Keep both", "Overwrite"]);
        assert_eq!(
            name(&bench, renamed),
            "untitled 2.ne5p",
            "nothing moved yet"
        );

        let acts = bench.browser.answer("Overwrite");
        bench.act(acts);
        assert_eq!(bench.workspace.get(kept).unwrap().bytes, incoming);
        assert_eq!(name(&bench, kept), "untitled.ne5p");
        assert!(
            bench.workspace.get(renamed).is_some(),
            "the one renamed stays until the overwrite is saved"
        );
    }

    /// Overwriting the only copy of an edit is not a choice.
    #[test]
    fn an_asset_with_unsaved_edits_is_not_offered_for_overwrite() {
        let mut bench = Bench::new();
        let [held, moving] = placed(&mut bench, 2)[..] else {
            unreachable!()
        };
        let edited = with_gain(&bench.workspace.get(held).unwrap().bytes);
        bench.workspace.replace_bytes(held, edited, &mut bench.log);

        bench.act(vec![Act::RenameLocal {
            id: moving,
            name: "untitled.ne5p".into(),
        }]);
        let (_, answers) = bench.browser.asking().expect("a question");
        assert_eq!(answers, ["Cancel", "Keep both"]);
    }

    #[test]
    fn a_file_opened_under_a_taken_name_lands_only_once_asked() {
        let mut bench = Bench::new();
        placed(&mut bench, 1);
        let bytes = Fresh::Program.bytes().unwrap();

        bench.act(vec![Act::Import {
            name: "Untitled.ne5p".into(),
            bytes,
        }]);
        assert_eq!(bench.workspace.listed().count(), 1, "nothing landed yet");
        let (title, _) = bench.browser.asking().expect("a question");
        assert_eq!(
            title,
            "“Untitled.ne5p” is already in the top level of the library"
        );

        let acts = bench.browser.answer("Keep both");
        bench.act(acts);
        let names: Vec<&str> = bench
            .workspace
            .listed()
            .map(|entity| entity.name.as_str())
            .collect();
        assert_eq!(names, ["untitled.ne5p", "Untitled 2.ne5p"]);
    }

    #[test]
    fn removing_a_folder_moves_what_is_in_it_up_and_deletes_nothing() {
        let mut bench = Bench::new();
        let [inside] = placed(&mut bench, 1)[..] else {
            unreachable!()
        };
        let root = crate::store::LibPath::root();
        let folder = bench.browser.folders.make(&root, &bench.workspace);
        let within = bench.browser.folders.path_of(folder).unwrap().clone();
        let nested = bench.browser.folders.make(&within, &bench.workspace);
        bench.act(vec![Act::RenameFolder {
            id: nested,
            name: "Strings".into(),
        }]);
        bench
            .browser
            .folders
            .file(&mut bench.workspace, inside, Some(folder));
        bench.browser.folders.take_ops();

        bench.act(vec![Act::RemoveFolder(folder)]);
        let path = bench.workspace.get(inside).unwrap().path.clone().unwrap();
        assert_eq!(path.as_str(), "untitled.ne5p");
        assert_eq!(
            bench
                .browser
                .folders
                .path_of(nested)
                .map(|path| path.as_str()),
            Some("Strings")
        );
        assert!(bench.browser.folders.path_of(folder).is_none());
        let ops = bench.browser.folders.take_ops();
        assert_eq!(
            ops.last(),
            Some(&crate::folders::Op::RemoveDir(within)),
            "removed once what was in it has moved: {ops:?}"
        );
    }

    #[test]
    fn a_folder_whose_contents_would_collide_a_level_up_stays() {
        let mut bench = Bench::new();
        let [outside, inside] = placed(&mut bench, 2)[..] else {
            unreachable!()
        };
        let root = crate::store::LibPath::root();
        let folder = bench.browser.folders.make(&root, &bench.workspace);
        let within = bench
            .browser
            .folders
            .path_of(folder)
            .unwrap()
            .join("untitled.ne5p");
        bench.workspace.place(inside, within.clone());

        bench.act(vec![Act::RemoveFolder(folder)]);
        assert!(bench.browser.folders.path_of(folder).is_some());
        assert_eq!(
            bench.workspace.get(inside).unwrap().path.as_ref(),
            Some(&within)
        );
        assert_eq!(name(&bench, outside), "untitled.ne5p");
        let said = bench.log.status().1;
        assert!(said.contains("was not removed"), "{said}");
    }

    fn with_gain(bytes: &[u8]) -> Vec<u8> {
        crate::fields::apply(bytes, &[("center_panel.gain".into(), "96".into())])
            .unwrap()
            .1
    }

    /// Deleting a slot lets go of its row, and a sound linked to it stops claiming to be
    /// on the keyboard once the bank is read again.
    #[test]
    fn a_deleted_slot_leaves_the_selection_and_its_sound_leaves_the_keyboard() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        let id = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        let crc = bench.workspace.get(id).unwrap().saved.crc32().unwrap();
        bench
            .device
            .pretend_bodies(class, 7, &[Some(("New program", crc))]);
        bench.device.relink(&mut bench.workspace);
        let dot = |bench: &Bench| {
            crate::library::keyboard_mark(
                bench.workspace.get(id).unwrap(),
                &bench.device.state,
                &bench.queue,
            )
        };
        assert_eq!(dot(&bench), Some(crate::library::Mark::Agrees));
        let slot = Item::Slot { class, at: at(0) };
        bench.browser.selection.only(slot);

        bench.act(vec![Act::DeleteSlot { class, at: at(0) }]);
        assert!(!bench.browser.selection.holds(slot), "still selected");

        bench.device.pump();
        bench.device.pretend(crate::device::DeviceEvent::Finished);
        poll(&mut bench);
        bench.device.pump();
        bench
            .device
            .pretend(crate::device::DeviceEvent::BankScanned {
                class,
                bank: 7,
                slots: vec![None],
            });
        poll(&mut bench);
        assert_eq!(dot(&bench), None);
    }

    /// The instrument keeps one sample of each name, so a sample whose name a slot
    /// already has stands for that slot and is queued to replace it. Sent to any other
    /// slot it is refused before it reaches the queue, as the instrument would refuse it.
    #[test]
    fn a_sample_whose_name_the_library_has_goes_to_that_slot_and_nowhere_else() {
        let mut bench = Bench::new();
        let class = ObjectClass::Sample;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench
            .device
            .pretend_scanned(class, 1, &["drawbar-tine", "", "drawbar-pad", ""]);
        let id = bench.workspace.ingest(
            "drawbar-pad.nsmp".into(),
            Origin::File("drawbar-pad.nsmp".into()),
            crate::testing::sample_bytes(),
            &mut bench.log,
        );
        bench.device.relink(&mut bench.workspace);
        let holder = Location::from_user(1, 3);
        assert_eq!(
            bound_for(
                bench.workspace.get(id).unwrap(),
                &bench.device.state,
                &bench.queue
            ),
            Bound::At(class, holder)
        );

        bench.act(vec![Act::Send {
            id,
            class,
            at: Location::from_user(1, 2),
        }]);
        assert!(!bench.queue.holds(id), "queued for a slot it cannot take");
        let said = bench.log.status().1;
        assert!(said.contains("Samples 1:3 already has its name"), "{said}");

        bench.act(vec![Act::Send {
            id,
            class,
            at: holder,
        }]);
        assert_eq!(bench.queue.entry(id).map(|held| held.at), Some(holder));
    }

    /// Two sounds of one name cannot both wait for a library, and a name the instrument
    /// gains after a sound was queued holds that sound back from the next send.
    #[test]
    fn a_library_name_already_waiting_or_newly_taken_holds_a_sample_back() {
        let mut bench = Bench::new();
        let class = ObjectClass::Sample;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench.device.pretend_scanned(class, 1, &["", "", ""]);
        let ids: Vec<u64> = ["lead.nsmp", "lead.nsmp"]
            .into_iter()
            .map(|name| {
                bench.workspace.ingest(
                    name.into(),
                    Origin::File(name.into()),
                    crate::testing::sample_bytes(),
                    &mut bench.log,
                )
            })
            .collect();
        let slot = |n| Location::from_user(1, n);

        bench.act(vec![
            Act::Send {
                id: ids[0],
                class,
                at: slot(1),
            },
            Act::Send {
                id: ids[1],
                class,
                at: slot(2),
            },
        ]);
        assert_eq!(bench.queue.ids(), vec![ids[0]]);
        let said = bench.log.status().1;
        assert!(said.contains("Something waiting for Samples 1:1"), "{said}");

        bench.device.pretend_scanned(class, 1, &["", "", "lead"]);
        crate::queue::refit(
            &bench.workspace,
            &bench.device.state,
            &mut bench.queue,
            &mut bench.log,
        );
        let failure = bench
            .queue
            .entry(ids[0])
            .and_then(|held| held.failure.clone());
        assert!(
            failure.is_some_and(|why| why.contains("Samples 1:3 already has its name")),
            "the entry would be sent into a refusal"
        );
    }

    /// A second sound of one name may still take the first one's place in the queue, and
    /// a rename that gives two waiting sounds one name holds the later one back.
    #[test]
    fn a_library_name_is_checked_against_the_queue_as_it_stands() {
        let mut bench = Bench::new();
        let class = ObjectClass::Sample;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench.device.pretend_scanned(class, 1, &["", "", ""]);
        let ids: Vec<u64> = ["lead.nsmp", "lead.nsmp", "pad.nsmp"]
            .into_iter()
            .map(|name| {
                bench.workspace.ingest(
                    name.into(),
                    Origin::File(name.into()),
                    crate::testing::sample_bytes(),
                    &mut bench.log,
                )
            })
            .collect();
        let slot = |n| Location::from_user(1, n);
        let send = |id, at| Act::Send { id, class, at };

        bench.act(vec![send(ids[0], slot(1)), send(ids[1], slot(1))]);
        assert_eq!(bench.queue.ids(), vec![ids[1]], "the second takes the slot");

        bench.act(vec![send(ids[2], slot(2))]);
        bench.workspace.rename(ids[2], "lead.nsmp".into());
        crate::queue::refit(
            &bench.workspace,
            &bench.device.state,
            &mut bench.queue,
            &mut bench.log,
        );
        let failure = |id| bench.queue.entry(id).and_then(|held| held.failure.clone());
        assert_eq!(failure(ids[1]), None, "the first in the queue goes");
        assert!(
            failure(ids[2]).is_some_and(|why| why.contains("Something waiting for Samples 1:1")),
            "the renamed one would be refused after the first landed"
        );
    }

    fn poll(bench: &mut Bench) {
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
    }
}
