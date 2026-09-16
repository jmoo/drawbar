//! About drawbar: what it is, where it lives, the licences of what is compiled into it,
//! and whose trademarks the names in it are.
//!
//! The same box on every target. The licences are compiled in, so a binary handed to
//! someone carries the text of the terms it is under.

use eframe::egui;

use crate::sheet::{link, GAP};
use crate::shell::GUIDE;
use crate::splash::{title, WIDTH};

mod crates;

const REPO: &str = "https://github.com/jmoo/drawbar";
pub(crate) const RELEASES: &str = "https://github.com/jmoo/drawbar/releases";

/// What one licence covers in the app, and its terms.
struct Notice {
    covers: &'static str,
    /// Where bundled material came from; `None` for drawbar itself.
    source: Option<&'static str>,
    licence: &'static str,
    text: &'static str,
}

/// The Rust crates under one licence: its text once, and who holds copyright in what.
struct Group {
    licence: &'static str,
    /// The licence, from `crates/drawbar/licences/<id>.txt`.
    text: &'static str,
    holders: &'static [Holder],
    /// How many crates under the licence have a file that names no copyright holder.
    unattributed: usize,
    /// Crates whose licence file says more than [`Group::text`] does.
    variants: &'static [Text],
}

impl Group {
    /// The licence and how many crates are under it, e.g. `MIT · 2 crates`.
    fn summary(&self) -> String {
        let held: usize = self.holders.iter().map(|holder| holder.crates.len()).sum();
        let varied: usize = self.variants.iter().map(|text| text.crates.len()).sum();
        match self.unattributed + held + varied {
            1 => format!("{} · 1 crate", self.licence),
            count => format!("{} · {count} crates", self.licence),
        }
    }
}

/// One copyright notice and the crates, as `name version`, whose licence file carries it.
struct Holder {
    notice: &'static str,
    crates: &'static [&'static str],
}

/// One licence text and the crates, as `name version`, that carry it.
struct Text {
    crates: &'static [&'static str],
    text: &'static str,
}

/// drawbar first, then the bundled material by what it covers, alphabetically, then the
/// published field maps drawbar's format placements derive from.
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
    Notice {
        covers: "Stage 2 and 3 field maps",
        source: Some("nord-documentation by Christian Florentz"),
        licence: "BSD 3-Clause",
        text: include_str!("../licences/nord-documentation.txt"),
    },
    Notice {
        covers: "Stage 4 field tables",
        source: Some("ns4decode by Randy"),
        licence: "MIT",
        text: include_str!("../licences/ns4decode.txt"),
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
    ui.label(crate::sheet::WHAT);
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
        .show(ui, licences);
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

/// Every licence whose terms require its notice to travel with a copy of the app: the
/// [`NOTICES`], then the Rust crates by licence.
fn licences(ui: &mut egui::Ui) {
    for notice in NOTICES {
        row(ui, notice.covers, notice.licence, |ui| {
            terms(ui, notice.source, notice.text);
        });
    }
    for group in crates::GROUPS {
        row(ui, "Rust crates", &group.summary(), |ui| {
            for holder in group.holders {
                credit(ui, holder.notice, holder.crates);
            }
            terms(ui, None, group.text);
            for variant in group.variants {
                ui.add_space(GAP);
                terms(ui, Some(&variant.crates.join(", ")), variant.text);
            }
        });
    }
}

/// A copyright notice and the crates whose licence file carries it.
fn credit(ui: &mut egui::Ui, notice: &str, crates: &[&str]) {
    ui.label(
        egui::RichText::new(notice)
            .font(egui::FontId::monospace(MONO))
            .weak(),
    );
    ui.label(egui::RichText::new(crates.join(", ")).small().weak());
    ui.add_space(GAP);
}

/// Collapsed, what a licence covers and its name; open, the terms.
fn row(ui: &mut egui::Ui, covers: &str, licence: &str, body: impl FnOnce(&mut egui::Ui)) {
    let id = ui.make_persistent_id((covers, licence));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            ui.label(covers);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The list's scrollbar floats over its content; keep the name out from under it.
                let scroll = ui.spacing().scroll;
                ui.add_space(scroll.bar_width + scroll.bar_outer_margin);
                ui.label(egui::RichText::new(licence).weak());
            });
        })
        .body(body);
}

/// A licence text, under what it applies to where that needs saying.
fn terms(ui: &mut egui::Ui, applies_to: Option<&str>, text: &str) {
    if let Some(applies_to) = applies_to {
        ui.label(egui::RichText::new(applies_to).small().weak());
        ui.add_space(GAP);
    }
    ui.label(
        egui::RichText::new(text)
            .font(egui::FontId::monospace(MONO))
            .weak(),
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

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
    fn every_crate_text_carries_the_licence_its_group_names() {
        let phrases: &[(&str, &[&str])] = &[
            ("Apache-2.0", &["Apache License", "Version 2.0"]),
            (
                "BSD-2-Clause",
                &["Redistribution and use in source and binary forms"],
            ),
            (
                "BSD-3-Clause",
                &[
                    "Redistribution and use in source and binary forms",
                    "endorse or promote products",
                ],
            ),
            (
                "ISC",
                &["Permission to use, copy, modify, and/or distribute this software"],
            ),
            ("MIT", &["Permission is hereby granted"]),
            ("Unicode-3.0", &["UNICODE LICENSE V3"]),
        ];
        for group in crates::GROUPS {
            let Some((_, required)) = phrases.iter().find(|(id, _)| *id == group.licence) else {
                panic!("no phrases known for {:?}", group.licence);
            };
            let texts = std::iter::once((group.licence, group.text)).chain(
                group
                    .variants
                    .iter()
                    .map(|variant| (variant.crates[0], variant.text)),
            );
            for (carrier, text) in texts {
                // Hard wrapping differs between copies of the same licence.
                let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
                for phrase in *required {
                    assert!(
                        words.contains(phrase),
                        "{carrier}: the {:?} text lacks {phrase:?}",
                        group.licence
                    );
                }
            }
        }
    }

    #[test]
    fn no_crate_text_is_an_unfilled_licence_template() {
        let unfilled = |text: &str| {
            let text = text.to_lowercase();
            ["<year>", "<owner>", "<copyright holder"]
                .iter()
                .any(|placeholder| text.contains(placeholder))
        };
        for group in crates::GROUPS {
            assert!(!unfilled(group.text), "{} is a template", group.licence);
            for holder in group.holders {
                assert!(
                    !unfilled(holder.notice),
                    "{:?} names no copyright holder",
                    holder.crates
                );
            }
            for variant in group.variants {
                assert!(
                    !unfilled(variant.text),
                    "{:?} carry a template",
                    variant.crates
                );
            }
        }
    }

    /// A licence sentence about copyright must not be filed as a notice of one.
    #[test]
    fn every_holder_notice_claims_a_copyright() {
        for group in crates::GROUPS {
            for holder in group.holders {
                let notice = holder.notice.to_lowercase();
                assert!(
                    ["copyright", "(c)", "©"]
                        .iter()
                        .any(|claim| notice.contains(claim)),
                    "{:?}: {:?} claims no copyright",
                    holder.crates,
                    holder.notice
                );
            }
        }
    }

    #[test]
    fn a_group_summary_counts_every_crate_under_its_licence() {
        let one = Group {
            licence: "ISC",
            text: "",
            holders: &[Holder {
                notice: "Copyright (c) 2015, Simonas Kazlauskas",
                crates: &["libloading 0.8.9"],
            }],
            unattributed: 0,
            variants: &[],
        };
        let four = Group {
            licence: "MIT",
            text: "",
            holders: &[Holder {
                notice: "Copyright (c) 2015 nwin",
                crates: &["png 0.17.16", "png 0.18.1"],
            }],
            unattributed: 1,
            variants: &[Text {
                crates: &["zip 2.4.2"],
                text: "",
            }],
        };
        assert_eq!(one.summary(), "ISC · 1 crate");
        assert_eq!(four.summary(), "MIT · 4 crates");
    }

    /// A summary sums each holder's and each variant's crates, so a repeat inflates it.
    #[test]
    fn no_crate_is_listed_twice_under_one_licence() {
        for group in crates::GROUPS {
            let mut crates: Vec<_> = group
                .holders
                .iter()
                .flat_map(|holder| holder.crates)
                .chain(group.variants.iter().flat_map(|variant| variant.crates))
                .collect();
            crates.sort_unstable();
            let repeated = crates.windows(2).find(|pair| pair[0] == pair[1]);
            assert_eq!(repeated, None, "{} lists a crate twice", group.licence);
        }
    }

    #[test]
    fn the_vendored_crate_licences_match_the_lockfile() {
        let locked: BTreeSet<_> = registry_packages(include_str!("../../Cargo.lock"));
        let vendored: BTreeSet<_> = crates::LOCKED.iter().copied().collect();
        let added: Vec<_> = locked.difference(&vendored).collect();
        let removed: Vec<_> = vendored.difference(&locked).collect();
        assert!(
            added.is_empty() && removed.is_empty(),
            "Cargo.lock's registry packages changed since the crate licences were vendored; \
             run scripts/licences.bash.\nadded: {added:?}\nremoved: {removed:?}"
        );
    }

    /// `(name, version)` of each `[[package]]` in a Cargo.lock whose source is a registry.
    fn registry_packages(lock: &str) -> BTreeSet<(&str, &str)> {
        lock.split("[[package]]")
            .filter_map(|block| {
                let field = |key: &str| {
                    block.lines().find_map(|line| {
                        line.strip_prefix(key)?
                            .strip_prefix(" = \"")?
                            .strip_suffix('"')
                    })
                };
                field("source")?
                    .starts_with("registry+")
                    .then_some((field("name")?, field("version")?))
            })
            .collect()
    }

    #[test]
    fn a_lockfile_yields_only_its_registry_packages() {
        let lock = r#"version = 4

[[package]]
name = "drawbar"
version = "0.5.0"
dependencies = [
 "egui",
]

[[package]]
name = "egui"
version = "0.32.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000"

[[package]]
name = "forked"
version = "1.0.0"
source = "git+https://example.com/forked#0000"
"#;
        assert_eq!(
            registry_packages(lock),
            BTreeSet::from([("egui", "0.32.3")])
        );
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
