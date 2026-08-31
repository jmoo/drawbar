//! The document: one view of one asset, which looking at and changing are the same act.
//!
//! An edit lands on the tab's working copy the moment it is made — set the field,
//! re-encode, re-check the bytes — and the tab goes dirty. Nothing on the instrument
//! moves until the header's Send does it. Revert goes back to the bytes the tab opened
//! with, which is the only undo there is.

use eframe::egui;
use nord_format::fields::Field;
use nord_format::formats::nsmp::codec;
use nord_usb::{Location, ObjectClass};

use crate::app::dot;
use crate::device::Device;
use crate::fields;
use crate::log::Log;
use crate::strings;
use crate::workspace::{LocalEntity, Workspace};

mod advanced;
mod controls;
mod encode;
mod panel;
mod project;
mod sample;
mod setlist;

use advanced::Advanced;
use controls::{Ctx, Sets};

pub use sample::note_picker;

/// The body's own scroll id — see [`crate::tabs::SCROLL`].
pub const SCROLL: &str = "document_body";

/// A put the header asked for. The browser owns the question it may need to raise.
pub struct SendBack {
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
}

/// What the header row asked for this frame.
#[derive(Default)]
struct Header {
    revert: bool,
    save: bool,
    send: Option<SendBack>,
}

/// What the Basic view asked for that cannot be done while the asset is borrowed to
/// draw it: audio, or a new asset made out of this one.
enum Asked {
    Zone(sample::Ask),
    Encode,
}

/// Which face of a document is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// The panel, in the instrument's own words. Happy-path edits.
    #[default]
    Basic,
    /// The whole body as a table. Nothing hidden.
    Advanced,
    /// The record: the container, the bytes that moved, and what the instrument says
    /// about the slot. Nothing here is a control.
    Meta,
}

#[derive(Default)]
pub struct Document {
    target: Option<u64>,
    /// Which face each document was left on.
    views: std::collections::HashMap<u64, View>,
    /// Per-field legal values and controls, cached as they are drawn. One per document —
    /// see [`Ctx`].
    ctx: Ctx,
    /// The name box for a sample instrument or a Sample Editor project, so a
    /// half-typed name survives a frame.
    name: String,
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
    /// Which zone is sounding, and the one backend that makes it sound.
    player: crate::audio::Player,
    /// The encode panel over a WAV, and the read of the WAV it works from.
    wav: Option<(encode::Draft, encode::Source)>,
}

impl Document {
    /// Draw the open tab's document. `opened` is what it looked like when the tab
    /// opened — Revert's target, and what the byte diff is measured against.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        opened: &[u8],
        workspace: &mut Workspace,
        device: &mut Device,
        log: &mut Log,
    ) -> Option<SendBack> {
        let entity = workspace.get(id)?;
        let decoded = entity.entity.as_ref();
        let registry = decoded.map(fields::fields_of).unwrap_or_default();

        if self.target != Some(id) {
            self.target = Some(id);
            self.error = None;
            self.fetched_deps = false;
            self.advanced.leave();
            self.ctx = Ctx::default();
            self.name = decoded
                .and_then(sample::snapshot)
                .and_then(Result::ok)
                .map(|snapshot| snapshot.name)
                .or_else(|| {
                    decoded
                        .and_then(project::snapshot)
                        .and_then(Result::ok)
                        .map(|snapshot| snapshot.name)
                })
                .unwrap_or_default();
            self.paths.clear();
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
        self.player.settle();
        if self.player.playing().is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }

        let faces = faces(entity, registry.as_deref());
        let view = showing(&faces, self.views.get(&id).copied().unwrap_or_default());

        let mut sets: Sets = Vec::new();
        let mut act = Header::default();

        self.header(ui, entity, opened, &mut act);
        let mut want = view;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right to left, so the labels read in their own order left to right.
                for (face, label) in faces.iter().rev() {
                    if ui.selectable_label(view == *face, *label).clicked() {
                        want = *face;
                    }
                }
            });
        });
        self.views.insert(id, want);
        if let Some(why) = &self.error {
            ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
        }
        ui.separator();

        let mut details = None;
        let mut typed = false;
        let mut asked = None;
        let mut piano = piano_lookup(entity, registry.as_deref(), device);
        egui::ScrollArea::vertical()
            .id_salt(SCROLL)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                // ⚠️ Widget state keyed only by field path leaks between tabs of the same
                // format, so every control also answers to the document id.
                ui.push_id(id, |ui| match view {
                    View::Basic => {
                        asked = self.body(ui, entity, registry.as_deref(), &mut piano, &mut sets)
                    }
                    View::Advanced => {
                        if let Some(fields) = registry.as_deref() {
                            self.advanced.table(ui, fields, &mut sets);
                            typed = !sets.is_empty();
                        }
                    }
                    View::Meta => details = self.advanced.meta(ui, entity, opened, device),
                });
            });

        if let Some(details) = details {
            for cmd in advanced::commands(details) {
                device.send(cmd, log);
            }
        }
        if let Some(asked) = asked {
            self.answer(id, asked, workspace, log);
        }
        if let Some((class, at)) = workspace.get(id).and_then(|e| e.origin.slot()) {
            if piano.asked || self.owes_deps(&piano, at, device) {
                self.fetched_deps = true;
                device.send(crate::device::DeviceCmd::Deps { class, at }, log);
            }
        }
        if act.save {
            workspace.export(id);
        }
        if act.revert {
            workspace.restore_bytes(id, opened.to_vec(), log);
            self.error = None;
            // Reread on the next frame: the name box is holding an edit that is gone.
            self.target = None;
            return act.send;
        }
        if !sets.is_empty() {
            let outcome = self.apply(id, sets, workspace, log);
            if typed {
                // The table keeps a refused cell open with what was typed in it.
                self.advanced.settled(outcome);
            }
        }
        act.send
    }

    /// Whether this frame should read the slot's dependencies without being asked to.
    ///
    /// The piano's name is the one thing about a program that no file carries, so a
    /// document opened off a slot with an instrument attached reads it straight away
    /// rather than sitting on an id until someone clicks. Once per document, and never
    /// with nothing to learn: no instrument, no piano named, a name already in hand, or a
    /// list the instrument has already given for this slot and simply did not name it in.
    fn owes_deps(&self, piano: &panel::PianoLookup, at: Location, device: &Device) -> bool {
        if self.fetched_deps || !piano.can_ask || piano.id.is_none() || piano.name.is_some() {
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

    /// Mark the open document as owed to the instrument, or say why it is not.
    pub fn stage(&mut self, id: u64, workspace: &mut Workspace, log: &mut Log) {
        let Some(entity) = workspace.get(id) else {
            return;
        };
        let name = entity.name.clone();
        match entity.origin.slot() {
            Some((class, at)) if crate::device::sendable(class) => {
                workspace.mark_pending(id, true);
                log.say(format!(
                    "“{name}” will be sent to {}.",
                    strings::place(class, at)
                ));
            }
            // ⚠️ Never a file export. Cmd+S means "keep what I did", and for something
            // that lives here that has already happened.
            _ => log.say(format!(
                "“{name}” is on this computer, and changes to it are kept as you make them."
            )),
        }
    }

    /// Where it came from, what has changed, and the three things to do about it.
    fn header(&mut self, ui: &mut egui::Ui, entity: &LocalEntity, opened: &[u8], act: &mut Header) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(&entity.name);
            if entity.dirty {
                let warn = crate::app::warn(ui.visuals());
                dot(ui, warn);
                ui.label(egui::RichText::new("changed").small().color(warn));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(entity.origin.label()).small().weak());
        });
        ui.horizontal_wrapped(|ui| {
            act.revert = ui
                .add_enabled(entity.bytes != opened, egui::Button::new("Revert"))
                .on_hover_text("back to how it was when this tab opened")
                .on_disabled_hover_text("nothing has changed")
                .clicked();
            act.save = ui.button("Export…").clicked();
            if let Some((class, at)) = entity.origin.slot() {
                let refusal = crate::device::put_refusal(class);
                let sendable = refusal.is_none() && !crate::device::read_only(class);
                let label = format!("Send to {}", strings::place(class, at));
                let button = ui.add_enabled(sendable, egui::Button::new(label));
                let button = match refusal {
                    Some(why) => button.on_disabled_hover_text(why),
                    None => button.on_disabled_hover_text(format!(
                        "{} are installed on the instrument, not sent to it",
                        strings::folder(class)
                    )),
                };
                if button.clicked() {
                    act.send = Some(SendBack {
                        id: entity.id,
                        class,
                        at,
                    });
                }
            }
        });
    }

    /// Whichever shape this asset is.
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        registry: Option<&[Field]>,
        piano: &mut panel::PianoLookup,
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
        if let Some(fields) = registry {
            if fields::is_electro5_panel(decoded) {
                panel::program(ui, &self.ctx, fields, piano, sets);
            } else if fields::is_electro5_settings(decoded) {
                panel::settings(ui, &self.ctx, fields, sets);
            } else {
                panel::plain(ui, &self.ctx, fields, sets);
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
        sample::ui(
            ui,
            &snapshot,
            &mut self.name,
            sample::is_editable(decoded),
            &sounds,
            sets,
        )
    }

    /// Do what the Basic view asked for, now that nothing is borrowing the asset.
    fn answer(&mut self, id: u64, asked: Asked, workspace: &mut Workspace, log: &mut Log) {
        match asked {
            Asked::Zone(sample::Ask::Decode(zone)) => {
                if let Some(decoded) = workspace.get(id).and_then(|e| e.entity.as_ref()) {
                    self.audio.decode(decoded, zone);
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
                let name = format!("{}.{}", draft.name, nord_format::formats::nsmp::FORMAT);
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
            Some(Ok(snapshot)) => project::ui(ui, &snapshot, &mut self.name, &mut self.paths, sets),
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
                // An edit to something read off the instrument is owed back to it. It
                // goes nowhere until the operator sends it.
                if workspace
                    .get(id)
                    .and_then(|e| e.origin.slot())
                    .is_some_and(|(class, _)| crate::device::sendable(class))
                {
                    workspace.mark_pending(id, true);
                }
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

/// The faces this document offers, in the order they are shown.
///
/// Meta is always one of them — every asset has a record, even bytes that decoded into
/// nothing. Where it is the *only* one it is called by its full name: a lone "Meta"
/// beside nothing reads as an abbreviation of something missing.
fn faces(entity: &LocalEntity, registry: Option<&[Field]>) -> Vec<(View, &'static str)> {
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
        faces.push((View::Basic, "Basic"));
    }
    if registry.is_some() {
        faces.push((View::Advanced, "Advanced"));
    }
    faces.push((
        View::Meta,
        match faces.is_empty() {
            true => "Metadata",
            false => "Meta",
        },
    ));
    faces
}

/// The face to show: the one this document was last left on, where that face still
/// exists — the panel otherwise, and the record where there is no panel either.
fn showing(faces: &[(View, &'static str)], remembered: View) -> View {
    match faces.iter().any(|(face, _)| *face == remembered) {
        true => remembered,
        false => faces.first().map_or(View::Meta, |(face, _)| *face),
    }
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
fn library_id(value: &str) -> Option<u32> {
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

    /// Paint one document into a headless context.
    ///
    /// Nothing checks pixels; what this catches is the failure a document can actually
    /// have — a section that indexes past its fields, or a control asked for a value the
    /// field cannot hold.
    fn render(sets: &[(&str, &str)], kind: Fresh) {
        render_view(sets, kind, View::Basic);
    }

    fn render_view(sets: &[(&str, &str)], kind: Fresh, view: View) {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();

        let id = workspace.create(kind, &mut log).expect("a fresh default");
        document.views.insert(id, view);
        if !sets.is_empty() {
            let bytes = workspace.get(id).unwrap().bytes.clone();
            let sets: Vec<(String, String)> = sets
                .iter()
                .map(|(p, v)| ((*p).to_string(), (*v).to_string()))
                .collect();
            let (_, edited) = fields::apply(&bytes, &sets).expect("the sets are legal");
            workspace.replace_bytes(id, edited, &mut log);
        }
        let opened = workspace.get(id).unwrap().bytes.clone();

        // Twice: the second pass runs with the caches and the widget state the first
        // one left behind, which is where a stale index would show up.
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    document.ui(ui, id, &opened, &mut workspace, &mut device, &mut log);
                });
            });
        }
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

    /// The record: the container grid, the byte diff — with something in it and with
    /// nothing — and the folded dump.
    #[test]
    fn the_meta_face_paints() {
        render_view(&[("center_panel.gain", "96")], Fresh::Program, View::Meta);
        render_view(&[], Fresh::Settings, View::Meta);
    }

    /// The engineer's table paints, filters and holds an edit — for a body with ninety
    /// fields and for one with forty.
    #[test]
    fn the_advanced_table_paints() {
        render_view(&[], Fresh::Program, View::Advanced);
        render_view(&[], Fresh::Settings, View::Advanced);
    }

    /// A body with a registry has all three faces; one with a view of its own but no
    /// registry has no field table to offer; bytes with neither have only the record,
    /// and then it is called by its full name.
    #[test]
    fn the_faces_offered_are_the_ones_the_asset_has() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();

        let labels = |workspace: &Workspace, id: u64| -> Vec<&'static str> {
            let entity = workspace.get(id).unwrap();
            let registry = entity.entity.as_ref().and_then(fields::fields_of);
            faces(entity, registry.as_deref())
                .iter()
                .map(|(_, label)| *label)
                .collect()
        };

        let program = workspace.create(Fresh::Program, &mut log).unwrap();
        assert_eq!(labels(&workspace, program), ["Basic", "Advanced", "Meta"]);

        let song = workspace.ingest(
            "blank.ne5t".into(),
            Origin::File("blank.ne5t".into()),
            crate::fields::blank::electro5_song(),
            &mut log,
        );
        assert_eq!(labels(&workspace, song), ["Basic", "Meta"]);

        let stub = workspace.ingest(
            "blank.ns3s".into(),
            Origin::File("blank.ns3s".into()),
            crate::fields::blank::stage3_song(),
            &mut log,
        );
        assert_eq!(labels(&workspace, stub), ["Metadata"]);

        let junk = workspace.ingest(
            "junk.bin".into(),
            Origin::File("junk.bin".into()),
            b"not a nord file".to_vec(),
            &mut log,
        );
        assert_eq!(labels(&workspace, junk), ["Metadata"]);
    }

    /// A document opens on the face it was left on, and on something that has no such
    /// face falls back rather than showing an empty page.
    #[test]
    fn a_document_falls_back_to_a_face_it_actually_has() {
        let all = [
            (View::Basic, "Basic"),
            (View::Advanced, "Advanced"),
            (View::Meta, "Meta"),
        ];
        assert_eq!(showing(&all, View::Meta), View::Meta);
        assert_eq!(showing(&all[..2], View::Meta), View::Basic);

        let record_only = [(View::Meta, "Metadata")];
        for left_on in [View::Basic, View::Advanced, View::Meta] {
            assert_eq!(showing(&record_only, left_on), View::Meta);
        }
        // A set list has no field table: the table's operator lands on the panel.
        let no_table = [(View::Basic, "Basic"), (View::Meta, "Meta")];
        assert_eq!(showing(&no_table, View::Advanced), View::Basic);
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
        let refused = document.apply(
            id,
            vec![("center_panel.gain".into(), "200".into())],
            &mut workspace,
            &mut log,
        );
        assert!(refused.is_err());
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

    /// An edit to something read off the instrument is owed back to it; an edit to
    /// something that only lives here is not.
    #[test]
    fn editing_a_device_document_marks_it_pending() {
        use crate::workspace::Origin;
        use nord_usb::{Location, ObjectClass};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();

        let local = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(local).unwrap().bytes.clone();
        let from_device = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
            bytes,
            &mut log,
        );

        let set = vec![("center_panel.gain".to_string(), "96".to_string())];
        document
            .apply(local, set.clone(), &mut workspace, &mut log)
            .unwrap();
        document
            .apply(from_device, set, &mut workspace, &mut log)
            .unwrap();

        assert!(!workspace.get(local).unwrap().pending, "stays here");
        assert!(workspace.get(from_device).unwrap().pending, "owed back");
        let owed: Vec<u64> = workspace.pending().iter().map(|e| e.id).collect();
        assert_eq!(owed, vec![from_device]);

        // Cmd+S on the local one says so rather than exporting anything.
        document.stage(local, &mut workspace, &mut log);
        assert!(
            log.status().1.contains("kept as you make them"),
            "{}",
            log.status().1
        );
        assert!(!workspace.get(local).unwrap().pending);
    }

    /// ⚠️ The strip and the body are two scroll regions in one `Ui`. While they shared
    /// egui's unsalted id they shared one state, and a wheel over the document moved the
    /// tab strip while the body stayed where it was.
    #[test]
    fn the_tab_strip_and_the_document_body_scroll_on_their_own() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();
        let mut tabs = crate::tabs::Tabs::default();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        tabs.open(id, &workspace);
        let opened = workspace.get(id).unwrap().bytes.clone();

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
                        ui.make_persistent_id(egui::Id::new(SCROLL)),
                    ));
                    tabs.ui(ui, &workspace);
                    ui.separator();
                    document.ui(ui, id, &opened, &mut workspace, &mut device, &mut log);
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
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();

        let first = workspace.create(Fresh::Program, &mut log).unwrap();
        let second = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(first).unwrap().bytes.clone();

        let mut show = |document: &mut Document, id: u64| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    document.ui(ui, id, &bytes, &mut workspace, &mut device, &mut log);
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
    fn render_file(name: &str, bytes: Vec<u8>, view: View) {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();

        let id = workspace.ingest(name.into(), Origin::File(name.into()), bytes, &mut log);
        let opened = workspace.get(id).unwrap().bytes.clone();
        document.views.insert(id, view);
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    document.ui(ui, id, &opened, &mut workspace, &mut device, &mut log);
                });
            });
        }
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
            render_file(name, bytes.clone(), View::Basic);
            render_file(name, bytes, View::Advanced);
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
        render_file("blank.ne5t", bytes, View::Basic);
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
        render_file("blank.ns3s", bytes, View::Meta);
    }

    /// Bytes that do not decode still have a document — it says so and shows the record.
    #[test]
    fn a_file_that_did_not_decode_still_paints() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();
        let id = workspace.ingest(
            "junk.bin".into(),
            Origin::File("junk.bin".into()),
            b"not a nord file".to_vec(),
            &mut log,
        );
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                document.ui(ui, id, &[], &mut workspace, &mut device, &mut log);
            });
        });
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
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();

        let bytes = wav_bytes();
        let id = workspace.ingest(
            "Marimba hit.wav".into(),
            Origin::File("Marimba hit.wav".into()),
            bytes.clone(),
            &mut log,
        );
        assert_eq!(
            faces(workspace.get(id).unwrap(), None)
                .iter()
                .map(|(_, label)| *label)
                .collect::<Vec<_>>(),
            ["Basic", "Meta"],
        );

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                document.ui(ui, id, &bytes, &mut workspace, &mut device, &mut log);
            });
        });
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
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut document = Document::default();

        let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
        let options = nord_format::formats::nsmp::encode::Options::new("Marimba").root_key(48);
        let bytes = nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap();
        let id = workspace.ingest(
            "whatever-it-was-called.nsmp".into(),
            Origin::File("whatever-it-was-called.nsmp".into()),
            bytes,
            &mut log,
        );
        document.target = Some(id);
        document.audio.follow(id, workspace.get(id).unwrap().stamp);

        assert!(document.audio.get(0).is_none(), "nothing decodes unasked");
        document.answer(
            id,
            Asked::Zone(sample::Ask::Decode(0)),
            &mut workspace,
            &mut log,
        );
        let decoded = document.audio.get(0).expect("asked for").as_ref().unwrap();
        assert!(decoded.audio.seconds() > 0.5);
        assert_eq!(decoded.audio.channels, 1);
        assert!(!decoded.envelope.is_empty());

        // With a zone open the document paints its envelope, which nothing else does.
        let opened = workspace.get(id).unwrap().bytes.clone();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                document.ui(ui, id, &opened, &mut workspace, &mut device, &mut log);
            });
        });

        assert_eq!(document.instrument_name(id, &workspace), "Marimba");
        assert_eq!(
            crate::workspace::zone_wav_name(&document.instrument_name(id, &workspace), 1),
            "Marimba-zone1.wav",
        );

        // Editing the instrument replaces its bytes, so what was decoded from the old
        // ones is dropped rather than kept beside a file it no longer describes.
        let edited = sample::apply(
            &workspace.get(id).unwrap().bytes,
            &[("name".into(), "Vibes".into())],
        )
        .unwrap();
        workspace.replace_bytes(id, edited, &mut log);
        document.audio.follow(id, workspace.get(id).unwrap().stamp);
        assert!(document.audio.get(0).is_none(), "decoded from stale bytes");
        assert_eq!(document.instrument_name(id, &workspace), "Vibes");
    }

    #[test]
    fn a_sample_document_paints() {
        let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
        let options = nord_format::formats::nsmp::encode::Options::new("Marimba");
        let bytes = nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap();
        render_file("Marimba.nsmp", bytes.clone(), View::Basic);
        render_file("Marimba.nsmp", bytes, View::Meta);
        render_file("Marimba hit.wav", wav_bytes(), View::Basic);
    }
}
