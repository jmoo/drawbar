//! The two sheets a session can open on: the welcome a first run shows, and what
//! changed since the version last read.
//!
//! The notes are GitHub's release body for `drawbar-v<version>`, which
//! `scripts/release.bash` writes in a fixed shape; [`classify`] reads that shape without
//! a Markdown parser, and a line it does not recognize stays plain text. Only the browser
//! opens a sheet unasked and fetches the notes.

use eframe::egui;

use crate::app::{accent, bold, good, warn};
use crate::browser::Act;
use crate::device::{Device, NO_USB};
use crate::icon::{sized, Glyph};
use crate::sheet::{self, GAP};
use crate::tabs::Spot;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::Splash;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::Splash;

pub(crate) use crate::sheet::VERSION;

/// The widest each sheet may be.
const WELCOME_WIDTH: f32 = 720.0;
const NEWS_WIDTH: f32 = 700.0;

/// The height a sheet reserves for its foot, so a short window scrolls the middle and
/// keeps the dismiss button on screen.
const AROUND: f32 = 96.0;

/// The shortest the scrolling middle gets, however short the window.
const FEWEST: f32 = 120.0;

/// The width the welcome's foot reserves for its button, right of the disclaimer.
const DISMISS: f32 = 200.0;

/// Which sheet a session opens on, given the version whose sheet was last dismissed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opening {
    /// Nobody has run drawbar here before.
    Welcome,
    /// The version last read is not the one running.
    News,
    Nothing,
}

/// A first run opens on the welcome, an update on what changed, and a version already
/// read on the app itself.
pub fn opening(seen: Option<&str>) -> Opening {
    match seen {
        None => Opening::Welcome,
        Some(seen) if seen == VERSION => Opening::Nothing,
        Some(_) => Opening::News,
    }
}

/// What a click on the welcome sheet asks of the app.
pub enum Wanted {
    /// Close, and record this version as read.
    Done,
    /// Close, record this version, and run this.
    Act(Act),
}

/// The risk box: a bold lead, then the rest.
const RISK_LEAD: &str = "Keep your own backups.";
const RISK_REST: &str = " drawbar is alpha software. Treat what it holds as a working copy.";

/// The start cards, in the order the sheet shows them.
const STARTS: [Start; 3] = [Start::Connect, Start::Open, Start::Demo];

#[derive(Clone, Copy)]
enum Start {
    Connect,
    Open,
    Demo,
}

/// What the Connect card can do.
#[derive(Clone, Copy)]
pub enum Usb<'a> {
    /// This browser has no WebUSB, so the card is grayed out.
    Missing,
    Ready,
    /// An instrument is attached, by its product name, so the card shows its slots.
    Attached(&'a str),
}

impl Usb<'_> {
    pub fn of(device: &Device) -> Usb<'_> {
        match (device.state.product(), device.usb()) {
            (Some(product), _) => Usb::Attached(product),
            (None, true) => Usb::Ready,
            (None, false) => Usb::Missing,
        }
    }
}

/// How one [`Start`] reads on the sheet.
struct Card<'a> {
    glyph: Glyph,
    label: &'a str,
    sub: &'a str,
    hint: &'a str,
    /// Drawn in the accent color, for the tested path.
    lead: bool,
}

impl Start {
    fn card(self, usb: Usb) -> Card {
        match (self, usb) {
            (Start::Connect, Usb::Attached(product)) => Card {
                glyph: Glyph::Usb,
                label: product,
                sub: "Attached. Every slot is in the Keyboard tab.",
                hint: "",
                lead: true,
            },
            (Start::Connect, _) => Card {
                glyph: Glyph::Usb,
                label: "Connect an instrument…",
                sub: "See every slot, pull sounds off to keep or edit, and put them back \
                      where you want them.",
                hint: "USB has been tested only with the Electro 5",
                lead: true,
            },
            (Start::Open, _) => Card {
                glyph: Glyph::FolderOpen,
                label: "Import files…",
                sub: "Programs, samples, pianos and set lists already on this computer.",
                hint: "",
                lead: false,
            },
            (Start::Demo, _) => Card {
                glyph: Glyph::AudioWaveform,
                label: "Start with a demo",
                sub: "A tine piano and a pad, in a Demo sounds folder on this computer.",
                hint: "From drawbar.app; anything already here is not added again",
                lead: false,
            },
        }
    }
}

/// The welcome sheet. `Some` once the reader has asked for something.
pub fn welcome(ctx: &egui::Context, usb: Usb) -> Option<Wanted> {
    egui::Modal::new(egui::Id::new("welcome"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| welcome_body(ui, usb))
        .inner
}

fn welcome_body(ui: &mut egui::Ui, usb: Usb) -> Option<Wanted> {
    ui.set_width(sheet::width(ui.ctx(), WELCOME_WIDTH));
    ui.add_space(GAP * 4.5);
    let mut wanted = sheet::section(ui, |ui| {
        sheet::masthead(ui);
        ui.add_space(GAP * 5.0);
        let wanted = starts(ui, usb);
        risk(ui);
        wanted
    });
    sheet::foot(
        ui,
        |ui| sheet::disclaimer(ui, DISMISS),
        |ui| {
            let done = sheet::primary(ui, None, "Continue")
                .on_hover_text("Help ▸ Welcome shows this again")
                .clicked();
            if done && wanted.is_none() {
                wanted = Some(Wanted::Done);
            }
        },
    );
    match escaped(ui) {
        true => Some(Wanted::Done),
        false => wanted,
    }
}

/// The alpha warning under the start cards.
fn risk(ui: &mut egui::Ui) {
    let tint = warn(ui.visuals());
    let ink = ui.visuals().strong_text_color();
    let mut job = egui::text::LayoutJob::default();
    for (text, family) in [
        (RISK_LEAD, bold()),
        (RISK_REST, egui::FontFamily::Proportional),
    ] {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::new(12.0, family),
                color: ink,
                ..Default::default()
            },
        );
    }
    ui.horizontal_top(|ui| {
        ui.add(sized(Glyph::TriangleAlert, 14.0, tint));
        ui.add(egui::Label::new(job).wrap());
    });
}

/// The narrowest a prose column gets, whatever the window width.
const LEAST_PROSE: f32 = 88.0;

/// A hairline across a boxed list, where the next row is about to start.
fn rule(ui: &mut egui::Ui, hairline: egui::Stroke) {
    let y = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), y, hairline);
}

/// One column of a row: `width` wide, its content laid out by `layout`.
fn cell(ui: &mut egui::Ui, width: f32, layout: egui::Layout, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), layout, |ui| {
        // ⚠️ Without this, an allocated ui reports only the width its contents took, and
        // the rest of the row slides left into the gap.
        ui.set_min_width(width);
        add(ui);
    });
}

/// A left-aligned column.
fn down() -> egui::Layout {
    egui::Layout::top_down(egui::Align::LEFT)
}

/// A start card's least width, the gap between two cards, and the padding inside one.
const CARD_LEAST: f32 = 210.0;
const CARD_GAP: f32 = 10.0;
const CARD_PAD: egui::Vec2 = egui::vec2(13.0, 11.0);
const CARD_STROKE: f32 = 1.0;

/// The text sizes of a card's title and sub-line.
const CARD_TITLE: f32 = 12.0;
const CARD_SUB: f32 = 10.5;

/// The cards, as many across as the sheet has room for.
fn starts(ui: &mut egui::Ui, usb: Usb) -> Option<Wanted> {
    let full = ui.available_width();
    let across = (((full + CARD_GAP) / (CARD_LEAST + CARD_GAP)) as usize).clamp(1, STARTS.len());
    let width = (full - CARD_GAP * (across - 1) as f32) / across as f32;
    // Every card is as tall as the tallest card's content, so a card without a sub-line
    // matches its neighbors. When that height changes, the frame is discarded and
    // redrawn.
    let told = ui.id().with("starts");
    let height: f32 = ui.data(|data| data.get_temp(told)).unwrap_or(0.0);
    let mut tallest: f32 = 0.0;
    let mut wanted = None;
    for row in STARTS.chunks(across) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for start in row {
                let reachable = !matches!((start, usb), (Start::Connect, Usb::Missing));
                let (drawn, needs) = ui
                    .add_enabled_ui(reachable, |ui| card(ui, start.card(usb), width, height))
                    .inner;
                let drawn = drawn.on_disabled_hover_text(NO_USB);
                tallest = tallest.max(needs);
                if !drawn.clicked() {
                    continue;
                }
                match start {
                    Start::Connect => {
                        wanted = Some(Wanted::Act(match usb {
                            Usb::Attached(_) => Act::ShowTab(Spot::Keyboard),
                            Usb::Missing | Usb::Ready => Act::Connect,
                        }))
                    }
                    Start::Open => wanted = Some(Wanted::Act(Act::OpenFiles)),
                    Start::Demo => wanted = Some(Wanted::Act(Act::FetchDemos)),
                }
            }
        });
        ui.add_space(CARD_GAP);
    }
    if tallest != height {
        ui.data_mut(|data| data.insert_temp(told, tallest));
        ui.ctx().request_discard("start cards");
    }
    wanted
}

/// The id of a card's response, the same whichever row the card lands in.
fn card_id(label: &str) -> egui::Id {
    egui::Id::new(("start card", label))
}

/// The room a card `width` wide leaves for its text.
fn inner(width: f32) -> f32 {
    width - 2.0 * (CARD_PAD.x + CARD_STROKE)
}

/// A card `width` wide and at least `height` tall, the card's own frame included, and the
/// height its content alone needs.
fn card(ui: &mut egui::Ui, card: Card, width: f32, height: f32) -> (egui::Response, f32) {
    let frame = 2.0 * (CARD_PAD.y + CARD_STROKE);
    let accent = accent(ui.visuals());
    let (stroke, fill, tint) = match card.lead {
        true => (
            egui::Stroke::new(CARD_STROKE, accent),
            ui.visuals().widgets.active.bg_fill,
            accent,
        ),
        false => (
            ui.visuals().widgets.noninteractive.bg_stroke,
            egui::Color32::TRANSPARENT,
            ui.visuals().weak_text_color(),
        ),
    };
    let drawn = egui::Frame::new()
        .stroke(stroke)
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(2))
        .inner_margin(egui::Margin::symmetric(CARD_PAD.x as i8, CARD_PAD.y as i8))
        .show(ui, |ui| {
            // ⚠️ A frame's content inherits the layout it was opened in, and the cards
            // are laid out in a row.
            ui.vertical(|ui| {
                ui.set_width(inner(width));
                // ⚠️ Before the content: a minimum height is reserved from the cursor down.
                ui.set_min_height((height - frame).max(0.0));
                ui.spacing_mut().item_spacing.y = GAP;
                let content = ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.add(sized(card.glyph, 14.0, tint));
                        ui.label(egui::RichText::new(card.label).strong().size(CARD_TITLE));
                    });
                    if !card.sub.is_empty() {
                        ui.add(
                            egui::Label::new(egui::RichText::new(card.sub).size(CARD_SUB).weak())
                                .wrap(),
                        );
                    }
                });
                content.response.rect.height() + frame
            })
            .inner
        });
    let needs = drawn.inner;
    let rect = drawn.response.rect;
    let response = ui
        .interact(rect, card_id(card.label), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0_f32, accent),
            egui::StrokeKind::Inside,
        );
    }
    let response = match card.hint.is_empty() {
        true => response,
        false => response.on_hover_text(card.hint),
    };
    (response, needs)
}

fn escaped(ui: &egui::Ui) -> bool {
    ui.input(|input| input.key_pressed(egui::Key::Escape))
}

/// The release notes, and how far fetching them has gone.
pub enum Notes {
    /// Nobody has asked for them yet.
    Unasked,
    Loading,
    Read {
        body: String,
        page: String,
    },
    /// No network, no such tag yet, a rate limit, or a body too long to be shown.
    Unavailable,
}

/// The kind of change a release-notes heading introduces. The sheet shows only these:
/// `scripts/release.bash` files work a user does not see under `Other changes`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    New,
    Fixed,
    Faster,
}

impl Change {
    /// The change a heading introduces, or `None` for a section the sheet leaves out.
    pub fn read(heading: &str) -> Option<Change> {
        match heading.trim() {
            "Features" => Some(Change::New),
            "Bug fixes" => Some(Change::Fixed),
            "Performance" => Some(Change::Faster),
            _ => None,
        }
    }

    /// What the sheet calls this section.
    pub fn title(self) -> &'static str {
        match self {
            Change::New => "New",
            Change::Fixed => "Fixed",
            Change::Faster => "Faster",
        }
    }

    /// The glyph every row of this section wears, unless it is breaking.
    fn glyph(self) -> Glyph {
        match self {
            Change::New => Glyph::Sparkles,
            Change::Fixed => Glyph::Wrench,
            Change::Faster => Glyph::Gauge,
        }
    }
}

/// One heading of the notes and the items beneath it.
struct Section<'a> {
    change: Change,
    items: Vec<Item<'a>>,
}

/// The sections the sheet shows, in the order the body gives them.
fn sections(body: &str) -> Vec<Section<'_>> {
    let mut read: Vec<(Option<Change>, Vec<Item<'_>>)> = vec![(None, Vec::new())];
    for line in body.lines().map(classify) {
        match line {
            Line::Heading(heading) => read.push((Change::read(heading), Vec::new())),
            Line::Item(item) => {
                if let Some((_, items)) = read.last_mut() {
                    items.push(item);
                }
            }
            Line::Blank | Line::Changelog(_) | Line::Text(_) => {}
        }
    }
    read.into_iter()
        .filter(|(_, items)| !items.is_empty())
        .filter_map(|(change, items)| {
            Some(Section {
                change: change?,
                items,
            })
        })
        .collect()
}

/// The compare link a release body ends with.
fn changelog(body: &str) -> Option<&str> {
    body.lines().find_map(|line| match classify(line) {
        Line::Changelog(url) => Some(url),
        _ => None,
    })
}

/// The change count shown beside a section's title.
fn tally(count: usize) -> String {
    const WORDS: [&str; 9] = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];

    let many = count
        .checked_sub(1)
        .and_then(|at| WORDS.get(at))
        .map_or_else(|| count.to_string(), |word| (*word).to_owned());
    match count {
        1 => format!("{many} change"),
        _ => format!("{many} changes"),
    }
}

/// The change list. Returns whether the reader is done with it.
pub fn news(ctx: &egui::Context, notes: &Notes) -> bool {
    egui::Modal::new(egui::Id::new("news"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| news_body(ui, notes))
        .inner
}

fn news_body(ui: &mut egui::Ui, notes: &Notes) -> bool {
    ui.set_width(sheet::width(ui.ctx(), NEWS_WIDTH));
    egui::ScrollArea::vertical()
        .id_salt("news")
        .max_height(sheet::middle(ui.ctx(), AROUND, FEWEST))
        .show(ui, |ui| {
            ui.add_space(GAP * 4.0);
            sheet::section(ui, |ui| {
                headline(ui);
                notes_body(ui, notes);
            });
            ui.add_space(GAP * 2.0);
        });
    let mut done = false;
    sheet::foot(
        ui,
        |ui| {
            let quiet = ui.visuals().weak_text_color();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add(sized(Glyph::HardDriveDownload, 12.0, quiet));
                ui.label(
                    egui::RichText::new("drawbar is alpha software. Keep your own backups.")
                        .size(10.5)
                        .weak(),
                );
            });
        },
        |ui| {
            done = sheet::primary(ui, None, "Continue").clicked();
            if let Notes::Read { body, page } = notes {
                sheet::link(ui, "Release page", page);
                if let Some(url) = changelog(body) {
                    sheet::link(ui, "Full changelog", url);
                }
            }
        },
    );
    done || escaped(ui)
}

/// What changed, and in which version.
fn headline(ui: &mut egui::Ui) {
    let accent = accent(ui.visuals());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        ui.label(egui::RichText::new("What changed").font(egui::FontId::new(14.0, bold())));
        ui.label(
            egui::RichText::new(VERSION)
                .font(egui::FontId::monospace(12.0))
                .color(accent),
        );
    });
}

fn notes_body(ui: &mut egui::Ui, notes: &Notes) {
    match notes {
        Notes::Unasked | Notes::Loading => {
            ui.add_space(GAP * 3.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Reading the release notes…").weak());
            });
        }
        Notes::Read { body, .. } => {
            let sections = sections(body);
            if sections.is_empty() {
                ui.add_space(GAP * 3.0);
                ui.label(egui::RichText::new(QUIET_RELEASE).weak());
            }
            let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
            for section in sections {
                section_head(ui, &section);
                for item in section.items {
                    change_row(ui, section.change, item);
                    rule(ui, hairline);
                }
            }
        }
        Notes::Unavailable => {
            ui.add_space(GAP * 3.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Release notes are unavailable.").weak());
                sheet::link(ui, "Releases", crate::about::RELEASES);
            });
        }
    }
}

/// What the sheet says for a release with nothing under the headings it shows.
const QUIET_RELEASE: &str = "Behind-the-scenes changes only.";

fn section_head(ui: &mut egui::Ui, section: &Section<'_>) {
    ui.add_space(GAP * 4.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(section.change.title())
                .font(egui::FontId::new(12.0, bold()))
                .color(ui.visuals().strong_text_color()),
        );
        ui.label(
            egui::RichText::new(tally(section.items.len()))
                .size(10.5)
                .weak(),
        );
    });
    ui.add_space(GAP * 1.5);
}

/// The width of the column holding an item's glyph.
const BADGE: f32 = 14.0;

fn change_row(ui: &mut egui::Ui, change: Change, item: Item<'_>) {
    let (glyph, tint) = match item.breaking {
        true => (Glyph::TriangleAlert, warn(ui.visuals())),
        false => (change.glyph(), good(ui.visuals())),
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        cell(ui, BADGE, down(), |ui| {
            ui.add_space(2.0);
            ui.add(sized(glyph, 12.0, tint));
        });
        let rest = ui.available_width().max(LEAST_PROSE);
        cell(ui, rest, down(), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(item.text).size(11.5)).wrap());
        });
    });
    ui.add_space(GAP);
}

/// One line of the release notes, classified for drawing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Line<'a> {
    Blank,
    /// `### Features`
    Heading(&'a str),
    /// `- ⚠️ **scope:** description (#NN) ([sha](url))`, every part but the
    /// description optional.
    Item(Item<'a>),
    /// `**Full changelog**: <url>`
    Changelog(&'a str),
    Text(&'a str),
}

/// One change, as a reader sees it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Item<'a> {
    /// The commit changes how something already worked.
    pub breaking: bool,
    pub text: &'a str,
}

/// Read one line of a release body.
pub fn classify(line: &str) -> Line<'_> {
    let line = line.trim_end();
    if line.is_empty() {
        return Line::Blank;
    }
    if let Some(title) = line.strip_prefix("### ") {
        return Line::Heading(title);
    }
    if let Some(url) = line.strip_prefix("**Full changelog**: ") {
        if let Some(url) = https(url) {
            return Line::Changelog(url);
        }
    }
    let Some(item) = line.strip_prefix("- ") else {
        return Line::Text(line);
    };
    let (breaking, item) = match item.strip_prefix(BREAKING) {
        Some(item) => (true, item),
        None => (false, item),
    };
    let text = without_pr(without_scope(without_commit(item)));
    Line::Item(Item { breaking, text })
}

/// The mark `scripts/release.bash` puts before a breaking item.
const BREAKING: &str = "\u{26a0}\u{fe0f} ";

/// `url`, when it is `https`. Only these are offered as links: the body is fetched text,
/// and `scripts/release.bash` writes only https URLs.
fn https(url: &str) -> Option<&str> {
    url.strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty())
        .then_some(url)
}

/// The item without the `([sha](url))` it ends with.
fn without_commit(item: &str) -> &str {
    let commit = item.strip_suffix("))").and_then(|rest| {
        let at = rest.rfind(" ([")?;
        rest[at..].contains("](").then(|| &rest[..at])
    });
    commit.unwrap_or(item)
}

/// The text without the `(#NN)` a squashed pull request leaves at its end. A `#NN`
/// anywhere else is part of what the item says.
fn without_pr(text: &str) -> &str {
    const OPEN: &str = " (#";
    let pr = text.strip_suffix(')').and_then(|rest| {
        let at = rest.rfind(OPEN)?;
        let number = &rest[at + OPEN.len()..];
        (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| &rest[..at])
    });
    pr.unwrap_or(text)
}

/// The description after the `**scope:**` an item may open with.
fn without_scope(item: &str) -> &str {
    // A description of its own can hold `:** `; a scope never holds a star.
    let scope = item
        .strip_prefix("**")
        .and_then(|rest| rest.split_once(":** "))
        .filter(|(scope, _)| !scope.contains('*'));
    scope.map_or(item, |(_, text)| text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, Word};

    /// Part of `drawbar-v0.10.0`'s commits, as `scripts/release.bash` writes them.
    const RELEASED: &str = "\
### Features
- ⚠️ **drawbar:** drawbar ui v3 + per-platform style and behavior (#147) ([80f5e95](https://github.com/jmoo/drawbar/commit/80f5e955a5b5a7fd9e0f7bb0f087548129e359c7))
- **drawbar:** offer Load on instrument in a linked document's header (#124) ([8b70284](https://github.com/jmoo/drawbar/commit/8b7028422b9e0dae4ca0c26e2624203bbd8571ca))

### Bug fixes
- **drawbar:** refuse a queue drag the attached instrument does not take (#138) ([1d56f76](https://github.com/jmoo/drawbar/commit/1d56f76de008711998b5c2e80b7ab5d60360b7b6))

### Other changes
- ⚠️ simplify code across the workspace (#136) ([ec10e2c](https://github.com/jmoo/drawbar/commit/ec10e2c3b9ea76bb8d6bca4e6c1c46283a51e423))

**Full changelog**: https://github.com/jmoo/drawbar/compare/drawbar-v0.9.1...drawbar-v0.10.0";

    fn headless() -> egui::Context {
        let ctx = testing::context();
        egui_extras::install_image_loaders(&ctx);
        ctx
    }

    /// Draw `add` on a screen this size and report what it painted, and where.
    fn drawn_at(
        ctx: &egui::Context,
        size: egui::Vec2,
        add: impl FnMut(&egui::Context),
    ) -> Vec<Word> {
        testing::painted(&testing::run(ctx, testing::screen(size, Vec::new()), add))
    }

    /// The painted bounds of a text, or `None` when it was never painted.
    fn box_of(said: &[Word], word: &str) -> Option<egui::Rect> {
        said.iter()
            .find(|painted| painted.text == word)
            .map(|painted| painted.bounds)
    }

    #[test]
    fn a_first_run_is_welcomed_an_update_says_what_changed_and_a_read_version_opens_nothing() {
        assert_eq!(opening(None), Opening::Welcome);
        assert_eq!(opening(Some("0.0.1")), Opening::News);
        assert_eq!(opening(Some(VERSION)), Opening::Nothing);
    }

    #[test]
    fn a_squashed_pull_request_number_leaves_the_text_and_a_mention_of_one_stays_in_it() {
        assert_eq!(
            without_pr("ship the v2 sample encoder (#83)"),
            "ship the v2 sample encoder"
        );
        assert_eq!(
            without_pr("undo the truncation #12 introduced"),
            "undo the truncation #12 introduced"
        );
        assert_eq!(
            without_pr("a trailer with no number (#)"),
            "a trailer with no number (#)"
        );
    }

    #[test]
    fn the_sheet_shows_features_fixes_and_speedups_and_no_other_heading() {
        for (heading, title) in [
            ("Features", "New"),
            ("Bug fixes", "Fixed"),
            ("Performance", "Faster"),
        ] {
            assert_eq!(
                Change::read(heading).map(Change::title),
                Some(title),
                "### {heading}"
            );
        }
        for heading in [
            "Other changes",
            "\u{26a0}\u{fe0f} Breaking changes",
            "Acknowledgements",
        ] {
            assert_eq!(Change::read(heading), None, "### {heading}");
        }
    }

    #[test]
    fn a_section_says_how_many_changes_it_holds() {
        assert_eq!(tally(1), "one change");
        assert_eq!(tally(3), "three changes");
        assert_eq!(tally(12), "12 changes");
    }

    #[test]
    fn only_what_a_user_notices_reaches_the_sheet_and_the_compare_link_is_left_for_the_foot() {
        let read: Vec<(&str, Vec<Item<'_>>)> = sections(RELEASED)
            .into_iter()
            .map(|section| (section.change.title(), section.items))
            .collect();
        let item = |breaking, text| Item { breaking, text };
        assert_eq!(
            read,
            vec![
                (
                    "New",
                    vec![
                        item(true, "drawbar ui v3 + per-platform style and behavior"),
                        item(
                            false,
                            "offer Load on instrument in a linked document's header"
                        ),
                    ]
                ),
                (
                    "Fixed",
                    vec![item(
                        false,
                        "refuse a queue drag the attached instrument does not take"
                    )]
                ),
            ]
        );
        assert_eq!(
            changelog(RELEASED),
            Some("https://github.com/jmoo/drawbar/compare/drawbar-v0.9.1...drawbar-v0.10.0")
        );
    }

    #[test]
    fn items_before_any_heading_are_left_out() {
        assert!(sections("- a line with no heading\n### Features\n").is_empty());
    }

    /// The shell refuses a smaller screen than this, so both sheets must fit in it with
    /// their dismiss button on screen, and the welcome with its warning and every start
    /// card too.
    #[test]
    fn the_welcome_sheet_keeps_its_button_and_its_choices_on_the_smallest_screen() {
        let ctx = headless();
        let size = crate::shell::LEAST;
        // Three times: each frame lays out against the one before, and the cards settle
        // their own height first.
        let mut said = Vec::new();
        for _ in 0..3 {
            said = drawn_at(&ctx, size, |ctx| {
                welcome(ctx, Usb::Ready);
            });
        }

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let risk = format!("{RISK_LEAD}{RISK_REST}");
        let starts = STARTS.map(|start| start.card(Usb::Ready).label);
        for label in ["Continue", risk.as_str()].into_iter().chain(starts) {
            let drawn = box_of(&said, label)
                .unwrap_or_else(|| panic!("{label} was never painted: {said:?}"));
            assert!(
                screen.contains_rect(drawn.expand(6.0)),
                "{label} is off a {size:?} screen: {drawn:?}"
            );
        }
    }

    #[test]
    fn the_news_sheet_keeps_its_button_on_the_smallest_screen_the_shell_allows() {
        let ctx = headless();
        let size = crate::shell::LEAST;
        let notes = Notes::Read {
            body: RELEASED.to_owned(),
            page: "https://github.com/jmoo/drawbar/releases/tag/drawbar-v0.5.0".to_owned(),
        };
        let _ = drawn_at(&ctx, size, |ctx| {
            news(ctx, &notes);
        });
        let said = drawn_at(&ctx, size, |ctx| {
            news(ctx, &notes);
        });

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let button =
            box_of(&said, "Continue").unwrap_or_else(|| panic!("no Continue button: {said:?}"));
        assert!(
            screen.contains_rect(button.expand(6.0)),
            "the button is off a {size:?} screen: {button:?}"
        );
        assert!(box_of(&said, "New").is_some(), "{said:?}");
        assert!(
            box_of(&said, "simplify code across the workspace").is_none(),
            "{said:?}"
        );
    }

    #[test]
    fn a_release_with_nothing_a_user_notices_says_so() {
        let ctx = headless();
        let notes = Notes::Read {
            body: "### Other changes\n- simplify code across the workspace\n".to_owned(),
            page: "https://github.com/jmoo/drawbar/releases".to_owned(),
        };
        let size = egui::vec2(1200.0, 900.0);
        let _ = drawn_at(&ctx, size, |ctx| {
            news(ctx, &notes);
        });
        let said = drawn_at(&ctx, size, |ctx| {
            news(ctx, &notes);
        });
        assert!(box_of(&said, QUIET_RELEASE).is_some(), "{said:?}");
    }

    #[test]
    fn without_usb_the_connect_card_is_grayed_out_and_says_why_on_hover() {
        let ctx = headless();
        let size = egui::vec2(1200.0, 900.0);
        let frame = |events: Vec<egui::Event>| {
            let input = testing::screen(size, events);
            let mut wanted = None;
            let output = testing::run(&ctx, input, |ctx| wanted = welcome(ctx, Usb::Missing));
            (wanted, testing::painted(&output))
        };
        // The sheet centers itself, and the cards match heights, over the first frames.
        for _ in 0..3 {
            let _ = frame(Vec::new());
        }
        let at = ctx
            .read_response(card_id("Connect an instrument…"))
            .expect("the sheet offers to connect")
            .rect
            .center();

        let _ = frame(vec![egui::Event::PointerMoved(at)]);
        for _ in 0..60 {
            let _ = frame(Vec::new());
        }
        let (_, said) = frame(Vec::new());
        assert!(box_of(&said, NO_USB).is_some(), "{said:?}");

        let (wanted, _) = frame(vec![testing::button(at, true), testing::button(at, false)]);
        assert!(wanted.is_none(), "the grayed card asked to connect");
    }

    #[test]
    fn with_an_instrument_attached_the_first_card_names_it_and_shows_its_slots() {
        let ctx = headless();
        let size = egui::vec2(1200.0, 900.0);
        let usb = Usb::Attached("Nord Electro 5");
        let frame = |events: Vec<egui::Event>| {
            let input = testing::screen(size, events);
            let mut wanted = None;
            let output = testing::run(&ctx, input, |ctx| wanted = welcome(ctx, usb));
            (wanted, testing::painted(&output))
        };
        for _ in 0..3 {
            let _ = frame(Vec::new());
        }
        let (_, said) = frame(Vec::new());
        assert!(
            box_of(&said, "Connect an instrument…").is_none(),
            "{said:?}"
        );
        let at = ctx
            .read_response(card_id("Nord Electro 5"))
            .expect("a card names the instrument")
            .rect
            .center();

        let (wanted, _) = frame(vec![testing::button(at, true), testing::button(at, false)]);
        assert!(
            matches!(wanted, Some(Wanted::Act(Act::ShowTab(Spot::Keyboard)))),
            "the card asked for something else"
        );
    }

    /// How tall the start cards stand on a screen `width` wide, once they have settled.
    fn cards_tall(ctx: &egui::Context, width: f32) -> f32 {
        let mut tall = 0.0;
        for _ in 0..2 {
            let _ = drawn_at(ctx, egui::vec2(width, 900.0), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let top = ui.cursor().top();
                    starts(ui, Usb::Ready);
                    tall = ui.cursor().top() - top;
                });
            });
        }
        tall
    }

    #[test]
    fn the_start_cards_shrink_back_when_the_screen_widens() {
        let wide = cards_tall(&headless(), 1200.0);
        let ctx = headless();
        let narrow = cards_tall(&ctx, 240.0);
        assert!(
            narrow > wide,
            "narrow {narrow} is not taller than wide {wide}"
        );
        assert_eq!(cards_tall(&ctx, 1200.0), wide);
    }

    /// The smallest window wraps the cards onto two rows, and a wide one keeps them on one.
    #[test]
    fn the_start_cards_are_one_size_at_every_screen_width() {
        let least = crate::shell::LEAST;
        let (mut wrapped, mut one_row) = (false, false);
        for width in [least.x, 700.0, 800.0, 1000.0, 1600.0] {
            let ctx = headless();
            let size = egui::vec2(width, least.y);
            for _ in 0..3 {
                let _ = drawn_at(&ctx, size, |ctx| {
                    welcome(ctx, Usb::Ready);
                });
            }
            let rects: Vec<(&str, egui::Rect)> = STARTS
                .iter()
                .map(|start| {
                    let label = start.card(Usb::Ready).label;
                    let rect = ctx
                        .read_response(card_id(label))
                        .unwrap_or_else(|| panic!("{label} was never laid out at {width}"))
                        .rect;
                    (label, rect)
                })
                .collect();
            let (first, one) = rects[0];
            for (label, rect) in &rects[1..] {
                assert_eq!(
                    rect.size(),
                    one.size(),
                    "at {width} wide, {label} is {rect:?} and {first} is {one:?}"
                );
            }
            match rects.iter().all(|(_, rect)| rect.top() == one.top()) {
                true => one_row = true,
                false => wrapped = true,
            }
        }
        assert!(wrapped && one_row, "wrapped {wrapped}, one row {one_row}");
    }

    #[test]
    fn an_unscoped_item_keeps_its_whole_description() {
        assert_eq!(
            classify("- say what changed"),
            Line::Item(Item {
                breaking: false,
                text: "say what changed",
            })
        );
    }

    #[test]
    fn a_breaking_mark_scope_pull_request_and_commit_leave_only_the_description() {
        assert_eq!(
            classify("- \u{26a0}\u{fe0f} **drawbar:** say what changed (#12) ([abc1234](https://example.com/c))"),
            Line::Item(Item {
                breaking: true,
                text: "say what changed",
            })
        );
    }

    #[test]
    fn a_changelog_line_that_is_not_https_is_not_a_link() {
        assert_eq!(
            classify("**Full changelog**: ftp://example.com/log"),
            Line::Text("**Full changelog**: ftp://example.com/log")
        );
    }

    #[test]
    fn a_line_the_shape_does_not_cover_stays_as_it_reads() {
        assert_eq!(
            classify("  Ordinary prose."),
            Line::Text("  Ordinary prose.")
        );
        assert_eq!(classify("   "), Line::Blank);
    }
}
