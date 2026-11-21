//! What is known about the piano and sample a program plays, the facts in a field
//! document that the file alone cannot supply: the ids the file stores, the names they
//! name, and the Model dial as the attached instrument's list of pianos.

use eframe::egui;
use nord_format::fields::{ControlKind, Field, Library};

use super::controls::Sets;
use super::sample;
use crate::strings;

/// The field the piano lookup decorates, and so the section it belongs in.
pub const PIANO_MODEL: &str = "piano_panel.piano_model";

/// The panel's Type dial, which picks the Pianos bank the Model dial counts in.
pub const PIANO_CATEGORY: &str = "piano_panel.category";

/// The id of the piano the program plays.
pub const PIANO_ID: &str = "piano_panel.id";

/// The id of the sample the program plays.
pub const SAMPLE_ID: &str = "sample_panel.id";

/// What is known about the piano a program plays.
///
/// ⚠️ The file stores the piano's id and, separately, the panel's category and Model dial
/// position, which mean something only in the instrument's own library. The file stores
/// no name: a name shown here is that of a piano file or slot whose name hashes to the id.
pub struct PianoLookup {
    /// The id the file names, or `None` where it stores 0 and the dials pick the piano.
    pub id: Option<u32>,
    /// The name of the piano that id names, where one here or on the instrument has it.
    pub name: Option<String>,
    /// Where the file stores 0, the scan's name for the piano the dials pick.
    pub dialed: Option<String>,
    /// The Pianos folder's names for the current category, by Model dial position.
    /// Empty when the scan cannot answer, and the Model dial stays numeric.
    pub models: Vec<(u32, String)>,
    /// The scan's name for the current position, where the id names another piano.
    pub scan_disagrees: Option<String>,
    /// The sample id the file stores, and the name of the sample it names, where one here
    /// or on the instrument has it.
    pub sample: Option<(u32, String)>,
}

impl PianoLookup {
    /// The name of a library reference, where this lookup resolves it: the piano or
    /// sample the id names, and 0 for the piano the dials pick. Every other reference
    /// shows the id the file stores.
    pub(super) fn names(&self, field: &Field) -> Option<&str> {
        let id = super::library_id(&field.value);
        match field.spec.control {
            ControlKind::Reference(Library::Piano) => match id {
                Some(0) => self.dialed.as_deref(),
                id if id == self.id => self.name.as_deref(),
                _ => None,
            },
            ControlKind::Reference(Library::Sample) => self
                .sample
                .as_ref()
                .filter(|(named, _)| id == Some(*named))
                .map(|(_, name)| name.as_str()),
            _ => None,
        }
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
        sample::cell(ui, &strings::label(&field.path), 230.0, |ui| {
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

    /// Where the dials point at another piano than the id names, says so: see
    /// `PianoPanel::id` in `nord-format` for which one plays.
    pub(super) fn ui(&self, ui: &mut egui::Ui) {
        let Some(scanned) = &self.scan_disagrees else {
            return;
        };
        ui.colored_label(
            crate::app::warn(ui.visuals()),
            format!("The dials point at “{scanned}”; this plays the piano below."),
        );
    }
}
