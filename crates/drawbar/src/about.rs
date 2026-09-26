//! About drawbar: what it is, where it lives, what this build is, the licences of what is
//! compiled into it, and whose trademarks the names in it are.
//!
//! The same box on every target. The licences are compiled in, so a binary handed to
//! someone carries the text of the terms it is under.

use eframe::egui;

use crate::device::Device;
use crate::icon::{sized, Glyph};
use crate::log::Log;
use crate::sheet::{self, GAP};
use crate::shell::GUIDE;
use crate::workspace::Workspace;

#[cfg(any(target_arch = "wasm32", test))]
mod agent;
mod crates;
#[cfg(target_arch = "wasm32")]
mod web;

const REPO: &str = "https://github.com/jmoo/drawbar";
pub(crate) const RELEASES: &str = "https://github.com/jmoo/drawbar/releases";
const ISSUES: &str = "https://github.com/jmoo/drawbar/issues";

/// What one licence covers in the app, and its terms.
struct Notice {
    covers: &'static str,
    /// The copyright line, as the licence text or the font's own name table gives it.
    holder: &'static str,
    /// Where the project lives.
    page: &'static str,
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
    /// Crates, as `name version`, whose licence file names no copyright holder.
    unattributed: &'static [&'static str],
    /// Crates whose licence file says more than [`Group::text`] does.
    variants: &'static [Text],
}

impl Group {
    /// How many crates are under the licence.
    fn count(&self) -> usize {
        let held: usize = self.holders.iter().map(|holder| holder.crates.len()).sum();
        let varied: usize = self.variants.iter().map(|text| text.crates.len()).sum();
        self.unattributed.len() + held + varied
    }

    fn held(&self) -> String {
        match self.count() {
            1 => "1 dependency".to_string(),
            count => format!("{count} dependencies"),
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

/// drawbar first, then the published field maps its format placements derive from, then
/// the bundled material by what it covers, alphabetically.
const NOTICES: &[Notice] = &[
    Notice {
        covers: "drawbar",
        holder: "Copyright (c) 2023-2026, John Moore",
        page: REPO,
        source: None,
        licence: "BSD 3-Clause",
        // A copy of `crates/LICENSE`: a packaged crate cannot reach outside its own root,
        // and a test keeps the two the same.
        text: include_str!("../assets/LICENSE"),
    },
    Notice {
        covers: "Stage 2 and 3 field maps",
        holder: "Copyright (c) 2020, Christian Florentz",
        page: "https://github.com/Chris55/nord-documentation",
        source: Some("nord-documentation by Christian Florentz"),
        licence: "BSD 3-Clause",
        text: include_str!("../licences/nord-documentation.txt"),
    },
    Notice {
        covers: "Stage 4 field tables",
        holder: "Copyright (c) 2024 Randy",
        page: "https://ns4decode.netlify.app",
        source: Some("ns4decode by Randy"),
        licence: "MIT",
        text: include_str!("../licences/ns4decode.txt"),
    },
    Notice {
        covers: "emoji-icon-font",
        holder: "Copyright (c) 2014 John Slegers",
        page: "https://github.com/jslegers/emoji-icon-font",
        source: Some("egui's epaint_default_fonts 0.32.3"),
        licence: "MIT",
        text: include_str!("../assets/fonts/egui/emoji-icon-font-mit-license.txt"),
    },
    Notice {
        covers: "Hack Regular",
        holder: "Copyright (c) 2018 Source Foundry Authors",
        page: "https://github.com/source-foundry/Hack",
        source: Some("egui's epaint_default_fonts 0.32.3"),
        licence: "MIT and Bitstream Vera",
        text: include_str!("../assets/fonts/egui/Hack-Regular.txt"),
    },
    Notice {
        covers: "Lucide icons",
        holder: "Copyright (c) 2022 Lucide Contributors, 2013-2022 Cole Bemis",
        page: "https://github.com/lucide-icons/lucide",
        source: Some("Lucide 0.469.0"),
        licence: "ISC",
        text: include_str!("../assets/icons/LICENSE"),
    },
    Notice {
        covers: "Noto Emoji Regular",
        holder: "Copyright 2013 Google Inc.",
        page: "https://github.com/googlefonts/noto-emoji",
        source: Some("egui's epaint_default_fonts 0.32.3"),
        licence: "SIL Open Font License 1.1",
        text: include_str!("../assets/fonts/egui/OFL.txt"),
    },
    Notice {
        covers: "Ubuntu Regular, Bold and Light",
        holder: "Copyright 2011 Canonical Ltd.",
        page: "https://design.ubuntu.com/font",
        source: Some("Ubuntu font family 0.83; Light from egui's epaint_default_fonts 0.32.3"),
        licence: "Ubuntu Font Licence 1.0",
        text: include_str!("../assets/fonts/LICENCE.txt"),
    },
];

/// What the reader is told the build lines are for.
const WHY: &str = "paste this into a bug report and we know what you were running";

/// What a crate group says over the crates whose licence file names nobody.
const UNATTRIBUTED: &str = "no copyright line in the licence file";

/// What Copy diagnostics says it takes, on hover.
const COPIES: &str = "Copies the lines below, plus the activity log's last 200 entries";

/// The widest the sheet is drawn.
const WIDE: f32 = 760.0;

/// The height the sheet needs around its scrolling middle — masthead, links and foot — so
/// an open licence scrolls inside the middle rather than pushing Close off-screen.
const AROUND: f32 = 300.0;

/// The middle is never shorter than this, however short the window.
const FEWEST: f32 = 140.0;

/// The room above the masthead. Every other edge is [`sheet::PAD`] or the foot's own.
const TOP: f32 = 18.0;

/// 10 px: the widest hard-wrapped text is 78 columns, and this is the size that fits them
/// in [`WIDE`] without wrapping them a second time. Texts with longer lines wrap to it.
const MONO: f32 = 10.0;

/// A build line: the key's column, the row it sits on, and the room between two columns
/// of them.
const KEY: f32 = 92.0;
const LINE: f32 = 22.0;
const GUTTER: f32 = 24.0;

/// The least room one column of build lines is given; two of them side by side need
/// twice this and a [`GUTTER`].
const COLUMN: f32 = 330.0;

/// The room the foot's Close button is left at the right of the disclaimer.
const CLOSE: f32 = 90.0;

/// A licence row, and the chevron that opens it.
const ROW: f32 = 26.0;
const CHEVRON: f32 = 12.0;

/// How long the button says "Copied", in seconds.
const SAID: f64 = 1.6;

/// The most log entries [`diagnostics`] carries.
const ENTRIES: usize = 200;

/// The release a version's notes were published on.
pub fn release_page(version: &str) -> String {
    format!("{RELEASES}/tag/drawbar-v{version}")
}

/// One line of what this build is: what it is called, what it says, and the aside after it.
struct Line {
    key: &'static str,
    value: String,
    note: String,
}

impl Line {
    fn new(key: &'static str, value: impl Into<String>, note: impl Into<String>) -> Line {
        Line {
            key,
            value: value.into(),
            note: note.into(),
        }
    }
}

/// What this build is: read when the box opens, not while it is drawn.
struct Build {
    lines: Vec<Line>,
}

impl Build {
    fn new(device: &Device, workspace: &Workspace) -> Build {
        let mut lines = vec![Line::new("Version", sheet::VERSION, "alpha"), target()];
        #[cfg(target_arch = "wasm32")]
        lines.push(Line::new("Browser", web::agent(), ""));
        lines.push(usb(device));
        lines.push(files(workspace));
        Build { lines }
    }
}

#[cfg(target_arch = "wasm32")]
fn target() -> Line {
    Line::new("Target", "wasm32-unknown-unknown", "in the browser")
}

#[cfg(not(target_arch = "wasm32"))]
fn target() -> Line {
    let (arch, os) = (std::env::consts::ARCH, std::env::consts::OS);
    Line::new("Target", format!("{arch} {os}"), "on the desktop")
}

#[cfg(target_arch = "wasm32")]
fn usb(device: &Device) -> Line {
    let instrument = device.state.product().unwrap_or("no instrument connected");
    match device.usb() {
        true => Line::new("Web USB", "available", instrument),
        false => Line::new("Web USB", "unavailable", crate::device::NO_USB_BRIEF),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn usb(device: &Device) -> Line {
    use crate::device::Connection;
    match &device.state.connection {
        Connection::Connected(card) => Line::new("USB", "connected", card.product.as_str()),
        Connection::Connecting => Line::new("USB", "connecting", ""),
        Connection::Disconnected => Line::new("USB", "no instrument connected", ""),
    }
}

/// How many files this computer is holding. Their bytes are not counted: nothing in the
/// store tracks how much room they take.
fn files(workspace: &Workspace) -> Line {
    let key = match cfg!(target_arch = "wasm32") {
        true => "Local storage",
        false => "Files",
    };
    match workspace.listed().count() {
        1 => Line::new(key, "1 file", ""),
        held => Line::new(key, format!("{held} files"), ""),
    }
}

/// What Copy diagnostics puts on the clipboard: what this build is, then the tail of the
/// activity log.
fn diagnostics(build: &Build, log: &Log) -> String {
    let mut out = String::new();
    for line in &build.lines {
        match line.note.is_empty() {
            true => out.push_str(&format!("{}: {}\n", line.key, line.value)),
            false => out.push_str(&format!("{}: {} ({})\n", line.key, line.value, line.note)),
        }
    }
    out.push('\n');
    out.push_str(&log.tail(ENTRIES));
    out
}

/// The About sheet while it is open.
pub struct About {
    build: Build,
    /// When the diagnostics were last copied, on egui's clock.
    copied: Option<f64>,
}

impl About {
    /// Read what this build is. Called when the box opens.
    pub fn new(device: &Device, workspace: &Workspace) -> About {
        About {
            build: Build::new(device, workspace),
            copied: None,
        }
    }

    /// Returns whether the reader is done with it.
    fn body(&mut self, ui: &mut egui::Ui, log: &Log) -> bool {
        ui.set_width(sheet::width(ui.ctx(), WIDE));
        ui.add_space(TOP);
        sheet::section(ui, |ui| {
            sheet::masthead(ui, true);
            ui.add_space(GAP * 2.0);
            links(ui);
        });
        egui::ScrollArea::vertical()
            .id_salt("about")
            .max_height(sheet::middle(ui.ctx(), AROUND, FEWEST))
            .show(ui, |ui| {
                // The scrollbar floats over the content; keep the rows out from under it.
                let scroll = ui.spacing().scroll;
                ui.set_width(ui.available_width() - scroll.bar_width - scroll.bar_outer_margin);
                sheet::section(ui, |ui| {
                    self.this_build(ui, log);
                    sheet::heading(ui, "Licences", Some(&inventory()));
                    licences(ui);
                });
            });
        let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
        let mut closed = false;
        sheet::foot(
            ui,
            |ui| sheet::disclaimer(ui, CLOSE),
            |ui| closed = sheet::secondary(ui, None, "Close").clicked(),
        );
        closed || escaped
    }

    /// The heading, the button that copies what is under it, and the build lines.
    fn this_build(&mut self, ui: &mut egui::Ui, log: &Log) {
        let head = ui
            .scope(|ui| sheet::heading(ui, "This build", Some(WHY)))
            .response
            .rect;
        // A child over the heading's band, claiming no room of its own: the button sits
        // on a line whose place is known only once that line has been drawn.
        let band = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), head.y_range());
        let mut beside = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(band)
                .layout(egui::Layout::right_to_left(egui::Align::Max)),
        );
        self.copy(&mut beside, log);
        grid(ui, &self.build.lines);
    }

    /// Put the diagnostics on the clipboard, and say so until [`SAID`] has passed.
    fn copy(&mut self, ui: &mut egui::Ui, log: &Log) {
        let now = ui.input(|input| input.time);
        let since = self.copied.map(|at| now - at).filter(|since| *since < SAID);
        let (glyph, label) = match since {
            Some(_) => (Glyph::Check, "Copied"),
            None => (Glyph::Clipboard, "Copy diagnostics"),
        };
        if let Some(since) = since {
            // egui repaints on demand, and an idle window would leave "Copied" standing
            // until something else asked for a frame.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(SAID - since));
        }
        if sheet::secondary(ui, Some(glyph), label)
            .on_hover_text(COPIES)
            .clicked()
        {
            ui.ctx().copy_text(diagnostics(&self.build, log));
            self.copied = Some(now);
        }
    }
}

/// Draw the box while it is open, and close it once the reader is done.
pub fn dialog(ctx: &egui::Context, open: &mut Option<About>, log: &Log) {
    let Some(about) = open.as_mut() else {
        return;
    };
    if egui::Modal::new(egui::Id::new("about"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| about.body(ui, log))
        .inner
    {
        *open = None;
    }
}

/// Where the rest of the project is.
fn links(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = GAP * 3.5;
        sheet::glyph_link(ui, Glyph::Github, "Source on GitHub", REPO);
        sheet::glyph_link(ui, Glyph::BookOpen, "User guide", GUIDE);
        sheet::glyph_link(ui, Glyph::Tag, "Releases", RELEASES);
        sheet::glyph_link(ui, Glyph::MessageSquareWarning, "Report a problem", ISSUES);
    });
}

/// How many columns of build lines `room` holds.
fn columns(room: f32) -> usize {
    match room >= 2.0 * COLUMN + GUTTER {
        true => 2,
        false => 1,
    }
}

/// The build lines, two columns wide where the sheet has the room and one where it has not.
fn grid(ui: &mut egui::Ui, lines: &[Line]) {
    let room = ui.available_width();
    let columns = columns(room);
    let width = (room - GUTTER * (columns - 1) as f32) / columns as f32;
    for row in lines.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GUTTER;
            for line in row {
                cell(ui, line, width);
            }
        });
    }
}

/// One build line, on a ruled row: its key, what it says, and the aside after it.
fn cell(ui: &mut egui::Ui, line: &Line, width: f32) {
    let row = egui::vec2(width, LINE);
    let laid = egui::Layout::left_to_right(egui::Align::Center);
    let rect = ui
        .allocate_ui_with_layout(row, laid, |ui| {
            ui.set_min_size(row);
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.allocate_ui_with_layout(egui::vec2(KEY, LINE), laid, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(line.key).size(11.0)).truncate());
            });
            ui.spacing_mut().item_spacing.x = 7.0;
            let value = egui::RichText::new(&line.value)
                .font(egui::FontId::monospace(11.0))
                .strong();
            ui.add(egui::Label::new(value).truncate());
            if !line.note.is_empty() {
                let note = egui::RichText::new(&line.note).small().weak();
                ui.add(egui::Label::new(note).truncate());
            }
        })
        .response
        .rect;
    let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().hline(rect.x_range(), rect.bottom(), hairline);
}

/// How much there is to read: the notices, and the crates behind them.
fn inventory() -> String {
    let crates: usize = crates::GROUPS.iter().map(Group::count).sum();
    format!("{} entries · {crates} crates", NOTICES.len())
}

/// Every licence whose terms require its notice to travel with a copy of the app: the
/// [`NOTICES`], then the Rust crates by licence.
fn licences(ui: &mut egui::Ui) {
    for notice in NOTICES {
        row(ui, notice.covers, notice.holder, notice.licence, |ui| {
            page(ui, notice.page);
            terms(ui, notice.source, notice.text);
        });
    }
    for group in crates::GROUPS {
        row(ui, "Rust crates", &group.held(), group.licence, |ui| {
            for holder in group.holders {
                credit(ui, holder.notice, holder.crates);
            }
            if !group.unattributed.is_empty() {
                credit(ui, UNATTRIBUTED, group.unattributed);
            }
            terms(ui, None, group.text);
            for variant in group.variants {
                ui.add_space(GAP);
                packages(ui, variant.crates);
                terms(ui, None, variant.text);
            }
        });
    }
}

/// Where a project lives, as a link that reads as its address.
fn page(ui: &mut egui::Ui, url: &str) {
    let shown = url.strip_prefix("https://").unwrap_or(url);
    sheet::glyph_link(ui, Glyph::ArrowUpRight, shown, url);
    ui.add_space(GAP);
}

/// A copyright notice and the crates whose licence file carries it.
fn credit(ui: &mut egui::Ui, notice: &str, crates: &[&str]) {
    ui.label(
        egui::RichText::new(notice)
            .font(egui::FontId::monospace(MONO))
            .weak(),
    );
    packages(ui, crates);
    ui.add_space(GAP);
}

/// The crates.io page of a `name version`.
fn crate_url(package: &str) -> String {
    match package.split_once(' ') {
        Some((name, version)) => format!("https://crates.io/crates/{name}/{version}"),
        None => format!("https://crates.io/crates/{package}"),
    }
}

/// Crates as `name version`, each a link to that version on crates.io.
fn packages(ui: &mut egui::Ui, crates: &[&str]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = GAP * 2.0;
        for package in crates {
            sheet::link(ui, package, &crate_url(package));
        }
    });
}

/// Collapsed, what a licence covers, whose it is and its name; open, the terms in a box.
fn row(
    ui: &mut egui::Ui,
    covers: &str,
    held: &str,
    licence: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id((covers, licence));
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    if header(ui, covers, held, licence, state.is_open()).clicked() {
        state.toggle(ui);
    }
    state.show_body_unindented(ui, |ui| {
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(egui::Margin::symmetric(9, 7))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                body(ui);
            });
        ui.add_space(GAP);
    });
}

/// The row itself: a chevron that says which way it goes, what it covers, whose it is, and
/// the licence at the right.
fn header(
    ui: &mut egui::Ui,
    covers: &str,
    held: &str,
    licence: &str,
    open: bool,
) -> egui::Response {
    let laid = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
        let backdrop = ui.painter().add(egui::Shape::Noop);
        let ink = ui.visuals().text_color();
        ui.horizontal(|ui| {
            ui.set_min_height(ROW);
            ui.spacing_mut().item_spacing.x = 8.0;
            let chevron = match open {
                true => Glyph::ChevronDown,
                false => Glyph::ChevronRight,
            };
            ui.add(sized(chevron, CHEVRON, ui.visuals().weak_text_color()));
            ui.add(egui::Label::new(
                egui::RichText::new(covers).size(11.5).strong(),
            ));
            // The licence name is never cut, so the holder takes what it leaves.
            let named = egui::FontId::monospace(10.5);
            let width = ui
                .fonts(|fonts| fonts.layout_no_wrap(licence.to_string(), named.clone(), ink))
                .size()
                .x;
            let room = egui::vec2((ui.available_width() - width - 8.0).max(0.0), ROW);
            ui.allocate_ui_with_layout(room, *ui.layout(), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(held)
                            .font(egui::FontId::monospace(9.5))
                            .weak(),
                    )
                    .truncate(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(licence).font(named).color(ink));
            });
        });
        backdrop
    });
    let response = laid.response;
    if response.hovered() {
        let fill = ui.visuals().widgets.hovered.weak_bg_fill;
        ui.painter()
            .set(laid.inner, egui::Shape::rect_filled(response.rect, 2, fill));
    }
    let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .hline(response.rect.x_range(), response.rect.bottom(), hairline);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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

    fn build() -> Build {
        Build {
            lines: vec![
                Line::new("Version", "0.7.1", "alpha"),
                Line::new("Target", "aarch64 macos", "on the desktop"),
                Line::new("Files", "2 files", ""),
            ],
        }
    }

    #[test]
    fn the_diagnostics_carry_every_build_line_and_its_aside() {
        let text = diagnostics(&build(), &Log::default());
        assert!(text.contains("Version: 0.7.1 (alpha)"), "{text}");
        assert!(
            text.contains("Target: aarch64 macos (on the desktop)"),
            "{text}"
        );
        assert!(text.contains("Files: 2 files\n"), "{text}");
    }

    #[test]
    fn the_diagnostics_carry_the_log_after_the_build_and_cap_it() {
        let mut log = Log::default();
        for n in 0..250 {
            log.info(format!("line {n}"));
        }
        let text = diagnostics(&build(), &log);
        let (build, tail) = text
            .split_once("\n\n")
            .expect("a blank line between the two");
        assert_eq!(build.lines().count(), 3);
        assert_eq!(tail.lines().count(), ENTRIES);
        assert!(!tail.contains("line 49"), "the log was not capped");
        assert!(
            tail.contains("line 50") && tail.contains("line 249"),
            "{tail}"
        );
    }

    #[test]
    fn the_diagnostics_of_a_silent_session_are_the_build_alone() {
        let text = diagnostics(&build(), &Log::default());
        assert!(text.ends_with("Files: 2 files\n\n"), "{text:?}");
    }

    fn headless() -> egui::Context {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        ctx.set_fonts(crate::app::fonts());
        ctx.all_styles_mut(crate::app::metrics);
        ctx
    }

    /// Draw the sheet in a window of `size`, and answer with the room it took.
    fn drawn_at(ctx: &egui::Context, size: egui::Vec2) -> egui::Rect {
        let mut about = About {
            build: build(),
            copied: None,
        };
        let log = Log::default();
        let mut rect = egui::Rect::ZERO;
        // Twice: a scrolling middle knows what it holds only once it has held it.
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                rect = egui::Modal::new(egui::Id::new("about"))
                    .frame(sheet::frame(&ctx.style().visuals))
                    .show(ctx, |ui| {
                        about.body(ui, &log);
                    })
                    .response
                    .rect;
            });
        }
        rect
    }

    /// The shell refuses a window smaller than this, so the box has to fit one.
    #[test]
    fn the_sheet_fits_the_smallest_window_the_shell_allows() {
        let least = crate::shell::LEAST;
        let took = drawn_at(&headless(), least);
        assert!(
            took.height() <= least.y && took.width() <= least.x,
            "the sheet took {} x {} in a window of {} x {}",
            took.width(),
            took.height(),
            least.x,
            least.y
        );
    }

    /// Two columns of build lines need room the smallest window does not have.
    #[test]
    fn the_build_lines_stand_in_two_columns_only_where_the_sheet_is_wide() {
        let ctx = headless();
        let room = |sheet: f32| sheet - 2.0 * sheet::PAD;
        drawn_at(&ctx, crate::shell::LEAST);
        assert_eq!(columns(room(sheet::width(&ctx, WIDE))), 1);
        drawn_at(&ctx, egui::vec2(1400.0, 980.0));
        assert_eq!(columns(room(sheet::width(&ctx, WIDE))), 2);
    }

    /// A holder is a copyright line, and every name in it is one the licence text
    /// carries — unless the text names nobody and the holder came off the font itself.
    #[test]
    fn every_notice_names_a_copyright_holder_its_licence_agrees_with() {
        for notice in NOTICES {
            assert!(
                notice.holder.starts_with("Copyright"),
                "{}: {:?} is not a copyright line",
                notice.covers,
                notice.holder
            );
            if !notice.text.contains("Copyright (c)") {
                continue;
            }
            let names = notice
                .holder
                .split(|c: char| !c.is_alphabetic())
                .filter(|word| word.len() > 3 && *word != "Copyright");
            for name in names {
                assert!(
                    notice.text.contains(name),
                    "{}: the licence text never names {name:?}",
                    notice.covers
                );
            }
        }
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
    fn a_group_counts_every_crate_under_its_licence() {
        let one = Group {
            licence: "ISC",
            text: "",
            holders: &[Holder {
                notice: "Copyright (c) 2015, Simonas Kazlauskas",
                crates: &["libloading 0.8.9"],
            }],
            unattributed: &[],
            variants: &[],
        };
        let four = Group {
            licence: "MIT",
            text: "",
            holders: &[Holder {
                notice: "Copyright (c) 2015 nwin",
                crates: &["png 0.17.16", "png 0.18.1"],
            }],
            unattributed: &["adler2 2.0.1"],
            variants: &[Text {
                crates: &["zip 2.4.2"],
                text: "",
            }],
        };
        assert_eq!(one.count(), 1);
        assert_eq!(one.held(), "1 dependency");
        assert_eq!(four.count(), 4);
        assert_eq!(four.held(), "4 dependencies");
    }

    /// A count sums each holder's and each variant's crates, so a repeat inflates it.
    #[test]
    fn no_crate_is_listed_twice_under_one_licence() {
        for group in crates::GROUPS {
            let mut crates: Vec<_> = group
                .holders
                .iter()
                .flat_map(|holder| holder.crates)
                .chain(group.unattributed)
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
    fn every_notice_points_at_its_project_over_https() {
        for notice in NOTICES {
            assert!(
                notice.page.starts_with("https://"),
                "{}: {:?}",
                notice.covers,
                notice.page
            );
        }
    }

    #[test]
    fn a_crate_link_names_the_version_that_is_compiled_in() {
        assert_eq!(
            crate_url("png 0.17.16"),
            "https://crates.io/crates/png/0.17.16"
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
            assert!(sheet::DISCLAIMER.contains(required), "missing: {required}");
        }
    }
}
