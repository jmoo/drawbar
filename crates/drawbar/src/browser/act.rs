//! What the browser asks for, and the running of it.
//!
//! Rendering answers with [`Act`]s rather than acting, so a row can be drawn while the
//! thing it stands for is about to change. [`apply`] is where they meet the workspace,
//! the device and the tabs.

use nord_usb::{Location, ObjectClass};

use super::drag::Item;
use super::Browser;
use crate::device::{write_warning, Device, DeviceCmd, Outgoing};
use crate::log::Log;
use crate::strings::place;
use crate::tabs::Tabs;
use crate::workspace::{Fresh, LocalEntity, Workspace};

/// What the browser asks the rest of the app to do.
pub enum Act {
    Connect,
    Disconnect,
    OpenFiles,
    New(Fresh),
    /// Pick the WAVs a new Sample Editor project is laid out from. The project itself
    /// is made once the dialog has each file's root key — see [`crate::newproject`].
    NewProject,
    /// Read the whole instrument again — every class, its geometry and its focus.
    Resync,
    ReadAgain(ObjectClass),
    Open(Item),
    /// A view of a slot becomes an asset on this computer.
    Keep(u64),
    NewFolder,
    RemoveFolder(u64),
    /// Put an asset in a folder, or out of the one it is in.
    File {
        id: u64,
        folder: Option<u64>,
    },
    /// Write every sendable asset in a folder back where it came from. Already agreed to.
    SendFolder(u64),
    Copy {
        class: ObjectClass,
        at: Location,
    },
    LoadOnInstrument {
        class: ObjectClass,
        at: Location,
    },
    /// Put a local asset into a slot, asking first if something is already there.
    Send {
        id: u64,
        class: ObjectClass,
        at: Location,
    },
    /// Write everything waiting, grouped by folder. Already agreed to.
    SendAll,
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
    Save(u64),
    /// Nothing happened, and this is why.
    Refused(String),
}

/// Run what the browser asked for.
pub fn apply(
    browser: &mut Browser,
    acts: Vec<Act>,
    workspace: &mut Workspace,
    device: &mut Device,
    tabs: &mut Tabs,
    log: &mut Log,
) {
    for act in acts {
        match act {
            Act::Connect => device.connect(log),
            Act::Disconnect => device.disconnect(log),
            Act::OpenFiles => workspace.open_dialog(),
            Act::New(kind) => {
                if let Some(id) = workspace.create(kind, log) {
                    tabs.open(id, workspace);
                }
            }
            Act::NewProject => workspace.pick_wavs(),
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
            Act::SendFolder(id) => {
                let members: Vec<u64> = browser
                    .folders
                    .members(id, workspace)
                    .iter()
                    .map(|entity| entity.id)
                    .collect();
                send_batch(&members, workspace, device, log);
            }
            Act::Open(Item::Folder(_)) => {}
            Act::Open(Item::Local(id)) => tabs.open(id, workspace),
            // ⚠️ One view per slot prevents divergent copies queued back to one address.
            Act::Open(Item::Slot { class, at }) => match workspace.view_of(class, at) {
                Some(id) => tabs.open(id, workspace),
                None => device.send(
                    DeviceCmd::Get {
                        class,
                        at,
                        body: false,
                        open: true,
                    },
                    log,
                ),
            },
            Act::Copy { class, at } => device.send(
                DeviceCmd::Get {
                    class,
                    at,
                    body: false,
                    open: false,
                },
                log,
            ),
            Act::LoadOnInstrument { class, at } => {
                device.send(DeviceCmd::Select { class, at }, log)
            }
            Act::Send { id, class, at } => {
                send(browser, workspace, device, log, id, class, at, true)
            }
            Act::Replace { id, class, at } => {
                send(browser, workspace, device, log, id, class, at, false)
            }
            Act::SendAll => {
                let waiting: Vec<u64> = workspace.pending().iter().map(|e| e.id).collect();
                send_batch(&waiting, workspace, device, log);
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
                tabs.close(id);
                browser.folders.forget(id);
                workspace.remove(id, log);
            }
            Act::Save(id) => workspace.export(id),
            Act::Refused(why) => log.say(why),
        }
    }
}

/// Write a set of assets back, one command per folder.
///
/// The one write path a batch takes, whether the batch is everything waiting or one of
/// this computer's own folders: same refusal, same grouping, same per-item flow.
fn send_batch(ids: &[u64], workspace: &Workspace, device: &mut Device, log: &mut Log) {
    // Validate the whole batch before the first delete-then-write.
    for entity in ids.iter().filter_map(|id| workspace.get(*id)) {
        if owed(entity).is_none() {
            continue;
        }
        if let Err(e) = nord_usb::envelope::unwrap(&entity.bytes) {
            log.error(format!("{}: {e}", entity.name));
            log.trouble(format!(
                "“{}” is not a file the instrument takes, so nothing was sent.",
                entity.name
            ));
            return;
        }
    }
    for (class, items) in grouped(ids, workspace) {
        device.send(DeviceCmd::SendAll { class, items }, log);
    }
}

/// The assets named, gathered per folder in the order the list holds them.
///
/// A session belongs to a folder, so a folder is the unit a batch is cut into.
fn grouped(ids: &[u64], workspace: &Workspace) -> Vec<(ObjectClass, Vec<Outgoing>)> {
    let mut by_class: Vec<(ObjectClass, Vec<Outgoing>)> = Vec::new();
    for entity in ids.iter().filter_map(|id| workspace.get(*id)) {
        let Some((class, at)) = owed(entity) else {
            continue;
        };
        let item = Outgoing {
            id: entity.id,
            at,
            name: entity.name.clone(),
            bytes: entity.bytes.clone(),
        };
        match by_class.iter_mut().find(|(held, _)| *held == class) {
            Some((_, items)) => items.push(item),
            None => by_class.push((class, vec![item])),
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

/// Everything worth reading before a write into `class` lands: what the format
/// comparison found, and what the class itself disturbs beyond the slot.
pub(super) fn write_warnings(
    class: ObjectClass,
    tag: &str,
    resident: &[String],
) -> impl Iterator<Item = String> {
    [
        foreign_format(tag, resident),
        write_warning(class).map(str::to_string),
    ]
    .into_iter()
    .flatten()
}

/// The same set as one note, for the dialog that asks about a single slot.
fn write_note(class: ObjectClass, tag: &str, resident: &[String]) -> Option<String> {
    let note: Vec<String> = write_warnings(class, tag, resident).collect();
    (!note.is_empty()).then(|| note.join("\n\n"))
}

/// Where an asset would be written back to, if anywhere: the slot it came off, and only
/// where this app will write into that class at all.
pub(super) fn owed(entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    let (class, at) = entity.origin.slot()?;
    crate::device::sendable(class).then_some((class, at))
}

/// Put a local asset into a slot.
///
/// `ask` is false once the replace question has been answered, which is what keeps the
/// answer from raising the question again.
#[allow(clippy::too_many_arguments)]
fn send(
    browser: &mut Browser,
    workspace: &Workspace,
    device: &mut Device,
    log: &mut Log,
    id: u64,
    class: ObjectClass,
    at: Location,
    ask: bool,
) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    // Refused before the transport is touched: bytes that are not what they claim to be
    // must not reach a delete-then-write.
    if let Err(e) = nord_usb::envelope::unwrap(&entity.bytes) {
        log.error(format!("{}: {e}", entity.name));
        log.trouble(format!(
            "“{}” is not a file the instrument takes.",
            entity.name
        ));
        return;
    }
    let occupant = device
        .state
        .slot(class, at)
        .flatten()
        .map(|info| info.name.trim().to_string());
    match (ask, occupant) {
        (true, Some(occupant)) => browser.ask_replace(
            &occupant,
            &entity.name,
            place(class, at),
            write_note(class, &entity.tag(), &device.state.formats_in(class)),
            Act::Replace { id, class, at },
        ),
        _ => device.send(
            DeviceCmd::Put {
                id,
                class,
                at,
                name: entity.name.clone(),
                bytes: entity.bytes.clone(),
            },
            log,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::bench::bench;
    use crate::device::BROWSED;
    use crate::strings::folder;
    use crate::tabs::Tabs;
    use eframe::egui;

    /// A batch is one command per folder, because a session belongs to a folder — and
    /// something that cannot be written is not queued at all.
    #[test]
    fn a_batch_is_grouped_into_one_command_per_folder() {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let at = |slot| Location { bank: 6, slot };
        for (class, slot) in [
            (ObjectClass::Program, 0),
            (ObjectClass::Program, 1),
            (ObjectClass::SetList, 0),
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
            workspace.mark_pending(id, true);
        }

        let waiting: Vec<u64> = workspace.pending().iter().map(|e| e.id).collect();
        let queued = grouped(&waiting, &workspace);
        assert_eq!(queued.len(), 2, "one command per folder");
        let programs = queued
            .iter()
            .find(|(class, _)| *class == ObjectClass::Program)
            .expect("programs are queued");
        assert_eq!(programs.1.len(), 2);
        assert!(queued.iter().all(|(class, _)| *class != ObjectClass::Piano));
    }

    /// A lone send names the asset it is sending, so the write can pay off that one
    /// document's debt the way a batch pays off its own.
    #[test]
    fn sending_one_document_names_the_asset_it_sends() {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        let at = Location { bank: 6, slot: 1 };
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let id = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            bytes,
            &mut log,
        );
        workspace.mark_pending(id, true);

        // The slot is empty in the scan, so nothing is asked and the put goes straight out.
        apply(
            &mut browser,
            vec![Act::Send {
                id,
                class: ObjectClass::Program,
                at,
            }],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        let queued = device.queued().front().expect("a put was queued");
        match queued {
            DeviceCmd::Put { id: sending, .. } => assert_eq!(*sending, id),
            other => panic!("{}", other.label()),
        }
    }

    /// Sending a folder is the batch the queue already runs: the same grouping into one
    /// command per instrument folder, and the same refusal of anything that cannot be
    /// written.
    #[test]
    fn a_folder_sends_only_what_can_go_back_to_a_slot() {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = crate::log::Log::default();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let at = |slot| Location { bank: 6, slot };
        let mut ids = Vec::new();
        for (class, slot) in [
            (ObjectClass::Program, 0),
            (ObjectClass::SetList, 0),
            // The live buffer takes a write like any other slot.
            (ObjectClass::Live, 0),
            // A piano is installed by the instrument, so it must not reach the queue.
            (ObjectClass::Piano, 0),
        ] {
            ids.push(workspace.ingest(
                format!("{}.ne5p", place(class, at(slot))),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                bytes.clone(),
                &mut log,
            ));
        }
        // Never off an instrument, so there is nowhere to send it back to.
        ids.push(workspace.create(Fresh::Program, &mut log).unwrap());

        let queued = grouped(&ids, &workspace);
        let classes: Vec<ObjectClass> = queued.iter().map(|(class, _)| *class).collect();
        assert_eq!(
            classes,
            vec![
                ObjectClass::Program,
                ObjectClass::SetList,
                ObjectClass::Live
            ]
        );
        assert!(queued.iter().all(|(_, items)| items.len() == 1));
        // A folder holding nothing sendable queues nothing at all: the piano, and the
        // one that never came off an instrument.
        assert!(grouped(&ids[3..], &workspace).is_empty());
    }

    /// A double-click on a slot opens a view: a tab and a document, and no new row in
    /// the list. Keeping it is what puts it there.
    #[test]
    fn opening_a_slot_does_not_put_it_on_this_computer() {
        use crate::device::DeviceEvent;
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let at = Location { bank: 6, slot: 3 };
        let origin = Origin::Device {
            class: ObjectClass::Program,
            at,
        };

        device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin,
            bytes,
            open: true,
        });
        device.poll(&mut log, &mut workspace, &mut tabs);

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
            vec![Act::Keep(id)],
            &mut workspace,
            &mut device,
            &mut tabs,
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
        let (mut browser, mut workspace, mut device, mut tabs, mut log) = bench();
        let mut new_folder = |browser: &mut Browser| {
            apply(
                browser,
                vec![Act::NewFolder],
                &mut workspace,
                &mut device,
                &mut tabs,
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
        let (mut browser, mut workspace, mut device, mut tabs, mut log) = bench();
        let mut act = |browser: &mut Browser, act| {
            apply(
                browser,
                vec![act],
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut log,
            )
        };
        act(&mut browser, Act::NewFolder);
        let Some(Item::Folder(id)) = browser.rename.as_ref().map(|r| r.what) else {
            panic!("a new folder arms its editor");
        };

        act(&mut browser, Act::RemoveFolder(id));
        assert!(browser.rename.is_none(), "the editor went with it");
        assert!(browser.selection.is_none());

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
        use crate::workspace::Origin;

        let (mut browser, mut workspace, mut device, mut tabs, mut log) = bench();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let class = ObjectClass::Program;
        let at = Location { bank: 6, slot: 3 };
        device.pretend_scanned(class, 7, &["", "", "", "Africa Split"]);

        device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin: Origin::Device { class, at },
            bytes,
            open: true,
        });
        device.poll(&mut log, &mut workspace, &mut tabs);
        let first = tabs.active().expect("a view opened");

        // Another double-click on the same slot.
        tabs.close(first);
        apply(
            &mut browser,
            vec![Act::Open(Item::Slot { class, at })],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        assert!(device.queued().is_empty(), "nothing was read again");
        assert_eq!(tabs.active(), Some(first), "its own tab came forward");
        assert_eq!(workspace.entities().len(), 1, "and there is one copy");

        // A slot with no view open is read, as it must be.
        let elsewhere = Location { bank: 6, slot: 4 };
        apply(
            &mut browser,
            vec![Act::Open(Item::Slot {
                class,
                at: elsewhere,
            })],
            &mut workspace,
            &mut device,
            &mut tabs,
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
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        apply(
            &mut browser,
            vec![Act::Resync],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        for class in BROWSED {
            let progress = device.state.scan.progress(class);
            assert!(
                progress.is_some_and(|progress| progress.running),
                "{}",
                folder(class)
            );
        }
    }
}
