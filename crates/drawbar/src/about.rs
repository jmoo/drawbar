//! About drawbar: what it is, where it lives, the licence it is offered under, and whose
//! trademarks the names in it are.
//!
//! The same box on every target. The licence is compiled in, so a binary handed to
//! someone carries the text of the terms it is under.

use eframe::egui;

use crate::shell::GUIDE;
use crate::splash::{link, title, GAP, WIDTH};

const REPO: &str = "https://github.com/jmoo/drawbar";
pub(crate) const RELEASES: &str = "https://github.com/jmoo/drawbar/releases";

const WHAT: &str = "A black-box Clavia / Nord reverse-engineering project in Rust that aims to be \
                    portable, complete, and well-tested.";

/// A copy of `crates/LICENSE`: a packaged crate cannot reach outside its own root, and
/// the test below keeps the two the same.
const LICENSE: &str = include_str!("../assets/LICENSE");

/// Required of every public face of the project — see `CONTRIBUTING.md`.
const DISCLAIMER: &str = "Not affiliated with, authorized, or endorsed by Clavia DMI AB. \
                          \"Nord\", \"Clavia\" and \"Electro\" are trademarks of Clavia DMI AB, \
                          used here only to identify the hardware these formats come from.";

/// The most of the modal the licence may claim, so the Close button stays on screen.
const TERMS: f32 = 200.0;

/// 10 px: the licence is hard-wrapped at 78 columns, and this is the size that fits them
/// in [`WIDTH`] without wrapping them a second time.
const MONO: f32 = 10.0;

/// The release a version's notes were published on.
pub fn release_page(version: &str) -> String {
    format!("{RELEASES}/tag/drawbar-v{version}")
}

/// Draw the box while it is open, and close it once the reader is done.
pub fn dialog(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }
    if egui::Modal::new(egui::Id::new("about"))
        .show(ctx, body)
        .inner
    {
        *open = false;
    }
}

/// Returns whether the reader is done with it.
fn body(ui: &mut egui::Ui) -> bool {
    ui.set_width(WIDTH);
    title(ui);
    ui.add_space(GAP);
    ui.label(WHAT);
    ui.add_space(GAP * 2.0);
    ui.horizontal(|ui| {
        link(ui, "Source on GitHub", REPO);
        link(ui, "User guide", GUIDE);
        link(ui, "Releases", RELEASES);
    });
    ui.add_space(GAP * 2.0);
    ui.separator();
    ui.add_space(GAP);
    ui.label(egui::RichText::new("BSD-3-Clause").strong());
    ui.add_space(GAP);
    egui::ScrollArea::vertical()
        .max_height(TERMS)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(LICENSE)
                    .font(egui::FontId::monospace(MONO))
                    .weak(),
            );
        });
    ui.add_space(GAP);
    ui.separator();
    ui.add_space(GAP);
    ui.label(egui::RichText::new(DISCLAIMER).small().weak());
    ui.add_space(GAP * 2.0);
    let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let closed = ui
        .horizontal(|ui| {
            ui.add(egui::Button::new(egui::RichText::new("Close").strong()))
                .clicked()
        })
        .inner;
    closed || escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_page_names_the_tag_a_version_was_cut_from() {
        assert_eq!(
            release_page("0.5.0"),
            "https://github.com/jmoo/drawbar/releases/tag/drawbar-v0.5.0"
        );
    }

    /// `include_str!` accepts whatever file it is pointed at, licence or not.
    #[test]
    fn the_compiled_in_licence_is_the_bsd_three_clause_text() {
        assert!(
            LICENSE.starts_with("BSD 3-Clause License"),
            "the box would show the wrong terms: {:?}",
            LICENSE.lines().next()
        );
        assert!(LICENSE.contains("Redistribution and use in source and binary forms"));
        assert!(LICENSE.contains("Copyright (c)"));
    }

    #[test]
    fn the_compiled_in_licence_is_the_workspace_licence() {
        let workspace = include_str!("../../LICENSE");
        assert_eq!(
            LICENSE, workspace,
            "crates/drawbar/assets/LICENSE differs from crates/LICENSE"
        );
    }

    #[test]
    fn the_disclaimer_disclaims_what_it_has_to() {
        for required in [
            "Not affiliated with, authorized, or endorsed by Clavia DMI AB",
            "trademarks of Clavia DMI AB",
        ] {
            assert!(DISCLAIMER.contains(required), "missing: {required}");
        }
    }
}
