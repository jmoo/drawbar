//! The header strip at the top of every document.
//!
//! Left to right: what the document is, its name, where it lives, how big it is, its
//! state, which face is showing, and the one action this kind of document is for. A
//! kind with nothing for one of those parts leaves the part out.
//!
//! The strip runs edge to edge on the window fill with a hairline under it; only the
//! body below it has a margin.

use eframe::egui;
use nord_format::accept::Family;
use nord_usb::{Location, ObjectClass};

use super::controls::{self, Sets};
use super::{encode, piano, project, sample, setlist, text, SendBack, Shape};
use crate::app::{accent, caption, good, warn};
use crate::browser::Kind;
use crate::device::{read_only, DeviceState};
use crate::icon::{icon, painted, Glyph};
use crate::library::{keyboard_mark, mark_words, Mark};
use crate::panel::caps;
use crate::queue::Queue;
use crate::room;
use crate::strings::{display_name, folder, kind_word, place, shown, tagged};
use crate::tags::Tags;
use crate::workspace::{LocalEntity, Origin};

/// The strip's minimum height, its padding at each end, and the gap between two of its
/// parts.
const HEIGHT: f32 = 38.0;
const PAD: i8 = 12;
const GAP: f32 = 10.0;

/// The height of every control on the strip, and its corner radius.
const CONTROL: f32 = 20.0;
const RADIUS: f32 = 2.0;

/// The glyph sizes: the kind, a face or a quiet action, the loud action, a tag chip.
const KIND: f32 = 15.0;
const SMALL: f32 = 11.0;
const LOUD: f32 = 12.0;
const TAG: f32 = 10.0;

/// The state dot's box, which holds a 6 px dot (see [`claim`]).
const DOT: f32 = 8.0;

/// The space between a glyph and the word after it.
const INSET: f32 = 5.0;

/// The text size of a control's label, and of the mono badge, place, and size.
const WORD: f32 = 10.5;
const MONO: f32 = 10.5;

/// The text size of a read-only value on the identity row.
const READ: f32 = 11.5;

/// The widths of the name box, the piano's shorter name box, and the variant box.
const NAME: f32 = 190.0;
const PIANO_NAME: f32 = 150.0;
const VARIANT: f32 = 84.0;

/// The text size of a name the instrument owns, which is drawn as text with no box.
const FIXED: f32 = 12.5;

/// The identity row's indent, measured from the strip's outer edge, and its own height.
const INDENT: f32 = 37.0;
const CHIP: f32 = 18.0;

/// Which face of a document is showing.
///
/// Basic is the sound. Advanced is what the file says about itself and the engineering
/// under it. [`super::faces`] decides which faces a document has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Face {
    /// The panel, in the instrument's own words. Happy-path edits.
    #[default]
    Basic,
    /// What the file says about itself, the whole body as a table, and the record the
    /// container keeps.
    Advanced,
}

impl Face {
    pub fn label(self) -> &'static str {
        match self {
            Face::Basic => "Basic",
            Face::Advanced => "Advanced",
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Face::Basic => Glyph::Pencil,
            Face::Advanced => Glyph::Wrench,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Face::Basic => "the fields that change the sound",
            Face::Advanced => "engineering detail: the record, the offsets, the raw values",
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

/// The ink a header phrase or stroke may use.
///
/// ⚠️ There is no `bad` ink. The header never shows red; red is reserved for the
/// hatches in a key map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Good,
    Warn,
    /// The caption ink, for a figure that makes no claim.
    Quiet,
}

impl Ink {
    pub(super) fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
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
#[derive(Clone)]
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

/// The one loud action a document has, in one of its three states.
pub struct Loud {
    pub label: String,
    /// What it is called where there is no room for the whole label.
    pub short: String,
    pub glyph: Glyph,
    pub tone: Tone,
    pub hint: String,
    /// The slot a click would queue a write for. `None` means a click does nothing.
    pub send: Option<(ObjectClass, Location)>,
}

/// What an editor adds to the header that the asset alone does not say.
///
/// Each field that is set replaces what the strip would work out for itself: the
/// piano's `188 of 194 MB`, its `trimmed`, and its refusal to queue a library that does
/// not fit.
#[derive(Default)]
pub struct Extras {
    pub size: Option<SizeLine>,
    /// The state an unsaved document shows where the editor has something more specific
    /// than `edited`, such as the field document's `N pending`. It keeps its own ink.
    pub edited: Option<StateLine>,
    /// The state a saved document shows in place of the strip's own, such as a set list
    /// naming programs the instrument does not have where the list says.
    pub state: Option<StateLine>,
    pub loud: Option<Loud>,
}

/// One cell of the identity row: a MICRO-caps label and the value beside it.
pub struct Cell {
    pub label: &'static str,
    pub body: Body,
    /// A note drawn in the row instead of on hover, for a constraint the field enforces.
    pub note: Option<String>,
    pub hint: &'static str,
}

/// What a cell shows beside its label.
///
/// No kind is editable because no header-level field of any format has a setter: the
/// sample's category and sub name are stated by the file and only read here.
pub enum Body {
    /// One chip per label the list puts on this asset.
    Chips(Vec<String>),
    /// A value the file states, which is read-only here.
    Read(String),
}

/// Everything the strip draws from besides the asset itself.
pub(super) struct Facts<'a> {
    pub faces: &'a [Face],
    pub showing: Face,
    pub device: &'a DeviceState,
    pub queue: &'a Queue,
    pub tags: &'a Tags,
    /// Whether this is a view of the instrument's own copy, not an asset held here.
    pub view: bool,
    /// The name and variant a piano library's plan will save, from
    /// [`piano::State::renaming`], so the boxes hold what a save writes.
    pub renaming: (Option<String>, Option<String>),
    /// What the document is, which decides where its name is kept.
    pub shape: Shape,
    pub extras: Extras,
}

/// What the header's controls asked for this frame.
#[derive(Default)]
pub(super) struct Clicked {
    pub revert: bool,
    pub export: bool,
    pub send: Option<SendBack>,
    /// The face picked, where one was.
    pub face: Option<Face>,
    /// The asset's new name, where a rename settled. Applied once nothing is borrowing
    /// the asset.
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
            strip(
                ui,
                &mut act,
                |rhs, act| right(rhs, entity, facts, stage, act),
                |lhs, act| left(lhs, entity, facts, boxes, sets, act),
            );
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

/// One row of the strip. The right-hand group takes the width it needs at the right
/// edge. The left-hand group gets the rest and wraps onto a second line when it runs
/// out of room, so it never runs under the controls.
///
/// The right group is laid out first because its width decides the left group's room.
fn strip<T>(
    ui: &mut egui::Ui,
    state: &mut T,
    right: impl FnOnce(&mut egui::Ui, &mut T),
    left: impl FnOnce(&mut egui::Ui, &mut T),
) {
    let row = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), HEIGHT - 8.0),
    );
    let mut rhs = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    rhs.spacing_mut().item_spacing.x = GAP;
    right(&mut rhs, state);
    let taken = rhs.min_rect();
    let room = egui::Rect::from_min_max(row.min, egui::pos2(taken.left() - GAP, row.max.y));
    let mut lhs = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(room)
            .layout(egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true)),
    );
    lhs.spacing_mut().item_spacing = egui::vec2(GAP, 4.0);
    left(&mut lhs, state);
    ui.advance_cursor_after_rect(lhs.min_rect().union(taken).union(row));
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
    let glyph = Kind::of(entity).glyph();
    icon(ui, glyph, KIND, accent(&visuals));

    let (held, stored) = named(entity, facts.shape, facts.view, facts.renaming.clone());
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
        claim(ui, &state.words, ink).on_hover_text(&state.hint);
    }
}

/// The state dot and its phrase as one widget, so a wrapping row keeps them on the same
/// line.
fn claim(ui: &mut egui::Ui, words: &str, ink: egui::Color32) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(words.to_owned(), egui::FontId::proportional(WORD), ink);
    let size = egui::vec2(DOT + INSET + galley.size().x, CONTROL);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(
        egui::pos2(rect.left() + DOT / 2.0, rect.center().y),
        DOT / 2.0 - 1.0,
        ink,
    );
    painter.galley(
        egui::pos2(
            rect.left() + DOT + INSET,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        ink,
    );
    response
}

/// The faces, the quiet actions, and the loud action, left to right. The layout runs
/// right to left, so they are drawn in reverse.
fn right(
    ui: &mut egui::Ui,
    entity: &LocalEntity,
    facts: &Facts<'_>,
    stage: Stage,
    act: &mut Clicked,
) {
    let visuals = ui.visuals().clone();
    let quiet = caption(&visuals);
    let own = action(entity, facts.device);
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

    // Right to left: Export is drawn first so that Revert sits to its left.
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

/// A hover that explains the control, and also names it once the stage has hidden its
/// label.
fn hint(label: &str, why: &str, words: bool) -> String {
    match words {
        true => why.to_string(),
        false => format!("{why} ({label})"),
    }
}

/// The faces as one control: one stroke around the group, a rule between segments, and
/// the showing face filled. It holds only the faces this document has.
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

/// The identity row under the name: one cell after another, each a MICRO-caps label and
/// its value.
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

/// A value the file states: an eye glyph marking it read-only, and the value in mono.
fn read(ui: &mut egui::Ui, value: &str) {
    ui.spacing_mut().item_spacing.x = INSET;
    icon(ui, Glyph::Eye, SMALL, caption(ui.visuals()));
    ui.label(
        egui::RichText::new(value)
            .font(egui::FontId::monospace(READ))
            .color(ui.visuals().weak_text_color()),
    );
}

/// One tag chip, in the accent stroke and ink of the inspector's chips, and shorter
/// than the strip's controls.
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
    /// The word beside the glyph, or `None` where the stage has hidden it.
    label: Option<&'a str>,
    ink: egui::Color32,
    mono: bool,
    stroke: Option<egui::Color32>,
    dashed: bool,
    pad: f32,
    height: f32,
    /// Whether it highlights on hover and responds to a click.
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
            true => crate::panel::dashed_rect(&painter, rect.shrink(0.5), stroke),
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

/// A mono readout for a figure such as the place or the size.
fn mono(ui: &mut egui::Ui, text: &str, ink: egui::Color32) -> egui::Response {
    // A figure that does not fit moves to the next line whole; it never breaks.
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(egui::FontId::monospace(MONO))
                .color(ink),
        )
        .wrap_mode(egui::TextWrapMode::Extend),
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
    /// The file stores it, and the editor's `name` field sets it. `variant` is the
    /// piano's second half, which has a box of its own.
    Stored {
        limit: Option<usize>,
        variant: Option<String>,
        width: f32,
    },
    /// The asset's own name on this computer: a rename, not an edit to the file.
    Asset,
    /// The instrument's one settings block, which is not renamed.
    Device,
}

/// The name the file itself stores, where this shape keeps one, and how its box is laid
/// out. `renaming` holds what a piano library's plan will save each half as, where it
/// renames them.
fn stored_name(
    entity: &LocalEntity,
    shape: Shape,
    renaming: (Option<String>, Option<String>),
) -> Option<(Named, String)> {
    let decoded = entity.entity.as_ref()?;
    match shape {
        Shape::Sample => {
            let held = sample::snapshot(decoded)?.ok()?;
            Some((
                Named::Stored {
                    limit: Some(held.max_name_len),
                    variant: None,
                    width: NAME,
                },
                held.name,
            ))
        }
        Shape::Project => {
            let held = project::snapshot(decoded)?.ok()?;
            Some((
                Named::Stored {
                    limit: None,
                    variant: None,
                    width: NAME,
                },
                held.name,
            ))
        }
        Shape::Piano => {
            let held = piano::snapshot(decoded)?.ok()?;
            let (name, variant) = renaming;
            Some((
                Named::Stored {
                    limit: None,
                    variant: Some(variant.unwrap_or(held.variant)),
                    width: PIANO_NAME,
                },
                name.unwrap_or(held.name),
            ))
        }
        Shape::Fields
        | Shape::SetList
        | Shape::Text
        | Shape::Verbatim
        | Shape::Wav
        | Shape::Undecoded => None,
    }
}

/// What the name box holds, and what typing in it does.
fn named(
    entity: &LocalEntity,
    shape: Shape,
    view: bool,
    renaming: (Option<String>, Option<String>),
) -> (Named, String) {
    if let Some(held) = stored_name(entity, shape, renaming) {
        return held;
    }
    let settings = Kind::of(entity) == Kind::Settings;
    match view && settings {
        true => (Named::Device, display_name(&entity.name).to_string()),
        false => (Named::Asset, display_name(&entity.name).to_string()),
    }
}

/// What the name boxes hold when a document opens.
pub(super) fn boxes(
    entity: &LocalEntity,
    shape: Shape,
    view: bool,
    renaming: (Option<String>, Option<String>),
) -> (String, String) {
    let (held, stored) = named(entity, shape, view, renaming);
    let variant = match held {
        Named::Stored { variant, .. } => variant.unwrap_or_default(),
        Named::Asset | Named::Device => String::new(),
    };
    (stored, variant)
}

/// The name: a text box where this app can change it, and plain text where the
/// instrument owns it. Returns the asset's new name, where a rename settled.
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

/// A single-line name box that commits when editing finishes, not per keystroke, because
/// the format would accept a half-typed name.
///
/// ⚠️ Editing finishes when this box loses focus, which a single-line `TextEdit` does on
/// Enter. Reading Enter from the window would settle every name box on screen, and send
/// a value the format had already refused back to it on every Enter the operator pressed.
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
    if mono {
        edit = edit.font(egui::FontId::monospace(MONO));
    }
    let response = ui.add(edit);
    if let Some(limit) = limit {
        controls::fits(text, limit);
    }
    response.lost_focus()
}

/// The mono badge over a document, and the sentence behind it.
///
/// ⚠️ Exhaustive over [`Kind`], so a kind the browser learns must get its badge here and
/// never shows a blank one.
pub(super) fn badge(entity: &LocalEntity) -> (String, String) {
    let tag = entity.tag();
    let kind = Kind::of(entity);
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
        | Kind::Text
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

/// The stream version a piano library states, which is separate from the container's.
fn stream_version(entity: &LocalEntity) -> Option<u16> {
    match entity.entity.as_ref()? {
        nord_format::Entity::Piano(piano) => piano.stream_version().ok(),
        _ => None,
    }
}

/// Where the document lives: a slot on the instrument, a folder on this computer, or the
/// computer itself.
pub(super) fn lives(entity: &LocalEntity) -> String {
    if let Some((class, at)) = entity.spot() {
        return place(class, at);
    }
    match &entity.origin {
        Origin::File(path) => folder_of(path, home()),
        Origin::Rescued { at } => shown(*at),
        // Unreachable: a device origin is a slot, and `spot` already returned it.
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
/// A bare name with no folder, which is what a drop or the file picker hands over, reads
/// as `This computer`.
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

/// How big the document is, in the unit its kind uses: lines for a note, entries for a
/// set list, and bytes for anything else. Settings are one block and show no size.
fn sized(entity: &LocalEntity) -> Option<SizeLine> {
    let kind = Kind::of(entity);
    if kind == Kind::Settings {
        return None;
    }
    if kind == Kind::Text {
        return Some(match text::read(&entity.bytes).map(text::lines) {
            Ok(lines) => SizeLine {
                text: match lines {
                    1 => "1 line".to_string(),
                    lines => format!("{lines} lines"),
                },
                warn: false,
                hint: format!("{} bytes", entity.bytes.len()),
            },
            Err(why) => SizeLine {
                text: "not text".to_string(),
                warn: true,
                hint: why.to_string(),
            },
        });
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

/// The one state the header shows for this document: that it has unsaved edits, or how
/// it compares with what the attached instrument holds in its slot.
///
/// ⚠️ Unsaved is checked first: a slot that matches the saved bytes says nothing about
/// the edited ones in front of the reader.
fn state(entity: &LocalEntity, facts: &Facts<'_>) -> Option<StateLine> {
    let waiting = facts.queue.holds(entity.id);
    if entity.is_unsaved() {
        return Some(match &facts.extras.edited {
            Some(line) => line.clone(),
            None => phrase(Mark::Unsaved, waiting),
        });
    }
    if let Some(claim) = &facts.extras.state {
        return Some(claim.clone());
    }
    let mark = keyboard_mark(entity, facts.device, facts.queue)?;
    Some(phrase(mark, waiting))
}

/// The strip's short phrase for a mark, never in red.
///
/// ⚠️ Exhaustive over [`Mark`], and every arm's ink is an [`Ink`], which has no `bad`.
fn phrase(mark: Mark, waiting: bool) -> StateLine {
    let hint = mark_words(mark).to_string();
    match mark {
        Mark::Unsaved => StateLine {
            words: "edited".to_string(),
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
/// ⚠️ The checks are ordered so the label gives the right reason. A project has nothing
/// to send whatever is attached, and neither does a kind with no folder and no slot. An
/// unattached instrument cannot be written to whatever the asset is. A class this app
/// does not write into is never a question of room.
pub(super) fn action(entity: &LocalEntity, device: &DeviceState) -> Loud {
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

    if Kind::of(entity) == Kind::Project {
        return Loud {
            label: "Build → .nsmp".to_string(),
            short: "Build".to_string(),
            glyph: Glyph::CircleAlert,
            tone: Tone::Blocked,
            hint: "the codec is not understood yet".to_string(),
            send: None,
        };
    }
    let kind = Kind::of(entity);
    if entity.spot().is_none() && kind.home().is_none() {
        return idle(format!(
            "no instrument has a folder for this {}",
            kind.chip()
        ));
    }
    if !device.connected() {
        return idle("no instrument attached, so there is nothing to send to".to_string());
    }
    let Some((class, at)) = entity.spot() else {
        return idle("this is in no slot, so there is nothing to replace".to_string());
    };
    if read_only(class) {
        return idle(format!(
            "this app does not know what {} holds, so it writes nothing there",
            folder(class)
        ));
    }
    if let Some((over, free)) = over(entity, class, device) {
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

/// How far the document exceeds the free space in its folder, and that free space.
/// `None` where the folder does not count in bytes or the document fits.
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
/// ⚠️ An empty cell is left out, and a kind with no cells has no row: an indented empty
/// line under the name reads as a field that failed to draw.
fn identity(entity: &LocalEntity, tags: &Tags) -> Vec<Cell> {
    let mut cells = Vec::new();
    if Kind::of(entity) == Kind::Program {
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

    #[test]
    fn a_name_box_settles_only_on_an_enter_it_has_the_focus_for() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        let mut text = "Marimba".to_string();
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let at = |pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |events: Vec<egui::Event>, text: &mut String| {
            let mut done = false;
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 100.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    done = settled(ui, text, NAME, None, false);
                });
            });
            done
        };

        assert!(!frame(Vec::new(), &mut text), "nothing has happened");
        assert!(
            !frame(vec![key(egui::Key::Enter)], &mut text),
            "the box never had the focus"
        );
        let _ = frame(vec![at(egui::pos2(40.0, 20.0))], &mut text);
        assert!(
            frame(vec![key(egui::Key::Enter)], &mut text),
            "an Enter typed in the box settles it"
        );
    }

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

    /// The home directory is written `~`.
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
        // A folder that only shares the home directory's prefix is not inside it.
        assert_eq!(folder_of("/Users/xavier/a.nsmp", home()), "/Users/xavier");
        // A drop hands over a bare name, which names no folder.
        assert_eq!(folder_of("a.nsmp", home()), "This computer");
        assert_eq!(folder_of("/opt/a.nsmp", None), "/opt");
    }

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

    /// The badge names the format, and the hover gives the content or stream version.
    #[test]
    fn every_kind_says_what_format_it_is() {
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

        let (held, id) = opened("blank.ne5t", Fresh::SetList.bytes().unwrap());
        assert_eq!(
            badge(held.get(id).unwrap()),
            (
                "set list".to_string(),
                "Electro 5 set list, content version 1".to_string()
            )
        );

        let (held, id) = opened("junk.bin", vec![0x00, 0xff, 0x01, 0xfe]);
        assert_eq!(
            badge(held.get(id).unwrap()),
            ("?".to_string(), "these bytes did not decode".to_string())
        );

        let (held, id) = opened("Set 1.txt", b"Set 1\n".to_vec());
        assert_eq!(
            badge(held.get(id).unwrap()),
            ("txt".to_string(), "note".to_string()),
            "a note has no container, so its bytes decide its badge"
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

    /// A set list is measured in entries, settings show no size, and anything else is
    /// measured in bytes.
    #[test]
    fn the_size_is_whatever_this_kind_measures_in() {
        let (mut workspace, mut log) = workspace();
        let settings = workspace.create(Fresh::Settings, &mut log).unwrap();
        assert!(sized(workspace.get(settings).unwrap()).is_none());

        let (held, id) = opened("blank.ne5t", Fresh::SetList.bytes().unwrap());
        assert_eq!(sized(held.get(id).unwrap()).unwrap().text, "4 entries");

        let program = workspace.create(Fresh::Program, &mut log).unwrap();
        let held = workspace.get(program).unwrap();
        assert_eq!(
            sized(held).unwrap().text,
            room::measure(held.bytes.len() as u64)
        );
    }

    /// A typed name keeps the stored name's tag, because the box never shows the tag and
    /// the glyph beside it gives the kind of file.
    #[test]
    fn a_renamed_asset_keeps_the_tag_its_stored_name_carries() {
        assert_eq!(
            tagged("Africa-Split.ne5p", "Kenya Split"),
            "Kenya Split.ne5p"
        );
        assert_eq!(tagged("Africa Split", "Kenya Split"), "Kenya Split");
        assert_eq!(tagged("notes.txt", "list"), "list.txt");
    }

    /// One phrase per mark, the queue's word where a write is waiting, and never red.
    #[test]
    fn every_mark_has_one_phrase_and_none_of_them_is_red() {
        let marks = [Mark::Unsaved, Mark::Agrees, Mark::Differs, Mark::Unknown];
        for mark in marks {
            for waiting in [false, true] {
                let held = phrase(mark, waiting);
                assert!(!held.words.is_empty(), "{mark:?} says something");
                assert_eq!(held.hint, mark_words(mark), "{mark:?} explains itself");
                // Only `dark_mode` decides an ink, so egui's two defaults cover every case.
                for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
                    assert_ne!(
                        held.ink.color(&visuals),
                        crate::app::bad(&visuals),
                        "{mark:?} in the header"
                    );
                }
            }
        }
        assert_eq!(phrase(Mark::Unsaved, false).words, "edited");
        assert_eq!(phrase(Mark::Agrees, false).words, "matches keyboard");
        assert_eq!(phrase(Mark::Differs, false).words, "differs from keyboard");
        assert_eq!(phrase(Mark::Differs, true).words, "waiting to send");
        assert_eq!(phrase(Mark::Unknown, false).words, "on the keyboard");
    }

    fn facts<'a>(device: &'a DeviceState, queue: &'a Queue, tags: &'a Tags) -> Facts<'a> {
        Facts {
            faces: &[Face::Basic],
            showing: Face::Basic,
            device,
            queue,
            tags,
            view: false,
            renaming: (None, None),
            shape: Shape::Fields,
            extras: Extras::default(),
        }
    }

    /// An editor's own phrase for an unsaved document replaces `edited` and keeps the
    /// editor's ink.
    #[test]
    fn an_editors_own_state_phrase_stands_in_its_own_ink() {
        let (queue, tags) = (Queue::default(), Tags::default());
        let device = crate::device::Device::new(egui::Context::default());
        let (mut workspace, mut log) = workspace();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let mut edited = workspace.get(id).unwrap().bytes.clone();
        *edited.last_mut().expect("a byte to move") ^= 0xff;
        workspace.replace_bytes(id, edited, &mut log);

        let mut facts = facts(&device.state, &queue, &tags);
        let held = state(workspace.get(id).unwrap(), &facts).expect("an unsaved document");
        assert_eq!((held.words.as_str(), held.ink), ("edited", Ink::Warn));

        facts.extras.edited = Some(StateLine {
            words: "applying…".to_string(),
            ink: Ink::Quiet,
            hint: "laying the plan out over the library".to_string(),
        });
        let held = state(workspace.get(id).unwrap(), &facts).expect("an unsaved document");
        assert_eq!(held.words, "applying…");
        assert_eq!(held.hint, "laying the plan out over the library");
        assert_eq!(
            held.ink,
            Ink::Quiet,
            "the editor's ink, not the strip's warn"
        );
    }

    /// The loud action's three states: a send that can happen, a folder with no room
    /// for it, and an instrument that is not there.
    #[test]
    fn the_loud_action_says_which_of_its_three_states_it_is_in() {
        use crate::device::Device;
        use nord_usb::wire::Status;

        let (mut workspace, mut log) = workspace();
        let at = Location { bank: 6, slot: 3 };
        let bytes = Fresh::SetList.bytes().unwrap();

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
        let held = action(workspace.get(id).unwrap(), &unattached.state);
        assert_eq!(held.tone, Tone::Idle, "{}", held.hint);
        assert_eq!(held.send, None, "an idle action queues nothing");
        assert!(
            held.hint.contains("no instrument attached"),
            "{}",
            held.hint
        );

        let mut attached = Device::new(egui::Context::default());
        attached.pretend_scanned(ObjectClass::SetList, 7, &["Blue Room"]);
        let held = action(workspace.get(id).unwrap(), &attached.state);
        assert_eq!(held.tone, Tone::Ready);
        assert_eq!(held.send, Some((ObjectClass::SetList, at)));
        assert_eq!(held.label, "Queue send");
        assert_eq!(held.short, "Send");
        assert_eq!(held.hint, "replaces Set lists 7:4");

        // A piano in a slot is sent like anything else. The Pianos partition has not
        // reported its free space, and an unknown free space is no reason to refuse.
        let in_pianos = Location { bank: 0, slot: 3 };
        let library = workspace.ingest(
            "Grand.npno".into(),
            Origin::Device {
                class: ObjectClass::Piano,
                at: in_pianos,
            },
            bytes.clone(),
            &mut log,
        );
        let held = action(workspace.get(library).unwrap(), &attached.state);
        assert_eq!(held.tone, Tone::Ready, "{}", held.hint);
        assert_eq!(held.send, Some((ObjectClass::Piano, in_pianos)));
        assert_eq!(held.hint, "replaces Pianos 1:4");

        // The same library against a Pianos partition with nothing left in it.
        let mut full = Device::new(egui::Context::default());
        full.pretend_scanned(ObjectClass::Piano, 1, &["Royal Grand 3D"]);
        full.pretend_partitions(&crate::device::ELECTRO5);
        full.state.inventory.push(Status {
            class: ObjectClass::Piano,
            count: 1,
            free: 0,
            used: 1072,
            dirty: 0,
            spare: 0,
        });
        let held = action(workspace.get(library).unwrap(), &full.state);
        assert_eq!(held.tone, Tone::Blocked);
        assert_eq!(
            held.label,
            format!("Won't fit · {} over", room::measure(bytes.len() as u64))
        );
        assert_eq!(held.send, None, "a blocked action queues nothing");
        assert!(held.hint.contains("free in Pianos"), "{}", held.hint);
    }

    /// A project lives on this computer, so its loud action is a build. The build is
    /// blocked because this app cannot yet write an nsmp from a project.
    #[test]
    fn a_project_offers_a_blocked_build_in_place_of_a_send() {
        let device = crate::device::Device::new(egui::Context::default());
        let (held, id) = opened("clarinet.nsmpproj", project_bytes());
        let loud = action(held.get(id).unwrap(), &device.state);
        assert_eq!(loud.tone, Tone::Blocked);
        assert_eq!(loud.label, "Build → .nsmp");
        assert_eq!(loud.short, "Build");
        assert_eq!(loud.send, None);
    }

    #[test]
    fn a_note_says_no_instrument_has_a_folder_for_it() {
        let mut device = crate::device::Device::new(egui::Context::default());
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);
        let (held, id) = opened("Set 1.txt", b"Set 1\n".to_vec());
        let loud = action(held.get(id).unwrap(), &device.state);
        assert_eq!(loud.tone, Tone::Idle);
        assert_eq!(loud.send, None);
        assert_eq!(loud.hint, "no instrument has a folder for this note");
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
