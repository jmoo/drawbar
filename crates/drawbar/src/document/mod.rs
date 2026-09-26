//! The document: one view of one asset.
//!
//! An edit lands on the tab's working copy as soon as it is made: the field is set, the
//! body re-encoded and the bytes re-checked, and the asset shows as unsaved. Nothing on
//! the instrument changes until the header's Send. Revert restores the bytes the asset was
//! last saved as; there is no other undo.

use eframe::egui;
use nord_format::fields::Field;
use nord_format::formats::nsmp::codec;
use nord_usb::{Location, ObjectClass};

use crate::device::Device;
use crate::fields;
use crate::log::Log;
use crate::midi::Played;
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
mod table;
pub(crate) mod text;
mod verbatim;

use advanced::Advanced;
use controls::{Ctx, Sets};

pub use header::{Body, Cell, Extras, Face, Ink, Loud, SizeLine, Stage, StateLine, Tone};
pub use sample::note_picker;

/// The body's scroll id. See [`crate::tabs::SCROLL`].
pub const SCROLL: &str = "document_body";

/// The id the body's `Ui` is salted with, so the scroll region inside it keeps an id that
/// does not depend on how many widgets were drawn before it.
pub const PAGE: &str = "document_page";

/// The body's margin inside the central panel. The header is full bleed and has none.
const BODY_MARGIN: f32 = 8.0;

/// The kind of document, which decides its body, the faces it offers, its header, and
/// what an edit to it produces.
///
/// ⚠️ Decided once per frame by an exhaustive match on [`nord_format::Entity`], so a
/// family the library adds fails to compile here until it is given a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    /// A body whose fields the generated registry declares.
    Fields,
    /// An Electro 5 set list, which holds only references to four programs.
    SetList,
    Sample,
    Project,
    /// An `npno` piano library. Edits are kept as a plan so the library's bytes are not
    /// copied for each edit.
    Piano,
    /// Undecoded bytes that are text, edited as text.
    Text,
    /// A body no registry describes, kept byte for byte.
    Verbatim,
    /// Undecoded bytes that are a WAV file, which can be encoded into an instrument.
    Wav,
    /// Bytes that did not decode.
    Undecoded,
}

/// One asset and its shape, kept together so that no frame draws one asset's body with
/// another's shape.
#[derive(Clone, Copy)]
struct Asset<'a> {
    entity: &'a LocalEntity,
    shape: Shape,
}

impl<'a> Asset<'a> {
    fn of(entity: &'a LocalEntity) -> Asset<'a> {
        Asset {
            entity,
            shape: shape(entity),
        }
    }

    fn decoded(&self) -> Option<&'a nord_format::Entity> {
        self.entity.entity.as_ref()
    }
}

fn shape(entity: &LocalEntity) -> Shape {
    use nord_format::Entity as E;

    let Some(decoded) = &entity.entity else {
        // ⚠️ Checked before `is_text`, so a WAV always opens in the encode panel and
        // never as text.
        if encode::is_wav(&entity.bytes) {
            return Shape::Wav;
        }
        return match entity.is_text {
            true => Shape::Text,
            false => Shape::Undecoded,
        };
    };
    match decoded {
        E::Sample(_) => Shape::Sample,
        E::SampleProject(_) => Shape::Project,
        E::Piano(_) => Shape::Piano,
        E::Song(_) => match fields::is_set_list(decoded) {
            true => Shape::SetList,
            false => Shape::Verbatim,
        },
        // ⚠️ A Stage Classic piano library is not a `Shape::Piano`. Only `npno` decodes
        // into strokes; the others are containers over a body this app keeps unchanged.
        E::Bundle(_)
        | E::Cne3(_)
        | E::Live(_)
        | E::Midi(_)
        | E::OrganPreset(_)
        | E::Performance(_)
        | E::PianoLibrary(_)
        | E::PianoPreset(_)
        | E::PipeLibrary(_)
        | E::Program(_)
        | E::Settings(_)
        | E::Synth(_)
        | E::Sysex(_) => match fields::has_registry(decoded) {
            true => Shape::Fields,
            false => Shape::Verbatim,
        },
    }
}

/// A write to the instrument that the header asked for. The browser asks any question
/// the write needs first.
pub struct SendBack {
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
}

/// The window state a document reads: the send queue, this computer's tags, and what a
/// MIDI controller played since the last frame.
pub struct Around<'a> {
    pub queue: &'a Queue,
    pub tags: &'a Tags,
    /// Keys played here strike the key map when the current face shows one.
    pub played: &'a Played,
}

/// What a document's frame asked the app for.
#[derive(Default)]
pub struct Wants {
    /// The write the header queued.
    pub send: Option<SendBack>,
    /// The banner's offer to put a view of a slot on this computer.
    pub keep: bool,
    /// An item a body asked to open, such as the program a set list entry points at.
    /// The browser opens it.
    pub open: Option<crate::browser::Item>,
}

/// Requests from the Basic face that cannot run while the asset is borrowed for drawing:
/// audio, or a new asset made from this one.
enum Asked {
    Zone(sample::Ask),
    Root(piano::Ask),
    Encode,
    /// The header's Export, offered by a body with nothing to edit.
    Export,
    Open(crate::browser::Item),
    /// The Advanced link under a section the instrument is not using.
    Advanced,
}

/// The state one open document keeps between frames.
///
/// ⚠️ Switching to another asset replaces all of it. Clearing fields one by one on a
/// switch would eventually miss one, and a half-typed value would follow the user into
/// the next tab.
struct Opened {
    /// The asset this is open on.
    id: u64,
    /// Per-field legal values and controls, cached as they are drawn. See [`Ctx`].
    ctx: Ctx,
    /// The header's name box, so a half-typed name survives a frame, and the piano's
    /// variant box beside it.
    name: String,
    variant: String,
    /// The path boxes for a project's audio files, by file id, kept for the same reason.
    paths: std::collections::HashMap<u32, String>,
    /// The last refusal message.
    error: Option<String>,
    /// The encode panel over a WAV, and the read of the WAV it works from.
    wav: Option<(encode::Draft, encode::Source)>,
    /// What the instrument editor keeps between frames: the open zone, the struck key,
    /// the folded key table. Never an edit: edits go to the working copy immediately.
    sample: sample::State,
    /// What the field document keeps between frames: the morph lens, where the reader
    /// is, and the two decodes a pending count is measured across. Never an edit.
    fields: field::State,
    /// What the set list editor keeps: the half-typed address boxes and whether a
    /// reorder has been made.
    list: setlist::State,
    /// What the note editor keeps between frames: the words in its box.
    text: text::State,
}

impl Opened {
    /// Open a document on `asset`.
    ///
    /// ⚠️ Reading a WAV copies every sample, so it happens once here and never per
    /// frame.
    fn new(asset: Asset<'_>, view: bool, renaming: (Option<String>, Option<String>)) -> Opened {
        let entity = asset.entity;
        let (name, variant) = header::boxes(entity, asset.shape, view, renaming);
        Opened {
            id: entity.id,
            ctx: Ctx::default(),
            name,
            variant,
            paths: std::collections::HashMap::new(),
            error: None,
            wav: match asset.shape {
                Shape::Wav => Some((
                    encode::Draft::new(&entity.name),
                    encode::Source::read(&entity.bytes),
                )),
                Shape::Fields
                | Shape::SetList
                | Shape::Sample
                | Shape::Project
                | Shape::Piano
                | Shape::Text
                | Shape::Verbatim
                | Shape::Undecoded => None,
            },
            sample: sample::State::default(),
            fields: field::State::default(),
            list: setlist::State::default(),
            text: text::State::default(),
        }
    }
}

#[derive(Default)]
pub struct Document {
    /// The open document's state, including which asset it is on.
    open: Option<Opened>,
    /// Which face each document was left on.
    views: std::collections::HashMap<u64, Face>,
    /// The Advanced table's filter, selected cell, and last byte diff. One table serves
    /// every tab; see [`Advanced::leave`].
    advanced: Advanced,
    /// Decoded audio for zones shown in open rows, dropped when their strokes change.
    audio: sample::Cache,
    /// The sounding zones, and the audio backend that plays them.
    player: crate::audio::Player,
    /// The piano library's plan, the facts it is based on, and its decoded strokes.
    ///
    /// ⚠️ Not tied to one document. An apply runs on its own thread, and the acts it
    /// holds may come back after the tab that started it has closed. See
    /// [`Document::settle`].
    piano: piano::State,
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
        let played = around.played;
        let Some(entity) = workspace.get(id) else {
            return Wants::default();
        };
        let stamp = entity.stamp;
        let decoded = entity.entity.as_ref();
        let registry = decoded.map(fields::fields_of).unwrap_or_default();
        let viewing = workspace.is_view(id);
        let asset = Asset::of(entity);
        let shape = asset.shape;

        if self.opened() != Some(id) {
            self.open = Some(Opened::new(
                asset,
                viewing,
                self.piano.renaming(asset.entity),
            ));
            self.advanced.leave();
            // ⚠️ Leaving the tab stops playback: a zone still playing over another
            // document has no control on screen to stop it.
            self.player.stop();
            self.piano.leave();
        }
        self.audio.follow(id, entity.stamp);
        // Paint marks are measured against the bytes the asset was last saved as.
        if let Some(open) = &mut self.open {
            sample::follow(&mut open.sample, id, &entity.saved);
        }
        // A release is heard whatever face is showing: the key it lets go of may have
        // been struck on another.
        for key in &played.released {
            self.player.release(*key);
        }
        self.player.settle();
        if self.player.sounding().next().is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        // An apply reports progress from another thread. Without repaints the header
        // freezes on `applying…` and the window looks hung.
        if self.piano.applying() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }

        let faces = faces(shape);
        let face = showing(&faces, self.views.get(&id).copied().unwrap_or_default());

        // ⚠️ Only a registry body. Reading the saved bytes means decoding them, and a
        // piano library is hundreds of megabytes with no fields.
        if let (Shape::Fields, Some(open)) = (shape, self.open.as_mut()) {
            open.fields.follow(entity);
        }
        let doc = match (decoded, registry.as_deref()) {
            (Some(decoded), Some(fields)) => Some(field::of(decoded, fields)),
            _ => None,
        };
        let pending = match (doc.is_some(), self.open.as_ref()) {
            (true, Some(open)) => open.fields.pending().len(),
            _ => 0,
        };
        let extras = match shape {
            Shape::Piano => self.piano.begin(id, entity, &device.state),
            Shape::Fields
            | Shape::SetList
            | Shape::Sample
            | Shape::Project
            | Shape::Text
            | Shape::Verbatim
            | Shape::Wav
            | Shape::Undecoded => extras(asset, device, workspace, pending),
        };

        let Some(open) = self.open.as_mut() else {
            return Wants::default();
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
                renaming: self.piano.renaming(entity),
                shape,
                extras,
            },
            (&mut open.name, &mut open.variant),
            &mut sets,
        );
        self.views.insert(id, act.face.unwrap_or(face));

        let mut wants = Wants {
            send: act.send,
            ..Wants::default()
        };
        let mut details = None;
        let mut typed = false;
        let mut asked = Vec::new();
        let mut lookup = piano_lookup(entity, registry.as_deref(), device);
        // A child `Ui` with a salted id keeps the body's scroll state under one id,
        // whatever the header drew above it. See [`PAGE`].
        let mut page = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(PAGE)
                .max_rect(ui.available_rect_before_wrap().shrink(BODY_MARGIN)),
        );
        {
            let ui = &mut page;
            // ⚠️ A view's tab looks like a local document; the banner is the only
            // visible sign that its bytes still belong to the instrument.
            if viewing {
                wants.keep = viewing_banner(ui, entity);
            }
            if let Some(why) = self.open.as_ref().and_then(|open| open.error.as_ref()) {
                ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
            }
            if face == Face::Basic {
                asked = self.pinned(ui, asset, doc.as_ref(), &mut sets, played);
            }
            egui::ScrollArea::vertical()
                .id_salt(SCROLL)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    // ⚠️ Widget state keyed only by field path leaks between tabs of
                    // the same format, so every control also answers to the document id.
                    ui.push_id(id, |ui| match face {
                        Face::Basic => {
                            let from_body = self.body(
                                ui,
                                asset,
                                doc.as_ref(),
                                &mut lookup,
                                &setlist::Catalog {
                                    device: &device.state,
                                    workspace,
                                },
                                &mut sets,
                            );
                            asked.extend(from_body);
                        }
                        // What the file states about itself, the record of its bytes,
                        // then the body, which is the longest and so goes last.
                        Face::Advanced => {
                            self.states(ui, asset, doc.as_ref());
                            details = self.advanced.meta(ui, entity, device);
                            typed = self.deep(
                                ui,
                                asset,
                                doc.as_ref(),
                                registry.as_deref().unwrap_or_default(),
                                &mut sets,
                            );
                        }
                    });
                });
        }
        let drawn = page.min_rect();
        ui.advance_cursor_after_rect(drawn.expand(BODY_MARGIN));

        if let Some(details) = details {
            for cmd in advanced::commands(details) {
                device.send(cmd, log);
            }
        }
        for asked in asked {
            match asked {
                Asked::Open(item) => wants.open = Some(item),
                Asked::Advanced => {
                    self.views.insert(id, Face::Advanced);
                }
                Asked::Export => self.export(ui.ctx(), id, workspace),
                asked => self.answer(id, asked, workspace, log),
            }
        }
        if let Some((class, at)) = workspace.get(id).and_then(|e| e.origin.slot()) {
            match lookup.asked {
                true => device.ask_deps_again(class, at, log),
                false if lookup.wants_a_name() => device.read_deps(class, at, log),
                false => {}
            }
        }
        if let Some(name) = act.rename {
            // ⚠️ Rename only. The box already holds what was typed, and reopening the
            // document would discard everything else it keeps, including a WAV's encode
            // draft, which is stored nowhere else.
            workspace.rename(id, name);
        }
        if act.export {
            self.export(ui.ctx(), id, workspace);
        }
        if act.revert {
            workspace.revert(id, log);
            self.piano.forget(id);
            // Reopened on the next frame, because every box holds a reverted edit.
            self.open = None;
            return wants;
        }
        if !sets.is_empty() {
            let outcome = self.apply(id, sets, workspace, log);
            if typed {
                // The table keeps a refused cell open with what was typed in it.
                self.advanced.settled(outcome);
            }
        }
        self.replan(id, workspace, log);
        // The strip was drawn before this frame's edit reached the document; one more
        // frame shows the result of the edit or of the plan it left.
        let edited = self.note_pending(id, workspace)
            || workspace.get(id).is_some_and(|held| held.stamp != stamp);
        if edited {
            ui.ctx().request_repaint();
        }
        wants
    }

    /// Commit the plan a piano library's frame drafted, if the library accepts it.
    ///
    /// ⚠️ Nothing is rebuilt here. The plan is only checked against the saved bytes, so a
    /// name the format refuses, or a switch that would leave no strokes, is refused now
    /// and cannot block every later edit. The bytes are laid out only when something
    /// needs them; see [`piano::State::start`].
    fn replan(&mut self, id: u64, workspace: &Workspace, log: &mut Log) {
        let Some(plan) = self.piano.drafted() else {
            return;
        };
        let checked = match workspace.get(id) {
            Some(entity) => piano::planned(&entity.saved.bytes, &plan).map(|_| ()),
            None => return,
        };
        match checked {
            Ok(()) => {
                self.piano.commit(plan);
                self.refused(None);
            }
            Err(why) => {
                self.piano.discard();
                log.error(why.clone());
                self.refused(Some(why));
            }
        }
    }

    /// Whether this document holds an edit its bytes do not.
    fn pends(&self, id: u64) -> bool {
        self.piano.pending(id)
    }

    /// Tell the workspace whether this document holds a pending plan, and return whether
    /// that changed.
    ///
    /// The plan lives here, but the unsaved state is read from the asset; see
    /// [`LocalEntity::is_unsaved`]. Call this wherever a plan is committed or laid out.
    fn note_pending(&self, id: u64, workspace: &mut Workspace) -> bool {
        workspace.mark_pending(id, self.pends(id))
    }

    /// Hold back acts that would carry a piano library's bytes while its plan is not yet
    /// applied, and start the apply they wait for.
    ///
    /// ⚠️ Call after the frame's acts are collected and before any of them run. An act
    /// let through here writes the bytes as they stand, which for a pending plan is the
    /// library before the trim.
    ///
    /// It polls before returning, because on a single-threaded target the apply runs
    /// where it starts, and the acts it held come back with this frame's acts.
    pub fn settle(
        &mut self,
        ctx: &egui::Context,
        acts: Vec<crate::browser::Act>,
        workspace: &mut Workspace,
        log: &mut Log,
    ) -> Vec<crate::browser::Act> {
        let mut out: Vec<crate::browser::Act> = acts
            .into_iter()
            .filter_map(|act| {
                // Whatever the gesture's source, the saved bytes are about to be
                // restored or removed, and a plan over them would edit nothing.
                if let crate::browser::Act::Revert(id) | crate::browser::Act::Remove(id) = &act {
                    self.piano.forget(*id);
                }
                self.piano.hold(ctx, act, workspace)
            })
            .collect();
        out.extend(self.released(ctx, workspace, log));
        out
    }

    /// Release the acts a finished apply was holding, and put the bytes it made into
    /// the document.
    pub fn released(
        &mut self,
        ctx: &egui::Context,
        workspace: &mut Workspace,
        log: &mut Log,
    ) -> Vec<crate::browser::Act> {
        match self.piano.answered(ctx, workspace) {
            Some(applied) => self.put_back(applied, workspace, log),
            None => Vec::new(),
        }
    }

    fn put_back(
        &mut self,
        applied: piano::Applied,
        workspace: &mut Workspace,
        log: &mut Log,
    ) -> Vec<crate::browser::Act> {
        match applied.made {
            Some(Ok(bytes)) => {
                self.refused(None);
                workspace.replace_bytes(applied.id, bytes, log);
                self.note_pending(applied.id, workspace);
            }
            Some(Err(why)) => {
                log.error(why.clone());
                log.trouble("That library could not be laid out, so nothing was written.");
                self.refused(Some(why));
            }
            None => {}
        }
        applied.acts
    }

    /// Export the document's bytes once any pending plan has been applied to them.
    fn export(&mut self, ctx: &egui::Context, id: u64, workspace: &mut Workspace) {
        if self
            .piano
            .hold(ctx, crate::browser::Act::Export(id), workspace)
            .is_some()
        {
            workspace.export(id);
        }
    }

    /// The asset the open document shows.
    fn opened(&self) -> Option<u64> {
        self.open.as_ref().map(|open| open.id)
    }

    /// The refusal the open document is showing.
    #[cfg(test)]
    fn refusal(&self) -> Option<&str> {
        self.open.as_ref()?.error.as_deref()
    }

    /// Set or clear the refusal shown on the open document.
    ///
    /// The message belongs to the open document and closes with it. With nothing open,
    /// it is only in the log.
    fn refused(&mut self, why: Option<String>) {
        if let Some(open) = &mut self.open {
            open.error = why;
        }
    }

    /// The sounding roots that belong to this document.
    fn sounding_roots(&self) -> Vec<u8> {
        let opened = self.opened();
        self.player
            .sounding()
            .filter(|(id, _)| Some(*id) == opened)
            .filter_map(|(_, root)| u8::try_from(root).ok())
            .collect()
    }

    /// Close the document.
    ///
    /// ⚠️ A zone keeps sounding until something stops it, and the stop control is on the
    /// document. With no document there is nothing to click.
    pub fn leave(&mut self) {
        self.open = None;
        self.player.stop();
    }

    /// The body this shape draws.
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        asset: Asset<'_>,
        doc: Option<&field::Doc<'_>>,
        piano: &mut panel::PianoLookup,
        seen: &setlist::Catalog<'_>,
        sets: &mut Sets,
    ) -> Option<Asked> {
        match asset.shape {
            Shape::Wav | Shape::Undecoded => self.wav_body(ui),
            Shape::Sample => self
                .sample_body(ui, asset.decoded()?, sets)
                .map(Asked::Zone),
            Shape::Project => {
                self.project_body(ui, asset.decoded()?, sets);
                None
            }
            Shape::SetList => {
                let open = self.open.as_mut()?;
                setlist::ui(ui, &mut open.list, asset.entity, seen, sets).map(Asked::Open)
            }
            Shape::Piano => {
                let sounding = self.sounding_roots();
                self.piano.ui(ui, &sounding).map(Asked::Root)
            }
            Shape::Fields => {
                let open = self.open.as_mut()?;
                field::body(ui, &open.ctx, &mut open.fields, doc?, piano, sets)
                    .then_some(Asked::Advanced)
            }
            Shape::Text => {
                let open = self.open.as_mut()?;
                open.text.ui(ui, asset.entity, sets);
                None
            }
            Shape::Verbatim => verbatim::ui(ui, asset.entity).then_some(Asked::Export),
        }
    }

    /// Undecoded bytes: the encode panel for a WAV, and a plain notice for anything
    /// else.
    fn wav_body(&mut self, ui: &mut egui::Ui) -> Option<Asked> {
        let Some((draft, source)) = self.open.as_mut().and_then(|open| open.wav.as_mut()) else {
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
        let target = self.opened();
        let sounds: Vec<sample::Sound> = (0..snapshot.zones.len())
            .map(|index| sample::Sound {
                decoded: self.audio.get(index),
                playing: target.is_some_and(|id| self.player.sounds((id, index))),
            })
            .collect();
        let open = self.open.as_mut()?;
        sample::ui(ui, &mut open.sample, &snapshot, &sounds, sets)
    }

    /// What the file states about itself, shown at the top of the Advanced face.
    fn states(&mut self, ui: &mut egui::Ui, asset: Asset<'_>, doc: Option<&field::Doc<'_>>) {
        match asset.shape {
            Shape::Fields => {
                if let Some(doc) = doc {
                    Advanced::about(ui, &field::about(doc, asset.entity));
                }
            }
            Shape::Piano => self.piano.meta(ui),
            Shape::SetList
            | Shape::Sample
            | Shape::Project
            | Shape::Verbatim
            | Shape::Text
            | Shape::Wav
            | Shape::Undecoded => record(ui, asset),
        }
    }

    /// The body, under the record: the field table where a registry describes the bytes,
    /// and the format's capabilities where none does. Returns whether the table made an
    /// edit this frame.
    fn deep(
        &mut self,
        ui: &mut egui::Ui,
        asset: Asset<'_>,
        doc: Option<&field::Doc<'_>>,
        registry: &[Field],
        sets: &mut Sets,
    ) -> bool {
        match asset.shape {
            Shape::Fields => {
                let (Some(doc), Some(open)) = (doc, self.open.as_ref()) else {
                    return false;
                };
                let table = advanced::Table {
                    fields: registry,
                    saved: open.fields.settled(),
                    changed: open.fields.pending(),
                    doc: Some(doc),
                };
                self.advanced.table(ui, &table, sets);
                !sets.is_empty()
            }
            Shape::Piano => {
                self.piano.advanced(ui);
                false
            }
            Shape::SetList
            | Shape::Sample
            | Shape::Project
            | Shape::Text
            | Shape::Verbatim
            | Shape::Wav
            | Shape::Undecoded => {
                capabilities(ui, asset);
                false
            }
        }
    }

    /// What an editor pins above the scroll region, on the panel fill, so it stays put
    /// while the rows under it scroll.
    ///
    /// It holds the instrument key map, or a field document's navigation. A shape with
    /// nothing to pin takes no room.
    fn pinned(
        &mut self,
        ui: &mut egui::Ui,
        asset: Asset<'_>,
        doc: Option<&field::Doc<'_>>,
        sets: &mut Sets,
        played: &Played,
    ) -> Vec<Asked> {
        if asset.shape == Shape::Piano {
            return self
                .piano
                .map(ui, played)
                .into_iter()
                .map(Asked::Root)
                .collect();
        }
        let (Some(open), Some(decoded)) = (self.open.as_mut(), asset.decoded()) else {
            return Vec::new();
        };
        match asset.shape {
            Shape::Fields => {
                if let Some(doc) = doc {
                    field::nav(ui, &mut open.fields, doc);
                }
            }
            Shape::Sample => {
                if let Some(Ok(snapshot)) = sample::snapshot(decoded) {
                    let asks = sample::map(ui, &mut open.sample, &snapshot, sets, played);
                    return asks.into_iter().map(Asked::Zone).collect();
                }
            }
            Shape::Project => {
                if let Some(Ok(snapshot)) = project::snapshot(decoded) {
                    project::map(ui, &mut open.sample, &snapshot, sets, played);
                }
            }
            Shape::Piano
            | Shape::SetList
            | Shape::Text
            | Shape::Verbatim
            | Shape::Wav
            | Shape::Undecoded => {}
        }
        Vec::new()
    }

    /// Do what the Basic view asked for, now that nothing is borrowing the asset.
    fn answer(&mut self, id: u64, asked: Asked, workspace: &mut Workspace, log: &mut Log) {
        match asked {
            Asked::Zone(sample::Ask::Decode(zone)) => {
                if !self.audio.due(zone) {
                    return;
                }
                if let Some(decoded) = workspace.get(id).and_then(|e| e.entity.as_ref()) {
                    self.audio.decode(decoded, zone);
                }
            }
            Asked::Zone(sample::Ask::Strike {
                zone,
                semitones,
                finger,
            }) => {
                if let Some(decoded) = workspace.get(id).and_then(|e| e.entity.as_ref()) {
                    self.audio.decode(decoded, zone);
                }
                let Some(Ok(decoded)) = self.audio.get(zone) else {
                    return;
                };
                let rate = crate::audio::rate(semitones);
                if let Err(why) = self.player.strike(
                    finger,
                    (id, zone),
                    &decoded.audio.samples,
                    decoded.audio.channels,
                    rate,
                ) {
                    log.error(why);
                    log.trouble("This computer would not play that zone.");
                }
            }
            Asked::Zone(sample::Ask::Play(zone)) => {
                // ⚠️ An edit drops the decoded audio of a zone that is still sounding,
                // so stopping a sounding zone must not depend on decoded audio.
                if self.player.sounds((id, zone)) {
                    self.player.silence((id, zone));
                } else if let Some(Ok(decoded)) = self.audio.get(zone) {
                    if let Err(why) = self.player.toggle(
                        (id, zone),
                        &decoded.audio.samples,
                        decoded.audio.channels,
                    ) {
                        log.error(why);
                        log.trouble("This computer would not play that zone.");
                    }
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
            Asked::Root(ask) => self.root_audio(id, ask, workspace, log),
            Asked::Encode => self.encode(id, workspace, log),
            // ⚠️ Handled in `ui`, where the frame collects its wants: the browser opens
            // tabs, the frame switches faces, and an export waits for the plan. See
            // [`Document::export`].
            Asked::Export | Asked::Open(_) | Asked::Advanced => {}
        }
    }

    /// Show, play, or save one root of a piano library. The stroke is decoded first,
    /// once, because every request needs it.
    fn root_audio(&mut self, id: u64, ask: piano::Ask, workspace: &mut Workspace, log: &mut Log) {
        let root = ask.root();
        if ask == piano::Ask::Show(root) && !self.piano.due(root) {
            return;
        }
        let Some(entity) = workspace.get(id) else {
            return;
        };
        if let Err(why) = self.piano.decode(entity, root) {
            // ⚠️ An open row asks for its waveform itself and shows why it has none.
            // The log is for requests the user made.
            if !matches!(ask, piano::Ask::Show(_)) {
                log.error(why);
                log.trouble("That root could not be decoded.");
            }
            return;
        }
        let Some(sound) = self.piano.sound(root) else {
            return;
        };
        match ask {
            piano::Ask::Show(_) => {}
            piano::Ask::Play(_) => {
                if let Err(why) =
                    self.player
                        .toggle((id, usize::from(root)), sound.samples, sound.channels)
                {
                    log.error(why);
                    log.trouble("This computer would not play that root.");
                }
            }
            piano::Ask::Strike {
                semitones, finger, ..
            } => {
                let rate = crate::audio::rate(semitones);
                if let Err(why) = self.player.strike(
                    finger,
                    (id, usize::from(root)),
                    sound.samples,
                    sound.channels,
                    rate,
                ) {
                    log.error(why);
                    log.trouble("This computer would not play that root.");
                }
            }
            piano::Ask::Save(_) => {
                match nord_format::wav::pcm16(sound.samples, sound.rate, sound.channels) {
                    Ok(bytes) => workspace.save_bytes(sound.name, bytes),
                    Err(e) => {
                        log.error(e.to_string());
                        log.trouble("That root could not be written as a WAV.");
                    }
                }
            }
        }
    }

    /// The name a zone's WAV file is based on: the instrument's name, or the asset's
    /// name when the instrument has none.
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

    /// Build an instrument from the open WAV as a new asset. The WAV is left unchanged.
    fn encode(&mut self, id: u64, workspace: &mut Workspace, log: &mut Log) {
        let Some((draft, source)) = self.open.as_ref().and_then(|open| open.wav.as_ref()) else {
            return;
        };
        let made = encode::instrument(draft, source).map(|bytes| {
            (
                format!("{}.{}", draft.name, draft.layout.extension()),
                bytes,
            )
        });
        match made {
            Ok((name, bytes)) => {
                self.refused(None);
                workspace.ingest(name, crate::workspace::Origin::Fresh, bytes, log);
            }
            Err(why) => {
                log.error(format!(
                    "encode {}: {why}",
                    workspace.get(id).map_or("", |e| &e.name)
                ));
                self.refused(Some(why));
            }
        }
    }

    fn project_body(&mut self, ui: &mut egui::Ui, decoded: &nord_format::Entity, sets: &mut Sets) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match project::snapshot(decoded) {
            Some(Ok(snapshot)) => {
                project::ui(ui, &mut open.sample, &snapshot, &mut open.paths, sets)
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
        let (before, edited) = (entity.stamp, shape(entity));
        // ⚠️ Works on the asset's own bytes, never a copy. A piano library is hundreds
        // of megabytes and every set of every frame comes through here; the piano arm
        // makes no bytes because its sets go into a plan.
        let made = match edited {
            Shape::Sample => sample::apply(&entity.bytes, &sets).map(Some),
            Shape::Project => project::apply(&entity.bytes, &sets).map(Some),
            // The plan makes a piano's bytes; see [`Document::replan`].
            Shape::Piano => self.piano.take(&sets).map(|()| None),
            Shape::SetList => setlist::apply(&entity.bytes, &sets).map(Some),
            Shape::Text => text::apply(&entity.bytes, &sets).map(Some),
            Shape::Fields | Shape::Verbatim | Shape::Wav | Shape::Undecoded => {
                fields::apply(&entity.bytes, &sets).map(|(_, out)| Some(out))
            }
        };
        let made = match made {
            Ok(made) => made,
            Err(why) => {
                log.error(why.clone());
                self.refused(Some(why.clone()));
                return Err(why);
            }
        };
        self.refused(None);
        // `replace_bytes` decides whether the bytes changed; it is the only place two
        // bodies are compared.
        if let Some(out) = made {
            workspace.replace_bytes(id, out, log);
        }
        if let (Shape::Sample, Some(held)) = (edited, workspace.get(id)) {
            if let Some(decoded) = &held.entity {
                self.audio.carry(id, (before, held.stamp), decoded);
            }
        }
        Ok(())
    }
}

/// The faces this document offers, in the order the control shows them.
///
/// Advanced is always offered, because every asset has a record, even bytes that did
/// not decode.
fn faces(shape: Shape) -> Vec<Face> {
    // A WAV does not decode, but it can be encoded into an instrument, so it gets a
    // panel as well as the byte record.
    let panel = match shape {
        Shape::Fields
        | Shape::SetList
        | Shape::Sample
        | Shape::Project
        | Shape::Piano
        | Shape::Text
        | Shape::Verbatim
        | Shape::Wav => true,
        Shape::Undecoded => false,
    };
    let mut faces = Vec::new();
    if panel {
        faces.push(Face::Basic);
    }
    faces.push(Face::Advanced);
    faces
}

/// What the header strip shows in place of its own defaults.
fn extras(
    asset: Asset<'_>,
    device: &Device,
    workspace: &Workspace,
    pending: usize,
) -> header::Extras {
    let entity = asset.entity;
    match asset.shape {
        Shape::Fields => header::Extras {
            edited: (pending > 0).then(|| StateLine {
                words: format!("{pending} pending"),
                ink: Ink::Warn,
                hint: format!("raw ≠ bits on {pending} fields"),
            }),
            loud: queued(entity, device, pending),
            ..header::Extras::default()
        },
        Shape::SetList => header::Extras {
            state: asset.decoded().and_then(|decoded| {
                setlist::claim(
                    decoded,
                    &setlist::Catalog {
                        device: &device.state,
                        workspace,
                    },
                )
            }),
            ..header::Extras::default()
        },
        Shape::Verbatim => {
            // The strip's own write, relabeled for a body that cannot be edited.
            let mut loud = header::action(entity, &device.state);
            loud.label = "Send as-is".to_string();
            loud.short = "Send".to_string();
            header::Extras {
                loud: Some(loud),
                ..header::Extras::default()
            }
        }
        Shape::Sample
        | Shape::Project
        | Shape::Piano
        | Shape::Text
        | Shape::Wav
        | Shape::Undecoded => header::Extras::default(),
    }
}

/// The face to show: the one this document was last left on if it is still offered,
/// otherwise the first face offered.
fn showing(faces: &[Face], remembered: Face) -> Face {
    match faces.contains(&remembered) {
        true => remembered,
        false => faces.first().copied().unwrap_or(Face::Advanced),
    }
}

/// The file's own metadata, shown ahead of the container record every asset has.
fn record(ui: &mut egui::Ui, asset: Asset<'_>) {
    let Some(decoded) = asset.decoded() else {
        return;
    };
    match asset.shape {
        Shape::Sample => {
            if let Some(Ok(snapshot)) = sample::snapshot(decoded) {
                sample::metadata(ui, &snapshot);
            }
        }
        Shape::Project => {
            if let Some(Ok(snapshot)) = project::snapshot(decoded) {
                project::metadata(ui, &snapshot);
            }
        }
        Shape::Fields
        | Shape::SetList
        | Shape::Piano
        | Shape::Text
        | Shape::Verbatim
        | Shape::Wav
        | Shape::Undecoded => {}
    }
}

/// The Advanced face of a body with no field registry: the format's capabilities and
/// field offsets, the addresses a set list stores, or the raw bytes of a body no
/// registry describes.
fn capabilities(ui: &mut egui::Ui, asset: Asset<'_>) {
    let Some(decoded) = asset.decoded() else {
        return;
    };
    match asset.shape {
        Shape::Sample => {
            if let Some(Ok(snapshot)) = sample::snapshot(decoded) {
                capability::table(ui, &sample::capabilities(snapshot.generation));
                capability::offsets(ui, &sample::offsets(&snapshot));
            }
        }
        Shape::Project => {
            if let Some(Ok(snapshot)) = project::snapshot(decoded) {
                capability::table(ui, &project::capabilities());
                capability::offsets(ui, &project::offsets(&snapshot));
            }
        }
        Shape::SetList => setlist::stored(ui, decoded),
        Shape::Verbatim => verbatim::bytes(ui, asset.entity),
        Shape::Fields | Shape::Piano | Shape::Text | Shape::Wav | Shape::Undecoded => {}
    }
}

/// The banner over a view of an instrument slot, which is not stored on this computer.
/// Returns whether the user chose to keep it.
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

/// The header's loud action for a field document, with the pending count added.
///
/// ⚠️ The count is added only when the header already offers the write. A document
/// with nothing to send to says so, and a count beside that message would read as an
/// offer.
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
        hint: format!("{pending} pending sets, applied as one batch: all or none"),
        ..loud
    })
}

/// What is known about the piano a program plays.
///
/// ⚠️ The name can only come from the instrument. A `.ne5p` stores the piano's id and no
/// name, and the category and model beside it are the panel's dial positions, which do
/// not identify a piano. A document opened with no instrument attached shows the id and
/// says so.
fn piano_lookup(
    entity: &LocalEntity,
    registry: Option<&[Field]>,
    device: &Device,
) -> panel::PianoLookup {
    let id = registry
        .and_then(|fields| fields.iter().find(|field| field.path == "piano_panel.id"))
        .and_then(|field| library_id(&field.value))
        // Zero means the program references no piano.
        .filter(|id| *id != 0);
    let slot = entity.origin.slot();
    let name = id
        .and_then(|id| device.state.dependency_name(ObjectClass::Piano, id))
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
        refused: slot.is_some_and(|(class, at)| device.deps_refused(class, at)),
        asked: false,
        models,
        scan_disagrees,
    }
}

/// The Pianos folder's names for the document's current category, by Model dial
/// position, which turns the Model dial into a list of pianos.
///
/// Banks map to categories and slot order to dial position. Confirmed on hardware. The
/// instrument's bank list names the piano banks after the panel's categories, in panel
/// order, and a program's stored category and model match the bank and slot the
/// instrument reports for the piano it depends on. The dependency name remains the
/// check; see `scan_disagrees` in [`piano_lookup`].
fn piano_models(fields: &[Field], device: &Device) -> Vec<(u32, String)> {
    let Some(category) = fields
        .iter()
        .find(|field| field.path == "piano_panel.category")
    else {
        return Vec::new();
    };
    // The category's stored bits are its position in the legal list, because
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

/// Parse a library id: decimal as the field list spells it, or hex as a person may type
/// it.
pub(crate) fn library_id(value: &str) -> Option<u32> {
    let text = value.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Every shape a frame painted, with its clip rect, with nested shape lists flattened.
#[cfg(test)]
fn leaves(output: &egui::FullOutput) -> Vec<(egui::Rect, &egui::Shape)> {
    fn open<'a>(
        clip: egui::Rect,
        shape: &'a egui::Shape,
        into: &mut Vec<(egui::Rect, &'a egui::Shape)>,
    ) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| open(clip, shape, into)),
            shape => into.push((clip, shape)),
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        open(clipped.clip_rect, &clipped.shape, &mut found);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Fresh, Origin};

    /// One document open in a headless window, with everything a frame of it needs.
    ///
    /// Nothing checks pixels. A paint pass catches the failures a document can have: a
    /// section that indexes past its fields, or a control given a value the field
    /// cannot hold.
    struct Open {
        ctx: egui::Context,
        workspace: Workspace,
        device: Device,
        log: Log,
        queue: Queue,
        tags: Tags,
        document: Document,
        id: u64,
        /// The window width, which decides how far the header collapses.
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
            // Install the app's text styles and fonts. A named text style the header
            // asks for panics mid-frame when it is not registered, and so does a missing
            // semibold family for section headings and zone rows.
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

        /// The open document's state.
        fn state(&mut self) -> &mut Opened {
            self.document.open.as_mut().expect("a document is open")
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
                            played: &Played::default(),
                        },
                    );
                });
            });
            placed(&output).into_iter().map(|(word, _)| word).collect()
        }

        /// One frame, and every shape it painted, for tests that check where a word
        /// lands.
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
                            played: &Played::default(),
                        },
                    );
                });
            })
        }

        /// Two frames. The second runs with the caches and widget state the first left
        /// behind, which is where a stale index shows up.
        fn twice(&mut self) -> Vec<String> {
            self.frame(Vec::new());
            self.frame(Vec::new())
        }
    }

    /// Every word a frame painted, with the rect it was painted in.
    fn placed(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        leaves(output)
            .into_iter()
            .filter_map(|(_, shape)| match shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect()
    }

    fn render(sets: &[(&str, &str)], kind: Fresh) {
        render_view(sets, kind, Face::Basic);
    }

    /// Every kind, in both the dark and the light theme.
    #[test]
    fn every_kind_shows_the_header_and_names_its_faces() {
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
                assert!(has("Basic"), "{kind:?} in {dark}: {said:?}");
                assert!(has("Advanced"), "{kind:?} in {dark}: {said:?}");
                assert!(has("Queue send"), "the loud action: {said:?}");
                assert!(has("Revert") && has("Export…"), "the quiet ones: {said:?}");
            }
        }
    }

    /// The controls hold the right edge, and the words on the left wrap to a row below
    /// the controls instead of running under them.
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

        let words: Vec<(String, egui::Rect)> = leaves(&output)
            .into_iter()
            .filter_map(|(_, shape)| match shape {
                egui::Shape::Text(text) => Some((
                    match text.galley.rows.len() {
                        1 => text.galley.text().to_string(),
                        rows => format!("{} (in {rows} rows)", text.galley.text()),
                    },
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect();
        let header: Vec<&(String, egui::Rect)> =
            words.iter().filter(|(_, rect)| rect.top() < 70.0).collect();
        for (word, _) in &header {
            assert!(
                !word.ends_with(" rows)"),
                "{word} was broken across rows instead of wrapping whole"
            );
        }
        // The controls, and the glyphs painted where their icons would load. The kind
        // glyph at the far left is the only glyph that is not a control.
        const CONTROLS: [&str; 6] = [
            "Send",
            "Queue send",
            "Basic",
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
            "the left group holds the state phrase that fills it: {header:?}"
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

    /// The quiet actions go first, then the faces. The loud action keeps a short label
    /// and never becomes a bare glyph.
    #[test]
    fn a_narrowing_header_drops_labels_in_a_fixed_order() {
        let at = |width: f32| {
            let mut open = Open::fresh(Fresh::Program);
            open.width = width;
            open.twice()
        };

        let full = at(1100.0);
        assert!(full.contains(&"Export…".to_string()), "{full:?}");
        assert!(full.contains(&"Basic".to_string()));
        assert!(full.contains(&"Queue send".to_string()));

        let quiet = at(900.0);
        assert!(!quiet.contains(&"Export…".to_string()), "{quiet:?}");
        assert!(
            quiet.contains(&"Basic".to_string()),
            "the faces keep their labels"
        );
        assert!(quiet.contains(&"Queue send".to_string()));

        let faces = at(800.0);
        assert!(!faces.contains(&"Basic".to_string()), "{faces:?}");
        assert!(faces.contains(&"Queue send".to_string()));

        let narrow = at(700.0);
        assert!(!narrow.contains(&"Queue send".to_string()), "{narrow:?}");
        assert!(
            narrow.contains(&"Send".to_string()),
            "the loud action is never a bare glyph: {narrow:?}"
        );
    }

    /// The name box edits the asset's name where the file stores none, and the name
    /// keeps the extension the box does not show.
    #[test]
    fn typing_in_the_name_box_renames_the_asset_and_keeps_its_extension() {
        let mut open = Open::fresh(Fresh::Program);
        assert_eq!(open.entity().name, "untitled.ne5p");
        open.frame(Vec::new());
        open.frame(vec![click(NAME_BOX)]);
        open.frame(vec![egui::Event::Text("X".to_string())]);
        open.frame(vec![enter()]);

        let renamed = open.entity().name.clone();
        assert_ne!(renamed, "untitled.ne5p", "the box was typed into");
        assert!(renamed.ends_with(".ne5p"), "the extension stays: {renamed}");
        assert!(renamed.contains('X'), "X is in the name: {renamed}");
    }

    /// ⚠️ A WAV's encode draft is stored nowhere else. A document rebuilt after a rename
    /// would reset it to the encoder's defaults and lose the user's choices.
    #[test]
    fn renaming_a_wav_keeps_the_encode_draft_it_is_open_on() {
        let mut open = Open::file("Marimba hit.wav", wav_bytes());
        open.frame(Vec::new());
        let (draft, _) = open.state().wav.as_mut().expect("a WAV opens the panel");
        assert_ne!(
            (draft.root_key, draft.top_note),
            (48, 60),
            "the test values must differ from the defaults"
        );
        (draft.root_key, draft.top_note) = (48, 60);

        open.frame(vec![click(NAME_BOX)]);
        open.frame(vec![egui::Event::Text("X".to_string())]);
        open.frame(vec![enter()]);
        let renamed = open.entity().name.clone();
        assert!(renamed.contains('X'), "the box was typed into: {renamed}");
        assert!(renamed.ends_with(".wav"), "{renamed}");

        open.frame(Vec::new());
        let (draft, _) = open.state().wav.as_ref().expect("the same panel");
        assert_eq!((draft.root_key, draft.top_note), (48, 60));
    }

    /// Where the file stores the name, the box writes it through the format and leaves
    /// the asset's name alone.
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
            "the box does not rename the asset"
        );
    }

    /// ⚠️ A stored name box is limited to the field's length in bytes. A box counting
    /// characters would accept an accented name twice that long, and the format would
    /// refuse it only after the whole name was typed.
    #[test]
    fn a_stored_name_box_holds_no_more_bytes_than_the_field_does() {
        let mut open = Open::file("whatever.nsmp", sample_bytes());
        let held = |open: &Open| {
            sample::snapshot(open.entity().entity.as_ref().expect("it decoded"))
                .expect("an instrument")
                .expect("it reads")
        };
        let limit = held(&open).max_name_len;
        assert!(limit > 2, "there is room for an accented letter");

        open.frame(Vec::new());
        open.frame(vec![click(NAME_BOX)]);
        open.frame(vec![egui::Event::Text("é".repeat(limit))]);
        open.frame(vec![enter()]);

        let stored = held(&open).name;
        assert!(stored.contains('é'), "é was stored: {stored:?}");
        assert!(stored.len() <= limit, "{} bytes: {stored:?}", stored.len());
        assert_eq!(
            open.document.refusal(),
            None,
            "the box never passes the field more bytes than it holds"
        );
    }

    /// Tags show as chips on the identity row, and a program with none has no tags
    /// cell.
    #[test]
    fn the_identity_row_shows_the_assets_tags() {
        let mut open = Open::fresh(Fresh::Program);
        let said = open.twice();
        assert!(
            !said.iter().any(|word| word == "TAGS"),
            "no tags, so no cell: {said:?}"
        );

        let friday = open.tags.make("Friday — Blue Room").unwrap();
        let organ = open.tags.make("Organ-heavy").unwrap();
        for tag in [friday, organ] {
            open.tags.set(open.id, tag, true);
        }
        let said = open.twice();
        assert!(said.iter().any(|word| word == "TAGS"), "{said:?}");
        for worn in ["Friday — Blue Room", "Organ-heavy"] {
            assert!(said.iter().any(|word| word == worn), "{worn}: {said:?}");
        }
    }

    /// A click inside the name box, just after the kind glyph at the left of the strip.
    const NAME_BOX: egui::Pos2 = egui::pos2(100.0, 19.0);

    /// The bottom-right corner of the page a document draws in, which is inside a
    /// note's box only if the box fills the page.
    const PAGE_CORNER: egui::Pos2 = egui::pos2(SCREEN.x - 24.0, SCREEN.y - 24.0);

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

    /// An empty send queue and no tags.
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

    /// A count on a dashed action would read as an offer.
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
            "with nothing attached, the action carries no count: {unattached:?}"
        );

        open.device
            .pretend_scanned(ObjectClass::Program, 7, &["", "", "", "Africa Split"]);
        let attached = open.twice();
        assert!(
            attached.iter().any(|word| word == "Queue send · 1"),
            "{attached:?}"
        );
    }

    /// A group is not in use when the file's state leaves its controls without effect.
    /// Its controls are hidden, and the section names the group.
    #[test]
    fn a_group_the_instrument_is_not_using_is_named() {
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
            idle.iter().all(|line| line.contains("Kept, not cleared")),
            "{idle:?}"
        );
    }

    /// Advanced opens on the file's own record, and the table under it holds every
    /// field, including those Basic hides, with a count of how many that is.
    #[test]
    fn the_advanced_face_holds_the_record_and_the_fields_basic_hides() {
        let mut open = Open::fresh(Fresh::Program);
        open.document.views.insert(open.id, Face::Advanced);
        let said = open.twice();
        let reading = said
            .iter()
            .find(|word| word.contains("hidden from Basic"))
            .unwrap_or_else(|| panic!("the table counts what Basic does not draw: {said:?}"));
        assert!(!reading.contains("· 0 hidden"), "{reading}");
        for section in ["About this file", "Every field"] {
            assert!(
                said.iter().any(|word| word == section),
                "{section}: {said:?}"
            );
        }
    }

    /// The body is the longest block, so it comes last.
    #[test]
    fn the_advanced_face_reads_from_the_record_down_to_the_body() {
        let mut open = Open::fresh(Fresh::Program);
        open.document.views.insert(open.id, Face::Advanced);
        open.frame(Vec::new());
        let output = open.output(Vec::new());
        let placed = placed(&output);
        let top = |word: &str| -> f32 {
            placed
                .iter()
                .find(|(text, _)| text == word)
                .unwrap_or_else(|| panic!("{word} was never painted: {placed:?}"))
                .1
                .top()
        };

        let order = ["About this file", "Container", "Changes", "Every field"];
        for pair in order.windows(2) {
            assert!(
                top(pair[0]) < top(pair[1]),
                "{} is not above {}",
                pair[0],
                pair[1],
            );
        }
    }

    #[test]
    fn every_column_of_the_advanced_face_reads_down_from_its_heading() {
        let mut open = Open::fresh(Fresh::Program);
        open.document.views.insert(open.id, Face::Advanced);
        open.frame(Vec::new());
        let output = open.output(Vec::new());
        let placed = placed(&output);
        let left = |word: &str| -> f32 {
            placed
                .iter()
                .find(|(text, _)| text == word)
                .unwrap_or_else(|| panic!("{word} was never painted: {placed:?}"))
                .1
                .left()
        };

        let edge = left("Format");
        for word in ["Fields", "Layout", "Stored at", "Instrument", "PATH"] {
            assert_eq!(left(word), edge, "{word} is not aligned with Format");
        }
        assert!(
            left("program v4") > edge,
            "the value column starts right of the labels"
        );

        for (head, cell) in [
            ("PATH", "center_panel.lower_part"),
            ("BITS", "0..=2"),
            ("CONTROL", "selector"),
            ("RAW", "Organ"),
        ] {
            let under = left(head);
            assert!(
                placed
                    .iter()
                    .any(|(text, rect)| text == cell && rect.left() == under),
                "no {cell} cell is aligned under {head} at {under}",
            );
        }
    }

    /// The playing registration says so, and the other offers a switch to it.
    #[test]
    fn both_stored_alternatives_paint_and_the_playing_one_says_so() {
        let mut open = Open::fresh(Fresh::Program);
        let said = open.twice();
        for title in ["B3 · Preset 1", "B3 · Preset 2"] {
            assert!(said.iter().any(|word| word == title), "{title}: {said:?}");
        }
        assert!(said.iter().any(|word| word == "playing"), "{said:?}");
        assert!(said.iter().any(|word| word == "select"), "{said:?}");
    }

    /// The lens shows each morphed control's value under one performance control, and a
    /// banner above the sections says so.
    #[test]
    fn the_morph_lens_shows_what_a_control_becomes_under_the_wheel() {
        let mut open = Open::file("blank.ns4y", Fresh::Stage4Synth.bytes().unwrap());
        open.set(&[("synth_a_volume", "40"), ("synth_a_volume_wheel", "211")]);
        let panel = open.twice();
        assert!(
            !panel.iter().any(|word| word == "211"),
            "without the lens, the panel value shows: {panel:?}"
        );

        open.state().fields.pretend_lens(0);
        let wheel = open.twice();
        assert!(wheel.iter().any(|word| word == "211"), "{wheel:?}");
        assert!(
            wheel
                .iter()
                .any(|word| word.contains("writes the morph slot")),
            "the banner says where an edit lands: {wheel:?}"
        );
    }

    /// Real files hold unnamed positions, and that spelling is the only way to write one
    /// back.
    #[test]
    fn a_position_the_library_cannot_name_is_shown() {
        let mut open = Open::fresh(Fresh::Program);
        open.set(&[("center_panel.organ_type", "unknown (6)")]);
        let said = open.twice();
        assert!(
            said.iter().any(|word| word == "unrecognized value (6)"),
            "{said:?}"
        );
    }

    /// An unpolished label should look unpolished. Cells show names in capitals.
    #[test]
    fn a_path_with_no_label_shows_a_name_derived_from_the_path() {
        let said = Open::file("blank.ns4y", Fresh::Stage4Synth.bytes().unwrap()).twice();
        assert!(!strings::known("synth_a_volume"));
        assert!(
            said.iter().any(|word| word == "SYNTH A VOLUME"),
            "{:?}",
            &said[..said.len().min(40)]
        );
    }

    #[test]
    fn every_section_of_a_stage_program_is_open_and_named_in_the_nav() {
        let bytes = Fresh::Stage4Program.bytes().unwrap();
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
                "{title} was painted {drawn} times; expected the nav chip and the heading",
            );
        }
    }

    /// The Advanced face paints for a program with and without an edit, and for a
    /// settings file, including the container grid, the byte diff, and the folded dump.
    #[test]
    fn the_advanced_face_paints() {
        render_view(
            &[("center_panel.gain", "96")],
            Fresh::Program,
            Face::Advanced,
        );
        render_view(&[], Fresh::Program, Face::Advanced);
        render_view(&[], Fresh::Settings, Face::Advanced);
    }

    /// Anything with a panel offers both faces. Bytes that did not decode offer only
    /// Advanced, which holds their record.
    #[test]
    fn the_faces_offered_are_the_ones_the_asset_has() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();

        let offered = |workspace: &Workspace, id: u64| -> Vec<&'static str> {
            faces(shape(workspace.get(id).unwrap()))
                .iter()
                .map(|face| face.label())
                .collect()
        };

        let program = workspace.create(Fresh::Program, &mut log).unwrap();
        assert_eq!(offered(&workspace, program), ["Basic", "Advanced"]);

        let song = workspace.ingest(
            "blank.ne5t".into(),
            Origin::File("blank.ne5t".into()),
            Fresh::SetList.bytes().unwrap(),
            &mut log,
        );
        assert_eq!(
            offered(&workspace, song),
            ["Basic", "Advanced"],
            "the four entries, and the record with the addresses as stored"
        );

        let stub = workspace.ingest(
            "blank.ns3s".into(),
            Origin::File("blank.ns3s".into()),
            crate::fields::blank::stage3_song(),
            &mut log,
        );
        assert_eq!(
            offered(&workspace, stub),
            ["Basic", "Advanced"],
            "a body no registry describes still says so, and shows its bytes"
        );

        let junk = workspace.ingest(
            "junk.bin".into(),
            Origin::File("junk.bin".into()),
            junk_bytes(),
            &mut log,
        );
        assert_eq!(offered(&workspace, junk), ["Advanced"]);
    }

    /// A document opens on the face it was left on, or Basic if it was never left on
    /// one. When that face is not offered, it falls back to one that is.
    #[test]
    fn a_document_falls_back_to_a_face_it_has() {
        let both = [Face::Basic, Face::Advanced];
        assert_eq!(showing(&both, Face::default()), Face::Basic);
        assert_eq!(showing(&both, Face::Advanced), Face::Advanced);

        let record_only = [Face::Advanced];
        for left_on in [Face::Basic, Face::Advanced] {
            assert_eq!(showing(&record_only, left_on), Face::Advanced);
        }
    }

    /// A cell the library refuses stays open with what was typed in it, because that is
    /// the only copy of what the user meant.
    #[test]
    fn a_refused_cell_keeps_its_error() {
        let mut open = Open::fresh(Fresh::Program);
        open.frame(Vec::new());
        let (id, before) = (open.id, open.entity().bytes.clone());

        // The same call the frame makes, so this covers how the table handles the
        // library's answer.
        let refused = open.document.apply(
            id,
            vec![("center_panel.gain".into(), "200".into())],
            &mut open.workspace,
            &mut open.log,
        );
        assert!(refused.is_err());
        assert!(
            refused.as_ref().unwrap_err().contains("0 .. 127"),
            "the library's message reaches the user: {refused:?}"
        );
        assert_eq!(
            open.entity().bytes,
            before,
            "a refused value leaves the file untouched"
        );
        open.document.advanced.settled(refused);
        assert!(open.document.refusal().is_some());

        let taken = open.document.apply(
            id,
            vec![("center_panel.gain".into(), "96".into())],
            &mut open.workspace,
            &mut open.log,
        );
        assert!(taken.is_ok());
        open.document.advanced.settled(taken);
        assert!(open.document.refusal().is_none());
    }

    /// A set that writes a field's current value is not an edit: the asset keeps its
    /// bytes and the stamp that caches are keyed on.
    #[test]
    fn a_set_that_leaves_the_bytes_alone_is_not_an_edit() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let mut document = Document::default();
        let id = workspace.create(Fresh::Program, &mut log).expect("a fresh");

        let entity = workspace.get(id).expect("it is open");
        let (stamp, bytes) = (entity.stamp, entity.bytes.clone());
        let held = fields::apply(&bytes, &[]).expect("it decodes").0;
        let gain = held
            .iter()
            .find(|field| field.path == "center_panel.gain")
            .expect("a gain field")
            .value
            .clone();

        let outcome = document.apply(
            id,
            vec![("center_panel.gain".into(), gain)],
            &mut workspace,
            &mut log,
        );
        assert!(outcome.is_ok(), "{outcome:?}");

        let entity = workspace.get(id).expect("it is open");
        assert_eq!(entity.bytes, bytes);
        assert_eq!(entity.stamp, stamp, "the stamp did not change");
        assert!(!entity.is_unsaved());
    }

    /// ⚠️ The tab strip and the body are two scroll regions in one `Ui`. On one id they
    /// would share scroll state, and the wheel over the document would scroll the tab
    /// strip.
    #[test]
    fn the_tab_strip_and_the_document_body_scroll_independently() {
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
                            played: &Played::default(),
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
            "the body scrolled: {:?}",
            offset(body)
        );
    }

    /// A program stores its piano's id; only the instrument knows the name.
    #[test]
    fn a_pianos_name_comes_only_from_the_instrument() {
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
        assert!(!local.can_ask, "and nothing can be asked");

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
        // A fresh program references no piano, and zero is not an id.
        assert_eq!(copied.id, None);
    }

    /// The piano's name survives a read of another slot.
    #[test]
    fn a_document_asks_what_its_slot_plays_and_keeps_the_answer() {
        use nord_usb::wire::Dependency;

        let class = ObjectClass::Program;
        let at = Location { bank: 6, slot: 3 };
        let piano = 0x0102_0304;

        let mut open = Open::empty();
        open.device.pretend_attached();
        let fresh = open
            .workspace
            .create(Fresh::Program, &mut open.log)
            .expect("a fresh default");
        let bytes = open.workspace.get(fresh).expect("just made").bytes.clone();
        let (_, plays) = fields::apply(&bytes, &[("piano_panel.id".into(), piano.to_string())])
            .expect("a program can name a piano");
        open.id = open.workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device { class, at },
            plays,
            &mut open.log,
        );

        let reads = |open: &Open| {
            open.device
                .queued()
                .iter()
                .filter(|cmd| {
                    matches!(cmd, crate::device::DeviceCmd::Deps { at: asked, .. }
                    if *asked == at)
                })
                .count()
        };
        open.frame(Vec::new());
        assert_eq!(reads(&open), 1, "the document asked what the slot plays");
        open.frame(Vec::new());
        assert_eq!(reads(&open), 1, "and asked only once");

        let named = |open: &Open| {
            let registry = fields::apply(&open.entity().bytes, &[])
                .expect("the registry reads it")
                .0;
            piano_lookup(open.entity(), Some(&registry), &open.device)
                .name
                .clone()
        };
        assert_eq!(named(&open), None, "nothing has answered yet");

        open.device.pretend_deps(
            class,
            at,
            vec![Dependency {
                flag: 1,
                class: ObjectClass::Piano,
                id: piano,
                name: "Royal Grand 3D".into(),
                location: None,
            }],
        );
        assert_eq!(named(&open).as_deref(), Some("Royal Grand 3D"));

        open.device
            .pretend_deps(class, Location { bank: 0, slot: 0 }, Vec::new());
        assert_eq!(
            named(&open).as_deref(),
            Some("Royal Grand 3D"),
            "the name survives another slot's read"
        );
    }

    /// Without a scan, the dial stays numeric and never shows a guessed name.
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
            "empty slots keep their positions"
        );
        // No dependency name is in hand, so there is nothing to disagree with.
        assert!(scanned.scan_disagrees.is_none());

        // Another bank belongs to another category.
        let mut other = Device::new(egui::Context::default());
        other.pretend_scanned(ObjectClass::Piano, 3, &["Clav D6"]);
        let elsewhere = piano_lookup(workspace.get(id).unwrap(), Some(&fields), &other);
        assert!(elsewhere.models.is_empty());
    }

    /// ⚠️ A cell being edited belongs to the document it was opened in. One table serves
    /// every tab, and two programs declare the same paths, so a half-typed value must be
    /// dropped on a switch. Otherwise it follows the user into the next tab and lands
    /// there on Enter.
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
                            played: &Played::default(),
                        },
                    );
                });
            });
        };

        show(&mut document, first);
        document.advanced.pretend_editing("center_panel.gain", "0");
        assert_eq!(document.advanced.editing(), Some("center_panel.gain"));

        show(&mut document, second);
        assert_eq!(document.advanced.editing(), None, "dropped on the switch");
    }

    /// ⚠️ Only an Enter in the cell submits it. A refused cell stays open with what was
    /// typed, and an Enter read from the window would submit it again, and log the
    /// refusal again, wherever the user was typing.
    #[test]
    fn an_enter_elsewhere_does_not_submit_a_refused_cell_again() {
        let mut open = Open::fresh(Fresh::Program);
        open.document.views.insert(open.id, Face::Advanced);
        open.frame(Vec::new());
        open.document
            .advanced
            .pretend_editing("center_panel.gain", "200");
        // One frame takes the focus the cell asked for, the next types Enter in it.
        open.frame(Vec::new());
        open.frame(vec![enter()]);
        assert!(open.document.refusal().is_some(), "the library refused 200");
        assert_eq!(
            open.document.advanced.editing(),
            Some("center_panel.gain"),
            "and the cell keeps what was typed"
        );

        // Move the focus to the header's name box; the cell stays open behind it.
        open.frame(Vec::new());
        open.frame(vec![click(NAME_BOX)]);
        let said = open.log.len();
        open.frame(vec![enter()]);
        assert_eq!(open.log.len(), said, "nothing was submitted a second time");
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

    /// The Stage bodies have no panel of their own here, so they use the generic one:
    /// large ones as folds, small ones open with every control drawn.
    #[test]
    fn a_stage_document_paints_from_the_registry_alone() {
        for (name, bytes) in [
            ("blank.ns2p", Fresh::Stage2Program.bytes().unwrap()),
            ("blank.ns3y", Fresh::Stage3Synth.bytes().unwrap()),
            ("blank.ns4p", Fresh::Stage4Program.bytes().unwrap()),
            ("blank.ns4o", Fresh::Stage4Organ.bytes().unwrap()),
            ("blank.ns4n", Fresh::Stage4Piano.bytes().unwrap()),
            ("blank.ns4y", Fresh::Stage4Synth.bytes().unwrap()),
        ] {
            render_file(name, bytes.clone(), Face::Basic);
            render_file(name, bytes, Face::Advanced);
        }
    }

    /// An Electro 5 set list's view shows its four entries and does not come from the
    /// registry, which lists nothing for that body. Both faces paint.
    #[test]
    fn a_set_list_has_its_own_view() {
        let bytes = Fresh::SetList.bytes().unwrap();
        let song = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert!(fields::is_set_list(&song));
        assert!(!fields::has_registry(&song));
        for face in [Face::Basic, Face::Advanced] {
            render_file("blank.ne5t", bytes.clone(), face);
        }
    }

    /// A body no registry describes says there is nothing to draw (not that nothing was
    /// read) and lists what the container records. Only the Advanced face shows its bytes.
    #[test]
    fn a_body_with_no_registry_says_why_and_shows_its_bytes() {
        let bytes = crate::fields::blank::stage3_song();
        let song = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert!(!fields::is_set_list(&song));
        assert!(!fields::has_registry(&song));

        let mut open = Open::file("blank.ns3s", bytes);
        let said = open.twice();
        let has = |word: &str| said.iter().any(|held| held == word);
        assert!(has("Nothing to edit here yet"), "{said:?}");
        assert!(
            said.iter()
                .any(|word| word.starts_with("No registry declares this model's fields yet")),
            "the sentence: {said:?}"
        );
        for fact in ["Format", "Model", "Container", "Body", "Version", "Where"] {
            assert!(has(fact), "{fact} is one of the facts: {said:?}");
        }
        assert!(has("Save a copy…"), "the only action left: {said:?}");
        assert!(
            has("Send as-is"),
            "the loud action is the send, relabeled for this body: {said:?}"
        );
        assert!(
            !has("0000"),
            "the hex dump belongs on the Advanced face: {said:?}"
        );

        open.document.views.insert(open.id, Face::Advanced);
        let said = open.twice();
        assert!(
            said.iter().any(|word| word == "Body bytes"),
            "the Advanced face shows the whole body: {said:?}"
        );
        assert!(said.iter().any(|word| word == "3 rows"), "{said:?}");
        assert!(
            said.iter().any(|word| word == "0000"),
            "the bytes: {said:?}"
        );
    }

    /// A Stage Classic piano library (`nsp`): a container over a body this app does not
    /// decode. The stub is a zeroed body under the format's tag, built here instead of
    /// committed as a fixture.
    fn piano_library_bytes() -> Vec<u8> {
        use nord_format::cbin::{Cbin, Header, RawBody};
        use nord_format::formats::nsclassic;

        let file = Cbin {
            header: Header::new(nsclassic::piano_library::FORMAT, (0, 0), 0),
            body: RawBody(vec![0u8; 48]),
        };
        nord_format::to_bytes(&nord_format::Entity::PianoLibrary(file)).expect("a stub encodes")
    }

    /// Each editor claims the bodies it has a view for, and any other decoded body is
    /// verbatim.
    #[test]
    fn a_body_with_no_editor_of_its_own_is_verbatim_whatever_kind_it_is() {
        let held = |bytes: Vec<u8>| shape(Open::file("held", bytes).entity());
        assert_eq!(
            held(crate::workspace::Fresh::Stage4Program.bytes().unwrap()),
            Shape::Fields
        );
        assert_eq!(
            held(crate::workspace::Fresh::SetList.bytes().unwrap()),
            Shape::SetList
        );
        assert_eq!(held(sample_bytes()), Shape::Sample);
        assert_eq!(held(project_bytes()), Shape::Project);
        assert_eq!(held(piano_bytes()), Shape::Piano);
        assert_eq!(held(fields::blank::stage3_song()), Shape::Verbatim);
        assert_eq!(held(piano_library_bytes()), Shape::Verbatim);
        assert_eq!(held(wav_bytes()), Shape::Wav);
        assert_eq!(held(junk_bytes()), Shape::Undecoded);
        assert_eq!(
            held(b"Set 1\n  1. One More Time\n".to_vec()),
            Shape::Text,
            "undecoded bytes that are text open as text"
        );
    }

    #[test]
    fn typing_in_a_note_edits_its_bytes_and_marks_it_unsaved() {
        let mut open = Open::file("Set 1.txt", b"Set 1\n".to_vec());
        assert!(!open.entity().is_unsaved(), "just opened, it is saved");
        let said = open.twice();
        assert_eq!(
            faces(Shape::Text),
            vec![Face::Basic, Face::Advanced],
            "the box is the page, and the record is under Advanced"
        );
        assert!(
            said.iter().any(|word| word == "1 line"),
            "the strip counts the lines, and the page is only the words: {said:?}"
        );
        assert!(
            said.iter().any(|word| word == "Set 1\n"),
            "the page draws the text: {said:?}"
        );

        // ⚠️ Click the far corner of the page. A box that did not fill the space under
        // the header would not take the caret here, and nothing would be typed.
        open.frame(vec![click(PAGE_CORNER)]);
        open.frame(vec![egui::Event::Text("X".to_string())]);

        let written = String::from_utf8(open.entity().bytes.clone()).expect("still text");
        assert!(written.contains('X'), "X is in the bytes: {written:?}");
        assert_eq!(
            written.replace('X', ""),
            "Set 1\n",
            "and nothing else changed: {written:?}"
        );
        assert!(open.entity().is_unsaved(), "the edit marks it unsaved");
        assert!(
            open.log
                .iter()
                .all(|entry| entry.level == crate::log::Level::Info),
            "a note has nothing to verify, and typing in one is not a warning: {}",
            open.log.transcript()
        );

        open.workspace.mark_saved(open.id);
        assert!(!open.entity().is_unsaved(), "a save clears unsaved");
    }

    #[test]
    fn an_unedited_note_keeps_its_bytes() {
        let bytes = "\u{feff}Set 1\r\n\tcue\r\nlast line".as_bytes().to_vec();
        let mut open = Open::file("Set 1.txt", bytes.clone());
        open.twice();
        open.frame(vec![click(PAGE_CORNER)]);
        open.frame(Vec::new());
        open.document.leave();
        assert_eq!(open.entity().bytes, bytes);
        assert!(!open.entity().is_unsaved());
    }

    #[test]
    fn tab_in_a_note_types_a_tab() {
        let mut open = Open::file("Set 1.txt", b"Set 1\n".to_vec());
        open.twice();
        open.frame(vec![click(PAGE_CORNER)]);
        // The box takes the Tab key for itself only from the frame after it gains focus.
        open.frame(Vec::new());
        open.frame(vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        open.frame(vec![egui::Event::Text("cue".to_string())]);
        assert_eq!(
            String::from_utf8_lossy(&open.entity().bytes),
            "Set 1\n\tcue"
        );
    }

    /// A piano library this app cannot decode opens like any other verbatim body: a page
    /// that says so, its bytes, and Send as-is.
    #[test]
    fn a_stage_classic_piano_library_gets_the_verbatim_faces() {
        let mut open = Open::file("Grand.nsp", piano_library_bytes());
        assert_eq!(
            faces(shape(open.entity()))
                .iter()
                .map(|face| face.label())
                .collect::<Vec<_>>(),
            ["Basic", "Advanced"],
        );
        let said = open.twice();
        let has = |word: &str| said.iter().any(|held| held == word);
        assert!(has("Nothing to edit here yet"), "{said:?}");
        assert!(has("Send as-is"), "{said:?}");
    }

    /// A set list's header reports problems only after the instrument has been read: an
    /// unread bank is not four problems.
    #[test]
    fn a_set_lists_header_claims_only_what_the_instrument_showed() {
        let mut open = Open::file("Blue Room.ne5t", Fresh::SetList.bytes().unwrap());
        let said = open.twice();
        assert!(
            !said.iter().any(|word| word.contains("needs attention")),
            "nothing is attached, so nothing is claimed: {said:?}"
        );

        // Bank 1 is read, and the slot the second entry names is empty.
        open.device.pretend_scanned(
            ObjectClass::Program,
            1,
            &["Africa Split", "", "Gospel Perc"],
        );
        let said = open.twice();
        assert!(
            said.iter().any(|word| word == "1 entry needs attention"),
            "{said:?}"
        );
    }

    /// Bytes that do not decode still open a document that says so.
    #[test]
    fn a_file_that_did_not_decode_still_paints() {
        let said = Open::file("junk.bin", junk_bytes()).twice();
        assert!(
            said.iter().any(|word| word.contains("did not decode")),
            "the record is all these bytes have: {said:?}"
        );
    }

    /// Bytes that are neither a format this app reads nor a note.
    fn junk_bytes() -> Vec<u8> {
        vec![0x00, 0xff, 0x01, 0xfe]
    }

    /// One second of 44.1 kHz mono, long enough for the encoder's shortest stroke.
    fn wav_bytes() -> Vec<u8> {
        let samples: Vec<i16> = (0..codec::SOURCE_RATE as usize)
            .map(|i| ((i as f64 / 40.0).sin() * 12_000.0) as i16)
            .collect();
        nord_format::wav::mono_pcm16(&samples, codec::SOURCE_RATE).unwrap()
    }

    /// A WAV has no field table and no capabilities to list, so its Advanced face is
    /// the record every asset has.
    #[test]
    fn the_advanced_face_of_a_wav_is_the_record() {
        let mut open = Open::file("Marimba hit.wav", wav_bytes());
        open.document.views.insert(open.id, Face::Advanced);
        let said = open.twice();
        for section in ["Container", "Changes"] {
            assert!(
                said.iter().any(|word| word == section),
                "{section}: {said:?}"
            );
        }
    }

    #[test]
    fn a_wav_offers_an_encode_and_is_left_unchanged() {
        let bytes = wav_bytes();
        let mut open = Open::file("Marimba hit.wav", bytes.clone());
        assert_eq!(
            faces(shape(open.entity()))
                .iter()
                .map(|face| face.label())
                .collect::<Vec<_>>(),
            ["Basic", "Advanced"],
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

        assert!(document.refusal().is_none(), "{:?}", document.refusal());
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
        let stamp = open.entity().stamp;
        open.document.audio.follow(id, stamp);

        assert!(
            open.document.audio.get(0).is_none(),
            "nothing decodes until asked"
        );
        for _ in 0..2 {
            open.document.answer(
                id,
                Asked::Zone(sample::Ask::Decode(0)),
                &mut open.workspace,
                &mut open.log,
            );
        }
        {
            let decoded = open.document.audio.get(0).expect("asked for");
            let decoded = decoded.as_ref().unwrap();
            assert!(decoded.audio.seconds() > 0.5);
            assert_eq!(decoded.audio.channels, 1);
            assert!(!decoded.envelope.is_empty());
        }
        // A frame with a decoded zone is the only path that paints its envelope.
        open.frame(Vec::new());

        assert_eq!(
            open.document.instrument_name(id, &open.workspace),
            "Marimba"
        );
        assert_eq!(
            crate::workspace::zone_wav_name(&open.document.instrument_name(id, &open.workspace), 1),
            "Marimba-zone1.wav",
        );

        // Bytes replaced from outside the editor may hold other strokes, so audio
        // decoded from the old bytes is dropped.
        let edited =
            sample::apply(&open.entity().bytes, &[("name".into(), "Vibes".into())]).unwrap();
        open.workspace.replace_bytes(id, edited, &mut open.log);
        let stamp = open.entity().stamp;
        open.document.audio.follow(id, stamp);
        assert!(
            open.document.audio.get(0).is_none(),
            "audio decoded from stale bytes is dropped"
        );
        assert_eq!(open.document.instrument_name(id, &open.workspace), "Vibes");
    }

    /// A three-zone project from the `nord-format` fixtures, written by this project's
    /// tools.
    fn project_bytes() -> Vec<u8> {
        include_bytes!("../../../nord-format/tests/fixtures/nsmpproj/three-zones.nsmpproj").to_vec()
    }

    /// Basic shows the panel, and Advanced the record with the format's capability
    /// table.
    #[test]
    fn an_instrument_and_a_project_offer_both_faces() {
        for (name, bytes) in [
            ("Marimba.nsmp", sample_bytes()),
            ("clarinet.nsmpproj", project_bytes()),
        ] {
            let open = Open::file(name, bytes);
            let entity = open.entity();
            let registry = entity.entity.as_ref().and_then(fields::fields_of);
            assert!(registry.is_none(), "{name} declares no field registry");
            assert_eq!(
                faces(shape(entity))
                    .iter()
                    .map(|face| face.label())
                    .collect::<Vec<_>>(),
                ["Basic", "Advanced"],
                "{name}",
            );
        }
    }

    /// Both faces of an instrument and of a project paint, two frames each.
    #[test]
    fn a_project_document_paints_on_every_face() {
        for face in [Face::Basic, Face::Advanced] {
            render_file("clarinet.nsmpproj", project_bytes(), face);
            render_file("Marimba.nsmp", sample_bytes(), face);
        }
    }

    /// ⚠️ Inside the scroll area the key map would scroll away, and it is how a zone is
    /// picked.
    #[test]
    fn the_key_map_is_painted_above_the_scrolling_rows() {
        let mut open = Open::file("Marimba.nsmp", sample_bytes());
        open.frame(Vec::new());
        let output = open.output(Vec::new());

        let placed = |word: &str| -> (egui::Rect, egui::Rect) {
            leaves(&output)
                .into_iter()
                .find_map(|(clip, shape)| match shape {
                    egui::Shape::Text(text) if text.galley.text() == word => Some((
                        egui::Rect::from_min_size(text.pos, text.galley.size()),
                        clip,
                    )),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{word} was never painted"))
        };

        let (map, pinned) = placed("Key map");
        let (_, scrolling) = placed("Zones");
        assert_ne!(pinned, scrolling, "the map shares the rows' clip region");
        assert!(
            map.bottom() <= scrolling.top(),
            "the map at {map:?} overlaps the rows' region {scrolling:?}",
        );
    }

    /// An open row asks for its zone's decode: one frame says it is reading, the next
    /// decodes. Nothing decodes while every row is closed, and the cache keeps later
    /// frames from decoding again.
    #[test]
    fn an_open_zone_draws_its_own_waveform() {
        let mut open = Open::file("Marimba.nsmp", sample_bytes());
        let said = open.twice();
        assert!(
            open.document.audio.get(0).is_none(),
            "a closed row decodes nothing: {said:?}"
        );

        sample::pick_row(&mut open.state().sample, 0);
        let said = open.frame(Vec::new());
        assert!(
            said.iter().any(|word| word == "reading the zone…"),
            "the row says what it is waiting on: {said:?}"
        );
        assert!(open.document.audio.get(0).is_none(), "{said:?}");
        open.frame(Vec::new());
        let decoded = open
            .document
            .audio
            .get(0)
            .expect("the open row asked for the decode")
            .as_ref()
            .expect("the zone decodes");
        assert!(!decoded.envelope.is_empty());
        let said = open.frame(Vec::new());
        assert!(
            said.iter().any(|word| word == "Save WAV…"),
            "the frame after the decode draws the audio: {said:?}"
        );
    }

    /// Moving a zone's keys does not change its stroke, so dragging across the key map
    /// or stepping the root note keeps the open row's waveform without decoding again.
    #[test]
    fn an_edit_that_leaves_the_strokes_alone_keeps_the_zones_audio() {
        let mut open = Open::file("Marimba.nsmp", sample_bytes());
        open.frame(Vec::new());
        sample::pick_row(&mut open.state().sample, 0);
        open.twice();
        let before: *const sample::Decoded = open
            .document
            .audio
            .get(0)
            .expect("the open row decoded")
            .as_ref()
            .expect("the zone decodes");

        let stamp = open.entity().stamp;
        let id = open.id;
        for root in ["D4", "E4"] {
            open.document
                .apply(
                    id,
                    vec![("zone1.root_key".into(), root.into())],
                    &mut open.workspace,
                    &mut open.log,
                )
                .expect("the root key is settable");
            let said = open.frame(Vec::new());
            assert!(
                !said.iter().any(|word| word == "reading the zone…"),
                "root {root}: {said:?}"
            );
        }
        assert_ne!(open.entity().stamp, stamp, "the edits made new bytes");
        let after: *const sample::Decoded = open
            .document
            .audio
            .get(0)
            .expect("the audio outlived the edits")
            .as_ref()
            .expect("the zone decodes");
        assert!(std::ptr::eq(before, after), "the zone was decoded again");
    }

    /// ⚠️ A zone index belongs to the instrument it was opened on. Leaving the tab drops
    /// the selection, the open rows, and the struck key. Edits are already on the
    /// working copy and stay.
    #[test]
    fn leaving_a_document_forgets_the_open_zone_and_keeps_the_edit() {
        let mut open = Open::file("Marimba.nsmp", sample_bytes());
        open.frame(Vec::new());
        sample::pick_row(&mut open.state().sample, 0);
        assert_eq!(sample::selected(&open.state().sample), Some(0));

        let edited = sample::apply(
            &open.entity().bytes,
            &[("zone1.top_note".into(), "C6".into())],
        )
        .unwrap();
        open.workspace.replace_bytes(open.id, edited, &mut open.log);

        // Switch to another document and back, as the tab strip does.
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
            sample::selected(&open.state().sample),
            None,
            "the selection does not survive a switch"
        );
        let snapshot = sample::snapshot(open.entity().entity.as_ref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.zones[0].top_note, 84, "the edit survives");
    }

    fn piano_bytes() -> Vec<u8> {
        nord_format::formats::npno::synthetic::Build::new()
            .bytes()
            .expect("the builder lays out a library")
    }

    #[test]
    fn a_piano_document_offers_every_face_and_paints_on_each_of_them() {
        let mut open = Open::file("Test Piano.npno", piano_bytes());
        assert_eq!(
            faces(shape(open.entity()))
                .iter()
                .map(|face| face.label())
                .collect::<Vec<_>>(),
            ["Basic", "Advanced"],
        );

        for face in [Face::Basic, Face::Advanced] {
            for dark in [true, false] {
                let mut open = Open::file("Test Piano.npno", piano_bytes());
                open.ctx.set_theme(match dark {
                    true => egui::ThemePreference::Dark,
                    false => egui::ThemePreference::Light,
                });
                open.document.views.insert(open.id, face);
                let said = open.twice();
                let has = |word: &str| said.iter().any(|held| held == word);
                assert!(has("Test Piano"), "{face:?} in {dark}: {said:?}");
                match face {
                    Face::Basic => assert!(has("Trim to fit"), "{said:?}"),
                    Face::Advanced => {
                        assert!(has("About this file") && has("Container"), "{said:?}");
                        assert!(has("What this format holds"), "{said:?}");
                        assert!(!has("Key map"), "the map is only on Basic: {said:?}");
                    }
                }
            }
        }
        // Basic shows the map.
        open.document.views.insert(open.id, Face::Basic);
        assert!(open.twice().iter().any(|word| word == "Key map"));
    }

    /// ⚠️ A library is hundreds of megabytes, so an edit to one is a plan and the bytes
    /// are left alone. An act that would carry the bytes waits until the plan is laid
    /// out, and is released as soon as it is.
    #[test]
    fn a_piano_lays_its_plan_out_before_anything_carries_its_bytes() {
        let mut open = Open::file("Test Piano.npno", piano_bytes());
        let named = |open: &Open| {
            piano::snapshot(open.entity().entity.as_ref().unwrap())
                .unwrap()
                .unwrap()
                .name
        };
        open.frame(Vec::new());
        open.frame(vec![click(NAME_BOX)]);
        open.frame(vec![egui::Event::Text("X".to_string())]);
        open.frame(vec![enter()]);

        assert!(open.document.pends(open.id), "the rename is a plan");
        assert_eq!(
            open.entity().bytes,
            open.entity().saved.bytes,
            "and nothing has copied the body for it",
        );
        assert_eq!(named(&open), "Test Piano", "nor written it");

        let ctx = open.ctx.clone();
        let mut acts = open.document.settle(
            &ctx,
            vec![crate::browser::Act::SaveDoc(open.id)],
            &mut open.workspace,
            &mut open.log,
        );
        assert!(
            acts.is_empty() || !open.document.pends(open.id),
            "the save waits for the apply",
        );
        if acts.is_empty() {
            let applied = open
                .document
                .piano
                .awaited(&ctx, &open.workspace)
                .expect("the apply the save waits on is in flight");
            acts = open
                .document
                .put_back(applied, &mut open.workspace, &mut open.log);
        }
        assert!(
            matches!(acts.as_slice(), [crate::browser::Act::SaveDoc(id)] if *id == open.id),
            "the held save is released",
        );
        assert!(named(&open).contains('X'), "{}", named(&open));
        assert!(open.entity().is_unsaved(), "only the save clears unsaved");
        assert!(!open.document.pends(open.id));

        open.workspace.mark_saved(open.id);
        assert!(
            !open.entity().is_unsaved(),
            "the saved bytes include the plan",
        );
    }

    /// A pending plan marks the asset unsaved, which offers Revert. A revert drops the
    /// plan whichever control raised it; the File menu raises the same act as the
    /// header.
    #[test]
    fn reverting_a_piano_from_the_menu_drops_the_plan_it_was_holding() {
        let mut open = Open::file("Test Piano.npno", piano_bytes());
        open.frame(Vec::new());
        open.frame(vec![click(NAME_BOX)]);
        open.frame(vec![egui::Event::Text("X".to_string())]);
        open.frame(vec![enter()]);
        assert!(open.document.pends(open.id));
        assert!(
            open.entity().is_unsaved(),
            "which offers Revert to saved in the menu",
        );
        assert_eq!(
            open.entity().bytes,
            open.entity().saved.bytes,
            "though no body was copied for it",
        );

        let ctx = open.ctx.clone();
        let acts = open.document.settle(
            &ctx,
            vec![crate::browser::Act::Revert(open.id)],
            &mut open.workspace,
            &mut open.log,
        );
        assert!(
            matches!(acts.as_slice(), [crate::browser::Act::Revert(id)] if *id == open.id),
            "the revert itself still runs",
        );
        assert!(!open.document.pends(open.id), "the revert drops the plan");

        crate::browser::apply(
            &mut crate::browser::Browser::default(),
            &mut crate::shell::Shell::default(),
            acts,
            &mut open.workspace,
            &mut open.device,
            &mut crate::tabs::Tabs::default(),
            &mut open.queue,
            &mut open.log,
        );
        assert!(
            !open.entity().is_unsaved(),
            "the asset is back to its saved bytes",
        );
        assert!(
            open.log.status().1.contains("back as it was last saved"),
            "{}",
            open.log.status().1,
        );
    }

    #[test]
    fn a_sample_document_paints() {
        let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
        let options = nord_format::formats::nsmp::encode::Options::new("Marimba");
        let bytes = nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap();
        render_file("Marimba.nsmp", bytes.clone(), Face::Basic);
        render_file("Marimba.nsmp", bytes, Face::Advanced);
        render_file("Marimba hit.wav", wav_bytes(), Face::Basic);
    }
}
