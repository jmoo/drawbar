//! The report sheet: a problem or feedback, written by the operator and sent to the
//! collector only when they press Send. It is sent whatever the sharing switch says,
//! and it shows everything it attaches before it goes.

use std::cell::RefCell;
use std::rc::Rc;

use eframe::egui;

use crate::device::DeviceState;
use crate::log::Log;
use crate::sheet;
use crate::store::Store;
use crate::telemetry::{self, Undelivered};
use crate::workspace::Workspace;

/// The widest the sheet grows.
const WIDE: f32 = 520.0;

/// The longest message, in characters. The collector refuses longer ones.
const LONGEST: usize = 5_000;

/// The longest contact address, in characters.
const CONTACT: usize = 200;

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
    Sending(Rc<RefCell<Option<Result<(), Undelivered>>>>),
    /// The collector could not be reached; the draft is kept and sent again once the
    /// browser is online and [`RETRY`] has passed.
    Waiting {
        since: f64,
    },
    Refused(u16),
    Sent,
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
    state: State,
}

impl Report {
    pub fn problem(device: &DeviceState, workspace: &Workspace, store: Option<&Store>) -> Report {
        Report::new(Kind::Problem, device, workspace, store)
    }

    pub fn feedback(device: &DeviceState, workspace: &Workspace, store: Option<&Store>) -> Report {
        Report::new(Kind::Feedback, device, workspace, store)
    }

    fn new(
        kind: Kind,
        device: &DeviceState,
        workspace: &Workspace,
        store: Option<&Store>,
    ) -> Report {
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
            state: State::Drafting,
        }
    }

    /// The report as the collector reads it.
    fn body(&self, log: &Log) -> String {
        let instrument = telemetry::instrument();
        let faults = self.faults.join("\n");
        let build = self.build.clone();
        let tail = log.tail(crate::about::ENTRIES);
        let kept = |on: bool, text: &str| match on {
            true => text.to_string(),
            false => String::new(),
        };
        telemetry::report_json(&[
            ("id", &self.id),
            ("kind", self.kind.wire()),
            ("text", self.text.trim()),
            ("contact", self.contact.trim()),
            ("version", sheet::VERSION),
            ("model", &instrument.model),
            ("firmware", &instrument.firmware),
            ("faults", &kept(self.with_faults, &faults)),
            ("build", &kept(self.with_build, &build)),
            ("log", &kept(self.with_log, &tail)),
        ])
    }

    fn send(&mut self, ctx: &egui::Context, log: &Log) {
        let slot = Rc::new(RefCell::new(None));
        let body = self.body(log);
        let (done, ctx) = (slot.clone(), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            *done.borrow_mut() = Some(telemetry::submit(body).await);
            ctx.request_repaint();
        });
        self.state = State::Sending(slot);
    }

    /// Move the send along: collect an answer, or try again once the wait is over.
    fn advance(&mut self, ctx: &egui::Context, log: &Log) {
        let now = ctx.input(|input| input.time);
        match &self.state {
            State::Sending(slot) => {
                let answer = slot.borrow_mut().take();
                self.state = match answer {
                    None => return,
                    Some(Ok(())) => State::Sent,
                    Some(Err(Undelivered::Unreachable)) => State::Waiting { since: now },
                    Some(Err(Undelivered::Refused(status))) => State::Refused(status),
                };
            }
            State::Waiting { since } => {
                if now - since >= RETRY && telemetry::online() {
                    self.send(ctx, log);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs(1));
                }
            }
            State::Drafting | State::Refused(_) | State::Sent => {}
        }
    }

    fn busy(&self) -> bool {
        matches!(self.state, State::Sending(_) | State::Waiting { .. })
    }

    /// Returns whether the reader is done with it.
    fn show(&mut self, ui: &mut egui::Ui, log: &Log) -> bool {
        self.advance(ui.ctx(), log);
        ui.set_width(sheet::width(ui.ctx(), WIDE));
        ui.add_space(sheet::PAD);
        sheet::section(ui, |ui| match self.state {
            State::Sent => self.thanks(ui),
            _ => self.form(ui, log),
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
        if send {
            self.send(ui.ctx(), log);
        }
        closed || escaped
    }

    fn form(&mut self, ui: &mut egui::Ui, log: &Log) {
        ui.horizontal(|ui| {
            // Radios, not selectable labels: their fill is the instrument's red, which
            // reads as a warning.
            ui.radio_value(&mut self.kind, Kind::Problem, "Report a problem");
            ui.radio_value(&mut self.kind, Kind::Feedback, "Send feedback");
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
        ui.horizontal_wrapped(|ui| {
            ui.label("Its number is");
            ui.label(egui::RichText::new(&self.id).monospace());
            ui.label(". To have it deleted, write to contact@drawbar.app with that number.");
        });
    }

    /// Everything that goes, as the operator reads it.
    fn preview(&self, log: &Log) -> String {
        let instrument = telemetry::instrument();
        let mut out = format!(
            "version: {}\nmodel: {}\nfirmware: {}\n",
            sheet::VERSION,
            none(&instrument.model),
            none(&instrument.firmware)
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
            out.push_str(&format!("\n{}", self.build));
        }
        if self.with_log {
            out.push_str(&format!("\n{}", log.tail(crate::about::ENTRIES)));
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
            State::Refused(status) => {
                format!("The report was not accepted (error {status}). Shorten it and try again.")
            }
        }
    }
}

fn none(text: &str) -> &str {
    match text.is_empty() {
        true => "none",
        false => text,
    }
}

/// Draw the sheet if it is open, and close it when the reader is done.
pub fn dialog(ctx: &egui::Context, open: &mut Option<Report>, log: &Log) {
    let Some(report) = open.as_mut() else {
        return;
    };
    if egui::Modal::new(egui::Id::new("report"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| report.show(ui, log))
        .inner
    {
        *open = None;
    }
}
