//! The sample-instrument document.
//!
//! A sample is mostly encoded audio, so what is settable is what the format can patch in
//! place without touching a stroke: the name, and each zone's root key and top note.
//! The v3/v4 generations decode read-only and are carried verbatim.

use std::io::Cursor;

use eframe::egui;
use nord_format::cbin::Cbin;
use nord_format::formats::nsmp::{self, Sample};
use nord_format::Entity;

use super::controls::Sets;
use crate::note;

pub fn is_editable(entity: &Entity) -> bool {
    matches!(entity, Entity::Sample(nord_format::Sample::V2(_)))
}

pub fn is_sample(entity: &Entity) -> bool {
    matches!(entity, Entity::Sample(_))
}

fn sample(entity: &Entity) -> Option<&Cbin<Sample>> {
    match entity {
        Entity::Sample(nord_format::Sample::V2(sample)) => Some(sample),
        _ => None,
    }
}

fn sample_mut(entity: &mut Entity) -> Option<&mut Cbin<Sample>> {
    match entity {
        Entity::Sample(nord_format::Sample::V2(sample)) => Some(sample),
        _ => None,
    }
}

/// One zone, numbered the way the panel numbers them: from 1, top of the keyboard first.
#[derive(Clone, PartialEq, Eq)]
pub struct Zone {
    pub root_key: u8,
    pub top_note: u8,
}

/// Everything settable, in one read.
#[derive(Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    pub zones: Vec<Zone>,
}

pub fn snapshot(entity: &Entity) -> Option<Result<Snapshot, String>> {
    let sample = sample(entity)?;
    Some(read(sample))
}

fn read(sample: &Cbin<Sample>) -> Result<Snapshot, String> {
    let zones = sample.zones().map_err(|e| e.to_string())?;
    let strokes = sample.strokes().map_err(|e| e.to_string())?;
    Ok(Snapshot {
        name: sample.name().map_err(|e| e.to_string())?,
        zones: zones
            .iter()
            .zip(&strokes)
            .map(|(zone, stroke)| Zone {
                root_key: stroke.root_key,
                top_note: zone.top_note,
            })
            .collect(),
    })
}

/// Apply one `path = value`. Paths are the CLI's: `name`, `zone1.root_key`,
/// `zone1.top_note`.
fn set(sample: &mut Cbin<Sample>, path: &str, value: &str) -> Result<(), String> {
    if path == "name" {
        return sample.set_name(value).map_err(|e| e.to_string());
    }
    let unknown = || format!("unknown field {path:?}");
    let (zone, field) = path.split_once('.').ok_or_else(unknown)?;
    let index = zone
        .strip_prefix("zone")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .ok_or_else(unknown)?;
    // Checked here so the message speaks the panel's 1-based numbering, not the format
    // crate's 0-based one.
    let zones = sample.zones().map_err(|e| e.to_string())?.len();
    if index > zones {
        return Err(format!("there is no zone {index}: this sample has {zones}"));
    }
    let note = note::parse(value)?;
    match field {
        "root_key" => sample.set_root_key(index - 1, note),
        "top_note" => sample.set_zone_top_note(index - 1, note),
        _ => return Err(unknown()),
    }
    .map_err(|e| e.to_string())
}

/// Apply every set to a fresh decode and re-encode, the same all-or-nothing rule the
/// registry bodies follow.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut entity =
        nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let sample = sample_mut(&mut entity).ok_or("not a v2 sample instrument")?;
    for (path, value) in sets {
        set(sample, path, value)?;
    }
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// The range a zone covers, in plain words.
///
/// Zones are stored high to low and the panel numbers them from 1 at the top of the
/// keyboard; a zone's bottom is one note above the next record's top.
pub fn range(zones: &[Zone], index: usize) -> String {
    let top = note::name(zones[index].top_note);
    match zones.get(index + 1) {
        Some(below) => format!(
            "{} up to {top}",
            note::name(below.top_note.saturating_add(1))
        ),
        None => format!("up to {top}"),
    }
}

/// The name and the zone map. Everything else about a sample is audio.
pub fn ui(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    name: &mut String,
    editable: bool,
    sets: &mut Sets,
) {
    if !editable {
        ui.label(
            egui::RichText::new(
                "This sample is carried as it came: only version 2 content can be changed here.",
            )
            .weak(),
        );
    }
    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            ui.add_sized(
                [120.0, ui.spacing().interact_size.y],
                egui::Label::new("Name").halign(egui::Align::LEFT),
            );
            let response = ui.add(
                egui::TextEdit::singleline(name)
                    .desired_width(200.0)
                    .char_limit(nsmp::MAX_NAME_LEN),
            );
            // Not committed per keystroke: half a name is a name the format would take.
            let done =
                response.lost_focus() || response.ctx.input(|i| i.key_pressed(egui::Key::Enter));
            if done && *name != snapshot.name {
                sets.push(("name".to_string(), name.clone()));
            }
        });

        for (i, zone) in snapshot.zones.iter().enumerate() {
            let n = i + 1;
            ui.horizontal(|ui| {
                ui.add_sized(
                    [120.0, ui.spacing().interact_size.y],
                    egui::Label::new(format!("Zone {n}")).halign(egui::Align::LEFT),
                );
                ui.label(
                    egui::RichText::new(range(&snapshot.zones, i))
                        .small()
                        .weak(),
                );
                ui.label("root key");
                if let Some(note) = note_picker(ui, ("root", n), zone.root_key) {
                    sets.push((format!("zone{n}.root_key"), note));
                }
                ui.label("top note");
                if let Some(note) = note_picker(ui, ("top", n), zone.top_note) {
                    sets.push((format!("zone{n}.top_note"), note));
                }
            });
        }
    });
}

/// A MIDI note as a name: `C4` is middle C. Typing a number works too.
fn note_picker(ui: &mut egui::Ui, id: (&str, usize), note: u8) -> Option<String> {
    let mut value = note as f64;
    let response = ui.push_id(id, |ui| {
        ui.add(
            egui::DragValue::new(&mut value)
                .range(0.0..=127.0)
                .speed(0.2)
                .custom_formatter(|n, _| note::name(n as u8))
                .custom_parser(|text| note::parse(text).ok().map(|n| n as f64)),
        )
    });
    let picked = value.round() as u8;
    (response.inner.changed() && picked != note).then(|| note::name(picked))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A note is spelled the way the document shows it, and the round trip is exact.
    #[test]
    fn zone_notes_are_spelled_as_names() {
        assert_eq!(note::name(60), "C4");
        assert_eq!(note::parse("C4").unwrap(), 60);
    }

    /// A zone reads as the stretch of keyboard it covers, and the last one runs to the
    /// bottom.
    #[test]
    fn a_zone_reads_as_the_keys_it_covers() {
        let zones = vec![
            Zone {
                root_key: 72,
                top_note: 96,
            },
            Zone {
                root_key: 60,
                top_note: 71,
            },
        ];
        // Zone 2 tops out at B4, so zone 1 starts one key above it.
        assert_eq!(range(&zones, 0), "C5 up to C7");
        assert_eq!(range(&zones, 1), "up to B4");
    }

    /// Only v2 content has the in-place patches this editor makes; a v3/v4 instrument
    /// is carried and shown, not edited.
    #[test]
    fn a_later_generation_is_carried_rather_than_edited() {
        let entity = Entity::Sample(nord_format::Sample::V3(Cbin {
            header: nord_format::cbin::Header::new("nsmp", (0, 0), 300),
            body: nsmp::SampleV3 {
                sections: Vec::new(),
            },
        }));
        assert!(is_sample(&entity));
        assert!(!is_editable(&entity));
        assert!(snapshot(&entity).is_none());
    }
}
