//! The document: one view of one asset, which looking at and changing are the same act.
//!
//! An edit lands on the tab's working copy the moment it is made — set the field,
//! re-encode, re-check the bytes — and the asset reads as unsaved. Nothing on the
//! instrument moves until the header's Send does it. Revert goes back to the bytes the
//! asset was last saved as, which is the only undo there is.

use eframe::egui;
use nord_format::fields::Field;
use nord_format::formats::nsmp::codec;
use nord_usb::{Location, ObjectClass};

use crate::device::Device;
use crate::fields;
use crate::log::Log;
use crate::queue::Queue;
use crate::strings;
use crate::tags::Tags;
use crate::workspace::{LocalEntity, Workspace};

mod advanced;
pub mod capability;
pub mod controls;
pub(crate) mod encode;
mod field;
mod header;
pub mod keys;
mod panel;
mod piano;
mod project;
pub(crate) mod sample;
mod setlist;

use advanced::Advanced;
use controls::{Ctx, Sets};

pub use header::{Body, Cell, Extras, Face, Ink, Loud, SizeLine, Stage, StateLine, Tone};
pub use sample::note_picker;

/// The body's own scroll id — see [`crate::tabs::SCROLL`].
pub const SCROLL: &str = "document_body";

/// The id the body's own `Ui` is salted with, so the scroll region inside it answers to
/// an id that does not move with the number of widgets drawn before it.
pub const PAGE: &str = "document_page";

/// The room the body keeps inside the centre. The header is full bleed and claims none
/// of it.
const BODY_MARGIN: f32 = 8.0;

/// A put the header asked for. The browser owns the question it may need to raise.
pub struct SendBack {
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
}

/// The rest of the window a document reads: what is waiting to be sent, and what this
/// computer's list labels things with.
pub struct Around<'a> {
    pub queue: &'a Queue,
    pub tags: &'a Tags,
}

/// What a document's frame asked the app for.
#[derive(Default)]
pub struct Wants {
    /// The write the header queued.
    pub send: Option<SendBack>,
    /// The banner's offer to put a view of a slot on this computer.
    pub keep: bool,
}

/// What the Edit face asked for that cannot be done while the asset is borrowed to
/// draw it: audio, or a new asset made out of this one.
enum Asked {
    Zone(sample::Ask),
    Encode,
}

#[derive(Default)]
pub struct Document {
    target: Option<u64>,
    /// Which face each document was left on.
    views: std::collections::HashMap<u64, Face>,
    /// Per-field legal values and controls, cached as they are drawn. One per document —
    /// see [`Ctx`].
    ctx: Ctx,
    /// The header's name box, so a half-typed name survives a frame, and the piano's
    /// variant box beside it.
    name: String,
    variant: String,
    /// The path boxes for a project's audio files, by file id — same reason.
    paths: std::collections::HashMap<u32, String>,
    /// The last refusal, and what caused it.
    error: Option<String>,
    /// Whether this document has already had its dependencies read without being asked.
    /// One read per document: the button is what asks for another.
    fetched_deps: bool,
    advanced: Advanced,
    /// Zone audio decoded on request, dropped when the bytes under it change.
    audio: sample::Cache,
    /// What the instrument editor keeps between frames: the open zone, the struck key,
    /// the folded key table. Never an edit — an edit is on the working copy at once.
    sample: sample::State,
    /// Which zone is sounding, and the one backend that makes it sound.
    player: crate::audio::Player,
    /// The encode panel over a WAV, and the read of the WAV it works from.
    wav: Option<(encode::Draft, encode::Source)>,
    /// What the field document keeps between frames: the morph lens, where the reader
    /// is, and the two decodes a pending count is measured across. Never an edit.
    fields: field::State,
}

impl Document {
    /// Draw the open tab's document: the header full bleed, and the body under it.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        workspace: &mut Workspace,
        device: &mut Device,
        log: &mut Log,
        around: &Around<'_>,
    ) -> Wants {
        let Some(entity) = workspace.get(id) else {
            return Wants::default();
        };
        let stamp = entity.stamp;
        let decoded = entity.entity.as_ref();
        let registry = decoded.map(fields::fields_of).unwrap_or_default();
        let viewing = workspace.is_view(id);

        if self.target != Some(id) {
            self.target = Some(id);
            self.error = None;
            self.fetched_deps = false;
            self.advanced.leave();
            self.ctx = Ctx::default();
            (self.name, self.variant) = header::boxes(entity, viewing);
            self.paths.clear();
            self.sample = sample::State::default();
            self.fields = field::State::default();
            // ⚠️ Leaving the tab is leaving the sound: a zone that goes on playing over
            // another document is a sound with nothing on screen to stop it.
            self.player.stop();
            // Reading a WAV copies every sample, so it happens on arrival and never per
            // frame — the panel works from what is read here.
            self.wav = match decoded.is_none() && encode::is_wav(&entity.bytes) {
                true => Some((
                    encode::Draft::new(&entity.name),
                    encode::Source::read(&entity.bytes),
                )),
                false => None,
            };
        }
        // Decoded audio belongs to one set of bytes; an edit re-encodes all of them.
        self.audio.follow(id, entity.stamp);
        // Paint marks are measured against the bytes the asset was last saved as.
        sample::follow(&mut self.sample, id, &entity.saved.bytes);
        self.player.settle();
        if self.player.playing().is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }

        let faces = faces(entity, registry.as_deref());
        let face = showing(&faces, self.views.get(&id).copied().unwrap_or_default());

        // ⚠️ Only a registry body. Reading the saved bytes means decoding them, and a
        // piano library is hundreds of megabytes with no field in it.
        if registry.is_some() {
            self.fields.follow(entity);
        }
        let doc = match (decoded, registry.as_deref()) {
            (Some(decoded), Some(fields)) => Some(field::of(decoded, fields)),
            _ => None,
        };
        let pending = match doc.is_some() {
            true => self.fields.pending().len(),
            false => 0,
        };

        let mut sets: Sets = Vec::new();
        let act = header::ui(
            ui,
            entity,
            &header::Facts {
                faces: &faces,
                showing: face,
                device: &device.state,
                queue: around.queue,
                tags: around.tags,
                view: viewing,
                // The piano's `188 of 194 MB` and its refusal to queue a library that
                // does not fit go here beside the field document's pending count.
                extras: header::Extras {
                    edited: (pending > 0).then(|| StateLine {
                        words: format!("{pending} pending"),
                        ink: Ink::Warn,
                        hint: format!("raw ≠ bits on {pending} fields"),
                    }),
                    loud: queued(entity, device, pending),
                    size: None,
                },
            },
            (&mut self.name, &mut self.variant),
            &mut sets,
        );
        self.views.insert(id, act.face.unwrap_or(face));

        let mut wants = Wants {
            send: act.send,
            keep: false,
        };
        let mut details = None;
        let mut typed = false;
        let mut asked = None;
        let mut wanted = None;
        let mut lookup = piano_lookup(entity, registry.as_deref(), device);
        // A `Ui` of its own rather than a `Frame`: the margin is the same, and the
        // salted id keeps the body's scroll state answering to one name whatever the
        // header drew above it — see [`PAGE`].
        let mut page = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(PAGE)
                .max_rect(ui.available_rect_before_wrap().shrink(BODY_MARGIN)),
        );
        {
            let ui = &mut page;
            // ⚠️ A view's tab looks like a local document; the banner is the only
            // visible indication that its bytes still belong to the instrument.
            if viewing {
                wants.keep = viewing_banner(ui, entity);
            }
            if let Some(why) = &self.error {
                ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
            }
            if face == Face::Edit {
                asked = self
                    .pinned(ui, entity, doc.as_ref(), &mut sets)
                    .map(Asked::Zone);
            }
            egui::ScrollArea::vertical()
                .id_salt(SCROLL)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    // ⚠️ Widget state keyed only by field path leaks between tabs of
                    // the same format, so every control also answers to the document id.
                    ui.push_id(id, |ui| match face {
                        Face::Edit => {
                            if let Some(from_body) = self.body(
                                ui,
                                entity,
                                doc.as_ref(),
                                &mut lookup,
                                &mut wanted,
                                &mut sets,
                            ) {
                                asked = Some(from_body);
                            }
                        }
                        Face::Advanced => match doc.as_ref() {
                            Some(doc) => {
                                Advanced::about(ui, &field::about(doc, entity));
                                let table = advanced::Table {
                                    fields: registry.as_deref().unwrap_or_default(),
                                    saved: self.fields.settled(),
                                    changed: self.fields.pending(),
                                    doc: Some(doc),
                                };
                                self.advanced.table(ui, &table, &mut sets);
                                typed = !sets.is_empty();
                            }
                            None => capabilities(ui, entity),
                        },
                        Face::Metadata => {
                            record(ui, entity);
                            details = self.advanced.meta(ui, entity, device)
                        }
                    });
                });
        }
        let drawn = page.min_rect();
        ui.advance_cursor_after_rect(drawn.expand(BODY_MARGIN));

        if let Some(face) = wanted {
            self.views.insert(id, face);
        }

        if let Some(details) = details {
            for cmd in advanced::commands(details) {
                device.send(cmd, log);
            }
        }
        if let Some(asked) = asked {
            self.answer(id, asked, workspace, log);
        }
        if let Some((class, at)) = workspace.get(id).and_then(|e| e.origin.slot()) {
            if lookup.asked || self.owes_deps(&lookup, at, device) {
                self.fetched_deps = true;
                device.send(crate::device::DeviceCmd::Deps { class, at }, log);
            }
        }
        if let Some(name) = act.rename {
            workspace.rename(id, name);
            // The box is holding what was typed; the asset now carries it with its tag.
            self.target = None;
        }
        if act.export {
            workspace.export(id);
        }
        if act.revert {
            workspace.revert(id, log);
            self.error = None;
            // Reread on the next frame: the name box is holding an edit that is gone.
            self.target = None;
            return wants;
        }
        if !sets.is_empty() {
            let outcome = self.apply(id, sets, workspace, log);
            if typed {
                // The table keeps a refused cell open with what was typed in it.
                self.advanced.settled(outcome);
            }
        }
        if workspace.get(id).is_some_and(|held| held.stamp != stamp) {
            // The strip was drawn from the bytes this frame then edited; one more frame
            // shows what the edit made of them.
            ui.ctx().request_repaint();
        }
        wants
    }

    /// Whether this frame should read the slot's dependencies without being asked to.
    ///
    /// The piano's name is the one thing about a program that no file carries, so a
    /// document opened off a slot with an instrument attached reads it straight away
    /// rather than sitting on an id until someone clicks. Once per document, and never
    /// with nothing to learn: no instrument, no piano named, a name already in hand, or a
    /// list the instrument has already given for this slot and simply did not name it in.
    fn owes_deps(&self, lookup: &panel::PianoLookup, at: Location, device: &Device) -> bool {
        if self.fetched_deps || !lookup.can_ask || lookup.id.is_none() || lookup.name.is_some() {
            return false;
        }
        let detail = &device.state.detail;
        !(detail.at == Some(at) && detail.deps.is_some())
    }

    /// Nothing is open any more.
    ///
    /// ⚠️ A zone goes on sounding until something stops it, and the control that would
    /// stop it is on the document. With no document there is nothing to click.
    pub fn leave(&mut self) {
        self.target = None;
        self.player.stop();
    }

    /// Whichever shape this asset is.
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        doc: Option<&field::Doc<'_>>,
        piano: &mut panel::PianoLookup,
        wanted: &mut Option<Face>,
        sets: &mut Sets,
    ) -> Option<Asked> {
        let Some(decoded) = &entity.entity else {
            return self.wav_body(ui);
        };
        if sample::is_sample(decoded) {
            return self.sample_body(ui, decoded, sets).map(Asked::Zone);
        }
        if project::is_project(decoded) {
            self.project_body(ui, decoded, sets);
            return None;
        }
        if let Some(doc) = doc {
            if field::body(ui, &self.ctx, &mut self.fields, doc, piano, sets) {
                *wanted = Some(Face::Advanced);
            }
            return None;
        }
        setlist::ui(ui, decoded, sets);
        None
    }

    /// Bytes that did not decode: the encode panel where they are a WAV, and the plain
    /// report where they are anything else.
    fn wav_body(&mut self, ui: &mut egui::Ui) -> Option<Asked> {
        let Some((draft, source)) = &mut self.wav else {
            ui.label(
                egui::RichText::new(
                    "This file did not decode, so there is nothing to show but its bytes.",
                )
                .weak(),
            );
            return None;
        };
        encode::ui(ui, draft, source).then_some(Asked::Encode)
    }

    fn sample_body(
        &mut self,
        ui: &mut egui::Ui,
        decoded: &nord_format::Entity,
        sets: &mut Sets,
    ) -> Option<sample::Ask> {
        let snapshot = match sample::snapshot(decoded)? {
            Ok(snapshot) => snapshot,
            Err(why) => {
                ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
                return None;
            }
        };
        let sounding = self.player.playing();
        let target = self.target;
        let sounds: Vec<sample::Sound> = (0..snapshot.zones.len())
            .map(|index| sample::Sound {
                decoded: self.audio.get(index),
                playing: sounding == target.map(|id| (id, index)),
            })
            .collect();
        sample::ui(ui, &mut self.sample, &snapshot, &sounds, sets)
    }

    /// What an editor keeps in front of the body: above the scroll region, on the panel
    /// fill, so it stays where it is while the rows under it move.
    ///
    /// The instrument key map is what this region is for. A kind with nothing to pin
    /// takes up no room.
    fn pinned(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        doc: Option<&field::Doc<'_>>,
        sets: &mut Sets,
    ) -> Option<sample::Ask> {
        if let Some(doc) = doc {
            field::nav(ui, &mut self.fields, doc);
            return None;
        }
        let decoded = entity.entity.as_ref()?;
        if let Some(Ok(snapshot)) = sample::snapshot(decoded) {
            return sample::map(ui, &mut self.sample, &snapshot, sets);
        }
        if let Some(Ok(snapshot)) = project::snapshot(decoded) {
            project::map(ui, &mut self.sample, &snapshot, sets);
        }
        None
    }

    /// Do what the Basic view asked for, now that nothing is borrowing the asset.
    fn answer(&mut self, id: u64, asked: Asked, workspace: &mut Workspace, log: &mut Log) {
        match asked {
            Asked::Zone(sample::Ask::Decode(zone)) => {
                if let Some(decoded) = workspace.get(id).and_then(|e| e.entity.as_ref()) {
                    self.audio.decode(decoded, zone);
                }
            }
            Asked::Zone(sample::Ask::Strike { zone, semitones }) => {
                if let Some(decoded) = workspace.get(id).and_then(|e| e.entity.as_ref()) {
                    self.audio.decode(decoded, zone);
                }
                let Some(Ok(decoded)) = self.audio.get(zone) else {
                    return;
                };
                let rate = crate::audio::rate(semitones);
                if let Err(why) = self.player.strike((id, zone), &decoded.audio, rate) {
                    log.error(why);
                    log.trouble("This computer would not play that zone.");
                }
            }
            Asked::Zone(sample::Ask::Play(zone)) => {
                let Some(Ok(decoded)) = self.audio.get(zone) else {
                    return;
                };
                if let Err(why) = self.player.toggle((id, zone), &decoded.audio) {
                    log.error(why);
                    log.trouble("This computer would not play that zone.");
                }
            }
            Asked::Zone(sample::Ask::Save(zone)) => {
                let Some(Ok(decoded)) = self.audio.get(zone) else {
                    return;
                };
                let audio = &decoded.audio;
                match nord_format::wav::pcm16(&audio.samples, codec::FIELD_RATE, audio.channels) {
                    Ok(bytes) => workspace.save_bytes(
                        crate::workspace::zone_wav_name(
                            &self.instrument_name(id, workspace),
                            zone + 1,
                        ),
                        bytes,
                    ),
                    Err(e) => {
                        log.error(e.to_string());
                        log.trouble("That zone could not be written as a WAV.");
                    }
                }
            }
            Asked::Encode => self.encode(id, workspace, log),
        }
    }

    /// What a zone's WAV is named after: the instrument's own name, or the asset's
    /// where the instrument carries none.
    fn instrument_name(&self, id: u64, workspace: &Workspace) -> String {
        let entity = workspace.get(id);
        entity
            .and_then(|e| e.entity.as_ref())
            .and_then(sample::snapshot)
            .and_then(Result::ok)
            .map(|snapshot| snapshot.name)
            .filter(|name| !name.trim().is_empty())
            .or_else(|| entity.map(|e| e.name.clone()))
            .unwrap_or_default()
    }

    /// Build an instrument out of the open WAV. The WAV is left as it is: what comes out
    /// is another asset, not a replacement for the one it was made from.
    fn encode(&mut self, id: u64, workspace: &mut Workspace, log: &mut Log) {
        let Some((draft, source)) = &self.wav else {
            return;
        };
        match encode::instrument(draft, source) {
            Ok(bytes) => {
                let name = format!("{}.{}", draft.name, draft.layout.extension());
                self.error = None;
                workspace.ingest(name, crate::workspace::Origin::Fresh, bytes, log);
            }
            Err(why) => {
                log.error(format!(
                    "encode {}: {why}",
                    workspace.get(id).map_or("", |e| &e.name)
                ));
                self.error = Some(why);
            }
        }
    }

    fn project_body(&mut self, ui: &mut egui::Ui, decoded: &nord_format::Entity, sets: &mut Sets) {
        match project::snapshot(decoded) {
            Some(Ok(snapshot)) => {
                project::ui(ui, &mut self.sample, &snapshot, &mut self.paths, sets)
            }
            Some(Err(why)) => {
                ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
            }
            None => {}
        }
    }

    /// Apply this frame's changes to the working copy.
    ///
    /// All of them or none: a control that owns two fields must not leave one half
    /// written when the library refuses the other.
    fn apply(
        &mut self,
        id: u64,
        sets: Sets,
        workspace: &mut Workspace,
        log: &mut Log,
    ) -> Result<(), String> {
        let Some(entity) = workspace.get(id) else {
            return Ok(());
        };
        let bytes = entity.bytes.clone();
        let decoded = entity.entity.as_ref();
        let result = if decoded.is_some_and(sample::is_sample) {
            sample::apply(&bytes, &sets)
        } else if decoded.is_some_and(project::is_project) {
            project::apply(&bytes, &sets)
        } else if decoded.is_some_and(piano::is_piano) {
            piano::apply(&bytes, &sets)
        } else if decoded.is_some_and(fields::is_set_list) {
            setlist::apply(&bytes, &sets)
        } else {
            fields::apply(&bytes, &sets).map(|(_, out)| out)
        };
        match result {
            Ok(out) if out == bytes => {
                self.error = None;
                Ok(())
            }
            Ok(out) => {
                self.error = None;
                workspace.replace_bytes(id, out, log);
                Ok(())
            }
            Err(why) => {
                log.error(why.clone());
                self.error = Some(why.clone());
                Err(why)
            }
        }
    }
}

/// The faces this document offers, in the order the control shows them.
///
/// Metadata is always one of them — every asset has a record, even bytes that decoded
/// into nothing.
fn faces(entity: &LocalEntity, registry: Option<&[Field]>) -> Vec<Face> {
    let mut faces = Vec::new();
    let friendly = match &entity.entity {
        Some(e) => {
            fields::has_registry(e)
                || fields::is_set_list(e)
                || sample::is_sample(e)
                || project::is_project(e)
        }
        // A WAV decodes into nothing, but it is the one thing this app can make an
        // instrument out of, so it gets a panel rather than only a byte record.
        None => encode::is_wav(&entity.bytes),
    };
    if friendly {
        faces.push(Face::Edit);
    }
    faces.push(Face::Metadata);
    let capabilities = match &entity.entity {
        Some(e) => sample::is_sample(e) || project::is_project(e),
        None => false,
    };
    if registry.is_some() || capabilities {
        faces.push(Face::Advanced);
    }
    faces
}

/// The face to show: the one this document was last left on, where that face still
/// exists — the panel otherwise, and the record where there is no panel either.
fn showing(faces: &[Face], remembered: Face) -> Face {
    match faces.contains(&remembered) {
        true => remembered,
        false => faces.first().copied().unwrap_or(Face::Metadata),
    }
}

/// What the file says about itself, ahead of the container record every asset has.
fn record(ui: &mut egui::Ui, entity: &LocalEntity) {
    let Some(decoded) = &entity.entity else {
        return;
    };
    if let Some(Ok(snapshot)) = sample::snapshot(decoded) {
        sample::metadata(ui, &snapshot);
    } else if let Some(Ok(snapshot)) = project::snapshot(decoded) {
        project::metadata(ui, &snapshot);
    }
}

/// The Advanced face of a body with no field registry: what the format holds, and where
/// each field of it lands.
fn capabilities(ui: &mut egui::Ui, entity: &LocalEntity) {
    let Some(decoded) = &entity.entity else {
        return;
    };
    if let Some(Ok(snapshot)) = sample::snapshot(decoded) {
        capability::table(ui, &sample::capabilities(snapshot.generation));
        capability::offsets(ui, &sample::offsets(&snapshot));
    } else if let Some(Ok(snapshot)) = project::snapshot(decoded) {
        capability::table(ui, &project::capabilities());
        capability::offsets(ui, &project::offsets(&snapshot));
    }
}

/// The strip over a document that is a view of a slot rather than an asset on this
/// computer, and the one way to make it one.
fn viewing_banner(ui: &mut egui::Ui, entity: &LocalEntity) -> bool {
    let Some(where_) = entity
        .origin
        .slot()
        .map(|(class, at)| strings::place(class, at))
    else {
        return false;
    };
    let mut keep = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!("Viewing {where_} on the instrument."))
                    .strong()
                    .small(),
            );
            ui.label(
                egui::RichText::new(
                    "Edits and Send back work from here; it is not on this computer.",
                )
                .small()
                .weak(),
            );
            keep = ui
                .small_button("Keep on this computer")
                .on_hover_text("put it in the list, where it stays after this tab closes")
                .clicked();
        });
    });
    keep
}

/// The loud action a field document offers: the one the header would make anyway, with
/// the pending count on it.
///
/// ⚠️ The count is added only where the header would already say the write can happen.
/// A document with nothing to send to says so, and a number in front of that would read
/// as an offer.
fn queued(entity: &LocalEntity, device: &Device, pending: usize) -> Option<Loud> {
    if pending == 0 {
        return None;
    }
    let loud = header::action(entity, &device.state);
    if loud.tone != Tone::Ready {
        return None;
    }
    Some(Loud {
        label: format!("Queue send · {pending}"),
        short: format!("Send {pending}"),
        hint: format!("{pending} pending sets, applied as one batch — all or none"),
        ..loud
    })
}

/// What is known about the piano a program plays.
///
/// ⚠️ The name can only come from the instrument. A `.ne5p` stores the piano's **id** and
/// no name at all, and the category/model pair beside it is the panel's dial position
/// rather than an identity — so nothing here resolves a name out of the file, and a
/// document opened with no instrument attached shows the id and says so.
fn piano_lookup(
    entity: &LocalEntity,
    registry: Option<&[Field]>,
    device: &Device,
) -> panel::PianoLookup {
    let id = registry
        .and_then(|fields| fields.iter().find(|field| field.path == "piano_panel.id"))
        .and_then(|field| library_id(&field.value))
        // Zero is "this program references no piano", not an id to go looking for.
        .filter(|id| *id != 0);
    let slot = entity.origin.slot();
    let name = id
        .and_then(|id| device.state.dependency_name(slot, ObjectClass::Piano, id))
        .map(str::to_string);
    let models = registry
        .map(|fields| piano_models(fields, device))
        .unwrap_or_default();
    // Dependency ids are authoritative; disagreement means the scanned position mapping
    // is wrong.
    let scan_disagrees = match (&name, registry) {
        (Some(named), Some(fields)) => current_model(fields)
            .and_then(|n| models.iter().find(|(i, _)| *i == n))
            .map(|(_, scanned)| scanned)
            .filter(|scanned| scanned.trim() != named.trim())
            .cloned(),
        _ => None,
    };
    panel::PianoLookup {
        id,
        name,
        can_ask: slot.is_some() && device.state.connected(),
        asked: false,
        models,
        scan_disagrees,
    }
}

/// The Pianos folder's names for the document's current category, by Model dial
/// position — what turns the Model dial into a list of pianos.
///
/// Bank ↔ category and slot order ↔ dial position are confirmed on hardware: the
/// device's own bank list names the piano banks after the panel's categories, in the
/// panel's order, and a program's stored category and model read back as the bank and
/// slot the instrument reports for the piano it depends on. The dependency name stays
/// the standing check — see the mismatch note where this is used.
fn piano_models(fields: &[Field], device: &Device) -> Vec<(u32, String)> {
    let Some(category) = fields
        .iter()
        .find(|field| field.path == "piano_panel.category")
    else {
        return Vec::new();
    };
    // The category's stored bits, recovered from its position in the legal list —
    // `legal_values` walks the bit patterns in stored order.
    let Some(raw) = (category.spec.legal)()
        .iter()
        .position(|value| *value == category.value)
    else {
        return Vec::new();
    };
    let Some(slots) = device.state.bank(ObjectClass::Piano, raw as u32 + 1) else {
        return Vec::new();
    };
    slots
        .iter()
        .enumerate()
        .filter_map(|(position, info)| {
            info.as_ref()
                .map(|piano| (position as u32, piano.name.trim().to_string()))
        })
        .collect()
}

/// The Model dial's current position, as the registry spells it.
fn current_model(fields: &[Field]) -> Option<u32> {
    fields
        .iter()
        .find(|field| field.path == "piano_panel.piano_model")
        .and_then(|field| field.value.trim().parse().ok())
}

/// A library id as the registry spells it — decimal from the field list, hex where a
/// person typed it.
pub(crate) fn library_id(value: &str) -> Option<u32> {
    let text = value.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Fresh, Origin};

    /// One document open in a headless window: everything a frame of it needs, and the
    /// words it painted.
    ///
    /// Nothing checks pixels; what a paint pass catches is the failure a document can
    /// actually have — a section that indexes past its fields, or a control asked for a
    /// value the field cannot hold.
    struct Open {
        ctx: egui::Context,
        workspace: Workspace,
        device: Device,
        log: Log,
        queue: Queue,
        tags: Tags,
        document: Document,
        id: u64,
        /// How wide the window is, which is what the header's collapse is measured on.
        width: f32,
    }

    /// The window a document is painted into, wide enough for [`Stage::Full`].
    const SCREEN: egui::Vec2 = egui::vec2(1280.0, 720.0);

    impl Open {
        fn fresh(kind: Fresh) -> Open {
            let mut open = Open::empty();
            open.id = open
                .workspace
                .create(kind, &mut open.log)
                .expect("a fresh default");
            open
        }

        fn file(name: &str, bytes: Vec<u8>) -> Open {
            let mut open = Open::empty();
            open.id =
                open.workspace
                    .ingest(name.into(), Origin::File(name.into()), bytes, &mut open.log);
            open
        }

        fn empty() -> Open {
            let ctx = egui::Context::default();
            // Both faces, the way the app dresses them: a named text style the header
            // asks for and nothing registered is a panic mid-frame, and so is the
            // semibold family a section heading and a zone row are set in.
            ctx.all_styles_mut(crate::app::metrics);
            ctx.set_fonts(crate::app::fonts());
            Open {
                workspace: Workspace::new(ctx.clone()),
                device: Device::new(ctx.clone()),
                ctx,
                log: Log::default(),
                queue: Queue::default(),
                tags: Tags::default(),
                document: Document::default(),
                id: 0,
                width: SCREEN.x,
            }
        }

        fn entity(&self) -> &LocalEntity {
            self.workspace.get(self.id).expect("it is still open")
        }

        fn set(&mut self, sets: &[(&str, &str)]) {
            let bytes = self.entity().bytes.clone();
            let sets: Vec<(String, String)> = sets
                .iter()
                .map(|(path, value)| ((*path).to_string(), (*value).to_string()))
                .collect();
            let (_, edited) = fields::apply(&bytes, &sets).expect("the sets are legal");
            self.workspace.replace_bytes(self.id, edited, &mut self.log);
        }

        /// One frame, and every word it put on screen.
        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<String> {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.width, SCREEN.y),
                )),
                ..Default::default()
            };
            let output = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    self.document.ui(
                        ui,
                        self.id,
                        &mut self.workspace,
                        &mut self.device,
                        &mut self.log,
                        &Around {
                            queue: &self.queue,
                            tags: &self.tags,
                        },
                    );
                });
            });
            let mut said = Vec::new();
            for clipped in &output.shapes {
                words(&clipped.shape, &mut said);
            }
            said
        }

        /// One frame, and every shape it painted — for what a word says and where.
        fn output(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.width, SCREEN.y),
                )),
                ..Default::default()
            };
            self.ctx.clone().run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    self.document.ui(
                        ui,
                        self.id,
                        &mut self.workspace,
                        &mut self.device,
                        &mut self.log,
                        &Around {
                            queue: &self.queue,
                            tags: &self.tags,
                        },
                    );
                });
            })
        }

        /// Twice: the second pass runs with the caches and the widget state the first
        /// one left behind, which is where a stale index would show up.
        fn twice(&mut self) -> Vec<String> {
            self.frame(Vec::new());
            self.frame(Vec::new())
        }
    }

    fn words(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| words(shape, into)),
            _ => {}
        }
    }

    fn render(sets: &[(&str, &str)], kind: Fresh) {
        render_view(sets, kind, Face::Edit);
    }

    /// Every kind gets the same strip, in both faces of the theme, and it names the
    /// faces in their own words. The old abbreviations are gone from it.
    #[test]
    fn every_kind_wears_the_header_and_names_its_faces() {
        let every = Fresh::FAMILIES.iter().flat_map(|family| family.kinds);
        for kind in every.copied() {
            for dark in [true, false] {
                let mut open = Open::fresh(kind);
                open.ctx.set_theme(match dark {
                    true => egui::ThemePreference::Dark,
                    false => egui::ThemePreference::Light,
                });
                let said = open.twice();
                let has = |word: &str| said.iter().any(|held| held == word);
                assert!(has("Edit"), "{kind:?} in {dark}: {said:?}");
                assert!(has("Metadata"), "{kind:?} in {dark}: {said:?}");
                assert!(has("Queue send"), "the loud action: {said:?}");
                assert!(has("Revert") && has("Export…"), "the quiet ones: {said:?}");
                assert!(
                    !has("Basic") && !has("Meta"),
                    "the faces are called by their own names: {said:?}"
                );
            }
        }
    }

    /// The strip's two groups never run into each other: the controls hold the right
    /// edge, and the words on the left wrap under themselves rather than under them.
    #[test]
    fn the_headers_words_never_run_under_its_controls() {
        let mut open = Open::fresh(Fresh::Program);
        let bytes = open.entity().bytes.clone();
        open.id = open.workspace.ingest(
            "Africa Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
            bytes,
            &mut open.log,
        );
        open.set(&[("center_panel.organ_type", "Vox")]);
        open.width = 720.0;
        open.frame(Vec::new());
        let output = open.output(Vec::new());

        fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    match text.galley.rows.len() {
                        1 => text.galley.text().to_string(),
                        rows => format!("{} (in {rows} rows)", text.galley.text()),
                    },
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
                _ => {}
            }
        }
        let mut words = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut words);
        }
        let header: Vec<&(String, egui::Rect)> =
            words.iter().filter(|(_, rect)| rect.top() < 70.0).collect();
        for (word, _) in &header {
            assert!(
                !word.ends_with(" rows)"),
                "{word} was broken inside itself rather than moved whole"
            );
        }
        // The controls, and the glyphs painted where their art would load. The kind
        // glyph at the far left is the one glyph that is not a control.
        const CONTROLS: [&str; 7] = [
            "Send",
            "Queue send",
            "Edit",
            "Metadata",
            "Advanced",
            "Revert",
            "Export…",
        ];
        type Placed<'a> = Vec<&'a (String, egui::Rect)>;
        let (right, left): (Placed, Placed) = header.iter().partition(|(word, rect)| {
            CONTROLS.contains(&word.as_str()) || (word == "⚠" && rect.left() > 300.0)
        });
        let edge = right
            .iter()
            .map(|(_, rect)| rect.left())
            .fold(f32::MAX, f32::min);
        let row_bottom = right
            .iter()
            .map(|(_, rect)| rect.bottom())
            .fold(f32::MIN, f32::max);
        assert!(
            left.iter().any(|(word, _)| word == "1 pending"),
            "the state phrase is what the left group runs out of room with: {header:?}"
        );
        for (word, rect) in &left {
            if rect.right() > edge {
                assert!(
                    rect.top() >= row_bottom - 1.0,
                    "{word:?} at {rect:?} runs under the controls, whose edge is {edge}"
                );
            }
        }
        assert!(
            left.iter().any(|(_, rect)| rect.top() >= row_bottom - 1.0),
            "at 720 px something has to wrap: {header:?}"
        );
    }

    /// The strip gives up its words in one order as it narrows: the quiet actions
    /// first, then the faces, and the loud action keeps a short label rather than
    /// becoming a bare glyph.
    #[test]
    fn a_narrowing_header_gives_up_its_words_in_one_order() {
        let at = |width: f32| {
            let mut open = Open::fresh(Fresh::Program);
            open.width = width;
            open.twice()
        };

        let full = at(1100.0);
        assert!(full.contains(&"Export…".to_string()), "{full:?}");
        assert!(full.contains(&"Edit".to_string()));
        assert!(full.contains(&"Queue send".to_string()));

        let quiet = at(900.0);
        assert!(!quiet.contains(&"Export…".to_string()), "{quiet:?}");
        assert!(quiet.contains(&"Edit".to_string()), "the faces keep theirs");
        assert!(quiet.contains(&"Queue send".to_string()));

        let faces = at(800.0);
        assert!(!faces.contains(&"Edit".to_string()), "{faces:?}");
        assert!(faces.contains(&"Queue send".to_string()));

        let narrow = at(700.0);
        assert!(!narrow.contains(&"Queue send".to_string()), "{narrow:?}");
        assert!(
            narrow.contains(&"Send".to_string()),
            "the loud action is never a bare glyph: {narrow:?}"
        );
    }

    /// The name box edits the asset's own name where the file stores none, and the
    /// stored name keeps the tag the box never shows.
    #[test]
    fn typing_in_the_name_box_renames_the_asset_and_keeps_its_tag() {
        let mut open = Open::fresh(Fresh::Program);
        assert_eq!(open.entity().name, "untitled.ne5p");
        open.frame(Vec::new());
        open.frame(vec![click(NAME_BOX)]);
        open.frame(vec![egui::Event::Text("X".to_string())]);
        open.frame(vec![enter()]);

        let renamed = open.entity().name.clone();
        assert_ne!(renamed, "untitled.ne5p", "the box was typed into");
        assert!(renamed.ends_with(".ne5p"), "the tag survives: {renamed}");
        assert!(renamed.contains('X'), "what was typed landed: {renamed}");
    }

    /// Where the file stores the name, the box commits through the format rather than
    /// renaming the asset.
    #[test]
    fn typing_in_a_samples_name_box_writes_the_name_the_file_stores() {
        let mut open = Open::file("whatever.nsmp", sample_bytes());
        let stored = |open: &Open| {
            sample::snapshot(open.entity().entity.as_ref().unwrap())
                .unwrap()
                .unwrap()
                .name
        };
        assert_eq!(stored(&open), "Marimba");

        open.frame(Vec::new());
        open.frame(vec![click(NAME_BOX)]);
        open.frame(vec![egui::Event::Text("X".to_string())]);
        open.frame(vec![enter()]);

        assert_ne!(stored(&open), "Marimba", "the instrument was renamed");
        assert!(stored(&open).contains('X'), "{}", stored(&open));
        assert_eq!(
            open.entity().name,
            "whatever.nsmp",
            "the asset's own name is not what that box edits"
        );
    }

    /// A program wears its tags as chips on the identity row, and a program wearing none
    /// has no row for them.
    #[test]
    fn the_identity_row_carries_the_tags_the_asset_wears() {
        let mut open = Open::fresh(Fresh::Program);
        let said = open.twice();
        assert!(
            !said.iter().any(|word| word == "TAGS"),
            "nothing is worn, so there is no cell: {said:?}"
        );

        let friday = open.tags.make("Friday — Blue Room");
        let organ = open.tags.make("Organ-heavy");
        for tag in [friday, organ] {
            open.tags.set(open.id, tag, true);
        }
        let said = open.twice();
        assert!(said.iter().any(|word| word == "TAGS"), "{said:?}");
        for worn in ["Friday — Blue Room", "Organ-heavy"] {
            assert!(said.iter().any(|word| word == worn), "{worn}: {said:?}");
        }
    }

    /// A click inside the name box, which sits after the kind glyph at the left of the
    /// strip.
    const NAME_BOX: egui::Pos2 = egui::pos2(100.0, 19.0);

    fn click(at: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn enter() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn sample_bytes() -> Vec<u8> {
        let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
        let options = nord_format::formats::nsmp::encode::Options::new("Marimba");
        nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    /// The rest of the window, as a document with nothing waiting and nothing labelled
    /// sees it.
    fn alone() -> (Queue, Tags) {
        (Queue::default(), Tags::default())
    }

    fn render_view(sets: &[(&str, &str)], kind: Fresh, face: Face) {
        let mut open = Open::fresh(kind);
        open.document.views.insert(open.id, face);
        if !sets.is_empty() {
            open.set(sets);
        }
        open.twice();
    }

    #[test]
    fn a_program_document_paints_for_every_organ_the_panel_offers() {
        for organ in ["B3", "B3Bass", "Vox", "Farfisa", "Pipe", "unknown (6)"] {
            render(&[("center_panel.organ_type", organ)], Fresh::Program);
        }
    }

    /// A part pointed at each instrument in turn: every section opens at least once.
    #[test]
    fn a_program_document_paints_with_each_part_in_use() {
        for instrument in ["Organ", "Piano", "Sample"] {
            render(&[("center_panel.upper_part", instrument)], Fresh::Program);
        }
    }

    /// An edit lands on the working copy at once, so the strip counts the fields that
    /// differ from the saved bytes rather than saying only that something did.
    #[test]
    fn the_header_counts_the_fields_that_differ_from_the_saved_bytes() {
        let mut open = Open::fresh(Fresh::Program);
        let said = open.twice();
        assert!(
            !said.iter().any(|word| word.ends_with("pending")),
            "nothing has been touched: {said:?}"
        );

        open.set(&[("center_panel.gain", "96")]);
        let said = open.twice();
        assert!(said.iter().any(|word| word == "1 pending"), "{said:?}");
        assert!(
            !said.iter().any(|word| word == "edited"),
            "a field document counts what moved: {said:?}"
        );

        open.set(&[("center_panel.split", "true")]);
        let said = open.twice();
        assert!(said.iter().any(|word| word == "2 pending"), "{said:?}");
    }

    /// The loud action carries the count, but only where the header would already say
    /// the write can happen — a number in front of a dashed action would read as an
    /// offer.
    #[test]
    fn the_loud_action_carries_the_count_where_there_is_somewhere_to_send_it() {
        let mut open = Open::fresh(Fresh::Program);
        let bytes = open.entity().bytes.clone();
        open.id = open.workspace.ingest(
            "Africa Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
            bytes,
            &mut open.log,
        );
        open.set(&[("center_panel.gain", "96")]);

        let unattached = open.twice();
        assert!(
            unattached.iter().any(|word| word == "Queue send"),
            "nothing is attached, so the count is not an offer: {unattached:?}"
        );

        open.device
            .pretend_scanned(ObjectClass::Program, 7, &["", "", "", "Africa Split"]);
        let attached = open.twice();
        assert!(
            attached.iter().any(|word| word == "Queue send · 1"),
            "{attached:?}"
        );
    }

    /// Not relevant means the instrument is not using these controls for the state the
    /// file holds. They are hidden, and the section says so rather than leaving a gap.
    #[test]
    fn a_group_the_instrument_is_not_using_is_named_rather_than_silently_absent() {
        let mut open = Open::fresh(Fresh::Program);
        let said = open.twice();
        let idle: Vec<&String> = said
            .iter()
            .filter(|word| word.contains("stored but not in use"))
            .collect();
        assert!(!idle.is_empty(), "{said:?}");
        assert!(
            idle.iter().any(|line| line.contains("Vox")),
            "a B3 program keeps the other models' registrations: {idle:?}"
        );
        assert!(
            idle.iter().all(|line| line.contains("kept, not cleared")),
            "{idle:?}"
        );
    }

    /// What the Edit face hides is a row in Advanced like any other, counted where the
    /// reader can see how much of the body is not on the other face.
    #[test]
    fn the_fields_the_edit_face_hides_are_rows_under_advanced() {
        let mut open = Open::fresh(Fresh::Program);
        open.document.views.insert(open.id, Face::Advanced);
        let said = open.twice();
        let reading = said
            .iter()
            .find(|word| word.contains("hidden from Edit"))
            .unwrap_or_else(|| panic!("the table counts what Edit does not draw: {said:?}"));
        assert!(!reading.contains("· 0 hidden"), "{reading}");
        assert!(
            said.iter().any(|word| word == "About this file"),
            "{said:?}"
        );
        assert!(said.iter().any(|word| word == "Every field"), "{said:?}");
    }

    /// Both stored registrations stay on screen: the one the instrument plays says so,
    /// and the other is the switch that would bring it back.
    #[test]
    fn both_stored_alternatives_paint_and_the_playing_one_says_so() {
        let mut open = Open::fresh(Fresh::Program);
        let said = open.twice();
        for title in ["B3 · Preset 1", "B3 · Preset 2"] {
            assert!(said.iter().any(|word| word == title), "{title}: {said:?}");
        }
        assert!(said.iter().any(|word| word == "playing"), "{said:?}");
        assert!(said.iter().any(|word| word == "select"), "{said:?}");
        // ⚠️ The head of each card is the switch. The layout keeps the selector in the
        // group above the ones it picks between, and drawn there as well it would be
        // two controls for one preset.
        assert!(
            !said.iter().any(|word| word == "B3 preset"),
            "the selector is drawn once, as the cards' own heads: {said:?}"
        );
    }

    /// The lens swaps every morphed control to what it becomes under one performance
    /// control, and says so above the sections.
    #[test]
    fn the_morph_lens_shows_what_a_control_becomes_under_the_wheel() {
        let mut open = Open::file("blank.ns4y", crate::fields::blank::stage4_synth());
        open.set(&[("synth_a_volume", "40"), ("synth_a_volume_wheel", "211")]);
        let panel = open.twice();
        assert!(
            !panel.iter().any(|word| word == "211"),
            "the panel shows the panel value: {panel:?}"
        );

        open.document.fields.pretend_lens(0);
        let wheel = open.twice();
        assert!(wheel.iter().any(|word| word == "211"), "{wheel:?}");
        assert!(
            wheel
                .iter()
                .any(|word| word.contains("writes the morph slot")),
            "the banner says where an edit lands: {wheel:?}"
        );
    }

    /// A position the library could not name is shown as what it is. Real files hold
    /// them, and that spelling is the only way to write one back.
    #[test]
    fn a_position_the_library_could_not_name_is_shown_rather_than_hidden() {
        let mut open = Open::fresh(Fresh::Program);
        open.set(&[("center_panel.organ_type", "unknown (6)")]);
        let said = open.twice();
        assert!(
            said.iter().any(|word| word == "unrecognized value (6)"),
            "{said:?}"
        );
    }

    /// A path this app has no word for reads as a rough name rather than as a nameless
    /// knob — unpolished has to look unpolished.
    #[test]
    fn a_path_with_no_label_yet_reads_as_its_prettified_self() {
        let said = Open::file("blank.ns4y", crate::fields::blank::stage4_synth()).twice();
        assert!(!strings::known("synth_a_volume"));
        assert!(
            said.iter().any(|word| word == "Synth a volume"),
            "{:?}",
            &said[..said.len().min(40)]
        );
    }

    /// ⚠️ Every section of a Stage program is open, and the nav names each of them. The
    /// old view folded a body this size away behind its headings, which made a control
    /// you cannot see a control you do not know you have.
    #[test]
    fn every_section_of_a_stage_program_is_open_and_named_in_the_nav() {
        let bytes = crate::fields::blank::stage4_program();
        let (registry, _) = fields::apply(&bytes, &[]).unwrap();
        let resolved = nord_format::formats::ns4::program::PANEL.resolve(&registry);
        let titles: Vec<&str> = resolved
            .sections
            .iter()
            .filter(|section| section.relevant)
            .map(|section| section.group.title)
            .collect();
        assert!(titles.len() > 1, "{titles:?}");

        let mut open = Open::file("blank.ns4p", bytes);
        let said = open.twice();
        for title in titles {
            let drawn = said.iter().filter(|word| *word == title).count();
            assert!(
                drawn >= 2,
                "{title} was painted {drawn} times; the nav chip and the heading are two",
            );
        }
    }

    /// The record: the container grid, the byte diff — with something in it and with
    /// nothing — and the folded dump.
    #[test]
    fn the_meta_face_paints() {
        render_view(
            &[("center_panel.gain", "96")],
            Fresh::Program,
            Face::Metadata,
        );
        render_view(&[], Fresh::Settings, Face::Metadata);
    }

    /// The engineer's table paints, filters and holds an edit — for a body with ninety
    /// fields and for one with forty.
    #[test]
    fn the_advanced_table_paints() {
        render_view(&[], Fresh::Program, Face::Advanced);
        render_view(&[], Fresh::Settings, Face::Advanced);
    }

    /// A body with a registry has all three faces; one with a view of its own but no
    /// registry has no field table to offer; bytes with neither have only the record.
    #[test]
    fn the_faces_offered_are_the_ones_the_asset_has() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();

        let offered = |workspace: &Workspace, id: u64| -> Vec<&'static str> {
            let entity = workspace.get(id).unwrap();
            let registry = entity.entity.as_ref().and_then(fields::fields_of);
            faces(entity, registry.as_deref())
                .iter()
                .map(|face| face.label())
                .collect()
        };

        let program = workspace.create(Fresh::Program, &mut log).unwrap();
        assert_eq!(
            offered(&workspace, program),
            ["Edit", "Metadata", "Advanced"]
        );

        let song = workspace.ingest(
            "blank.ne5t".into(),
            Origin::File("blank.ne5t".into()),
            crate::fields::blank::electro5_song(),
            &mut log,
        );
        assert_eq!(offered(&workspace, song), ["Edit", "Metadata"]);

        let stub = workspace.ingest(
            "blank.ns3s".into(),
            Origin::File("blank.ns3s".into()),
            crate::fields::blank::stage3_song(),
            &mut log,
        );
        assert_eq!(offered(&workspace, stub), ["Metadata"]);

        let junk = workspace.ingest(
            "junk.bin".into(),
            Origin::File("junk.bin".into()),
            b"not a nord file".to_vec(),
            &mut log,
        );
        assert_eq!(offered(&workspace, junk), ["Metadata"]);
    }

    /// A document opens on the face it was left on, and on something that has no such
    /// face falls back rather than showing an empty page.
    #[test]
    fn a_document_falls_back_to_a_face_it_actually_has() {
        let all = [Face::Edit, Face::Metadata, Face::Advanced];
        assert_eq!(showing(&all, Face::Metadata), Face::Metadata);
        assert_eq!(showing(&all[..2], Face::Advanced), Face::Edit);

        let record_only = [Face::Metadata];
        for left_on in [Face::Edit, Face::Advanced, Face::Metadata] {
            assert_eq!(showing(&record_only, left_on), Face::Metadata);
        }
        // A set list has no field table: the table's operator lands on the panel.
        let no_table = [Face::Edit, Face::Metadata];
        assert_eq!(showing(&no_table, Face::Advanced), Face::Edit);
    }

    /// A cell the library refuses stays open with what was typed in it, because that is
    /// the only copy of what the operator meant.
    #[test]
    fn a_refused_cell_keeps_its_error() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();

        // What the table does with the library's answer, which is the part worth
        // pinning: the same call the frame makes.
        let before = workspace.get(id).unwrap().bytes.clone();
        let refused = document.apply(
            id,
            vec![("center_panel.gain".into(), "200".into())],
            &mut workspace,
            &mut log,
        );
        assert!(refused.is_err());
        assert!(
            refused.as_ref().unwrap_err().contains("0 .. 127"),
            "the library's own words reach the operator: {refused:?}"
        );
        assert_eq!(
            workspace.get(id).unwrap().bytes,
            before,
            "a refused value leaves the file untouched"
        );
        document.advanced.settled(refused);
        assert!(document.error.is_some());

        let taken = document.apply(
            id,
            vec![("center_panel.gain".into(), "96".into())],
            &mut workspace,
            &mut log,
        );
        assert!(taken.is_ok());
        document.advanced.settled(taken);
        assert!(document.error.is_none());
    }

    /// ⚠️ The strip and the body are two scroll regions in one `Ui`. While they shared
    /// egui's unsalted id they shared one state, and a wheel over the document moved the
    /// tab strip while the body stayed where it was.
    #[test]
    fn the_tab_strip_and_the_document_body_scroll_on_their_own() {
        let ctx = egui::Context::default();
        ctx.style_mut(crate::app::metrics);
        ctx.set_fonts(crate::app::fonts());
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();
        let mut tabs = crate::tabs::Tabs::default();
        let (queue, tags) = alone();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        tabs.open(id);

        let mut ids = None;
        for frame in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                // The first frame lays the document out; the rest wheel over its middle.
                events: match frame {
                    0 => Vec::new(),
                    _ => vec![
                        egui::Event::PointerMoved(egui::pos2(900.0, 400.0)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, -200.0),
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                },
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ids = Some((
                        ui.make_persistent_id(egui::Id::new(crate::tabs::SCROLL)),
                        ui.make_persistent_id(egui::Id::new(PAGE))
                            .with(egui::Id::new(SCROLL)),
                    ));
                    tabs.ui(ui, &workspace, &mut Vec::new());
                    ui.separator();
                    document.ui(
                        ui,
                        id,
                        &mut workspace,
                        &mut device,
                        &mut log,
                        &Around {
                            queue: &queue,
                            tags: &tags,
                        },
                    );
                });
            });
        }

        let (strip, body) = ids.expect("the panel drew");
        assert_ne!(strip, body, "one state each");
        let offset = |id| egui::scroll_area::State::load(&ctx, id).map(|state| state.offset.y);
        assert_eq!(offset(strip), Some(0.0), "the strip has nothing to scroll");
        assert!(
            offset(body).is_some_and(|y| y > 0.0),
            "the body moved: {:?}",
            offset(body)
        );
    }

    /// A program's piano is an id in the file and a name on the instrument. With nothing
    /// attached the document has the id and says as much; it never invents the name.
    #[test]
    fn a_pianos_name_comes_off_the_instrument_or_not_at_all() {
        use nord_usb::{Location, ObjectClass};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let device = Device::new(ctx);
        let mut log = Log::default();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.clone();
        let fields = fields::apply(&bytes, &[]).unwrap().0;
        let local = piano_lookup(workspace.get(id).unwrap(), Some(&fields), &device);
        assert!(local.name.is_none(), "nothing has been asked");
        assert!(!local.can_ask, "and there is nothing to ask");

        let from_device = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
            bytes,
            &mut log,
        );
        let copied = piano_lookup(workspace.get(from_device).unwrap(), Some(&fields), &device);
        assert!(copied.name.is_none(), "still nothing has been asked");
        // A fresh program references no piano at all, and zero is not an id to hunt for.
        assert_eq!(copied.id, None);
    }

    /// The Model dial lists the scanned pianos of the document's category — and only
    /// when the scan can answer. The fallback is the numeric dial, never a guessed name.
    #[test]
    fn the_model_dial_lists_the_scanned_pianos_of_the_current_category() {
        use nord_usb::ObjectClass;

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.clone();
        let fields = fields::apply(&bytes, &[]).unwrap().0;

        // Nothing scanned: the dial stays numeric.
        let unscanned = piano_lookup(workspace.get(id).unwrap(), Some(&fields), &device);
        assert!(unscanned.models.is_empty());

        // A fresh program's category sits at stored position 0, so its bank is 1.
        device.pretend_scanned(ObjectClass::Piano, 1, &["Royal Grand", "", "White Grand"]);
        let scanned = piano_lookup(workspace.get(id).unwrap(), Some(&fields), &device);
        assert_eq!(
            scanned.models,
            vec![
                (0, "Royal Grand".to_string()),
                (2, "White Grand".to_string())
            ],
            "vacant slots are positions with no piano, not renumberings"
        );
        // No dependency name is in hand, so there is nothing to disagree with.
        assert!(scanned.scan_disagrees.is_none());

        // A different bank answers a different category, not this one.
        let mut other = Device::new(egui::Context::default());
        other.pretend_scanned(ObjectClass::Piano, 3, &["Clav D6"]);
        let elsewhere = piano_lookup(workspace.get(id).unwrap(), Some(&fields), &other);
        assert!(elsewhere.models.is_empty());
    }

    /// ⚠️ A cell being typed into belongs to the document it was opened in. One table
    /// serves every tab, and the two programs in front of an operator declare all the
    /// same paths — so a half-typed value has to be dropped at the door rather than
    /// following them into the next tab and landing there on Enter.
    #[test]
    fn a_half_typed_cell_does_not_follow_the_operator_into_the_next_document() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        ctx.set_fonts(crate::app::fonts());
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();

        let (queue, tags) = alone();

        let first = workspace.create(Fresh::Program, &mut log).unwrap();
        let second = workspace.create(Fresh::Program, &mut log).unwrap();

        let mut show = |document: &mut Document, id: u64| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    document.ui(
                        ui,
                        id,
                        &mut workspace,
                        &mut device,
                        &mut log,
                        &Around {
                            queue: &queue,
                            tags: &tags,
                        },
                    );
                });
            });
        };

        show(&mut document, first);
        document.advanced.pretend_editing("center_panel.gain", "0");
        assert_eq!(document.advanced.editing(), Some("center_panel.gain"));

        show(&mut document, second);
        assert_eq!(document.advanced.editing(), None, "left behind");
    }

    /// The registry spells an id in decimal; a person spells it the way `nord deps` does.
    #[test]
    fn a_library_id_reads_in_either_spelling() {
        assert_eq!(library_id("16909060"), Some(0x0102_0304));
        assert_eq!(library_id("0x01020304"), Some(0x0102_0304));
        assert_eq!(library_id(" 0 "), Some(0));
        assert_eq!(library_id("nothing"), None);
    }

    #[test]
    fn the_other_fresh_defaults_paint() {
        render(&[], Fresh::Live);
        render(&[], Fresh::Settings);
    }

    /// Paint a document over bytes the workspace has no fresh default for.
    fn render_file(name: &str, bytes: Vec<u8>, face: Face) {
        let mut open = Open::file(name, bytes);
        open.document.views.insert(open.id, face);
        open.twice();
    }

    /// The Stage bodies have no panel of their own here, so they get the generic one: the
    /// big ones as folds, the small ones open with every control drawn.
    #[test]
    fn a_stage_document_paints_from_the_registry_alone() {
        use crate::fields::blank;
        for (name, bytes) in [
            ("blank.ns2p", blank::stage2_program()),
            ("blank.ns3y", blank::stage3_synth()),
            ("blank.ns4p", blank::stage4_program()),
            ("blank.ns4o", blank::stage4_organ_preset()),
            ("blank.ns4n", blank::stage4_piano_preset()),
            ("blank.ns4y", blank::stage4_synth()),
        ] {
            render_file(name, bytes.clone(), Face::Edit);
            render_file(name, bytes, Face::Advanced);
        }
    }

    /// An Electro 5 set list has a Basic view of its own — the four slots — and it does
    /// not come from the registry, which lists nothing for that body.
    #[test]
    fn a_set_list_has_its_own_view() {
        let bytes = crate::fields::blank::electro5_song();
        let song = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert!(fields::is_set_list(&song));
        assert!(!fields::has_registry(&song));
        render_file("blank.ne5t", bytes, Face::Edit);
    }

    /// ⚠️ A song that decodes no further than its container has no Basic view to offer,
    /// and must not be given one: an empty page saying nothing is editable stands in
    /// front of the byte record, which is everything that file has.
    #[test]
    fn an_undecoded_song_keeps_the_record_it_has() {
        let bytes = crate::fields::blank::stage3_song();
        let song = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert!(!fields::is_set_list(&song));
        assert!(!fields::has_registry(&song));
        render_file("blank.ns3s", bytes, Face::Metadata);
    }

    /// Bytes that do not decode still have a document — it says so and shows the record.
    #[test]
    fn a_file_that_did_not_decode_still_paints() {
        let said = Open::file("junk.bin", b"not a nord file".to_vec()).twice();
        assert!(
            said.iter().any(|word| word.contains("did not decode")),
            "the record is all these bytes have: {said:?}"
        );
    }

    /// One second of 44.1 kHz mono — long enough for the encoder's shortest stroke.
    fn wav_bytes() -> Vec<u8> {
        let samples: Vec<i16> = (0..codec::SOURCE_RATE as usize)
            .map(|i| ((i as f64 / 40.0).sin() * 12_000.0) as i16)
            .collect();
        nord_format::wav::mono_pcm16(&samples, codec::SOURCE_RATE).unwrap()
    }

    #[test]
    fn a_wav_offers_an_encode_and_leaves_itself_alone() {
        let bytes = wav_bytes();
        let mut open = Open::file("Marimba hit.wav", bytes.clone());
        assert_eq!(
            faces(open.entity(), None)
                .iter()
                .map(|face| face.label())
                .collect::<Vec<_>>(),
            ["Edit", "Metadata"],
        );

        open.frame(Vec::new());
        let Open {
            mut document,
            mut workspace,
            mut log,
            id,
            ..
        } = open;
        document.answer(id, Asked::Encode, &mut workspace, &mut log);

        assert!(document.error.is_none(), "{:?}", document.error);
        assert_eq!(workspace.get(id).unwrap().bytes, bytes, "the WAV is intact");
        let made = workspace
            .entities()
            .iter()
            .find(|e| e.id != id)
            .expect("an instrument was added");
        assert_eq!(made.name, "Marimba hit.nsmp");
        let snapshot = sample::snapshot(made.entity.as_ref().expect("it decoded"))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.name, "Marimba hit");
        assert_eq!(snapshot.zones.len(), 1);
    }

    #[test]
    fn a_zone_decodes_on_request_and_exports_under_the_instruments_name() {
        let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
        let options = nord_format::formats::nsmp::encode::Options::new("Marimba").root_key(48);
        let bytes = nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap();
        let mut open = Open::file("whatever-it-was-called.nsmp", bytes);
        let id = open.id;
        open.document.target = Some(id);
        let stamp = open.entity().stamp;
        open.document.audio.follow(id, stamp);

        assert!(
            open.document.audio.get(0).is_none(),
            "nothing decodes unasked"
        );
        open.document.answer(
            id,
            Asked::Zone(sample::Ask::Decode(0)),
            &mut open.workspace,
            &mut open.log,
        );
        {
            let decoded = open.document.audio.get(0).expect("asked for");
            let decoded = decoded.as_ref().unwrap();
            assert!(decoded.audio.seconds() > 0.5);
            assert_eq!(decoded.audio.channels, 1);
            assert!(!decoded.envelope.is_empty());
        }
        // With a zone open the document paints its envelope, which nothing else does.
        open.frame(Vec::new());

        assert_eq!(
            open.document.instrument_name(id, &open.workspace),
            "Marimba"
        );
        assert_eq!(
            crate::workspace::zone_wav_name(&open.document.instrument_name(id, &open.workspace), 1),
            "Marimba-zone1.wav",
        );

        // Editing the instrument replaces its bytes, so what was decoded from the old
        // ones is dropped rather than kept beside a file it no longer describes.
        let edited =
            sample::apply(&open.entity().bytes, &[("name".into(), "Vibes".into())]).unwrap();
        open.workspace.replace_bytes(id, edited, &mut open.log);
        let stamp = open.entity().stamp;
        open.document.audio.follow(id, stamp);
        assert!(
            open.document.audio.get(0).is_none(),
            "decoded from stale bytes"
        );
        assert_eq!(open.document.instrument_name(id, &open.workspace), "Vibes");
    }

    /// A three-zone project, as the format's own fixtures hold one.
    ///
    /// Committed in `nord-format`, written by this project's tools: the editor's own
    /// output has no fixture that may be redistributed.
    fn project_bytes() -> Vec<u8> {
        include_bytes!("../../../nord-format/tests/fixtures/nsmpproj/three-zones.nsmpproj").to_vec()
    }

    /// An instrument and the project it is built from each have all three faces: the
    /// panel, the record, and the capability table that says what the format holds.
    #[test]
    fn an_instrument_and_a_project_offer_all_three_faces() {
        for (name, bytes) in [
            ("Marimba.nsmp", sample_bytes()),
            ("clarinet.nsmpproj", project_bytes()),
        ] {
            let open = Open::file(name, bytes);
            let entity = open.entity();
            let registry = entity.entity.as_ref().and_then(fields::fields_of);
            assert!(registry.is_none(), "{name} declares no field registry");
            assert_eq!(
                faces(entity, registry.as_deref())
                    .iter()
                    .map(|face| face.label())
                    .collect::<Vec<_>>(),
                ["Edit", "Metadata", "Advanced"],
                "{name}",
            );
        }
    }

    /// Every face of an instrument and of a project paints, twice over.
    #[test]
    fn a_project_document_paints_on_every_face() {
        for face in [Face::Edit, Face::Metadata, Face::Advanced] {
            render_file("clarinet.nsmpproj", project_bytes(), face);
            render_file("Marimba.nsmp", sample_bytes(), face);
        }
    }

    /// ⚠️ The key map is pinned: it is painted above the region the rows scroll in, so
    /// it stays put while they move. Inside the scroll area it would scroll away, and
    /// the map is how a zone is picked.
    #[test]
    fn the_key_map_is_painted_above_the_scrolling_rows() {
        let mut open = Open::file("Marimba.nsmp", sample_bytes());
        open.frame(Vec::new());
        let output = open.output(Vec::new());

        let placed = |word: &str| -> (egui::Rect, egui::Rect) {
            fn walk(
                shape: &egui::Shape,
                clip: egui::Rect,
                word: &str,
                into: &mut Vec<(egui::Rect, egui::Rect)>,
            ) {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == word => into.push((
                        egui::Rect::from_min_size(text.pos, text.galley.size()),
                        clip,
                    )),
                    egui::Shape::Vec(shapes) => shapes
                        .iter()
                        .for_each(|shape| walk(shape, clip, word, into)),
                    _ => {}
                }
            }
            let mut found = Vec::new();
            for clipped in &output.shapes {
                walk(&clipped.shape, clipped.clip_rect, word, &mut found);
            }
            *found
                .first()
                .unwrap_or_else(|| panic!("{word} was never painted"))
        };

        let (map, pinned) = placed("Key map");
        let (_, scrolling) = placed("Zones");
        assert_ne!(pinned, scrolling, "two regions, not one");
        assert!(
            map.bottom() <= scrolling.top(),
            "the map at {map:?} is inside the rows' own region {scrolling:?}",
        );
    }

    /// ⚠️ A zone index belongs to the instrument it was opened on. Leaving the tab
    /// drops the selection, the open rows and the struck key — and nothing else: an
    /// edit is on the working copy already.
    #[test]
    fn leaving_a_document_forgets_the_open_zone_and_keeps_the_edit() {
        let mut open = Open::file("Marimba.nsmp", sample_bytes());
        open.frame(Vec::new());
        sample::pick_row(&mut open.document.sample, 0);
        assert_eq!(sample::selected(&open.document.sample), Some(0));

        let edited = sample::apply(
            &open.entity().bytes,
            &[("zone1.top_note".into(), "C6".into())],
        )
        .unwrap();
        open.workspace.replace_bytes(open.id, edited, &mut open.log);

        // Another document, and back: the same frame the tab strip's own switch makes.
        let elsewhere = open.workspace.ingest(
            "other.nsmp".into(),
            Origin::File("other.nsmp".into()),
            sample_bytes(),
            &mut open.log,
        );
        let id = open.id;
        open.id = elsewhere;
        open.frame(Vec::new());
        open.id = id;
        open.frame(Vec::new());

        assert_eq!(
            sample::selected(&open.document.sample),
            None,
            "the selection is the instrument's, not the editor's"
        );
        let snapshot = sample::snapshot(open.entity().entity.as_ref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.zones[0].top_note, 84, "the edit stands");
    }

    #[test]
    fn a_sample_document_paints() {
        let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
        let options = nord_format::formats::nsmp::encode::Options::new("Marimba");
        let bytes = nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap();
        render_file("Marimba.nsmp", bytes.clone(), Face::Edit);
        render_file("Marimba.nsmp", bytes, Face::Metadata);
        render_file("Marimba hit.wav", wav_bytes(), Face::Edit);
    }
}
