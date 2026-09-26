//! The two sheets a session can open on: the welcome a first run shows, and what
//! changed since the version last read.
//!
//! The notes are GitHub's release body for `drawbar-v<version>`, which
//! `scripts/release.bash` writes in a fixed shape. [`classify`] reads that shape back so
//! the sheet can paint it without a markdown parser, and anything it does not recognise
//! stays the plain line it was.
//!
//! Both sheets are painted here, on every target, because Help opens the welcome in a
//! window as well as in a tab. Only the rule that opens one unasked and the fetch behind
//! the notes are the browser's.

use eframe::egui;

use crate::app::{accent, bold, caption, good, unlit, warn};
use crate::browser::Act;
use crate::device::{NO_USB, NO_USB_BRIEF};
use crate::icon::{sized, Glyph};
use crate::panel::caps;
use crate::sheet::{self, GAP};
use crate::shell::{BROWSERS, GUIDE};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::Splash;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::Splash;

pub(crate) use crate::sheet::VERSION;

/// The most each sheet is allowed to be wide.
const WELCOME_WIDTH: f32 = 940.0;
const NEWS_WIDTH: f32 = 700.0;

/// The room a sheet keeps for its foot, so a short window scrolls the middle rather than
/// pushing the one button that dismisses it off screen.
const AROUND: f32 = 96.0;

/// The middle is never shorter than this, however short the window.
const FEWEST: f32 = 120.0;

/// The room the welcome's foot keeps for its one button, at the right of the disclaimer.
const LET_IN: f32 = 200.0;

/// Which sheet a session opens on, given the version whose sheet was last dismissed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opening {
    /// Nobody has run drawbar here before.
    Welcome,
    /// A version was read, and it is not the one running.
    News,
    Nothing,
}

/// The opening rule: a first run is welcomed, an update says what changed, and a version
/// already read opens on the app itself.
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

/// How far one column's claim has been borne out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Yes,
    /// Implemented, or implemented in part, but not borne out on an instrument.
    Partly,
    No,
}

/// One column of a [`Row`], and what it claims in the words the pointer shows.
pub struct Claim {
    pub mark: Mark,
    pub hint: &'static str,
}

const fn claim(mark: Mark, hint: &'static str) -> Claim {
    Claim { mark, hint }
}

/// One line of [`SUPPORT`]: what the files are, the four columns, and what the row means
/// in a sentence.
pub struct Row {
    pub instrument: &'static str,
    pub kinds: &'static str,
    /// In the order of [`COLUMNS`].
    pub marks: [Claim; 4],
    pub note: &'static str,
}

/// The columns [`Row::marks`] answers.
pub const COLUMNS: [&str; 4] = ["Read", "Edit", "Send", "Tested"];

/// What works today. Every claim here is one `docs/src/getting-started/support.md` makes:
/// that page is where a claim is argued, and this is where it is shown.
pub const SUPPORT: &[Row] = &[
    Row {
        instrument: "Nord Electro 5",
        kinds: "programs · live · settings · set lists",
        marks: [
            claim(
                Mark::Yes,
                "Views programs, live slots, set lists and settings",
            ),
            claim(
                Mark::Yes,
                "Edits programs, live slots, set lists and settings",
            ),
            claim(
                Mark::Yes,
                "Transfers from macOS, Linux and the browser. Windows builds have not \
                 been run against an instrument",
            ),
            claim(Mark::Yes, "Tested on an Electro 5"),
        ],
        note: "Tested on an Electro 5 from macOS, Linux and the browser. Windows builds \
               have not been run against an instrument.",
    },
    Row {
        instrument: "Piano and sample files",
        kinds: "npno · nsmp · nsmp3 · nsmp4",
        marks: [
            claim(Mark::Yes, "Decodes all four"),
            claim(
                Mark::Yes,
                "Samples: edit, encode, audition. Pianos: trim, split, rename, retune, \
                 remap, build from WAVs",
            ),
            claim(Mark::Yes, "Transfers to an Electro 5"),
            claim(
                Mark::Partly,
                "Samples encoded as v3 or v4, and pianos renamed, retuned, remapped or \
                 narrowed, have not been played",
            ),
        ],
        note: "Sample files encoded as v3 or v4, and pianos that were renamed, retuned, \
               remapped or narrowed, have not been played on an instrument.",
    },
    Row {
        instrument: "Nord Stage 2 · 3 · 4",
        kinds: "programs · presets",
        marks: [
            claim(Mark::Yes, "Views programs and presets"),
            claim(Mark::Yes, "Edits programs and presets"),
            claim(
                Mark::No,
                "No Stage has been connected, so USB support cannot be guaranteed",
            ),
            claim(Mark::No, "Not tested on an instrument"),
        ],
        note: "Not tested on an instrument.",
    },
    Row {
        instrument: "Every other Nord",
        kinds: "any file it recognises",
        marks: [
            claim(
                Mark::Partly,
                "Recognised and kept byte for byte, without decoding what is inside",
            ),
            claim(Mark::No, "Nothing is decoded for these models yet"),
            claim(
                Mark::No,
                "No other instrument has been connected, so USB support cannot be \
                 guaranteed",
            ),
            claim(Mark::No, "Not tested on an instrument"),
        ],
        note: "Recognised and kept byte for byte, without editing.",
    },
];

/// The lead of the risk box, in bold, and the rest of it.
const RISK_LEAD: &str = "Keep your own backups.";
const RISK_REST: &str =
    " This is alpha — treat what is in drawbar as a working copy, not an archive.";

/// The three ways in, in the order the sheet offers them. A browser that cannot
/// connect is shown the browsers that can.
fn offered(usb: bool) -> [Start; 3] {
    let first = match usb {
        true => Start::Connect,
        false => Start::Browsers,
    };
    [first, Start::Open, Start::Guide]
}

#[derive(Clone, Copy)]
enum Start {
    Connect,
    Browsers,
    Open,
    Guide,
}

/// How one [`Start`] reads on the sheet.
struct Card {
    glyph: Glyph,
    label: &'static str,
    sub: &'static str,
    hint: &'static str,
    /// The tested path, drawn in the accent.
    lead: bool,
}

impl Start {
    const fn card(self) -> Card {
        match self {
            Start::Connect => Card {
                glyph: Glyph::Usb,
                label: "Connect an instrument…",
                sub: "See every slot, pull sounds off to keep or edit, and put them back \
                      where you want them.",
                hint: "Electro 5 over USB is the tested path",
                lead: true,
            },
            Start::Browsers => Card {
                glyph: Glyph::Usb,
                label: NO_USB_BRIEF,
                sub: NO_USB,
                hint: "",
                lead: true,
            },
            Start::Open => Card {
                glyph: Glyph::FolderOpen,
                label: "Open files…",
                sub: "Programs, samples, pianos and set lists already on this computer.",
                hint: "",
                lead: false,
            },
            Start::Guide => Card {
                glyph: Glyph::BookOpen,
                label: "Read the guide",
                sub: "",
                hint: "",
                lead: false,
            },
        }
    }
}

/// The welcome sheet. `Some` once the reader has asked for something.
pub fn welcome(ctx: &egui::Context, usb: bool) -> Option<Wanted> {
    egui::Modal::new(egui::Id::new("welcome"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| welcome_body(ui, usb))
        .inner
}

fn welcome_body(ui: &mut egui::Ui, usb: bool) -> Option<Wanted> {
    ui.set_width(sheet::width(ui.ctx(), WELCOME_WIDTH));
    let mut wanted = None;
    egui::ScrollArea::vertical()
        .id_salt("welcome")
        .max_height(sheet::middle(ui.ctx(), AROUND, FEWEST))
        .show(ui, |ui| {
            ui.add_space(GAP * 4.5);
            sheet::section(ui, |ui| {
                sheet::masthead(ui, true);
                ui.add_space(GAP * 3.5);
                risk(ui);
                sheet::heading(ui, "What works today", None);
                support(ui);
                ui.add_space(GAP * 2.0);
                legend(ui);
                sheet::heading(ui, "Start here", None);
                wanted = starts(ui, usb);
            });
        });
    sheet::foot(
        ui,
        |ui| sheet::disclaimer(ui, LET_IN),
        |ui| {
            let done = sheet::primary(ui, Some(Glyph::Check), "I understand — let me in")
                .on_hover_text("You can read all of this again from the Help menu")
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

/// What this build is, before anything it can do.
fn risk(ui: &mut egui::Ui) {
    let tint = warn(ui.visuals());
    let ink = ui.visuals().strong_text_color();
    let faint = ui.visuals().faint_bg_color;
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
    egui::Frame::new()
        .fill(faint)
        .stroke(egui::Stroke::new(1.0_f32, tint))
        .corner_radius(egui::CornerRadius::same(2))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.add_space(-2.0);
                ui.add(sized(Glyph::TriangleAlert, 14.0, tint));
                ui.add(egui::Label::new(job).wrap());
            });
        });
}

/// The room a cell keeps from the table's edge, and the room between two columns.
const CELL: f32 = 12.0;
const COLGAP: f32 = 10.0;

/// The three mark columns, and the wider one that says whether a row was tried on
/// hardware.
const DOT: f32 = 52.0;
const TESTED: f32 = 82.0;

/// The mark itself.
const MARK: f32 = 9.0;

/// Neither prose column is squeezed past this, whatever the window does.
const LEAST_PROSE: f32 = 88.0;

/// The instrument column and the note column, at the room the table has left for them.
fn prose(full: f32) -> (f32, f32) {
    let fixed = DOT * 3.0 + TESTED + COLGAP * 5.0 + CELL * 2.0;
    let free = (full - fixed).max(2.0 * LEAST_PROSE);
    (free * 1.5 / 3.2, free * 1.7 / 3.2)
}

/// [`SUPPORT`] as a table: a head, then one row per line of it, hairlines between.
fn support(ui: &mut egui::Ui) {
    let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
    let faint = ui.visuals().faint_bg_color;
    egui::Frame::new()
        .stroke(hairline)
        .corner_radius(egui::CornerRadius::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let widths = prose(ui.available_width());
            egui::Frame::new()
                .fill(faint)
                .inner_margin(egui::Margin::symmetric(CELL as i8, 5))
                .show(ui, |ui| head(ui, widths));
            for row in SUPPORT {
                rule(ui, hairline);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(CELL as i8, 7))
                    .show(ui, |ui| support_row(ui, widths, row));
            }
        });
}

/// The column heads, in MICRO-caps over the cells they name.
fn head(ui: &mut egui::Ui, widths: (f32, f32)) {
    let ink = caption(ui.visuals());
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = COLGAP;
        cell(ui, widths.0, down(), |ui| {
            ui.label(caps("Instrument").color(ink));
        });
        for (column, width) in COLUMNS.iter().zip([DOT, DOT, DOT, TESTED]) {
            cell(ui, width, middle(), |ui| {
                ui.label(caps(column).color(ink));
            });
        }
        cell(ui, widths.1, down(), |ui| {
            ui.label(caps("What that means").color(ink));
        });
    });
}

fn support_row(ui: &mut egui::Ui, widths: (f32, f32), row: &Row) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = COLGAP;
        cell(ui, widths.0, down(), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(
                egui::Label::new(egui::RichText::new(row.instrument).strong().size(12.0)).wrap(),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(row.kinds)
                        .font(egui::FontId::monospace(10.0))
                        .weak(),
                )
                .wrap(),
            );
        });
        for (claim, width) in row.marks.iter().zip([DOT, DOT, DOT, TESTED]) {
            cell(ui, width, middle(), |ui| {
                ui.add_space(3.0);
                mark(ui, claim.mark).on_hover_text(claim.hint);
            });
        }
        cell(ui, widths.1, down(), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(row.note).size(11.0)).wrap());
        });
    });
}

/// A hairline across a boxed list, where the next row is about to start.
fn rule(ui: &mut egui::Ui, hairline: egui::Stroke) {
    let y = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), y, hairline);
}

/// One column of a row: `width` wide, its content laid out by `layout`.
fn cell(ui: &mut egui::Ui, width: f32, layout: egui::Layout, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), layout, |ui| {
        // ⚠️ A column narrower than the one asked for: an allocated ui gives its parent
        // only the room its contents took, and the rest of the row would slide into it.
        ui.set_min_width(width);
        add(ui);
    });
}

/// A column that reads down from its left edge, which is most of them.
fn down() -> egui::Layout {
    egui::Layout::top_down(egui::Align::LEFT)
}

/// A column whose one mark sits in the middle of it.
fn middle() -> egui::Layout {
    egui::Layout::top_down(egui::Align::Center)
}

/// A claim that holds is a lit dot; one that does not is the ring where a dot would be.
fn mark(ui: &mut egui::Ui, mark: Mark) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(MARK), egui::Sense::hover());
    let (fill, ring) = match mark {
        Mark::Yes => (good(ui.visuals()), egui::Stroke::NONE),
        Mark::Partly => (warn(ui.visuals()), egui::Stroke::NONE),
        Mark::No => (
            egui::Color32::TRANSPARENT,
            egui::Stroke::new(1.0_f32, unlit(ui.visuals())),
        ),
    };
    ui.painter()
        .circle(rect.center(), MARK / 2.0 - 0.5, fill, ring);
    response
}

/// What the three marks mean, under the table that uses them.
fn legend(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        for (shown, label) in [
            (Mark::Yes, "works here"),
            (Mark::Partly, "partly, or unverified"),
            (Mark::No, "not yet"),
        ] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                mark(ui, shown);
                ui.label(egui::RichText::new(label).size(10.5).weak());
            });
        }
    });
}

/// The least room a start card is given, the room between two of them, and the room
/// inside one.
const CARD_LEAST: f32 = 210.0;
const CARD_GAP: f32 = 10.0;
const CARD_PAD: egui::Vec2 = egui::vec2(13.0, 11.0);
const CARD_STROKE: f32 = 1.0;

/// A card's title and its sub-line.
const CARD_TITLE: f32 = 12.0;
const CARD_SUB: f32 = 10.5;

/// The cards, as many across as the sheet has room for.
fn starts(ui: &mut egui::Ui, usb: bool) -> Option<Wanted> {
    let offered = offered(usb);
    let full = ui.available_width();
    let across = (((full + CARD_GAP) / (CARD_LEAST + CARD_GAP)) as usize).clamp(1, offered.len());
    let width = (full - CARD_GAP * (across - 1) as f32) / across as f32;
    // Every card stands as tall as the tallest one's content needs, so a card without a
    // sub-line matches its neighbours in any row. A change of need asks for the frame again.
    let told = ui.id().with("starts");
    let height: f32 = ui.data(|data| data.get_temp(told)).unwrap_or(0.0);
    let mut tallest: f32 = 0.0;
    let mut wanted = None;
    for row in offered.chunks(across) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for start in row {
                let (drawn, needs) = card(ui, start.card(), width, height);
                tallest = tallest.max(needs);
                if !drawn.clicked() {
                    continue;
                }
                match start {
                    Start::Connect => wanted = Some(Wanted::Act(Act::Connect)),
                    Start::Browsers => ui.ctx().open_url(egui::OpenUrl::new_tab(BROWSERS)),
                    Start::Open => wanted = Some(Wanted::Act(Act::OpenFiles)),
                    Start::Guide => ui.ctx().open_url(egui::OpenUrl::new_tab(GUIDE)),
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

/// One card's response, whichever row it landed in.
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

/// The release notes, as far as they have been read.
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

/// What a release body's heading says the section under it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change<'a> {
    Breaking,
    New,
    Fixed,
    Faster,
    Other,
    /// A heading `scripts/release.bash` does not write: kept as it reads.
    Unknown(&'a str),
}

impl<'a> Change<'a> {
    /// Read a heading, spelt either way: [`plain`] takes the variation selector out of a
    /// fetched body, and a body read from anywhere else still carries it.
    pub fn read(heading: &'a str) -> Change<'a> {
        match heading.trim() {
            "\u{26a0} Breaking changes" | "\u{26a0}\u{fe0f} Breaking changes" => Change::Breaking,
            "Features" => Change::New,
            "Bug fixes" => Change::Fixed,
            "Performance" => Change::Faster,
            "Other changes" => Change::Other,
            heading => Change::Unknown(heading),
        }
    }

    /// What the sheet calls this section.
    pub fn title(self) -> &'a str {
        match self {
            Change::Breaking => "Breaking",
            Change::New => "New",
            Change::Fixed => "Fixed",
            Change::Faster => "Faster",
            Change::Other => "Other",
            Change::Unknown(heading) => heading,
        }
    }

    /// The mark every row of this section wears.
    fn badge(self, visuals: &egui::Visuals) -> (Glyph, egui::Color32) {
        match self {
            Change::Breaking => (Glyph::TriangleAlert, warn(visuals)),
            Change::New => (Glyph::Sparkles, good(visuals)),
            Change::Fixed => (Glyph::Wrench, good(visuals)),
            Change::Faster => (Glyph::Gauge, good(visuals)),
            Change::Other => (Glyph::CircleDot, good(visuals)),
            Change::Unknown(_) => (Glyph::CircleDot, visuals.weak_text_color()),
        }
    }
}

/// One heading of the notes and the lines beneath it; `None` before the first heading.
struct Section<'a> {
    change: Option<Change<'a>>,
    lines: Vec<Line<'a>>,
}

/// The body grouped under its headings, in the order it was written.
fn sections(body: &str) -> Vec<Section<'_>> {
    let mut sections = vec![Section {
        change: None,
        lines: Vec::new(),
    }];
    for line in body.lines().map(classify) {
        match line {
            Line::Heading(heading) => sections.push(Section {
                change: Some(Change::read(heading)),
                lines: Vec::new(),
            }),
            // The compare link is the foot's, and a blank line is the markdown's own air.
            Line::Changelog(_) | Line::Blank => {}
            line => {
                if let Some(section) = sections.last_mut() {
                    section.lines.push(line);
                }
            }
        }
    }
    sections.retain(|section| !section.lines.is_empty());
    sections
}

/// The compare link a release body ends with.
fn changelog(body: &str) -> Option<&str> {
    body.lines().find_map(|line| match classify(line) {
        Line::Changelog(url) => Some(url),
        _ => None,
    })
}

/// How many changes a section holds, in the words the aside reads. `None` for a section
/// of prose alone, which has no changes to count.
fn tally(count: usize, breaking: bool) -> Option<String> {
    const WORDS: [&str; 9] = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];

    let many = WORDS
        .get(count.checked_sub(1)?)
        .map_or_else(|| count.to_string(), |word| (*word).to_owned());
    Some(match (count == 1, breaking) {
        (true, false) => format!("{many} change"),
        (false, false) => format!("{many} changes"),
        (true, true) => format!("{many} change that needs your attention"),
        (false, true) => format!("{many} changes that need your attention"),
    })
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
                    egui::RichText::new("Still alpha — keep your own backups.")
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
            let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
            for section in sections(body) {
                section_head(ui, section.change, &section.lines);
                for line in section.lines {
                    change_row(ui, section.change, line);
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

fn section_head(ui: &mut egui::Ui, change: Option<Change<'_>>, lines: &[Line<'_>]) {
    let breaking = change == Some(Change::Breaking);
    let ink = match breaking {
        true => warn(ui.visuals()),
        false => ui.visuals().strong_text_color(),
    };
    let count = lines
        .iter()
        .filter(|line| matches!(line, Line::Item { .. }))
        .count();
    ui.add_space(GAP * 4.0);
    ui.horizontal(|ui| {
        if let Some(change) = change {
            ui.label(
                egui::RichText::new(change.title())
                    .font(egui::FontId::new(12.0, bold()))
                    .color(ink),
            );
        }
        if let Some(tally) = tally(count, breaking) {
            ui.label(egui::RichText::new(tally).size(10.5).weak());
        }
    });
    ui.add_space(GAP * 1.5);
}

/// The column an item's pull request and commit sit in.
const REF: f32 = 88.0;

/// The column its glyph sits in.
const BADGE: f32 = 14.0;

fn change_row(ui: &mut egui::Ui, change: Option<Change<'_>>, line: Line<'_>) {
    let (scope, text, commit) = match line {
        Line::Item {
            scope,
            text,
            commit,
        } => (scope, text, commit),
        Line::Text(text) => (None, text, None),
        Line::Blank | Line::Heading(_) | Line::Changelog(_) => return,
    };
    let (glyph, tint) = match change {
        Some(change) => change.badge(ui.visuals()),
        None => (Glyph::CircleDot, ui.visuals().weak_text_color()),
    };
    let (text, pr) = split_pr(text);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        cell(ui, BADGE, down(), |ui| {
            ui.add_space(2.0);
            ui.add(sized(glyph, 12.0, tint));
        });
        let rest = (ui.available_width() - REF - 9.0).max(LEAST_PROSE);
        cell(ui, rest, down(), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(egui::RichText::new(text).size(11.5)).wrap());
            if let Some(scope) = scope {
                ui.add(egui::Label::new(egui::RichText::new(scope).size(10.5).weak()).wrap());
            }
        });
        cell(
            ui,
            REF,
            egui::Layout::right_to_left(egui::Align::TOP),
            |ui| reference(ui, pr, commit),
        );
    });
    ui.add_space(GAP);
}

/// `#NN · sha`, the sha standing for the commit it links to. Laid right to left, so the
/// column ends flush however much of it there is.
fn reference(ui: &mut egui::Ui, pr: Option<&str>, commit: Option<Commit<'_>>) {
    let mono = egui::FontId::monospace(10.0);
    ui.spacing_mut().item_spacing.x = 4.0;
    if let Some(commit) = commit {
        ui.add(
            egui::Hyperlink::from_label_and_url(
                egui::RichText::new(commit.sha).font(mono.clone()).weak(),
                commit.url,
            )
            .open_in_new_tab(true),
        );
    }
    if let Some(pr) = pr {
        let said = match commit.is_some() {
            true => format!("{pr} ·"),
            false => pr.to_owned(),
        };
        ui.label(egui::RichText::new(said).font(mono).weak());
    }
}

/// One line of the notes, in the terms the modal paints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Line<'a> {
    Blank,
    /// `### Features`
    Heading(&'a str),
    /// `- **scope:** description ([sha](url))`, either half of which may be absent.
    Item {
        scope: Option<&'a str>,
        text: &'a str,
        commit: Option<Commit<'a>>,
    },
    /// `**Full changelog**: <url>`
    Changelog(&'a str),
    Text(&'a str),
}

/// The commit an item is credited to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Commit<'a> {
    pub sha: &'a str,
    pub url: &'a str,
}

/// A release body with the emoji variation selector taken out.
///
/// ⚠️ No bundled font has a glyph for U+FE0F, and a missing one renders as an empty box
/// — so `### ⚠️ Breaking changes` would read as a warning sign beside a blank tile.
pub fn plain(body: &str) -> String {
    body.replace('\u{fe0f}', "")
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
    let (item, commit) = split_commit(item);
    let (scope, text) = split_scope(item);
    Line::Item {
        scope,
        text,
        commit,
    }
}

/// A URL is offered as a link only when it is `https`.
///
/// The body is fetched text, and every URL `scripts/release.bash` writes is an https one.
fn https(url: &str) -> Option<&str> {
    match url
        .strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty())
    {
        true => Some(url),
        false => None,
    }
}

/// The trailing `([sha](url))`, and the item with it taken off.
fn split_commit(item: &str) -> (&str, Option<Commit<'_>>) {
    const OPEN: &str = " ([";

    let Some(at) = item.rfind(OPEN) else {
        return (item, None);
    };
    let Some(inner) = item[at + OPEN.len()..].strip_suffix("))") else {
        return (item, None);
    };
    let Some((sha, url)) = inner.split_once("](") else {
        return (item, None);
    };
    if sha.is_empty() {
        return (item, None);
    }
    let Some(url) = https(url) else {
        return (item, None);
    };
    (&item[..at], Some(Commit { sha, url }))
}

/// The `(#NN)` a squashed pull request leaves at the end of a subject, and the text
/// without it. A `#NN` anywhere else is part of what the item says.
pub fn split_pr(text: &str) -> (&str, Option<&str>) {
    const OPEN: &str = " (#";

    let Some(rest) = text.strip_suffix(')') else {
        return (text, None);
    };
    let Some(at) = rest.rfind(OPEN) else {
        return (text, None);
    };
    let number = &rest[at + OPEN.len()..];
    match !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) {
        // From the `#`, so the column reads as the reference GitHub shows.
        true => (&rest[..at], Some(&rest[at + OPEN.len() - 1..])),
        false => (text, None),
    }
}

/// The `**scope:**` an item may open with, and the description after it.
fn split_scope(item: &str) -> (Option<&str>, &str) {
    let Some(rest) = item.strip_prefix("**") else {
        return (None, item);
    };
    let Some((scope, text)) = rest.split_once(":** ") else {
        return (None, item);
    };
    // A description of its own can hold `:** `; a scope never holds a star.
    match scope.contains('*') {
        true => (None, item),
        false => (Some(scope), text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The head of the `drawbar-v0.5.0` release body, as `gh release view` returns it.
    const RELEASED: &str = "\
### ⚠️ Breaking changes
- **nord-cli:** ship the v2 sample encoder unflagged and gate v3/v4 as unverified (#83) ([163abc4](https://github.com/jmoo/drawbar/commit/163abc4e00b0b088fca9a1212fd26faf4cb11770))

### Features
- name every write drawbar sends, and read settings from nord-cli (#71) ([3029a35](https://github.com/jmoo/drawbar/commit/3029a35ff1b0f812ccb21cb1a86fbaf0ae5e0256))

**Full changelog**: https://github.com/jmoo/drawbar/compare/drawbar-v0.4.0...drawbar-v0.5.0";

    fn headless() -> egui::Context {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        ctx.set_fonts(crate::app::fonts());
        ctx.all_styles_mut(crate::app::metrics);
        ctx
    }

    /// Every word painted in a frame, with the box it was painted in.
    fn painted(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    into.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
                _ => {}
            }
        }
        let mut said = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut said);
        }
        said
    }

    /// Draw `add` on a screen this size and report what it painted, and where.
    fn drawn_at(
        ctx: &egui::Context,
        size: egui::Vec2,
        add: impl FnMut(&egui::Context),
    ) -> Vec<(String, egui::Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        painted(&ctx.run(input, add))
    }

    /// Where a word landed, or nothing when it was never painted.
    fn box_of(said: &[(String, egui::Rect)], word: &str) -> Option<egui::Rect> {
        said.iter()
            .find(|(text, _)| text == word)
            .map(|(_, rect)| *rect)
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
            split_pr("ship the v2 sample encoder (#83)"),
            ("ship the v2 sample encoder", Some("#83"))
        );
        assert_eq!(
            split_pr("ship the v2 sample encoder"),
            ("ship the v2 sample encoder", None)
        );
        assert_eq!(
            split_pr("undo the truncation #12 introduced"),
            ("undo the truncation #12 introduced", None)
        );
        assert_eq!(
            split_pr("a trailer with no number (#)"),
            ("a trailer with no number (#)", None)
        );
    }

    #[test]
    fn a_breaking_heading_is_named_with_or_without_the_warning_sign() {
        assert_eq!(
            Change::read("\u{26a0} Breaking changes"),
            Change::Breaking,
            "the heading a fetched body carries once `plain` has run"
        );
        assert_eq!(
            Change::read("\u{26a0}\u{fe0f} Breaking changes"),
            Change::Breaking
        );
        assert_eq!(Change::Breaking.title(), "Breaking");
    }

    #[test]
    fn every_heading_the_release_script_writes_has_a_name_and_an_unknown_one_keeps_its_own() {
        for (heading, title) in [
            ("Features", "New"),
            ("Bug fixes", "Fixed"),
            ("Performance", "Faster"),
            ("Other changes", "Other"),
        ] {
            assert_eq!(Change::read(heading).title(), title, "### {heading}");
        }
        let odd = "Acknowledgements";
        assert_eq!(Change::read(odd), Change::Unknown(odd));
        assert_eq!(Change::read(odd).title(), odd);
    }

    #[test]
    fn a_section_says_how_many_changes_it_holds_and_breaking_says_what_that_asks_of_you() {
        let said = |count, breaking| tally(count, breaking).unwrap_or_default();
        assert_eq!(said(1, false), "one change");
        assert_eq!(said(3, false), "three changes");
        assert_eq!(said(12, false), "12 changes");
        assert_eq!(said(3, true), "three changes that need your attention");
        assert_eq!(said(1, true), "one change that needs your attention");
    }

    #[test]
    fn a_section_of_prose_alone_has_no_tally() {
        assert_eq!(tally(0, false), None);
        assert_eq!(tally(0, true), None);
    }

    #[test]
    fn the_notes_group_under_their_headings_and_the_compare_link_is_left_for_the_foot() {
        let body = plain(RELEASED);
        let read = sections(&body);
        let named: Vec<&str> = read
            .iter()
            .map(|section| section.change.map_or("", Change::title))
            .collect();
        assert_eq!(named, vec!["Breaking", "New"]);
        assert!(read.iter().all(|section| section.lines.len() == 1));
        assert_eq!(
            changelog(&body),
            Some("https://github.com/jmoo/drawbar/compare/drawbar-v0.4.0...drawbar-v0.5.0")
        );
    }

    /// Neither the note nor a hover text is blank: a mark nobody can read is a claim
    /// nobody can check.
    #[test]
    fn every_supported_row_says_what_each_mark_claims() {
        assert!(!SUPPORT.is_empty());
        for row in SUPPORT {
            assert!(!row.note.is_empty(), "{}: no note", row.instrument);
            assert!(!row.kinds.is_empty(), "{}: no kinds", row.instrument);
            for (claim, column) in row.marks.iter().zip(COLUMNS) {
                assert!(
                    !claim.hint.is_empty(),
                    "{} has no hover text under {column}",
                    row.instrument
                );
            }
        }
    }

    /// The shell refuses a smaller screen than this, so both sheets have to lay out in it
    /// with the one button that dismisses them still on it.
    #[test]
    fn the_welcome_sheet_keeps_its_button_on_the_smallest_screen_the_shell_allows() {
        let ctx = headless();
        let size = crate::shell::LEAST;
        // Twice: the first frame is what the second lays itself out against.
        let _ = drawn_at(&ctx, size, |ctx| {
            welcome(ctx, true);
        });
        let said = drawn_at(&ctx, size, |ctx| {
            welcome(ctx, true);
        });

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let button = box_of(&said, "I understand — let me in")
            .unwrap_or_else(|| panic!("the button was never painted: {said:?}"));
        assert!(
            screen.contains_rect(button.expand(6.0)),
            "the button is off a {size:?} screen: {button:?}"
        );
        assert!(box_of(&said, "Nord Electro 5").is_some(), "{said:?}");
    }

    #[test]
    fn the_news_sheet_keeps_its_button_on_the_smallest_screen_the_shell_allows() {
        let ctx = headless();
        let size = crate::shell::LEAST;
        let notes = Notes::Read {
            body: plain(RELEASED),
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
        assert!(box_of(&said, "Breaking").is_some(), "{said:?}");
    }

    #[test]
    fn a_browser_without_usb_is_offered_the_browsers_that_can_connect() {
        let ctx = headless();
        let size = egui::vec2(1200.0, 900.0);
        // Twice: the first frame is what the second lays itself out against.
        let _ = drawn_at(&ctx, size, |ctx| {
            welcome(ctx, false);
        });
        let said = drawn_at(&ctx, size, |ctx| {
            welcome(ctx, false);
        });
        assert!(
            box_of(&said, "Connect an instrument…").is_none(),
            "{said:?}"
        );
        let card = ctx
            .read_response(card_id(NO_USB_BRIEF))
            .unwrap_or_else(|| panic!("no {NO_USB_BRIEF} card: {said:?}"))
            .rect;

        let at = card.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events: vec![egui::Event::PointerMoved(at), press(true), press(false)],
            ..Default::default()
        };
        let mut wanted = None;
        let output = ctx.run(input, |ctx| wanted = welcome(ctx, false));

        assert!(wanted.is_none(), "the card asked the app for something");
        let opened: Vec<_> = output
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, [BROWSERS]);
    }

    /// How tall the start cards stand on a screen `width` wide, once they have settled.
    fn cards_tall(ctx: &egui::Context, width: f32) -> f32 {
        let mut tall = 0.0;
        for _ in 0..2 {
            let _ = drawn_at(ctx, egui::vec2(width, 900.0), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let top = ui.cursor().top();
                    starts(ui, true);
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
                    welcome(ctx, true);
                });
            }
            let rects: Vec<(&str, egui::Rect)> = offered(true)
                .iter()
                .map(|start| {
                    let label = start.card().label;
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
    fn a_heading_keeps_its_title_alone() {
        assert_eq!(classify("### Bug fixes"), Line::Heading("Bug fixes"));
    }

    #[test]
    fn an_item_splits_its_scope_its_text_and_its_commit() {
        let line = "- **drawbar:** say what changed ([abc1234](https://example.com/c/abc1234))";
        assert_eq!(
            classify(line),
            Line::Item {
                scope: Some("drawbar"),
                text: "say what changed",
                commit: Some(Commit {
                    sha: "abc1234",
                    url: "https://example.com/c/abc1234",
                }),
            }
        );
    }

    #[test]
    fn an_unscoped_item_keeps_its_whole_description() {
        assert_eq!(
            classify("- say what changed"),
            Line::Item {
                scope: None,
                text: "say what changed",
                commit: None,
            }
        );
    }

    #[test]
    fn a_commit_link_that_is_not_https_stays_in_the_text() {
        let line = "- fixed it ([abc1234](javascript:alert(1)))";
        assert_eq!(
            classify(line),
            Line::Item {
                scope: None,
                text: "fixed it ([abc1234](javascript:alert(1)))",
                commit: None,
            }
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

    #[test]
    fn a_heading_keeps_its_warning_sign_without_the_variation_selector() {
        assert_eq!(
            plain("### \u{26a0}\u{fe0f} Breaking changes"),
            "### \u{26a0} Breaking changes"
        );
    }

    #[test]
    fn every_line_of_a_published_release_body_is_recognised() {
        let read: Vec<Line<'_>> = RELEASED.lines().map(classify).collect();
        assert_eq!(
            read,
            vec![
                Line::Heading("⚠️ Breaking changes"),
                Line::Item {
                    scope: Some("nord-cli"),
                    text: "ship the v2 sample encoder unflagged and gate v3/v4 as unverified (#83)",
                    commit: Some(Commit {
                        sha: "163abc4",
                        url: "https://github.com/jmoo/drawbar/commit/163abc4e00b0b088fca9a1212fd26faf4cb11770",
                    }),
                },
                Line::Blank,
                Line::Heading("Features"),
                Line::Item {
                    scope: None,
                    text: "name every write drawbar sends, and read settings from nord-cli (#71)",
                    commit: Some(Commit {
                        sha: "3029a35",
                        url: "https://github.com/jmoo/drawbar/commit/3029a35ff1b0f812ccb21cb1a86fbaf0ae5e0256",
                    }),
                },
                Line::Blank,
                Line::Changelog(
                    "https://github.com/jmoo/drawbar/compare/drawbar-v0.4.0...drawbar-v0.5.0"
                ),
            ]
        );
    }
}
