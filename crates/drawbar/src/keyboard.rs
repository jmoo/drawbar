//! The keyboard tab: one instrument, with the folder switched inside it.
//!
//! Two shapes, because the instrument has two. A folder divided into banks of equal
//! slots is a map of them; a folder whose items differ in size and in what they say is a
//! list. Both draw the same slot, so a cell and a row answer the same gestures and offer
//! the same menu.

use std::collections::BTreeMap;
use std::ops::Range;

use eframe::egui;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, ui as ui_text, warn};
use crate::browser::{cell_ink, Act, Browser, Held, Item, Kind, Onto};
use crate::device::{occupancy, read_only, Device};
use crate::icon::{icon, painted, Glyph};
use crate::library::Needs;
use crate::panel::{caps, Track};
use crate::queue::{Queue, Queued};
use crate::room;
use crate::strings::{place, shown};
use crate::tabs::Tabs;
use crate::workspace::Workspace;

/// The bands over whatever the folder is drawn as.
const HEADER: f32 = 28.0;
const SWITCHER: f32 = 26.0;
const BANKS: f32 = 24.0;

/// A list's column heads, and a row under them.
const HEAD: f32 = 20.0;
const ROW: f32 = 24.0;

/// A slot in the map, and the grid they are laid out in.
const CELL: f32 = 42.0;
const CELL_GAP: f32 = 4.0;
const COLUMNS: usize = 5;

/// The room a band keeps at each end, and the gap between its parts.
const PAD: f32 = 8.0;
const GAP: f32 = 6.0;

/// A chip's own height and the room it keeps at each end.
const CHIP: f32 = 19.0;
const CHIP_PAD: f32 = 6.0;

/// The room a cell keeps inside its own border, and the gap between a chip's parts.
const INSET: f32 = 5.0;

/// A glyph in a band, and the smaller one at the end of a row.
const GLYPH: f32 = 13.0;
const SMALL: f32 = 11.0;

/// The faces this view paints in. Painted rather than laid out, so the sizes are here
/// rather than resolved from the named styles in [`crate::app`].
const NAME: f32 = 12.0;
const CELL_NAME: f32 = 11.0;
const MONO: f32 = 10.5;
const ADDRESS: f32 = 9.5;

/// The gap between two columns of a list.
const LIST_GAP: f32 = 10.0;

/// The list geometry every folder drawn as a list shares: an address, a name, a size,
/// the one fact that folder carries, and the state at the end.
const LIST: [Track; 5] = [
    Track::Px(58.0),
    Track::Share(1.5),
    Track::Px(74.0),
    Track::Share(1.0),
    Track::Px(20.0),
];

/// What the Pianos folder is, since nothing in it can be changed from here.
const PIANOS: &str = "Pianos are installed by Nord Sound Manager. They are listed here so \
                      a program can name what it plays.";

/// The centre's view of the attached instrument.
#[derive(Default)]
pub struct Keyboard {
    /// The bank each folder is showing, by the raw class number. A folder keeps the bank
    /// it was left on while the switcher is somewhere else.
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
        // The bands and the body are flush: a folder's own lines are the only rules.
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

    /// The bank a folder is showing: what was picked, while the folder still has it, and
    /// otherwise the first bank read.
    fn bank(&self, class: ObjectClass, banks: &[u32]) -> Option<u32> {
        self.banks
            .get(&class.to_raw())
            .copied()
            .filter(|held| banks.contains(held))
            .or_else(|| banks.first().copied())
    }

    /// A bank of equal slots, five to a row.
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
                grid(ui, slots.len(), |ui, index, rect| {
                    let at = Location::from_user(bank, index as u32 + 1);
                    cell(ui, browser, &view, rect, at, slots[index].as_ref(), acts);
                });
            });
    }
}

/// The five-column grid of 42 px cells a bank of slots is laid out in, whoever is
/// drawing them. `each` is handed one slot's index and the rect it sits in.
pub fn grid(
    ui: &mut egui::Ui,
    slots: usize,
    mut each: impl FnMut(&mut egui::Ui, usize, egui::Rect),
) {
    ui.add_space(CELL_GAP);
    for row in 0..slots.div_ceil(COLUMNS) {
        let (strip, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), CELL + CELL_GAP),
            egui::Sense::hover(),
        );
        let room = strip.width() - 2.0 * PAD - CELL_GAP * (COLUMNS - 1) as f32;
        let width = (room / COLUMNS as f32).max(0.0);
        for column in 0..COLUMNS {
            let index = row * COLUMNS + column;
            if index >= slots {
                break;
            }
            let rect = egui::Rect::from_min_size(
                egui::pos2(
                    strip.left() + PAD + (width + CELL_GAP) * column as f32,
                    strip.top(),
                ),
                egui::vec2(width, CELL),
            );
            each(ui, index, rect);
        }
    }
}

/// The width a five-column grid wants, for a caller laying out room for one.
pub fn grid_width() -> f32 {
    2.0 * PAD + CELL * COLUMNS as f32 + CELL_GAP * (COLUMNS - 1) as f32
}

// ---- the bands over every folder ---------------------------------------------------

/// 28 px: what is attached, how stale what is on screen is, and the way to refresh it.
fn header(ui: &mut egui::Ui, device: &Device, class: ObjectClass, acts: &mut Vec<Act>) {
    let now = ui.input(|input| input.time);
    let said = freshness(device, class, now);
    let product = device.state.product().unwrap_or_default().to_string();
    band(ui, HEADER, None, |ui| {
        let ink = ui.visuals().widgets.inactive.fg_stroke.color;
        icon(ui, Glyph::Keyboard, GLYPH, ink);
        ui.label(egui::RichText::new(product).size(12.5).strong().color(ink));
        if !said.is_empty() {
            ui.label(egui::RichText::new(said).size(10.5).weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if chip(
                ui,
                Some(Glyph::RefreshCw),
                ("Read again", egui::FontId::proportional(11.0)),
                None,
                false,
            )
            .on_hover_text(format!("read {} again", device.state.folder_name(class)))
            .clicked()
            {
                acts.push(Act::ReadAgain(class));
            }
        });
    });
}

/// The firmware, and how long ago the folder on show last answered.
fn freshness(device: &Device, class: ObjectClass, now: f64) -> String {
    let mut said: Vec<String> = device.state.firmware().into_iter().collect();
    if let Some(at) = device.state.scan.read_at(class) {
        said.push(ago(now - at));
    }
    said.join(" · ")
}

/// How long ago something was read, in the width the header has for it.
fn ago(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    match seconds < 60.0 {
        true => format!("read {} s ago", seconds as u64),
        false => format!("read {} min ago", (seconds / 60.0) as u64),
    }
}

/// 26 px: one chip per folder, the one on show wearing the selection.
///
/// Switching is a switch and nothing more — every folder here has already been read, and
/// asking the instrument again is what the header's own chip is for.
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
    let faint = ui.visuals().faint_bg_color;
    band(ui, SWITCHER, Some(faint), |ui| {
        for (class, room) in classes.iter().zip(&rooms) {
            let picked = chip(
                ui,
                Some(Kind::from_class(*class).glyph()),
                (
                    device.state.folder_name(*class),
                    egui::FontId::proportional(11.0),
                ),
                room.as_deref(),
                *class == on,
            )
            .clicked();
            if picked {
                acts.push(Act::ShowClass(*class));
            }
        }
    });
}

/// 24 px: the banks a folder divides into, and what the one on show holds. The bank the
/// click asked for, if it asked for one.
fn bank_row(ui: &mut egui::Ui, on: u32, banks: &[u32], said: &str) -> Option<u32> {
    let names: Vec<String> = banks.iter().map(u32::to_string).collect();
    let mut picked = None;
    band(ui, BANKS, None, |ui| {
        let ink = crate::app::caption(ui.visuals());
        ui.label(caps("bank").color(ink));
        for (bank, name) in banks.iter().zip(&names) {
            if chip(
                ui,
                None,
                (name, egui::FontId::monospace(MONO)),
                None,
                *bank == on,
            )
            .clicked()
            {
                picked = Some(*bank);
            }
        }
        ui.label(egui::RichText::new(said).text_style(ui_text()).weak());
    });
    picked
}

/// What a bank holds and what is on its way to it.
pub fn sentence(held: usize, slots: usize, incoming: usize) -> String {
    let said = format!("{held} of {slots} slots hold something");
    match incoming {
        0 => said,
        n => format!("{said} · {n} incoming"),
    }
}

// ---- the map ------------------------------------------------------------------------

/// What a slot in the map is: what it holds, and what is about to happen to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Empty,
    Held,
    Incoming,
    Loaded,
}

impl State {
    /// ⚠️ The panel wins. A slot the instrument is playing is the one thing on the map a
    /// player has to be able to find, whatever else is true of it.
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

    /// The border, and whether it is drawn as a dashed one.
    fn edge(self, visuals: &egui::Visuals) -> (egui::Color32, bool) {
        match self {
            State::Empty => (visuals.widgets.noninteractive.bg_stroke.color, true),
            State::Held => (visuals.widgets.noninteractive.bg_stroke.color, false),
            State::Incoming => (warn(visuals), false),
            State::Loaded => (accent(visuals), false),
        }
    }

    /// The glyph at the top right, where the slot has anything to say.
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
/// whichever word happens to be under it.
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

/// A slot painted as a cell: the ground its state and its selection earn, the border,
/// the address, and what it holds. Whatever senses it is the caller's.
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
    let ground = match (state, selected, hovered) {
        (_, true, _) | (State::Loaded, _, _) => Some(visuals.selection.bg_fill),
        (_, false, true) => Some(visuals.faint_bg_color),
        (State::Empty, _, _) => None,
        _ => Some(visuals.window_fill),
    };
    if let Some(fill) = ground {
        painter.rect_filled(rect, 3.0, fill);
    }
    let (edge, dashed) = state.edge(&visuals);
    let stroke = egui::Stroke::new(1.0_f32, cell_ink(selected, edge, &visuals));
    match dashed {
        true => dashed_rect(&painter, rect, stroke),
        false => {
            painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
        }
    }

    let lit = selected || state == State::Loaded;
    let ink = cell_ink(lit, visuals.text_color(), &visuals);
    let quiet = cell_ink(lit, visuals.weak_text_color(), &visuals);
    let room = (rect.width() - 2.0 * INSET).max(0.0);
    cut(
        &painter,
        rect.left() + INSET,
        rect.top() + 4.0 + SMALL / 2.0,
        room - SMALL,
        &shown(at),
        egui::TextFormat::simple(egui::FontId::monospace(ADDRESS), quiet),
    );
    if let Some((glyph, tint)) = state.glyph(&visuals) {
        painted(
            ui,
            glyph,
            egui::Rect::from_min_size(
                egui::pos2(rect.right() - INSET - SMALL, rect.top() + 4.0),
                egui::Vec2::splat(SMALL),
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
                ..egui::TextFormat::simple(egui::FontId::proportional(CELL_NAME), quiet)
            },
        ),
        false => (
            name,
            egui::TextFormat::simple(egui::FontId::proportional(CELL_NAME), ink),
        ),
    };
    cut(
        &painter,
        rect.left() + INSET,
        rect.bottom() - INSET - CELL_NAME / 2.0,
        room,
        name,
        format,
    );
}

/// The four sides of a dashed border. egui draws dashes along a line, so a rectangle is
/// four of them.
fn dashed_rect(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    const DASH: f32 = 3.0;
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

// ---- the lists ----------------------------------------------------------------------

/// A folder whose items differ in size and in what they say.
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
    if class == ObjectClass::Piano {
        prose(ui, PIANOS);
    }
    if class == ObjectClass::Sample {
        egui::TopBottomPanel::bottom("keyboard_room")
            .resizable(false)
            .frame(egui::Frame::new())
            .show_inside(ui, |ui| footer(ui, class, device, queue, workspace));
    }
    // Settings is one live object rather than a folder of them, so what a queued write
    // would change to it is shown beside it rather than only in the dock.
    if class == ObjectClass::Settings {
        if let Some(held) = waiting_in(queue, class) {
            egui::TopBottomPanel::bottom("keyboard_settings")
                .resizable(false)
                .frame(egui::Frame::new())
                .show_inside(ui, |ui| crate::queue::table(ui, held));
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

    let body = ui.available_height() - HEAD;
    let scrolls = slots.len() as f32 * ROW > body;
    let bar = match scrolls {
        true => ui.spacing().scroll.bar_width,
        false => 0.0,
    };
    let width = (ui.available_width() - bar).max(0.0);
    let tracks = crate::panel::tracks(width, &LIST, LIST_GAP);
    head(ui, class, width, &tracks);

    let view = View {
        class,
        list: &items,
        workspace,
        device,
        queue,
    };
    egui::ScrollArea::vertical()
        .id_salt("keyboard_list")
        .auto_shrink([false; 2])
        .show_rows(ui, ROW, slots.len(), |ui, rows| {
            for (at, info) in rows.filter_map(|index| slots.get(index)) {
                row(ui, browser, &view, width, &tracks, *at, *info, acts);
            }
        });
}

/// The word over a folder's fourth column — the one fact its list carries that no other
/// folder's does.
fn column(class: ObjectClass) -> &'static str {
    match class {
        ObjectClass::SetList => "plays",
        ObjectClass::Sample => "played by",
        ObjectClass::Piano => "category",
        _ => "",
    }
}

/// 20 px of column heads over a list.
fn head(ui: &mut egui::Ui, class: ObjectClass, width: f32, tracks: &[Range<f32>]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, HEAD), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 0.0, visuals.faint_bg_color);
    let ink = crate::app::caption(&visuals);
    for (head, track) in ["at", "name", "size", column(class), ""].iter().zip(tracks) {
        cut(
            &painter,
            rect.left() + track.start,
            rect.center().y,
            track.end - track.start,
            &head.to_uppercase(),
            egui::TextFormat::simple(egui::FontId::proportional(ADDRESS), ink),
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
    let fill = match (selected, state == State::Loaded, response.hovered()) {
        (true, _, _) | (_, true, _) => Some(visuals.selection.bg_fill),
        (false, false, true) => Some(visuals.faint_bg_color),
        (false, false, false) => None,
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, 3.0, fill);
    }
    // ⚠️ The loaded row carries the selection's fill, so every cell on it switches ink
    // with it — the signal colours do not carry on that ground.
    let lit = selected || state == State::Loaded;
    let ink = cell_ink(lit, visuals.text_color(), &visuals);
    let quiet = cell_ink(lit, visuals.weak_text_color(), &visuals);

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
        egui::TextFormat::simple(egui::FontId::proportional(NAME - 1.0), quiet),
    ];
    for ((said, face), track) in text.iter().zip(faces).zip(tracks) {
        cut(
            &painter,
            rect.left() + track.start,
            rect.center().y,
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
                egui::pos2(rect.left() + track.start + SMALL / 2.0, rect.center().y),
                egui::Vec2::splat(SMALL),
            ),
            cell_ink(lit, tint, &visuals),
        );
    }

    let response = response.on_hover_text(hint(view, at, info, state));
    gestures(ui, browser, view, at, info, &response, acts);
}

/// What each of a row's four written cells says.
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

/// The one fact a folder's list carries that the others do not.
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
/// ⚠️ Only out of a decoded body — a copy kept on this computer, or a slot open as a
/// view. A walk reports a slot's name, length and format and nothing about what is
/// inside it, so a set list nothing here holds says nothing.
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
/// ⚠️ A program's file stores a bare library id and a walk reports a bare name, so the
/// two meet only through a dependency list the instrument answered for that program's own
/// slot. Nothing else this app reads can match them, and an unmatched sample says so
/// rather than claiming nothing plays it.
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

/// The samples footer: how full the partition is, and what is left in it.
fn footer(
    ui: &mut egui::Ui,
    class: ObjectClass,
    device: &Device,
    queue: &Queue,
    workspace: &Workspace,
) {
    let unit = device.state.allocation_unit(class);
    let Some(held) = room::meter(class, &device.state.inventory, unit, queue, workspace) else {
        return;
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                if let Some(room) = occupancy(class, &device.state.inventory, unit) {
                    ui.label(egui::RichText::new(room).monospace().size(MONO));
                }
                if let Some(free) = room::free_space(class, &device.state.inventory, unit) {
                    ui.label(egui::RichText::new(free).text_style(ui_text()).weak());
                }
            });
            room::bar(ui, held);
        });
}

// ---- what every slot answers to -----------------------------------------------------

/// The first entry waiting for anywhere in a folder.
fn waiting_in(queue: &Queue, class: ObjectClass) -> Option<&Queued> {
    queue.entries().iter().find(|held| held.class == class)
}

/// The whole of what a slot is, which is what a hover asks for.
fn hint(view: &View, at: Location, info: Option<&ProgramInfo>, state: State) -> String {
    let where_ = place(view.class, at);
    let mut said = match info {
        Some(info) => format!("“{}” in {where_}", info.name.trim()),
        None => format!("{where_} is empty"),
    };
    match state {
        State::Loaded => said.push_str(" — on the instrument's panel now"),
        State::Incoming => said.push_str(" — something is waiting to be written here"),
        State::Empty | State::Held => {}
    }
    let fact = fourth(view, at, info);
    if !fact.is_empty() {
        said.push_str(&format!("\n{}: {fact}", column(view.class)));
    }
    said
}

/// Everything a slot answers to, wherever it is drawn: it is dragged, it takes a drop,
/// it is picked, it opens, and it offers the slot's own menu.
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
    // ⚠️ Pianos are large libraries fetched whole, so this view only lists them.
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
            };
            let carried = browser.carrying(head, name, view.workspace);
            egui::DragAndDrop::set_payload(ui.ctx(), carried);
        }
    }
    browser.drop_zone(ui, response, Onto::Slot { class, at }, acts);

    if response.double_clicked() {
        if name.is_some() && fetchable {
            acts.push(Act::Open(item));
        }
    } else if response.clicked() {
        browser.pick(ui, item, name.unwrap_or_default(), response, view.list);
    }
    if name.is_some() {
        response
            .clone()
            .context_menu(|ui| browser.menu(ui, item, view.workspace, view.device, acts));
    }
}

// ---- the shared pieces --------------------------------------------------------------

/// A full-width band: filled, padded at each end, laid out left to right.
fn band<R>(
    ui: &mut egui::Ui,
    height: f32,
    fill: Option<egui::Color32>,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inner.spacing_mut().item_spacing.x = GAP;
    contents(&mut inner)
}

/// One chip: an optional glyph, a word, and an optional mono readout after it.
fn chip(
    ui: &mut egui::Ui,
    glyph: Option<Glyph>,
    text: (&str, egui::FontId),
    readout: Option<&str>,
    active: bool,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    let ink = match active {
        true => visuals.selection.stroke.color,
        false => visuals.widgets.inactive.fg_stroke.color,
    };
    let quiet = cell_ink(active, visuals.weak_text_color(), &visuals);
    let painter = ui.painter().clone();
    let word = painter.layout_no_wrap(text.0.to_string(), text.1, ink);
    let count = readout
        .map(|held| painter.layout_no_wrap(held.to_string(), egui::FontId::monospace(MONO), quiet));

    let marked = glyph.map_or(0.0, |_| GLYPH + INSET);
    let counted = count.as_ref().map_or(0.0, |held| INSET + held.size().x);
    let width = CHIP_PAD + marked + word.size().x + counted + CHIP_PAD;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, CHIP), egui::Sense::click());

    let fill = match (active, response.hovered()) {
        (true, _) => Some(visuals.selection.bg_fill),
        (false, true) => Some(visuals.window_fill),
        (false, false) => None,
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, 2.0, fill);
    }
    if !active {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    let mut x = rect.left() + CHIP_PAD;
    if let Some(glyph) = glyph {
        painted(
            ui,
            glyph,
            egui::Rect::from_center_size(
                egui::pos2(x + GLYPH / 2.0, rect.center().y),
                egui::Vec2::splat(GLYPH),
            ),
            ink,
        );
        x += GLYPH + INSET;
    }
    painter.galley(
        egui::pos2(x, rect.center().y - word.size().y / 2.0),
        word.clone(),
        egui::Color32::PLACEHOLDER,
    );
    if let Some(count) = count {
        painter.galley(
            egui::pos2(
                x + word.size().x + INSET,
                rect.center().y - count.size().y / 2.0,
            ),
            count,
            egui::Color32::PLACEHOLDER,
        );
    }
    response
}

/// One cell of text, cut to `width` with an ellipsis and centred on `middle`.
fn cut(
    painter: &egui::Painter,
    left: f32,
    middle: f32,
    width: f32,
    text: &str,
    format: egui::TextFormat,
) {
    if width <= 0.0 || text.is_empty() {
        return;
    }
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, format);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(left, middle - galley.size().y / 2.0),
        galley,
        egui::Color32::PLACEHOLDER,
    );
}

/// The line a folder shows where there is nothing to draw.
fn nothing(ui: &mut egui::Ui, said: &str) {
    ui.add_space(GAP);
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.label(
            egui::RichText::new(said)
                .text_style(ui_text())
                .weak()
                .italics(),
        );
    });
}

/// A sentence about the folder itself, over the list it is about.
fn prose(ui: &mut egui::Ui, said: &str) {
    ui.add_space(GAP);
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.label(egui::RichText::new(said).text_style(ui_text()).weak());
    });
    ui.add_space(GAP);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::apply;
    use crate::log::Log;
    use crate::queue::enqueue;
    use crate::shell::Shell;
    use crate::strings::folder;
    use crate::tabs::{Spot, Tabs};
    use crate::workspace::{Fresh, Origin};
    use nord_usb::wire::Status;

    /// A context dressed the way `DrawbarApp::new` dresses one: the named text styles a
    /// band resolves are installed there, on both faces.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        ctx
    }

    /// An instrument answering for every folder the tab can be switched to, and a copy
    /// of a set list, a settings object and a sample on this computer.
    #[allow(clippy::type_complexity)]
    fn bench() -> (Keyboard, Browser, Workspace, Device, Tabs, Queue, Log) {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let mut queue = Queue::default();

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

        // A set list off the slot the instrument holds, so its own body says what it
        // plays, and a settings write waiting, so the folder has a diff to draw.
        for (kind, class, slot) in [
            (Fresh::SetList, ObjectClass::SetList, 0),
            (Fresh::Settings, ObjectClass::Settings, 0),
        ] {
            let made = workspace.create(kind, &mut log).unwrap();
            let bytes = workspace.get(made).unwrap().bytes.clone();
            workspace.remove(made, &mut log);
            let at = Location { bank: 0, slot };
            let id = workspace.ingest(
                format!("{}.file", crate::strings::place(class, at)),
                Origin::Device { class, at },
                bytes,
                &mut log,
            );
            if class == ObjectClass::Settings {
                enqueue(&workspace, &mut device, &mut queue, &mut log, id, class, at);
            }
        }

        (
            Keyboard::default(),
            Browser::default(),
            workspace,
            device,
            Tabs::default(),
            queue,
            log,
        )
    }

    /// Draw the tab for `class` and run whatever it asked for, `frames` times.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        ctx: &egui::Context,
        width: f32,
        events: Vec<egui::Event>,
        keyboard: &mut Keyboard,
        browser: &mut Browser,
        workspace: &mut Workspace,
        device: &mut Device,
        tabs: &mut Tabs,
        queue: &mut Queue,
        log: &mut Log,
    ) {
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 540.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            // The frame the centre actually uses: panels own their own padding.
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    let acts = keyboard.ui(ui, browser, workspace, device, queue, tabs);
                    apply(
                        browser,
                        &mut Shell::default(),
                        acts,
                        workspace,
                        device,
                        tabs,
                        queue,
                        log,
                    );
                });
        });
    }

    /// Every folder draws its own layout, at the width the centre has with both docks
    /// open and at the width it has with none.
    ///
    /// Nothing checks pixels. What this catches is a layout that panics, an id that
    /// collides, or a track a row paints past.
    #[test]
    fn every_folder_paints_its_own_layout_at_every_width_the_centre_has() {
        let ctx = context();
        let (mut keyboard, mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) =
            bench();
        tabs.show(Spot::Keyboard);

        for class in device.state.classes() {
            tabs.keyboard_on(class);
            for width in [430.0_f32, 900.0] {
                // Twice: the second pass runs with the widget state the first left.
                for _ in 0..2 {
                    draw(
                        &ctx,
                        width,
                        Vec::new(),
                        &mut keyboard,
                        &mut browser,
                        &mut workspace,
                        &mut device,
                        &mut tabs,
                        &mut queue,
                        &mut log,
                    );
                }
            }
            assert_eq!(
                tabs.keyboard_class(),
                Some(class),
                "{} stayed on show",
                folder(class)
            );
        }
    }

    /// ⚠️ Switching folders is a switch and nothing more. Every folder here has already
    /// been read, and re-reading one on every click would put a session between the
    /// player and the thing they are looking for.
    #[test]
    fn the_switcher_changes_the_folder_without_asking_the_instrument_for_anything() {
        let ctx = context();
        let (mut keyboard, mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) =
            bench();
        tabs.show(Spot::Keyboard);
        tabs.keyboard_on(ObjectClass::SetList);
        // What the queued settings write already asked for, so what is here afterwards
        // is what the click added.
        let asked = device.queued().len();

        // The first chip of the switcher is the first folder the instrument declares.
        let on_programs = egui::pos2(20.0, HEADER + SWITCHER / 2.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: on_programs,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let frames: [Vec<egui::Event>; 4] = [
            Vec::new(),
            vec![egui::Event::PointerMoved(on_programs)],
            vec![press(true), press(false)],
            Vec::new(),
        ];
        for events in frames {
            draw(
                &ctx,
                900.0,
                events,
                &mut keyboard,
                &mut browser,
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut queue,
                &mut log,
            );
        }

        assert_eq!(
            tabs.keyboard_class(),
            device.state.classes().first().copied()
        );
        assert_eq!(tabs.showing(), Some(Spot::Keyboard));
        assert_eq!(
            device.queued().len(),
            asked,
            "the click asked the instrument for nothing"
        );
    }

    /// The bank's own line: what it holds out of what it could, and what is on its way.
    #[test]
    fn the_bank_sentence_counts_what_is_there_and_what_is_coming() {
        assert_eq!(sentence(12, 50, 0), "12 of 50 slots hold something");
        assert_eq!(
            sentence(12, 50, 4),
            "12 of 50 slots hold something · 4 incoming"
        );
        assert_eq!(sentence(0, 50, 0), "0 of 50 slots hold something");
    }

    /// How stale the names on screen are, in seconds up to a minute and in minutes after
    /// it — and never in the future, whatever the clock does between frames.
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
