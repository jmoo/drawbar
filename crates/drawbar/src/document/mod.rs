//! The document: one view of one asset.
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
mod table;
mod verbatim;

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

/// What a document is, which decides the body it draws, the faces it offers, what its
/// header says and what an edit to it becomes.
///
/// ⚠️ One answer per frame, from an exhaustive match on [`nord_format::Entity`]: a
/// family the library adds is a compile error here rather than a document that quietly
/// loses a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    /// A body whose fields the generated registry declares.
    Fields,
    /// An Electro 5 set list: the four programs it points at are the whole of it.
    SetList,
    Sample,
    Project,
    /// An `npno` piano library, edited as a plan over bytes nothing copies.
    Piano,
    /// A body no registry describes, kept byte for byte.
    Verbatim,
    /// Bytes that did not decode, and are audio an instrument can be built from.
    Wav,
    /// Bytes that did not decode.
    Undecoded,
}

/// One asset as a frame reads it: what it holds, and what that makes it. The two travel
/// together so that nothing can draw one asset's body from another's shape.
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

/// Which of the shapes this asset is.
fn shape(entity: &LocalEntity) -> Shape {
    use nord_format::Entity as E;

    let Some(decoded) = &entity.entity else {
        return match encode::is_wav(&entity.bytes) {
            true => Shape::Wav,
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
        // ⚠️ A Stage Classic piano library is not a [`Shape::Piano`]: `npno` is the one
        // library that decodes into strokes, and the rest is a container over a body
        // this app can only keep as it found it.
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
    /// The thing a body pointed at: a set list's entry is a program of its own, and
    /// opening it is the browser's act, not the document's.
    pub open: Option<crate::browser::Item>,
}

/// What the Basic face asked for that cannot be done while the asset is borrowed to
/// draw it: audio, or a new asset made out of this one.
enum Asked {
    Zone(sample::Ask),
    Root(piano::Ask),
    Encode,
    /// The copy a body with nothing to edit offers, which is the header's Export.
    Export,
    Open(crate::browser::Item),
    /// The Advanced link under a section the instrument is not using.
    Advanced,
}

/// What one open document keeps between frames.
///
/// ⚠️ None of it outlives the target it was opened on, so a switch replaces the whole of
/// it. A field cleared by hand at the door is a field that will one day be forgotten
/// there, and half-typed values then follow the operator into the next tab.
struct Opened {
    /// The asset this is open on.
    id: u64,
    /// Per-field legal values and controls, cached as they are drawn — see [`Ctx`].
    ctx: Ctx,
    /// The header's name box, so a half-typed name survives a frame, and the piano's
    /// variant box beside it.
    name: String,
    variant: String,
    /// The path boxes for a project's audio files, by file id — same reason.
    paths: std::collections::HashMap<u32, String>,
    /// The last refusal, and what caused it.
    error: Option<String>,
    /// The encode panel over a WAV, and the read of the WAV it works from.
    wav: Option<(encode::Draft, encode::Source)>,
    /// What the instrument editor keeps between frames: the open zone, the struck key,
    /// the folded key table. Never an edit — an edit is on the working copy at once.
    sample: sample::State,
    /// What the field document keeps between frames: the morph lens, where the reader
    /// is, and the two decodes a pending count is measured across. Never an edit.
    fields: field::State,
    /// What the set list editor keeps: the half-typed address boxes and whether a
    /// reorder has been made.
    list: setlist::State,
}

impl Opened {
    /// Open a document on `asset`.
    ///
    /// ⚠️ Reading a WAV copies every sample, so it happens here and never per frame —
    /// the encode panel works from what is read once.
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
                | Shape::Verbatim
                | Shape::Undecoded => None,
            },
            sample: sample::State::default(),
            fields: field::State::default(),
            list: setlist::State::default(),
        }
    }
}

#[derive(Default)]
pub struct Document {
    /// What the document in front of the reader is keeping, and which asset it is on.
    open: Option<Opened>,
    /// Which face each document was left on.
    views: std::collections::HashMap<u64, Face>,
    /// The engineering table's filter and cell, and the decode it last laid out. One
    /// table serves every tab — see [`Advanced::leave`].
    advanced: Advanced,
    /// Zone audio decoded on request, dropped when the bytes under it change.
    audio: sample::Cache,
    /// Which zone is sounding, and the one backend that makes it sound.
    player: crate::audio::Player,
    /// The piano library's plan, the facts it is a plan over, and its decoded strokes.
    ///
    /// ⚠️ Not one document's. An apply runs on a thread of its own and the acts it holds
    /// come back after the tab it was started in may have gone — see [`Document::settle`].
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
            // ⚠️ Leaving the tab is leaving the sound: a zone that goes on playing over
            // another document is a sound with nothing on screen to stop it.
            self.player.stop();
            self.piano.leave();
        }
        // Decoded audio belongs to one set of bytes; an edit re-encodes all of them.
        self.audio.follow(id, entity.stamp);
        // Paint marks are measured against the bytes the asset was last saved as.
        if let Some(open) = &mut self.open {
            sample::follow(&mut open.sample, id, &entity.saved);
        }
        self.player.settle();
        if let Some(left) = self.piano.settle(ui.input(|input| input.time)) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
        if self.player.playing().is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        // An apply says where it is from another thread, and a header frozen on
        // `applying…` is a window that looks hung.
        if self.piano.applying() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }

        let faces = faces(shape);
        let face = showing(&faces, self.views.get(&id).copied().unwrap_or_default());

        // ⚠️ Only a registry body. Reading the saved bytes means decoding them, and a
        // piano library is hundreds of megabytes with no field in it.
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
        let mut asked = None;
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
            if let Some(why) = self.open.as_ref().and_then(|open| open.error.as_ref()) {
                ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
            }
            if face == Face::Basic {
                asked = self.pinned(ui, asset, doc.as_ref(), &mut sets);
            }
            egui::ScrollArea::vertical()
                .id_salt(SCROLL)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    // ⚠️ Widget state keyed only by field path leaks between tabs of
                    // the same format, so every control also answers to the document id.
                    ui.push_id(id, |ui| match face {
                        Face::Basic => {
                            if let Some(from_body) = self.body(
                                ui,
                                asset,
                                doc.as_ref(),
                                &mut lookup,
                                &setlist::Catalogue {
                                    device: &device.state,
                                    workspace,
                                },
                                &mut sets,
                            ) {
                                asked = Some(from_body);
                            }
                        }
                        Face::Advanced => {
                            match shape {
                                Shape::Fields => {
                                    if let (Some(doc), Some(open)) =
                                        (doc.as_ref(), self.open.as_ref())
                                    {
                                        Advanced::about(ui, &field::about(doc, entity));
                                        let table = advanced::Table {
                                            fields: registry.as_deref().unwrap_or_default(),
                                            saved: open.fields.settled(),
                                            changed: open.fields.pending(),
                                            doc: Some(doc),
                                        };
                                        self.advanced.table(ui, &table, &mut sets);
                                        typed = !sets.is_empty();
                                    }
                                }
                                Shape::Piano => {
                                    self.piano.meta(ui);
                                    self.piano.advanced(ui);
                                }
                                Shape::SetList
                                | Shape::Sample
                                | Shape::Project
                                | Shape::Verbatim
                                | Shape::Wav
                                | Shape::Undecoded => {
                                    record(ui, asset);
                                    capabilities(ui, asset);
                                }
                            }
                            details = self.advanced.meta(ui, entity, device)
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
        match asked {
            Some(Asked::Open(item)) => wants.open = Some(item),
            Some(Asked::Advanced) => {
                self.views.insert(id, Face::Advanced);
            }
            Some(Asked::Export) => self.export(ui.ctx(), id, workspace),
            Some(asked) => self.answer(id, asked, workspace, log),
            None => {}
        }
        if let Some((class, at)) = workspace.get(id).and_then(|e| e.origin.slot()) {
            match lookup.asked {
                true => device.ask_deps_again(class, at, log),
                false if lookup.wants_a_name() => device.read_deps(class, at, log),
                false => {}
            }
        }
        if let Some(name) = act.rename {
            // ⚠️ Only the name moves. The box already holds what was typed, and opening
            // the document again would throw away every other thing it is keeping —
            // among them a WAV's encode draft, which nothing else remembers.
            workspace.rename(id, name);
        }
        if act.export {
            self.export(ui.ctx(), id, workspace);
        }
        if act.revert {
            workspace.revert(id, log);
            self.piano.forget(id);
            // Opened again on the next frame: every box is holding an edit that is gone.
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
        // frame shows what the edit, or the plan it left standing, made of it.
        let edited = self.note_pending(id, workspace)
            || workspace.get(id).is_some_and(|held| held.stamp != stamp);
        if edited {
            ui.ctx().request_repaint();
        }
        wants
    }

    /// Take up the plan a piano library's frame left behind, where the library accepts
    /// it.
    ///
    /// ⚠️ Nothing is rebuilt here. The plan is checked against the baseline — a name the
    /// format refuses, or a switch that would leave the library with no strokes, is
    /// turned down now rather than refusing every later edit — and the bytes it makes
    /// are laid out only when something has to carry them: see [`piano::State::start`].
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

    /// Tell the workspace what this document is holding, and answer with whether that
    /// moved.
    ///
    /// The plan lives here, but unsaved is asked of the asset — see
    /// [`LocalEntity::is_unsaved`]. Run wherever a plan is taken up or laid out.
    fn note_pending(&self, id: u64, workspace: &mut Workspace) -> bool {
        workspace.mark_pending(id, self.pends(id))
    }

    /// Hold back the acts that would carry a piano library's bytes while its plan is
    /// still only a plan, and start the apply they are waiting for.
    ///
    /// ⚠️ Run after the frame's own acts are collected and before any of them are: an
    /// act let through here writes the bytes as they stand, which for a pending plan is
    /// the library before the trim.
    ///
    /// It polls before it answers, because on a target with one thread the apply runs
    /// where it is started and what it was holding comes back with this frame's acts.
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
                // Wherever the gesture came from: the saved bytes are about to be put
                // back or taken away, and a plan over bytes nothing holds is not an edit
                // of anything.
                if let crate::browser::Act::Revert(id) | crate::browser::Act::Remove(id) = &act {
                    self.piano.forget(*id);
                }
                self.piano.hold(ctx, act, workspace)
            })
            .collect();
        out.extend(self.released(ctx, workspace, log));
        out
    }

    /// The acts an apply that has answered was holding, and the bytes it made put back
    /// under the document.
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

    /// Hand the document's bytes to the user, once the plan in hand is in them.
    fn export(&mut self, ctx: &egui::Context, id: u64, workspace: &mut Workspace) {
        if self
            .piano
            .hold(ctx, crate::browser::Act::Export(id), workspace)
            .is_some()
        {
            workspace.export(id);
        }
    }

    /// The asset the document in front of the reader is open on.
    fn opened(&self) -> Option<u64> {
        self.open.as_ref().map(|open| open.id)
    }

    /// The refusal the open document is showing.
    #[cfg(test)]
    fn refusal(&self) -> Option<&str> {
        self.open.as_ref()?.error.as_deref()
    }

    /// Say what refused the last act, or that nothing did.
    ///
    /// The message belongs to the document showing it and goes when that document does,
    /// so with nothing open there is nowhere for it but the log it is already in.
    fn refused(&mut self, why: Option<String>) {
        if let Some(open) = &mut self.open {
            open.error = why;
        }
    }

    /// The root the speakers are on, where it is this document's.
    fn sounding_root(&self) -> Option<u8> {
        let (id, root) = self.player.playing()?;
        (Some(id) == self.opened()).then_some(())?;
        u8::try_from(root).ok()
    }

    /// Nothing is open any more.
    ///
    /// ⚠️ A zone goes on sounding until something stops it, and the control that would
    /// stop it is on the document. With no document there is nothing to click.
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
        seen: &setlist::Catalogue<'_>,
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
                let sounding = self.sounding_root();
                self.piano.ui(ui, sounding).map(Asked::Root)
            }
            Shape::Fields => {
                let open = self.open.as_mut()?;
                field::body(ui, &open.ctx, &mut open.fields, doc?, piano, sets)
                    .then_some(Asked::Advanced)
            }
            Shape::Verbatim => verbatim::ui(ui, asset.entity).then_some(Asked::Export),
        }
    }

    /// Bytes that did not decode: the encode panel where they are a WAV, and the plain
    /// report where they are anything else.
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
        let sounding = self.player.playing();
        let target = self.opened();
        let sounds: Vec<sample::Sound> = (0..snapshot.zones.len())
            .map(|index| sample::Sound {
                decoded: self.audio.get(index),
                playing: sounding == target.map(|id| (id, index)),
            })
            .collect();
        let open = self.open.as_mut()?;
        sample::ui(ui, &mut open.sample, &snapshot, &sounds, sets)
    }

    /// What an editor keeps in front of the body: above the scroll region, on the panel
    /// fill, so it stays where it is while the rows under it move.
    ///
    /// The instrument key map is what this region is for. A kind with nothing to pin
    /// takes up no room.
    fn pinned(
        &mut self,
        ui: &mut egui::Ui,
        asset: Asset<'_>,
        doc: Option<&field::Doc<'_>>,
        sets: &mut Sets,
    ) -> Option<Asked> {
        match asset.shape {
            Shape::Fields => {
                field::nav(ui, &mut self.open.as_mut()?.fields, doc?);
                None
            }
            Shape::Piano => self.piano.map(ui).map(Asked::Root),
            Shape::Sample => match sample::snapshot(asset.decoded()?)? {
                Ok(snapshot) => {
                    let open = self.open.as_mut()?;
                    sample::map(ui, &mut open.sample, &snapshot, sets).map(Asked::Zone)
                }
                Err(_) => None,
            },
            Shape::Project => {
                if let Some(Ok(snapshot)) = project::snapshot(asset.decoded()?) {
                    project::map(ui, &mut self.open.as_mut()?.sample, &snapshot, sets);
                }
                None
            }
            Shape::SetList | Shape::Verbatim | Shape::Wav | Shape::Undecoded => None,
        }
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
                if let Err(why) = self.player.strike(
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
                // ⚠️ An edit drops the decode of a zone that goes on sounding, so
                // stopping the one that is sounding cannot wait on audio in hand.
                if self.player.playing() == Some((id, zone)) {
                    self.player.stop();
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
            // ⚠️ Answered where the frame collects what it wants: the browser owns
            // opening a tab, the face is the frame's own to switch, and an export waits
            // on the plan — see [`Document::export`].
            Asked::Export | Asked::Open(_) | Asked::Advanced => {}
        }
    }

    /// Draw, hear or write one root of a piano library. The stroke is decoded on the
    /// way, once, because every answer needs it.
    fn root_audio(&mut self, id: u64, ask: piano::Ask, workspace: &mut Workspace, log: &mut Log) {
        let root = ask.root();
        let Some(entity) = workspace.get(id) else {
            return;
        };
        if let Err(why) = self.piano.decode(entity, root) {
            // ⚠️ An open row asks for its own waveform: it says why it has none, and
            // the log is for what the operator asked for.
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
            piano::Ask::Strike { semitones, .. } => {
                let rate = crate::audio::rate(semitones);
                if let Err(why) =
                    self.player
                        .strike((id, usize::from(root)), sound.samples, sound.channels, rate)
                {
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
        // ⚠️ Over the asset's own bytes, never a copy of them. A piano library is
        // hundreds of megabytes and every set of every frame comes through here; the
        // piano arm makes no bytes at all, because its sets land in a plan.
        let made = match shape(entity) {
            Shape::Sample => sample::apply(&entity.bytes, &sets).map(Some),
            Shape::Project => project::apply(&entity.bytes, &sets).map(Some),
            // A piano's sets land in its plan, and the plan is what makes its bytes —
            // see [`Document::replan`].
            Shape::Piano => self.piano.take(&sets).map(|()| None),
            Shape::SetList => setlist::apply(&entity.bytes, &sets).map(Some),
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
        // Bytes that did not move are not a new set of bytes, which `replace_bytes` is
        // what decides — and that is the one comparison of two bodies there is.
        if let Some(out) = made {
            workspace.replace_bytes(id, out, log);
        }
        Ok(())
    }
}

/// The faces this document offers, in the order the control shows them.
///
/// Advanced is always one of them — every asset has a record, even bytes that decoded
/// into nothing.
fn faces(shape: Shape) -> Vec<Face> {
    // A WAV decodes into nothing, but it is the one thing this app can make an
    // instrument out of, so it gets a panel rather than only a byte record.
    let panel = match shape {
        Shape::Fields
        | Shape::SetList
        | Shape::Sample
        | Shape::Project
        | Shape::Piano
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

/// What the strip shows instead of what it works out for itself.
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
                    &setlist::Catalogue {
                        device: &device.state,
                        workspace,
                    },
                )
            }),
            ..header::Extras::default()
        },
        Shape::Verbatim => {
            // The same write the strip works out for itself, in the words a body
            // nothing can edit calls it by.
            let mut loud = header::action(entity, &device.state);
            loud.label = "Send as-is".to_string();
            loud.short = "Send".to_string();
            header::Extras {
                loud: Some(loud),
                ..header::Extras::default()
            }
        }
        Shape::Sample | Shape::Project | Shape::Piano | Shape::Wav | Shape::Undecoded => {
            header::Extras::default()
        }
    }
}

/// The face to show: the one this document was last left on, where that face still
/// exists — the panel otherwise, and the record where there is no panel either.
fn showing(faces: &[Face], remembered: Face) -> Face {
    match faces.contains(&remembered) {
        true => remembered,
        false => faces.first().copied().unwrap_or(Face::Advanced),
    }
}

/// What the file says about itself, ahead of the container record every asset has.
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
        | Shape::Verbatim
        | Shape::Wav
        | Shape::Undecoded => {}
    }
}

/// The Advanced face of a body with no field registry: what the format holds and where
/// each field of it lands, the addresses a set list stores, or the bytes a body nothing
/// describes is keeping.
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
        Shape::Fields | Shape::Piano | Shape::Wav | Shape::Undecoded => {}
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
/// position — what turns the Model dial into a list of pianos.
///
/// Bank ↔ category and slot order ↔ dial position: Confirmed on hardware. The
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

        /// What the document is keeping for the target it is open on.
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
        render_view(sets, kind, Face::Basic);
    }

    /// Every kind gets the same strip, in both faces of the theme, and it names the
    /// faces in their own words.
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
                assert!(has("Basic"), "{kind:?} in {dark}: {said:?}");
                assert!(has("Advanced"), "{kind:?} in {dark}: {said:?}");
                assert!(has("Queue send"), "the loud action: {said:?}");
                assert!(has("Revert") && has("Export…"), "the quiet ones: {said:?}");
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
        assert!(full.contains(&"Basic".to_string()));
        assert!(full.contains(&"Queue send".to_string()));

        let quiet = at(900.0);
        assert!(!quiet.contains(&"Export…".to_string()), "{quiet:?}");
        assert!(
            quiet.contains(&"Basic".to_string()),
            "the faces keep theirs"
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

    /// A rename moves the asset's name and nothing else.
    ///
    /// ⚠️ The encode draft over a WAV is the one thing on a document that nothing else
    /// holds a copy of: a document rebuilt after the rename opens it back at the
    /// encoder's defaults, and what the operator picked is gone.
    #[test]
    fn renaming_a_wav_keeps_the_encode_draft_it_is_open_on() {
        let mut open = Open::file("Marimba hit.wav", wav_bytes());
        open.frame(Vec::new());
        let (draft, _) = open.state().wav.as_mut().expect("a WAV opens the panel");
        assert_ne!((draft.root_key, draft.top_note), (48, 60), "the defaults");
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

    /// ⚠️ A stored name box holds what the field holds, which is a number of **bytes**.
    /// A box counting characters takes an accented name of twice that length, and the
    /// format's refusal arrives only once the operator has typed the whole of it.
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
        assert!(stored.contains('é'), "what was typed landed: {stored:?}");
        assert!(stored.len() <= limit, "{} bytes: {stored:?}", stored.len());
        assert_eq!(
            open.document.refusal(),
            None,
            "the box never offers the field more than it holds"
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

    /// Advanced opens on what the file says about itself, and the table under it holds
    /// every field — the ones Basic hides included, counted where the reader can see how
    /// much of the body the other face leaves out.
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

    /// ⚠️ Every column of the Advanced face reads down from its own heading. A cell
    /// centred in the space its column keeps has no edge for the eye to follow, and a
    /// record read that way is read a row at a time.
    #[test]
    fn every_column_of_the_advanced_face_reads_down_from_its_heading() {
        let mut open = Open::fresh(Fresh::Program);
        open.document.views.insert(open.id, Face::Advanced);
        open.frame(Vec::new());
        let output = open.output(Vec::new());

        fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
                _ => {}
            }
        }
        let mut placed = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut placed);
        }
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
            assert_eq!(left(word), edge, "{word} left the column its label starts");
        }
        assert!(
            left("program v4") > edge,
            "the value column stands clear of it"
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
                "no {cell} cell stands under {head} at {under}",
            );
        }
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
    }

    /// The lens swaps every morphed control to what it becomes under one performance
    /// control, and says so above the sections.
    #[test]
    fn the_morph_lens_shows_what_a_control_becomes_under_the_wheel() {
        let mut open = Open::file("blank.ns4y", Fresh::Stage4Synth.bytes().unwrap());
        open.set(&[("synth_a_volume", "40"), ("synth_a_volume_wheel", "211")]);
        let panel = open.twice();
        assert!(
            !panel.iter().any(|word| word == "211"),
            "the panel shows the panel value: {panel:?}"
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
    /// knob — unpolished has to look unpolished. A cell wears its name in caps.
    #[test]
    fn a_path_with_no_label_yet_reads_as_its_prettified_self() {
        let said = Open::file("blank.ns4y", Fresh::Stage4Synth.bytes().unwrap()).twice();
        assert!(!strings::known("synth_a_volume"));
        assert!(
            said.iter().any(|word| word == "SYNTH A VOLUME"),
            "{:?}",
            &said[..said.len().min(40)]
        );
    }

    /// ⚠️ Every section of a Stage program is open, and the nav names each of them.
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
                "{title} was painted {drawn} times; the nav chip and the heading are two",
            );
        }
    }

    /// The engineer's face paints, filters and holds an edit — for a body with ninety
    /// fields and for one with forty — and with it the record: the container grid, the
    /// byte diff with something in it and with nothing, and the folded dump.
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

    /// Anything with a panel offers both faces; bytes that decoded into nothing have
    /// only the deep one, which is where their record is.
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
            b"not a nord file".to_vec(),
            &mut log,
        );
        assert_eq!(offered(&workspace, junk), ["Advanced"]);
    }

    /// A document opens on the face it was left on — the panel where it has never been
    /// left on one — and on something that has no such face falls back rather than
    /// showing an empty page.
    #[test]
    fn a_document_falls_back_to_a_face_it_actually_has() {
        let both = [Face::Basic, Face::Advanced];
        assert_eq!(showing(&both, Face::default()), Face::Basic);
        assert_eq!(showing(&both, Face::Advanced), Face::Advanced);

        let record_only = [Face::Advanced];
        for left_on in [Face::Basic, Face::Advanced] {
            assert_eq!(showing(&record_only, left_on), Face::Advanced);
        }
    }

    /// A cell the library refuses stays open with what was typed in it, because that is
    /// the only copy of what the operator meant.
    #[test]
    fn a_refused_cell_keeps_its_error() {
        let mut open = Open::fresh(Fresh::Program);
        open.frame(Vec::new());
        let (id, before) = (open.id, open.entity().bytes.clone());

        // What the table does with the library's answer, which is the part worth
        // pinning: the same call the frame makes.
        let refused = open.document.apply(
            id,
            vec![("center_panel.gain".into(), "200".into())],
            &mut open.workspace,
            &mut open.log,
        );
        assert!(refused.is_err());
        assert!(
            refused.as_ref().unwrap_err().contains("0 .. 127"),
            "the library's own words reach the operator: {refused:?}"
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

    /// A set that spells a field the way it is already spelled is not an edit: the
    /// asset keeps the bytes it had, and the stamp anything cached over them answers to.
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
        assert_eq!(entity.stamp, stamp, "nothing new landed under this id");
        assert!(!entity.is_unsaved());
    }

    /// ⚠️ The strip and the body are two scroll regions in one `Ui`, and each answers
    /// to an id of its own: on one id they share one state, and a wheel over the
    /// document moves the tab strip.
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

    /// A document opened off a slot asks once what that slot plays, and keeps the
    /// piano's name after another slot is read.
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
        assert_eq!(reads(&open), 1, "and asked once, not once a frame");

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
            "the name stands after another slot's read"
        );
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

    /// ⚠️ A cell's Enter is the cell's. A cell the library refused stays open holding
    /// what was typed, and an Enter read from the window submitted it again — and
    /// logged the refusal again — wherever the operator was typing at the time.
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
        assert!(
            open.document.refusal().is_some(),
            "the library turned 200 down"
        );
        assert_eq!(
            open.document.advanced.editing(),
            Some("center_panel.gain"),
            "and the cell keeps what was typed"
        );

        // The focus is the header's name box now; the cell is still open behind it.
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

    /// The Stage bodies have no panel of their own here, so they get the generic one: the
    /// big ones as folds, the small ones open with every control drawn.
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

    /// An Electro 5 set list has a view of its own — the four entries — and it does not
    /// come from the registry, which lists nothing for that body. Every face of it
    /// paints.
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

    /// A body no registry describes says which of the two silences it is — nothing to
    /// draw, rather than nothing read — states what the container does say, and shows
    /// the bytes it is keeping.
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
        assert!(has("Save a copy…"), "the one thing left to do: {said:?}");
        assert!(
            has("Send as-is"),
            "the loud action is the same send in this body's words: {said:?}"
        );
        assert!(has("0000") && has("0020"), "the body as hex: {said:?}");

        open.document.views.insert(open.id, Face::Advanced);
        let said = open.twice();
        assert!(
            said.iter().any(|word| word == "Body bytes"),
            "the whole body has a face of its own: {said:?}"
        );
        assert!(said.iter().any(|word| word == "3 rows"), "{said:?}");
    }

    /// A Stage Classic piano library (`nsp`): a container over a body nothing here
    /// decodes. Built rather than committed — a stub container is a zeroed body under
    /// the format's own tag.
    fn piano_library_bytes() -> Vec<u8> {
        use nord_format::cbin::{Cbin, Header, RawBody};
        use nord_format::formats::nsclassic;

        let file = Cbin {
            header: Header::new(nsclassic::piano_library::FORMAT, (0, 0), 0),
            body: RawBody(vec![0u8; 48]),
        };
        nord_format::to_bytes(&nord_format::Entity::PianoLibrary(file)).expect("a stub encodes")
    }

    /// Each editor claims the bodies it has a view for, and everything else is the
    /// verbatim body.
    ///
    /// ⚠️ A Stage Classic piano library is a verbatim body, not a [`Shape::Piano`]:
    /// `npno` is the one library this app decodes into strokes.
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
        assert_eq!(held(b"not a nord file".to_vec()), Shape::Undecoded);
    }

    /// A piano library this app cannot decode is a document like any other body it can
    /// only keep as it found it: the page saying so, the bytes under it, and the send
    /// in that page's own words.
    #[test]
    fn a_stage_classic_piano_library_wears_the_verbatim_faces() {
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

    /// A set list's header says what the list amounts to, and only when that is
    /// something to look at: nothing read is not four problems.
    #[test]
    fn a_set_lists_header_claims_only_what_the_instrument_showed() {
        let mut open = Open::file("Blue Room.ne5t", Fresh::SetList.bytes().unwrap());
        let said = open.twice();
        assert!(
            !said.iter().any(|word| word.contains("needs attention")),
            "nothing is attached, so nothing is claimed: {said:?}"
        );

        // Bank 1 read, and the slot the second entry names is vacant.
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

    /// A WAV has no field table and no capabilities to list, so its Advanced face is
    /// the record every asset has — which is the one page the merged face must not have
    /// dropped.
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
    fn a_wav_offers_an_encode_and_leaves_itself_alone() {
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

    /// An instrument and the project it is built from each have both faces: the panel,
    /// and the record with the capability table that says what the format holds.
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

    /// Every face of an instrument and of a project paints, twice over.
    #[test]
    fn a_project_document_paints_on_every_face() {
        for face in [Face::Basic, Face::Advanced] {
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

    /// ⚠️ An open zone shows what its stroke sounds like, so the row asks for the
    /// decode itself. Nothing decodes while every row is closed, and the cache is what
    /// keeps a frame from decoding again.
    #[test]
    fn an_open_zone_draws_its_own_waveform() {
        let mut open = Open::file("Marimba.nsmp", sample_bytes());
        let said = open.twice();
        assert!(
            open.document.audio.get(0).is_none(),
            "a closed row decodes nothing: {said:?}"
        );

        sample::pick_row(&mut open.state().sample, 0);
        let said = open.twice();
        let decoded = open
            .document
            .audio
            .get(0)
            .expect("the open row asked for the decode")
            .as_ref()
            .expect("the zone decodes");
        assert!(!decoded.envelope.is_empty());
        assert!(
            said.iter().any(|word| word == "Save WAV…"),
            "the second frame drew the audio the first asked for: {said:?}"
        );
    }

    /// ⚠️ A zone index belongs to the instrument it was opened on. Leaving the tab
    /// drops the selection, the open rows and the struck key — and nothing else: an
    /// edit is on the working copy already.
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
            sample::selected(&open.state().sample),
            None,
            "the selection is the instrument's, not the editor's"
        );
        let snapshot = sample::snapshot(open.entity().entity.as_ref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.zones[0].top_note, 84, "the edit stands");
    }

    fn piano_bytes() -> Vec<u8> {
        nord_format::formats::npno::synthetic::Build::new()
            .bytes()
            .expect("the builder lays out a library")
    }

    /// A piano library is a document like any other: it offers both faces, and the
    /// panel that decides what goes on the instrument is one of them.
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
                        assert!(has("About this file"), "{said:?}");
                        assert!(has("What this format holds") && has("Offsets"), "{said:?}");
                        assert!(!has("Key map"), "the map is the Basic face's: {said:?}");
                    }
                }
            }
        }
        // And the map is pinned above the body rather than drawn inside it.
        open.document.views.insert(open.id, Face::Basic);
        assert!(open.twice().iter().any(|word| word == "Key map"));
    }

    /// ⚠️ A library is hundreds of megabytes, so an edit to one is a plan and the bytes
    /// are left alone. What would carry those bytes waits for the plan to be laid over
    /// them — and is let go the moment it has been.
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
            "the save is what comes back",
        );
        assert!(named(&open).contains('X'), "{}", named(&open));
        assert!(open.entity().is_unsaved(), "and it is the save's to settle");
        assert!(!open.document.pends(open.id));

        open.workspace.mark_saved(open.id);
        assert!(
            !open.entity().is_unsaved(),
            "which settles it: the plan is in the bytes the save wrote",
        );
    }

    /// A plan the bytes do not hold is what the asset is unsaved for, which is what
    /// offers the revert — and a revert is the end of a plan wherever the gesture came
    /// from: the File menu raises the same act the header's own control does.
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
            "which is what puts Revert to saved in the menu",
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
        assert!(!open.document.pends(open.id), "the plan is gone with it");

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
            "and what is left is what it was saved as",
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
