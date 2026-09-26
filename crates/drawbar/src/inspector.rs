//! The right dock: the selection, and the instrument while one is attached.
//!
//! The dock has two headers. SELECTION describes the rows selected in the browser,
//! whether or not an instrument is connected. It is flat, because a fact, a tag, and a
//! dependency are lines about one selection, not separate panels. INSTRUMENT holds what
//! an attached instrument reports, so it is absent without one. ROOM and INFO under it
//! collapse independently, and their state is kept between sessions with the docks.
//!
//! Nothing here asks for data the rest of the app does not already have; a panel with no
//! data says so.

use eframe::egui;

use nord_usb::wire::Dependency;
use nord_usb::{Location, ObjectClass};

use crate::app::{good, ui as ui_text};
use crate::browser::{Act, Browser, Item, Kind};
use crate::device::{fit, occupancy, Device, Fit};
use crate::icon::{painted, Glyph};
use crate::library::{row_of, Row, Where};
use crate::panel::{chip, dock_header, panel_header};
use crate::queue::Queue;
use crate::room;
use crate::shell::Shell;
use crate::strings::{kind_word, place};
use crate::tags::Tags;
use crate::workspace::Workspace;

/// A panel body's padding at each end, and the gap between its parts.
const PAD: i8 = 8;
const GAP: f32 = 6.0;

/// The size of a glyph in a line, and of the smaller one on a tag chip.
const GLYPH: f32 = 12.0;
const TAG: f32 = 11.0;

/// The size of monospace readouts: a meter's figures, a slot label, an id.
const MONO: f32 = 10.5;

/// The width of a fact's label column, so the values line up.
const FACT: f32 = 52.0;

/// The dock's two headers, top to bottom.
pub fn ui(
    ui: &mut egui::Ui,
    shell: &mut Shell,
    browser: &mut Browser,
    workspace: &Workspace,
    device: &Device,
    queue: &Queue,
) -> Vec<Act> {
    dock_header(ui, "selection");
    let acts = selection(ui, browser, workspace, device, queue);
    if !device.state.connected() {
        return acts;
    }
    dock_header(ui, "instrument");
    panel_header(ui, "room", Some(&mut shell.room_open), None);
    if shell.room_open {
        room_panel(ui, workspace, device, queue);
    }
    panel_header(ui, "info", Some(&mut shell.info_open), None);
    if shell.info_open {
        body(ui, |ui| crate::browser::about(ui, device));
    }
    acts
}

/// The selection: what it is, how it is tagged, and what it plays, as lines under one
/// header.
fn selection(
    ui: &mut egui::Ui,
    browser: &Browser,
    workspace: &Workspace,
    device: &Device,
    queue: &Queue,
) -> Vec<Act> {
    let mut acts = Vec::new();
    let picked = browser.picked().items().count();
    if picked == 0 {
        body(ui, |ui| faint(ui, "Nothing is selected."));
        return acts;
    }
    let rows: Vec<Row> = browser
        .picked()
        .items()
        .filter_map(|item| row_of(item, workspace, &device.state, queue, browser.tags()))
        .collect();

    body(ui, |ui| {
        about_selection(ui, picked, &rows, workspace, device)
    });
    tags(ui, &browser.picked().locals(), browser.tags(), &mut acts);
    // Only a slot the instrument has been asked about has a dependency list; for
    // everything else the lines are absent.
    let answered = needs(&slots(browser), device);
    if !answered.is_empty() {
        dependencies(ui, &answered, device);
    }
    acts
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
            said: kind_word(row.kind, row.family),
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

/// The summary of a multiple selection: how many rows are picked, how many are unsaved,
/// and how many are on the keyboard.
///
/// ⚠️ `picked` counts the rows the browser holds; the other two count only assets, which
/// excludes folder and tag rows.
pub fn tally(picked: usize, rows: &[Row]) -> String {
    let unsaved = rows.iter().filter(|row| row.unsaved).count();
    let keyboard = rows
        .iter()
        .filter(|row| matches!(row.where_, Where::Both(_) | Where::Keyboard))
        .count();
    format!("{picked} selected, {unsaved} unsaved, {keyboard} on the keyboard")
}

/// The facts about one picked row, or a summary of several.
fn about_selection(
    ui: &mut egui::Ui,
    picked: usize,
    rows: &[Row],
    workspace: &Workspace,
    device: &Device,
) {
    let [row] = rows else {
        return void(ui, tally(picked, rows));
    };
    if picked > 1 {
        // A folder picked beside one asset is still several picked.
        return void(ui, tally(picked, rows));
    }
    let held = row
        .item
        .local()
        .and_then(|id| workspace.get(id))
        .map(|entity| fit(&device.state, entity))
        .unwrap_or(Fit::Unattached);
    for fact in facts(row, &held) {
        fact_line(ui, fact);
    }
}

/// One fact: its label in a fixed column and its full value beside it.
///
/// ⚠️ The value wraps instead of truncating. A refusal, a location sentence, or a name
/// can be any length, and a truncated one is misleading.
fn fact_line(ui: &mut egui::Ui, fact: Fact) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [FACT, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(fact.what).text_style(ui_text()).weak())
                .halign(egui::Align::LEFT),
        );
        let said =
            ui.add(egui::Label::new(egui::RichText::new(&fact.said).text_style(ui_text())).wrap());
        if let Some(hint) = fact.hint {
            said.on_hover_text(hint);
        }
    });
}

/// A line about the selection as a whole.
fn void(ui: &mut egui::Ui, said: String) {
    ui.label(egui::RichText::new(said).text_style(ui_text()));
}

/// The slots the selection holds. Only a slot has a dependency list on the instrument.
fn slots(browser: &Browser) -> Vec<(ObjectClass, Location)> {
    browser
        .picked()
        .items()
        .filter_map(|item| match item {
            Item::Slot { class, at } => Some((class, at)),
            _ => None,
        })
        .collect()
}

/// One meter per folder the instrument has counted, and a sentence on the limit the
/// queue runs into.
fn room_panel(ui: &mut egui::Ui, workspace: &Workspace, device: &Device, queue: &Queue) {
    body(ui, |ui| {
        let mut drawn = 0;
        for class in device.state.classes() {
            let unit = device.state.allocation_unit(class);
            let banks = device.state.banks(class);
            let Some(held) = room::meter(
                class,
                &device.state.inventory,
                unit,
                banks,
                queue,
                workspace,
            ) else {
                continue;
            };
            drawn += 1;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(device.state.folder_name(class)).text_style(ui_text()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // ⚠️ The bar takes the status color and the readout stays in the text
                    // color: the signal colors are not legible as 10 px figures.
                    if let Some(said) = occupancy(class, &device.state.inventory, unit) {
                        ui.label(egui::RichText::new(said).monospace().size(MONO).weak());
                    }
                });
            });
            room::bar(ui, held);
            ui.add_space(GAP);
        }
        if drawn == 0 {
            return faint(ui, "The instrument's contents have not been counted.");
        }
        if let Some(said) = room::constraint(queue, workspace, &device.state) {
            ui.label(egui::RichText::new(said).text_style(ui_text()).weak());
        }
    });
}

/// The dependency list the instrument gave for `slot`, if it is the slot last asked
/// about and the list is not empty.
///
/// ⚠️ `DEPENDENCIES` answers for one slot at a time and the cache holds the last answer,
/// so it applies only to that class at that address. A slot nobody asked about has no
/// answer, which is not the same as needing nothing.
fn answer(device: &Device, slot: (ObjectClass, Location)) -> Option<&[Dependency]> {
    if device.state.detail.at != Some(slot) {
        return None;
    }
    let deps = device.state.detail.deps.as_deref()?;
    (!deps.is_empty()).then_some(deps)
}

/// The picked slots the instrument has answered about. The panel appears only when there
/// is at least one.
fn needs(picked: &[(ObjectClass, Location)], device: &Device) -> Vec<(ObjectClass, Location)> {
    picked
        .iter()
        .copied()
        .filter(|slot| answer(device, *slot).is_some())
        .collect()
}

/// What the instrument said the picked slots need.
fn dependencies(ui: &mut egui::Ui, answered: &[(ObjectClass, Location)], device: &Device) {
    body(ui, |ui| {
        for (class, at) in answered.iter().copied() {
            let Some(deps) = answer(device, (class, at)) else {
                continue;
            };
            for dep in deps {
                let named = Some(dep.name.trim()).filter(|name| !name.is_empty());
                needed(ui, dep.class, named, dep.id);
            }
            ui.label(
                egui::RichText::new(place(class, at))
                    .monospace()
                    .size(MONO)
                    .weak(),
            );
        }
    });
}

/// One library a slot needs: the name the instrument gave it, or its bare id when that
/// is all there is.
fn needed(ui: &mut egui::Ui, class: ObjectClass, named: Option<&str>, id: u32) {
    ui.horizontal(|ui| {
        let quiet = ui.visuals().weak_text_color();
        let lit = good(ui.visuals());
        mark(ui, Kind::from_class(class).glyph(), GLYPH, quiet);
        match named {
            Some(name) => {
                ui.label(egui::RichText::new(name).text_style(ui_text()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("installed")
                            .text_style(ui_text())
                            .color(lit),
                    );
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
/// ⚠️ Toggling only. Tags are created, renamed, and removed in the browser's TAGS
/// section, and added to something new from the row's Tag menu.
fn tags(ui: &mut egui::Ui, picked: &[u64], worn: &Tags, acts: &mut Vec<Act>) {
    let wearing = wearing(picked, worn);
    if wearing.is_empty() {
        return;
    }
    body(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (id, name, on_all) in wearing {
                let visuals = ui.visuals().clone();
                let (tint, fill) = match on_all {
                    true => (visuals.text_color(), Some(visuals.faint_bg_color)),
                    false => (crate::app::caption(&visuals), None),
                };
                let drawn = chip(ui, Glyph::Tag, TAG, name, tint, fill);
                let clicked = ui
                    .interact(drawn.rect, drawn.id.with(id), egui::Sense::click())
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

/// The tags any picked asset has, and whether each is on all of them.
///
/// ⚠️ Only tags in use, in list order. A tag nothing picked has is not part of this
/// selection's state: the full list belongs to the tree, and adding a new tag belongs to
/// the row's Tag menu.
fn wearing<'a>(picked: &[u64], worn: &'a Tags) -> Vec<(u64, &'a str, bool)> {
    worn.all()
        .iter()
        .filter(|tag| picked.iter().any(|id| worn.worn(*id).contains(&tag.id)))
        .map(|tag| (tag.id, tag.name.as_str(), worn.on_all(picked, tag.id)))
        .collect()
}

/// A glyph in a line, claiming its own box so the words after it line up.
fn mark(ui: &mut egui::Ui, glyph: Glyph, size: f32, tint: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    painted(ui, glyph, rect, tint);
}

/// A panel's body: padded at each end, with its lines under each other.
fn body<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(PAD, 6))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(GAP, 4.0);
            contents(ui)
        })
        .inner
}

/// The line a panel shows when it has nothing to say.
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
    use crate::browser::apply;
    use crate::log::Log;
    use crate::tabs::Tabs;
    use crate::workspace::Fresh;
    use nord_usb::wire::{Dependency, Status};

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        ctx
    }

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
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| {
                        super::ui(panel, shell, &mut browser, &workspace, device, &queue);
                        dependencies(panel, picked, device);
                        tags(panel, &[7, 8], &labels, &mut Vec::new());
                    });
            });
            said = crate::browser::bench::words(&output);
        }
        said
    }

    #[test]
    fn instrument_is_headed_only_while_one_is_attached() {
        let ctx = context();
        let mut shell = Shell {
            info_open: true,
            ..Shell::default()
        };
        let detached = paint(&mut shell, &Device::new(ctx.clone()), &[]);
        assert!(
            detached.iter().any(|word| word == "SELECTION"),
            "{detached:?}"
        );
        assert!(
            !detached.iter().any(|word| word == "INSTRUMENT"),
            "{detached:?}"
        );

        let (device, at) = attached(&ctx);
        let held = paint(&mut shell, &device, &[(ObjectClass::Program, at)]);
        for header in ["SELECTION", "INSTRUMENT", "ROOM", "INFO"] {
            assert!(held.iter().any(|word| word == header), "{header}: {held:?}");
        }

        // Collapsed panels under INSTRUMENT draw only their headers.
        let mut shut = Shell {
            room_open: false,
            info_open: false,
            ..Shell::default()
        };
        paint(&mut shut, &device, &[(ObjectClass::Program, at)]);
    }

    #[test]
    fn the_facts_of_one_picked_asset_are_its_rows_own() {
        use nord_format::accept::Family;

        let row = Row {
            item: Item::Local(1),
            kind: Kind::Program,
            family: Some(Family::Stage4),
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
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |ui| {
                        fact_line(
                            ui,
                            Fact {
                                what: "refused",
                                said: said.to_string(),
                                hint: None,
                            },
                        );
                    });
            });
            crate::browser::bench::galleys(&output)
                .iter()
                .find(|galley| galley.text() == said)
                .map(|galley| galley.rows.len())
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
            family: None,
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

        // A picked folder is not an asset, so it counts only as picked.
        assert_eq!(tally(1, &[]), "1 selected, 0 unsaved, 0 on the keyboard");
    }

    /// ⚠️ Every class is addressed by the same banks and slots, so the sample at a
    /// program's address must not show the program's list.
    #[test]
    fn a_selection_the_instrument_was_not_asked_about_shows_nothing() {
        let ctx = context();
        let (device, at) = attached(&ctx);
        assert_eq!(
            needs(&[(ObjectClass::Program, at)], &device),
            [(ObjectClass::Program, at)],
            "the slot it was asked about"
        );
        assert!(
            needs(&[(ObjectClass::Sample, at)], &device).is_empty(),
            "another class at the same address"
        );
        let elsewhere = Location { bank: 0, slot: 0 };
        assert!(
            needs(&[(ObjectClass::Program, elsewhere)], &device).is_empty(),
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
    fn the_selections_tags_are_chips_and_nothing_at_all_where_there_are_none() {
        let ctx = context();
        let mut labels = Tags::default();
        let (both, some) = (labels.make("Sunday").unwrap(), labels.make("Loud").unwrap());
        for tag in [both, some] {
            labels.set(7, tag, true);
        }
        labels.set(8, both, true);

        assert_eq!(
            wearing(&[7, 8], &labels),
            [(both, "Sunday", true), (some, "Loud", false)]
        );
        assert!(wearing(&[], &labels).is_empty(), "nothing picked");
        assert!(wearing(&[9], &labels).is_empty(), "picked, with no tags");

        let painted = |picked: &[u64]| {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| tags(panel, picked, &labels, &mut Vec::new()));
            });
            crate::browser::bench::words(&output)
        };
        let said = painted(&[7, 8]);
        for name in ["Sunday", "Loud"] {
            assert!(said.contains(&name.to_string()), "{name}: {said:?}");
        }
        assert!(painted(&[9]).is_empty(), "{:?}", painted(&[9]));
    }

    #[test]
    fn a_tag_goes_on_the_whole_selection_and_comes_off_it_again() {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut browser = Browser::default();
        let mut queue = Queue::default();
        let mut tabs = Tabs::default();
        let mut log = Log::default();
        let mut shell = Shell::default();
        let ids: Vec<u64> = (0..2)
            .map(|_| workspace.create(Fresh::Program, &mut log).unwrap())
            .collect();

        let mut run = |browser: &mut Browser, acts| {
            apply(
                browser,
                &mut shell,
                acts,
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut queue,
                &mut log,
            )
        };
        run(&mut browser, vec![Act::NewTag("Sunday".into())]);
        let tag = browser.tags().all()[0].id;
        assert!(!browser.tags().on_all(&ids, tag), "hollow to begin with");

        run(
            &mut browser,
            vec![Act::Tag {
                ids: ids.clone(),
                tag,
            }],
        );
        assert!(browser.tags().on_all(&ids, tag), "a hollow one goes on all");

        run(
            &mut browser,
            vec![Act::Untag {
                ids: ids.clone(),
                tag,
            }],
        );
        assert!(!browser.tags().on_all(&ids, tag), "a solid one comes off");
    }
}
