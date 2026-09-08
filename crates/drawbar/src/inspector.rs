//! The inspector: how much room the instrument has, what the selection needs, and what
//! it is labelled with.
//!
//! Three panels, each collapsed on its own and each kept between sessions beside the
//! docks. Nothing here reads anything the rest of the app has not already been told —
//! a panel with nothing behind it says so rather than filling itself in.

use eframe::egui;

use nord_usb::{Location, ObjectClass};

use crate::app::{accent, good, ui as ui_text};
use crate::browser::{Act, Browser, Item, Kind};
use crate::device::{occupancy, Device, BROWSED};
use crate::icon::{painted, Glyph};
use crate::panel::panel_header;
use crate::queue::Queue;
use crate::room;
use crate::shell::Shell;
use crate::strings::{folder, place};
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

/// The three panels, in the order the design stacks them.
pub fn ui(
    ui: &mut egui::Ui,
    shell: &mut Shell,
    browser: &mut Browser,
    workspace: &Workspace,
    device: &Device,
    queue: &Queue,
) -> Vec<Act> {
    let mut acts = Vec::new();
    panel_header(ui, "room", Some(&mut shell.room_open), None);
    if shell.room_open {
        room_panel(ui, workspace, device, queue);
    }
    panel_header(ui, "dependencies", Some(&mut shell.deps_open), None);
    if shell.deps_open {
        dependencies(ui, &slots(browser), device);
    }
    panel_header(ui, "tags", Some(&mut shell.tags_open), None);
    if shell.tags_open {
        tags(ui, &browser.picked().locals(), browser.tags(), &mut acts);
    }
    acts
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
        for class in BROWSED {
            let unit = device.state.allocation_unit(class);
            let Some(held) = room::meter(class, &device.state.inventory, unit, queue, workspace)
            else {
                continue;
            };
            drawn += 1;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(folder(class)).text_style(ui_text()));
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
            ui.add_space(GAP);
        }
        crate::browser::about(ui, device);
    });
}

/// What the instrument said the picked slots need.
///
/// ⚠️ `DEPENDENCIES` answers for one slot at a time and the cache holds the last answer,
/// so this speaks for the slot that was asked about and for no other. A selection nothing
/// has asked about says so, rather than saying that it needs nothing.
fn dependencies(ui: &mut egui::Ui, picked: &[(ObjectClass, Location)], device: &Device) {
    body(ui, |ui| {
        let mut drawn = 0;
        for (class, at) in picked.iter().copied() {
            if device.state.detail.at != Some(at) {
                continue;
            }
            let Some(deps) = device.state.detail.deps.as_ref() else {
                continue;
            };
            for dep in deps {
                drawn += 1;
                let named = device
                    .state
                    .dependency_name(Some((class, at)), dep.class, dep.id)
                    .filter(|name| !name.is_empty());
                needed(ui, dep.class, named, dep.id);
            }
            if drawn > 0 {
                ui.label(
                    egui::RichText::new(place(class, at))
                        .monospace()
                        .size(MONO)
                        .weak(),
                );
            }
        }
        if drawn == 0 {
            faint(ui, "Nothing here says what the selection plays.");
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
                );
            }
        }
    });
}

/// Every tag, filled where the whole selection wears it and outlined where it does not.
///
/// ⚠️ Only a **kept** asset can wear one — a tag hangs on a workspace id, and a slot has
/// none — so this is over what of the selection is on this computer.
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
        let new = ui
            .horizontal(|ui| {
                mark(ui, Glyph::Plus, PIP, ui.visuals().weak_text_color());
                ui.add(
                    egui::Label::new(egui::RichText::new("new tag").text_style(ui_text()).weak())
                        .sense(egui::Sense::click()),
                )
                .clicked()
            })
            .inner;
        if new {
            acts.push(Act::NewTag("New tag".into()));
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
        device.pretend_unit(ObjectClass::Sample, 131_064);
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

    /// Draw the whole inspector, and the two panels that take a selection with one.
    ///
    /// Nothing checks pixels. What this catches is a layout that panics or an id that
    /// collides, neither of which a test on the rules would see.
    fn paint(shell: &mut Shell, device: &Device, picked: &[(ObjectClass, Location)]) {
        let ctx = context();
        let mut browser = Browser::default();
        let workspace = Workspace::new(ctx.clone());
        let queue = Queue::default();
        let mut labels = Tags::default();
        let sunday = labels.make("Sunday");
        labels.set(7, sunday, true);
        // Twice: the second pass runs with the widget state the first left behind.
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::right("inspector")
                    .exact_width(crate::shell::INSPECTOR)
                    .show(ctx, |panel| {
                        super::ui(panel, shell, &mut browser, &workspace, device, &queue);
                        dependencies(panel, picked, device);
                        tags(panel, &[7, 8], &labels, &mut Vec::new());
                    });
            });
        }
    }

    /// Every panel draws, with an instrument answering and with nothing attached at all.
    #[test]
    fn the_three_panels_paint_with_and_without_an_instrument() {
        let ctx = context();
        let mut shell = Shell::default();
        paint(&mut shell, &Device::new(ctx.clone()), &[]);

        let (device, at) = attached(&ctx);
        paint(&mut shell, &device, &[(ObjectClass::Program, at)]);

        // Shut, every panel draws its header and nothing under it.
        let mut shut = Shell {
            room_open: false,
            deps_open: false,
            tags_open: false,
            ..Shell::default()
        };
        paint(&mut shut, &device, &[(ObjectClass::Program, at)]);
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
            deps_open: true,
            tags_open: false,
            ..Shell::default()
        };
        before.keep(&mut store);

        let mut after = Shell::default();
        after.restore(&store);
        assert!(!after.room_open);
        assert!(after.deps_open);
        assert!(!after.tags_open);
    }
}
