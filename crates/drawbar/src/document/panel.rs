//! A decoded body as a document: the sections the instrument itself is divided into,
//! holding the controls that instrument would be showing.
//!
//! The division is `nord_format::panel`'s, not this app's — which groups a body has, in
//! what order, and which of them the instrument is using for the state the file holds all
//! come off the layout the library ships. A body becomes a panel here by having one.

use std::collections::HashMap;

use eframe::egui;
use nord_format::fields::Field;
use nord_format::panel::{Panel, Section};

use super::controls::{self, Ctx, Sets};
use crate::strings;

/// The field the piano lookup decorates, and so the section it belongs in.
const PIANO_MODEL: &str = "piano_panel.piano_model";

/// The two halves of the transpose control — see [`transpose`].
const TRANSPOSE_ENABLED: &str = "center_panel.transpose_enabled";
const TRANSPOSE: &str = "center_panel.transpose";

/// Any body the library has authored a layout for, nested to whatever depth it uses.
///
/// ⚠️ A group the instrument is not using is not drawn. It is still state the file
/// carries and still writable — the Advanced table is where it stays reachable — but
/// drawing an organ registration for a model that is not selected asserts a sound the
/// program does not make. The pickers that bring a section back are themselves in a
/// group nothing conditions, so nothing can be hidden beyond reach.
pub fn program(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    layout: &'static Panel,
    fields: &[Field],
    piano: &mut PianoLookup,
    sets: &mut Sets,
) {
    let resolved = layout.resolve(fields);
    let folded = fields.len() > FOLD_ABOVE;
    for section in &resolved.sections {
        if !section.relevant {
            continue;
        }
        match folded {
            true => {
                egui::CollapsingHeader::new(section.group.title)
                    .id_salt(section.group.title)
                    .show(ui, |ui| section_body(ui, ctx, section, fields, piano, sets));
            }
            false => controls::section(ui, section.group.title, |ui| {
                section_body(ui, ctx, section, fields, piano, sets);
            }),
        }
    }
}

/// One group: its own controls, then the groups under it, however deep they go.
fn section_body(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    section: &Section,
    fields: &[Field],
    piano: &mut PianoLookup,
    sets: &mut Sets,
) {
    if section.fields.iter().any(|field| field.path == PIANO_MODEL) {
        piano.ui(ui);
    }
    let selectors: Vec<&str> = section
        .groups
        .iter()
        .filter_map(|group| group.selection.map(|selection| selection.field))
        .collect();
    controls::strip(ui, |ui| {
        for cluster in clustered(&section.fields) {
            let field = cluster.parameter;
            if selectors.contains(&field.path.as_str()) {
                continue;
            }
            // ⚠️ Two fields, one control. The layout puts them side by side because
            // neither reads on its own; drawing them as two cells would offer a
            // semitone count the instrument ignores.
            if field.path == TRANSPOSE_ENABLED {
                transpose(ui, &section.fields, sets);
                continue;
            }
            if field.path == TRANSPOSE {
                continue;
            }
            if field.path == PIANO_MODEL && piano.model_cell(ui, field, sets) {
                continue;
            }
            controls::morphed(ui, ctx, field, &cluster.morphs, sets);
        }
    });
    for nested in &section.groups {
        if !nested.relevant {
            continue;
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            match nested.selection {
                Some(selection) => {
                    let selected = selection.selected(fields);
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(
                                selected,
                                egui::RichText::new(nested.group.title).strong(),
                            )
                            .on_hover_text("select the preset the instrument plays")
                            .clicked()
                            && !selected
                        {
                            sets.push((selection.field.to_string(), selection.value.to_string()));
                        }
                        if selected {
                            ui.label(
                                egui::RichText::new("playing")
                                    .small()
                                    .color(crate::app::good(ui.visuals())),
                            );
                        }
                    });
                    ui.add_enabled_ui(selected, |ui| {
                        section_body(ui, ctx, nested, fields, piano, sets)
                    });
                }
                None => {
                    ui.label(egui::RichText::new(nested.group.title).strong());
                    section_body(ui, ctx, nested, fields, piano, sets);
                }
            }
        });
    }
}

/// The settings body, in the order the instrument's own menus run.
pub fn settings(ui: &mut egui::Ui, ctx: &Ctx, fields: &[Field], sets: &mut Sets) {
    for section in strings::SETTINGS_SECTIONS {
        let rows = gather(fields, section);
        if rows.is_empty() {
            continue;
        }
        controls::section(ui, section.title(), |ui| {
            controls::strip(ui, |ui| {
                for field in &rows {
                    controls::cell(ui, ctx, field, sets);
                }
            });
        });
    }
}

/// How many fields a body may hold before its sections start folded away.
///
/// ⚠️ The instrument's own panel does not fold, and neither does the Electro 5 document.
/// A Stage program declares hundreds, and one strip of those is a page nobody reads to
/// the end of — so above this the sections are what the reader opens, one at a time.
const FOLD_ABOVE: usize = 200;

/// How many fields a section may hold before it is divided again on each field's own
/// leading word.
const SPLIT_ABOVE: usize = 128;

/// Any other registry-backed body: its own path prefixes as sections, because nothing
/// here knows how that instrument's panel is divided but the registry does say which
/// fields belong together.
pub fn plain(ui: &mut egui::Ui, ctx: &Ctx, fields: &[Field], sets: &mut Sets) {
    let folded = fields.len() > FOLD_ABOVE;
    ui.label(
        egui::RichText::new(match folded {
            true => "Every field this format declares, under the section of its name. Open one to see it.",
            false => "Every field this format declares.",
        })
        .small()
        .weak(),
    );
    for group in sections(fields) {
        match folded {
            true => {
                egui::CollapsingHeader::new(&group.title)
                    .id_salt(&group.key)
                    .show(ui, |ui| cells(ui, ctx, &group.rows, sets));
            }
            false => controls::section(ui, &group.title, |ui| cells(ui, ctx, &group.rows, sets)),
        }
    }
}

fn cells(ui: &mut egui::Ui, ctx: &Ctx, rows: &[&Field], sets: &mut Sets) {
    controls::strip(ui, |ui| {
        for cluster in clustered(rows) {
            controls::morphed(ui, ctx, cluster.parameter, &cluster.morphs, sets);
        }
    });
}

/// A parameter and the performance controls that morph it, drawn as one cell.
struct Cluster<'a> {
    parameter: &'a Field,
    /// The wheel, aftertouch and control-pedal targets, in declaration order. Empty for
    /// a parameter nothing morphs, which is most of them.
    morphs: Vec<&'a Field>,
}

/// The rows as cells, with every morph slot moved onto the parameter it moves.
///
/// A slot whose parameter is not in the same run — the layout named one and not the
/// other, or the body declares no such parameter — stands as its own cell rather than
/// being dropped.
fn clustered<'a>(rows: &[&'a Field]) -> Vec<Cluster<'a>> {
    let mut out: Vec<Cluster<'a>> = Vec::new();
    let mut at: HashMap<&'a str, usize> = HashMap::new();
    let mut slots: Vec<(&'a Field, String)> = Vec::new();
    for field in rows {
        match field.spec.morph_parent() {
            Some(parent) => slots.push((field, parent)),
            None => {
                at.insert(field.path.as_str(), out.len());
                out.push(Cluster {
                    parameter: field,
                    morphs: Vec::new(),
                });
            }
        }
    }
    for (slot, parent) in slots {
        match at.get(parent.as_str()) {
            Some(&index) => out[index].morphs.push(slot),
            None => out.push(Cluster {
                parameter: slot,
                morphs: Vec::new(),
            }),
        }
    }
    out
}

/// One titled run of a field list.
struct Group<'a> {
    /// What these fields share — the fold's id, which their titles are not unique enough
    /// to be.
    key: String,
    title: String,
    rows: Vec<&'a Field>,
}

/// The sections a field list falls into.
///
/// A nested body's fields are contiguous and share a dotted prefix, which is the division
/// the registry itself makes. A prefix too long to read in one run is divided again on
/// the leading word of each field's own name — the Stage bodies spell their sections
/// there (`slot_a.organ_preset_1_drawbar_1`) — and a word that recurs later joins the
/// division it opened rather than starting a second one.
fn sections(fields: &[Field]) -> Vec<Group<'_>> {
    let mut out: Vec<Group> = Vec::new();
    for field in fields {
        let prefix = field.path.rsplit_once('.').map_or("", |(head, _)| head);
        match out.last_mut() {
            Some(group) if group.key == prefix => group.rows.push(field),
            _ => out.push(Group {
                key: prefix.to_string(),
                title: match prefix.is_empty() {
                    true => "General".to_string(),
                    false => strings::title(prefix),
                },
                rows: vec![field],
            }),
        }
    }
    out.into_iter().flat_map(divide).collect()
}

fn divide(group: Group<'_>) -> Vec<Group<'_>> {
    if group.rows.len() <= SPLIT_ABOVE {
        return vec![group];
    }
    let mut out: Vec<Group> = Vec::new();
    for field in group.rows {
        let leaf = field.path.rsplit('.').next().unwrap_or(&field.path);
        let word = leaf.split('_').next().unwrap_or(leaf);
        let key = format!("{}.{word}", group.key);
        match out.iter().position(|part| part.key == key) {
            Some(at) => out[at].rows.push(field),
            None => out.push(Group {
                title: match group.key.is_empty() {
                    true => strings::title(word),
                    false => format!("{} — {word}", group.title),
                },
                key,
                rows: vec![field],
            }),
        }
    }
    out
}

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
    /// The Model dial as a list of the instrument's own pianos, where the Pianos folder
    /// scan can supply them. `false` where it cannot, and the numeric control stands.
    fn model_cell(&self, ui: &mut egui::Ui, field: &Field, sets: &mut Sets) -> bool {
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

    fn ui(&mut self, ui: &mut egui::Ui) {
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

/// The fields a settings section shows.
///
/// ⚠️ The settings body has no authored layout, so its division is still this app's own
/// table — see `strings::FIELDS`.
fn gather(fields: &[Field], section: strings::Section) -> Vec<&Field> {
    fields
        .iter()
        .filter(|field| strings::section(&field.path) == section)
        .collect()
}

/// The transpose control: a lamp and a number, written together the way the panel's own
/// button writes them — two cells, because that is how the panel prints it.
///
/// ⚠️ Neither field reads on its own. `transpose_enabled` is sticky — the instrument sets
/// it the first time transposition is touched and never clears it — and an untouched
/// program stores `+1` in the value rather than `0`. The instrument ignores the amount
/// while the lamp is dark, and moving the amount is what lights it.
/// Confirmed on hardware.
pub fn transpose(ui: &mut egui::Ui, fields: &[&Field], sets: &mut Sets) {
    /// The panel's own travel, either side of nothing.
    const SEMITONES: i64 = 6;

    let value = |path: &str| fields.iter().find(|field| field.path == path);
    let on = value(TRANSPOSE_ENABLED).is_some_and(|field| field.value == "true");
    let Some(semitones) =
        value(TRANSPOSE).and_then(|field| field.value.trim_start_matches('+').parse::<i64>().ok())
    else {
        return;
    };

    let mut switched = None;
    controls::named_cell(ui, TRANSPOSE_ENABLED, 78.0, |ui| {
        switched = crate::led::ui(ui, on, "");
    })
    .on_hover_text("the transpose light on the panel");

    let mut moved = None;
    controls::named_cell(ui, TRANSPOSE, 78.0, |ui| {
        moved = crate::knob::ui(ui, TRANSPOSE, semitones, -SEMITONES, SEMITONES);
    });

    // Moving the semitones turns the light on, which is what the panel does.
    let (on, semitones) = match (switched, moved) {
        (_, Some(want)) => (true, want),
        (Some(want_on), None) => (want_on, semitones),
        (None, None) => return,
    };
    sets.push((TRANSPOSE_ENABLED.to_string(), on.to_string()));
    sets.push((TRANSPOSE.to_string(), semitones.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{apply, blank};
    use nord_format::formats::{ne5, ns4};
    use nord_format::{Entity, Program};

    fn electro5() -> Vec<Field> {
        let entity = Entity::Program(Program::Electro5(ne5::program::new(
            (0, 0).try_into().unwrap(),
        )));
        apply(&nord_format::to_bytes(&entity).unwrap(), &[])
            .unwrap()
            .0
    }

    /// Every control the view offers comes off the layout, so nothing this app knows
    /// about a body's sections can disagree with what the library says.
    #[test]
    fn the_electro5_view_is_the_librarys_layout() {
        let fields = electro5();
        let resolved = ne5::program::PANEL.resolve(&fields);
        let leftovers: Vec<&str> = resolved
            .leftovers
            .iter()
            .map(|field| field.path.as_str())
            .collect();
        assert_eq!(leftovers.len(), 6, "{leftovers:?}");
        assert!(leftovers.contains(&"program_version"));
        assert!(leftovers.contains(&"piano_panel.id"));
        assert!(leftovers.contains(&"sample_panel.id"));
        let titles: Vec<&str> = resolved
            .sections
            .iter()
            .filter(|section| section.relevant)
            .map(|section| section.group.title)
            .collect();
        // A fresh program plays organ on both parts, so piano and sample are state
        // rather than controls and the view does not offer them.
        assert!(titles.contains(&"Keyboard & split"));
        assert!(titles.contains(&"Organ"));
        assert!(!titles.contains(&"Piano"));
    }

    /// The transpose pair is drawn as one control, which needs both halves in the same
    /// group — the layout is what puts them there.
    #[test]
    fn the_transpose_pair_stays_in_one_group() {
        let fields = electro5();
        let resolved = ne5::program::PANEL.resolve(&fields);
        let keyboard = resolved
            .sections
            .iter()
            .find(|section| section.group.title == "Keyboard & split")
            .expect("the keyboard section");
        let paths: Vec<&str> = keyboard
            .fields
            .iter()
            .map(|field| field.path.as_str())
            .collect();
        assert!(paths.contains(&TRANSPOSE_ENABLED));
        assert!(paths.contains(&TRANSPOSE));
    }

    /// A morph target is the value its parameter is driven to, so it is drawn on that
    /// parameter and never as a control of its own.
    #[test]
    fn a_morph_slot_is_drawn_on_the_parameter_it_moves() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let rows: Vec<&Field> = fields.iter().collect();
        let clusters = clustered(&rows);

        let volume = clusters
            .iter()
            .find(|cluster| cluster.parameter.path == "organ_a_volume")
            .expect("organ_a_volume");
        let morphs: Vec<&str> = volume
            .morphs
            .iter()
            .map(|field| field.path.as_str())
            .collect();
        assert_eq!(
            morphs,
            [
                "organ_a_volume_wheel",
                "organ_a_volume_aftertouch",
                "organ_a_volume_ctrl_pedal",
            ]
        );
        assert!(
            !clusters
                .iter()
                .any(|cluster| cluster.parameter.path == "organ_a_volume_wheel"),
            "a slot with a parameter is not a cell of its own",
        );
        assert_eq!(morph_paths(&clusters).len(), 0, "no slot stands alone here");
    }

    /// A slot whose parameter the body does not declare has nothing to ride on, so it
    /// keeps a cell rather than disappearing.
    #[test]
    fn a_slot_with_no_parameter_beside_it_still_gets_a_cell() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let rows: Vec<&Field> = fields
            .iter()
            .filter(|field| field.path != "organ_a_volume")
            .collect();
        let clusters = clustered(&rows);
        assert!(clusters
            .iter()
            .any(|cluster| cluster.parameter.path == "organ_a_volume_wheel"));
    }

    /// A layout exists for the Stage 4 program too, so the same view serves it.
    #[test]
    fn a_stage4_program_has_a_layout_as_well() {
        let (fields, _) = apply(&blank::stage4_program(), &[]).unwrap();
        let resolved = ns4::program::PANEL.resolve(&fields);
        assert!(!resolved.sections.is_empty());
    }

    /// The slots that ended up as their own cells.
    fn morph_paths<'a>(clusters: &[Cluster<'a>]) -> Vec<&'a str> {
        clusters
            .iter()
            .filter(|cluster| cluster.parameter.spec.morph_parent().is_some())
            .map(|cluster| cluster.parameter.path.as_str())
            .collect()
    }

    fn titles(bytes: Vec<u8>) -> Vec<String> {
        let (fields, _) = apply(&bytes, &[]).unwrap();
        let groups = sections(&fields);
        assert_eq!(
            fields.len(),
            groups.iter().map(|group| group.rows.len()).sum::<usize>(),
            "every field lands in exactly one section",
        );
        let mut keys: Vec<&str> = groups.iter().map(|group| group.key.as_str()).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "every fold answers to an id of its own");
        groups.into_iter().map(|group| group.title).collect()
    }

    /// The generic view's sections are the registry's own divisions: the prefix a nested
    /// body's fields share, and the body's own fields under one heading before them.
    #[test]
    fn a_body_falls_into_the_sections_its_paths_name() {
        let titles = titles(blank::stage4_program());
        assert_eq!(titles.first().map(String::as_str), Some("General"));
        assert!(titles.contains(&"Organ a".to_string()), "{titles:?}");
        assert!(titles.contains(&"Synth a fx".to_string()), "{titles:?}");
    }

    /// A prefix longer than a page divides again on the leading word of each field's own
    /// name, which is where the Stage 2 spells what part of the slot a field belongs to.
    #[test]
    fn a_section_too_long_to_read_divides_on_its_fields_own_words() {
        let titles = titles(blank::stage2_program());
        assert!(titles.contains(&"Slot a — organ".to_string()), "{titles:?}");
        assert!(titles.contains(&"Slot b — piano".to_string()), "{titles:?}");
    }

    /// A body small enough to read whole keeps one section per prefix.
    #[test]
    fn a_short_body_is_not_divided() {
        assert_eq!(titles(blank::stage3_synth()), ["General"]);
    }
}
