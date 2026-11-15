//! The right dock: the cards of the document in front, the selection, and the instrument
//! while one is attached.
//!
//! Each part is a card. A document draws its own cards, such as a program's Morph card,
//! while the dock follows it. The selection cards describe that document, or else the rows
//! selected in the browser. Room and Info hold what an attached instrument reports, and are
//! absent without one.
//! Nothing here asks for data the rest of the app does not already have.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui;

use nord_usb::wire::Dependency;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, bold, caption, good, tint, ui as ui_text, warn};
use crate::browser::{tag_items, Act, Browser, Bulk, Item, Kind, Offer};
use crate::device::link_offers;
use crate::device::{fit, occupancy, Device, DeviceState, Fit};
use crate::document::Click;
use crate::icon::{icon, painted, Glyph};
use crate::library::{mark_ink, mark_words, row_of, Mark, Row, Where};
use crate::panel::{cut, signal_pill, tonal_button, CARD_RADIUS, GAP, GUTTER};
use crate::plays::Catalog;
use crate::queue::{Diff, Queue};
use crate::room;
use crate::shell::Shell;
use crate::strings::{kind_word, place, shown};
use crate::tags::Tags;
use crate::workspace::Workspace;

/// The size of a glyph in a line, and of the smaller one on a tag chip.
const GLYPH: f32 = 13.0;
const TAG: f32 = 11.0;

/// The size of monospace readouts: a meter's figures, an id.
const MONO: f32 = 10.5;

/// The width of a fact's label column, so the values line up.
const FACT: f32 = 52.0;

/// The height of a line that holds a pill: a library a slot needs.
const LINE: f32 = 28.0;

/// The space under a meter, before the next folder's name.
const AFTER_METER: f32 = 7.0;

/// A card's header: its height, the room at each end, the gap between its parts, and the
/// sizes of its glyph, title, and badge.
const HEAD: f32 = 34.0;
const HEAD_SIDE: f32 = 10.0;
const HEAD_GAP: f32 = 7.0;
const HEAD_GLYPH: f32 = 14.0;
const TITLE: f32 = 12.5;
const BADGE: f32 = 11.5;

/// The size of the triangle at the right end of a card that collapses.
const CHEVRON: f32 = 12.0;

/// The space around a card's body, inside the card.
const BODY: egui::Margin = egui::Margin {
    left: 12,
    right: 12,
    top: 2,
    bottom: 12,
};

/// The document's cards, the selection's, then the instrument's while one is attached.
///
/// `front` is the document the dock follows, if any, which the selection's cards describe
/// alone in place of the browser's selection; a view of a slot is not on this computer, so they describe
/// nothing for it. `cards` draws the document's own cards and says whether it drew any;
/// while it does, the Selection and Tags cards fold beneath them.
#[allow(clippy::too_many_arguments)]
pub fn ui(
    ui: &mut egui::Ui,
    front: Option<u64>,
    cards: impl FnOnce(&mut egui::Ui) -> bool,
    shell: &mut Shell,
    browser: &mut Browser,
    workspace: &Workspace,
    device: &Device,
    queue: &Queue,
) -> Vec<Act> {
    let mut acts = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("inspector")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = GUTTER;
            let folded = cards(ui).then_some((&mut shell.selection_open, &mut shell.tags_open));
            let shown = shell.picked.now(front, browser, workspace, device, queue);
            selection(ui, shown, browser, device, folded, &mut acts);
            if !device.state.connected() {
                return;
            }
            room_card(ui, &mut shell.room_open, workspace, device, queue);
            let info = Head {
                glyph: Glyph::Info,
                title: "Info",
                badge: None,
                open: Some(&mut shell.info_open),
            };
            card(ui, info, |ui| crate::browser::about(ui, device));
        });
    acts
}

/// What a card's header shows.
pub(crate) struct Head<'a> {
    glyph: Glyph,
    title: &'a str,
    /// A short word at the right end, in its own color.
    badge: Option<(String, egui::Color32)>,
    /// Whether the card is open, for a card that collapses; `None` for one that does not.
    open: Option<&'a mut bool>,
}

impl<'a> Head<'a> {
    /// The header of a card that is always open and has no badge.
    pub(crate) fn fixed(glyph: Glyph, title: &'a str) -> Head<'a> {
        Head {
            glyph,
            title,
            badge: None,
            open: None,
        }
    }
}

/// A card of the dock, on the canvas: a header, and under it the body while the card is
/// open.
pub(crate) fn card(ui: &mut egui::Ui, head: Head, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .corner_radius(CARD_RADIUS)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            if !header(ui, head) {
                return;
            }
            egui::Frame::new().inner_margin(BODY).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = egui::vec2(GAP, 4.0);
                body(ui);
            });
        });
}

/// A card's header: its glyph and title, then the badge and the triangle at the right
/// end. A click anywhere on a collapsing card's header opens or shuts it.
///
/// Returns whether the body should be drawn.
fn header(ui: &mut egui::Ui, head: Head) -> bool {
    let sense = match head.open {
        Some(_) => egui::Sense::click(),
        None => egui::Sense::hover(),
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), HEAD), sense);
    let open = head.open.map(|open| {
        if response.clicked() {
            *open = !*open;
        }
        *open
    });
    let visuals = ui.visuals();
    let quiet = match open.is_some() && response.hovered() {
        true => visuals.widgets.hovered.fg_stroke.color,
        false => caption(visuals),
    };
    let painter = ui.painter();
    let middle = rect.center().y;
    let square = |left: f32, size: f32| {
        egui::Rect::from_min_size(
            egui::pos2(left, middle - size / 2.0),
            egui::Vec2::splat(size),
        )
    };

    let left = rect.left() + HEAD_SIDE;
    painted(ui, head.glyph, square(left, HEAD_GLYPH), quiet);
    let left = left + HEAD_GLYPH + HEAD_GAP;

    let mut right = rect.right() - HEAD_SIDE;
    if let Some(open) = open {
        let glyph = match open {
            true => Glyph::ChevronDown,
            false => Glyph::ChevronRight,
        };
        right -= CHEVRON;
        painted(ui, glyph, square(right, CHEVRON), quiet);
        right -= HEAD_GAP;
    }
    if let Some((said, ink)) = head.badge {
        let badge = painter.layout_no_wrap(said, egui::FontId::proportional(BADGE), ink);
        right -= badge.size().x;
        let top = middle - badge.size().y / 2.0;
        painter.galley(egui::pos2(right, top), badge, egui::Color32::PLACEHOLDER);
        right -= HEAD_GAP;
    }
    cut(
        painter,
        left,
        middle,
        right - left,
        head.title,
        egui::TextFormat::simple(
            egui::FontId::new(TITLE, bold()),
            visuals.widgets.inactive.fg_stroke.color,
        ),
    );

    if let Some(open) = open {
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, head.title)
        });
    }
    open.unwrap_or(true)
}

/// A selection of more than this many rows is summed up rather than described row by
/// row, and only this many of its assets are read for it.
pub const MANY: usize = 64;

/// What the selection's cards show, taken again only when what they describe, or
/// something it reads, changes.
#[derive(Default)]
pub struct Picked {
    shown: Option<Shown>,
    /// How many times what the cards show has been taken.
    #[cfg(test)]
    pub(crate) taken: usize,
}

/// Everything the selection's cards read.
#[derive(PartialEq)]
struct Sources {
    front: Option<u64>,
    picked: u64,
    revision: u64,
    device: u64,
    queue: Vec<(u64, std::mem::Discriminant<Diff>)>,
    tags: u64,
}

struct Shown {
    sources: Sources,
    checked: Vec<Item>,
    locals: Vec<u64>,
    about: About,
    /// What sending the selection would do, for one described row by row.
    going: Option<String>,
    offers: Vec<(Bulk, Offer)>,
    /// Each tag on any of the selection, and whether it is on all of it.
    wearing: Vec<(u64, String, bool)>,
    /// What one picked row plays, or what plays it.
    sounds: Option<Sounds>,
}

/// What the selection card says about what is picked.
enum About {
    Nothing,
    /// A folder or a tag.
    One(Vec<Fact>),
    /// One asset or slot.
    Row(Single),
    /// Several rows, in a line or two.
    Several(Vec<String>),
}

impl Picked {
    /// What the cards show now, taken again where something it reads has changed.
    fn now(
        &mut self,
        front: Option<u64>,
        browser: &Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
    ) -> &Shown {
        let sources = Sources {
            front,
            picked: browser.picked().revision(),
            revision: workspace.revision(),
            device: device.revision(),
            queue: queue.shape(),
            tags: browser.tags().revision(),
        };
        self.shown.take_if(|shown| shown.sources != sources);
        self.shown.get_or_insert_with(|| {
            #[cfg(test)]
            {
                self.taken += 1;
            }
            Shown::of(sources, browser, workspace, device, queue)
        })
    }
}

impl Shown {
    fn of(
        sources: Sources,
        browser: &Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
    ) -> Shown {
        let checked = match sources.front {
            Some(id) => workspace
                .get(id)
                .filter(|entity| entity.kept)
                .map(|_| Item::Local(id))
                .into_iter()
                .collect(),
            None => as_listed(browser.picked().items(), workspace, &device.state),
        };
        let locals: Vec<u64> = checked.iter().copied().filter_map(Item::local).collect();
        let (about, going) = match checked.len() {
            0 => (About::Nothing, None),
            many if many > MANY => (About::Several(summed(&checked, workspace, device)), None),
            _ => {
                let rows: Vec<Row> = checked
                    .iter()
                    .filter_map(|item| row_of(*item, workspace, &device.state, browser.tags()))
                    .collect();
                let going = crate::library::consequence(
                    &rows.iter().collect::<Vec<_>>(),
                    &device.state,
                    queue,
                );
                (about(&checked, &rows, browser, workspace, device), going)
            }
        };
        let offers = Bulk::ALL
            .into_iter()
            .map(|action| {
                (
                    action,
                    Offer::of(action, &checked, workspace, &device.state),
                )
            })
            .collect();
        let wearing = wearing(&locals, browser.tags());
        let sounds = match checked.as_slice() {
            [item] => sounds(*item, workspace, &device.state),
            _ => None,
        };
        Shown {
            sources,
            checked,
            locals,
            about,
            going,
            offers,
            wearing,
            sounds,
        }
    }
}

/// The selection, or the document in front: what it is and what can be asked of it, how
/// it is tagged, and what it plays, each in its own card.
fn selection(
    ui: &mut egui::Ui,
    shown: &Shown,
    browser: &mut Browser,
    device: &Device,
    folded: Option<(&mut bool, &mut bool)>,
    acts: &mut Vec<Act>,
) {
    let (selection_open, tags_open) = folded.unzip();
    let head = Head {
        open: selection_open,
        ..Head::fixed(Glyph::ScanEye, "Selection")
    };
    card(ui, head, |ui| {
        match &shown.about {
            About::Nothing => return faint(ui, "Nothing is selected."),
            About::One(facts) => facts.iter().for_each(|fact| fact_line(ui, fact)),
            About::Row(picked) => one(ui, picked, acts),
            About::Several(lines) => lines.iter().for_each(|line| void(ui, line)),
        }
        bulk_actions(ui, browser, shown, device, acts);
    });
    if shown.checked.is_empty() {
        return;
    }
    tags(
        ui,
        &shown.locals,
        &shown.wearing,
        browser.tags(),
        tags_open,
        acts,
    );
    kept_apart(ui, &shown.locals, browser, acts);
    if let Some(sounds) = &shown.sounds {
        sounds_card(ui, sounds, acts);
    }
    let answer = answered(browser, device).filter(|_| shown.sources.front.is_none());
    if let Some((slot, deps)) = answer {
        dependencies(ui, slot, deps);
    }
}

/// One picked row: its name, its place and the one action the document header offers,
/// the slots holding its bytes, then its other facts.
struct Single {
    name: String,
    /// The slot's place, or `None` for an asset with no slot.
    at: Option<String>,
    mark: Option<Mark>,
    faint: bool,
    /// The asset's id; a slot has none.
    id: Option<u64>,
    /// The header's action for an asset: its label, its hover, and what a click asks for.
    action: Option<(String, String, Click)>,
    /// The slots [`crate::device::link_offers`] finds for an asset with no slot.
    also: Vec<(ObjectClass, Location)>,
    facts: Vec<Fact>,
}

/// One line about a single picked asset: its label, its value, and the full text when
/// the value is shortened.
pub struct Fact {
    pub what: &'static str,
    pub said: String,
    pub hint: Option<String>,
}

/// The facts about the single picked asset besides its name and place.
///
/// The row is the library table's own, so these facts match the table. `fit` is the
/// attached instrument's verdict on the asset, which gets a line only when it refuses.
pub fn facts(row: &Row, fit: &Fit) -> Vec<Fact> {
    let mut said = vec![
        Fact {
            what: "kind",
            said: kind_word(row.kind, row.qualifier),
            hint: None,
        },
        Fact {
            what: "size",
            said: room::measure(row.size),
            hint: None,
        },
    ];
    if let Fit::Refuses(why) = fit {
        said.push(Fact {
            what: "refused",
            said: why.clone(),
            hint: None,
        });
    }
    said
}

/// The summary of a multiple selection: how many rows are picked, and of the assets among
/// them, how many are unsaved and how many are on the keyboard.
///
/// ⚠️ `picked` counts the rows the browser holds; `rows` holds only assets, which
/// excludes folder and tag rows.
pub fn tally(picked: usize, rows: &[Row]) -> String {
    if rows.is_empty() {
        return format!("{picked} selected");
    }
    let unsaved = rows.iter().filter(|row| row.unsaved).count();
    let keyboard = rows
        .iter()
        .filter(|row| matches!(row.where_, Where::Both(_) | Where::Keyboard))
        .count();
    format!("{picked} selected, {unsaved} unsaved, {keyboard} on the keyboard")
}

/// The facts about one picked row, or a summary of several.
fn about(
    checked: &[Item],
    rows: &[Row],
    browser: &Browser,
    workspace: &Workspace,
    device: &Device,
) -> About {
    let picked = checked.len();
    if let [item] = checked {
        if let Some(facts) = grouping(*item, browser, workspace) {
            return About::One(facts);
        }
    }
    let [row] = rows else {
        return About::Several(vec![tally(picked, rows)]);
    };
    if picked > 1 {
        // A folder picked beside one asset is still several picked.
        return About::Several(vec![tally(picked, rows)]);
    }
    let entity = row.item.local().and_then(|id| workspace.get(id));
    let held = entity.map_or(Fit::Unattached, |entity| fit(&device.state, entity));
    let action = entity
        .and_then(|entity| crate::document::action(entity, &device.state))
        .map(|loud| (loud.label, loud.hint, loud.click));
    let also = entity
        .filter(|_| row.at.is_none())
        .map_or_else(Vec::new, |entity| {
            link_offers(&device.state, workspace, entity)
        });
    About::Row(Single {
        name: row.name.clone(),
        at: row.at.map(|(class, at)| place(class, at)),
        mark: row.mark,
        faint: row.faint,
        id: entity.map(|entity| entity.id),
        action,
        also,
        facts: facts(row, &held),
    })
}

/// What a click on the document header's action asks for, from the inspector.
fn clicked(click: Click, id: u64) -> Option<Act> {
    match click {
        Click::Send(class, at) => Some(Act::Send { id, class, at }),
        Click::Pick => Some(Act::SendTo(id)),
        Click::Load(class, at) => Some(Act::LoadOnInstrument { class, at }),
        Click::Save => Some(Act::SaveDoc(id)),
        Click::Build => Some(Act::Build(id)),
        Click::Nothing => None,
    }
}

/// One picked row: its name, where it is with its mark, the header's one action, the
/// slots that also hold its bytes, then its facts.
fn one(ui: &mut egui::Ui, picked: &Single, acts: &mut Vec<Act>) {
    let name = crate::strings::display_name(&picked.name);
    ui.label(egui::RichText::new(name).font(egui::FontId::new(14.0, bold())))
        .on_hover_text(&picked.name);
    let quiet = ui.visuals().weak_text_color();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let place = picked.at.as_deref().unwrap_or("On this computer");
        let ink = match picked.faint {
            true => quiet,
            false => ui.visuals().text_color(),
        };
        ui.label(egui::RichText::new(place).text_style(ui_text()).color(ink));
        if let Some(mark) = picked.mark {
            let said = format!(" · {}", mark_words(mark).to_lowercase());
            let ink = mark_ink(mark, ui.visuals());
            ui.label(egui::RichText::new(said).text_style(ui_text()).color(ink));
        }
    });
    let Some(id) = picked.id else {
        return picked.facts.iter().for_each(|fact| fact_line(ui, fact));
    };
    if let Some((label, hint, click)) = &picked.action {
        crate::panel::tonal(ui);
        let live = *click != Click::Nothing;
        let button = ui
            .add_enabled(live, egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(hint);
        if let (true, Some(act)) = (button.clicked(), clicked(*click, id)) {
            acts.push(act);
        }
    }
    if !picked.also.is_empty() {
        also_at(ui, id, &picked.also, acts);
    }
    picked.facts.iter().for_each(|fact| fact_line(ui, fact));
}

/// `Also at 5:46 · Link`: the first two slots holding a file's bytes, how many more, and
/// Link, which links the file to the first.
fn also_at(ui: &mut egui::Ui, id: u64, offers: &[(ObjectClass, Location)], acts: &mut Vec<Act>) {
    let shown: Vec<String> = offers.iter().take(2).map(|(_, at)| shown(*at)).collect();
    let more = match offers.len().saturating_sub(2) {
        0 => String::new(),
        n => format!(" +{n}"),
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let quiet = ui.visuals().weak_text_color();
        let said = format!("Also at {}{more} · ", shown.join(", "));
        ui.label(egui::RichText::new(said).text_style(ui_text()).color(quiet));
        let (class, at) = offers[0];
        let link = ui
            .add(egui::Link::new(
                egui::RichText::new("Link").text_style(ui_text()),
            ))
            .on_hover_text(format!("link it to {}", place(class, at)));
        if link.clicked() {
            acts.push(Act::Link { id, class, at });
        }
    });
}

/// The facts about a folder or a tag: its name, and how many files are in it or wear it.
fn grouping(item: Item, browser: &Browser, workspace: &Workspace) -> Option<Vec<Fact>> {
    let (name, kind, files) = match item {
        Item::Folder(id) => (
            browser.folders.name_of(id)?,
            "folder",
            browser.folders.count(Some(id), workspace),
        ),
        Item::Tag(id) => (browser.tags().name_of(id)?, "tag", browser.tags().count(id)),
        Item::Local(_) | Item::Slot { .. } => return None,
    };
    let fact = |what, said: String| Fact {
        what,
        said,
        hint: None,
    };
    Some(vec![
        fact("name", name.to_string()),
        fact("kind", kind.to_string()),
        fact("files", files.to_string()),
    ])
}

/// The selection as the library's table shows it: a slot that an asset on this computer
/// is linked to is that asset's row.
fn as_listed(
    items: impl Iterator<Item = Item>,
    workspace: &Workspace,
    device: &DeviceState,
) -> Vec<Item> {
    let items: Vec<Item> = items.collect();
    if !items.iter().any(|item| matches!(item, Item::Slot { .. })) {
        return items;
    }
    let mut spots = BTreeMap::new();
    for entity in workspace.listed() {
        if let Some((class, at)) = crate::device::slot_of(device, entity) {
            spots.entry(Item::Slot { class, at }).or_insert(entity.id);
        }
    }
    let listed: BTreeSet<Item> = items
        .into_iter()
        .map(|item| spots.get(&item).map_or(item, |id| Item::Local(*id)))
        .collect();
    listed.into_iter().collect()
}

/// A selection too large to describe row by row: how many rows, how many bytes, and how
/// many of each kind.
fn summed(checked: &[Item], workspace: &Workspace, device: &Device) -> Vec<String> {
    let mut kinds = [0usize; Kind::ALL.len()];
    let mut bytes = 0u64;
    for item in checked {
        let (kind, len) = match *item {
            Item::Local(id) => {
                let Some(entity) = workspace.get(id) else {
                    continue;
                };
                (Kind::of(entity), entity.size())
            }
            Item::Slot { class, at } => {
                let Some(info) = device.state.slot(class, at).flatten() else {
                    continue;
                };
                (Kind::from_class(class), u64::from(info.body_len))
            }
            Item::Folder(_) | Item::Tag(_) => continue,
        };
        if let Some(at) = Kind::ALL.iter().position(|held| *held == kind) {
            kinds[at] += 1;
        }
        bytes = bytes.saturating_add(len);
    }
    let each: Vec<String> = Kind::ALL
        .iter()
        .zip(kinds)
        .filter(|(_, count)| *count > 0)
        .map(|(kind, count)| format!("{} {count}", kind.plural()))
        .collect();
    let mut said = vec![format!(
        "{} selected, {}",
        checked.len(),
        room::measure(bytes)
    )];
    if !each.is_empty() {
        said.push(each.join(" · "));
    }
    said
}

/// What sending the selection would do, when there is something to say, then every
/// action on the whole of it, wrapped to the card's width.
fn bulk_actions(
    ui: &mut egui::Ui,
    browser: &mut Browser,
    shown: &Shown,
    device: &Device,
    acts: &mut Vec<Act>,
) {
    ui.add_space(4.0);
    if let Some(going) = &shown.going {
        ui.add(egui::Label::new(egui::RichText::new(going).text_style(ui_text()).weak()).wrap());
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
        // ⚠️ Unwrapped, so a button short of room moves to the next row instead of
        // folding its label into the room left.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        crate::panel::tonal(ui);
        for (action, offer) in &shown.offers {
            browser.bulk_button(ui, *action, offer, &shown.checked, &device.state, acts);
        }
    });
}

/// One fact: its label in a fixed column and its full value beside it.
///
/// ⚠️ The value wraps instead of truncating. A refusal, a location sentence, or a name
/// can be any length, and a truncated one is misleading.
fn fact_line(ui: &mut egui::Ui, fact: &Fact) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [FACT, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(fact.what).text_style(ui_text()).weak())
                .halign(egui::Align::LEFT),
        );
        let said =
            ui.add(egui::Label::new(egui::RichText::new(&fact.said).text_style(ui_text())).wrap());
        if let Some(hint) = &fact.hint {
            said.on_hover_text(hint);
        }
    });
}

/// A line about the selection as a whole.
fn void(ui: &mut egui::Ui, said: &str) {
    ui.label(egui::RichText::new(said).text_style(ui_text()));
}

/// One meter per folder the instrument has counted, and a sentence on the limit the
/// queue runs into. The badge counts the folders past the warning point.
fn room_card(
    ui: &mut egui::Ui,
    open: &mut bool,
    workspace: &Workspace,
    device: &Device,
    queue: &Queue,
) {
    let meters: Vec<(ObjectClass, room::Meter)> = device
        .state
        .classes()
        .into_iter()
        .filter_map(|class| Some((class, room::meter(class, &device.state, queue, workspace)?)))
        .collect();
    let crowded = meters.iter().filter(|(_, held)| held.crowded()).count();
    let head = Head {
        glyph: Glyph::Gauge,
        title: "Room",
        badge: (crowded > 0).then(|| (format!("{crowded} nearly full"), warn(ui.visuals()))),
        open: Some(open),
    };
    card(ui, head, |ui| {
        if meters.is_empty() {
            return faint(ui, "The instrument's contents have not been counted.");
        }
        for (class, held) in meters {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(device.state.folder_name(class)).text_style(ui_text()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // ⚠️ The bar takes the status color and the readout stays in the text
                    // color: the signal colors are not legible as 10 px figures.
                    let unit = device.state.allocation_unit(class);
                    if let Some(said) = occupancy(class, &device.state.inventory, unit) {
                        ui.label(egui::RichText::new(said).monospace().size(MONO).weak());
                    }
                });
            });
            room::bar(ui, held);
            ui.add_space(AFTER_METER);
        }
        if let Some(said) = room::constraint(queue, workspace, &device.state) {
            ui.label(egui::RichText::new(said).text_style(ui_text()).weak());
        }
    });
}

/// The picked slot the instrument last listed dependencies for, and that list when it is
/// not empty.
///
/// ⚠️ `DEPENDENCIES` answers for one slot at a time and the cache holds the last answer,
/// so it applies only to that class at that address. A slot nobody asked about has no
/// answer, which is not the same as needing nothing.
fn answered<'a>(
    browser: &Browser,
    device: &'a Device,
) -> Option<((ObjectClass, Location), &'a [Dependency])> {
    let (class, at) = device.state.detail.at?;
    if !browser.picked().holds(Item::Slot { class, at }) {
        return None;
    }
    let deps = device.state.detail.deps.as_deref()?;
    (!deps.is_empty()).then_some(((class, at), deps))
}

/// What the instrument said a picked slot needs, with the slot it answered for as the
/// badge.
fn dependencies(ui: &mut egui::Ui, (class, at): (ObjectClass, Location), deps: &[Dependency]) {
    let head = Head {
        glyph: Glyph::Link,
        title: "Dependencies",
        badge: Some((place(class, at), caption(ui.visuals()))),
        open: None,
    };
    card(ui, head, |ui| {
        for dep in deps {
            let named = Some(dep.name.trim()).filter(|name| !name.is_empty());
            needed(ui, dep.class, named, dep.id);
        }
    });
}

/// One library a slot needs: the name the instrument gave it, or its bare id when that
/// is all there is.
fn needed(ui: &mut egui::Ui, class: ObjectClass, named: Option<&str>, id: u32) {
    ui.horizontal(|ui| {
        ui.set_min_height(LINE);
        let quiet = ui.visuals().weak_text_color();
        icon(ui, Kind::from_class(class).glyph(), GLYPH, quiet);
        match named {
            Some(name) => {
                let lit = good(ui.visuals());
                ui.label(egui::RichText::new(name).text_style(ui_text()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    signal_pill(ui, "installed", lit);
                });
            }
            None => {
                ui.label(
                    egui::RichText::new(format!("{id:#010x}"))
                        .monospace()
                        .size(MONO)
                        .color(quiet),
                )
                .on_hover_text(format!(
                    "the instrument holds no {} by this id",
                    Kind::from_class(class).chip()
                ));
            }
        }
    });
}

/// What the Plays or Played by card lists for the one picked row.
#[derive(Debug, PartialEq, Eq)]
enum Sounds {
    /// The pianos and samples a program plays, piano first.
    Plays(Vec<Sound>),
    /// The programs a set list plays, by address, each with its name where it resolves.
    Programs(Vec<(Location, Option<String>)>),
    /// The programs on this computer that play a piano or sample.
    PlayedBy(Vec<(u64, String)>),
}

/// A piano or sample a program plays.
#[derive(Debug, PartialEq, Eq)]
struct Sound {
    class: ObjectClass,
    /// Its name, or its id where nothing here or on the instrument goes by it.
    name: String,
    lies: Lies,
}

/// Where a piano or sample a program plays is.
#[derive(Debug, PartialEq, Eq)]
enum Lies {
    Here,
    /// Only in this slot of the attached instrument.
    KeyboardOnly(Location),
    Nowhere,
}

/// What the one picked row plays, or what plays it: `None` for a row that neither plays
/// nor is played.
fn sounds(item: Item, workspace: &Workspace, device: &DeviceState) -> Option<Sounds> {
    let entity = match item {
        Item::Local(id) => workspace.get(id)?,
        Item::Slot { class, at } if class.is_library() => {
            let name = device.slot(class, at).flatten()?.name.trim();
            return Some(played_by(workspace, crate::plays::slot_key(class, name)));
        }
        Item::Slot { .. } | Item::Folder(_) | Item::Tag(_) => return None,
    };
    if let Some(key) = crate::plays::key_of(entity) {
        return Some(played_by(workspace, key));
    }
    let decoded = entity.entity.as_deref();
    let seen = crate::document::setlist::Catalog { device, workspace };
    if let Some(programs) = decoded.and_then(|song| crate::document::setlist::programs(song, &seen))
    {
        return Some(Sounds::Programs(programs));
    }
    let catalog = Catalog::of(workspace, device);
    let mut plays: Vec<Sound> = crate::plays::named(entity)
        .iter()
        .map(|plays| sound(plays.key(), &catalog))
        .collect();
    let dialed = crate::bundle::dialed(decoded)
        .and_then(|(category, model)| device.piano_at(category, model));
    if let Some(piano) = dialed {
        plays.insert(
            0,
            sound(crate::plays::slot_key(ObjectClass::Piano, piano), &catalog),
        );
    }
    (!plays.is_empty()).then_some(Sounds::Plays(plays))
}

fn sound((class, id): crate::plays::Key, catalog: &Catalog) -> Sound {
    let Some(found) = catalog.get((class, id)) else {
        return Sound {
            class,
            name: format!("{} {id:#010x}", Kind::from_class(class).chip()),
            lies: Lies::Nowhere,
        };
    };
    let lies = match (found.here.is_empty(), found.there) {
        (true, Some(at)) => Lies::KeyboardOnly(at),
        _ => Lies::Here,
    };
    Sound {
        class,
        name: found.name.clone(),
        lies,
    }
}

fn played_by(workspace: &Workspace, key: crate::plays::Key) -> Sounds {
    let programs = crate::plays::played_by(workspace, key)
        .map(|entity| {
            (
                entity.id,
                crate::strings::display_name(&entity.name).to_string(),
            )
        })
        .collect();
    Sounds::PlayedBy(programs)
}

/// The Plays card of a program or set list, or the Played by card of a piano or sample.
fn sounds_card(ui: &mut egui::Ui, sounds: &Sounds, acts: &mut Vec<Act>) {
    match sounds {
        Sounds::Plays(plays) => card(ui, Head::fixed(Glyph::ListMusic, "Plays"), |ui| {
            plays.iter().for_each(|sound| sound_line(ui, sound, acts));
        }),
        Sounds::Programs(programs) => card(ui, Head::fixed(Glyph::ListMusic, "Plays"), |ui| {
            for (n, (at, name)) in (1..).zip(programs) {
                let name = name.as_deref().unwrap_or("—");
                void(ui, &format!("{n}  {}  {name}", crate::strings::shown(*at)));
            }
        }),
        Sounds::PlayedBy(programs) => {
            let head = Head {
                badge: Some((programs.len().to_string(), caption(ui.visuals()))),
                ..Head::fixed(Glyph::ListMusic, "Played by")
            };
            card(ui, head, |ui| {
                if programs.is_empty() {
                    return faint(ui, "No program here plays it.");
                }
                for (id, name) in programs {
                    if ui
                        .link(egui::RichText::new(name).text_style(ui_text()))
                        .clicked()
                    {
                        acts.push(Act::Open(Item::Local(*id)));
                    }
                }
            });
        }
    }
}

/// One piano or sample a program plays: its name, and where it is when that is not here.
fn sound_line(ui: &mut egui::Ui, sound: &Sound, acts: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        ui.set_min_height(LINE);
        let quiet = ui.visuals().weak_text_color();
        icon(ui, Kind::from_class(sound.class).glyph(), GLYPH, quiet);
        if sound.lies == Lies::Nowhere {
            ui.label(
                egui::RichText::new("!")
                    .text_style(ui_text())
                    .color(warn(ui.visuals())),
            );
        }
        ui.label(egui::RichText::new(&sound.name).text_style(ui_text()));
        let note = match sound.lies {
            Lies::Here => return,
            Lies::KeyboardOnly(_) => "on the keyboard only",
            Lies::Nowhere => "not here",
        };
        ui.label(egui::RichText::new(note).text_style(ui_text()).weak());
        if let Lies::KeyboardOnly(at) = sound.lies {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Copy").clicked() {
                    acts.push(Act::Copy {
                        class: sound.class,
                        at,
                    });
                }
            });
        }
    });
}

/// A note that a change to something picked is kept apart, opening Kept apart.
fn kept_apart(ui: &mut egui::Ui, picked: &[u64], browser: &Browser, acts: &mut Vec<Act>) {
    if !picked.iter().any(|&id| browser.folders.keeps_apart(id)) {
        return;
    }
    card(ui, Head::fixed(Glyph::FolderGit2, "Kept apart"), |ui| {
        ui.add(
            egui::Label::new("A change to what is selected is kept apart from the library.").wrap(),
        );
        ui.add_space(GAP);
        if tonal_button(ui, None, "Show kept apart").clicked() {
            acts.push(Act::ShowKept);
        }
    });
}

/// The selection's tags, as chips: solid when every picked asset has the tag and hollow
/// when only some do. Clicking a solid chip removes its tag from all of them; clicking a
/// hollow one adds it to all.
///
/// ⚠️ Only a kept asset can have a tag: a tag attaches to a workspace id, and a slot has
/// none. So this covers only the part of the selection on this computer.
///
/// Add tag opens every tag of `labels`, as a row's Tag menu does. `open` folds the card,
/// while a document's cards stand above it.
fn tags(
    ui: &mut egui::Ui,
    picked: &[u64],
    wearing: &[(u64, String, bool)],
    labels: &Tags,
    open: Option<&mut bool>,
    acts: &mut Vec<Act>,
) {
    if picked.is_empty() {
        return;
    }
    let head = Head {
        open,
        ..Head::fixed(Glyph::Tags, "Tags")
    };
    card(ui, head, |ui| {
        let add = tonal_button(ui, Some(Glyph::Plus), "Add tag");
        egui::Popup::menu(&add).show(|ui| tag_items(ui, labels, picked, acts));
        if wearing.is_empty() {
            return;
        }
        ui.add_space(GAP);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::splat(5.0);
            for &(id, ref name, on_all) in wearing {
                let clicked = tag_chip(ui, id, name, on_all)
                    .on_hover_text(match on_all {
                        true => "on everything selected; click to remove it from all",
                        false => "on some of what is selected; click to add it to all",
                    })
                    .clicked();
                if clicked {
                    let ids = picked.to_vec();
                    acts.push(match on_all {
                        true => Act::Untag { ids, tag: id },
                        false => Act::Tag { ids, tag: id },
                    });
                }
            }
        });
    });
}

/// One tag on the selection, as a pill: solid in the accent when it is on everything
/// picked, hollow when it is on only some.
fn tag_chip(ui: &mut egui::Ui, id: u64, name: &str, on_all: bool) -> egui::Response {
    const HEIGHT: f32 = 26.0;
    const SIDE: f32 = 10.0;
    const SPACE: f32 = 5.0;
    const TEXT: f32 = 12.0;

    let visuals = ui.visuals();
    let lit = accent(visuals);
    let (ink, fill, border) = match on_all {
        true => (
            visuals.widgets.active.fg_stroke.color,
            tint(lit, 0.16),
            tint(lit, 0.6),
        ),
        false => (
            caption(visuals),
            egui::Color32::TRANSPARENT,
            visuals.widgets.noninteractive.bg_stroke.color,
        ),
    };
    let hovered_border = visuals.widgets.hovered.bg_stroke.color;
    let galley =
        ui.painter()
            .layout_no_wrap(name.to_owned(), egui::FontId::proportional(TEXT), ink);
    let size = egui::vec2(SIDE + TAG + SPACE + galley.size().x + SIDE, HEIGHT);
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, ui.id().with(("tag", id)), egui::Sense::click());
    let border = match response.hovered() {
        true => hovered_border,
        false => border,
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        u8::MAX,
        fill,
        egui::Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );
    let middle = rect.center().y;
    let left = rect.left() + SIDE;
    painted(
        ui,
        Glyph::Tag,
        egui::Rect::from_min_size(egui::pos2(left, middle - TAG / 2.0), egui::Vec2::splat(TAG)),
        ink,
    );
    let top = middle - galley.size().y / 2.0;
    painter.galley(
        egui::pos2(left + TAG + SPACE, top),
        galley,
        egui::Color32::PLACEHOLDER,
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on_all, name));
    response
}

/// The tags any picked asset has, and whether each is on all of them.
///
/// Only tags in use, in list order.
fn wearing(picked: &[u64], worn: &Tags) -> Vec<(u64, String, bool)> {
    worn.all()
        .iter()
        .filter(|tag| picked.iter().any(|id| worn.worn(*id).contains(&tag.id)))
        .map(|tag| (tag.id, tag.name.clone(), worn.on_all(picked, tag.id)))
        .collect()
}

/// The line a card shows when it has nothing to say.
fn faint(ui: &mut egui::Ui, said: &str) {
    ui.label(
        egui::RichText::new(said)
            .text_style(ui_text())
            .weak()
            .italics(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::LOAD_ON_INSTRUMENT;
    use crate::link::Standing;
    use crate::tags::Asked;
    use crate::testing::{self, context, Bench};
    use crate::workspace::{Fresh, Origin};
    use nord_usb::wire::{Dependency, Status};

    /// An instrument that has counted a library, holds a program, and has answered which
    /// libraries one slot needs: one named and one not.
    fn attached(ctx: &egui::Context) -> (Device, Location) {
        let at = Location { bank: 6, slot: 0 };
        let mut device = Device::new(ctx.clone());
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.state.inventory.push(Status {
            class: ObjectClass::Sample,
            count: 84,
            free: 60,
            used: 1472,
            dirty: 0,
            spare: 4,
        });
        device.pretend_deps(
            ObjectClass::Program,
            at,
            vec![
                Dependency {
                    flag: 1,
                    class: ObjectClass::Piano,
                    id: 0x0102_0304,
                    name: "Royal Grand 3D ".into(),
                    location: None,
                },
                Dependency {
                    flag: 1,
                    class: ObjectClass::Sample,
                    id: 0x0999_0999,
                    name: "   ".into(),
                    location: None,
                },
            ],
        );
        (device, at)
    }

    /// Draw the whole inspector over `picked`, plus the two panels that take their own
    /// selection.
    ///
    /// No pixels are checked. This catches a layout that panics or an id that collides,
    /// which the rule tests would miss.
    fn paint(
        shell: &mut Shell,
        device: &Device,
        picked: &[(ObjectClass, Location)],
    ) -> Vec<String> {
        let ctx = context();
        let mut browser = Browser::default();
        for (class, at) in picked.iter().copied() {
            browser.check(Item::Slot { class, at });
        }
        let workspace = Workspace::new(ctx.clone());
        let queue = Queue::default();
        let mut labels = Tags::default();
        let sunday = labels.make("Sunday").unwrap();
        labels.set(7, sunday, true);
        let mut said = Vec::new();
        // Twice: the second pass runs with the widget state the first left behind.
        for _ in 0..2 {
            let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| {
                        super::ui(
                            panel,
                            None,
                            |_: &mut egui::Ui| false,
                            shell,
                            &mut browser,
                            &workspace,
                            device,
                            &queue,
                        );
                        if let Some((slot, deps)) = answered(&browser, device) {
                            dependencies(panel, slot, deps);
                        }
                        tags(
                            panel,
                            &[7, 8],
                            &wearing(&[7, 8], &labels),
                            &labels,
                            None,
                            &mut Vec::new(),
                        );
                    });
            });
            said = testing::words(&output);
        }
        said
    }

    #[test]
    fn the_instruments_cards_show_only_while_one_is_attached() {
        let ctx = context();
        let mut shell = Shell {
            info_open: true,
            ..Shell::default()
        };
        let detached = paint(&mut shell, &Device::new(ctx.clone()), &[]);
        assert!(
            detached.iter().any(|word| word == "Selection"),
            "{detached:?}"
        );
        for title in ["Room", "Info"] {
            assert!(!detached.iter().any(|word| word == title), "{detached:?}");
        }

        let (device, at) = attached(&ctx);
        let held = paint(&mut shell, &device, &[(ObjectClass::Program, at)]);
        for title in ["Selection", "Dependencies", "Room", "Info"] {
            assert!(held.iter().any(|word| word == title), "{title}: {held:?}");
        }

        // Collapsed cards draw only their headers.
        let mut shut = Shell {
            room_open: false,
            info_open: false,
            ..Shell::default()
        };
        let shut = paint(&mut shut, &device, &[(ObjectClass::Program, at)]);
        for title in ["Room", "Info"] {
            assert!(shut.iter().any(|word| word == title), "{title}: {shut:?}");
        }
        assert!(shut.len() < held.len(), "{shut:?}");
    }

    /// One frame of a collapsing card, clicking `at` when `press` is set, and leaving in
    /// `at` the right end of the card's header.
    fn headed(
        ctx: &egui::Context,
        open: &mut bool,
        press: bool,
        at: &std::cell::Cell<egui::Pos2>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            events: match press {
                true => testing::click(at.get()),
                false => Vec::new(),
            },
            ..Default::default()
        };
        testing::run(ctx, input, |ctx| {
            egui::SidePanel::right("inspector")
                .exact_width(crate::shell::INSPECTOR)
                .show(ctx, |ui| {
                    let top = ui.cursor().top();
                    let head = Head {
                        glyph: Glyph::Gauge,
                        title: "Room",
                        badge: Some(("1 nearly full".into(), egui::Color32::RED)),
                        open: Some(&mut *open),
                    };
                    card(ui, head, |ui| {
                        ui.label("Programs");
                    });
                    // The right end, past the title, where only the header is.
                    let right = ui.min_rect().right() - HEAD_SIDE - CHEVRON / 2.0;
                    at.set(egui::pos2(right, top + HEAD / 2.0));
                });
        })
    }

    /// A collapsing card opens and shuts from anywhere on its header, its triangle is at
    /// the right end, and it keeps its header while shut.
    #[test]
    fn a_collapsing_card_toggles_from_its_header_and_keeps_it_while_shut() {
        let ctx = context();
        let at = std::cell::Cell::new(egui::Pos2::ZERO);
        let mut open = true;
        // The first frame only learns where the header is; the second presses it.
        let first = testing::words(&headed(&ctx, &mut open, false, &at));
        assert!(first.contains(&"Programs".to_string()), "{first:?}");
        let shut = testing::words(&headed(&ctx, &mut open, true, &at));
        assert!(!open, "a click on the header shuts the card");
        assert!(!shut.contains(&"Programs".to_string()), "{shut:?}");
        for word in ["Room", "1 nearly full"] {
            assert!(shut.contains(&word.to_string()), "{word}: {shut:?}");
        }
        headed(&ctx, &mut open, true, &at);
        assert!(open, "the next click opens it again");
    }

    /// The dependency answer is a list of pills in the tone of each line's state.
    #[test]
    fn an_installed_dependency_says_so_on_a_good_pill() {
        let ctx = context();
        let (device, at) = attached(&ctx);
        let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
            egui::SidePanel::right("inspector")
                .exact_width(crate::shell::INSPECTOR)
                .show(ctx, |ui| {
                    let deps = device.state.detail.deps.as_deref().unwrap();
                    dependencies(ui, (ObjectClass::Program, at), deps);
                });
        });
        let said = testing::painted(&output);
        let installed = testing::where_(&said, "installed");
        let lit = good(&ctx.style().visuals);
        let pill = testing::rects(&output)
            .into_iter()
            .filter(|drawn| drawn.rect.contains_rect(installed))
            .min_by(|one, other| one.rect.area().total_cmp(&other.rect.area()))
            .expect("the word sits on a pill");
        assert_eq!(pill.fill, tint(lit, 0.15));
        assert!(said.iter().any(|word| word.text == "Royal Grand 3D"));
        assert!(
            said.iter().any(|word| word.text == "0x09990999"),
            "a library listed without a name shows its id: {said:?}"
        );
        assert!(
            said.iter()
                .any(|word| word.text == place(ObjectClass::Program, at)),
            "the badge names the slot that was asked about: {said:?}"
        );
    }

    fn program_playing(piano: &str, sample: u32) -> Vec<u8> {
        let sets = [
            (
                "piano_panel.id".into(),
                crate::plays::slot_key(ObjectClass::Piano, piano)
                    .1
                    .to_string(),
            ),
            ("sample_panel.id".into(), sample.to_string()),
        ];
        crate::fields::apply(&Fresh::Program.bytes().unwrap(), &sets)
            .unwrap()
            .1
    }

    #[test]
    fn a_program_plays_what_is_here_on_the_keyboard_only_or_nowhere() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let marimba = crate::plays::slot_key(ObjectClass::Sample, "Marimba").1;
        let program = workspace.ingest(
            "Keys.ne5p".into(),
            Origin::Fresh,
            program_playing("Tine", marimba),
            &mut log,
        );
        let gone = workspace.ingest(
            "Pad.ne5p".into(),
            Origin::Fresh,
            program_playing("Tine", 0x0999_0999),
            &mut log,
        );
        workspace.ingest(
            "Tine.npno".into(),
            Origin::Fresh,
            testing::piano(1),
            &mut log,
        );
        device.pretend_lengths(ObjectClass::Sample, 1, &[None, Some(("Marimba", 1000))]);
        let marimba_at = Location::from_user(1, 2);

        let plays = |id| sounds(Item::Local(id), &workspace, &device.state);
        let tine = Sound {
            class: ObjectClass::Piano,
            name: "Tine".into(),
            lies: Lies::Here,
        };
        assert_eq!(
            plays(program),
            Some(Sounds::Plays(vec![
                tine,
                Sound {
                    class: ObjectClass::Sample,
                    name: "Marimba".into(),
                    lies: Lies::KeyboardOnly(marimba_at),
                },
            ]))
        );
        let Some(Sounds::Plays(of_gone)) = plays(gone) else {
            panic!("a program plays");
        };
        assert_eq!(
            of_gone[1],
            Sound {
                class: ObjectClass::Sample,
                name: "sample 0x09990999".into(),
                lies: Lies::Nowhere,
            }
        );
    }

    #[test]
    fn a_piano_is_played_by_the_programs_here_that_name_it() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let keys = workspace.ingest(
            "Keys.ne5p".into(),
            Origin::Fresh,
            program_playing("Tine", 0),
            &mut log,
        );
        workspace.ingest(
            "Other.ne5p".into(),
            Origin::Fresh,
            program_playing("Wurli", 0),
            &mut log,
        );
        let tine = workspace.ingest(
            "Tine.npno".into(),
            Origin::Fresh,
            testing::piano(1),
            &mut log,
        );
        device.pretend_lengths(ObjectClass::Piano, 3, &[Some(("Tine", 1000))]);
        let by_keys = Some(Sounds::PlayedBy(vec![(keys, "Keys".into())]));

        assert_eq!(
            sounds(Item::Local(tine), &workspace, &device.state),
            by_keys
        );
        let slot = Item::Slot {
            class: ObjectClass::Piano,
            at: Location::from_user(3, 1),
        };
        assert_eq!(sounds(slot, &workspace, &device.state), by_keys);
    }

    #[test]
    fn a_set_list_plays_its_programs_by_address_named_where_they_resolve() {
        use nord_format::formats::ne5;
        use nord_format::{Entity, Song};

        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let (one, two) = ((0, 0).try_into().unwrap(), (1, 3).try_into().unwrap());
        let song = ne5::song::new((0, 0).try_into().unwrap(), 1, [one, two, one, one]).unwrap();
        let bytes = nord_format::to_bytes(&Entity::Song(Song::Electro5(song))).unwrap();
        let id = workspace.ingest("Gig.ne5t".into(), Origin::Fresh, bytes, &mut log);
        device.pretend_scanned(ObjectClass::Program, 1, &["Africa Split"]);

        let Some(Sounds::Programs(programs)) = sounds(Item::Local(id), &workspace, &device.state)
        else {
            panic!("a set list plays programs");
        };
        assert_eq!(
            programs[..2],
            [
                (Location::from_user(1, 1), Some("Africa Split".to_string())),
                (Location::from_user(2, 4), None),
            ]
        );
    }

    #[test]
    fn the_plays_card_says_where_a_sound_is_missing_and_offers_a_copy() {
        let ctx = context();
        let plays = Sounds::Plays(vec![
            Sound {
                class: ObjectClass::Piano,
                name: "Tine".into(),
                lies: Lies::Here,
            },
            Sound {
                class: ObjectClass::Sample,
                name: "Marimba".into(),
                lies: Lies::KeyboardOnly(Location::from_user(1, 2)),
            },
            Sound {
                class: ObjectClass::Sample,
                name: "sample 0x09990999".into(),
                lies: Lies::Nowhere,
            },
        ]);
        let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                sounds_card(ui, &plays, &mut Vec::new());
            });
        });
        let said = testing::words(&output);
        for word in [
            "Plays",
            "Tine",
            "on the keyboard only",
            "Copy",
            "!",
            "not here",
        ] {
            assert!(said.iter().any(|held| held == word), "{word}: {said:?}");
        }
    }

    #[test]
    fn the_facts_of_one_picked_asset_are_its_rows_own() {
        use crate::browser::Qualifier;
        use nord_format::accept::Family;

        let row = Row {
            item: Item::Local(1),
            kind: Kind::Program,
            qualifier: Some(Qualifier::Family(Family::Stage4)),
            name: "Africa Split".into(),
            tags: 1,
            unsaved: true,
            where_: Where::Both(Standing::Ahead),
            at: None,
            faint: false,
            mark: None,
            size: 2_048,
            needs: crate::library::Needs::Nothing,
        };

        let said = facts(&row, &Fit::Takes);
        let lines: Vec<(&str, &str)> = said
            .iter()
            .map(|fact| (fact.what, fact.said.as_str()))
            .collect();
        assert_eq!(lines, [("kind", "Stage 4 program"), ("size", "2.0 kB")]);

        // The only fact not taken from the row is the instrument's refusal.
        let why = "This is a Stage 4 file and the instrument is a Nord Electro 5D.";
        let refused = facts(&row, &Fit::Refuses(why.into()));
        assert_eq!(refused.len(), said.len() + 1);
        assert_eq!(refused[2].what, "refused");
        assert_eq!(refused[2].said, why);
        assert_eq!(
            facts(&row, &Fit::Warn("untried".into())).len(),
            said.len(),
            "an untried write is the queue's warning, not a fact about the asset"
        );
    }

    #[test]
    fn a_fact_too_long_for_the_dock_wraps_rather_than_truncating() {
        let ctx = context();
        let lines = |said: &str| {
            let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |ui| {
                        fact_line(
                            ui,
                            &Fact {
                                what: "refused",
                                said: said.to_string(),
                                hint: None,
                            },
                        );
                    });
            });
            testing::painted(&output)
                .iter()
                .find(|word| word.text == said)
                .map(|word| word.galley.rows.len())
        };

        assert_eq!(lines("2.0 kB"), Some(1), "a short value keeps its one line");
        let why = "This is a Stage 4 file and the instrument is a Nord Electro 5D.";
        assert!(
            lines(why).is_some_and(|rows| rows > 1),
            "the whole refusal is painted: {:?}",
            lines(why)
        );
    }

    #[test]
    fn a_selection_of_several_says_how_many_and_what_of_them() {
        let row = |name: &str, unsaved: bool, where_: Where| Row {
            item: Item::Local(1),
            kind: Kind::Program,
            qualifier: None,
            name: name.into(),
            tags: 0,
            unsaved,
            where_,
            at: None,
            faint: false,
            mark: None,
            size: 0,
            needs: crate::library::Needs::Nothing,
        };
        let rows = [
            row("kept", false, Where::Computer),
            row("edited", true, Where::Both(Standing::Ahead)),
            row("read off a slot", false, Where::Keyboard),
        ];
        assert_eq!(tally(3, &rows), "3 selected, 1 unsaved, 2 on the keyboard");

        // Folders and tags are not assets, so they count only as picked.
        assert_eq!(tally(2, &[]), "2 selected");
    }

    /// One frame of the inspector over the bench, and every word it painted.
    fn inspected(bench: &mut Bench) -> Vec<String> {
        let input = testing::screen(egui::vec2(800.0, 900.0), Vec::new());
        let output = testing::run(&bench.ctx, input, |ctx| {
            egui::SidePanel::right("inspector")
                .exact_width(crate::shell::INSPECTOR)
                .show(ctx, |panel| {
                    let Bench {
                        shell,
                        browser,
                        workspace,
                        device,
                        queue,
                        ..
                    } = bench;
                    super::ui(
                        panel,
                        None,
                        |_: &mut egui::Ui| false,
                        shell,
                        browser,
                        workspace,
                        device,
                        queue,
                    );
                });
        });
        testing::words(&output)
    }

    #[test]
    fn a_folder_selection_is_not_summarized_as_an_asset() {
        let mut bench = Bench::new();
        let root = crate::store::LibPath::root();
        let folder = bench.browser.folders.make(&root, &bench.workspace);
        let dir = bench.browser.folders.path_of(folder).unwrap().clone();
        for name in ["Grand.ne5p", "Upright.ne5p"] {
            let id = bench
                .workspace
                .create(Fresh::Program, &mut bench.log)
                .unwrap();
            bench.workspace.place(id, dir.join(name));
        }
        let tag = bench.browser.tags.make("Sunday").unwrap();

        for (item, name, kind, files) in [
            (Item::Folder(folder), "New folder", "folder", "2"),
            (Item::Tag(tag), "Sunday", "tag", "0"),
        ] {
            bench.browser.check(item);
            let said = inspected(&mut bench);
            for word in [name, kind, files] {
                assert!(said.iter().any(|said| said == word), "{word}: {said:?}");
            }
            assert!(
                !said.iter().any(|word| word.contains("selected")),
                "{kind} is described, not counted: {said:?}"
            );
            bench.browser.check(item);
        }
    }

    #[test]
    fn a_slot_copied_here_is_described_as_the_asset_it_became() {
        let mut bench = Bench::new();
        let (class, at) = (ObjectClass::Program, Location { bank: 6, slot: 0 });
        let bytes = Fresh::Program.bytes().unwrap();
        let origin = || crate::workspace::Origin::Device { class, at };
        let mut scratch = Workspace::new(bench.ctx.clone());
        let held = scratch.ingest("x".into(), origin(), bytes.clone(), &mut bench.log);
        let crc = scratch.get(held).unwrap().saved.crc32().unwrap();
        bench
            .device
            .pretend_bodies(class, 7, &[Some(("Africa Split", crc))]);
        bench.browser.check(Item::Slot { class, at });
        let copy = |bench: &Bench| {
            let shown = bench.shell.picked.shown.as_ref().unwrap();
            let mut offers = shown.offers.iter();
            offers
                .find(|(action, _)| *action == Bulk::Copy)
                .unwrap()
                .1
                .live
        };

        let said = inspected(&mut bench);
        assert!(said.iter().any(|word| word == "Programs 7:1"), "{said:?}");
        assert!(copy(&bench), "a slot on the keyboard only can be copied");

        let name = "Africa Split.ne5p".to_string();
        let print = crate::workspace::print_of(&bytes).unwrap();
        let id = bench
            .workspace
            .ingest(name, origin(), bytes, &mut bench.log);
        crate::device::link_copy(
            &bench.device.state,
            &mut bench.workspace,
            id,
            class,
            at,
            print,
        );
        let said = inspected(&mut bench);
        assert!(said.iter().any(|word| word == "Programs 7:1"), "{said:?}");
        assert!(
            said.iter().any(|word| word == LOAD_ON_INSTRUMENT),
            "{said:?}"
        );
        assert!(!copy(&bench), "a copy is already on this computer");
    }

    #[test]
    fn one_picked_file_says_where_it_is_its_one_action_and_where_else_its_bytes_are() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        let bytes = Fresh::Program.bytes().unwrap();
        let id = bench
            .workspace
            .ingest("Jazzy.ne5p".into(), Origin::Fresh, bytes, &mut bench.log);
        let crc = bench.workspace.get(id).unwrap().saved_print().unwrap().crc;
        bench
            .device
            .pretend_bodies(class, 5, &[None, Some(("First", crc))]);
        bench.browser.check(Item::Local(id));
        let said = inspected(&mut bench);
        for word in ["On this computer", "Send to…", "Also at 5:2 · ", "Link"] {
            assert!(said.iter().any(|said| said == word), "{word}: {said:?}");
        }

        let base = crate::link::Print {
            crc: !crc,
            len: 121,
        };
        let held = crate::link::Held::new(
            class,
            crate::link::Spot::at(Location { bank: 4, slot: 0 }),
            base,
        );
        let keyboard = bench.device.state.keyboard().unwrap();
        bench.workspace.link(id, &keyboard, held.unwrap());
        bench
            .device
            .pretend_bodies(class, 5, &[Some(("Jazzy", !crc))]);
        let said = inspected(&mut bench);
        for word in ["Programs 5:1", " · ready to send", "Send to 5:1"] {
            assert!(said.iter().any(|said| said == word), "{word}: {said:?}");
        }
        assert!(
            !said.iter().any(|word| word.starts_with("Also at")),
            "{said:?}"
        );
    }

    /// ⚠️ Every class is addressed by the same banks and slots, so the sample at a
    /// program's address must not show the program's list.
    #[test]
    fn a_selection_the_instrument_was_not_asked_about_shows_nothing() {
        let ctx = context();
        let (device, at) = attached(&ctx);
        let picking = |class, at| {
            let mut browser = Browser::default();
            browser.check(Item::Slot { class, at });
            answered(&browser, &device).map(|(slot, _)| slot)
        };
        assert_eq!(
            picking(ObjectClass::Program, at),
            Some((ObjectClass::Program, at)),
            "the slot it was asked about"
        );
        assert_eq!(
            picking(ObjectClass::Sample, at),
            None,
            "another class at the same address"
        );
        let elsewhere = Location { bank: 0, slot: 0 };
        assert_eq!(
            picking(ObjectClass::Program, elsewhere),
            None,
            "a program nothing has asked about"
        );
    }

    #[test]
    fn the_selections_tags_are_chips_beside_add_tag_and_nothing_without_a_selection() {
        let ctx = context();
        let mut labels = Tags::default();
        let (both, some) = (labels.make("Sunday").unwrap(), labels.make("Loud").unwrap());
        for tag in [both, some] {
            labels.set(7, tag, true);
        }
        labels.set(8, both, true);

        assert_eq!(
            wearing(&[7, 8], &labels),
            [
                (both, "Sunday".to_string(), true),
                (some, "Loud".to_string(), false)
            ]
        );
        assert!(wearing(&[], &labels).is_empty(), "nothing picked");
        assert!(wearing(&[9], &labels).is_empty(), "picked, with no tags");

        let painted = |picked: &[u64]| {
            let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| {
                        tags(
                            panel,
                            picked,
                            &wearing(picked, &labels),
                            &labels,
                            None,
                            &mut Vec::new(),
                        )
                    });
            });
            testing::words(&output)
        };
        let said = painted(&[7, 8]);
        for name in ["Sunday", "Loud", "Add tag"] {
            assert!(said.contains(&name.to_string()), "{name}: {said:?}");
        }
        let untagged = painted(&[9]);
        assert!(untagged.contains(&"Add tag".to_string()), "{untagged:?}");
        assert!(!untagged.contains(&"Sunday".to_string()), "{untagged:?}");
        assert!(painted(&[]).is_empty(), "{:?}", painted(&[]));
    }

    /// The Selection card offers every action on the selection, whole, on one line, and
    /// inside the card at any width the inspector takes, and Queue for sending queues what
    /// is selected.
    #[test]
    fn the_selection_card_offers_every_action_on_the_selection() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            mut device,
            queue,
            mut shell,
            mut log,
            ..
        } = Bench::new();
        device.pretend_attached_as("Nord Electro 5");
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.check(Item::Local(id));
        let screen = egui::vec2(800.0, 900.0);

        let least = crate::shell::SIDE_LEAST as usize;
        for width in (least..=400).step_by(4).map(|width| width as f32) {
            let mut acts = Vec::new();
            let mut frame = |events: Vec<egui::Event>| {
                let input = testing::screen(screen, events);
                let output = testing::run(&ctx, input, |ctx| {
                    egui::SidePanel::right("inspector")
                        .exact_width(width)
                        .show(ctx, |panel| {
                            acts.extend(super::ui(
                                panel,
                                None,
                                |_: &mut egui::Ui| false,
                                &mut shell,
                                &mut browser,
                                &workspace,
                                &device,
                                &queue,
                            ));
                        });
                });
                testing::painted(&output)
            };
            frame(Vec::new());
            let said = frame(Vec::new());
            let line = testing::where_(&said, "Selection").height();
            for action in Bulk::ALL {
                let word = testing::where_(&said, action.label());
                assert!(
                    word.left() >= screen.x - width && word.right() <= screen.x - GUTTER,
                    "{width}: {} at {word:?} runs past the card",
                    action.label()
                );
                assert!(
                    word.height() < 1.5 * line,
                    "{width}: {} at {word:?} folds onto more than one line",
                    action.label()
                );
            }
            frame(testing::click(
                testing::where_(&said, Bulk::Queue.label()).center(),
            ));
            assert!(
                acts.iter()
                    .any(|act| matches!(act, Act::SendChecked(ids) if ids == &[id])),
                "{width}: Queue for sending queues the selection; got {} acts",
                acts.len()
            );
        }
    }

    #[test]
    fn a_large_selection_is_summed_up_once_and_not_row_by_row() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            mut shell,
            mut log,
            ..
        } = Bench::new();
        let saved = (1..=200)
            .map(|id| crate::workspace::Saved {
                id,
                name: format!("Sound {id:03}.ne5p"),
                path: Some(crate::store::LibPath::root().join(&format!("Sound {id:03}.ne5p"))),
                origin: crate::workspace::Origin::Fresh,
                content: crate::workspace::Content::unread(100),
                unsaved: None,
            })
            .collect();
        workspace.restore(saved, None, &mut log);
        for id in 1..=200 {
            browser.check(Item::Local(id));
        }
        let frame = |browser: &mut Browser, shell: &mut Shell| {
            let input = testing::screen(egui::vec2(800.0, 900.0), Vec::new());
            let output = testing::run(&ctx, input, |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| {
                        super::ui(
                            panel,
                            None,
                            |_: &mut egui::Ui| false,
                            shell,
                            browser,
                            &workspace,
                            &device,
                            &queue,
                        );
                    });
            });
            testing::painted(&output)
        };
        for _ in 0..3 {
            frame(&mut browser, &mut shell);
        }
        let said = frame(&mut browser, &mut shell);
        assert_eq!(shell.picked.taken, 1, "taken once for four frames");
        let has = |text: &str| said.iter().any(|word| word.text == text);
        let total = format!("200 selected, {}", room::measure(200 * 100));
        assert!(has(&total), "{total}");
        assert!(has(&format!("{} 200", Kind::Program.plural())));
        assert!(
            !said.iter().any(|word| word.text.contains("unsaved")),
            "no line counts row by row"
        );

        browser.check(Item::Local(1));
        let said = frame(&mut browser, &mut shell);
        assert_eq!(shell.picked.taken, 2, "taken again for a new selection");
        let total = format!("199 selected, {}", room::measure(199 * 100));
        assert!(said.iter().any(|word| word.text == total), "{total}");
    }

    /// What picking `item` from the Tags card's Add tag menu asks of a selection of 7
    /// and 8, where only 7 wears Loud and nothing wears Sunday.
    fn added_from_the_card(item: &str) -> (Vec<Act>, u64) {
        let ctx = context();
        egui_extras::install_image_loaders(&ctx);
        let mut labels = Tags::default();
        let (sunday, loud) = (labels.make("Sunday").unwrap(), labels.make("Loud").unwrap());
        labels.set(7, loud, true);
        let mut acts = Vec::new();
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let output = testing::run(&ctx, input, |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| {
                        tags(
                            panel,
                            &[7, 8],
                            &wearing(&[7, 8], &labels),
                            &labels,
                            None,
                            &mut acts,
                        )
                    });
            });
            testing::painted(&output)
        };
        let add = testing::where_(&frame(Vec::new()), "Add tag").center();
        frame(testing::click(add));
        let mut said = frame(Vec::new());
        for _ in 0..2 {
            said = frame(Vec::new());
        }
        frame(testing::click(testing::where_(&said, item).center()));
        (acts, sunday)
    }

    #[test]
    fn add_tag_in_the_card_puts_a_tag_nothing_selected_wears_on_all_of_it() {
        let (acts, sunday) = added_from_the_card("Sunday");
        assert!(
            matches!(acts.as_slice(), [Act::Tag { ids, tag }] if ids == &[7, 8] && *tag == sunday),
            "Sunday on the selection; got {acts:?}"
        );
    }

    #[test]
    fn add_tag_in_the_card_offers_a_new_tag_on_the_selection() {
        let (acts, _) = added_from_the_card("New tag…");
        assert!(
            matches!(acts.as_slice(), [Act::NewTag(ids)] if ids == &[7, 8]),
            "one new tag on the selection; got {acts:?}"
        );
    }

    #[test]
    fn a_tag_goes_on_the_whole_selection_and_comes_off_it_again() {
        let mut bench = Bench::new();
        let ids: Vec<u64> = (0..2)
            .map(|_| {
                bench
                    .workspace
                    .create(Fresh::Program, &mut bench.log)
                    .unwrap()
            })
            .collect();

        bench.act(vec![Act::NewTag(Vec::new())]);
        let tag = bench.browser.tags().all()[0].id;
        bench.act(vec![Act::RenameTag {
            id: tag,
            name: "Sunday".into(),
        }]);
        let name = bench.browser.tags().name_of(tag).unwrap().to_string();
        assert!(
            !bench.browser.tags().on_all(&ids, tag),
            "hollow to begin with"
        );

        bench.act(vec![Act::Tag {
            ids: ids.clone(),
            tag,
        }]);
        bench.act(vec![Act::Untag {
            ids: ids.clone(),
            tag,
        }]);
        let assets = ids.clone();
        assert_eq!(
            bench.browser.take_tag_asks(),
            [
                Asked::Tag {
                    assets: assets.clone(),
                    name: name.clone(),
                    make: true
                },
                Asked::Untag { assets, name }
            ],
            "on all, then off all"
        );
    }
}
