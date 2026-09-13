//! About drawbar: what it is, where it lives, the licences of what is compiled into it,
//! and whose trademarks the names in it are.
//!
//! The same box on every target. The licences are compiled in, so a binary handed to
//! someone carries the text of the terms it is under.

use eframe::egui;

use crate::shell::GUIDE;
use crate::splash::{link, title, GAP, WIDTH};

const REPO: &str = "https://github.com/jmoo/drawbar";
pub(crate) const RELEASES: &str = "https://github.com/jmoo/drawbar/releases";

const WHAT: &str = "A black-box Clavia / Nord reverse-engineering project in Rust that aims to be \
                    portable, complete, and well-tested.";

/// What one licence covers in the app, and its terms.
struct Notice {
    covers: &'static str,
    /// Where bundled material came from; `None` for drawbar itself.
    source: Option<&'static str>,
    licence: &'static str,
    text: &'static str,
}

/// Every licence whose terms require its notice to travel with a copy of the app: drawbar
/// first, then the bundled material by what it covers, alphabetically.
const NOTICES: &[Notice] = &[
    Notice {
        covers: "drawbar",
        source: None,
        licence: "BSD 3-Clause",
        // A copy of `crates/LICENSE`: a packaged crate cannot reach outside its own root,
        // and a test keeps the two the same.
        text: include_str!("../assets/LICENSE"),
    },
    Notice {
        covers: "emoji-icon-font",
        source: Some("egui's epaint_default_fonts 0.32.3"),
        licence: "MIT",
        text: include_str!("../assets/fonts/egui/emoji-icon-font-mit-license.txt"),
    },
    Notice {
        covers: "Hack Regular",
        source: Some("egui's epaint_default_fonts 0.32.3"),
        licence: "MIT and Bitstream Vera",
        text: include_str!("../assets/fonts/egui/Hack-Regular.txt"),
    },
    Notice {
        covers: "Lucide icons",
        source: Some("Lucide 0.469.0"),
        licence: "ISC",
        text: include_str!("../assets/icons/LICENSE"),
    },
    Notice {
        covers: "Noto Emoji Regular",
        source: Some("egui's epaint_default_fonts 0.32.3"),
        licence: "SIL Open Font License 1.1",
        text: include_str!("../assets/fonts/egui/OFL.txt"),
    },
    Notice {
        covers: "Ubuntu Regular, Bold and Light",
        source: Some("Ubuntu font family 0.83; Light from egui's epaint_default_fonts 0.32.3"),
        licence: "Ubuntu Font Licence 1.0",
        text: include_str!("../assets/fonts/LICENCE.txt"),
    },
];

/// Required of every public face of the project — see `CONTRIBUTING.md`.
const DISCLAIMER: &str = "Not affiliated with, authorized, or endorsed by Clavia DMI AB. \
                          \"Nord\", \"Clavia\" and \"Electro\" are trademarks of Clavia DMI AB, \
                          used here only to identify the hardware these formats come from.";

/// The height the modal needs around the list — title, links, disclaimer and Close — so
/// an open licence scrolls inside the list rather than pushing Close off-screen.
const AROUND: f32 = 320.0;

/// The list is never shorter than this, however short the window.
const LICENCES: f32 = 160.0;

/// 10 px: the widest hard-wrapped text is 78 columns, and this is the size that fits them
/// in [`WIDTH`] without wrapping them a second time. Texts with longer lines wrap to it.
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
    ui.label(egui::RichText::new("Licences").strong());
    ui.add_space(GAP);
    egui::ScrollArea::vertical()
        .id_salt("licences")
        .max_height((ui.ctx().screen_rect().height() - AROUND).max(LICENCES))
        .show(ui, |ui| {
            for notice in NOTICES {
                row(ui, notice);
            }
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

/// Collapsed, what a licence covers and its name; open, where that came from and the terms.
fn row(ui: &mut egui::Ui, notice: &Notice) {
    let id = ui.make_persistent_id(notice.covers);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            ui.label(notice.covers);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The list's scrollbar floats over its content; keep the name out from under it.
                let scroll = ui.spacing().scroll;
                ui.add_space(scroll.bar_width + scroll.bar_outer_margin);
                ui.label(egui::RichText::new(notice.licence).weak());
            });
        })
        .body(|ui| {
            if let Some(source) = notice.source {
                ui.label(egui::RichText::new(source).small().weak());
                ui.add_space(GAP);
            }
            ui.label(
                egui::RichText::new(notice.text)
                    .font(egui::FontId::monospace(MONO))
                    .weak(),
            );
        });
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
    fn every_notice_carries_the_text_of_the_licence_it_names() {
        let titles: &[(&str, &[&str])] = &[
            ("BSD 3-Clause", &["BSD 3-Clause License"]),
            ("ISC", &["ISC License"]),
            ("MIT", &["MIT License", "Permission is hereby granted"]),
            (
                "MIT and Bitstream Vera",
                &[
                    "MIT License",
                    "Permission is hereby granted",
                    "BITSTREAM VERA LICENSE",
                ],
            ),
            (
                "SIL Open Font License 1.1",
                &["SIL OPEN FONT LICENSE Version 1.1"],
            ),
            (
                "Ubuntu Font Licence 1.0",
                &["UBUNTU FONT LICENCE Version 1.0"],
            ),
        ];
        for notice in NOTICES {
            let Some((_, phrases)) = titles.iter().find(|(name, _)| *name == notice.licence) else {
                panic!("{}: no title known for {:?}", notice.covers, notice.licence);
            };
            for phrase in *phrases {
                assert!(
                    notice.text.contains(phrase),
                    "{}: the {:?} text lacks {phrase:?}",
                    notice.covers,
                    notice.licence
                );
            }
        }
    }

    #[test]
    fn no_two_notices_cover_the_same_thing() {
        for (at, notice) in NOTICES.iter().enumerate() {
            assert!(
                NOTICES[at + 1..]
                    .iter()
                    .all(|other| other.covers != notice.covers),
                "{:?} is listed twice",
                notice.covers
            );
        }
    }

    #[test]
    fn the_ubuntu_fonts_are_listed_under_their_licence() {
        assert!(
            NOTICES
                .iter()
                .any(|notice| notice.covers.starts_with("Ubuntu")
                    && notice.licence == "Ubuntu Font Licence 1.0"),
            "the About box does not list the Ubuntu Font Licence"
        );
    }

    #[test]
    fn the_compiled_in_licence_is_the_workspace_licence() {
        let drawbar = NOTICES
            .iter()
            .find(|notice| notice.covers == "drawbar")
            .expect("drawbar lists its own licence");
        assert_eq!(
            drawbar.text,
            include_str!("../../LICENSE"),
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
