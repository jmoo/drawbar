//! Shared setup for the tests in every part of the browser.

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use super::drag::{Carried, Item, Kind, Onto};
use super::Browser;
use crate::device::Device;
use crate::tabs::Tabs;
use crate::workspace::Workspace;

pub(in crate::browser) fn local(kind: Kind) -> Carried {
    Carried {
        from: Item::Local(1),
        kind,
        name: "Africa Split".into(),
        filed: None,
    }
}

pub(in crate::browser) fn slot(class: ObjectClass, bank: u32, slot: u32) -> Carried {
    Carried {
        from: Item::Slot {
            class,
            at: Location { bank, slot },
        },
        kind: Kind::from_class(class),
        name: "Squabble B".into(),
        filed: None,
    }
}

pub(in crate::browser) fn onto(class: ObjectClass, bank: u32, at: u32) -> Onto {
    Onto::Slot {
        class,
        at: Location { bank, slot: at },
    }
}

/// Everything an act needs run against it.
pub(in crate::browser) fn bench() -> (Browser, Workspace, Device, Tabs, crate::log::Log) {
    let ctx = egui::Context::default();
    (
        Browser::default(),
        Workspace::new(ctx.clone()),
        Device::new(ctx),
        Tabs::default(),
        crate::log::Log::default(),
    )
}
