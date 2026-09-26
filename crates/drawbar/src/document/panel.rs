//! What is known about the piano a program plays: the one fact in a field document
//! that the file alone cannot supply.
//!
//! `nord_format::panel` divides a body into sections and [`super::field`] draws the
//! cells. This module is the catalog beside them: the id the file stores, the name only
//! an attached instrument has for it, and the Model dial as that instrument's list of
//! pianos.

use eframe::egui;
use nord_format::fields::{ControlKind, Field, Library};

use super::controls::{self, Sets};

/// The field the piano lookup decorates, and so the section it belongs in.
pub const PIANO_MODEL: &str = "piano_panel.piano_model";

/// What is known about the piano a program plays, and what it would take to know more.
///
/// ⚠️ The file stores an **id** for the piano and, separately, the panel's category and
/// Model dial position. The id identifies the piano; the dial position means something
/// only in the instrument's own library. Only the id resolves to a name, and only the
/// instrument can resolve it, so a name shown here always comes from the instrument and
/// never from the file.
pub struct PianoLookup {
    /// The id the file names, or `None` where it references no piano at all.
    pub id: Option<u32>,
    /// What the instrument called that id, where it has named it.
    pub name: Option<String>,
    /// Whether asking is possible: an attached instrument, and a slot to ask about.
    pub can_ask: bool,
    /// Whether the instrument refused the last read of the slot's dependencies.
    pub refused: bool,
    /// Set when the operator asks again after a refusal.
    pub asked: bool,
    /// The Pianos folder's names for the current category, by Model dial position.
    /// Empty when the scan cannot answer, and the Model dial stays numeric.
    pub models: Vec<(u32, String)>,
    /// The scan's name for the current position, where it disagrees with the
    /// instrument's dependency reply. A disagreement means the position mapping is wrong.
    pub scan_disagrees: Option<String>,
}

impl PianoLookup {
    /// Whether the instrument could put a name to the piano this program references and
    /// has not done so yet.
    pub(super) fn wants_a_name(&self) -> bool {
        self.can_ask && self.id.is_some() && self.name.is_none()
    }

    /// The catalog's name for a library reference, where this lookup resolves it.
    ///
    /// Only the piano library has a catalog here, and only for the id the instrument has
    /// named. Every other reference shows the id the file stores.
    pub(super) fn names(&self, field: &Field) -> Option<&str> {
        if field.spec.control != ControlKind::Reference(Library::Piano) {
            return None;
        }
        if super::library_id(&field.value) != self.id {
            return None;
        }
        self.name.as_deref()
    }

    /// The Model dial as a list of the instrument's own pianos, where the Pianos folder
    /// scan can supply them. `false` where it cannot, and the numeric control stands.
    pub(super) fn model_cell(&self, ui: &mut egui::Ui, field: &Field, sets: &mut Sets) -> bool {
        if self.models.is_empty() {
            return false;
        }
        let current: Option<u32> = field.value.trim().parse().ok();
        let shown = current
            .and_then(|n| self.models.iter().find(|(position, _)| *position == n))
            .map(|(position, name)| format!("{position} — {name}"))
            // A dial position past the scanned list shows its number.
            .unwrap_or_else(|| field.value.clone());
        controls::named_cell(ui, &field.path, 230.0, |ui| {
            egui::ComboBox::from_id_salt("piano-model-names")
                .selected_text(shown)
                .width(214.0)
                .show_ui(ui, |ui| {
                    for (position, name) in &self.models {
                        let row = format!("{position} — {name}");
                        if ui
                            .selectable_label(current == Some(*position), row)
                            .clicked()
                        {
                            sets.push((field.path.clone(), position.to_string()));
                        }
                    }
                });
        });
        true
    }

    /// ⚠️ The dependency reply is the instrument's own answer about the piano this
    /// program plays; the model list is the scan's reading of a dial position. Where the
    /// two disagree, the position mapping is wrong and the reader has to be told.
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        if self.refused && self.wants_a_name() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("the instrument did not say which piano this plays")
                        .small()
                        .weak(),
                );
                self.asked |= ui
                    .small_button("Ask again")
                    .on_hover_text("read this program's dependencies again")
                    .clicked();
            });
        }
        let Some(scanned) = &self.scan_disagrees else {
            return;
        };
        ui.colored_label(
            crate::app::warn(ui.visuals()),
            format!(
                "the model list calls this position {scanned:?}, but the instrument's \
                 dependency reply names the piano below. Trust the instrument.",
            ),
        );
    }
}
