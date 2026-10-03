//! The keyboard tab: the attached instrument, one folder at a time.
//!
//! A folder divided into banks of equal slots is drawn as a map; a folder whose items
//! differ in size and content is drawn as a list. Both draw the same slot, so a cell and
//! a row respond to the same gestures and offer the same menu.

use std::collections::BTreeMap;
use std::ops::Range;

use eframe::egui;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, caption, tint, ui as ui_text, warn};
use crate::browser::{cell_ink, Act, Browser, Held, Item, Kind, Onto};
use crate::device::{occupancy, read_only, Device};
use crate::icon::{painted, Glyph};
use crate::library::Needs;
use crate::panel::{
    cut, list_width, row_ink, tonal_button, view_header, Track, GAP, GLYPH, PAD, ROW_INSET,
    VIEW_PAD,
};
use crate::queue::{Queue, Queued};
use crate::room;
use crate::strings::{place, shown};
use crate::tabs::Tabs;
use crate::workspace::Workspace;

/// The space under each band above the folder.
const UNDER: f32 = 10.0;

/// The switcher: a segment's height and side padding, and the track's padding around
/// the segments and between them.
const SEGMENT: f32 = 28.0;
const SEGMENT_PAD: f32 = 11.0;
const TRACK_PAD: i8 = 3;
const TRACK_GAP: f32 = 2.0;

/// A bank square's size.
const BANK: egui::Vec2 = egui::vec2(28.0, 26.0);

/// The heights of a list's column heads and of one row, the gap between two rows, and
/// the padding at each end of a row.
const HEAD: f32 = 28.0;
const ROW: f32 = 32.0;
const ROW_GAP: f32 = 2.0;
const CELL_PAD: f32 = 10.0;

/// How the slots of a bank are laid out: a card's height, the gap between cards, and
/// the margin at each side.
struct Lattice {
    height: f32,
    gap: f32,
    margin: f32,
}

/// The map's cards, as many to a row as fit at [`CARD_MIN`] or wider.
const MAP: Lattice = Lattice {
    height: 54.0,
    gap: 6.0,
    margin: VIEW_PAD,
};
const CARD_MIN: f32 = 150.0;

/// A picker's cells, a count to a row the picker chooses, each [`CELL`] wide.
const PICKER: Lattice = Lattice {
    height: CELL,
    gap: 4.0,
    margin: PAD,
};
const CELL: f32 = 42.0;

/// The rounding of a card, in the map or a picker.
const CARD_RADIUS: f32 = 9.0;

/// How a card's contents are set: its padding, the sizes of its address and its name,
/// and the size of its state's glyph.
struct Face {
    pad: egui::Vec2,
    address: f32,
    name: f32,
    glyph: f32,
}

/// A map card's text, and a picker cell's, which has less room.
const CARD_FACE: Face = Face {
    pad: egui::vec2(10.0, 7.0),
    address: 10.5,
    name: 12.5,
    glyph: SMALL,
};
const CELL_FACE: Face = Face {
    pad: egui::vec2(3.0, 4.0),
    address: 9.5,
    name: 11.0,
    glyph: 10.0,
};

/// The size of a state's glyph on a card or at the end of a row.
const SMALL: f32 = 12.0;

/// Font sizes for this view. It paints its text directly, so the sizes are set here
/// instead of taken from the named styles in [`crate::app`].
const NAME: f32 = 13.0;
const TEXT: f32 = 12.0;
const MONO: f32 = 11.5;
const READOUT: f32 = 10.5;

/// The gap between two columns of a list.
const LIST_GAP: f32 = 12.0;

/// The columns every list shares: address, name, size, the folder's own fact, and the
/// state at the end.
const LIST: [Track; 5] = [
    Track::Px(58.0),
    Track::Share(1.5),
    Track::Px(74.0),
    Track::Share(1.0),
    Track::Px(SMALL),
];

/// The center's view of the attached instrument.
#[derive(Default)]
pub struct Keyboard {
    /// The bank each folder shows, by raw class number. A folder keeps its bank while
    /// another folder is shown.
    banks: BTreeMap<u32, u32>,
}

/// Everything a slot is drawn from, other than the slot itself.
struct View<'a> {
    class: ObjectClass,
    /// Every slot of the list this one sits in, for a ⇧-click.
    list: &'a [Item],
    workspace: &'a Workspace,
    device: &'a Device,
    queue: &'a Queue,
}

impl Keyboard {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        browser: &mut Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        tabs: &Tabs,
    ) -> Vec<Act> {
        let mut acts = Vec::new();
        if !device.state.connected() {
            nothing(ui, "Nothing is attached.");
            return acts;
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let class = tabs.keyboard_class().unwrap_or(ObjectClass::Program);
        header(ui, device, class, &mut acts);
        switcher(ui, device, class, &mut acts);
        match class {
            ObjectClass::Program | ObjectClass::Live => {
                self.map(ui, class, browser, workspace, device, queue, &mut acts)
            }
            _ => list(ui, class, browser, workspace, device, queue, &mut acts),
        }
        acts
    }

    /// The bank a folder shows: the one picked, if the folder still has it, otherwise the
    /// first bank read.
    fn bank(&self, class: ObjectClass, banks: &[u32]) -> Option<u32> {
        self.banks
            .get(&class.to_raw())
            .copied()
            .filter(|held| banks.contains(held))
            .or_else(|| banks.first().copied())
    }

    /// A bank of equal slots, as many cards to a row as fit.
    #[allow(clippy::too_many_arguments)]
    fn map(
        &mut self,
        ui: &mut egui::Ui,
        class: ObjectClass,
        browser: &mut Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let banks = device.state.banks_of(class);
        let Some((bank, slots)) = self
            .bank(class, &banks)
            .and_then(|bank| Some((bank, device.state.bank(class, bank)?)))
        else {
            return nothing(ui, "Nothing read yet.");
        };
        let held = slots.iter().filter(|slot| slot.is_some()).count();
        let incoming = (0..slots.len())
            .filter(|index| {
                queue
                    .waiting(class, Location::from_user(bank, *index as u32 + 1))
                    .is_some()
            })
            .count();
        let said = sentence(held, slots.len(), incoming);
        if let Some(picked) = bank_row(ui, bank, &banks, &said) {
            self.banks.insert(class.to_raw(), picked);
        }

        let list: Vec<Item> = (0..slots.len())
            .map(|index| Item::Slot {
                class,
                at: Location::from_user(bank, index as u32 + 1),
            })
            .collect();
        let view = View {
            class,
            list: &list,
            workspace,
            device,
            queue,
        };
        egui::ScrollArea::vertical()
            .id_salt("keyboard_map")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let columns = columns(ui.available_width());
                lay(ui, &MAP, columns, slots.len(), |ui, index, rect| {
                    let at = Location::from_user(bank, index as u32 + 1);
                    cell(ui, browser, &view, rect, at, slots[index].as_ref(), acts);
                });
                ui.add_space(MAP.margin - MAP.gap);
            });
    }
}

/// How many map cards of at least [`CARD_MIN`] fit across `width`, and never none.
fn columns(width: f32) -> usize {
    let room = width - 2.0 * MAP.margin + MAP.gap;
    ((room / (CARD_MIN + MAP.gap)).floor() as usize).max(1)
}

/// Lay out a bank of slots as a picker's grid of 42 px cells, `columns` across. `each`
/// gets one slot's index and its rect.
pub fn grid(
    ui: &mut egui::Ui,
    columns: usize,
    slots: usize,
    each: impl FnMut(&mut egui::Ui, usize, egui::Rect),
) {
    ui.add_space(PICKER.gap);
    lay(ui, &PICKER, columns, slots, each);
}

/// The width a picker's grid of `columns` cells needs.
pub fn grid_width(columns: usize) -> f32 {
    2.0 * PICKER.margin + CELL * columns as f32 + PICKER.gap * (columns.saturating_sub(1)) as f32
}

/// Lay out `slots` cards `columns` across, filling the width inside the lattice's
/// margins. `each` gets one slot's index and its rect.
fn lay(
    ui: &mut egui::Ui,
    lattice: &Lattice,
    columns: usize,
    slots: usize,
    mut each: impl FnMut(&mut egui::Ui, usize, egui::Rect),
) {
    let columns = columns.max(1);
    for row in 0..slots.div_ceil(columns) {
        let (strip, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), lattice.height + lattice.gap),
            egui::Sense::hover(),
        );
        let room = strip.width() - 2.0 * lattice.margin - lattice.gap * (columns - 1) as f32;
        let width = (room / columns as f32).max(0.0);
        for column in 0..columns {
            let index = row * columns + column;
            if index >= slots {
                break;
            }
            let rect = egui::Rect::from_min_size(
                egui::pos2(
                    strip.left() + lattice.margin + (width + lattice.gap) * column as f32,
                    strip.top(),
                ),
                egui::vec2(width, lattice.height),
            );
            each(ui, index, rect);
        }
    }
}

/// The header block: what is attached, how old the shown contents are, and a button to
/// read them again.
fn header(ui: &mut egui::Ui, device: &Device, class: ObjectClass, acts: &mut Vec<Act>) {
    let now = ui.input(|input| input.time);
    let said = freshness(device, class, now);
    let product = device.state.product().unwrap_or_default().to_string();
    let good = crate::app::good(ui.visuals());
    view_header(ui, Glyph::Keyboard, good, &product, &said, |ui| {
        if tonal_button(ui, Some(Glyph::RefreshCw), "Read again")
            .on_hover_text(format!("read {} again", device.state.folder_name(class)))
            .clicked()
        {
            acts.push(Act::ReadAgain(class));
        }
    });
}

/// The firmware, and how long ago the shown folder was read.
fn freshness(device: &Device, class: ObjectClass, now: f64) -> String {
    let mut said: Vec<String> = device.state.firmware().into_iter().collect();
    if let Some(at) = device.state.scan.read_at(class) {
        said.push(ago(now - at));
    }
    said.join(" · ")
}

/// How long ago something was read, short enough for the header.
fn ago(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    match seconds < 60.0 {
        true => format!("read {} s ago", seconds as u64),
        false => format!("read {} min ago", (seconds / 60.0) as u64),
    }
}

/// The switcher: one segment per folder on a sunken track, with the shown one raised.
///
/// Switching reads nothing. Every folder here has already been read, and the header's
/// button reads it again.
fn switcher(ui: &mut egui::Ui, device: &Device, on: ObjectClass, acts: &mut Vec<Act>) {
    let classes = device.state.classes();
    let rooms: Vec<Option<String>> = classes
        .iter()
        .map(|class| {
            occupancy(
                *class,
                &device.state.inventory,
                device.state.allocation_unit(*class),
            )
        })
        .collect();
    let canvas = crate::app::canvas(ui.visuals());
    band(ui, |ui| {
        egui::Frame::new()
            .fill(canvas)
            .corner_radius(10.0)
            .inner_margin(TRACK_PAD)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::splat(TRACK_GAP);
                    for (class, room) in classes.iter().zip(&rooms) {
                        let picked = segment(
                            ui,
                            Kind::from_class(*class).glyph(),
                            device.state.folder_name(*class),
                            room.as_deref(),
                            *class == on,
                        )
                        .clicked();
                        if picked {
                            acts.push(Act::ShowClass(*class));
                        }
                    }
                });
            });
    });
}

/// One segment of the switcher: a glyph, a word, and an optional monospace readout after
/// it. The shown folder's segment is raised off the track.
fn segment(
    ui: &mut egui::Ui,
    glyph: Glyph,
    word: &str,
    readout: Option<&str>,
    on: bool,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let word = painter.layout_no_wrap(
        word.to_string(),
        ui_text().resolve(ui.style()),
        egui::Color32::PLACEHOLDER,
    );
    let readout = readout.map(|held| {
        painter.layout_no_wrap(
            held.to_string(),
            egui::FontId::monospace(READOUT),
            egui::Color32::PLACEHOLDER,
        )
    });
    let counted = readout.as_ref().map_or(0.0, |held| GAP + held.size().x);
    let width = SEGMENT_PAD + GLYPH + GAP + word.size().x + counted + SEGMENT_PAD;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, SEGMENT), egui::Sense::click());

    let ink = match (on, response.hovered()) {
        (true, _) => visuals.widgets.active.fg_stroke.color,
        (false, true) => visuals.widgets.hovered.fg_stroke.color,
        (false, false) => caption(&visuals),
    };
    if on {
        let lift = egui::Shadow {
            offset: [0, 1],
            blur: 2,
            spread: 0,
            color: egui::Color32::from_black_alpha(64),
        };
        painter.add(lift.as_shape(rect, 7.0));
        painter.rect_filled(rect, 7.0, visuals.panel_fill);
    }
    let mut x = rect.left() + SEGMENT_PAD;
    painted(
        ui,
        glyph,
        egui::Rect::from_center_size(
            egui::pos2(x + GLYPH / 2.0, rect.center().y),
            egui::Vec2::splat(GLYPH),
        ),
        ink,
    );
    x += GLYPH + GAP;
    let word_width = word.size().x;
    painter.galley(
        egui::pos2(x, rect.center().y - word.size().y / 2.0),
        word,
        ink,
    );
    if let Some(readout) = readout {
        painter.galley(
            egui::pos2(
                x + word_width + GAP,
                rect.center().y - readout.size().y / 2.0,
            ),
            readout,
            ink.gamma_multiply(0.75),
        );
    }
    response
}

/// The bank row: the folder's banks, and what the shown one holds. Returns the bank
/// clicked, if any.
fn bank_row(ui: &mut egui::Ui, on: u32, banks: &[u32], said: &str) -> Option<u32> {
    let mut picked = None;
    band(ui, |ui| {
        let quiet = caption(ui.visuals());
        ui.label(egui::RichText::new("Bank").size(TEXT).color(quiet));
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for bank in banks {
                if square(ui, &bank.to_string(), *bank == on).clicked() {
                    picked = Some(*bank);
                }
            }
        });
        ui.label(egui::RichText::new(said).size(TEXT).color(quiet));
    });
    picked
}

/// One bank's square: its number, ringed and washed in the accent while it is shown.
fn square(ui: &mut egui::Ui, name: &str, on: bool) -> egui::Response {
    let visuals = ui.visuals().clone();
    let ink = match on {
        true => visuals.widgets.active.fg_stroke.color,
        false => caption(&visuals),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_string(), egui::FontId::monospace(MONO), ink);
    let size = egui::vec2(BANK.x.max(galley.size().x + 8.0), BANK.y);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let accent = accent(&visuals);
    let (fill, edge) = match (on, response.hovered()) {
        (true, _) => (tint(accent, 0.14), tint(accent, 0.6)),
        (false, true) => (visuals.window_fill, visuals.widgets.hovered.bg_stroke.color),
        (false, false) => (visuals.window_fill, egui::Color32::TRANSPARENT),
    };
    ui.painter().rect(
        rect,
        7.0,
        fill,
        egui::Stroke::new(1.0_f32, edge),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        egui::Color32::PLACEHOLDER,
    );
    response
}

/// What a bank holds and what is queued for it.
pub fn sentence(held: usize, slots: usize, incoming: usize) -> String {
    let said = format!("{held} of {slots} slots hold something");
    match incoming {
        0 => said,
        n => format!("{said} · {n} incoming"),
    }
}

/// A map slot's state: what it holds, and what is about to happen to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Empty,
    Held,
    Incoming,
    Loaded,
}

impl State {
    /// ⚠️ Loaded wins. A player must always be able to find the slot the instrument is
    /// playing, whatever else is true of it.
    pub fn of(loaded: bool, incoming: bool, held: bool) -> State {
        if loaded {
            return State::Loaded;
        }
        if incoming {
            return State::Incoming;
        }
        match held {
            true => State::Held,
            false => State::Empty,
        }
    }

    /// The border, if the state has one of its own.
    fn edge(self, visuals: &egui::Visuals) -> Option<egui::Color32> {
        match self {
            State::Empty => Some(visuals.widgets.noninteractive.bg_stroke.color),
            State::Held => None,
            State::Incoming => Some(tint(warn(visuals), 0.7)),
            State::Loaded => Some(tint(accent(visuals), 0.7)),
        }
    }

    /// The glyph at the top right, if the state has one.
    fn glyph(self, visuals: &egui::Visuals) -> Option<(Glyph, egui::Color32)> {
        match self {
            State::Incoming => Some((Glyph::ArrowDownToLine, warn(visuals))),
            State::Loaded => Some((Glyph::CircleDot, accent(visuals))),
            State::Empty | State::Held => None,
        }
    }
}

/// One slot of the map: a full-width click target with its parts painted into it.
///
/// ⚠️ Nothing inside is a widget, for the reason [`crate::browser::Cells`] gives: a label
/// allocates a hover rect that wins the hit test over the cell, and the click lands on
/// whichever word is under the pointer.
fn cell(
    ui: &mut egui::Ui,
    browser: &mut Browser,
    view: &View,
    rect: egui::Rect,
    at: Location,
    info: Option<&ProgramInfo>,
    acts: &mut Vec<Act>,
) {
    let class = view.class;
    let item = Item::Slot { class, at };
    let selected = browser.picked().holds(item);
    let response = ui.interact(
        rect,
        ui.id().with(("cell", class.to_raw(), at.bank, at.slot)),
        egui::Sense::click_and_drag(),
    );
    let state = State::of(
        view.device.state.focused(class) == Some(at),
        view.queue.waiting(class, at).is_some(),
        info.is_some(),
    );
    paint_cell(ui, rect, at, info, state, selected, response.hovered());
    let response = response.on_hover_text(hint(view, at, info, state));
    gestures(ui, browser, view, at, info, &response, acts);
}

/// A slot painted as a card: the fill for its state and selection, the border, the
/// address, and what it holds. The caller handles input.
///
/// An empty slot has a dashed border and no fill. Under the pointer, every border takes
/// the hovered stroke.
pub fn paint_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    at: Location,
    info: Option<&ProgramInfo>,
    state: State,
    selected: bool,
    hovered: bool,
) {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let lit = selected || state == State::Loaded;
    let fill = match (lit, state) {
        (true, _) => Some(visuals.selection.bg_fill),
        (false, State::Empty) => None,
        (false, _) => Some(visuals.window_fill),
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, CARD_RADIUS, fill);
    }
    let edge = match hovered {
        true => Some(visuals.widgets.hovered.bg_stroke.color),
        false => state.edge(&visuals),
    };
    if let Some(edge) = edge {
        let stroke = egui::Stroke::new(1.0_f32, cell_ink(selected, edge, &visuals));
        match state {
            State::Empty => {
                crate::panel::dashed_round_rect(&painter, rect.shrink(0.5), CARD_RADIUS, stroke)
            }
            _ => {
                painter.rect_stroke(rect, CARD_RADIUS, stroke, egui::StrokeKind::Inside);
            }
        }
    }

    let ink = cell_ink(lit, visuals.text_color(), &visuals);
    let quiet = cell_ink(lit, visuals.weak_text_color(), &visuals);
    let face = match rect.height() < MAP.height {
        true => &CELL_FACE,
        false => &CARD_FACE,
    };
    let inner = rect.shrink2(face.pad);
    let glyph = state.glyph(&visuals);
    let beside = glyph.map_or(0.0, |_| face.glyph);
    cut(
        &painter,
        inner.left(),
        inner.top() + face.glyph / 2.0,
        inner.width() - beside,
        &shown(at),
        egui::TextFormat::simple(egui::FontId::monospace(face.address), quiet),
    );
    if let Some((glyph, tint)) = glyph {
        painted(
            ui,
            glyph,
            egui::Rect::from_min_size(
                egui::pos2(inner.right() - face.glyph, inner.top()),
                egui::Vec2::splat(face.glyph),
            ),
            cell_ink(lit, tint, &visuals),
        );
    }
    let name = info.map(|info| info.name.trim()).unwrap_or_default();
    let (name, format) = match name.is_empty() {
        true => (
            "empty",
            egui::TextFormat {
                italics: true,
                ..egui::TextFormat::simple(egui::FontId::proportional(face.name), quiet)
            },
        ),
        false => (
            name,
            egui::TextFormat::simple(egui::FontId::proportional(face.name), ink),
        ),
    };
    cut(
        &painter,
        inner.left(),
        inner.bottom() - face.name / 2.0,
        inner.width(),
        name,
        format,
    );
}

/// A folder whose items differ in size and content, drawn as a list.
#[allow(clippy::too_many_arguments)]
fn list(
    ui: &mut egui::Ui,
    class: ObjectClass,
    browser: &mut Browser,
    workspace: &Workspace,
    device: &Device,
    queue: &Queue,
    acts: &mut Vec<Act>,
) {
    let banks = device.state.banks_of(class);
    if banks.is_empty() {
        return nothing(ui, "Nothing read yet.");
    }
    // A library partition fills by bytes, not slots, so its free space decides whether
    // the next send fits.
    if let Some(held) = class
        .is_library()
        .then(|| room::meter(class, &device.state, queue, workspace))
        .flatten()
    {
        egui::TopBottomPanel::bottom("keyboard_room")
            .resizable(false)
            .frame(egui::Frame::new())
            .show_inside(ui, |ui| boxed(ui, |ui| meter(ui, class, device, held)));
    }
    // Settings is a single live object, so what a queued write would change is shown
    // here as well as in the send queue.
    if class == ObjectClass::Settings {
        if let Some(held) = waiting_in(queue, class) {
            egui::TopBottomPanel::bottom("keyboard_settings")
                .resizable(false)
                .frame(egui::Frame::new())
                .show_inside(ui, |ui| boxed(ui, |ui| crate::queue::table(ui, held)));
        }
    }

    let slots: Vec<(Location, Option<&ProgramInfo>)> = banks
        .iter()
        .flat_map(|bank| {
            device
                .state
                .bank(class, *bank)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(index, info)| (Location::from_user(*bank, index as u32 + 1), info.as_ref()))
        })
        .collect();
    let items: Vec<Item> = slots
        .iter()
        .map(|(at, _)| Item::Slot { class, at: *at })
        .collect();

    let room = ui.available_rect_before_wrap();
    let ui = &mut ui.new_child(
        egui::UiBuilder::new()
            .max_rect(room.shrink2(egui::vec2(ROW_INSET, 0.0)))
            .layout(*ui.layout()),
    );
    let width = list_width(ui, slots.len(), ROW + ROW_GAP, HEAD + ROW_INSET);
    let tracks = crate::panel::tracks((width - 2.0 * CELL_PAD).max(0.0), &LIST, LIST_GAP);
    head(ui, class, width, &tracks);
    ui.add_space(ROW_INSET);

    let view = View {
        class,
        list: &items,
        workspace,
        device,
        queue,
    };
    ui.spacing_mut().item_spacing.y = ROW_GAP;
    egui::ScrollArea::vertical()
        .id_salt("keyboard_list")
        .auto_shrink([false; 2])
        .show_rows(ui, ROW, slots.len(), |ui, rows| {
            for (at, info) in rows.filter_map(|index| slots.get(index)) {
                row(ui, browser, &view, width, &tracks, *at, *info, acts);
            }
        });
}

/// The heading of a folder's fourth column, for the fact only that folder's list
/// carries.
fn column(class: ObjectClass) -> &'static str {
    match class {
        ObjectClass::SetList => "Plays",
        ObjectClass::Sample => "Played by",
        ObjectClass::Piano => "Category",
        _ => "",
    }
}

/// The column heads over a list, on a hairline.
fn head(ui: &mut egui::Ui, class: ObjectClass, width: f32, tracks: &[Range<f32>]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, HEAD), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
    );
    let ink = caption(&visuals);
    let font = crate::app::section().resolve(ui.style());
    let content = rect.shrink2(egui::vec2(CELL_PAD, 0.0));
    for (head, track) in ["At", "Name", "Size", column(class), ""].iter().zip(tracks) {
        cut(
            &painter,
            content.left() + track.start,
            content.center().y,
            track.end - track.start,
            head,
            egui::TextFormat::simple(font.clone(), ink),
        );
    }
}

/// One row of a list.
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    browser: &mut Browser,
    view: &View,
    width: f32,
    tracks: &[Range<f32>],
    at: Location,
    info: Option<&ProgramInfo>,
    acts: &mut Vec<Act>,
) {
    let class = view.class;
    let item = Item::Slot { class, at };
    let selected = browser.picked().holds(item);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::click_and_drag());
    let state = State::of(
        view.device.state.focused(class) == Some(at),
        view.queue.waiting(class, at).is_some(),
        info.is_some(),
    );

    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    // ⚠️ The loaded row uses the selection fill, so every cell on it switches text color
    // too: the signal colors are not legible on that fill.
    let lit = selected || state == State::Loaded;
    let (ink, quiet) = row_ink(&painter, rect, lit, response.hovered(), &visuals);
    let content = rect.shrink2(egui::vec2(CELL_PAD, 0.0));

    let text = cells(view, at, info);
    let faces = [
        egui::TextFormat::simple(egui::FontId::monospace(MONO), quiet),
        match info.is_some() {
            true => egui::TextFormat::simple(egui::FontId::proportional(NAME), ink),
            false => egui::TextFormat {
                italics: true,
                ..egui::TextFormat::simple(egui::FontId::proportional(NAME), quiet)
            },
        },
        egui::TextFormat::simple(egui::FontId::monospace(MONO), quiet),
        egui::TextFormat::simple(egui::FontId::proportional(TEXT), quiet),
    ];
    for ((said, face), track) in text.iter().zip(faces).zip(tracks) {
        cut(
            &painter,
            content.left() + track.start,
            content.center().y,
            track.end - track.start,
            said,
            face,
        );
    }
    if let Some((glyph, tint)) = state.glyph(&visuals) {
        let track = &tracks[4];
        painted(
            ui,
            glyph,
            egui::Rect::from_center_size(
                egui::pos2(
                    content.left() + track.start + SMALL / 2.0,
                    content.center().y,
                ),
                egui::Vec2::splat(SMALL),
            ),
            cell_ink(lit, tint, &visuals),
        );
    }

    let response = response.on_hover_text(hint(view, at, info, state));
    gestures(ui, browser, view, at, info, &response, acts);
}

/// The text of a row's four text cells.
fn cells(view: &View, at: Location, info: Option<&ProgramInfo>) -> [String; 4] {
    let name = info.map(|info| info.name.trim()).unwrap_or_default();
    [
        shown(at),
        match name.is_empty() {
            true => "empty".to_string(),
            false => name.to_string(),
        },
        info.map(|info| room::measure(u64::from(info.body_len)))
            .unwrap_or_default(),
        fourth(view, at, info),
    ]
}

/// The fact only this folder's list carries.
fn fourth(view: &View, at: Location, info: Option<&ProgramInfo>) -> String {
    let Some(info) = info else {
        return String::new();
    };
    let unknown = || "—".to_string();
    match view.class {
        ObjectClass::SetList => plays(at, view.workspace).unwrap_or_else(unknown),
        ObjectClass::Sample => {
            played_by(info.name.trim(), view.workspace, view.device).unwrap_or_else(unknown)
        }
        ObjectClass::Piano => view
            .device
            .state
            .bank_name(view.class, at.bank + 1)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// The four programs a set list plays.
///
/// ⚠️ Only from a decoded body: a copy kept on this computer, or a slot open as a view.
/// A walk reports only a slot's name, length, and format, so a set list with no local
/// copy shows nothing.
fn plays(at: Location, workspace: &Workspace) -> Option<String> {
    let entity = workspace
        .entities()
        .iter()
        .find(|held| held.origin.slot() == Some((ObjectClass::SetList, at)))?;
    let nord_format::Entity::Song(nord_format::Song::Electro5(song)) = entity.entity.as_ref()?
    else {
        return None;
    };
    let played: Vec<String> = song
        .programs()
        .iter()
        .map(|held| {
            let (bank, slot) = held.inner();
            format!("{}:{}", bank + 1, slot + 1)
        })
        .collect();
    Some(played.join(" · "))
}

/// The programs on this computer that play this sample.
///
/// ⚠️ A program's file stores a bare library id and a walk reports a bare name, so they
/// can be matched only through the dependency list the instrument gave for that
/// program's slot. An unmatched sample shows as unknown, not as unused.
fn played_by(name: &str, workspace: &Workspace, device: &Device) -> Option<String> {
    let played: Vec<&str> = workspace
        .entities()
        .iter()
        .filter(|entity| {
            matches!(
                crate::library::wanted(entity, &device.state),
                Needs::Named { class, name: held } if class == ObjectClass::Sample && held == name
            )
        })
        .map(|entity| entity.name.as_str())
        .collect();
    (!played.is_empty()).then(|| played.join(", "))
}

/// A library partition's meter: how full it is, and how much space is left.
fn meter(ui: &mut egui::Ui, class: ObjectClass, device: &Device, held: room::Meter) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let unit = device.state.allocation_unit(class);
        if let Some(room) = occupancy(class, &device.state.inventory, unit) {
            ui.label(egui::RichText::new(room).monospace().size(MONO));
        }
        if let Some(free) = room::free_space(class, &device.state) {
            ui.label(egui::RichText::new(free).text_style(ui_text()).weak());
        }
    });
    room::bar(ui, held);
}

/// A box of its own under a list, padded like the bands above it.
fn boxed<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .corner_radius(CARD_RADIUS)
        .inner_margin(12)
        .outer_margin(egui::Margin {
            left: VIEW_PAD as i8,
            right: VIEW_PAD as i8,
            top: 12,
            bottom: 14,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            contents(ui)
        })
        .inner
}

/// The first queued entry for any slot in a folder.
fn waiting_in(queue: &Queue, class: ObjectClass) -> Option<&Queued> {
    queue.entries().iter().find(|held| held.class == class)
}

/// The full description of a slot, shown on hover.
fn hint(view: &View, at: Location, info: Option<&ProgramInfo>, state: State) -> String {
    let where_ = place(view.class, at);
    let mut said = match info {
        Some(info) => format!("“{}” in {where_}", info.name.trim()),
        None => format!("{where_} is empty"),
    };
    match state {
        State::Loaded => said.push_str(", loaded on the instrument now"),
        State::Incoming => said.push_str("; a write to this slot is queued"),
        State::Empty | State::Held => {}
    }
    let fact = fourth(view, at, info);
    if !fact.is_empty() {
        said.push_str(&format!("\n{}: {fact}", column(view.class)));
    }
    said
}

/// The gestures every slot handles, as a cell or a row: drag, drop, select, open, and the
/// slot's context menu.
fn gestures(
    ui: &mut egui::Ui,
    browser: &mut Browser,
    view: &View,
    at: Location,
    info: Option<&ProgramInfo>,
    response: &egui::Response,
    acts: &mut Vec<Act>,
) {
    let class = view.class;
    let item = Item::Slot { class, at };
    // ⚠️ A partition this app cannot name is only listed.
    let fetchable = !read_only(class);
    let name = info
        .map(|info| info.name.trim())
        .filter(|name| !name.is_empty());

    if let Some(name) = name {
        if fetchable && response.dragged() {
            let head = Held {
                what: item,
                kind: Kind::from_class(class),
                filed: None,
                fits: true,
            };
            let carried = browser.carrying(head, name, view.workspace, &view.device.state);
            egui::DragAndDrop::set_payload(ui.ctx(), carried);
        }
    }
    browser.drop_zone(ui, response, Onto::Slot { class, at }, acts);

    if response.double_clicked() {
        if name.is_some() && fetchable {
            acts.push(Act::Open(item));
        }
    } else if response.clicked() {
        browser.pick(ui, item, view.list);
    }
    if name.is_some() {
        response.clone().context_menu(|ui| {
            browser.menu(ui, item, view.workspace, view.device, view.queue, acts)
        });
    }
}

/// A band under the header: padded at each side like the header, laid out left to
/// right, and wrapping when the center is narrow.
fn band<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: VIEW_PAD as i8,
            right: VIEW_PAD as i8,
            top: 0,
            bottom: UNDER as i8,
        })
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 4.0);
                contents(ui)
            })
            .inner
        })
        .inner
}

/// The line a folder shows when there is nothing to draw.
fn nothing(ui: &mut egui::Ui, said: &str) {
    ui.add_space(VIEW_PAD);
    ui.horizontal(|ui| {
        ui.add_space(VIEW_PAD);
        ui.label(
            egui::RichText::new(said)
                .text_style(ui_text())
                .weak()
                .italics(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::enqueue;
    use crate::strings::folder;
    use crate::tabs::Spot;
    use crate::testing::{self, Bench};
    use crate::workspace::{Fresh, Origin};
    use nord_usb::wire::Status;

    /// An instrument with every folder the tab can switch to, and local copies of a set
    /// list and a settings object.
    fn bench() -> Bench {
        let mut bench = Bench::new();
        let Bench {
            workspace,
            device,
            queue,
            log,
            ..
        } = &mut bench;

        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", "Squabble B"]);
        device.pretend_scanned(ObjectClass::Program, 8, &["Bass Manual"]);
        device.pretend_geometry(ObjectClass::Program, &[("Bank 7", 50), ("Bank 8", 50)]);
        device.pretend_focused(ObjectClass::Program, Location { bank: 6, slot: 2 });
        device.pretend_scanned(ObjectClass::Live, 1, &["Live"]);
        device.pretend_scanned(ObjectClass::SetList, 1, &["Sunday", ""]);
        device.pretend_scanned(ObjectClass::Sample, 1, &["Rhodes MkI"]);
        device.pretend_scanned(ObjectClass::Piano, 1, &["Royal Grand 3D"]);
        device.pretend_geometry(ObjectClass::Piano, &[("Grand", 1)]);
        device.pretend_scanned(ObjectClass::Settings, 1, &["Settings"]);
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.state.inventory.push(Status {
            class: ObjectClass::Sample,
            count: 84,
            free: 60,
            used: 1472,
            dirty: 0,
            spare: 4,
        });
        device.state.scan.heard(ObjectClass::Program, 0.0);

        // A set list read from an instrument slot, so its body says what it plays, and a
        // queued settings write, so the folder has a diff to draw.
        for (kind, class, slot) in [
            (Fresh::SetList, ObjectClass::SetList, 0),
            (Fresh::Settings, ObjectClass::Settings, 0),
        ] {
            let at = Location { bank: 0, slot };
            let id = workspace.ingest(
                format!("{}.file", crate::strings::place(class, at)),
                Origin::Device { class, at },
                kind.bytes().unwrap(),
                log,
            );
            if class == ObjectClass::Settings {
                enqueue(workspace, device, queue, log, id, class, at);
            }
        }
        bench
    }

    /// Draw the tab for the shown class and run whatever it asked for.
    fn draw(
        width: f32,
        events: Vec<egui::Event>,
        keyboard: &mut Keyboard,
        bench: &mut Bench,
    ) -> egui::FullOutput {
        let ctx = bench.ctx.clone();
        let input = testing::screen(egui::vec2(width, 540.0), events);
        testing::run(&ctx, input, |ctx| {
            // The frame the center uses: panels handle their own padding.
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    let acts = keyboard.ui(
                        ui,
                        &mut bench.browser,
                        &bench.workspace,
                        &bench.device,
                        &bench.queue,
                        &bench.tabs,
                    );
                    bench.act(acts);
                });
        })
    }

    /// Each folder is drawn at the center's width with both docks open and with none.
    ///
    /// No pixels are checked. This catches a layout that panics, an id that collides, or
    /// a row that paints past its track.
    #[test]
    fn every_folder_paints_its_own_layout_at_every_width_the_center_has() {
        let mut bench = bench();
        let mut keyboard = Keyboard::default();
        bench.tabs.show(Spot::Keyboard);

        for class in bench.device.state.classes() {
            bench.tabs.keyboard_on(class);
            for width in [430.0_f32, 900.0] {
                // Twice: the second pass runs with the widget state the first left.
                for _ in 0..2 {
                    draw(width, Vec::new(), &mut keyboard, &mut bench);
                }
            }
            assert_eq!(
                bench.tabs.keyboard_class(),
                Some(class),
                "{} stayed shown",
                folder(class)
            );
        }
    }

    /// ⚠️ Every folder here has already been read. Re-reading one on every click would
    /// make the player wait on the instrument to see a folder.
    #[test]
    fn the_switcher_changes_the_folder_without_asking_the_instrument_for_anything() {
        let mut bench = bench();
        let mut keyboard = Keyboard::default();
        bench.tabs.show(Spot::Keyboard);
        bench.tabs.keyboard_on(ObjectClass::SetList);
        // Requests already made by the queued settings write, so any later ones came
        // from the click.
        let asked = bench.device.queued().len();

        // The switcher's first segment is the first folder the instrument declares.
        let first = bench.device.state.classes()[0];
        let name = bench.device.state.folder_name(first).to_string();
        let drawn = draw(900.0, Vec::new(), &mut keyboard, &mut bench);
        let on_first = testing::where_(&testing::painted(&drawn), &name).center();
        let frames: [Vec<egui::Event>; 3] = [
            vec![egui::Event::PointerMoved(on_first)],
            vec![
                testing::button(on_first, true),
                testing::button(on_first, false),
            ],
            Vec::new(),
        ];
        for events in frames {
            draw(900.0, events, &mut keyboard, &mut bench);
        }

        assert_eq!(bench.tabs.keyboard_class(), Some(first));
        assert_eq!(bench.tabs.showing(), Spot::Keyboard);
        assert_eq!(
            bench.device.queued().len(),
            asked,
            "the click asked the instrument for nothing"
        );
    }

    /// A click on a bank's square shows that bank's slots in the map.
    #[test]
    fn a_click_on_a_bank_square_shows_that_bank() {
        let mut bench = bench();
        let mut keyboard = Keyboard::default();
        bench.tabs.show(Spot::Keyboard);
        bench.tabs.keyboard_on(ObjectClass::Program);

        let said = testing::painted(&draw(900.0, Vec::new(), &mut keyboard, &mut bench));
        assert!(
            said.iter().any(|word| word.text == "Africa Split"),
            "{said:?}"
        );
        let square = testing::where_(&said, "8").center();
        draw(900.0, testing::click(square), &mut keyboard, &mut bench);
        let said = testing::words(&draw(900.0, Vec::new(), &mut keyboard, &mut bench));
        assert!(said.contains(&"Bass Manual".to_string()), "{said:?}");
        assert!(!said.contains(&"Africa Split".to_string()), "{said:?}");
    }

    /// The header's button asks for the shown folder again.
    #[test]
    fn read_again_asks_for_the_shown_folder() {
        let mut bench = bench();
        let mut keyboard = Keyboard::default();
        bench.tabs.show(Spot::Keyboard);
        bench.tabs.keyboard_on(ObjectClass::SetList);

        let said = testing::painted(&draw(900.0, Vec::new(), &mut keyboard, &mut bench));
        let button = testing::where_(&said, "Read again").center();
        let input = testing::screen(egui::vec2(900.0, 540.0), testing::click(button));
        let mut asked = Vec::new();
        testing::run(&bench.ctx.clone(), input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    asked = keyboard.ui(
                        ui,
                        &mut bench.browser,
                        &bench.workspace,
                        &bench.device,
                        &bench.queue,
                        &bench.tabs,
                    );
                });
        });
        assert!(
            matches!(asked.as_slice(), [Act::ReadAgain(ObjectClass::SetList)]),
            "{asked:?}"
        );
    }

    /// Every card in the map is at least 150 px wide, unless the center is too narrow for
    /// even one, and a wider center fits more of them.
    #[test]
    fn the_map_fits_as_many_cards_as_the_width_holds_at_their_minimum() {
        for width in [100.0_f32, 330.0, 430.0, 900.0, 1600.0] {
            let fit = columns(width);
            let room = width - 2.0 * MAP.margin - MAP.gap * (fit - 1) as f32;
            assert!(
                fit == 1 || room / fit as f32 >= CARD_MIN,
                "{fit} cards at {width}"
            );
            let one_more = width - 2.0 * MAP.margin - MAP.gap * fit as f32;
            assert!(
                one_more / ((fit + 1) as f32) < CARD_MIN,
                "one more card would fit at {width}"
            );
        }
        assert_eq!(columns(100.0), 1);
        assert!(columns(900.0) > columns(430.0));
    }

    /// A slot's address and the word "empty" fit whole on a map card and on a picker's
    /// smaller cell, beside the glyph of a queued write.
    #[test]
    fn a_card_writes_its_address_and_empty_in_full_at_the_map_and_picker_sizes() {
        let ctx = testing::context();
        let at = Location { bank: 6, slot: 49 };
        for size in [egui::vec2(CARD_MIN, MAP.height), egui::Vec2::splat(CELL)] {
            let said = testing::painted(&testing::run(
                &ctx,
                testing::screen(egui::vec2(400.0, 200.0), Vec::new()),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), size);
                        paint_cell(ui, rect, at, None, State::Incoming, false, false);
                    });
                },
            ));
            for word in ["7:50", "empty"] {
                assert!(
                    said.iter()
                        .any(|painted| painted.text == word && !painted.galley.elided),
                    "{word} at {size:?}: {said:?}"
                );
            }
        }
    }

    #[test]
    fn the_bank_sentence_counts_what_is_there_and_what_is_coming() {
        assert_eq!(sentence(12, 50, 0), "12 of 50 slots hold something");
        assert_eq!(
            sentence(12, 50, 4),
            "12 of 50 slots hold something · 4 incoming"
        );
        assert_eq!(sentence(0, 50, 0), "0 of 50 slots hold something");
    }

    /// Seconds up to a minute, then minutes, and never negative if the clock steps back
    /// between frames.
    #[test]
    fn the_header_says_how_long_ago_the_folder_answered() {
        assert_eq!(ago(0.0), "read 0 s ago");
        assert_eq!(ago(59.4), "read 59 s ago");
        assert_eq!(ago(60.0), "read 1 min ago");
        assert_eq!(ago(119.0), "read 1 min ago");
        assert_eq!(ago(3600.0), "read 60 min ago");
        assert_eq!(ago(-2.0), "read 0 s ago");
    }
}
