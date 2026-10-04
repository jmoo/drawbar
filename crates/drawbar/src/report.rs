//! The report sheet: a problem or feedback, written by the operator and sent to the
//! collector only when they press Send. It is sent whatever the sharing switch says,
//! and it shows everything it attaches before it goes.

use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::device::Device;
use crate::log::Log;
use crate::sheet;
use crate::store::Store;
use crate::telemetry::{self, Undelivered};
use crate::workspace::Workspace;

/// The widest the sheet grows.
const WIDE: f32 = 520.0;

/// The height the sheet needs around its scrolling middle, so a long message scrolls and
/// Send stays on screen.
const AROUND: f32 = 140.0;

/// The shortest the scrolling middle gets, however short the window.
const FEWEST: f32 = 120.0;

/// The longest message, in characters. The collector refuses longer ones.
const LONGEST: usize = 5_000;

/// The longest contact address, in characters.
const CONTACT: usize = 200;

/// The most of the activity log a report carries, newest kept, in characters. Held under
/// the collector's limit for the field, `log` in `js/telemetry/src/check.js`.
const LOG: usize = 60_000;

/// How long to wait before trying again once a send found no one, in seconds.
const RETRY: f64 = 15.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Problem,
    Feedback,
}

impl Kind {
    fn wire(self) -> &'static str {
        match self {
            Kind::Problem => "problem",
            Kind::Feedback => "feedback",
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Kind::Problem => "What happened, and what did you expect?",
            Kind::Feedback => "What would make drawbar better for you?",
        }
    }
}

/// Where the send is.
enum State {
    Drafting,
    Sending(Arc<Mutex<Option<Result<(), Undelivered>>>>),
    /// The collector could not be reached; the draft is kept and sent again once the
    /// browser is online and [`RETRY`] has passed.
    Waiting {
        since: f64,
    },
    Refused(u16),
    Sent,
}

/// A report as Send took it: the body, and the parts of it read from outside the sheet,
/// which the preview shows from here until the send is settled.
#[derive(Clone, PartialEq, Debug)]
struct Frozen {
    instrument: telemetry::Instrument,
    tail: String,
    body: String,
}

/// What the reader pressed.
enum Pressed {
    Send,
    Close,
}

/// The sheet while it is open.
pub struct Report {
    /// Made when the sheet opens, so a send that is retried arrives once.
    id: String,
    kind: Kind,
    text: String,
    contact: String,
    with_faults: bool,
    with_build: bool,
    with_log: bool,
    /// What went wrong recently and what this build is, read when the sheet opens.
    faults: Vec<String>,
    build: String,
    /// What Send took, which every retry sends again unchanged, so the report the
    /// collector keeps under [`Report::id`] is the one the reader sent. Thawed by a
    /// refusal, since nothing was kept.
    frozen: Option<Frozen>,
    state: State,
}

impl Report {
    pub fn problem(device: &Device, workspace: &Workspace, store: Option<&Store>) -> Report {
        Report::new(Kind::Problem, device, workspace, store)
    }

    pub fn feedback(device: &Device, workspace: &Workspace, store: Option<&Store>) -> Report {
        Report::new(Kind::Feedback, device, workspace, store)
    }

    fn new(kind: Kind, device: &Device, workspace: &Workspace, store: Option<&Store>) -> Report {
        Report {
            id: telemetry::report_id(),
            kind,
            text: String::new(),
            contact: String::new(),
            with_faults: true,
            with_build: true,
            with_log: false,
            faults: telemetry::recent(),
            build: crate::about::build_lines(device, workspace, store),
            frozen: None,
            state: State::Drafting,
        }
    }

    fn send(&mut self, ctx: &egui::Context, log: &Log) {
        let slot = Arc::new(Mutex::new(None));
        let tail = newest(&log.tail(crate::about::ENTRIES), LOG);
        let body = self.freeze(telemetry::instrument(), tail);
        let (done, ctx) = (slot.clone(), ctx.clone());
        spawn(async move {
            let answer = telemetry::submit(body).await;
            if let Ok(mut done) = done.lock() {
                *done = Some(answer);
            }
            ctx.request_repaint();
        });
        self.state = State::Sending(slot);
    }

    /// Move the send along: collect an answer, or try again once the wait is over.
    fn advance(&mut self, ctx: &egui::Context, log: &Log) {
        let now = ctx.input(|input| input.time);
        self.collect(now);
        let State::Waiting { since } = self.state else {
            return;
        };
        if now - since >= RETRY && telemetry::online() {
            self.send(ctx, log);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

impl Report {
    /// The report as the collector reads it. The instrument goes with the build lines:
    /// unchecking it leaves out both.
    fn body(&self, instrument: &telemetry::Instrument, tail: &str) -> String {
        let attached = |on: bool, text: &str| match on {
            true => text.to_string(),
            false => String::new(),
        };
        telemetry::report_json(&[
            ("id", &self.id),
            ("kind", self.kind.wire()),
            ("text", self.text.trim()),
            ("contact", self.contact.trim()),
            ("version", sheet::VERSION),
            ("model", &attached(self.with_build, &instrument.model)),
            ("firmware", &attached(self.with_build, &instrument.firmware)),
            (
                "faults",
                &attached(self.with_faults, &self.faults.join("\n")),
            ),
            ("build", &attached(self.with_build, &self.build)),
            ("log", &attached(self.with_log, tail)),
        ])
    }

    /// The body to send: the one already frozen, or this one, frozen now.
    fn freeze(&mut self, instrument: telemetry::Instrument, tail: String) -> String {
        if let Some(frozen) = &self.frozen {
            return frozen.body.clone();
        }
        let body = self.body(&instrument, &tail);
        self.frozen = Some(Frozen {
            instrument,
            tail,
            body: body.clone(),
        });
        body
    }

    /// Take the collector's answer, if it has come, at `now`. A server error is waited
    /// out like no answer at all.
    fn collect(&mut self, now: f64) {
        let State::Sending(slot) = &self.state else {
            return;
        };
        let Some(answer) = slot.lock().ok().and_then(|mut answer| answer.take()) else {
            return;
        };
        self.state = match answer {
            Ok(()) => State::Sent,
            Err(Undelivered::Unreachable) => State::Waiting { since: now },
            Err(Undelivered::Refused(status)) if status >= 500 => State::Waiting { since: now },
            Err(Undelivered::Refused(status)) => {
                self.frozen = None;
                State::Refused(status)
            }
        };
    }

    fn busy(&self) -> bool {
        matches!(self.state, State::Sending(_) | State::Waiting { .. })
    }

    fn show(&mut self, ui: &mut egui::Ui, log: &Log) -> Option<Pressed> {
        ui.set_width(sheet::width(ui.ctx(), WIDE));
        ui.add_space(sheet::PAD);
        egui::ScrollArea::vertical()
            .id_salt("report")
            .max_height(sheet::middle(ui.ctx(), AROUND, FEWEST))
            .show(ui, |ui| {
                // The scrollbar floats over the content; keep the form out from under it.
                let scroll = ui.spacing().scroll;
                ui.set_width(ui.available_width() - scroll.bar_width - scroll.bar_outer_margin);
                sheet::section(ui, |ui| match self.state {
                    State::Sent => self.thanks(ui),
                    _ => self.form(ui, log),
                });
            });
        let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
        let mut closed = false;
        let mut send = false;
        sheet::foot(
            ui,
            |ui| {
                ui.label(egui::RichText::new(self.progress()).small().weak());
            },
            |ui| match self.state {
                State::Sent => closed = sheet::primary(ui, None, "Close").clicked(),
                _ => {
                    let ready = !self.text.trim().is_empty() && !self.busy();
                    send = ui
                        .add_enabled_ui(ready, |ui| sheet::primary(ui, None, "Send"))
                        .inner
                        .clicked();
                    closed = sheet::secondary(ui, None, "Cancel").clicked();
                }
            },
        );
        if closed || escaped {
            return Some(Pressed::Close);
        }
        send.then_some(Pressed::Send)
    }

    fn form(&mut self, ui: &mut egui::Ui, log: &Log) {
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.kind, Kind::Problem, "Report a problem");
                ui.radio_value(&mut self.kind, Kind::Feedback, "Send feedback");
            });
        });
        ui.add_space(sheet::GAP * 2.0);
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .hint_text(self.kind.prompt())
                    .char_limit(LONGEST)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(sheet::GAP);
            ui.add(
                egui::TextEdit::singleline(&mut self.contact)
                    .hint_text("Email address, only if you would like a reply")
                    .char_limit(CONTACT)
                    .desired_width(f32::INFINITY),
            );
            sheet::heading(ui, "Attach", Some("shown below exactly as sent"));
            let faults = match self.faults.len() {
                0 => "Recent errors (none)".to_string(),
                n => format!("Recent errors ({n})"),
            };
            ui.checkbox(&mut self.with_faults, faults);
            ui.checkbox(&mut self.with_build, "This build and the instrument");
            ui.checkbox(
                &mut self.with_log,
                format!(
                    "The activity log's last {} entries, which name your files",
                    crate::about::ENTRIES
                ),
            );
        });
        egui::CollapsingHeader::new("What will be sent")
            .id_salt("report-preview")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(self.preview(log)).monospace().small(),
                            )
                            .wrap(),
                        );
                    });
            });
        ui.add_space(sheet::GAP * 2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(
                egui::RichText::new(
                    "Reports go only to drawbar's developer, are never published, and are \
                     deleted after three months. ",
                )
                .small()
                .weak(),
            );
            sheet::link(ui, "Privacy", telemetry::PRIVACY);
        });
    }

    fn thanks(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Thank you. Your report was sent.").strong());
        ui.add_space(sheet::GAP * 2.0);
        ui.horizontal(|ui| {
            ui.label("Its number:");
            ui.label(egui::RichText::new(&self.id).monospace());
        });
        ui.label("To have it deleted, write to contact@drawbar.app with that number.");
    }

    /// Everything that goes, as the operator reads it: what Send froze while a send is
    /// out, else what Send would take now.
    fn preview(&self, log: &Log) -> String {
        let (instrument, tail) = match &self.frozen {
            Some(frozen) => (frozen.instrument.clone(), frozen.tail.clone()),
            None => (
                telemetry::instrument(),
                newest(&log.tail(crate::about::ENTRIES), LOG),
            ),
        };
        let mut out = format!(
            "report: {} ({})\nversion: {}\n",
            self.id,
            self.kind.wire(),
            sheet::VERSION,
        );
        if !self.contact.trim().is_empty() {
            out.push_str(&format!("contact: {}\n", self.contact.trim()));
        }
        if self.with_faults {
            out.push_str(&format!(
                "\nrecent errors:\n{}\n",
                none(&self.faults.join("\n"))
            ));
        }
        if self.with_build {
            out.push_str(&format!(
                "\nmodel: {}\nfirmware: {}\n{}",
                none(&instrument.model),
                none(&instrument.firmware),
                self.build
            ));
        }
        if self.with_log {
            out.push_str(&format!("\n{tail}"));
        }
        out
    }

    fn progress(&self) -> String {
        match self.state {
            State::Drafting | State::Sent => String::new(),
            State::Sending(_) => "Sending…".to_string(),
            State::Waiting { .. } => {
                "drawbar could not reach its server. It will try again while this stays \
                 open."
                    .to_string()
            }
            State::Refused(403) => "Only drawbar.app can send reports.".to_string(),
            State::Refused(429) => {
                "drawbar has had too many reports today. Please try again tomorrow.".to_string()
            }
            State::Refused(status) => format!("The report was not accepted (error {status})."),
        }
    }
}

/// The last `most` characters of `text`, starting on a line of its own where one is
/// in reach.
fn newest(text: &str, most: usize) -> String {
    let skip = text.chars().count().saturating_sub(most);
    if skip == 0 {
        return text.to_string();
    }
    let kept: String = text.chars().skip(skip).collect();
    match kept.split_once('\n') {
        Some((_, whole)) => whole.to_string(),
        None => kept,
    }
}

fn none(text: &str) -> &str {
    match text.is_empty() {
        true => "none",
        false => text,
    }
}

/// Run a send that outlives the frame that started it.
///
/// ⚠️ wasm has one thread and cannot block: the future goes to the microtask queue there,
/// and to a thread of its own in a window, where it blocks until the collector answers.
#[cfg(target_arch = "wasm32")]
fn spawn(future: impl std::future::Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(future);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(future: impl std::future::Future<Output = ()> + Send + 'static) {
    std::thread::spawn(move || nord_usb::block_on(future));
}

/// Draw the sheet if it is open, send it when asked, and close it when the reader is
/// done.
pub fn dialog(ctx: &egui::Context, open: &mut Option<Report>, log: &Log) {
    let Some(report) = open.as_mut() else {
        return;
    };
    report.advance(ctx, log);
    let pressed = egui::Modal::new(egui::Id::new("report"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| report.show(ui, log))
        .inner;
    match pressed {
        Some(Pressed::Send) => report.send(ctx, log),
        Some(Pressed::Close) => *open = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// The state a send reaches once the collector's `answer` arrives at 1 s.
    fn after(answer: Result<(), Undelivered>) -> State {
        let mut report = report(State::Sending(Arc::new(Mutex::new(Some(answer)))));
        report.collect(1.0);
        report.state
    }

    #[test]
    fn a_server_error_is_waited_out_and_a_refusal_is_final() {
        assert!(matches!(after(Ok(())), State::Sent));
        assert!(matches!(
            after(Err(Undelivered::Unreachable)),
            State::Waiting { since } if since == 1.0
        ));
        assert!(matches!(
            after(Err(Undelivered::Refused(503))),
            State::Waiting { .. }
        ));
        assert!(matches!(
            after(Err(Undelivered::Refused(413))),
            State::Refused(413)
        ));
    }

    fn electro() -> telemetry::Instrument {
        telemetry::Instrument {
            model: "Nord Electro 5D".to_string(),
            firmware: "2.04".to_string(),
        }
    }

    #[test]
    fn unchecking_the_build_leaves_out_the_instrument_too() {
        let mut report = report(State::Drafting);
        report.with_build = false;
        let body = report.body(&electro(), "");
        assert!(!body.contains("Nord Electro 5D"), "{body}");
        assert!(!body.contains("2.04"), "{body}");
        assert!(body.contains("\"model\":\"\""), "{body}");
    }

    #[test]
    fn a_retry_sends_what_send_froze_whatever_changed_since() {
        let mut report = report(State::Drafting);
        let first = report.freeze(electro(), "connected".to_string());
        report.kind = Kind::Feedback;
        report.with_build = false;
        let later = report.freeze(
            telemetry::Instrument::default(),
            "connected\nlost".to_string(),
        );
        assert_eq!(later, first);
    }

    #[test]
    fn while_a_send_is_out_the_preview_shows_what_send_took() {
        let mut report = report(State::Waiting { since: 0.0 });
        report.freeze(electro(), "connected: Nord Electro 5D".to_string());
        let mut log = Log::default();
        log.info("a line logged after Send");
        let preview = report.preview(&log);
        assert!(preview.contains("model: Nord Electro 5D"), "{preview}");
        assert!(preview.contains("connected: Nord Electro 5D"), "{preview}");
        assert!(!preview.contains("after Send"), "{preview}");
    }

    #[test]
    fn a_refusal_thaws_the_body_since_nothing_was_kept() {
        let mut report = report(State::Sending(Arc::new(Mutex::new(Some(Err(
            Undelivered::Refused(413),
        ))))));
        report.frozen = Some(Frozen {
            instrument: electro(),
            tail: String::new(),
            body: "the old body".to_string(),
        });
        report.collect(1.0);
        assert_eq!(report.frozen, None);
    }

    #[test]
    fn a_send_still_out_stays_sending() {
        let mut report = report(State::Sending(Arc::default()));
        report.collect(1.0);
        assert!(matches!(report.state, State::Sending(_)));
    }

    fn report(state: State) -> Report {
        Report {
            id: "23456789ab".to_string(),
            kind: Kind::Problem,
            text: "The sample would not send.\n".repeat(40),
            contact: String::new(),
            with_faults: true,
            with_build: true,
            with_log: true,
            faults: vec!["put: transport".to_string()],
            build: "Version: 0.10.0\n".to_string(),
            frozen: None,
            state,
        }
    }

    /// The shell refuses a smaller screen than this, so a long message scrolls and the
    /// sheet's button stays on screen in every state.
    #[test]
    fn the_sheet_keeps_its_button_on_the_smallest_screen_the_shell_allows() {
        let size = crate::shell::LEAST;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        for (name, state, button) in [
            ("drafting", State::Drafting, "Send"),
            ("sending", State::Sending(Arc::default()), "Send"),
            ("waiting", State::Waiting { since: 0.0 }, "Send"),
            ("refused", State::Refused(403), "Send"),
            ("sent", State::Sent, "Close"),
        ] {
            let mut report = report(state);
            let ctx = testing::context();
            egui_extras::install_image_loaders(&ctx);
            let log = Log::default();
            let mut said = Vec::new();
            // Twice, because a scroll area sizes itself from the previous frame's content.
            for _ in 0..2 {
                let output = testing::run(&ctx, testing::screen(size, Vec::new()), |ctx| {
                    egui::Modal::new(egui::Id::new("report"))
                        .frame(sheet::frame(&ctx.style().visuals))
                        .show(ctx, |ui| report.show(ui, &log));
                });
                said = testing::painted(&output);
            }
            let drawn = testing::where_(&said, button);
            assert!(
                screen.contains_rect(drawn.expand(6.0)),
                "{button} is off a {size:?} screen while {name}: {drawn:?}"
            );
        }
    }
}
