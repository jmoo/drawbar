//! The right dock: what is picked, and — while one is attached — the instrument.
//!
//! Two dock headers rather than a dock header and two groups. SELECTION heads the dock
//! itself and answers about the rows the browser has picked, whatever is on the bus; it
//! is flat, because a fact, a tag and a dependency are three lines about one selection
//! rather than three panels. INSTRUMENT heads what an attached instrument has to say,
//! so it is absent without one, and ROOM and INFO under it collapse on their own and are
//! kept between sessions beside the docks.
//!
//! Nothing here reads anything the rest of the app has not already been told — a panel
//! with nothing behind it says so rather than filling itself in.

use eframe::egui;

use nord_usb::wire::Dependency;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, good, ui as ui_text};
use crate::browser::{Act, Browser, Item, Kind};
use crate::device::{fit, occupancy, Device, Fit};
use crate::icon::{painted, Glyph};
use crate::library::{row_of, Row, Where};
use crate::panel::{dock_header, panel_header};
use crate::queue::Queue;
use crate::room;
use crate::shell::Shell;
use crate::strings::{kind_word, place};
use crate::tags::Tags;
use crate::workspace::Workspace;

/// The room a panel's body keeps at each end, and the gap between its parts.
const PAD: i8 = 8;
const GAP: f32 = 6.0;

/// A glyph in a line, and the mark a tag wears.
const GLYPH: f32 = 12.0;
const PIP: f32 = 9.0;

/// The mono readout beside a meter, and the words under one.
const MONO: f32 = 10.5;

/// The column a fact's own word takes, so the values under each other line up.
const FACT: f32 = 52.0;

/// The dock's two headers, in the order the design stacks them.
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

/// What is picked: what it is, what it is labelled with, and what it plays — one run of
/// lines under one header.
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
        body(ui, |ui| faint(ui, "Nothing is picked."));
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
    // Only a slot the instrument has been asked about has a dependency list, so the
    // lines are absent rather than empty for everything else.
    let answered = needs(&slots(browser), device);
    if !answered.is_empty() {
        dependencies(ui, &answered, device);
    }
    acts
}

/// One line of the FACTS panel: what it is, what it says, and the whole of it where the
/// short form leaves something out.
pub struct Fact {
    pub what: &'static str,
    pub said: String,
    pub hint: Option<String>,
}

/// What the panel says about the one asset that is picked.
///
/// The row is the table's own, so a fact here is the fact the table shows. `fit` is
/// what the attached instrument makes of it, and it is worth a line only where it
/// refuses: everything else is either silence or the row's own kind.
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

/// What the panel says about a selection of several: how many were picked, and what of
/// them the two things worth acting on hold.
///
/// ⚠️ `picked` counts the rows the browser holds; the other two count the assets among
/// them, which a folder or a tag row is not.
pub fn tally(picked: usize, rows: &[Row]) -> String {
    let unsaved = rows.iter().filter(|row| row.unsaved).count();
    let keyboard = rows
        .iter()
        .filter(|row| matches!(row.where_, Where::Both(_) | Where::Keyboard))
        .count();
    format!("{picked} picked, {unsaved} unsaved, {keyboard} on the keyboard")
}

/// The FACTS panel: one picked row read out, or a count of the several that are.
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

/// One fact: its own word in a fixed column, and the whole of what it says beside it.
///
/// ⚠️ The value wraps rather than truncating. A refusal, a where sentence and a name are
/// each as long as they are, and half of one is not a fact.
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

/// A line the panel says about the selection as a whole rather than about a field.
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

/// One meter per folder the instrument has counted, and the one sentence saying what the
/// queue runs into.
fn room_panel(ui: &mut egui::Ui, workspace: &Workspace, device: &Device, queue: &Queue) {
    body(ui, |ui| {
        let mut drawn = 0;
        for class in device.state.classes() {
            let unit = device.state.allocation_unit(class);
            let Some(held) = room::meter(class, &device.state.inventory, unit, queue, workspace)
            else {
                continue;
            };
            drawn += 1;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(device.state.folder_name(class)).text_style(ui_text()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // ⚠️ The bar takes the tone and the readout keeps its own ink: the
                    // signal colours do not carry as 10 px figures on the panel.
                    if let Some(said) = occupancy(class, &device.state.inventory, unit) {
                        ui.label(egui::RichText::new(said).monospace().size(MONO).weak());
                    }
                });
            });
            room::bar(ui, held);
            ui.add_space(GAP);
        }
        if drawn == 0 {
            return faint(ui, "Nothing has counted what is on the instrument.");
        }
        if let Some(said) = room::constraint(queue, workspace, &device.state) {
            ui.label(egui::RichText::new(said).text_style(ui_text()).weak());
        }
    });
}

/// The dependency list the instrument gave for `at`, where that is the slot it was last
/// asked about and it named something.
///
/// ⚠️ `DEPENDENCIES` answers for one slot at a time and the cache holds the last answer,
/// so this speaks for the slot that was asked about and for no other. A selection nothing
/// has asked about has no answer, which is not the same as needing nothing.
fn answer(device: &Device, at: Location) -> Option<&[Dependency]> {
    if device.state.detail.at != Some(at) {
        return None;
    }
    let deps = device.state.detail.deps.as_deref()?;
    (!deps.is_empty()).then_some(deps)
}

/// The picked slots the instrument has answered about — what the panel would have to
/// say, and so whether there is a panel at all.
fn needs(picked: &[(ObjectClass, Location)], device: &Device) -> Vec<(ObjectClass, Location)> {
    picked
        .iter()
        .copied()
        .filter(|(_, at)| answer(device, *at).is_some())
        .collect()
}

/// What the instrument said the picked slots need.
fn dependencies(ui: &mut egui::Ui, answered: &[(ObjectClass, Location)], device: &Device) {
    body(ui, |ui| {
        for (class, at) in answered.iter().copied() {
            let Some(deps) = answer(device, at) else {
                continue;
            };
            for dep in deps {
                let named = device
                    .state
                    .dependency_name(Some((class, at)), dep.class, dep.id)
                    .filter(|name| !name.is_empty());
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

/// One library a slot names: the name the instrument gave it, or the bare id nothing has
/// resolved.
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

/// Every tag, filled where the whole selection wears it and outlined where it does not.
///
/// ⚠️ Only a **kept** asset can wear one — a tag hangs on a workspace id, and a slot has
/// none — so this is over what of the selection is on this computer.
///
/// ⚠️ Toggling only. A tag is made in the browser's own TAGS section, which is where the
/// list of them lives and where one is renamed and removed.
fn tags(ui: &mut egui::Ui, picked: &[u64], worn: &Tags, acts: &mut Vec<Act>) {
    body(ui, |ui| {
        if picked.is_empty() {
            faint(ui, "Nothing on this computer is picked.");
        }
        for tag in worn.all() {
            let on_all = worn.on_all(picked, tag.id);
            let on_some = picked.iter().any(|id| worn.worn(*id).contains(&tag.id));
            let clicked = ui
                .horizontal(|ui| {
                    pip(ui, on_all);
                    ui.add(
                        egui::Label::new(egui::RichText::new(&tag.name).text_style(ui_text()))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(match (on_all, on_some) {
                        (true, _) => "on everything picked — click to take it off",
                        (false, true) => "on some of what is picked — click to put it on all",
                        (false, false) => "click to put it on everything picked",
                    })
                    .clicked()
                })
                .inner;
            if clicked && !picked.is_empty() {
                let ids = picked.to_vec();
                acts.push(match on_all {
                    true => Act::Untag { ids, tag: tag.id },
                    false => Act::Tag { ids, tag: tag.id },
                });
            }
        }
    });
}

/// A tag's own mark: filled where the whole selection wears it, outlined where it does
/// not.
fn pip(ui: &mut egui::Ui, solid: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(PIP), egui::Sense::hover());
    let tint = accent(ui.visuals());
    match solid {
        true => ui.painter().circle_filled(rect.center(), PIP / 2.0, tint),
        false => ui.painter().circle_stroke(
            rect.center(),
            PIP / 2.0 - 0.5,
            egui::Stroke::new(1.0_f32, tint),
        ),
    };
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

/// The line a panel shows where there is nothing to say.
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
    use crate::device::Detail;
    use crate::log::Log;
    use crate::store::Fake;
    use crate::tabs::Tabs;
    use crate::workspace::Fresh;
    use nord_usb::wire::{Dependency, Status};

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        ctx
    }

    /// An instrument that has counted a library, holds a program, and has been asked what
    /// one of its slots plays — one library it named and one it did not.
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
        device.state.detail = Detail {
            at: Some(at),
            info: None,
            asked: true,
            deps: Some(vec![
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
            ]),
        };
        (device, at)
    }

    /// Draw the whole inspector over `picked`, and the two panels that take a selection
    /// of their own.
    ///
    /// Nothing checks pixels. What this catches is a layout that panics or an id that
    /// collides, neither of which a test on the rules would see.
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
        let sunday = labels.make("Sunday");
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

    /// SELECTION heads the dock whatever is on the bus; INSTRUMENT is only there while
    /// one is attached, because everything under it is something an instrument said.
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

        // Shut, each panel under INSTRUMENT draws its header and nothing under it.
        let mut shut = Shell {
            room_open: false,
            info_open: false,
            ..Shell::default()
        };
        paint(&mut shut, &device, &[(ObjectClass::Program, at)]);
    }

    /// The facts about one picked asset are the ones its library row already carries,
    /// so the panel and the table cannot disagree.
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
            "the short word carries the whole of it"
        );

        // The one thing the panel says that the row does not know: what the attached
        // instrument makes of it, and only where that is a refusal.
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

    /// ⚠️ A refusal is a sentence, and a sentence the dock cannot fit on one line is laid
    /// out on more of them rather than cut off at the panel's edge.
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

    /// Several picked is a count rather than a reading-out, and the two counts are the
    /// two that decide what can be done with the set.
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
        assert_eq!(tally(3, &rows), "3 picked, 1 unsaved, 2 on the keyboard");

        // A folder is picked and is no asset, so it is counted as picked and as nothing
        // else.
        assert_eq!(tally(1, &[]), "1 picked, 0 unsaved, 0 on the keyboard");
    }

    /// ⚠️ A dependency list answers for the slot it was asked about and for no other, so
    /// a selection elsewhere is *not asked* rather than *needs nothing*.
    #[test]
    fn a_selection_the_instrument_was_not_asked_about_shows_nothing() {
        let ctx = context();
        let (device, at) = attached(&ctx);
        assert_eq!(
            device.state.dependency_name(
                Some((ObjectClass::Program, at)),
                ObjectClass::Piano,
                0x0102_0304
            ),
            Some("Royal Grand 3D"),
        );
        let elsewhere = Location { bank: 0, slot: 0 };
        assert_eq!(
            device.state.dependency_name(
                Some((ObjectClass::Program, elsewhere)),
                ObjectClass::Piano,
                0x0102_0304
            ),
            None,
        );
        // And the one whose name came back blank has nothing but its id to show.
        assert_eq!(
            device
                .state
                .dependency_name(
                    Some((ObjectClass::Program, at)),
                    ObjectClass::Sample,
                    0x0999_0999
                )
                .filter(|name| !name.is_empty()),
            None,
        );
    }

    /// A tag goes on everything picked and comes off it again, which is the whole of what
    /// the panel asks for.
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

    /// What the last session left collapsed comes back collapsed, panel by panel.
    #[test]
    fn each_inspector_panel_comes_back_as_it_was_left() {
        let mut store = Fake::default();
        let before = Shell {
            room_open: false,
            info_open: true,
            ..Shell::default()
        };
        before.keep(&mut store);

        let mut after = Shell::default();
        after.restore(&store);
        assert!(!after.room_open);
        assert!(after.info_open);
    }
}
