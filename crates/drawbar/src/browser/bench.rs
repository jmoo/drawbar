//! Shared setup for the tests in every part of the browser.

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use super::drag::{Held, Item, Kind, Onto};
use super::Browser;
use crate::device::Device;
use crate::queue::Queue;
use crate::tabs::Tabs;
use crate::workspace::Workspace;

pub(in crate::browser) fn local(kind: Kind) -> Held {
    Held {
        what: Item::Local(1),
        kind,
        filed: None,
        fits: true,
    }
}

pub(in crate::browser) fn slot(class: ObjectClass, bank: u32, slot: u32) -> Held {
    Held {
        what: Item::Slot {
            class,
            at: Location { bank, slot },
        },
        kind: Kind::from_class(class),
        filed: None,
        fits: true,
    }
}

pub(in crate::browser) fn onto(class: ObjectClass, bank: u32, at: u32) -> Onto {
    Onto::Slot {
        class,
        at: Location { bank, slot: at },
    }
}

/// A context dressed the way `DrawbarApp::new` dresses one.
///
/// ⚠️ The named text styles a panel header resolves are installed there, on both faces.
/// A face that never learned them panics the frame that resolves one.
pub(in crate::browser) fn context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(crate::app::metrics);
    ctx
}

/// Every galley a frame painted, in the order it painted them.
pub(crate) fn galleys(output: &egui::FullOutput) -> Vec<std::sync::Arc<egui::Galley>> {
    fn walk(shape: &egui::Shape, into: &mut Vec<std::sync::Arc<egui::Galley>>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
            _ => {}
        }
    }
    let mut painted = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut painted);
    }
    painted
}

/// Every word a frame painted, in the order it painted them.
pub(crate) fn words(output: &egui::FullOutput) -> Vec<String> {
    galleys(output)
        .iter()
        .map(|galley| galley.text().to_string())
        .collect()
}

/// Everything an act needs run against it.
pub(in crate::browser) fn bench() -> (Browser, Workspace, Device, Tabs, Queue, crate::log::Log) {
    let ctx = context();
    (
        Browser::default(),
        Workspace::new(ctx.clone()),
        Device::new(ctx),
        Tabs::default(),
        Queue::default(),
        crate::log::Log::default(),
    )
}
