//! What is known about the piano a program plays, which is the one thing in a field
//! document that the file alone cannot answer.
//!
//! The division of a body into sections is `nord_format::panel`'s and the cells are
//! [`super::field`]'s. This is the catalogue beside them: an id the file stores, the name
//! only an attached instrument has for it, and the Model dial as that instrument's own
//! list of pianos.

use eframe::egui;
use nord_format::fields::{ControlKind, Field, Library};

use super::controls::{self, Sets};

/// The field the piano lookup decorates, and so the section it belongs in.
pub const PIANO_MODEL: &str = "piano_panel.piano_model";

/// What is known about the piano a program plays, and the way to ask for the rest.
///
/// ⚠️ The file stores an **id** for the piano and, separately, the panel's category and
/// Model dial position. The id is the identity; the dial position is a coordinate whose
/// meaning lives in the instrument's own library. Only the id can be resolved to a name,
/// and only the instrument can resolve it — so a name shown here always came off the
/// wire, never out of the file.
pub struct PianoLookup {
    /// The id the file names, or `None` where it references no piano at all.
    pub id: Option<u32>,
    /// What the instrument called that id, once it has been asked.
    pub name: Option<String>,
    /// Whether asking is possible: an attached instrument, and a slot to ask about.
    pub can_ask: bool,
    /// Set when the operator asks. The document turns it into one `DEPENDENCIES` read.
    pub asked: bool,
    /// The Pianos folder's names for the current category, by Model dial position.
    /// Empty when the scan cannot answer, and the Model dial stays numeric.
    pub models: Vec<(u32, String)>,
    /// The scan's name for the current position, where it disagrees with the
    /// instrument's dependency reply — the signal that the position mapping is wrong.
    pub scan_disagrees: Option<String>,
}

impl PianoLookup {
    /// The catalogue's name for a library reference, where this lookup resolves it.
    ///
    /// Only the piano library has a catalogue here, and only for the id the instrument
    /// was asked about — every other reference shows the id the file stores.
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
            // A dial position past the scanned list is shown as the number it is,
            // not silently snapped to a piano it does not name.
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

    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(scanned) = &self.scan_disagrees {
            ui.colored_label(
                crate::app::warn(ui.visuals()),
                format!(
                    "the model list calls this position {scanned:?}, but the instrument's \
                     dependency reply names the piano below — trust the instrument",
                ),
            );
        }
        let Some(id) = self.id else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("currently").small().weak());
            match &self.name {
                Some(name) => {
                    ui.label(egui::RichText::new(name).strong());
                    ui.label(
                        egui::RichText::new("— named by the instrument")
                            .small()
                            .weak(),
                    );
                    if self.can_ask {
                        self.asked |= ui
                            .small_button("Ask again")
                            .on_hover_text("read this program's dependencies again")
                            .clicked();
                    }
                }
                None => {
                    ui.label(egui::RichText::new(format!("piano {id:#010x}")).monospace());
                    match self.can_ask {
                        true => {
                            self.asked |= ui
                                .small_button("Ask the instrument")
                                .on_hover_text("read this program's dependencies for the name")
                                .clicked();
                        }
                        false => {
                            ui.label(
                                egui::RichText::new(
                                    "— the file stores the id; only the instrument knows the name",
                                )
                                .small()
                                .weak(),
                            );
                        }
                    }
                }
            }
        });
    }
}
