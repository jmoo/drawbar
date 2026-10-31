//! Kept apart: the changes the library shows apart from it rather than merging them,
//! one card per run, each change taken or the run dismissed.

use eframe::egui;

use super::{Act, Ask, Browser, Item, Verb};
use crate::folders::Kept;
use crate::sheet;
use crate::store::KeptKind;

/// The widest Kept apart is.
const WIDTH: f32 = 560.0;

/// What Kept apart says above its cards.
const ABOUT: &str = "These changes were made where the library's history split, as when \
                     two computers went on from one copy of drawbar's own data, or a sync \
                     left part of a history behind. drawbar shows them here rather than \
                     mixing them in. Take the changes you want; dismiss a branch you don't \
                     want.";

/// What a card offers, by its kind: whether each change can be taken, and whether the
/// whole run can be taken and dismissed.
pub(crate) fn offers(kind: KeptKind) -> (bool, bool) {
    match kind {
        KeptKind::Run => (true, true),
        KeptKind::Unplaced => (true, true),
        KeptKind::Gap => (true, false),
    }
}

impl Browser {
    /// Draw Kept apart while it shows and no question does.
    pub(super) fn kept_sheet(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        if self.folders.kept.is_empty() {
            self.show_kept = false;
        }
        if !self.show_kept {
            return;
        }
        let mut done = false;
        let mut dismiss = None;
        egui::Modal::new(egui::Id::new("browser_kept"))
            .frame(sheet::frame(&ctx.style().visuals))
            .show(ctx, |ui| {
                ui.set_width(sheet::width(ui.ctx(), WIDTH));
                ui.add_space(sheet::GAP * 4.0);
                sheet::section(ui, |ui| {
                    sheet::heading(ui, "Kept apart", None);
                    ui.add_space(sheet::GAP * 2.0);
                    ui.add(egui::Label::new(ABOUT).wrap());
                    ui.add_space(sheet::GAP * 3.0);
                    egui::ScrollArea::vertical()
                        .id_salt("browser_kept_cards")
                        .max_height(sheet::middle(ui.ctx(), 240.0, 120.0))
                        .show(ui, |ui| {
                            for kept in &self.folders.kept {
                                card(ui, kept, acts, &mut dismiss, &mut done);
                                ui.add_space(sheet::GAP * 3.0);
                            }
                        });
                });
                sheet::foot(
                    ui,
                    |_| {},
                    |ui| {
                        if sheet::primary(ui, None, "Done").clicked() {
                            done = true;
                        }
                    },
                );
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    done = true;
                }
            });
        if let Some(run) = dismiss {
            self.ask_dismiss_kept(run);
        }
        if done {
            self.show_kept = false;
        }
    }

    /// Ask before dismissing a run kept apart, which cannot be undone.
    pub(crate) fn ask_dismiss_kept(&mut self, run: toshokan::log::Run) {
        self.raise(Ask::new(
            "Dismiss these changes?".to_string(),
            Some(
                "drawbar will not show or merge them again, on this computer or any \
                 other. This cannot be undone."
                    .to_string(),
            ),
            Verb::Discard,
            vec![Act::DismissKept(Box::new(run))],
        ));
    }
}

/// One run's card: who and when, each change with what can be done with it, and what
/// can be done with the whole run.
fn card(
    ui: &mut egui::Ui,
    kept: &Kept,
    acts: &mut Vec<Act>,
    dismiss: &mut Option<toshokan::log::Run>,
    done: &mut bool,
) {
    let (each, whole) = offers(kept.kind);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(egui::RichText::new(&kept.title).strong()).wrap());
        if kept.kind == KeptKind::Gap {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("Waiting for the rest of these changes to sync.").weak(),
                )
                .wrap(),
            );
        }
        if kept.rows.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new("Nothing left to review.").weak()).wrap());
        }
        for row in &kept.rows {
            ui.horizontal(|ui| {
                let mut said = row.said.clone();
                if row.forgotten {
                    said.push_str(" (only taking it settles this)");
                }
                ui.add(egui::Label::new(said).wrap());
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match (each && row.takes, row.id) {
                        (true, _) => {
                            if sheet::secondary(ui, None, "Take").clicked() {
                                acts.push(Act::TakeKept {
                                    run: Box::new(kept.run),
                                    changes: vec![row.key.clone()],
                                });
                            }
                        }
                        (false, Some(id)) if !row.takes => {
                            if sheet::secondary(ui, None, "Show file").clicked() {
                                acts.push(Act::Reveal(Item::Local(id)));
                                *done = true;
                            }
                        }
                        (false, _) => {}
                    },
                );
            });
        }
        let takes = each && kept.rows.iter().any(|row| row.takes);
        if takes || whole {
            ui.horizontal(|ui| {
                if takes && sheet::primary(ui, None, "Take all").clicked() {
                    acts.push(Act::TakeKept {
                        run: Box::new(kept.run),
                        changes: Vec::new(),
                    });
                }
                if whole && sheet::secondary(ui, None, "Dismiss…").clicked() {
                    *dismiss = Some(kept.run);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_or_unplaced_lines_are_taken_or_dismissed_and_a_gap_only_taken() {
        assert_eq!(offers(KeptKind::Run), (true, true));
        assert_eq!(offers(KeptKind::Unplaced), (true, true));
        assert_eq!(offers(KeptKind::Gap), (true, false));
    }
}
