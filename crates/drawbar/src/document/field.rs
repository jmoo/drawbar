//! One registry body as a document: the sections it reads in, the cell every field is
//! drawn as, and the morph lens over the lot.
//!
//! The division is `nord_format::panel`'s where the library authors one, the app's own
//! menu order for a settings body, and the registry's own path prefixes for a body
//! nothing has laid out. The cell is one renderer per [`ControlKind`], so a field the
//! library learns arrives here already drawn rather than waiting for a table here to
//! name it.
//!
//! Nothing here clamps: a moved control hands back the spelling `set_field` takes, the
//! document applies every set of the frame to a fresh decode, and a refused value leaves
//! the file untouched with the library's own words on screen.

use std::collections::{HashMap, HashSet};

use eframe::egui;
use nord_format::fields::{ControlKind, Field, PackedOrder, Unit};
use nord_format::panel::{Panel, Section as Placed, Selection};

use super::controls::{self, Ctx, Sets};
use super::panel::{PianoLookup, PIANO_MODEL};
use crate::app;
use crate::icon::{icon, Glyph};
use crate::workspace::LocalEntity;
use crate::{drawbar_widget, knob, led, strings};

/// The two halves of the Electro 5 transpose control — see [`transpose`].
const TRANSPOSE_ENABLED: &str = "center_panel.transpose_enabled";
const TRANSPOSE: &str = "center_panel.transpose";

/// The name under a control, the edited dot beside it, and the morph dots below.
const LABEL: f32 = 9.5;
const DOT: f32 = 6.0;
const RADIUS: f32 = 2.0;

/// A nested group's title, and the chip a nav row is made of.
const CARD_TITLE: f32 = 11.5;
const CHIP: f32 = 20.0;
const CHIP_TEXT: f32 = 11.0;
const COUNT_TEXT: f32 = 9.5;

/// The reading beside a knob, and the caption that stands where there is none.
const READING: f32 = 10.0;

/// How long a legal-value list stays a menu for an unclassified field. Past it a
/// contiguous run turns instead.
const MENU_MAX: usize = 12;

/// The performance controls a morph slot may belong to: the suffix the declaration binds
/// on, the word for it, and the letter under a control.
///
/// ⚠️ The order is the lens order and the dot order; both are read as `W · AT · P`.
const SLOTS: [(&str, &str, &str); 3] = [
    ("_wheel", "Wheel", "W"),
    ("_aftertouch", "Aftertouch", "AT"),
    ("_ctrl_pedal", "Control pedal", "P"),
];

// ---- what the document keeps between frames -----------------------------------------

/// What the field document holds that is not an edit: which morph lens is on, where the
/// reader is, and the box a wide field is being typed into.
///
/// ⚠️ An edit is never here. Every set lands on the working copy in the frame it is
/// made, so leaving the tab drops all of this and none of that.
#[derive(Default)]
pub struct State {
    /// The morph slot every morphed control is showing, or the panel's own values.
    lens: Option<usize>,
    /// The section the reader is in, and the one a nav chip asked to be taken to.
    active: Option<String>,
    jump: Option<String>,
    /// Where each section's top was painted, and the top of the region they scroll in.
    /// Read a frame later, which is what lets a chip track the scroll.
    tops: Vec<(String, f32)>,
    view_top: f32,
    /// The decode of the bytes this document was last saved as, and the paths whose
    /// value the working copy spells differently — see [`crate::fields::changed`].
    settled: Vec<Field>,
    pending: Vec<String>,
    /// The bytes `settled` and `pending` were read from.
    read: Option<(u64, u64, Option<u32>)>,
}

impl State {
    /// Read the saved bytes and the working ones, where they have moved since last time.
    ///
    /// ⚠️ Both decodes walk the whole body, so they happen when the bytes change and not
    /// per frame: a Stage 4 declares eight hundred fields.
    pub fn follow(&mut self, entity: &LocalEntity) {
        let read = (entity.id, entity.stamp, entity.saved.crc32);
        if self.read == Some(read) {
            return;
        }
        self.read = Some(read);
        self.settled = crate::fields::decoded(&entity.saved.bytes).unwrap_or_default();
        self.pending = crate::fields::changed(&entity.saved.bytes, &entity.bytes);
    }

    /// The paths the working copy spells differently from the saved bytes.
    pub fn pending(&self) -> &[String] {
        &self.pending
    }

    /// The saved bytes' fields, which are what the Advanced table reads as raw.
    pub fn settled(&self) -> &[Field] {
        &self.settled
    }

    /// Turn the lens on as the nav row would, for a test that cannot click it.
    #[cfg(test)]
    pub(super) fn pretend_lens(&mut self, slot: usize) {
        self.lens = Some(slot);
    }
}

// ---- the document ---------------------------------------------------------------------

/// How a body arrived at its sections.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// The library's own layout for this body.
    Authored { exhaustive: bool },
    /// The app's menu order, for the settings body the library lays out nowhere.
    Menus,
    /// The registry's own path prefixes, for a body nothing has laid out.
    Flat,
}

/// One registry body, divided the way it will be drawn.
pub struct Doc<'a> {
    sections: Vec<Sect<'a>>,
    /// Registered fields no group named, in registry order.
    leftovers: Vec<&'a Field>,
    /// The titles of the top-level groups the instrument is not using.
    idle: Vec<&'static str>,
    shape: Shape,
    /// Every field a section draws, so the Advanced table can flag the rest.
    shown: HashSet<&'a str>,
    /// The selectors that pick between stored alternatives. Each is the head of the
    /// cards it picks between, so none of them is also a cell.
    picks: HashSet<&'a str>,
    /// Each parameter's morph slots, by the parameter's path.
    morphs: HashMap<&'a str, [Option<&'a Field>; SLOTS.len()]>,
    fields: usize,
    slots: usize,
}

/// One section of the document, and the cards under it.
struct Sect<'a> {
    /// What a nav chip scrolls to, which two sections of the same name must not share.
    key: String,
    title: String,
    fields: Vec<&'a Field>,
    nested: Vec<Sect<'a>>,
    /// The selection that makes this one of several stored alternatives play.
    pick: Option<&'a Selection>,
    selected: bool,
    /// The titles of the groups under this one the instrument is not using.
    idle: Vec<&'static str>,
    count: usize,
}

/// A parameter and the performance controls that morph it.
struct Part<'a> {
    field: &'a Field,
    morphs: [Option<&'a Field>; SLOTS.len()],
}

/// What one cell of a section stands for.
enum Cell<'a> {
    One(Part<'a>),
    /// Nine single-bar drawbar fields, ranked 1..=9 under one prefix, as one register.
    Register(Vec<Part<'a>>),
    /// The Electro 5 transpose lamp and amount, which neither reads without the other.
    Transpose,
}

/// The document a decoded body is drawn as.
pub fn of<'a>(decoded: &nord_format::Entity, fields: &'a [Field]) -> Doc<'a> {
    let morphs = slots_of(fields);
    let mut doc = match nord_format::panel::of(decoded) {
        Some(layout) => authored(layout, fields, morphs),
        None if crate::fields::is_electro5_settings(decoded) => menus(fields, morphs),
        None => flat(fields, morphs),
    };
    doc.fields = fields.len();
    doc.slots = fields
        .iter()
        .filter(|field| matches!(field.spec.control, ControlKind::Morph { .. }))
        .count();
    for section in &mut doc.sections {
        count(section, &doc.morphs);
    }
    doc.shown = doc.sections.iter().flat_map(paths).collect();
    doc.picks = doc.sections.iter().flat_map(selectors).collect();
    doc
}

impl Doc<'_> {
    /// Whether the Edit face draws this path at all.
    pub fn shows(&self, path: &str) -> bool {
        self.shown.contains(path)
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// How many fields there are, and how many of them are morph slots.
    pub fn tally(&self) -> (usize, usize) {
        (self.fields, self.slots)
    }

    /// How many fields no group named.
    pub fn unplaced(&self) -> usize {
        self.leftovers.len()
    }
}

/// Each parameter's three morph slots, found through the declaration rather than by
/// matching names.
///
/// ⚠️ Indexed rather than searched: a Stage body declares three hundred slots among
/// nine hundred fields, and this is rebuilt every frame.
fn slots_of(fields: &[Field]) -> HashMap<&str, [Option<&Field>; SLOTS.len()]> {
    let at: HashMap<&str, &Field> = fields
        .iter()
        .map(|field| (field.path.as_str(), field))
        .collect();
    let mut out: HashMap<&str, [Option<&Field>; SLOTS.len()]> = HashMap::new();
    for slot in fields {
        let Some(parent) = slot.spec.morph_parent() else {
            continue;
        };
        let Some(parent) = at.get(parent.as_str()) else {
            continue;
        };
        let Some(which) = which_slot(&slot.path) else {
            continue;
        };
        out.entry(parent.path.as_str()).or_default()[which] = Some(slot);
    }
    out
}

/// Which performance control a slot belongs to, off the suffix the declaration binds on.
fn which_slot(path: &str) -> Option<usize> {
    let leaf = path.rsplit('.').next().unwrap_or(path);
    SLOTS
        .iter()
        .position(|(suffix, _, _)| leaf.ends_with(suffix))
}

fn authored<'a>(
    layout: &'a Panel,
    fields: &'a [Field],
    morphs: HashMap<&'a str, [Option<&'a Field>; SLOTS.len()]>,
) -> Doc<'a> {
    let resolved = layout.resolve(fields);
    let mut sections = Vec::new();
    let mut idle = Vec::new();
    for (nth, placed) in resolved.sections.iter().enumerate() {
        if !placed.relevant {
            idle.push(placed.group.title);
            continue;
        }
        let mut nested = Vec::new();
        let mut under = Vec::new();
        hoist(&placed.groups, None, fields, &mut nested, &mut under);
        sections.push(Sect {
            key: format!("s{nth}"),
            title: placed.group.title.to_string(),
            fields: placed.fields.clone(),
            nested,
            pick: None,
            selected: true,
            idle: under,
            count: 0,
        });
    }
    let named: Vec<&Field> = resolved
        .leftovers
        .iter()
        .copied()
        .filter(|field| strings::known(&field.path))
        .collect();
    if !named.is_empty() {
        sections.push(plain_sect(
            "also".to_string(),
            strings::Section::Other.title(),
            named,
        ));
    }
    Doc {
        sections,
        leftovers: resolved.leftovers,
        idle,
        shape: Shape::Authored {
            exhaustive: layout.exhaustive,
        },
        shown: HashSet::new(),
        picks: HashSet::new(),
        morphs,
        fields: 0,
        slots: 0,
    }
}

/// The groups under a section, with the third level flattened onto the second.
///
/// Nesting has no depth limit and a card inside a card inside a card reads as an
/// indent rather than as a division, so a grandchild becomes a sibling called
/// `Parent · Child`.
fn hoist<'a>(
    groups: &[Placed<'a>],
    under: Option<&str>,
    fields: &'a [Field],
    into: &mut Vec<Sect<'a>>,
    idle: &mut Vec<&'static str>,
) {
    for group in groups {
        if !group.relevant {
            idle.push(group.group.title);
            continue;
        }
        let title = match under {
            Some(parent) => format!("{parent} · {}", group.group.title),
            None => group.group.title.to_string(),
        };
        let pick = group.group.selected_by.as_ref();
        into.push(Sect {
            key: String::new(),
            title: title.clone(),
            fields: group.fields.clone(),
            nested: Vec::new(),
            pick,
            selected: pick.is_some_and(|selection| selection.selected(fields)),
            idle: Vec::new(),
            count: 0,
        });
        hoist(&group.groups, Some(&title), fields, into, idle);
    }
}

/// The settings body, in the order the instrument's own menus run.
///
/// ⚠️ The library lays out no settings body, so the division is this app's own table —
/// see `strings::FIELDS`.
fn menus<'a>(
    fields: &'a [Field],
    morphs: HashMap<&'a str, [Option<&'a Field>; SLOTS.len()]>,
) -> Doc<'a> {
    let sections = strings::SETTINGS_SECTIONS
        .iter()
        .enumerate()
        .filter_map(|(nth, section)| {
            let rows: Vec<&Field> = fields
                .iter()
                .filter(|field| strings::section(&field.path) == *section)
                .collect();
            (!rows.is_empty()).then(|| plain_sect(format!("m{nth}"), section.title(), rows))
        })
        .collect();
    Doc {
        sections,
        leftovers: Vec::new(),
        idle: Vec::new(),
        shape: Shape::Menus,
        shown: HashSet::new(),
        picks: HashSet::new(),
        morphs,
        fields: 0,
        slots: 0,
    }
}

/// Any other registry-backed body: its own path prefixes as sections, because nothing
/// here knows how that instrument's panel is divided but the registry does say which
/// fields belong together.
fn flat<'a>(
    fields: &'a [Field],
    morphs: HashMap<&'a str, [Option<&'a Field>; SLOTS.len()]>,
) -> Doc<'a> {
    let sections = prefixes(fields)
        .into_iter()
        .enumerate()
        .map(|(nth, group)| plain_sect(format!("f{nth}"), &group.title, group.rows))
        .collect();
    Doc {
        sections,
        leftovers: Vec::new(),
        idle: Vec::new(),
        shape: Shape::Flat,
        shown: HashSet::new(),
        picks: HashSet::new(),
        morphs,
        fields: 0,
        slots: 0,
    }
}

fn plain_sect<'a>(key: String, title: &str, rows: Vec<&'a Field>) -> Sect<'a> {
    Sect {
        key,
        title: title.to_string(),
        fields: rows,
        nested: Vec::new(),
        pick: None,
        selected: true,
        idle: Vec::new(),
        count: 0,
    }
}

/// One titled run of a field list.
struct Group<'a> {
    /// What these fields share, which is not what their title is unique by.
    key: String,
    title: String,
    rows: Vec<&'a Field>,
}

/// The sections a field list falls into.
///
/// A nested body's fields are contiguous and share a dotted prefix, which is the
/// division the registry itself makes. A prefix too long to read in one run is divided
/// again on the leading word of each field's own name — the Stage bodies spell their
/// sections there (`slot_a.organ_preset_1_drawbar_1`) — and a word that recurs later
/// joins the division it opened rather than starting a second one.
fn prefixes(fields: &[Field]) -> Vec<Group<'_>> {
    let mut out: Vec<Group> = Vec::new();
    for field in fields {
        let prefix = field.path.rsplit_once('.').map_or("", |(head, _)| head);
        match out.last_mut() {
            Some(group) if group.key == prefix => group.rows.push(field),
            _ => out.push(Group {
                key: prefix.to_string(),
                title: match prefix.is_empty() {
                    true => "General".to_string(),
                    false => strings::title(prefix),
                },
                rows: vec![field],
            }),
        }
    }
    out.into_iter().flat_map(divide).collect()
}

/// How many fields a section may hold before it is divided again on each field's own
/// leading word.
const SPLIT_ABOVE: usize = 128;

fn divide(group: Group<'_>) -> Vec<Group<'_>> {
    if group.rows.len() <= SPLIT_ABOVE {
        return vec![group];
    }
    let mut out: Vec<Group> = Vec::new();
    for field in group.rows {
        let leaf = field.path.rsplit('.').next().unwrap_or(&field.path);
        let word = leaf.split('_').next().unwrap_or(leaf);
        let key = format!("{}.{word}", group.key);
        match out.iter().position(|part| part.key == key) {
            Some(at) => out[at].rows.push(field),
            None => out.push(Group {
                title: match group.key.is_empty() {
                    true => strings::title(word),
                    false => format!("{} — {word}", group.title),
                },
                key,
                rows: vec![field],
            }),
        }
    }
    out
}

/// How many registry fields a section stands for: its own, the morph slots riding on
/// them, and everything under it.
fn count(section: &mut Sect<'_>, morphs: &HashMap<&str, [Option<&Field>; SLOTS.len()]>) {
    let mut total = section.fields.len();
    for field in &section.fields {
        total += morphs.get(field.path.as_str()).map_or(0, |slots| {
            slots.iter().filter(|slot| slot.is_some()).count()
        });
    }
    for nested in &mut section.nested {
        count(nested, morphs);
        total += nested.count;
    }
    section.count = total;
}

/// The selectors that pick between a section's stored alternatives, however deep the
/// layout nested them before they were flattened onto one row.
fn selectors<'a>(section: &Sect<'a>) -> Vec<&'a str> {
    let mut out: Vec<&str> = section
        .pick
        .map(|selection| selection.field)
        .into_iter()
        .collect();
    for nested in &section.nested {
        out.extend(selectors(nested));
    }
    out
}

/// Every path a section draws, its nested cards included.
fn paths<'a>(section: &Sect<'a>) -> Vec<&'a str> {
    let mut out: Vec<&str> = section
        .fields
        .iter()
        .map(|field| field.path.as_str())
        .collect();
    for nested in &section.nested {
        out.extend(paths(nested));
    }
    out
}

// ---- the sticky nav and the morph lens -------------------------------------------------

/// The row above the scroll region: one chip per section, and the morph lens where the
/// body has morph slots.
///
/// ⚠️ It wraps rather than scrolling sideways. The lens is at the right of it, and a row
/// that clipped would put the lens out of reach on a narrow window.
pub fn nav(ui: &mut egui::Ui, state: &mut State, doc: &Doc<'_>) {
    if doc.sections.is_empty() {
        return;
    }
    let quiet = app::caption(ui.visuals());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
        for section in &doc.sections {
            let active = state.active.as_deref() == Some(section.key.as_str());
            if chip(ui, &section.title, &section.count.to_string(), active).clicked() {
                state.jump = Some(section.key.clone());
            }
        }
        if doc.slots == 0 {
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (nth, (_, word, _)) in SLOTS.iter().enumerate().rev() {
                let count = stored_targets(doc, nth);
                if chip(ui, word, &count.to_string(), state.lens == Some(nth)).clicked() {
                    state.lens = Some(nth);
                }
            }
            if chip(ui, "Panel", "", state.lens.is_none()).clicked() {
                state.lens = None;
            }
            ui.label(
                egui::RichText::new("MORPH")
                    .font(egui::FontId::proportional(COUNT_TEXT))
                    .color(quiet),
            );
        });
    });
}

/// How many parameters have a target stored under one performance control.
fn stored_targets(doc: &Doc<'_>, slot: usize) -> usize {
    doc.morphs
        .values()
        .filter(|slots| slots[slot].is_some_and(|field| !is_neutral(field)))
        .count()
}

fn chip(ui: &mut egui::Ui, title: &str, count: &str, active: bool) -> egui::Response {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let ink = match active {
        true => visuals.text_color(),
        false => visuals.weak_text_color(),
    };
    let word = painter.layout_no_wrap(
        title.to_string(),
        egui::FontId::proportional(CHIP_TEXT),
        ink,
    );
    let tail = (!count.is_empty()).then(|| {
        painter.layout_no_wrap(
            count.to_string(),
            egui::FontId::monospace(COUNT_TEXT),
            match active {
                true => app::accent(&visuals),
                false => app::caption(&visuals),
            },
        )
    });
    let width = 16.0 + word.size().x + tail.as_ref().map_or(0.0, |laid| 5.0 + laid.size().x);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, CHIP), egui::Sense::click());
    if active || response.hovered() {
        let fill = match active {
            true => visuals.widgets.active.weak_bg_fill,
            false => visuals.widgets.hovered.weak_bg_fill,
        };
        painter.rect_filled(rect, RADIUS, fill);
    }
    let mut x = rect.left() + 8.0;
    painter.galley(
        egui::pos2(x, rect.center().y - word.size().y / 2.0),
        word.clone(),
        ink,
    );
    x += word.size().x + 5.0;
    if let Some(tail) = tail {
        painter.galley(
            egui::pos2(x, rect.center().y - tail.size().y / 2.0),
            tail,
            ink,
        );
    }
    response
}

// ---- the body ---------------------------------------------------------------------

/// Draw the whole document. Returns whether something asked for the Advanced face.
pub fn body(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    state: &mut State,
    doc: &Doc<'_>,
    piano: &mut PianoLookup,
    sets: &mut Sets,
) -> bool {
    let mut to_advanced = false;
    state.view_top = ui.clip_rect().top();
    let mut tops = Vec::with_capacity(doc.sections.len());
    if state.lens.is_some() {
        banner(ui, state);
    }
    for section in &doc.sections {
        let top = ui.cursor().top();
        if state.jump.as_deref() == Some(section.key.as_str()) {
            ui.scroll_to_rect(
                egui::Rect::from_min_size(ui.cursor().min, egui::vec2(1.0, 1.0)),
                Some(egui::Align::TOP),
            );
        }
        tops.push((section.key.clone(), top));
        to_advanced |= drew(ui, ctx, state, doc, section, piano, sets);
        ui.add_space(6.0);
        ui.separator();
    }
    state.jump = None;
    state.tops = tops;
    state.active = active(state);
    to_advanced |= foot(ui, doc);
    to_advanced
}

/// The section whose top is the last one above the region's own top.
fn active(state: &State) -> Option<String> {
    state
        .tops
        .iter()
        .rfind(|(_, top)| *top <= state.view_top + 1.0)
        .or_else(|| state.tops.first())
        .map(|(key, _)| key.clone())
}

/// The strip that says the panel values are not what is on screen.
fn banner(ui: &mut egui::Ui, state: &mut State) {
    let Some(slot) = state.lens else { return };
    let visuals = ui.visuals().clone();
    let drawn = egui::Frame::new()
        .fill(visuals.window_fill)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                icon(ui, Glyph::ScanEye, 13.0, app::accent(&visuals));
                ui.label(
                    egui::RichText::new(format!(
                        "Showing {} targets. A lit outline is a control with a morph stored, a \
                         grey one is neutral, and editing here writes the morph slot rather than \
                         the panel value.",
                        SLOTS[slot].1.to_lowercase()
                    ))
                    .font(egui::FontId::proportional(CHIP_TEXT))
                    .color(visuals.text_color()),
                );
                if ui
                    .small_button("Back to the panel")
                    .on_hover_text("the values the instrument holds with nothing moving")
                    .clicked()
                {
                    state.lens = None;
                }
            });
        });
    let rule = egui::Stroke::new(1.0_f32, app::accent(&visuals));
    let rect = drawn.response.rect;
    ui.painter()
        .hline(rect.x_range(), rect.bottom() - 0.5, rule);
}

/// One section: its own controls, the cards under it, the alternatives beside each
/// other, and the line that says what is stored and idle.
fn drew(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    state: &State,
    doc: &Doc<'_>,
    section: &Sect<'_>,
    piano: &mut PianoLookup,
    sets: &mut Sets,
) -> bool {
    let quiet = app::caption(ui.visuals());
    let reading = match section.count {
        1 => "1 field".to_string(),
        n => format!("{n} fields"),
    };
    controls::heading(ui, &section.title, "", Some((&reading, quiet)));
    if section.fields.iter().any(|field| field.path == PIANO_MODEL) {
        piano.ui(ui);
    }
    cells(ui, ctx, state, doc, &section.fields, piano, sets);

    let (alternatives, cards): (Vec<&Sect>, Vec<&Sect>) = section
        .nested
        .iter()
        .partition(|nested| nested.pick.is_some());
    for card in cards {
        egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(egui::Stroke::new(
                1.0_f32,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .corner_radius(RADIUS)
            .inner_margin(egui::Margin::same(8))
            .outer_margin(egui::Margin::symmetric(12, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 24.0);
                card_title(ui, &card.title, None);
                cells(ui, ctx, state, doc, &card.fields, piano, sets);
            });
    }
    if !alternatives.is_empty() {
        side_by_side(ui, ctx, state, doc, &alternatives, piano, sets);
    }
    idle_line(ui, &section.idle)
}

/// The stored alternatives, side by side: one is playing and the other is kept.
///
/// ⚠️ Both stay on screen. The selector that picks between them is the head of each
/// card, and drawing only the one in use would put the switch inside the thing it
/// switches.
fn side_by_side(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    state: &State,
    doc: &Doc<'_>,
    alternatives: &[&Sect<'_>],
    piano: &mut PianoLookup,
    sets: &mut Sets,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        for alternative in alternatives {
            let stroke = match alternative.selected {
                true => app::accent(ui.visuals()),
                false => ui.visuals().widgets.noninteractive.bg_stroke.color,
            };
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .stroke(egui::Stroke::new(1.0_f32, stroke))
                .corner_radius(RADIUS)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    if let Some(selection) = alternative.pick {
                        if card_title(ui, &alternative.title, Some(alternative.selected))
                            && !alternative.selected
                        {
                            sets.push((selection.field.to_string(), selection.value.to_string()));
                        }
                    }
                    if !alternative.selected {
                        ui.set_opacity(0.45);
                    }
                    cells(ui, ctx, state, doc, &alternative.fields, piano, sets);
                });
        }
    });
}

/// A card's own head. With `playing` it is the selector as well, and returns whether it
/// was clicked.
fn card_title(ui: &mut egui::Ui, title: &str, playing: Option<bool>) -> bool {
    let visuals = ui.visuals().clone();
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if let Some(playing) = playing {
            let lit = match playing {
                true => app::accent(&visuals),
                false => app::unlit(&visuals),
            };
            clicked |= app::dot(ui, lit, 8.0).clicked();
        }
        clicked |= ui
            .add(
                egui::Label::new(
                    egui::RichText::new(title)
                        .font(egui::FontId::new(CARD_TITLE, app::bold()))
                        .color(visuals.text_color()),
                )
                .sense(match playing.is_some() {
                    true => egui::Sense::click(),
                    false => egui::Sense::hover(),
                }),
            )
            .clicked();
        match playing {
            Some(true) => {
                ui.label(
                    egui::RichText::new("playing")
                        .font(egui::FontId::proportional(READING))
                        .color(app::good(&visuals)),
                );
            }
            Some(false) => {
                clicked |= ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new("select")
                                .font(egui::FontId::proportional(READING))
                                .color(app::caption(&visuals)),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text("the other stays stored, it is simply not the one playing")
                    .clicked();
            }
            None => {}
        }
    });
    ui.add_space(4.0);
    clicked
}

/// The one line a section ends with when something under it is stored and idle.
fn idle_line(ui: &mut egui::Ui, idle: &[&'static str]) -> bool {
    if idle.is_empty() {
        return false;
    }
    let quiet = app::caption(ui.visuals());
    let mut asked = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add_space(12.0);
        icon(ui, Glyph::EyeOff, 11.0, quiet);
        ui.label(
            egui::RichText::new(format!(
                "{} {} stored but not in use for the state this file holds — kept, not cleared.",
                listed(idle),
                match idle.len() {
                    1 => "is",
                    _ => "are",
                }
            ))
            .font(egui::FontId::proportional(READING))
            .color(quiet),
        );
        asked = ui
            .add(
                egui::Label::new(
                    egui::RichText::new("Advanced")
                        .font(egui::FontId::proportional(READING))
                        .color(app::accent(ui.visuals())),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_text("every field, including the ones this face does not draw")
            .clicked();
    });
    asked
}

/// `A`, `A and B`, `A, B and C`.
fn listed(words: &[&str]) -> String {
    match words {
        [] => String::new(),
        [one] => (*one).to_string(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// The line under the last section: what the layout does not place.
fn foot(ui: &mut egui::Ui, doc: &Doc<'_>) -> bool {
    let quiet = app::caption(ui.visuals());
    let mut asked = false;
    if !doc.idle.is_empty() {
        asked |= idle_line(ui, &doc.idle);
    }
    let unplaced = doc.leftovers.len();
    if matches!(doc.shape, Shape::Authored { exhaustive: false }) && unplaced > 0 {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_space(12.0);
            icon(ui, Glyph::CircleAlert, 11.0, quiet);
            ui.label(
                egui::RichText::new(format!(
                    "{unplaced} fields the layout does not place — under Advanced, and under \
                     Also stored once the strings table names them."
                ))
                .font(egui::FontId::proportional(READING))
                .color(quiet),
            );
        });
    }
    asked
}

// ---- the cells ---------------------------------------------------------------------

/// A run of fields as cells, wrapping where the window is narrow.
fn cells(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    state: &State,
    doc: &Doc<'_>,
    rows: &[&Field],
    piano: &mut PianoLookup,
    sets: &mut Sets,
) {
    let built = clustered(rows, doc);
    if built.is_empty() {
        return;
    }
    let row = egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true);
    ui.with_layout(row, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(14.0, 10.0);
        for cell in &built {
            match cell {
                Cell::One(part) => one(ui, ctx, state, part, rows, piano, sets),
                Cell::Register(bars) => register(ui, ctx, state, bars, sets),
                Cell::Transpose => transpose(ui, state, rows, sets),
            }
        }
    });
}

/// The cells a run of fields becomes: morph slots folded onto the parameters they move,
/// drawbar runs merged into registers, and the transpose pair as one control.
fn clustered<'a>(rows: &[&'a Field], doc: &Doc<'a>) -> Vec<Cell<'a>> {
    let mut parts: Vec<Part<'a>> = Vec::new();
    let mut transposed = false;
    for field in rows {
        // ⚠️ The selector that picks one of several stored alternatives is the head of
        // each alternative's own card. Drawn here as well it would be two controls for
        // one switch — and the layout keeps it in the group *above* the ones it picks
        // between, so it reaches a strip that has no alternatives of its own.
        if doc.picks.contains(field.path.as_str()) {
            continue;
        }
        if field.path == TRANSPOSE_ENABLED || field.path == TRANSPOSE {
            transposed = true;
            continue;
        }
        // A slot whose parameter is in this body rides on that parameter; one with no
        // parameter beside it stands as its own cell rather than disappearing.
        if field
            .spec
            .morph_parent()
            .is_some_and(|parent| doc.morphs.contains_key(parent.as_str()))
        {
            continue;
        }
        parts.push(Part {
            field,
            morphs: doc
                .morphs
                .get(field.path.as_str())
                .copied()
                .unwrap_or_default(),
        });
    }
    let mut out = merged(parts);
    if transposed {
        out.push(Cell::Transpose);
    }
    out
}

/// Nine single-bar drawbar fields under one prefix, ranked 1..=9 in order, as one
/// register.
///
/// ⚠️ Branch on the kind's bar count, never on the width: the Electro 5 packs a whole
/// registration into one field and the Stage bodies give each bar its own.
fn merged(parts: Vec<Part<'_>>) -> Vec<Cell<'_>> {
    let mut out: Vec<Cell> = Vec::new();
    let mut run: Vec<Part> = Vec::new();
    for part in parts {
        if !fitting(&run, part.field) {
            out.extend(run.drain(..).map(Cell::One));
        }
        match fitting(&run, part.field) {
            true => run.push(part),
            false => out.push(Cell::One(part)),
        }
        if run.len() == drawbar_widget::BARS {
            out.push(Cell::Register(std::mem::take(&mut run)));
        }
    }
    out.extend(run.into_iter().map(Cell::One));
    out
}

/// Whether a bar carries on the register the run has opened.
fn fitting(run: &[Part<'_>], field: &Field) -> bool {
    let Some((stem, rank)) = ranked(field) else {
        return false;
    };
    usize::from(rank) == run.len() + 1
        && run
            .first()
            .and_then(|first| ranked(first.field))
            .is_none_or(|(opened, _)| opened == stem)
}

/// A single-bar drawbar's register prefix and its one-based place in it, or `None` for
/// anything the widget cannot place.
fn ranked(field: &Field) -> Option<(&str, u8)> {
    let ControlKind::Drawbar {
        bars: 1,
        rank: Some(rank),
        ..
    } = field.spec.control
    else {
        return None;
    };
    if !(1..=drawbar_widget::BARS as u8).contains(&rank) {
        return None;
    }
    let (stem, _) = field.path.rsplit_once('_')?;
    Some((stem, rank))
}

/// The morph target a lens puts under a parameter's own control, or nothing where the
/// lens is off or the parameter has no target for it.
///
/// ⚠️ It is what the cell **writes** as well as what it reads: an edit made under a lens
/// goes to the morph slot, never to the panel value beside it.
fn shown<'a>(part: &Part<'a>, lens: Option<usize>) -> Option<&'a Field> {
    lens.and_then(|slot| part.morphs[slot])
}

/// One parameter as a cell: the control, its name underneath, and the morph handles
/// under that.
fn one(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    state: &State,
    part: &Part<'_>,
    rows: &[&Field],
    piano: &mut PianoLookup,
    sets: &mut Sets,
) {
    let lensed = shown(part, state.lens);
    let drawn = lensed.unwrap_or(part.field);
    let dim = state.lens.is_some() && lensed.is_none();
    let legal = ctx.legal(drawn);

    if lensed.is_none() && part.field.path == PIANO_MODEL && piano.model_cell(ui, part.field, sets)
    {
        return;
    }
    let named = piano.names(drawn);

    let span = width(drawn, &legal);
    let drawn_at = ui
        .allocate_ui(egui::vec2(span, 0.0), |ui| {
            if dim {
                ui.set_opacity(0.45);
            }
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                if let Some(value) = control(ui, drawn, &legal, rows, named) {
                    sets.push((drawn.path.clone(), value));
                }
                caption(ui, part.field, state.pending.contains(&part.field.path));
                if state.lens.is_none() {
                    dots(ui, &part.morphs);
                }
            });
        })
        .response;
    if let Some(slot) = lensed {
        outline(ui, drawn_at.rect, is_neutral(slot));
    }
}

/// The lit or grey ring round a control the lens is showing a target for.
fn outline(ui: &egui::Ui, rect: egui::Rect, neutral: bool) {
    let ink = match neutral {
        true => app::unlit(ui.visuals()),
        false => app::accent(ui.visuals()),
    };
    ui.painter().rect_stroke(
        rect.expand(2.0),
        RADIUS,
        egui::Stroke::new(1.0_f32, ink),
        egui::StrokeKind::Inside,
    );
}

/// The name under a control: the app's word for it, or the prettified path in mono where
/// the table has no word yet.
fn caption(ui: &mut egui::Ui, field: &Field, edited: bool) {
    named_caption(ui, &field.path, edited, note(field));
}

/// The same caption over a path, for the register nine fields are drawn as.
fn named_caption(ui: &mut egui::Ui, path: &str, edited: bool, note: &str) {
    let known = strings::known(path);
    let quiet = ui.visuals().weak_text_color();
    let response = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if edited {
                app::dot(ui, app::warn(ui.visuals()), DOT);
            }
            if !known {
                icon(ui, Glyph::Tag, 9.0, app::caption(ui.visuals()));
            }
            ui.add(egui::Label::new(
                egui::RichText::new(strings::label(path))
                    .font(match known {
                        true => egui::FontId::proportional(LABEL),
                        false => egui::FontId::monospace(LABEL),
                    })
                    .color(quiet),
            ));
        })
        .response;
    let mut hint = path.to_string();
    if !known {
        hint.push_str(" — no label yet; showing the prettified path");
    }
    if !note.is_empty() {
        hint.push_str(" · ");
        hint.push_str(note);
    }
    response.on_hover_text(hint);
}

/// What a kind is worth saying beside its own control, and nothing where it is not.
///
/// Each line is a fact the library states about the encoding, not a description of the
/// widget.
fn note(field: &Field) -> &'static str {
    match field.spec.control {
        // Confirmed on hardware: the panel reads this slot either way.
        ControlKind::Bipolar(_) => {
            "centre is the slot midpoint — accurate at the ends, approximate between"
        }
        // Inferred from specimens; not confirmed on hardware.
        ControlKind::Pattern { .. } => "step order inferred",
        ControlKind::Drawbar { .. } => "rank is a position, not a pitch",
        // Inferred from specimens; not confirmed on hardware.
        ControlKind::Morph { .. } => "a morph slot with no parameter beside it",
        _ => "",
    }
}

/// The three performance controls under a morphed parameter.
fn dots(ui: &mut egui::Ui, morphs: &[Option<&Field>; SLOTS.len()]) {
    if morphs.iter().all(Option::is_none) {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (nth, (_, word, short)) in SLOTS.iter().enumerate() {
            let Some(slot) = morphs[nth] else { continue };
            let lit = !is_neutral(slot);
            let ink = match lit {
                true => app::accent(ui.visuals()),
                false => app::unlit(ui.visuals()),
            };
            let hint = match lit {
                true => format!("{word} → {}", slot.value),
                false => format!("{word} — neutral"),
            };
            app::dot(ui, ink, DOT).on_hover_text(hint.clone());
            ui.label(
                egui::RichText::new(*short)
                    .font(egui::FontId::proportional(COUNT_TEXT))
                    .color(app::caption(ui.visuals())),
            )
            .on_hover_text(hint);
            ui.add_space(2.0);
        }
    });
}

/// The value a morph slot of this width holds when nothing morphs its parent.
///
/// ⚠️ The morph encoding is not established, and a `Field` carries the stored number
/// rather than the library's own answer — so the midpoint is read back off the library's
/// constant for the slot's width rather than restated here.
/// Inferred from specimens; not confirmed on hardware.
fn neutral(width: u32) -> Option<u64> {
    use nord_format::components::MorphOf;
    Some(u64::from(match width {
        1 => MorphOf::<1>::NEUTRAL,
        2 => MorphOf::<2>::NEUTRAL,
        3 => MorphOf::<3>::NEUTRAL,
        4 => MorphOf::<4>::NEUTRAL,
        5 => MorphOf::<5>::NEUTRAL,
        6 => MorphOf::<6>::NEUTRAL,
        7 => MorphOf::<7>::NEUTRAL,
        8 => MorphOf::<8>::NEUTRAL,
        _ => return None,
    }))
}

fn is_neutral(slot: &Field) -> bool {
    word(&slot.value) == neutral(slot.spec.width)
}

// ---- one renderer per control kind ---------------------------------------------------

/// How wide a cell is, which is as wide as what stands in it.
fn width(field: &Field, legal: &[String]) -> f32 {
    match field.spec.control {
        ControlKind::Toggle => 84.0,
        ControlKind::Selector => 156.0,
        ControlKind::Shift(_) => 100.0,
        ControlKind::Drawbar { bars: 1, .. } => 44.0,
        ControlKind::Drawbar { .. } => 220.0,
        ControlKind::Pattern { steps, .. } => (f32::from(steps) * 13.0).max(110.0),
        ControlKind::Reference(_) => 176.0,
        _ if legal.is_empty() => 168.0,
        ControlKind::Knob(_) | ControlKind::Bipolar(_) => 78.0,
        ControlKind::Morph { .. } | ControlKind::Number => match legal.len() <= MENU_MAX {
            true => 156.0,
            false => 78.0,
        },
    }
}

/// The control alone, without its name. Returns the spelling `set_field` takes when it
/// has been moved.
///
/// ⚠️ Exhaustive over [`ControlKind`], so a kind the library adds is a compile error
/// here rather than a field that silently loses its widget.
fn control(
    ui: &mut egui::Ui,
    field: &Field,
    legal: &[String],
    rows: &[&Field],
    named: Option<&str>,
) -> Option<String> {
    match field.spec.control {
        ControlKind::Drawbar { bars: 1, rank, .. } => bar(ui, field, rank),
        ControlKind::Drawbar { order, .. } => packed(ui, field, order),
        ControlKind::Pattern {
            steps,
            bits_per_step,
            order,
        } => pattern(ui, field, steps, bits_per_step, order),
        ControlKind::Reference(library) => reference(ui, field, library, named),
        // Above the enumerable ceiling a field lists nothing to offer, so its stored
        // bits are the only spelling there is.
        _ if legal.is_empty() => wide(ui, field),
        ControlKind::Toggle => toggle(ui, field, legal),
        ControlKind::Selector => selector(ui, field, legal),
        ControlKind::Knob(unit) => turned(ui, field, legal, unit, false, rows),
        ControlKind::Bipolar(unit) => turned(ui, field, legal, unit, true, rows),
        ControlKind::Shift(unit) => shift(ui, field, legal, unit),
        ControlKind::Morph { .. } | ControlKind::Number => plain(ui, field, legal),
    }
}

/// A lamp and the word for the state it is in.
///
/// The two states may be named rather than spelled `true`/`false`, and what is written
/// back is whichever of the two the field itself lists.
fn toggle(ui: &mut egui::Ui, field: &Field, legal: &[String]) -> Option<String> {
    let named = match legal {
        [off, on] if off != "false" || on != "true" => Some((off.as_str(), on.as_str())),
        _ => None,
    };
    let (off, on) = named.unwrap_or(("off", "on"));
    let lit = match named {
        Some((_, on)) => field.value == on,
        None => field.value == "true",
    };
    let word = match lit {
        true => on,
        false => off,
    };
    let want = led::ui(ui, lit, word)?;
    Some(match named {
        Some((off, on)) => match want {
            true => on.to_string(),
            false => off.to_string(),
        },
        None => want.to_string(),
    })
}

/// A named-value picker over every position the field lists.
///
/// ⚠️ A position the library could not name is offered as `unknown (n)` and flagged, not
/// hidden: real files hold them, and that spelling is the only way to put one back.
fn selector(ui: &mut egui::Ui, field: &Field, legal: &[String]) -> Option<String> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if strings::unrecognised(&field.value).is_some() {
            icon(ui, Glyph::CircleHelp, 11.0, app::warn(ui.visuals()))
                .on_hover_text("the panel cannot produce this position, and the file holds it");
        }
        egui::ComboBox::from_id_salt(&field.path)
            .selected_text(
                egui::RichText::new(strings::value_label(&field.path, &field.value))
                    .text_style(egui::TextStyle::Small),
            )
            .width(ui.available_width().min(128.0))
            .show_ui(ui, |ui| {
                for value in offered(&field.path, legal, &field.value) {
                    let word = strings::value_label(&field.path, &value);
                    if ui.selectable_label(value == field.value, word).clicked() {
                        picked = Some(value);
                    }
                }
            });
    });
    picked.filter(|value| *value != field.value)
}

/// The values a picker offers.
fn offered(path: &str, legal: &[String], current: &str) -> Vec<String> {
    let mut out: Vec<String> = legal
        .iter()
        .filter(|value| offerable(path, value))
        .cloned()
        .collect();
    if !out.iter().any(|value| value == current) {
        out.push(current.to_string());
    }
    out
}

/// Whether a value is one a player would pick.
///
/// `Routing::Unknown` is a named variant rather than an unrecognised position, but it is
/// how older firmware spelled *off* and it presents as off. Two entries both meaning off
/// are not offered together.
/// Confirmed on hardware.
fn offerable(path: &str, value: &str) -> bool {
    !(value == "Unknown"
        && matches!(
            path,
            "effects_panel.fx1" | "effects_panel.fx2" | "effects_panel.fx3" | "effects_panel.fx4"
        ))
}

/// A knob, and beside it whatever reading the unit actually supports.
///
/// ⚠️ A unit is a label, not a promise of conversion. Where the panel's curve is not
/// published the knob prints the stored byte and the caption names the scale — nothing
/// here invents a reading the file does not support.
fn turned(
    ui: &mut egui::Ui,
    field: &Field,
    legal: &[String],
    unit: Unit,
    centred: bool,
    rows: &[&Field],
) -> Option<String> {
    let Some((min, max)) = contiguous(legal) else {
        return plain(ui, field, legal);
    };
    let value: i64 = field.value.trim_start_matches('+').parse().ok()?;
    let mut moved = None;
    let dial = ui
        .scope(|ui| moved = knob::ui(ui, &field.path, value, min, max))
        .response;
    if centred {
        detent(ui, dial.rect);
    }
    let shown = moved.unwrap_or(value);
    match reading(unit, centred, shown, min, max) {
        Some(text) => {
            ui.label(
                egui::RichText::new(text)
                    .font(egui::FontId::monospace(READING))
                    .color(ui.visuals().weak_text_color()),
            );
        }
        None => {
            if let Some(word) = scale(unit) {
                let hint = clocked(field, rows);
                let drawn = ui.label(
                    egui::RichText::new(word)
                        .font(egui::FontId::proportional(READING))
                        .color(app::warn(ui.visuals())),
                );
                match hint {
                    Some(sibling) => drawn.on_hover_text(sibling),
                    None => drawn.on_hover_text(
                        "the panel's curve for this unit is not published, so the stored value \
                         is what is shown",
                    ),
                };
            }
        }
    }
    moved.filter(|moved| *moved != value).map(|m| m.to_string())
}

/// The mark at twelve o'clock on a knob whose musical zero is its centre.
///
/// ⚠️ The knob's sweep is symmetrical about straight up, so the slot's midpoint is
/// already where the tick goes — the lit arc still fills from the bottom stop, because
/// the stored range runs `0..=127` and nothing in it is negative.
fn detent(ui: &egui::Ui, dial: egui::Rect) {
    let top = egui::pos2(dial.center().x, dial.top());
    ui.painter().line_segment(
        [top, egui::pos2(top.x, top.y + 5.0)],
        egui::Stroke::new(1.0_f32, app::caption(ui.visuals())),
    );
}

/// The panel reading beside a knob, where the unit supports one.
fn reading(unit: Unit, centred: bool, value: i64, min: i64, max: i64) -> Option<String> {
    if centred {
        // The slot midpoint, which is where the library takes centre to be.
        let centre = (min + max + 1) / 2;
        return Some(format!("{:+}", value - centre));
    }
    match (unit.describes_a_known_transform(), unit) {
        (true, Unit::Panel10) if max > min => Some(format!(
            "{:.1}",
            (value - min) as f64 / (max - min) as f64 * 10.0
        )),
        _ => None,
    }
}

/// What a knob's axis is called where its curve is unpublished.
fn scale(unit: Unit) -> Option<&'static str> {
    match unit {
        Unit::Hertz => Some("Hz scale"),
        Unit::Milliseconds => Some("ms scale"),
        Unit::Bpm => Some("BPM scale"),
        Unit::ClockDivision => Some("division"),
        Unit::Pan => Some("stored"),
        _ => None,
    }
}

/// The sibling flag a clocked rate reads against, where the body declares one.
///
/// ⚠️ Neither field is a reading on its own: the same slot reads in hertz or as a
/// subdivision depending on the flag beside it.
/// Inferred from specimens; not confirmed on hardware.
fn clocked(field: &Field, rows: &[&Field]) -> Option<String> {
    if !matches!(field.spec.control, ControlKind::Knob(Unit::ClockDivision)) {
        return None;
    }
    let stem = field.path.rsplit_once('_').map(|(head, _)| head)?;
    let sibling = rows
        .iter()
        .find(|other| other.path.starts_with(stem) && other.path.ends_with("_clock"))?;
    Some(format!(
        "reads as a clock division or a rate depending on {}",
        sibling.path
    ))
}

/// A signed offset, stepped one at a time and stopped at the ends of its own travel.
fn shift(ui: &mut egui::Ui, field: &Field, legal: &[String], unit: Unit) -> Option<String> {
    let Some((min, max)) = contiguous(legal) else {
        return plain(ui, field, legal);
    };
    let value: i64 = field.value.trim_start_matches('+').parse().ok()?;
    let word = match unit {
        Unit::Semitones => "st",
        Unit::Octaves => "oct",
        _ => "",
    };
    let mut moved = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        if ui
            .add_enabled(value > min, egui::Button::new("−").small())
            .clicked()
        {
            moved = Some(value - 1);
        }
        ui.label(
            egui::RichText::new(format!("{value:+}"))
                .font(egui::FontId::monospace(11.5))
                .color(ui.visuals().text_color()),
        );
        if ui
            .add_enabled(value < max, egui::Button::new("+").small())
            .clicked()
        {
            moved = Some(value + 1);
        }
        if !word.is_empty() {
            ui.label(
                egui::RichText::new(word)
                    .font(egui::FontId::proportional(READING))
                    .color(app::caption(ui.visuals())),
            );
        }
    });
    moved.map(|moved| moved.to_string())
}

/// One drawbar, for the bodies that give each bar its own field.
fn bar(ui: &mut egui::Ui, field: &Field, rank: Option<u8>) -> Option<String> {
    let position = field.value.trim().parse().ok()?;
    let rank = rank
        .and_then(|rank| usize::from(rank).checked_sub(1))
        .filter(|rank| *rank < drawbar_widget::BARS);
    drawbar_widget::ui_one(ui, rank, position, true).map(|moved| moved.to_string())
}

/// A whole register in one field.
///
/// ⚠️ Read from the wrong end a register comes out mirrored and looks like a plausible
/// registration, so the packing order is asked of the field rather than assumed.
fn packed(ui: &mut egui::Ui, field: &Field, order: PackedOrder) -> Option<String> {
    let bits = drawbar_widget::parse(&field.value)?;
    let stored = drawbar_widget::bars(bits);
    let shown = match order {
        PackedOrder::HighFirst => stored,
        PackedOrder::LowFirst => mirrored(stored),
    };
    let moved = bars(ui, shown)?;
    let back = match order {
        PackedOrder::HighFirst => moved,
        PackedOrder::LowFirst => mirrored(moved),
    };
    Some(drawbar_widget::spell(drawbar_widget::bits(back)))
}

fn mirrored(positions: [u8; drawbar_widget::BARS]) -> [u8; drawbar_widget::BARS] {
    let mut out = positions;
    out.reverse();
    out
}

/// The drawbars themselves, plus the digits under them. No hex: the digits are the
/// readout.
fn bars(
    ui: &mut egui::Ui,
    positions: [u8; drawbar_widget::BARS],
) -> Option<[u8; drawbar_widget::BARS]> {
    let mut moved = None;
    ui.vertical(|ui| {
        moved = drawbar_widget::ui_ranks(ui, positions, true, &drawbar_widget::ALL_RANKS);
        ui.label(
            egui::RichText::new(drawbar_widget::digits(&moved.unwrap_or(positions)))
                .font(egui::FontId::monospace(READING))
                .color(ui.visuals().weak_text_color()),
        );
    });
    moved
}

/// Nine ranked single-bar fields as one register: each bar writes its own field, and
/// only the bars that moved are written.
fn register(ui: &mut egui::Ui, ctx: &Ctx, state: &State, run: &[Part<'_>], sets: &mut Sets) {
    if let Some(slot) = state.lens {
        // The targets keep the register's own order, so the bars stay side by side under
        // every lens — even where a slot is wider than the bar it morphs and has no
        // drawbar of its own to be drawn as.
        for (nth, part) in run.iter().enumerate() {
            let Some(target) = part.morphs[slot] else {
                continue;
            };
            let legal = ctx.legal(target);
            let at = ui
                .allocate_ui(egui::vec2(64.0, 0.0), |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        if let Some(value) = plain(ui, target, &legal) {
                            sets.push((target.path.clone(), value));
                        }
                        ui.label(
                            egui::RichText::new(format!("bar {}", nth + 1))
                                .font(egui::FontId::proportional(LABEL))
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                })
                .response;
            outline(ui, at.rect, is_neutral(target));
        }
        return;
    }

    let positions: [u8; drawbar_widget::BARS] = std::array::from_fn(|n| {
        run.get(n)
            .and_then(|part| part.field.value.trim().parse().ok())
            .unwrap_or(0)
    });
    let edited = run
        .iter()
        .any(|part| state.pending.contains(&part.field.path));
    ui.allocate_ui(egui::vec2(220.0, 0.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if let Some(moved) = bars(ui, positions) {
                for (part, (was, now)) in run.iter().zip(positions.iter().zip(moved)) {
                    if *was != now {
                        sets.push((part.field.path.clone(), now.to_string()));
                    }
                }
            }
            let stem = ranked(run[0].field).map_or(run[0].field.path.as_str(), |(stem, _)| stem);
            named_caption(ui, stem, edited, "rank is a position, not a pitch");
            let any: [Option<&Field>; SLOTS.len()] = std::array::from_fn(|slot| {
                run.iter()
                    .find_map(|part| part.morphs[slot].filter(|slot| !is_neutral(slot)))
                    .or_else(|| run.iter().find_map(|part| part.morphs[slot]))
            });
            dots(ui, &any);
        });
    });
}

/// A per-step grid. A click moves one step on to the next value it can hold.
fn pattern(
    ui: &mut egui::Ui,
    field: &Field,
    steps: u8,
    bits_per_step: u8,
    order: PackedOrder,
) -> Option<String> {
    let stored = word(&field.value)?;
    let mask = (1u64 << bits_per_step) - 1;
    let at = |step: usize| -> u32 {
        let nth = match order {
            PackedOrder::LowFirst => step,
            PackedOrder::HighFirst => usize::from(steps) - 1 - step,
        };
        u32::from(bits_per_step) * nth as u32
    };
    let mut moved = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
        for step in 0..usize::from(steps) {
            let shift = at(step);
            let held = (stored >> shift) & mask;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(11.0, 14.0), egui::Sense::click());
            let ink = match held {
                0 => app::unlit(ui.visuals()),
                _ => app::accent(ui.visuals()),
            };
            ui.painter().rect_filled(rect, RADIUS, ink);
            if response
                .on_hover_text(format!("step {} — {held} · step order inferred", step + 1))
                .clicked()
            {
                let next = (held + 1) & mask;
                moved = Some((stored & !(mask << shift)) | (next << shift));
            }
        }
    });
    moved.map(|bits| format!("{bits:#x}"))
}

/// An id into a library the file does not contain.
fn reference(
    ui: &mut egui::Ui,
    field: &Field,
    library: nord_format::fields::Library,
    named: Option<&str>,
) -> Option<String> {
    let quiet = app::caption(ui.visuals());
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let (text, ink, under) = match named {
            Some(name) => (
                name.to_string(),
                ui.visuals().text_color(),
                format!("{} {}", library.label(), field.value),
            ),
            None => (
                field.value.clone(),
                app::warn(ui.visuals()),
                format!("{} — not in the library", library.label()),
            ),
        };
        ui.label(
            egui::RichText::new(text)
                .font(egui::FontId::monospace(11.5))
                .color(ink),
        )
        .on_hover_text("the file stores the id; only the instrument knows the name");
        ui.label(
            egui::RichText::new(under)
                .font(egui::FontId::proportional(LABEL))
                .color(quiet),
        );
    });
    None
}

/// The unclassified default: a menu over a short list, a knob over a long run.
fn plain(ui: &mut egui::Ui, field: &Field, legal: &[String]) -> Option<String> {
    if legal.len() <= MENU_MAX && !legal.is_empty() {
        return selector(ui, field, legal);
    }
    let Some((min, max)) = contiguous(legal) else {
        return selector(ui, field, legal);
    };
    let value: i64 = field.value.trim_start_matches('+').parse().ok()?;
    knob::ui(ui, &field.path, value, min, max)
        .filter(|moved| *moved != value)
        .map(|moved| moved.to_string())
}

/// A field too wide to enumerate: its stored bits, typed as they are spelled.
fn wide(ui: &mut egui::Ui, field: &Field) -> Option<String> {
    // ⚠️ Half a value is not a value the format would take, so the box commits when it
    // is done rather than per keystroke — and what has been typed waits under the
    // field's own id until then.
    let id = ui.id().with(("wide", field.path.as_str()));
    let held: Option<String> = ui.data(|data| data.get_temp(id));
    let mut text = held.clone().unwrap_or_else(|| field.value.clone());
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .desired_width(150.0)
            .font(egui::FontId::monospace(11.0)),
    );
    let entered = response.ctx.input(|i| i.key_pressed(egui::Key::Enter));
    if response.has_focus() && !entered {
        ui.data_mut(|data| data.insert_temp(id, text));
        return None;
    }
    held.as_ref()?;
    ui.data_mut(|data| data.remove::<String>(id));
    (text.trim() != field.value).then(|| text.trim().to_string())
}

/// The Electro 5 transpose control: a lamp and a number, written together the way the
/// panel's own button writes them.
///
/// ⚠️ Neither field reads on its own. `transpose_enabled` is sticky — the instrument
/// sets it the first time transposition is touched and never clears it — and an
/// untouched program stores `+1` in the value rather than `0`. The instrument ignores the
/// amount while the lamp is dark, and moving the amount is what lights it.
/// Confirmed on hardware.
fn transpose(ui: &mut egui::Ui, state: &State, rows: &[&Field], sets: &mut Sets) {
    /// The panel's own travel, either side of nothing.
    const SEMITONES: i64 = 6;

    let held = |path: &str| rows.iter().find(|field| field.path == path);
    let (Some(lamp), Some(amount)) = (held(TRANSPOSE_ENABLED), held(TRANSPOSE)) else {
        return;
    };
    let on = lamp.value == "true";
    let Some(semitones) = amount.value.trim_start_matches('+').parse::<i64>().ok() else {
        return;
    };
    let edited = state.pending.contains(&lamp.path) || state.pending.contains(&amount.path);

    let mut switched = None;
    let mut moved = None;
    ui.allocate_ui(egui::vec2(120.0, 0.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                switched = led::ui(ui, on, "");
                moved = knob::ui(ui, TRANSPOSE, semitones, -SEMITONES, SEMITONES);
            });
            caption(ui, lamp, edited);
        });
    })
    .response
    .on_hover_text("two fields, one control: the lamp and the semitones move together");

    // Moving the semitones turns the light on, which is what the panel does.
    let (on, semitones) = match (switched, moved) {
        (_, Some(want)) => (true, want),
        (Some(want_on), None) => (want_on, semitones),
        (None, None) => return,
    };
    sets.push((TRANSPOSE_ENABLED.to_string(), on.to_string()));
    sets.push((TRANSPOSE.to_string(), semitones.to_string()));
}

// ---- what the Advanced face reads --------------------------------------------------

/// The rows of the Advanced face's "About this file": a label, a value, and a note.
pub fn about(doc: &Doc<'_>, entity: &LocalEntity) -> Vec<(&'static str, String, String)> {
    let (fields, slots) = doc.tally();
    let (badge, format) = super::header::badge(entity);
    let layout = match doc.shape() {
        Shape::Authored { exhaustive: true } => (
            "authored — exhaustive".to_string(),
            "every field the body declares is placed".to_string(),
        ),
        Shape::Authored { exhaustive: false } => (
            "authored".to_string(),
            format!(
                "{} fields no group names; they show under Also stored",
                doc.unplaced()
            ),
        ),
        Shape::Menus => (
            "menus".to_string(),
            "this app's own table, in the order the instrument's menus run".to_string(),
        ),
        Shape::Flat => (
            "flat, registry order".to_string(),
            "nothing knows how this panel is divided".to_string(),
        ),
    };
    vec![
        ("Format", badge, format),
        (
            "Fields",
            fields.to_string(),
            match slots {
                0 => "no morph slots in this body".to_string(),
                n => format!("{n} of them morph slots"),
            },
        ),
        ("Layout", layout.0, layout.1),
        ("Stored at", super::header::lives(entity), String::new()),
        (
            "Instrument",
            nord_format::accept::Family::of_tag(&entity.tag())
                .map(|family| family.label().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            String::new(),
        ),
    ]
}

/// What a field's kind is called in the Advanced table's own column.
///
/// ⚠️ Exhaustive over [`ControlKind`], so a kind the library adds is named here rather
/// than falling into a catch-all that says nothing.
pub fn kind_word(field: &Field) -> String {
    match field.spec.control {
        ControlKind::Toggle => "toggle".to_string(),
        ControlKind::Selector => "selector".to_string(),
        ControlKind::Knob(unit) => format!("knob {}", unit_word(unit)),
        ControlKind::Bipolar(unit) => format!("bipolar {}", unit_word(unit)),
        ControlKind::Shift(unit) => format!("shift {}", unit_word(unit)),
        ControlKind::Drawbar { bars: 1, rank, .. } => match rank {
            Some(rank) => format!("drawbar {rank}"),
            None => "drawbar".to_string(),
        },
        ControlKind::Drawbar { bars, .. } => format!("{bars} drawbars"),
        ControlKind::Pattern { steps, .. } => format!("{steps} steps"),
        ControlKind::Reference(library) => format!("{} id", library.label()),
        ControlKind::Morph { .. } => "morph".to_string(),
        ControlKind::Number => "number".to_string(),
    }
}

fn unit_word(unit: Unit) -> &'static str {
    match unit {
        Unit::Panel10 => "0-10",
        Unit::Decibels => "dB",
        Unit::Milliseconds => "ms",
        Unit::Hertz => "Hz",
        Unit::Bpm => "BPM",
        Unit::ClockDivision => "division",
        Unit::Semitones => "st",
        Unit::Octaves => "oct",
        Unit::Pan => "pan",
        Unit::None => "",
    }
}

// ---- shared readings ----------------------------------------------------------------

/// A stored word as a field spells it: `0x…`, or decimal.
fn word(value: &str) -> Option<u64> {
    let text = value.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// The range a legal-value list covers, when every value is an integer and none is
/// missing.
///
/// ⚠️ A gapped set is not travel: a knob over it would stop on values the field refuses.
fn contiguous(legal: &[String]) -> Option<(i64, i64)> {
    let mut values = Vec::with_capacity(legal.len());
    for value in legal {
        values.push(value.trim_start_matches('+').parse::<i64>().ok()?);
    }
    let min = *values.iter().min()?;
    let max = *values.iter().max()?;
    (max.checked_sub(min)? + 1 == values.len() as i64 && min < max).then_some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{apply, blank};
    use nord_format::formats::{ne5, ns4};
    use nord_format::{Entity, Program};

    fn electro5() -> (Vec<u8>, Vec<Field>) {
        let entity = Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().unwrap(),
        )));
        let bytes = nord_format::to_bytes(&entity).unwrap();
        let fields = apply(&bytes, &[]).unwrap().0;
        (bytes, fields)
    }

    /// A gapped legal set is not travel: a knob over it would stop on values the field
    /// refuses.
    #[test]
    fn only_a_gapless_run_of_integers_is_travel() {
        let full: Vec<String> = (0..128).map(|n| n.to_string()).collect();
        assert_eq!(contiguous(&full), Some((0, 127)));
        assert_eq!(contiguous(&["0".into(), "1".into(), "9".into()]), None);
        assert_eq!(contiguous(&["Organ".into(), "Piano".into()]), None);
        // A run of one value has no travel.
        assert_eq!(contiguous(&["3".into()]), None);
    }

    /// A Panel10 knob reads in the panel's own `0..10`; a unit whose curve is not
    /// published has no reading at all and names its scale instead.
    #[test]
    fn a_reading_appears_only_where_the_unit_supports_one() {
        assert_eq!(
            reading(Unit::Panel10, false, 96, 0, 127),
            Some("7.6".to_string())
        );
        assert_eq!(reading(Unit::Hertz, false, 71, 0, 127), None);
        assert_eq!(reading(Unit::Milliseconds, false, 12, 0, 127), None);
        // Bipolar reads either side of the slot midpoint, whatever the unit.
        assert_eq!(
            reading(Unit::Decibels, true, 70, 0, 127),
            Some("+6".to_string())
        );
        assert_eq!(scale(Unit::Hertz), Some("Hz scale"));
        assert_eq!(scale(Unit::Panel10), None);
    }

    /// A morph slot's neutral value comes off the library's own constant for its width,
    /// so a slot beside a drawbar and one beside a knob are not read by one number.
    #[test]
    fn a_morph_slots_neutral_follows_the_width_the_library_declares() {
        assert_eq!(neutral(8), Some(127));
        assert_eq!(neutral(5), Some(15));
        assert_eq!(neutral(3), Some(3));
        assert_eq!(neutral(9), None);
    }

    /// The sentence a section's idle line is built out of.
    #[test]
    fn the_idle_line_names_what_is_stored_and_not_played() {
        assert_eq!(listed(&["Vox"]), "Vox");
        assert_eq!(listed(&["Vox", "Farfisa"]), "Vox and Farfisa");
        assert_eq!(listed(&["Vox", "Farfisa", "Pipe"]), "Vox, Farfisa and Pipe");
    }

    /// ⚠️ Both spellings of a drawbar carry the one kind: the Electro 5 packs a whole
    /// registration into one field and the Stage 4 gives each bar its own nibble. The
    /// kind's bar count is what separates them.
    #[test]
    fn a_drawbar_is_a_register_or_a_bar_by_what_its_kind_counts() {
        let (_, electro5) = electro5();
        let packed = electro5
            .iter()
            .find(|field| field.path == "organ_panel.vox_preset1_drawbars")
            .expect("the Vox register");
        assert!(matches!(
            packed.spec.control,
            ControlKind::Drawbar { bars: 9, .. }
        ));
        assert!(ranked(packed).is_none());

        let (stage4, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let bar = stage4
            .iter()
            .find(|field| field.path == "organ_a.drawbar_1")
            .expect("the first bar");
        assert_eq!(ranked(bar).map(|(_, rank)| rank), Some(1));
    }

    /// Nine ranked bars under one prefix are one register; anything short of nine stays
    /// nine cells rather than drawing a register that is not there.
    #[test]
    fn nine_ranked_bars_merge_into_one_register() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let doc = Doc {
            sections: Vec::new(),
            leftovers: Vec::new(),
            idle: Vec::new(),
            shape: Shape::Flat,
            shown: HashSet::new(),
            picks: HashSet::new(),
            morphs: slots_of(&fields),
            fields: 0,
            slots: 0,
        };
        let rows: Vec<&Field> = fields
            .iter()
            .filter(|field| ranked(field).is_some() && field.path.starts_with("organ_a."))
            .take(drawbar_widget::BARS)
            .collect();
        assert_eq!(rows.len(), drawbar_widget::BARS);
        let built = clustered(&rows, &doc);
        assert_eq!(built.len(), 1);
        assert!(matches!(built.first(), Some(Cell::Register(run)) if run.len() == 9));

        let short = clustered(&rows[..4], &doc);
        assert_eq!(short.len(), 4);
        assert!(short.iter().all(|cell| matches!(cell, Cell::One(_))));
    }

    /// A morph target is the value its parameter is driven to, so it rides on that
    /// parameter and is never a cell of its own.
    #[test]
    fn a_morph_slot_is_drawn_on_the_parameter_it_moves() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let morphs = slots_of(&fields);
        let slots = morphs
            .get("organ_a_volume")
            .expect("the volume knob is morphed");
        let named: Vec<&str> = slots
            .iter()
            .filter_map(|slot| slot.map(|field| field.path.as_str()))
            .collect();
        assert_eq!(
            named,
            [
                "organ_a_volume_wheel",
                "organ_a_volume_aftertouch",
                "organ_a_volume_ctrl_pedal",
            ]
        );
        assert!(!morphs.contains_key("organ_a_volume_wheel"));
    }

    /// A slot whose parameter the body does not declare has nothing to ride on, so it
    /// keeps a cell rather than disappearing.
    #[test]
    fn a_slot_with_no_parameter_beside_it_still_gets_a_cell() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let mut morphs = slots_of(&fields);
        morphs.remove("organ_a_volume");
        let doc = Doc {
            sections: Vec::new(),
            leftovers: Vec::new(),
            idle: Vec::new(),
            shape: Shape::Flat,
            shown: HashSet::new(),
            picks: HashSet::new(),
            morphs,
            fields: 0,
            slots: 0,
        };
        let rows: Vec<&Field> = fields
            .iter()
            .filter(|field| field.path == "organ_a_volume_wheel")
            .collect();
        let built = clustered(&rows, &doc);
        assert_eq!(built.len(), 1);
    }

    /// An unrecognised position is offered, because the file holds it and that spelling
    /// is the only way to put it back. Two spellings of off are not offered together.
    #[test]
    fn a_picker_offers_every_position_but_a_second_spelling_of_off() {
        let legal: Vec<String> = ["B3", "B3Bass", "Pipe", "unknown (6)"]
            .iter()
            .map(|value| value.to_string())
            .collect();
        assert_eq!(
            offered("center_panel.organ_type", &legal, "B3"),
            ["B3", "B3Bass", "Pipe", "unknown (6)"]
        );

        let routing: Vec<String> = ["Off", "Unknown", "Lower", "Upper"]
            .iter()
            .map(|value| value.to_string())
            .collect();
        assert_eq!(
            offered("effects_panel.fx1", &routing, "Off"),
            ["Off", "Lower", "Upper"]
        );
        // A file holding it keeps it reachable.
        assert_eq!(
            offered("effects_panel.fx1", &routing, "Unknown"),
            ["Off", "Lower", "Upper", "Unknown"]
        );
        assert!(offerable("some_other_field", "Unknown"));
    }

    /// Every control the Electro 5 view offers comes off the library's layout, and a
    /// group the instrument is not using is named rather than silently absent.
    #[test]
    fn the_electro5_document_is_the_librarys_layout() {
        let (bytes, fields) = electro5();
        let decoded =
            nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).expect("it decodes");
        let doc = of(&decoded, &fields);
        assert_eq!(doc.shape(), Shape::Authored { exhaustive: false });
        let titles: Vec<&str> = doc
            .sections
            .iter()
            .map(|section| section.title.as_str())
            .collect();
        assert!(titles.contains(&"Keyboard & split"), "{titles:?}");
        assert!(titles.contains(&"Organ"), "{titles:?}");
        // A fresh program plays organ on both parts, so piano is state rather than
        // controls — named as idle, never simply gone.
        assert!(!titles.contains(&"Piano"), "{titles:?}");
        assert!(doc.idle.contains(&"Piano"), "{:?}", doc.idle);
        assert!(doc.unplaced() > 0);
    }

    /// The transpose pair is one control, which needs both halves in the same group —
    /// the layout is what puts them there.
    #[test]
    fn the_transpose_pair_stays_in_one_group() {
        let (_, fields) = electro5();
        let resolved = ne5::program::PANEL.resolve(&fields);
        let keyboard = resolved
            .sections
            .iter()
            .find(|section| section.group.title == "Keyboard & split")
            .expect("the keyboard section");
        let paths: Vec<&str> = keyboard
            .fields
            .iter()
            .map(|field| field.path.as_str())
            .collect();
        assert!(paths.contains(&TRANSPOSE_ENABLED));
        assert!(paths.contains(&TRANSPOSE));
    }

    /// A Stage 4 program has a layout as well, and every one of its sections is open —
    /// nothing folds above a field count.
    #[test]
    fn a_stage4_program_opens_every_section_it_has() {
        let bytes = blank::stage4_program();
        let (fields, _) = apply(&bytes, &[]).unwrap();
        let decoded =
            nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).expect("it decodes");
        let doc = of(&decoded, &fields);
        assert!(!doc.sections.is_empty());
        assert!(fields.len() > 800, "{} fields", fields.len());
        let (all, slots) = doc.tally();
        assert_eq!(all, fields.len());
        assert!(slots > 300, "{slots} morph slots");
        assert!(ns4::program::PANEL.resolve(&fields).sections.len() > 1);
    }

    /// Which kind a field is, for a sweep that has to see every one of them drawn.
    fn kind_key(field: &Field) -> &'static str {
        match field.spec.control {
            ControlKind::Toggle => "toggle",
            ControlKind::Selector => "selector",
            ControlKind::Knob(_) => "knob",
            ControlKind::Bipolar(_) => "bipolar",
            ControlKind::Shift(_) => "shift",
            ControlKind::Drawbar { bars: 1, .. } => "bar",
            ControlKind::Drawbar { .. } => "register",
            ControlKind::Pattern { .. } => "pattern",
            ControlKind::Reference(_) => "reference",
            ControlKind::Morph { .. } => "morph",
            ControlKind::Number => "number",
        }
    }

    /// One headless frame with a single control in it, and how many shapes it painted.
    fn drawn(field: &Field) -> usize {
        fn count(shape: &egui::Shape) -> usize {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().map(count).sum(),
                _ => 1,
            }
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        ctx.all_styles_mut(crate::app::metrics);
        let legal = (field.spec.legal)();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 540.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                control(ui, field, &legal, &[], None);
            });
        });
        output
            .shapes
            .iter()
            .map(|clipped| count(&clipped.shape))
            .sum()
    }

    /// ⚠️ Every kind the library declares has a renderer, and every renderer paints. A
    /// kind that fell through would be a control the operator never sees at all — the
    /// field would still be in the file, and nothing on screen would say so.
    #[test]
    fn every_control_kind_the_registry_declares_paints_a_control() {
        let (_, electro5) = electro5();
        let (stage4, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let (stage2, _) = apply(&blank::stage2_program(), &[]).unwrap();

        let mut seen: Vec<&'static str> = Vec::new();
        for field in stage4.iter().chain(&electro5).chain(&stage2) {
            let kind = kind_key(field);
            if seen.contains(&kind) {
                continue;
            }
            seen.push(kind);
            assert!(drawn(field) > 0, "{kind} ({}) painted nothing", field.path);
        }
        seen.sort_unstable();
        assert_eq!(
            seen,
            [
                "bar",
                "bipolar",
                "knob",
                "morph",
                "number",
                "pattern",
                "reference",
                "register",
                "selector",
                "shift",
                "toggle",
            ],
        );

        // A field too wide to enumerate is the eleventh shape: a box its stored bits are
        // typed into, and there is nothing else it could be.
        let wide = stage2
            .iter()
            .find(|field| {
                (field.spec.legal)().is_empty() && matches!(field.spec.control, ControlKind::Number)
            })
            .expect("the Stage 2 declares a wide unclassified field");
        assert!(wide.spec.width > nord_format::fields::ENUMERABLE_BITS);
        assert!(wide.value.starts_with("0x"), "{}", wide.value);
        assert!(drawn(wide) > 0);
    }

    /// A morph lens puts the target under the parameter's own control, and an edit made
    /// there writes the slot rather than the panel value beside it.
    #[test]
    fn an_edit_under_the_lens_writes_the_morph_slot() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let morphs = slots_of(&fields);
        let part = Part {
            field: fields
                .iter()
                .find(|field| field.path == "organ_a_volume")
                .expect("the volume knob"),
            morphs: morphs["organ_a_volume"],
        };
        assert!(
            shown(&part, None).is_none(),
            "the panel writes the panel value"
        );
        assert_eq!(
            shown(&part, Some(0)).map(|field| field.path.as_str()),
            Some("organ_a_volume_wheel"),
        );
        assert_eq!(
            shown(&part, Some(2)).map(|field| field.path.as_str()),
            Some("organ_a_volume_ctrl_pedal"),
        );

        // A parameter nothing morphs keeps its own control, dimmed rather than swapped.
        let bare = Part {
            field: fields
                .iter()
                .find(|field| field.path == "split_enabled")
                .expect("the split switch"),
            morphs: Default::default(),
        };
        assert!(shown(&bare, Some(0)).is_none());
    }

    /// ⚠️ Read from the wrong end a register comes out mirrored, and mirrored looks like
    /// a plausible registration — so the packing order is asked of the field.
    #[test]
    fn a_packed_register_is_read_from_the_end_its_field_names() {
        let stored = drawbar_widget::bars(0x8_8880_0000);
        assert_eq!(stored, [8, 8, 8, 8, 0, 0, 0, 0, 0]);
        assert_eq!(mirrored(stored), [0, 0, 0, 0, 0, 8, 8, 8, 8]);
        assert_eq!(mirrored(mirrored(stored)), stored);

        let (_, electro5) = electro5();
        let register = electro5
            .iter()
            .find(|field| field.path == "organ_panel.vox_preset1_drawbars")
            .expect("the Vox register");
        assert!(matches!(
            register.spec.control,
            ControlKind::Drawbar {
                order: PackedOrder::HighFirst,
                ..
            }
        ));
    }

    /// A body with no layout falls into the sections its paths name, and every field
    /// lands in exactly one of them.
    #[test]
    fn a_body_with_no_layout_falls_into_the_sections_its_paths_name() {
        let titles = |bytes: Vec<u8>| -> Vec<String> {
            let (fields, _) = apply(&bytes, &[]).unwrap();
            let groups = prefixes(&fields);
            assert_eq!(
                fields.len(),
                groups.iter().map(|group| group.rows.len()).sum::<usize>(),
            );
            groups.into_iter().map(|group| group.title).collect()
        };
        let stage4 = titles(blank::stage4_program());
        assert_eq!(stage4.first().map(String::as_str), Some("General"));
        assert!(stage4.contains(&"Organ a".to_string()), "{stage4:?}");

        let stage2 = titles(blank::stage2_program());
        assert!(stage2.contains(&"Slot a — organ".to_string()), "{stage2:?}");
        assert_eq!(titles(blank::stage3_synth()), ["General"]);
    }
}
