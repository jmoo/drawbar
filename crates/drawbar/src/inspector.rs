//! The right dock: the selection, and the instrument while one is attached.
//!
//! Each part is a card. The selection's facts, its tags, and what a picked slot needs
//! describe the rows selected in the browser, whether or not an instrument is connected.
//! Room and Info hold what an attached instrument reports, so they are absent without
//! one. Those two collapse independently, and their state is kept between sessions with
//! the docks.
//!
//! Nothing here asks for data the rest of the app does not already have; a card with no
//! data says so.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui;

use nord_usb::wire::Dependency;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, bold, caption, good, tint, ui as ui_text, warn};
use crate::browser::{Act, Browser, Bulk, Item, Kind, Offer};
use crate::device::{fit, occupancy, Device, Fit};
use crate::icon::{icon, painted, Glyph};
use crate::library::{row_of, Row, Where};
use crate::panel::{cut, signal_pill, tonal_button, GAP, GUTTER, INNER_RADIUS};
use crate::queue::{Diff, Queue};
use crate::room;
use crate::shell::Shell;
use crate::strings::{kind_word, place};
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

/// The selection's cards, then the instrument's while one is attached.
pub fn ui(
    ui: &mut egui::Ui,
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
            egui::Frame::new().inner_margin(GUTTER).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = GUTTER;
                let picked = &mut shell.picked;
                selection(ui, picked, browser, workspace, device, queue, &mut acts);
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
        });
    acts
}

/// What a card's header shows.
struct Head<'a> {
    glyph: Glyph,
    title: &'a str,
    /// A short word at the right end, in its own color.
    badge: Option<(String, egui::Color32)>,
    /// Whether the card is open, for a card that collapses; `None` for one that does not.
    open: Option<&'a mut bool>,
}

impl<'a> Head<'a> {
    /// The header of a card that is always open and has no badge.
    fn fixed(glyph: Glyph, title: &'a str) -> Head<'a> {
        Head {
            glyph,
            title,
            badge: None,
            open: None,
        }
    }
}

/// A card inside the dock: a header, and under it the body while the card is open.
fn card(ui: &mut egui::Ui, head: Head, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .corner_radius(INNER_RADIUS)
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

/// What the selection's cards show, taken again only when the selection or something it
/// describes changes.
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
}

/// What the selection card says about what is picked.
enum About {
    Nothing,
    One(Vec<Fact>),
    /// Several rows, in a line or two.
    Several(Vec<String>),
}

impl Picked {
    /// What the cards show now, taken again where something it reads has changed.
    fn now(
        &mut self,
        browser: &Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
    ) -> &Shown {
        let sources = Sources {
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
        let checked = as_listed(browser.picked().items(), workspace);
        let locals: Vec<u64> = checked.iter().copied().filter_map(Item::local).collect();
        let (about, going) = match checked.len() {
            0 => (About::Nothing, None),
            many if many > MANY => (About::Several(summed(&checked, workspace, device)), None),
            _ => {
                let rows: Vec<Row> = checked
                    .iter()
                    .filter_map(|item| {
                        row_of(*item, workspace, &device.state, queue, browser.tags())
                    })
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
        Shown {
            sources,
            checked,
            locals,
            about,
            going,
            offers,
            wearing,
        }
    }
}

/// The selection: what it is and what can be asked of it, how it is tagged, and what it
/// plays, each in its own card.
fn selection(
    ui: &mut egui::Ui,
    picked: &mut Picked,
    browser: &mut Browser,
    workspace: &Workspace,
    device: &Device,
    queue: &Queue,
    acts: &mut Vec<Act>,
) {
    let shown = picked.now(browser, workspace, device, queue);
    card(ui, Head::fixed(Glyph::ScanEye, "Selection"), |ui| {
        match &shown.about {
            About::Nothing => return faint(ui, "Nothing is selected."),
            About::One(facts) => facts.iter().for_each(|fact| fact_line(ui, fact)),
            About::Several(lines) => lines.iter().for_each(|line| void(ui, line)),
        }
        bulk_actions(ui, browser, shown, device, acts);
    });
    if shown.checked.is_empty() {
        return;
    }
    tags(ui, &shown.locals, &shown.wearing, acts);
    if let Some((slot, deps)) = answered(browser, device) {
        dependencies(ui, slot, deps);
    }
}

/// One line about a single picked asset: its label, its value, and the full text when
/// the value is shortened.
pub struct Fact {
    pub what: &'static str,
    pub said: String,
    pub hint: Option<String>,
}

/// The facts about the single picked asset.
///
/// The row is the library table's own, so these facts match the table. `fit` is the
/// attached instrument's verdict on the asset, which gets a line only when it refuses.
pub fn facts(row: &Row, fit: &Fit) -> Vec<Fact> {
    let mut said = vec![
        Fact {
            what: "name",
            said: crate::strings::display_name(&row.name).to_string(),
            hint: (crate::strings::display_name(&row.name) != row.name).then(|| row.name.clone()),
        },
        Fact {
            what: "kind",
            said: kind_word(row.kind, row.qualifier),
            hint: None,
        },
        Fact {
            what: "where",
            said: row.where_.short().to_string(),
            hint: Some(row.where_.sentence().to_string()),
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
    let held = row
        .item
        .local()
        .and_then(|id| workspace.get(id))
        .map(|entity| fit(&device.state, entity))
        .unwrap_or(Fit::Unattached);
    About::One(facts(row, &held))
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
/// stands for is that asset's row.
fn as_listed(items: impl Iterator<Item = Item>, workspace: &Workspace) -> Vec<Item> {
    let items: Vec<Item> = items.collect();
    if !items.iter().any(|item| matches!(item, Item::Slot { .. })) {
        return items;
    }
    let mut spots = BTreeMap::new();
    for entity in workspace.listed() {
        if let Some((class, at)) = entity.spot() {
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
                    "this names a {} the instrument has not listed by id",
                    Kind::from_class(class).chip()
                ));
            }
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
/// ⚠️ Toggling only. Tags are created, renamed, and removed in the browser's Tags
/// section, and added to something new from the row's Tag menu.
fn tags(ui: &mut egui::Ui, picked: &[u64], wearing: &[(u64, String, bool)], acts: &mut Vec<Act>) {
    if picked.is_empty() {
        return;
    }
    card(ui, Head::fixed(Glyph::Tags, "Tags"), |ui| {
        let made = tonal_button(ui, Some(Glyph::Plus), "New tag")
            .on_hover_text("a new tag on everything selected, named as you type");
        if made.clicked() {
            acts.push(Act::NewTag(picked.to_vec()));
        }
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
/// ⚠️ Only tags in use, in list order. A tag nothing picked has is not part of this
/// selection's state: the full list belongs to the tree, and adding a new tag belongs to
/// the row's Tag menu.
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
    use crate::testing::{self, context, Bench};
    use crate::workspace::Fresh;
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
                        super::ui(panel, shell, &mut browser, &workspace, device, &queue);
                        if let Some((slot, deps)) = answered(&browser, device) {
                            dependencies(panel, slot, deps);
                        }
                        tags(panel, &[7, 8], &wearing(&[7, 8], &labels), &mut Vec::new());
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
            where_: Where::Both(Some(false)),
            at: None,
            size: 2_048,
            needs: crate::library::Needs::Nothing,
        };

        let said = facts(&row, &Fit::Takes);
        let lines: Vec<(&str, &str)> = said
            .iter()
            .map(|fact| (fact.what, fact.said.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                ("name", "Africa Split"),
                ("kind", "Stage 4 program"),
                ("where", "both ≠"),
                ("size", "2.0 kB"),
            ]
        );
        assert_eq!(
            said[2].hint.as_deref(),
            Some(Where::Both(Some(false)).sentence()),
            "the hover gives the full sentence"
        );

        // The only fact not taken from the row is the instrument's refusal.
        let why = "This is a Stage 4 file and the instrument is a Nord Electro 5D.";
        let refused = facts(&row, &Fit::Refuses(why.into()));
        assert_eq!(refused.len(), said.len() + 1);
        assert_eq!(refused[4].what, "refused");
        assert_eq!(refused[4].said, why);
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
            size: 0,
            needs: crate::library::Needs::Nothing,
        };
        let rows = [
            row("kept", false, Where::Computer),
            row("edited", true, Where::Both(Some(false))),
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
                    super::ui(panel, shell, browser, workspace, device, queue);
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
        assert!(said.iter().any(|word| word == "keyboard"), "{said:?}");
        assert!(copy(&bench), "a slot on the keyboard only can be copied");

        let name = "Africa Split.ne5p".to_string();
        bench
            .workspace
            .ingest(name, origin(), bytes, &mut bench.log);
        bench.device.relink(&mut bench.workspace);
        let said = inspected(&mut bench);
        assert!(said.iter().any(|word| word.starts_with("both")), "{said:?}");
        assert!(!said.iter().any(|word| word == "keyboard"), "{said:?}");
        assert!(!copy(&bench), "a copy is already on this computer");
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
        // A dependency returned with a blank name has only its id to show.
        assert_eq!(
            device
                .state
                .dependency_name(ObjectClass::Sample, 0x0999_0999),
            None,
        );
    }

    #[test]
    fn the_selections_tags_are_chips_beside_new_tag_and_nothing_without_a_selection() {
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
                        tags(panel, picked, &wearing(picked, &labels), &mut Vec::new())
                    });
            });
            testing::words(&output)
        };
        let said = painted(&[7, 8]);
        for name in ["Sunday", "Loud", "New tag"] {
            assert!(said.contains(&name.to_string()), "{name}: {said:?}");
        }
        let untagged = painted(&[9]);
        assert!(untagged.contains(&"New tag".to_string()), "{untagged:?}");
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
                        super::ui(panel, shell, browser, &workspace, &device, &queue);
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

    /// New tag in the Tags card puts a new tag on everything selected, and nothing else.
    #[test]
    fn new_tag_in_the_card_tags_the_selection() {
        let ctx = context();
        egui_extras::install_image_loaders(&ctx);
        let labels = Tags::default();
        let mut acts = Vec::new();
        let frame = |events: Vec<egui::Event>, acts: &mut Vec<Act>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let output = testing::run(&ctx, input, |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| {
                        tags(panel, &[7, 8], &wearing(&[7, 8], &labels), acts)
                    });
            });
            testing::where_(&testing::painted(&output), "New tag").center()
        };
        let button = frame(Vec::new(), &mut acts);
        frame(testing::click(button), &mut acts);
        assert!(
            matches!(acts.as_slice(), [Act::NewTag(ids)] if ids == &[7, 8]),
            "one new tag on the selection; got {} acts",
            acts.len()
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
        assert!(
            !bench.browser.tags().on_all(&ids, tag),
            "hollow to begin with"
        );

        bench.act(vec![Act::Tag {
            ids: ids.clone(),
            tag,
        }]);
        assert!(
            bench.browser.tags().on_all(&ids, tag),
            "a hollow one goes on all"
        );

        bench.act(vec![Act::Untag {
            ids: ids.clone(),
            tag,
        }]);
        assert!(
            !bench.browser.tags().on_all(&ids, tag),
            "a solid one comes off"
        );
    }
}
