//! The one strip every document wears.
//!
//! What it is, what it is called, where it lives, how big it is, what state it is in,
//! which face is showing, and the one thing this kind of document is for — in that
//! order, left to right. A kind with nothing to put in one of those parts leaves the
//! part out rather than showing an empty one.
//!
//! The strip is full bleed on the window fill with a hairline under it; the body below
//! it is the only thing given a margin.

use eframe::egui;
use nord_format::accept::Family;
use nord_usb::{Location, ObjectClass};

use super::controls::Sets;
use super::{encode, piano, project, sample, setlist, SendBack};
use crate::app::{accent, caption, dot, good, warn};
use crate::browser::Kind;
use crate::device::{sendable, DeviceState};
use crate::icon::{icon, painted, Glyph};
use crate::library::{keyboard_mark, mark_words, Mark};
use crate::panel::caps;
use crate::queue::Queue;
use crate::room;
use crate::strings::{carries_tag, display_name, folder, kind_word, place, shown};
use crate::tags::Tags;
use crate::workspace::{LocalEntity, Origin};

/// The strip's own room: how tall it is at least, what it keeps at each end, and the gap
/// between two of its parts.
const HEIGHT: f32 = 38.0;
const PAD: i8 = 12;
const GAP: f32 = 10.0;

/// Every control on the strip is this tall, and corners are cut to this.
const CONTROL: f32 = 20.0;
const RADIUS: f32 = 2.0;

/// The glyph sizes: the kind, a face or a quiet action, the loud action, a tag chip.
const KIND: f32 = 15.0;
const SMALL: f32 = 11.0;
const LOUD: f32 = 12.0;
const TAG: f32 = 10.0;

/// The state dot's box, which holds a 6 px dot — see [`dot`].
const DOT: f32 = 8.0;

/// The room between a glyph and the word after it.
const INSET: f32 = 5.0;

/// The words a control carries, and the mono the badge, the place and the size are set
/// in.
const WORD: f32 = 10.5;
const MONO: f32 = 10.5;

/// A read-only value on the identity row, which stands where a text box would.
const READ: f32 = 11.5;

/// The name box, the piano's shorter one, and the variant beside it.
const NAME: f32 = 190.0;
const PIANO_NAME: f32 = 150.0;
const VARIANT: f32 = 84.0;

/// The name the instrument owns, which is text rather than a box.
const FIXED: f32 = 12.5;

/// The identity row's indent, measured from the strip's outer edge, and its own height.
const INDENT: f32 = 37.0;
const CHIP: f32 = 18.0;

/// Which face of a document is showing.
///
/// Three files rather than three modes: Edit is the sound, Metadata is what the file
/// says about itself, and Advanced is the engineering. Which of them a document has is
/// [`super::faces`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Face {
    /// The panel, in the instrument's own words. Happy-path edits.
    #[default]
    Edit,
    /// The record: the container, the bytes that moved, and what the instrument says
    /// about the slot. Nothing here is a control.
    Metadata,
    /// The whole body as a table. Nothing hidden.
    Advanced,
}

impl Face {
    pub fn label(self) -> &'static str {
        match self {
            Face::Edit => "Edit",
            Face::Metadata => "Metadata",
            Face::Advanced => "Advanced",
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Face::Edit => Glyph::Pencil,
            Face::Metadata => Glyph::Info,
            Face::Advanced => Glyph::Wrench,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Face::Edit => "the fields that change the sound",
            Face::Metadata => "what the file says about itself — read only",
            Face::Advanced => "capabilities, offsets, raw values — engineering",
        }
    }
}

/// How much of the header fits, measured on the header's own width.
///
/// The collapse order, widest first: the quiet actions lose their words, then the faces
/// lose theirs, then the loud action keeps only its number. The identity row wraps after
/// all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Full,
    Quiet,
    Faces,
    Narrow,
}

/// The three widths the strip changes shape at.
const BREAKPOINTS: [f32; 3] = [1000.0, 860.0, 720.0];

pub fn stage(width: f32) -> Stage {
    match width {
        width if width >= BREAKPOINTS[0] => Stage::Full,
        width if width >= BREAKPOINTS[1] => Stage::Quiet,
        width if width >= BREAKPOINTS[2] => Stage::Faces,
        _ => Stage::Narrow,
    }
}

/// The ink a header phrase or stroke may wear.
///
/// ⚠️ There is no `bad` here. Red is for the hatches in a key map, and a header that
/// has already shouted has nothing louder left to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Good,
    Warn,
    /// The caption ink: a figure rather than a claim.
    Quiet,
}

impl Ink {
    fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            Ink::Good => good(visuals),
            Ink::Warn => warn(visuals),
            Ink::Quiet => caption(visuals),
        }
    }
}

/// The size line: what it reads, whether it is a warning, and the sentence behind it.
pub struct SizeLine {
    pub text: String,
    pub warn: bool,
    pub hint: String,
}

/// The dot and the phrase beside it: one claim about this document.
pub struct StateLine {
    pub words: String,
    pub ink: Ink,
    pub hint: String,
}

/// How the loud action is dressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// It can happen.
    Ready,
    /// It cannot yet, and the label carries the reason.
    Blocked,
    /// There is nothing to do it to.
    Idle,
}

/// The one loud action a document has, in whichever of its three states it is in.
pub struct Loud {
    pub label: String,
    /// What it is called where there is no room for the whole label.
    pub short: String,
    pub glyph: Glyph,
    pub tone: Tone,
    pub hint: String,
    /// The slot a click would queue a write for. `None` is an action nothing happens on.
    pub send: Option<(ObjectClass, Location)>,
}

/// What an editor adds to the header that the asset alone does not say.
///
/// Each field is what the strip shows *instead* of what it works out for itself: the
/// piano's `188 of 194 MB`, its `trimmed`, and its refusal to queue a library that does
/// not fit.
#[derive(Default)]
pub struct Extras {
    pub size: Option<SizeLine>,
    /// The word for an unsaved document, where the editor has a better one than
    /// `edited`.
    pub edited: Option<&'static str>,
    pub loud: Option<Loud>,
}

/// One cell of the identity row: a MICRO-caps label and what the document wears under
/// it.
pub struct Cell {
    pub label: &'static str,
    pub body: Body,
    /// The one note that is shown rather than hovered: a constraint the field enforces.
    pub note: Option<String>,
    pub hint: &'static str,
}

/// What a cell wears beside its label.
///
/// There is no third kind yet because no header-level field of any format has a setter:
/// the sample's category and sub name are stated by the file and read here.
pub enum Body {
    /// One chip per label the list puts on this asset.
    Chips(Vec<String>),
    /// A value the file states and nothing here writes.
    Read(String),
}

/// Everything the strip draws from besides the asset itself.
pub(super) struct Facts<'a> {
    pub faces: &'a [Face],
    pub showing: Face,
    pub device: &'a DeviceState,
    pub queue: &'a Queue,
    pub tags: &'a Tags,
    /// Whether this is a view of the instrument's own copy rather than an asset held
    /// here.
    pub view: bool,
    pub extras: Extras,
}

/// What the header was asked for this frame.
#[derive(Default)]
pub(super) struct Clicked {
    pub revert: bool,
    pub export: bool,
    pub send: Option<SendBack>,
    /// The face picked, where one was.
    pub face: Option<Face>,
    /// The name the asset is to be called, where a rename settled. Applied once nothing
    /// is borrowing the asset.
    pub rename: Option<String>,
}

/// Draw the strip, and the identity row under it where the kind has one.
pub(super) fn ui(
    ui: &mut egui::Ui,
    entity: &LocalEntity,
    facts: &Facts<'_>,
    boxes: (&mut String, &mut String),
    sets: &mut Sets,
) -> Clicked {
    let stage = stage(ui.available_width());
    let cells = identity(entity, facts.tags);
    let visuals = ui.visuals().clone();
    let mut act = Clicked::default();

    let drawn = egui::Frame::new()
        .fill(visuals.window_fill)
        .inner_margin(egui::Margin {
            left: PAD,
            right: PAD,
            top: 4,
            bottom: match cells.is_empty() {
                true => 4,
                false => 8,
            },
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            ui.spacing_mut().interact_size.y = CONTROL;
            ui.horizontal(|ui| {
                ui.set_min_height(HEIGHT - 8.0);
                left(ui, entity, facts, boxes, sets, &mut act);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    right(ui, entity, facts, stage, &mut act)
                });
            });
            if !cells.is_empty() {
                row(ui, &cells, stage);
            }
        });

    let hairline = egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color);
    let rect = drawn.response.rect;
    ui.painter()
        .hline(ui.max_rect().x_range(), rect.bottom() - 0.5, hairline);
    act
}

/// The kind, the name, the format, the place, the size and the state.
fn left(
    ui: &mut egui::Ui,
    entity: &LocalEntity,
    facts: &Facts<'_>,
    boxes: (&mut String, &mut String),
    sets: &mut Sets,
    act: &mut Clicked,
) {
    let visuals = ui.visuals().clone();
    let quiet = caption(&visuals);
    let glyph = Kind::of(entity.entity.as_ref()).glyph();
    icon(ui, glyph, KIND, accent(&visuals));

    let (held, stored) = named(entity, facts.view);
    act.rename = name(ui, entity, &held, &stored, boxes, sets);

    let (badge, hint) = badge(entity);
    let drawn = pill(
        ui,
        Pill {
            glyph: None,
            label: Some(&badge),
            ink: visuals
                .widgets
                .inactive
                .fg_stroke
                .color
                .gamma_multiply(0.85),
            mono: true,
            stroke: Some(visuals.widgets.noninteractive.bg_stroke.color),
            dashed: false,
            pad: 6.0,
            height: CONTROL,
            live: false,
        },
    );
    if !hint.is_empty() {
        drawn.on_hover_text(hint);
    }

    mono(ui, &lives(entity), quiet).on_hover_text(entity.origin.label());

    let own = sized(entity);
    if let Some(size) = facts.extras.size.as_ref().or(own.as_ref()) {
        rule(ui);
        let ink = match size.warn {
            true => warn(&visuals),
            false => quiet,
        };
        let drawn = mono(ui, &size.text, ink);
        if !size.hint.is_empty() {
            drawn.on_hover_text(&size.hint);
        }
    }

    if let Some(state) = state(entity, facts) {
        let ink = state.ink.color(&visuals);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = INSET;
            dot(ui, ink, DOT);
            ui.label(egui::RichText::new(&state.words).size(WORD).color(ink));
        })
        .response
        .on_hover_text(&state.hint);
    }
}

/// The faces, the quiet actions and the loud one, in that order left to right — which
/// in a right-to-left layout is this order backwards.
fn right(
    ui: &mut egui::Ui,
    entity: &LocalEntity,
    facts: &Facts<'_>,
    stage: Stage,
    act: &mut Clicked,
) {
    let visuals = ui.visuals().clone();
    let quiet = caption(&visuals);
    let own = action(entity, facts);
    let loud = facts.extras.loud.as_ref().unwrap_or(&own);
    let label = match stage {
        Stage::Narrow => &loud.short,
        _ => &loud.label,
    };
    let (stroke, mark, ink) = match loud.tone {
        Tone::Ready => (accent(&visuals), accent(&visuals), visuals.text_color()),
        Tone::Blocked => (warn(&visuals), warn(&visuals), visuals.text_color()),
        Tone::Idle => (visuals.widgets.noninteractive.bg_stroke.color, quiet, quiet),
    };
    let clicked = pill(
        ui,
        Pill {
            glyph: Some((loud.glyph, LOUD, mark)),
            label: Some(label),
            ink,
            mono: false,
            stroke: Some(stroke),
            dashed: loud.tone == Tone::Idle,
            pad: 8.0,
            height: CONTROL,
            live: loud.tone == Tone::Ready,
        },
    )
    .on_hover_text(&loud.hint)
    .clicked();
    if let (true, Some((class, at))) = (clicked, loud.send) {
        act.send = Some(SendBack {
            id: entity.id,
            class,
            at,
        });
    }

    // Right to left: Export is drawn before Revert so the two read the other way round.
    let words = stage == Stage::Full;
    let quiet_pill = |ui: &mut egui::Ui, glyph, label: &str, live| {
        pill(
            ui,
            Pill {
                glyph: Some((glyph, SMALL, quiet)),
                label: words.then_some(label),
                ink: quiet,
                mono: false,
                stroke: Some(visuals.widgets.noninteractive.bg_stroke.color),
                dashed: false,
                pad: match words {
                    true => 7.0,
                    false => 5.0,
                },
                height: CONTROL,
                live,
            },
        )
    };
    act.export = quiet_pill(ui, Glyph::ArrowDownToLine, "Export…", true)
        .on_hover_text(hint(
            "Export…",
            "a copy on this computer, under a name you pick",
            words,
        ))
        .clicked();
    let unsaved = entity.is_unsaved();
    let revert = quiet_pill(ui, Glyph::RotateCcw, "Revert", unsaved);
    act.revert = match unsaved {
        true => revert
            .on_hover_text(hint(
                "Revert",
                "back to the bytes it was last saved as",
                words,
            ))
            .clicked(),
        false => {
            revert.on_hover_text(hint(
                "Revert",
                "nothing has changed since it was saved",
                words,
            ));
            false
        }
    };

    rule(ui);
    act.face = segments(ui, facts, stage);
}

/// A hover that names the control as well as explaining it, once the stage has taken the
/// word off its face.
fn hint(label: &str, why: &str, words: bool) -> String {
    match words {
        true => why.to_string(),
        false => format!("{why} ({label})"),
    }
}

/// The faces as one control: one stroke round the group, a rule between two of them, and
/// the showing one filled. Only the faces this document has are in it.
fn segments(ui: &mut egui::Ui, facts: &Facts<'_>, stage: Stage) -> Option<Face> {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let words = matches!(stage, Stage::Full | Stage::Quiet);
    let pad = match words {
        true => 8.0,
        false => 6.0,
    };
    let ink = |face: Face| match face == facts.showing {
        true => visuals.text_color(),
        false => caption(&visuals),
    };
    let laid: Vec<(Face, Option<std::sync::Arc<egui::Galley>>, f32)> = facts
        .faces
        .iter()
        .map(|face| {
            let word = words.then(|| {
                painter.layout_no_wrap(
                    face.label().to_string(),
                    egui::FontId::proportional(WORD),
                    ink(*face),
                )
            });
            let width = pad * 2.0 + SMALL + word.as_ref().map_or(0.0, |laid| INSET + laid.size().x);
            (*face, word, width)
        })
        .collect();

    let total: f32 = laid.iter().map(|(_, _, width)| width).sum();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, CONTROL), egui::Sense::hover());
    let border = egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color);
    let mut picked = None;
    let mut x = rect.left();
    let last = laid.len().saturating_sub(1);
    for (nth, (face, word, width)) in laid.into_iter().enumerate() {
        let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(width, CONTROL));
        let response = ui.interact(
            cell,
            ui.id().with(("face", face.label())),
            egui::Sense::click(),
        );
        let fill = match (face == facts.showing, response.hovered()) {
            (true, _) => Some(visuals.widgets.active.weak_bg_fill),
            (false, true) => Some(visuals.widgets.hovered.weak_bg_fill),
            (false, false) => None,
        };
        if let Some(fill) = fill {
            painter.rect_filled(cell, 0.0, fill);
        }
        if nth < last {
            painter.vline(cell.right() - 0.5, cell.y_range(), border);
        }
        painted(
            ui,
            face.glyph(),
            egui::Rect::from_center_size(
                egui::pos2(cell.left() + pad + SMALL / 2.0, cell.center().y),
                egui::Vec2::splat(SMALL),
            ),
            ink(face),
        );
        if let Some(word) = word {
            let at = egui::pos2(
                cell.left() + pad + SMALL + INSET,
                cell.center().y - word.size().y / 2.0,
            );
            painter.galley(at, word, ink(face));
        }
        if response.clicked() {
            picked = Some(face);
        }
        response.on_hover_text(hint(face.label(), face.hint(), words));
        x = cell.right();
    }
    painter.rect_stroke(rect, RADIUS, border, egui::StrokeKind::Inside);
    picked
}

/// The identity row: a MICRO-caps label and what the document wears under it, one cell
/// after another under the name.
fn row(ui: &mut egui::Ui, cells: &[Cell], stage: Stage) {
    let indent = INDENT - f32::from(PAD);
    let draw = |ui: &mut egui::Ui| {
        ui.spacing_mut().item_spacing = egui::vec2(18.0, 4.0);
        ui.add_space(indent);
        for cell in cells {
            let response = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    ui.label(caps(cell.label).color(caption(ui.visuals())));
                    ui.spacing_mut().item_spacing.x = 4.0;
                    match &cell.body {
                        Body::Chips(chips) => {
                            for text in chips {
                                chip(ui, text);
                            }
                        }
                        Body::Read(value) => read(ui, value),
                    }
                    if let Some(note) = &cell.note {
                        ui.label(
                            egui::RichText::new(note)
                                .size(10.0)
                                .color(caption(ui.visuals())),
                        );
                    }
                })
                .response;
            if !cell.hint.is_empty() {
                response.on_hover_text(cell.hint);
            }
        }
    };
    match stage {
        Stage::Narrow => ui.horizontal_wrapped(draw),
        _ => ui.horizontal(draw),
    };
}

/// A value the file states: the eye that says nothing writes it, and the value in mono.
fn read(ui: &mut egui::Ui, value: &str) {
    ui.spacing_mut().item_spacing.x = INSET;
    icon(ui, Glyph::Eye, SMALL, caption(ui.visuals()));
    ui.label(
        egui::RichText::new(value)
            .font(egui::FontId::monospace(READ))
            .color(ui.visuals().weak_text_color()),
    );
}

/// One tag chip: the accent stroke and ink the inspector's own chips wear, in a box
/// shorter than the strip's own controls.
fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let ink = accent(ui.visuals());
    pill(
        ui,
        Pill {
            glyph: Some((Glyph::Tag, TAG, ink)),
            label: Some(text),
            ink,
            mono: false,
            stroke: Some(ink),
            dashed: false,
            pad: 6.0,
            height: CHIP,
            live: false,
        },
    )
}

/// What one button on the strip is made of.
struct Pill<'a> {
    /// The glyph, its box and its ink. The badge carries none.
    glyph: Option<(Glyph, f32, egui::Color32)>,
    /// The word beside it, or nothing where the stage has taken it.
    label: Option<&'a str>,
    ink: egui::Color32,
    mono: bool,
    stroke: Option<egui::Color32>,
    dashed: bool,
    pad: f32,
    height: f32,
    /// Whether the pointer lifts it and a click of it means anything.
    live: bool,
}

fn pill(ui: &mut egui::Ui, held: Pill<'_>) -> egui::Response {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let font = match held.mono {
        true => egui::FontId::monospace(MONO),
        false => egui::FontId::proportional(WORD),
    };
    let word = held
        .label
        .map(|label| painter.layout_no_wrap(label.to_string(), font, held.ink));
    let marked = held.glyph.map_or(0.0, |(_, size, _)| size);
    let gap = match (held.glyph.is_some(), word.is_some()) {
        (true, true) => INSET,
        _ => 0.0,
    };
    let width = held.pad * 2.0 + marked + gap + word.as_ref().map_or(0.0, |laid| laid.size().x);
    let sense = match held.live {
        true => egui::Sense::click(),
        false => egui::Sense::hover(),
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, held.height), sense);

    if held.live && response.hovered() {
        painter.rect_filled(rect, RADIUS, visuals.widgets.hovered.weak_bg_fill);
    }
    if let Some(color) = held.stroke {
        let stroke = egui::Stroke::new(1.0_f32, color);
        match held.dashed {
            true => dashed(&painter, rect, stroke),
            false => {
                painter.rect_stroke(rect, RADIUS, stroke, egui::StrokeKind::Inside);
            }
        }
    }
    let mut x = rect.left() + held.pad;
    if let Some((glyph, size, mark)) = held.glyph {
        painted(
            ui,
            glyph,
            egui::Rect::from_center_size(
                egui::pos2(x + size / 2.0, rect.center().y),
                egui::Vec2::splat(size),
            ),
            mark,
        );
        x += size + gap;
    }
    if let Some(word) = word {
        let at = egui::pos2(x, rect.center().y - word.size().y / 2.0);
        painter.galley(at, word, held.ink);
    }
    response
}

/// A dashed outline: the one stroke that says an act has nothing to act on.
fn dashed(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    const DASH: f32 = 3.0;
    let rect = rect.shrink(0.5);
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    for side in corners.windows(2) {
        painter.extend(egui::Shape::dashed_line(side, stroke, DASH, DASH));
    }
}

/// A mono readout: the place and the size, which are figures rather than controls.
fn mono(ui: &mut egui::Ui, text: &str, ink: egui::Color32) -> egui::Response {
    ui.label(
        egui::RichText::new(text)
            .font(egui::FontId::monospace(MONO))
            .color(ink),
    )
}

/// The 1 px rule that parts one group of the strip from the next.
fn rule(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 12.0), egui::Sense::hover());
    let color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    ui.painter().rect_filled(rect, 0.0, color);
}

/// Where a document's name is kept, which decides what typing in the box does.
enum Named {
    /// The file stores it, under the `name` its editor already spells. `variant` is the
    /// piano's second half, which is a box of its own.
    Stored {
        limit: Option<usize>,
        variant: Option<String>,
        width: f32,
    },
    /// The asset's own name on this computer: a rename, not an edit to the file.
    Asset,
    /// The instrument's own, and there is one of it.
    Device,
}

/// What the name box holds, and what typing in it does.
fn named(entity: &LocalEntity, view: bool) -> (Named, String) {
    let decoded = entity.entity.as_ref();
    if let Some(Ok(held)) = decoded.and_then(sample::snapshot) {
        return (
            Named::Stored {
                limit: Some(held.max_name_len),
                variant: None,
                width: NAME,
            },
            held.name,
        );
    }
    if let Some(Ok(held)) = decoded.and_then(project::snapshot) {
        return (
            Named::Stored {
                limit: None,
                variant: None,
                width: NAME,
            },
            held.name,
        );
    }
    if let Some(Ok(held)) = decoded.and_then(piano::snapshot) {
        return (
            Named::Stored {
                limit: None,
                variant: Some(held.variant),
                width: PIANO_NAME,
            },
            held.name,
        );
    }
    let settings = Kind::of(decoded) == Kind::Settings;
    match view && settings {
        true => (Named::Device, display_name(&entity.name).to_string()),
        false => (Named::Asset, display_name(&entity.name).to_string()),
    }
}

/// What the name boxes hold when a document opens.
pub(super) fn boxes(entity: &LocalEntity, view: bool) -> (String, String) {
    let (held, stored) = named(entity, view);
    let variant = match held {
        Named::Stored { variant, .. } => variant.unwrap_or_default(),
        Named::Asset | Named::Device => String::new(),
    };
    (stored, variant)
}

/// The name, as a box where something here can change it and as text where it is the
/// instrument's. Returns the name the asset is to be called, where a rename settled.
fn name(
    ui: &mut egui::Ui,
    entity: &LocalEntity,
    held: &Named,
    stored: &str,
    boxes: (&mut String, &mut String),
    sets: &mut Sets,
) -> Option<String> {
    let (typed, variant_box) = boxes;
    match held {
        Named::Device => {
            ui.label(
                egui::RichText::new(stored)
                    .size(FIXED)
                    .strong()
                    .color(ui.visuals().text_color()),
            )
            .on_hover_text("a keyboard has one settings block; it is not renamed");
            None
        }
        Named::Stored {
            limit,
            variant,
            width,
        } => {
            if settled(ui, typed, *width, *limit, false) && typed != stored {
                sets.push(("name".to_string(), typed.clone()));
            }
            if let Some(stored) = variant {
                ui.label(
                    egui::RichText::new("#")
                        .font(egui::FontId::monospace(12.0))
                        .color(caption(ui.visuals())),
                );
                if settled(ui, variant_box, VARIANT, None, true) && variant_box != stored {
                    sets.push(("variant".to_string(), variant_box.clone()));
                }
            }
            None
        }
        Named::Asset => {
            let done = settled(ui, typed, NAME, None, false);
            let wanted = tagged(&entity.name, typed);
            (done && !typed.trim().is_empty() && wanted != entity.name).then_some(wanted)
        }
    }
}

/// A single-line name box that commits when it is done rather than per keystroke: half a
/// name is a name the format would take.
fn settled(
    ui: &mut egui::Ui,
    text: &mut String,
    width: f32,
    limit: Option<usize>,
    mono: bool,
) -> bool {
    let mut edit = egui::TextEdit::singleline(text)
        .desired_width(width)
        .margin(egui::Margin::symmetric(4, 1));
    if let Some(limit) = limit {
        edit = edit.char_limit(limit);
    }
    if mono {
        edit = edit.font(egui::FontId::monospace(MONO));
    }
    let response = ui.add(edit);
    response.lost_focus() || response.ctx.input(|i| i.key_pressed(egui::Key::Enter))
}

/// What a typed name is stored as: the words that were typed, under the format tag the
/// stored name carries.
///
/// The glyph beside the box already says what kind of file it is, so the tag is never in
/// the box — and it must not be lost by typing in one.
fn tagged(stored: &str, typed: &str) -> String {
    match carries_tag(stored) {
        true => match stored.rsplit_once('.') {
            Some((_, tag)) => format!("{typed}.{tag}"),
            None => typed.to_string(),
        },
        false => typed.to_string(),
    }
}

/// The mono badge over a document, and the sentence behind it.
///
/// ⚠️ Exhaustive over [`Kind`], so a kind the browser learns is a badge decided here
/// rather than a blank one.
pub(super) fn badge(entity: &LocalEntity) -> (String, String) {
    let tag = entity.tag();
    let kind = Kind::of(entity.entity.as_ref());
    let word = kind_word(kind, Family::of_tag(&tag));
    let version = entity.container.as_ref().map(|held| held.header.version);
    let sentence = match version {
        Some(version) => format!("{word}, content version {version}"),
        None => word,
    };
    match kind {
        Kind::Sample => {
            let generation = entity
                .entity
                .as_ref()
                .and_then(sample::snapshot)
                .and_then(Result::ok)
                .map_or_else(String::new, |held| format!(" {}", held.generation));
            (
                format!("nsmp{generation}"),
                match version {
                    Some(version) => format!("content version {version}"),
                    None => sentence,
                },
            )
        }
        Kind::Piano => match stream_version(entity) {
            Some(stream) => (
                format!("npno {stream:#05x}"),
                format!("stream version {stream:#05x}"),
            ),
            None => (tag, sentence),
        },
        Kind::Project => (
            "project".to_string(),
            "a Nord Sample Editor project, which builds an nsmp".to_string(),
        ),
        Kind::Program => (format!("program v{}", version.unwrap_or(0)), sentence),
        Kind::SetList => ("set list".to_string(), sentence),
        Kind::Settings => ("settings".to_string(), sentence),
        Kind::Other => match encode::is_wav(&entity.bytes) {
            true => (
                "wav".to_string(),
                "audio this app can make an instrument out of".to_string(),
            ),
            false => (tag, "these bytes did not decode".to_string()),
        },
        Kind::Live
        | Kind::Synth
        | Kind::OrganPreset
        | Kind::PianoPreset
        | Kind::Performance
        | Kind::LeadBank
        | Kind::SampleLibrary
        | Kind::PipeLibrary
        | Kind::Bundle => (tag, sentence),
    }
}

/// The stream version a piano library states, which is not the container's.
fn stream_version(entity: &LocalEntity) -> Option<u16> {
    match entity.entity.as_ref()? {
        nord_format::Entity::Piano(piano) => piano.stream_version().ok(),
        _ => None,
    }
}

/// Where the document lives: a slot on the instrument, a folder on this computer, or the
/// computer itself.
fn lives(entity: &LocalEntity) -> String {
    if let Some((class, at)) = entity.spot() {
        return place(class, at);
    }
    match &entity.origin {
        Origin::File(path) => folder_of(path, home()),
        Origin::Rescued { at } => shown(*at),
        // ⚠️ Unreachable: a device origin is a slot, and `spot` answered for it.
        Origin::Device { class, at } => place(*class, *at),
        Origin::Fresh => "This computer".to_string(),
    }
}

/// The home directory, where the host has one to compare a path against.
fn home() -> Option<String> {
    std::env::var("HOME").ok().filter(|home| !home.is_empty())
}

/// The folder a path is in, with the home directory written `~`.
///
/// A name with no folder in it — what a drop and the file picker hand over — is on this
/// computer and nothing more can be said about where.
fn folder_of(path: &str, home: Option<String>) -> String {
    let parent = match path.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
        None => return "This computer".to_string(),
    };
    match home.filter(|home| parent == *home || parent.starts_with(&format!("{home}/"))) {
        Some(home) => format!("~{}", &parent[home.len()..]),
        None => parent,
    }
}

/// How big the document is, in whatever it is that a document of this kind has: bytes,
/// or the entries a set list orders. Settings are one block and there is nothing to say.
fn sized(entity: &LocalEntity) -> Option<SizeLine> {
    let kind = Kind::of(entity.entity.as_ref());
    if kind == Kind::Settings {
        return None;
    }
    if kind == Kind::SetList {
        let entries = setlist::entries(entity.entity.as_ref()?)?;
        return Some(SizeLine {
            text: match entries {
                1 => "1 entry".to_string(),
                entries => format!("{entries} entries"),
            },
            warn: false,
            hint: "the programs this set list orders".to_string(),
        });
    }
    let bytes = entity.bytes.len() as u64;
    Some(SizeLine {
        text: room::measure(bytes),
        warn: false,
        hint: format!("{bytes} bytes"),
    })
}

/// The one claim the header makes about this document: what it holds that is not what it
/// was saved as, or what the attached instrument holds where this document stands.
///
/// ⚠️ Ordered. Unsaved comes first because an edit is what the reader just did, and a
/// slot that agrees with the *saved* bytes says nothing about the ones in front of them.
fn state(entity: &LocalEntity, facts: &Facts<'_>) -> Option<StateLine> {
    let waiting = facts.queue.holds(entity.id);
    if entity.is_unsaved() {
        return Some(phrase(Mark::Unsaved, waiting, facts.extras.edited));
    }
    let mark = keyboard_mark(entity, facts.device, facts.queue)?;
    Some(phrase(mark, waiting, facts.extras.edited))
}

/// What a mark says in the strip, in the strip's own shorter words — and never in red.
///
/// ⚠️ Exhaustive over [`Mark`], and every arm's ink is an [`Ink`]: there is no spelling
/// of this that reaches `bad`.
fn phrase(mark: Mark, waiting: bool, edited: Option<&'static str>) -> StateLine {
    let hint = mark_words(mark).to_string();
    match mark {
        Mark::Unsaved => StateLine {
            words: edited.unwrap_or("edited").to_string(),
            ink: Ink::Warn,
            hint,
        },
        Mark::Agrees => StateLine {
            words: "matches keyboard".to_string(),
            ink: Ink::Good,
            hint,
        },
        Mark::Differs => StateLine {
            words: match waiting {
                true => "waiting to send".to_string(),
                false => "differs from keyboard".to_string(),
            },
            ink: Ink::Warn,
            hint,
        },
        Mark::Unknown => StateLine {
            words: "on the keyboard".to_string(),
            ink: Ink::Quiet,
            hint,
        },
    }
}

/// The one loud action, and which of its three states it is in.
///
/// ⚠️ Ordered, and the order is what makes the label honest: a project has nothing to
/// send whatever is attached, an unattached instrument cannot be written to whatever the
/// asset is, and a class this app does not write into is never a question of room.
fn action(entity: &LocalEntity, facts: &Facts<'_>) -> Loud {
    let send = |hint: String| Loud {
        label: "Queue send".to_string(),
        short: "Send".to_string(),
        glyph: Glyph::Upload,
        tone: Tone::Ready,
        hint,
        send: entity.spot(),
    };
    let idle = |hint: String| Loud {
        send: None,
        tone: Tone::Idle,
        ..send(hint)
    };

    if Kind::of(entity.entity.as_ref()) == Kind::Project {
        return Loud {
            label: "Build → .nsmp".to_string(),
            short: "Build".to_string(),
            glyph: Glyph::CircleAlert,
            tone: Tone::Blocked,
            hint: "the codec is not understood yet".to_string(),
            send: None,
        };
    }
    if !facts.device.connected() {
        return idle("no instrument attached — nothing to send to".to_string());
    }
    let Some((class, at)) = entity.spot() else {
        return idle("this stands on no slot — there is nothing to replace".to_string());
    };
    if !sendable(class) {
        return idle(format!(
            "{} are installed on the instrument, not sent to it",
            folder(class)
        ));
    }
    if let Some((over, free)) = over(entity, class, facts.device) {
        return Loud {
            label: format!("Won't fit · {} over", room::measure(over)),
            short: format!("{} over", room::measure(over)),
            glyph: Glyph::CircleAlert,
            tone: Tone::Blocked,
            hint: format!(
                "{} is free in {}, and this is {}",
                room::measure(free),
                folder(class),
                room::measure(entity.bytes.len() as u64)
            ),
            send: None,
        };
    }
    send(format!("replaces {}", place(class, at)))
}

/// How much larger the document is than the room left in its folder, and how much that
/// room is — where the folder counts in bytes and the document is the larger.
fn over(entity: &LocalEntity, class: ObjectClass, device: &DeviceState) -> Option<(u64, u64)> {
    let free = room::free_bytes(class, device)?;
    let bytes = entity.bytes.len() as u64;
    bytes
        .checked_sub(free)
        .filter(|over| *over > 0)
        .map(|over| (over, free))
}

/// The identity cells this kind has: the header-level fields the strip cannot carry.
///
/// ⚠️ A cell with nothing in it is left out, and a kind with no cell at all has no row —
/// an indented empty line under the name reads as a field that failed to draw.
fn identity(entity: &LocalEntity, tags: &Tags) -> Vec<Cell> {
    let mut cells = Vec::new();
    if Kind::of(entity.entity.as_ref()) == Kind::Program {
        let worn: Vec<String> = tags
            .worn(entity.id)
            .iter()
            .filter_map(|tag| tags.name_of(*tag))
            .map(str::to_string)
            .collect();
        if !worn.is_empty() {
            cells.push(Cell {
                label: "Tags",
                body: Body::Chips(worn),
                note: None,
                hint: "what this computer's list labels it with",
            });
        }
    }
    if let Some(stated) = entity.entity.as_ref().and_then(sample::stated) {
        cells.push(stated);
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Log;
    use crate::workspace::{Fresh, Workspace};

    fn workspace() -> (Workspace, Log) {
        let ctx = egui::Context::default();
        (Workspace::new(ctx), Log::default())
    }

    /// A workspace holding one file, and its id.
    fn opened(name: &str, bytes: Vec<u8>) -> (Workspace, u64) {
        let (mut workspace, mut log) = workspace();
        let id = workspace.ingest(
            name.to_string(),
            Origin::File(name.to_string()),
            bytes,
            &mut log,
        );
        (workspace, id)
    }

    /// The collapse order is decided on three widths, and a width exactly on one is
    /// still the wider stage.
    #[test]
    fn each_breakpoint_belongs_to_the_stage_above_it() {
        assert_eq!(stage(1600.0), Stage::Full);
        assert_eq!(stage(1000.0), Stage::Full);
        assert_eq!(stage(999.0), Stage::Quiet);
        assert_eq!(stage(860.0), Stage::Quiet);
        assert_eq!(stage(859.0), Stage::Faces);
        assert_eq!(stage(720.0), Stage::Faces);
        assert_eq!(stage(719.0), Stage::Narrow);
        assert_eq!(stage(0.0), Stage::Narrow);
    }

    /// A path's folder, with the home directory written the way a person writes it.
    #[test]
    fn a_path_reads_as_the_folder_it_is_in() {
        let home = || Some("/Users/x".to_string());
        assert_eq!(
            folder_of("/Users/x/Nord/projects/a.nsmpproj", home()),
            "~/Nord/projects"
        );
        assert_eq!(folder_of("/Users/x/a.nsmp", home()), "~");
        assert_eq!(folder_of("/opt/nord/a.nsmp", home()), "/opt/nord");
        assert_eq!(folder_of("/a.nsmp", home()), "/");
        // Another account's home is not this one's, however it begins.
        assert_eq!(folder_of("/Users/xavier/a.nsmp", home()), "/Users/xavier");
        // A drop hands over a bare name, and a name says nothing about a folder.
        assert_eq!(folder_of("a.nsmp", home()), "This computer");
        assert_eq!(folder_of("/opt/a.nsmp", None), "/opt");
    }

    /// A slot beats a path: an asset matched to the instrument is where the instrument
    /// has it.
    #[test]
    fn the_place_is_the_slot_where_there_is_one() {
        let (mut workspace, mut log) = workspace();
        let fresh = workspace.create(Fresh::Program, &mut log).unwrap();
        assert_eq!(lives(workspace.get(fresh).unwrap()), "This computer");

        let bytes = workspace.get(fresh).unwrap().bytes.clone();
        let copied = workspace.ingest(
            "Africa.ne5p".to_string(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
            bytes,
            &mut log,
        );
        assert_eq!(lives(workspace.get(copied).unwrap()), "Programs 7:4");
    }

    /// One badge per kind, and the version behind it: the generation or kind in the
    /// badge, the content or stream version in the hover.
    #[test]
    fn every_kind_says_what_format_it_is() {
        use crate::fields::blank;

        let (mut workspace, mut log) = workspace();
        let program = workspace.create(Fresh::Program, &mut log).unwrap();
        assert_eq!(
            badge(workspace.get(program).unwrap()),
            (
                "program v4".to_string(),
                "Electro 5 program, content version 4".to_string()
            )
        );

        let settings = workspace.create(Fresh::Settings, &mut log).unwrap();
        assert_eq!(
            badge(workspace.get(settings).unwrap()),
            (
                "settings".to_string(),
                "Electro 5 settings, content version 0".to_string()
            )
        );

        let (held, id) = opened("blank.ne5t", blank::electro5_song());
        assert_eq!(
            badge(held.get(id).unwrap()),
            (
                "set list".to_string(),
                "Electro 5 set list, content version 1".to_string()
            )
        );

        let (held, id) = opened("junk.bin", b"not a nord file".to_vec());
        assert_eq!(
            badge(held.get(id).unwrap()),
            ("?".to_string(), "these bytes did not decode".to_string())
        );

        let (held, id) = opened("Marimba.nsmp", sample_bytes());
        assert_eq!(
            badge(held.get(id).unwrap()),
            ("nsmp v2".to_string(), "content version 200".to_string())
        );

        let (held, id) = opened("clarinet.nsmpproj", project_bytes());
        let (text, hint) = badge(held.get(id).unwrap());
        assert_eq!(text, "project");
        assert!(hint.contains("Sample Editor project"), "{hint}");

        let (held, id) = opened("Marimba hit.wav", wav_bytes());
        assert_eq!(badge(held.get(id).unwrap()).0, "wav");
    }

    /// A set list is an order, so its size is the count of what it orders; settings are
    /// one block and say nothing; everything else is its bytes.
    #[test]
    fn the_size_is_whatever_this_kind_measures_in() {
        let (mut workspace, mut log) = workspace();
        let settings = workspace.create(Fresh::Settings, &mut log).unwrap();
        assert!(sized(workspace.get(settings).unwrap()).is_none());

        let (held, id) = opened("blank.ne5t", crate::fields::blank::electro5_song());
        assert_eq!(sized(held.get(id).unwrap()).unwrap().text, "4 entries");

        let program = workspace.create(Fresh::Program, &mut log).unwrap();
        let held = workspace.get(program).unwrap();
        assert_eq!(
            sized(held).unwrap().text,
            room::measure(held.bytes.len() as u64)
        );
    }

    /// A typed name keeps the tag the stored one carries, because the glyph beside the
    /// box says what kind of file it is and the box never shows the tag.
    #[test]
    fn a_renamed_asset_keeps_the_tag_its_stored_name_carries() {
        assert_eq!(
            tagged("Africa-Split.ne5p", "Kenya Split"),
            "Kenya Split.ne5p"
        );
        assert_eq!(tagged("Africa Split", "Kenya Split"), "Kenya Split");
        assert_eq!(tagged("notes.txt", "list"), "list.txt");
    }

    /// One phrase per mark, the queue's own word where a write is waiting, and never red
    /// — whatever the mark and whatever the editor calls an edit.
    #[test]
    fn every_mark_has_one_phrase_and_none_of_them_is_red() {
        let marks = [Mark::Unsaved, Mark::Agrees, Mark::Differs, Mark::Unknown];
        for mark in marks {
            for waiting in [false, true] {
                for edited in [None, Some("trimmed")] {
                    let held = phrase(mark, waiting, edited);
                    assert!(!held.words.is_empty(), "{mark:?} says something");
                    assert_eq!(held.hint, mark_words(mark), "{mark:?} explains itself");
                    // Only `dark_mode` decides an ink, so egui's own two faces answer.
                    for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
                        assert_ne!(
                            held.ink.color(&visuals),
                            crate::app::bad(&visuals),
                            "{mark:?} in the header"
                        );
                    }
                }
            }
        }
        assert_eq!(phrase(Mark::Unsaved, false, None).words, "edited");
        assert_eq!(
            phrase(Mark::Unsaved, false, Some("trimmed")).words,
            "trimmed"
        );
        assert_eq!(phrase(Mark::Agrees, false, None).words, "matches keyboard");
        assert_eq!(
            phrase(Mark::Differs, false, None).words,
            "differs from keyboard"
        );
        assert_eq!(phrase(Mark::Differs, true, None).words, "waiting to send");
        assert_eq!(phrase(Mark::Unknown, false, None).words, "on the keyboard");
    }

    fn facts<'a>(device: &'a DeviceState, queue: &'a Queue, tags: &'a Tags) -> Facts<'a> {
        Facts {
            faces: &[Face::Edit],
            showing: Face::Edit,
            device,
            queue,
            tags,
            view: false,
            extras: Extras::default(),
        }
    }

    /// The loud action's three states: a send that can happen, a class this app does not
    /// write into, and an instrument that is not there.
    #[test]
    fn the_loud_action_says_which_of_its_three_states_it_is_in() {
        use crate::device::Device;

        let (queue, tags) = (Queue::default(), Tags::default());
        let (mut workspace, mut log) = workspace();
        let at = Location { bank: 6, slot: 3 };
        let bytes = crate::fields::blank::electro5_song();

        let unattached = Device::new(egui::Context::default());
        let id = workspace.ingest(
            "Blue Room.ne5t".into(),
            Origin::Device {
                class: ObjectClass::SetList,
                at,
            },
            bytes.clone(),
            &mut log,
        );
        let held = action(
            workspace.get(id).unwrap(),
            &facts(&unattached.state, &queue, &tags),
        );
        assert_eq!(held.tone, Tone::Idle, "{}", held.hint);
        assert_eq!(held.send, None, "a dashed action asks for nothing");
        assert!(
            held.hint.contains("no instrument attached"),
            "{}",
            held.hint
        );

        let mut attached = Device::new(egui::Context::default());
        attached.pretend_scanned(ObjectClass::SetList, 7, &["Blue Room"]);
        let held = action(
            workspace.get(id).unwrap(),
            &facts(&attached.state, &queue, &tags),
        );
        assert_eq!(held.tone, Tone::Ready);
        assert_eq!(held.send, Some((ObjectClass::SetList, at)));
        assert_eq!(held.label, "Queue send");
        assert_eq!(held.short, "Send");
        assert_eq!(held.hint, "replaces Set lists 7:4");

        // A piano is installed on the instrument rather than sent to it, so there is
        // nothing for the action to do however much room there is.
        let installed = workspace.ingest(
            "Grand.npno".into(),
            Origin::Device {
                class: ObjectClass::Piano,
                at,
            },
            bytes,
            &mut log,
        );
        let held = action(
            workspace.get(installed).unwrap(),
            &facts(&attached.state, &queue, &tags),
        );
        assert_eq!(held.tone, Tone::Idle);
        assert_eq!(
            held.hint,
            "Pianos are installed on the instrument, not sent to it"
        );
    }

    /// A project lives on this computer, so its loud action is a build — and the build
    /// refuses, because nothing here writes an nsmp from one yet.
    #[test]
    fn a_project_offers_a_build_that_refuses_rather_than_a_send() {
        let (queue, tags) = (Queue::default(), Tags::default());
        let device = crate::device::Device::new(egui::Context::default());
        let (held, id) = opened("clarinet.nsmpproj", project_bytes());
        let loud = action(held.get(id).unwrap(), &facts(&device.state, &queue, &tags));
        assert_eq!(loud.tone, Tone::Blocked);
        assert_eq!(loud.label, "Build → .nsmp");
        assert_eq!(loud.short, "Build");
        assert_eq!(loud.send, None);
    }

    fn wav_bytes() -> Vec<u8> {
        use nord_format::formats::nsmp::codec;
        let samples: Vec<i16> = (0..codec::SOURCE_RATE as usize)
            .map(|i| ((i as f64 / 40.0).sin() * 12_000.0) as i16)
            .collect();
        nord_format::wav::mono_pcm16(&samples, codec::SOURCE_RATE).unwrap()
    }

    fn sample_bytes() -> Vec<u8> {
        let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
        let options = nord_format::formats::nsmp::encode::Options::new("Marimba");
        nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    fn project_bytes() -> Vec<u8> {
        use nord_format::formats::nsmpproj::{NewZone, Project};
        let project = Project::new(
            "clarinet",
            &[NewZone {
                path: "low.wav".into(),
                sample_rate: 44100,
                frames: 44100,
                root_key: 48,
            }],
            0,
        )
        .unwrap();
        nord_format::to_bytes(&nord_format::Entity::SampleProject(project)).unwrap()
    }
}
