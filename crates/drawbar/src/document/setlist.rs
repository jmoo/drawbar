//! The set list document: the four programs an Electro 5 song plays.
//!
//! The file stores only four addresses: no names and no program bytes. A name shown here
//! comes from the attached instrument's scan of that bank or from an asset on this
//! computer that stands on that slot. Where neither answers, the row says so.

use std::collections::HashMap;

use eframe::egui;
use nord_format::cbin::Cbin;
use nord_format::formats::ne5::{program, song, Song};
use nord_format::Entity;
use nord_usb::{Location, ObjectClass};

use super::capability::{facts, Fact};
use super::controls::{self, Sets};
use super::table::{self, Width, NAME_TEXT, PAD};
use crate::app;
use crate::browser::{Item, Kind};
use crate::device::DeviceState;
use crate::icon::{icon, painted, Glyph};
use crate::library::{wanted, Needs};
use crate::strings::{display_name, place, shown};
use crate::workspace::{LocalEntity, Workspace};

/// How many programs an Electro 5 song orders.
const SLOTS: usize = song::PROGRAM_COUNT;

fn song(entity: &Entity) -> Option<&Cbin<Song>> {
    match entity {
        Entity::Song(nord_format::Song::Electro5(song)) => Some(song),
        _ => None,
    }
}

fn song_mut(entity: &mut Entity) -> Option<&mut Cbin<Song>> {
    match entity {
        Entity::Song(nord_format::Song::Electro5(song)) => Some(song),
        _ => None,
    }
}

/// How many programs the set list orders, which stands in for its size: it holds no bytes
/// worth measuring.
pub fn entries(entity: &Entity) -> Option<usize> {
    Some(song(entity)?.programs().len())
}

/// Apply one `path = value`: `slot1 = 2:5`, both numbers as the panel shows them.
fn set(file: &mut Cbin<Song>, path: &str, value: &str) -> Result<(), String> {
    let slot = path
        .strip_prefix("slot")
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| song::Slot::at(n.checked_sub(1)?))
        .ok_or_else(|| format!("unknown field {path:?}"))?;
    let (bank, at) = value
        .split_once(':')
        .ok_or_else(|| format!("{path}: expected BANK:SLOT, got {value:?}"))?;
    let bank: u16 = bank
        .trim()
        .parse()
        .map_err(|_| format!("bad bank {bank:?}"))?;
    let at: u16 = at.trim().parse().map_err(|_| format!("bad slot {at:?}"))?;
    if bank == 0 || at == 0 {
        return Err("banks and slots are numbered from 1, as shown on the instrument".into());
    }
    let target: program::Location = (bank - 1, at - 1)
        .try_into()
        .map_err(|e| format!("{path}: {e}"))?;
    file.set(slot, target);
    Ok(())
}

/// Apply every set to a fresh decode and re-encode, the same all-or-nothing rule
/// the registry bodies follow.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut entity =
        nord_format::from_stream(&mut std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let file = song_mut(&mut entity).ok_or("not an Electro 5 set list")?;
    for (path, value) in sets {
        set(file, path, value)?;
    }
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// Which half of an address a box holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Half {
    Bank,
    Slot,
}

/// What the editor keeps between frames.
///
/// Holds no edits, since a committed address lands on the working copy at once. It keeps
/// the half-typed boxes and whether a reorder has happened, which the footer reports.
#[derive(Default)]
pub struct State {
    boxes: HashMap<(usize, Half), String>,
    reordered: bool,
}

/// The two places a name for an address can come from.
pub struct Catalog<'a> {
    pub device: &'a DeviceState,
    pub workspace: &'a Workspace,
}

/// What stands at one entry's address, and what the app knows about it.
///
/// Every entry has exactly one of these, and each is a claim with a source. No variant
/// stands for an empty entry in the song itself: an Electro 5 song stores four valid
/// addresses and cannot encode "no program".
#[derive(Clone, PartialEq, Eq, Debug)]
enum Stands {
    /// Something names it: the instrument's scan, or an asset here on that slot.
    Resolves,
    /// The instrument has read that bank and the slot holds nothing.
    Vacant,
    /// Nothing has read that bank, so what stands there is not known.
    Unread,
    /// The program is here, and a library it plays has no name.
    Needs { class: ObjectClass, id: u32 },
}

impl Stands {
    fn glyph(&self) -> Glyph {
        match self {
            Stands::Resolves => Glyph::CircleCheck,
            Stands::Vacant => Glyph::CircleAlert,
            Stands::Unread => Glyph::CircleDashed,
            Stands::Needs { .. } => Glyph::Link2Off,
        }
    }

    /// The color for this state. `resolved` is the color of an entry that resolves: the
    /// good ink in the State column, the accent on the program's disc.
    fn ink(&self, visuals: &egui::Visuals, resolved: egui::Color32) -> egui::Color32 {
        match self {
            Stands::Resolves => resolved,
            Stands::Vacant | Stands::Needs { .. } => app::warn(visuals),
            Stands::Unread => app::caption(visuals),
        }
    }

    /// Whether this is something to look at before the set is played.
    fn wants_attention(&self) -> bool {
        matches!(self, Stands::Vacant | Stands::Needs { .. })
    }

    /// Whether the slot has not been read. An unread slot is not a vacant one.
    fn is_unread(&self) -> bool {
        matches!(self, Stands::Unread)
    }

    fn words(&self, at: Location) -> String {
        match self {
            Stands::Resolves => "resolves".to_string(),
            Stands::Vacant => format!("no program at {}", shown(at)),
            Stands::Unread => "not read yet".to_string(),
            Stands::Needs { class, .. } => format!("needs a {}", Kind::from_class(*class).chip()),
        }
    }

    fn hint(&self, at: Location) -> String {
        match self {
            Stands::Resolves => format!(
                "{} is what this entry plays",
                place(ObjectClass::Program, at)
            ),
            Stands::Vacant => {
                "the instrument has read that bank; this slot holds no program".to_string()
            }
            Stands::Unread => format!(
                "{} has not been read, so what is there is unknown",
                place(ObjectClass::Program, at)
            ),
            Stands::Needs { class, id } => format!(
                "the program is there, and it plays a {} the instrument has not named ({id:#010x})",
                Kind::from_class(*class).chip()
            ),
        }
    }
}

/// One entry, as the row draws it.
struct Row {
    /// The address the working copy holds, as the panel numbers it.
    at: Location,
    /// The address the file was last saved with, where it differs.
    was: Option<Location>,
    name: Option<String>,
    /// What a click on the arrow opens, where there is something to open.
    open: Option<Item>,
    stands: Stands,
}

impl Row {
    fn read(
        index: usize,
        file: &Cbin<Song>,
        saved: Option<&Cbin<Song>>,
        seen: &Catalog<'_>,
    ) -> Row {
        let slot = song::Slot::at(index).expect("a set list entry");
        let at = panel(file.get(slot));
        let was = saved
            .map(|saved| panel(saved.get(slot)))
            .filter(|held| *held != at);
        let (name, open, stands) = resolve(at, seen);
        Row {
            at,
            was,
            name,
            open,
            stands,
        }
    }
}

/// The address one-indexed, as the instrument shows it, in the [`Location`] the rest of
/// the app addresses a slot with.
fn panel(at: program::Location) -> Location {
    let (bank, slot) = at.inner();
    Location::from_user(u32::from(bank) + 1, u32::from(slot) + 1)
}

/// What is known about one address.
///
/// ⚠️ The order matters: the attached instrument's scan comes first, and an asset here
/// speaks only for the slot it came off. A file opened from disk stands on no slot and
/// resolves nothing.
fn resolve(at: Location, seen: &Catalog<'_>) -> (Option<String>, Option<Item>, Stands) {
    let slot = Item::Slot {
        class: ObjectClass::Program,
        at,
    };
    match seen.device.slot(ObjectClass::Program, at) {
        Some(Some(info)) => {
            return (
                Some(info.name.trim().to_string()),
                Some(slot),
                Stands::Resolves,
            )
        }
        Some(None) => return (None, None, Stands::Vacant),
        None => {}
    }
    let Some(kept) = stands_on(at, seen.workspace) else {
        return (None, None, Stands::Unread);
    };
    (
        Some(display_name(&kept.name).to_string()),
        Some(Item::Local(kept.id)),
        plays(kept, seen.device),
    )
}

/// The asset on this computer that stands on `at`, where there is one.
fn stands_on(at: Location, workspace: &Workspace) -> Option<&LocalEntity> {
    workspace
        .entities()
        .iter()
        .find(|held| held.spot() == Some((ObjectClass::Program, at)))
}

/// Whether the library a resolved program plays has a name.
///
/// ⚠️ Only while an instrument is attached. With no instrument to ask, an unresolved
/// piano id is normal ([`Needs::Wanted`] means unresolved, not missing), and flagging it
/// would flag every entry of every set list opened from disk.
fn plays(program: &LocalEntity, device: &DeviceState) -> Stands {
    if !device.connected() {
        return Stands::Resolves;
    }
    match wanted(program, device) {
        Needs::Wanted { class, id } => Stands::Needs { class, id },
        Needs::Named { .. } | Needs::Nothing => Stands::Resolves,
    }
}

/// The reading beside the heading: what the whole list amounts to.
///
/// ⚠️ Unread entries never count as trouble: with no instrument attached every entry is
/// unread, and a set list opened from disk does not have four problems.
fn health(rows: &[Row]) -> (String, super::Ink) {
    let count = |wanted: fn(&Stands) -> bool| rows.iter().filter(|row| wanted(&row.stands)).count();
    match (count(Stands::wants_attention), count(Stands::is_unread)) {
        (0, 0) => ("every entry resolves".to_string(), super::Ink::Good),
        (0, 1) => ("1 entry not read yet".to_string(), super::Ink::Quiet),
        (0, unread) => (format!("{unread} entries not read yet"), super::Ink::Quiet),
        (1, _) => ("1 entry needs attention".to_string(), super::Ink::Warn),
        (bad, _) => (format!("{bad} entries need attention"), super::Ink::Warn),
    }
}

/// What the header claims about a saved set list: the entries that need attention, or
/// `None` to leave the strip's usual text.
pub fn claim(entity: &Entity, seen: &Catalog<'_>) -> Option<super::StateLine> {
    let rows = read(song(entity)?, None, seen);
    let (words, ink) = health(&rows);
    (ink == super::Ink::Warn).then(|| super::StateLine {
        words,
        ink,
        hint: "an entry points at a slot with no program, or at a program playing a \
               library the instrument has not named"
            .to_string(),
    })
}

fn read(file: &Cbin<Song>, saved: Option<&Cbin<Song>>, seen: &Catalog<'_>) -> Vec<Row> {
    (0..SLOTS)
        .map(|index| Row::read(index, file, saved, seen))
        .collect()
}

/// The sentence under the rows: where names come from, and what sending the list leaves
/// out.
fn foot(reordered: bool) -> String {
    let mut words = "Names come from the library on this computer and the attached \
                     instrument; the file stores only bank:slot. Sending the list does not \
                     send the programs it names; queue those separately if they differ."
        .to_string();
    if reordered {
        words.push_str(" Reordering rewrites the slots between the two positions.");
    }
    words
}

/// The `slotN = bank:slot` sets that move the entry at `from` to `to`.
///
/// The file has no order apart from its slots: slot 1 is always the first program, so
/// moving an entry rewrites every address between the two ends.
fn reorder(addresses: &[Location], from: usize, to: usize) -> Sets {
    let mut moved: Vec<Location> = addresses.to_vec();
    if from >= moved.len() || to >= moved.len() || from == to {
        return Sets::new();
    }
    let carried = moved.remove(from);
    moved.insert(to, carried);
    moved
        .iter()
        .enumerate()
        .filter(|(index, at)| addresses[*index] != **at)
        .map(|(index, at)| (format!("slot{}", index + 1), shown(*at)))
        .collect()
}

/// The index of the row being dragged.
#[derive(Clone, Copy)]
struct Carried(usize);

const ROW_H: f32 = 30.0;
const BOX_W: f32 = 34.0;
const BOX_H: f32 = 20.0;
const DOT: f32 = 6.0;
const SUB_TEXT: f32 = 10.0;
const STATE_TEXT: f32 = 10.5;
const MONO: f32 = 11.0;
const GLYPH: f32 = 13.0;
const MARK: f32 = 11.0;

/// The seven columns: the drag handle, the place in the set, the kind, the name, the
/// address, what stands there, and the open arrow.
const COLUMNS: [Width; 7] = [
    Width::Fixed(22.0),
    Width::Fixed(34.0),
    Width::Fixed(22.0),
    Width::Share(1.6),
    Width::Fixed(118.0),
    Width::Share(1.0),
    Width::Fixed(22.0),
];

const HEADS: [&str; 7] = ["", "#", "", "Plays", "Bank : slot", "State", ""];

/// The four programs the set list plays, in the order it plays them.
///
/// Returns the item a click on an entry's arrow asked to open.
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut State,
    entity: &LocalEntity,
    seen: &Catalog<'_>,
    sets: &mut Sets,
) -> Option<Item> {
    let file = song(entity.entity.as_ref()?)?;
    let saved = nord_format::from_stream(&mut std::io::Cursor::new(&entity.saved.bytes)).ok();
    let rows = read(file, saved.as_ref().and_then(song), seen);
    let (reading, ink) = health(&rows);
    let tint = ink.color(ui.visuals());
    controls::heading(
        ui,
        "The four programs this set list plays",
        "drag to reorder · type a bank and slot as the panel shows them, numbered from 1",
        Some((&reading, tint)),
    );
    table::heads(ui, COLUMNS, HEADS, &[]);

    let mut opened = None;
    let mut moved = None;
    for (index, row) in rows.iter().enumerate() {
        let (dropped, open) = entry(ui, state, index, row, sets);
        opened = opened.or(open);
        moved = moved.or(dropped);
    }
    if let Some((from, to)) = moved {
        let addresses: Vec<Location> = rows.iter().map(|row| row.at).collect();
        let shuffled = reorder(&addresses, from, to);
        if !shuffled.is_empty() {
            state.reordered = true;
            sets.extend(shuffled);
        }
    }
    footer(ui, state.reordered);
    opened
}

/// One entry. Returns the move a drop asked for and the item an arrow asked to open.
fn entry(
    ui: &mut egui::Ui,
    state: &mut State,
    index: usize,
    row: &Row,
    sets: &mut Sets,
) -> (Option<(usize, usize)>, Option<Item>) {
    let visuals = ui.visuals().clone();
    // A dragged entry drops onto the row; the handle picks one up, and the boxes are the
    // only click targets.
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    let cells = table::columns(rect, COLUMNS);
    let quiet = app::caption(&visuals);
    let hairline = egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color);
    let carried = response.dnd_hover_payload::<Carried>();
    {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
        }
        painter.hline(rect.x_range(), rect.bottom() - 0.5, hairline);
        if carried.is_some_and(|held| held.0 != index) {
            painter.hline(
                rect.x_range(),
                rect.top() + 1.0,
                egui::Stroke::new(2.0_f32, app::accent(&visuals)),
            );
        }
    }

    let grip = ui.interact(
        cell_rect(cells[0], rect, GLYPH),
        ui.id().with(("grip", index)),
        egui::Sense::drag(),
    );
    if grip.dragged() {
        egui::DragAndDrop::set_payload(ui.ctx(), Carried(index));
    }
    painted(
        ui,
        Glyph::GripVertical,
        cell_rect(cells[0], rect, GLYPH),
        quiet,
    );
    grip.on_hover_text("drag to move this entry within the set");

    let painter = ui.painter().clone();
    let number = painter.layout_no_wrap(
        format!("{:02}", index + 1),
        egui::FontId::monospace(MONO),
        quiet,
    );
    painter.galley(
        egui::pos2(cells[1].0, rect.center().y - number.size().y / 2.0),
        number,
        quiet,
    );
    painted(
        ui,
        Glyph::Disc3,
        cell_rect(cells[2], rect, GLYPH),
        row.stands.ink(&visuals, app::accent(&visuals)),
    );
    plays_cell(ui, cells[3], rect, row);
    address(ui, state, index, row, cells[4], rect, sets);
    state_cell(ui, index, cells[5], rect, row);
    let open = arrow(ui, cells[6], rect, index, row);

    let dropped = response
        .dnd_release_payload::<Carried>()
        .map(|held| (held.0, index))
        .filter(|(from, to)| from != to);
    (dropped, open)
}

/// A glyph's box inside one cell, at the cell's left edge.
fn cell_rect((left, _): (f32, f32), rect: egui::Rect, size: f32) -> egui::Rect {
    egui::Rect::from_center_size(
        egui::pos2(left + size / 2.0, rect.center().y),
        egui::Vec2::splat(size),
    )
}

/// The name the entry resolves to, with a note beside it for a vacant slot.
fn plays_cell(ui: &mut egui::Ui, (left, width): (f32, f32), rect: egui::Rect, row: &Row) {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    // Italic, so an unnamed row does not read as a program called “unresolved”.
    let (text, format) = match &row.name {
        Some(name) => (
            name.clone(),
            egui::TextFormat::simple(
                egui::FontId::new(NAME_TEXT, app::bold()),
                visuals.text_color(),
            ),
        ),
        None => (
            "unresolved".to_string(),
            egui::TextFormat {
                italics: true,
                ..egui::TextFormat::simple(
                    egui::FontId::proportional(NAME_TEXT),
                    app::caption(&visuals),
                )
            },
        ),
    };
    let ink = format.color;
    // Only a vacant slot gets a note; the State column covers the rest.
    let sub = match row.stands {
        Stands::Vacant => "nothing plays here",
        _ => "",
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&text, 0.0, format);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let name = painter.layout_job(job);
    let top = rect.center().y - name.size().y / 2.0;
    painter.galley(egui::pos2(left, top), name.clone(), ink);
    if sub.is_empty() {
        return;
    }
    let room = width - name.size().x - 8.0;
    if room <= 0.0 {
        return;
    }
    let quiet = app::caption(&visuals);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        sub,
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(SUB_TEXT), quiet),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(
            left + name.size().x + 8.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        quiet,
    );
}

/// The two boxes, the colon between them, and a dot when the address differs from the
/// one the file was saved with.
fn address(
    ui: &mut egui::Ui,
    state: &mut State,
    index: usize,
    row: &Row,
    (left, _): (f32, f32),
    rect: egui::Rect,
    sets: &mut Sets,
) {
    let visuals = ui.visuals().clone();
    let quiet = app::caption(&visuals);
    let bank = half(ui, state, index, Half::Bank, row.at.user_bank(), left, rect);
    let colon = left + BOX_W + 4.0;
    let painter = ui.painter().clone();
    let galley = painter.layout_no_wrap(":".to_string(), egui::FontId::monospace(MONO), quiet);
    painter.galley(
        egui::pos2(colon, rect.center().y - galley.size().y / 2.0),
        galley,
        quiet,
    );
    let slot = half(
        ui,
        state,
        index,
        Half::Slot,
        row.at.user_slot(),
        colon + 8.0,
        rect,
    );
    if bank.committed || slot.committed {
        sets.push((
            format!("slot{}", index + 1),
            format!("{}:{}", bank.text, slot.text),
        ));
    }
    if let Some(was) = row.was {
        let dot = egui::Rect::from_center_size(
            egui::pos2(colon + 8.0 + BOX_W + 9.0, rect.center().y),
            egui::Vec2::splat(DOT),
        );
        ui.painter()
            .circle_filled(dot.center(), DOT / 2.0, app::warn(&visuals));
        ui.interact(dot, ui.id().with(("was", index)), egui::Sense::hover())
            .on_hover_text(format!("was {}", shown(was)));
    }
}

/// What one box holds, and whether leaving it committed the value.
struct Typed {
    text: String,
    committed: bool,
}

/// One half of an address.
///
/// ⚠️ A box without focus is reset to the file's value. Otherwise it would keep showing
/// what was last typed after a reorder or a revert changed the address.
fn half(
    ui: &mut egui::Ui,
    state: &mut State,
    index: usize,
    which: Half,
    value: u64,
    left: f32,
    rect: egui::Rect,
) -> Typed {
    let stored = value.to_string();
    let held = state
        .boxes
        .entry((index, which))
        .or_insert_with(|| stored.clone());
    let box_rect = egui::Rect::from_min_size(
        egui::pos2(left, rect.center().y - BOX_H / 2.0),
        egui::vec2(BOX_W, BOX_H),
    );
    let response = ui.put(
        box_rect,
        egui::TextEdit::singleline(held)
            .font(egui::FontId::monospace(MONO))
            .margin(egui::Margin::symmetric(4, 2)),
    );
    let typed = Typed {
        text: held.trim().to_string(),
        committed: response.lost_focus() && held.trim() != stored,
    };
    if !response.has_focus() {
        *held = stored;
    }
    typed
}

fn state_cell(
    ui: &mut egui::Ui,
    index: usize,
    (left, width): (f32, f32),
    rect: egui::Rect,
    row: &Row,
) {
    let ink = row.stands.ink(ui.visuals(), app::good(ui.visuals()));
    painted(
        ui,
        row.stands.glyph(),
        egui::Rect::from_center_size(
            egui::pos2(left + MARK / 2.0, rect.center().y),
            egui::Vec2::splat(MARK),
        ),
        ink,
    );
    let painter = ui.painter().clone();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &row.stands.words(row.at),
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(STATE_TEXT), ink),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((width - MARK - 5.0).max(0.0));
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(left + MARK + 5.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    let cell = egui::Rect::from_min_size(
        egui::pos2(left, rect.top()),
        egui::vec2(width.max(1.0), rect.height()),
    );
    ui.interact(cell, ui.id().with(("stands", index)), egui::Sense::hover())
        .on_hover_text(row.stands.hint(row.at));
}

/// The arrow that opens the program an entry names.
fn arrow(
    ui: &mut egui::Ui,
    (left, width): (f32, f32),
    rect: egui::Rect,
    index: usize,
    row: &Row,
) -> Option<Item> {
    let box_ = egui::Rect::from_center_size(
        egui::pos2(left + width - MARK / 2.0, rect.center().y),
        egui::Vec2::splat(MARK + 1.0),
    );
    let Some(item) = row.open else {
        painted(ui, Glyph::ArrowUpRight, box_, app::unlit(ui.visuals()));
        ui.interact(box_, ui.id().with(("open", index)), egui::Sense::hover())
            .on_hover_text("nothing here names a program to open");
        return None;
    };
    let response = ui.interact(box_, ui.id().with(("open", index)), egui::Sense::click());
    let ink = match response.hovered() {
        true => ui.visuals().text_color(),
        false => app::caption(ui.visuals()),
    };
    painted(ui, Glyph::ArrowUpRight, box_, ink);
    response
        .on_hover_text("open this program")
        .clicked()
        .then_some(item)
}

fn footer(ui: &mut egui::Ui, reordered: bool) {
    let quiet = app::caption(ui.visuals());
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        ui.add_space(PAD);
        ui.spacing_mut().item_spacing.x = 6.0;
        icon(ui, Glyph::Info, MARK, quiet);
        ui.add(
            egui::Label::new(
                egui::RichText::new(foot(reordered))
                    .size(STATE_TEXT)
                    .color(quiet),
            )
            .wrap(),
        );
    });
}

/// The Advanced face: the four addresses as the body holds them.
pub fn stored(ui: &mut egui::Ui, entity: &Entity) {
    let Some(file) = song(entity) else {
        return;
    };
    controls::heading(
        ui,
        "Slots as stored",
        "the body's four addresses, one-indexed as the panel numbers them",
        Some((&format!("{SLOTS} slots"), app::caption(ui.visuals()))),
    );
    const PATHS: [&str; SLOTS] = ["slot1", "slot2", "slot3", "slot4"];
    let rows: Vec<Fact> = PATHS
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let at = file.get(song::Slot::at(index).expect("a set list entry"));
            Fact {
                key: path,
                value: format!("{} · {:#05x}", shown(panel(at)), at.as_u16()),
                note: "bank:slot, and the word the body holds",
            }
        })
        .collect();
    facts(ui, &rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::ne5;
    use nord_usb::ObjectClass;

    use crate::device::Device;
    use crate::log::Log;
    use crate::workspace::{Fresh, Origin};

    fn set_list() -> Vec<u8> {
        let song = ne5::song::new(
            (0, 0).try_into().unwrap(),
            ne5::song::DEFAULT_VERSION,
            [(0, 0).try_into().unwrap(); 4],
        )
        .unwrap();
        nord_format::to_bytes(&nord_format::Entity::Song(nord_format::Song::Electro5(
            song,
        )))
        .unwrap()
    }

    fn at(bank: u32, slot: u32) -> Location {
        Location::from_user(bank, slot)
    }

    /// A slot is set as the panel spells it, and the result decodes and round-trips.
    #[test]
    fn a_slot_edit_lands_and_round_trips() {
        let bytes = set_list();
        let out = apply(&bytes, &[("slot2".into(), "3:14".into())]).unwrap();
        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&out)).unwrap();
        let file = song(&entity).unwrap();
        assert_eq!(file.get(song::Slot::B).inner(), (2, 13));
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), out);
    }

    /// An address the bank map cannot hold is refused before anything is encoded.
    #[test]
    fn an_impossible_address_is_refused() {
        let bytes = set_list();
        for bad in ["9:1", "1:51", "0:1", "nonsense"] {
            assert!(
                apply(&bytes, &[("slot1".into(), bad.into())]).is_err(),
                "{bad}"
            );
        }
        assert!(apply(&bytes, &[("slot5".into(), "1:1".into())]).is_err());
    }

    #[test]
    fn a_reorder_rewrites_every_slot_between_the_two_ends() {
        let held = [at(1, 1), at(1, 2), at(1, 3), at(1, 4)];

        let up = reorder(&held, 3, 1);
        assert_eq!(
            up,
            [
                ("slot2".to_string(), "1:4".to_string()),
                ("slot3".to_string(), "1:2".to_string()),
                ("slot4".to_string(), "1:3".to_string()),
            ],
            "slot 1 was above the move and is left alone"
        );

        let down = reorder(&held, 0, 2);
        assert_eq!(
            down,
            [
                ("slot1".to_string(), "1:2".to_string()),
                ("slot2".to_string(), "1:3".to_string()),
                ("slot3".to_string(), "1:1".to_string()),
            ],
            "slot 4 was below the move and is left alone"
        );

        assert!(reorder(&held, 2, 2).is_empty(), "a drop where it was");
        assert!(reorder(&held, 0, 9).is_empty(), "a drop off the end");
    }

    /// Every set a reorder produces is one the format accepts, so a drag can never
    /// leave the file half written.
    #[test]
    fn the_sets_a_reorder_produces_are_accepted_by_the_format() {
        let bytes = apply(
            &set_list(),
            &[
                ("slot1".into(), "1:1".into()),
                ("slot2".into(), "2:2".into()),
                ("slot3".into(), "3:3".into()),
                ("slot4".into(), "4:4".into()),
            ],
        )
        .unwrap();
        let held = [at(1, 1), at(2, 2), at(3, 3), at(4, 4)];
        let moved = apply(&bytes, &reorder(&held, 0, 3)).unwrap();
        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&moved)).unwrap();
        let file = song(&entity).unwrap();
        assert_eq!(
            file.programs().map(panel),
            [at(2, 2), at(3, 3), at(4, 4), at(1, 1)]
        );
    }

    #[test]
    fn the_footer_names_its_sources_and_mentions_a_reorder_once_made() {
        assert!(foot(false).contains("the file stores only bank:slot"));
        assert!(!foot(false).contains("Reordering"));
        assert!(foot(true).ends_with("Reordering rewrites the slots between the two positions."));
    }

    /// One set list painted on its own, with whatever the instrument and this computer
    /// have been told to hold.
    struct Shown {
        ctx: egui::Context,
        workspace: Workspace,
        device: Device,
        log: Log,
        state: State,
        id: u64,
    }

    impl Shown {
        fn new() -> Shown {
            let ctx = egui::Context::default();
            ctx.all_styles_mut(crate::app::metrics);
            ctx.set_fonts(crate::app::fonts());
            let mut workspace = Workspace::new(ctx.clone());
            let mut log = Log::default();
            let id = workspace.ingest(
                "Blue Room.ne5t".into(),
                Origin::File("Blue Room.ne5t".into()),
                Fresh::SetList.bytes().unwrap(),
                &mut log,
            );
            Shown {
                device: Device::new(ctx.clone()),
                ctx,
                workspace,
                log,
                state: State::default(),
                id,
            }
        }

        /// An asset on this computer that stands on a slot, which is the other place a
        /// name can come from.
        fn kept(&mut self, at: Location, name: &str, bytes: Vec<u8>) {
            self.workspace.ingest(
                name.to_string(),
                Origin::Device {
                    class: ObjectClass::Program,
                    at,
                },
                bytes,
                &mut self.log,
            );
        }

        /// One frame: every word it painted and where, and the sets it asked for.
        fn frame(&mut self, events: Vec<egui::Event>) -> (Vec<(String, egui::Rect)>, Sets) {
            let mut sets = Sets::new();
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            };
            let state = &mut self.state;
            let workspace = &self.workspace;
            let device = &self.device.state;
            let id = self.id;
            let output = self.ctx.clone().run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let entity = workspace.get(id).expect("it is still open");
                    let seen = Catalog { device, workspace };
                    super::ui(ui, state, entity, &seen, &mut sets);
                });
            });
            let mut said = Vec::new();
            for clipped in &output.shapes {
                words(&clipped.shape, &mut said);
            }
            (said, sets)
        }

        fn settle(&mut self) -> Vec<String> {
            said(self.frame(Vec::new()).0)
        }

        /// Where one word was painted, out of a frame that has already been drawn.
        fn where_(&mut self, word: &str) -> egui::Rect {
            self.frame(Vec::new())
                .0
                .into_iter()
                .find(|(held, _)| held == word)
                .unwrap_or_else(|| panic!("{word} was never painted"))
                .1
        }
    }

    fn words(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => into.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| words(shape, into)),
            _ => {}
        }
    }

    fn said(painted: Vec<(String, egui::Rect)>) -> Vec<String> {
        painted.into_iter().map(|(word, _)| word).collect()
    }

    #[test]
    fn the_four_entries_paint_their_places_in_the_set() {
        let said = Shown::new().settle();
        for index in ["01", "02", "03", "04"] {
            assert!(said.iter().any(|word| word == index), "{index}: {said:?}");
        }
    }

    /// Where neither source names a slot, the row says whether it is unread or vacant.
    #[test]
    fn a_name_comes_from_the_instrument_or_from_an_asset_here() {
        let mut shown = Shown::new();
        let said = shown.settle();
        assert!(said.iter().any(|word| word == "not read yet"), "{said:?}");
        assert!(
            said.iter().any(|word| word == "4 entries not read yet"),
            "nothing read is not four problems: {said:?}"
        );

        // Bank 1 read: the first slot holds a program, the second is vacant.
        shown.device.pretend_scanned(
            ObjectClass::Program,
            1,
            &["Africa Split", "", "Gospel Perc"],
        );
        let said = shown.settle();
        assert!(said.iter().any(|word| word == "Africa Split"), "{said:?}");
        assert!(said.iter().any(|word| word == "resolves"), "{said:?}");
        assert!(
            said.iter().any(|word| word == "no program at 1:2"),
            "a scanned vacant slot is reported: {said:?}"
        );
        assert!(
            said.iter().any(|word| word == "not read yet"),
            "slot 1:4 is past what was scanned: {said:?}"
        );
        assert!(
            said.iter().any(|word| word == "1 entry needs attention"),
            "{said:?}"
        );

        // With nothing scanned, an asset here that stands on the slot names it.
        let mut alone = Shown::new();
        let program = alone
            .workspace
            .create(Fresh::Program, &mut alone.log)
            .expect("a fresh program");
        let bytes = alone.workspace.get(program).unwrap().bytes.clone();
        alone.kept(Location::from_user(1, 3), "Whiter Shade.ne5p", bytes);
        let said = alone.settle();
        assert!(said.iter().any(|word| word == "Whiter Shade"), "{said:?}");
    }

    #[test]
    fn an_unnamed_library_is_only_reported_while_an_instrument_is_attached() {
        let mut shown = Shown::new();
        let program = shown
            .workspace
            .create(Fresh::Program, &mut shown.log)
            .expect("a fresh program");
        let bytes = shown.workspace.get(program).unwrap().bytes.clone();
        let (_, plays_a_piano) =
            crate::fields::apply(&bytes, &[("piano_panel.id".into(), "5".into())])
                .expect("the id is a field");
        shown.kept(
            Location::from_user(1, 1),
            "Blue Swirl EP.ne5p",
            plays_a_piano,
        );

        let said = shown.settle();
        assert!(
            !said.iter().any(|word| word.starts_with("needs a")),
            "nothing has been asked, so nothing is missing: {said:?}"
        );

        shown.device.pretend_attached();
        let said = shown.settle();
        assert!(said.iter().any(|word| word == "needs a piano"), "{said:?}");
        assert!(
            said.iter().any(|word| word == "1 entry needs attention"),
            "{said:?}"
        );
    }

    /// Typing an address commits it to the file, spelled the way the panel spells it.
    #[test]
    fn typing_an_address_writes_the_slot_it_belongs_to() {
        let mut shown = Shown::new();
        shown.settle();
        // The first row's bank box ends where the colon between the two boxes begins,
        // and a click at the end of a box puts the caret after what it holds.
        let colon = shown.where_(":");
        let at = egui::pos2(colon.left() - 6.0, colon.center().y);
        shown.frame(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        shown.frame(vec![
            egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("3".to_string()),
        ]);
        let (_, sets) = shown.frame(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(sets, [("slot1".to_string(), "3:1".to_string())]);
        assert!(
            apply(&Fresh::SetList.bytes().unwrap(), &sets).is_ok(),
            "the format accepts what was typed"
        );
    }

    /// Dragging the first entry's handle onto the last entry moves it there, and the
    /// footer says the slots between were rewritten.
    #[test]
    fn dragging_an_entry_onto_another_rewrites_the_slots_between_them() {
        let mut shown = Shown::new();
        shown.settle();
        let first = shown.where_("01");
        let last = shown.where_("04");
        // The handle stands one cell to the left of the number it belongs to.
        let grip = egui::pos2(first.left() - 25.0, first.center().y);
        let onto = egui::pos2(last.left(), last.center().y);

        shown.frame(vec![
            egui::Event::PointerMoved(grip),
            egui::Event::PointerButton {
                pos: grip,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        shown.frame(vec![egui::Event::PointerMoved(onto)]);
        let (_, sets) = shown.frame(vec![
            egui::Event::PointerMoved(onto),
            egui::Event::PointerButton {
                pos: onto,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(
            sets,
            [
                ("slot1".to_string(), "1:2".to_string()),
                ("slot2".to_string(), "1:3".to_string()),
                ("slot3".to_string(), "1:4".to_string()),
                ("slot4".to_string(), "1:1".to_string()),
            ]
        );
        assert!(
            shown
                .settle()
                .iter()
                .any(|word| word.ends_with("rewrites the slots between the two positions.")),
            "the sentence says what the drag did"
        );
    }

    /// The header's hint covers both states the reading counts: a slot read and found
    /// empty, and a program here playing a library nothing has named.
    #[test]
    fn the_header_claim_covers_both_states_it_counts() {
        let mut shown = Shown::new();
        shown
            .device
            .pretend_scanned(ObjectClass::Program, 1, &["Africa Split", ""]);
        let entity = shown.workspace.get(shown.id).expect("it is still open");
        let seen = Catalog {
            device: &shown.device.state,
            workspace: &shown.workspace,
        };
        let claim = claim(entity.entity.as_ref().expect("a set list decodes"), &seen)
            .expect("a vacant slot is trouble");

        assert_eq!(claim.words, "1 entry needs attention");
        assert!(
            claim.hint.contains("no program"),
            "the vacant slot: {}",
            claim.hint
        );
        assert!(
            claim.hint.contains("has not named"),
            "the unnamed library: {}",
            claim.hint
        );
    }

    #[test]
    fn every_state_says_its_own_thing_and_only_trouble_counts() {
        let where_ = at(7, 4);
        let all = [
            Stands::Resolves,
            Stands::Vacant,
            Stands::Unread,
            Stands::Needs {
                class: ObjectClass::Piano,
                id: 9,
            },
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.words(where_), b.words(where_));
                assert_ne!(a.hint(where_), b.hint(where_));
            }
        }
        assert_eq!(Stands::Vacant.words(where_), "no program at 7:4");
        assert!(!Stands::Unread.wants_attention(), "not a claim of trouble");
        assert!(Stands::Vacant.wants_attention());
    }
}
