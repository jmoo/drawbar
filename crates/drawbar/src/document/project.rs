//! The Sample Editor project document.
//!
//! A `.nsmpproj` is the text file the editor saves and generates an `nsmp`
//! from. What is settable is what `nord-format` can edit in place: the
//! instrument's name and velocity defaults, each zone's root key and key
//! range, each stroke's trim, loop, gain and velocity window, and each audio
//! file's path. Paths are the CLI's — `name`, `zone129.root_key`,
//! `stroke1.loop_start`, `velocity.attack_amount`, `file1.path` — with zones
//! and files addressed by their stored ids and a stroke by its global id.

use std::collections::HashMap;
use std::io::Cursor;

use eframe::egui;
use nord_format::formats::nsmpproj::{Project, StrokeField, VelocityDefaults, MAX_VELOCITY};
use nord_format::Entity;

use super::controls::Sets;
use super::sample::note_picker;
use crate::note;

pub fn is_project(entity: &Entity) -> bool {
    matches!(entity, Entity::SampleProject(_))
}

fn project(entity: &Entity) -> Option<&Project> {
    match entity {
        Entity::SampleProject(project) => Some(project),
        _ => None,
    }
}

fn project_mut(entity: &mut Entity) -> Option<&mut Project> {
    match entity {
        Entity::SampleProject(project) => Some(project),
        _ => None,
    }
}

/// One zone, under the id the file gives it.
#[derive(Clone, PartialEq, Eq)]
pub struct Zone {
    pub id: u32,
    pub root_key: u8,
    pub bottom_note: u8,
    pub top_note: u8,
    pub enabled: bool,
}

/// One audio file, under the id the strokes reference it by.
#[derive(Clone, PartialEq, Eq)]
pub struct AudioFile {
    pub id: u32,
    pub path: String,
}

/// One stroke, under the global id both blocks naming it use.
#[derive(Clone, PartialEq)]
pub struct Stroke {
    pub id: u32,
    pub start: f64,
    pub stop: f64,
    pub gain: f64,
    /// `velocity_min..=velocity_max`.
    pub velocity: (u8, u8),
    pub loop_enabled: bool,
    pub loop_start: f64,
    pub loop_length: f64,
}

/// Everything settable, in one read.
#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub zones: Vec<Zone>,
    pub files: Vec<AudioFile>,
    pub strokes: Vec<Stroke>,
    pub velocity: VelocityDefaults,
}

pub fn snapshot(entity: &Entity) -> Option<Result<Snapshot, String>> {
    let project = project(entity)?;
    Some(read(project))
}

fn read(project: &Project) -> Result<Snapshot, String> {
    // Gain and the velocity window sit in the `map_stroke`, the trim and loop
    // points in the `common_stroke`; the global id is what joins them.
    let played: Vec<_> = project
        .zones()
        .map_err(|e| e.to_string())?
        .into_iter()
        .flat_map(|z| z.strokes)
        .collect();
    let strokes = project
        .strokes()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| {
            let map = played
                .iter()
                .find(|z| z.global_id == s.global_id)
                .ok_or_else(|| format!("stroke {} is in no zone's map", s.global_id))?;
            Ok(Stroke {
                id: s.global_id,
                start: s.start,
                stop: s.stop,
                gain: map.gain,
                velocity: map.velocity,
                loop_enabled: s.loop_enabled,
                loop_start: s.loop_start,
                loop_length: s.loop_length,
            })
        })
        .collect::<Result<_, String>>()?;

    Ok(Snapshot {
        name: project.name().map_err(|e| e.to_string())?,
        strokes,
        velocity: project.velocity_defaults().map_err(|e| e.to_string())?,
        zones: project
            .zones()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|z| Zone {
                id: z.zone_id,
                root_key: z.root_key,
                bottom_note: z.bottom_note,
                top_note: z.top_note,
                enabled: z.enabled,
            })
            .collect(),
        files: project
            .audio_files()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|f| AudioFile {
                id: f.id,
                path: f.path,
            })
            .collect(),
    })
}

/// Apply one `path = value`, in the CLI's vocabulary.
fn set(project: &mut Project, path: &str, value: &str) -> Result<(), String> {
    if path == "name" {
        return project.set_name(value).map_err(|e| e.to_string());
    }
    let unknown = || format!("unknown field {path:?}");
    let (block, field) = path.split_once('.').ok_or_else(unknown)?;
    if let Some(id) = indexed(block, "file") {
        if field != "path" {
            return Err(unknown());
        }
        return project.set_audio_path(id, value).map_err(|e| e.to_string());
    }
    if let Some(id) = indexed(block, "stroke") {
        let field = StrokeField::parse(field, value).map_err(|e| e.to_string())?;
        return project
            .set_stroke_field(id, field)
            .map_err(|e| e.to_string());
    }
    if block == "velocity" {
        let mut defaults = project.velocity_defaults().map_err(|e| e.to_string())?;
        let stored = || {
            value
                .parse::<u8>()
                .map_err(|_| format!("{path}: {value:?} is not a whole number, 0-255"))
        };
        match field {
            "attack_amount" => defaults.attack_amount = stored()?,
            "amplitude" => defaults.amplitude = stored()?,
            "timbre" => defaults.timbre = stored()?,
            _ => return Err(unknown()),
        }
        return project
            .set_velocity_defaults(defaults)
            .map_err(|e| e.to_string());
    }
    let id = indexed(block, "zone").ok_or_else(unknown)?;
    let zones = project.zones().map_err(|e| e.to_string())?;
    let zone = zones
        .iter()
        .find(|z| z.zone_id == id)
        .ok_or_else(|| format!("this project has no zone {id}"))?;
    let note = note::parse(value)?;
    match field {
        "root_key" => project.set_root_key(id, note),
        "bottom_note" => project.set_key_range(id, note, zone.top_note),
        "top_note" => project.set_key_range(id, zone.bottom_note, note),
        _ => return Err(unknown()),
    }
    .map_err(|e| e.to_string())
}

fn indexed(part: &str, label: &str) -> Option<u32> {
    part.strip_prefix(label).and_then(|n| n.parse().ok())
}

/// A drag over one number, spelling the new value only once it has moved.
fn drag<H: std::hash::Hash>(
    ui: &mut egui::Ui,
    id: (&str, H),
    value: f64,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
) -> Option<String> {
    let mut moved = value;
    let response = ui.push_id(id, |ui| {
        ui.add(egui::DragValue::new(&mut moved).range(range).speed(speed))
    });
    (response.inner.changed() && moved != value).then(|| moved.to_string())
}

/// A frame position. Nothing in the format caps one: the editor repairs a
/// position past the file's end on load.
fn frames(ui: &mut egui::Ui, id: (&str, u32), value: f64) -> Option<String> {
    drag(ui, id, value, 0.0..=f64::MAX, 1.0)
}

/// Apply every set to a fresh decode and re-encode, the same all-or-nothing rule
/// the registry bodies follow.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut entity =
        nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let project = project_mut(&mut entity).ok_or("not a Sample Editor project")?;
    for (path, value) in sets {
        set(project, path, value)?;
    }
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// The name, the zone map, and the audio files behind it.
pub fn ui(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    name: &mut String,
    paths: &mut HashMap<u32, String>,
    sets: &mut Sets,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [120.0, ui.spacing().interact_size.y],
            egui::Label::new("Name").halign(egui::Align::LEFT),
        );
        let response = ui.add(egui::TextEdit::singleline(name).desired_width(200.0));
        // Not committed per keystroke: half a name is a name the format would take.
        let done = response.lost_focus() || response.ctx.input(|i| i.key_pressed(egui::Key::Enter));
        if done && *name != snapshot.name {
            sets.push(("name".to_string(), name.clone()));
        }
    });

    for zone in &snapshot.zones {
        let id = zone.id;
        ui.horizontal(|ui| {
            ui.add_sized(
                [120.0, ui.spacing().interact_size.y],
                egui::Label::new(format!("Zone {id}")).halign(egui::Align::LEFT),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{}..{}{}",
                    note::name(zone.bottom_note),
                    note::name(zone.top_note),
                    if zone.enabled { "" } else { "  (disabled)" },
                ))
                .small()
                .weak(),
            );
            ui.label("root key");
            if let Some(note) = note_picker(ui, ("proj_root", id as usize), zone.root_key) {
                sets.push((format!("zone{id}.root_key"), note::name(note)));
            }
            ui.label("bottom");
            if let Some(note) = note_picker(ui, ("proj_btm", id as usize), zone.bottom_note) {
                sets.push((format!("zone{id}.bottom_note"), note::name(note)));
            }
            ui.label("top");
            if let Some(note) = note_picker(ui, ("proj_top", id as usize), zone.top_note) {
                sets.push((format!("zone{id}.top_note"), note::name(note)));
            }
        });
    }

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Strokes, by the global id the file gives them.")
            .small()
            .weak(),
    );
    for stroke in &snapshot.strokes {
        let id = stroke.id;
        ui.horizontal_wrapped(|ui| {
            ui.add_sized(
                [120.0, ui.spacing().interact_size.y],
                egui::Label::new(format!("Stroke {id}")).halign(egui::Align::LEFT),
            );
            ui.label("trim");
            if let Some(v) = frames(ui, ("proj_start", id), stroke.start) {
                sets.push((format!("stroke{id}.start"), v));
            }
            if let Some(v) = frames(ui, ("proj_stop", id), stroke.stop) {
                sets.push((format!("stroke{id}.stop"), v));
            }
            ui.label("gain");
            if let Some(v) = drag(ui, ("proj_gain", id), stroke.gain, 0.0..=16.0, 0.01) {
                sets.push((format!("stroke{id}.gain"), v));
            }
            ui.label("vel");
            let top = MAX_VELOCITY as f64;
            if let Some(v) = drag(
                ui,
                ("proj_vmin", id),
                stroke.velocity.0 as f64,
                0.0..=top,
                0.5,
            ) {
                sets.push((format!("stroke{id}.velocity_min"), v));
            }
            if let Some(v) = drag(
                ui,
                ("proj_vmax", id),
                stroke.velocity.1 as f64,
                0.0..=top,
                0.5,
            ) {
                sets.push((format!("stroke{id}.velocity_max"), v));
            }
            let mut on = stroke.loop_enabled;
            let toggled = ui
                .push_id(("proj_loop", id), |ui| ui.checkbox(&mut on, "loop"))
                .inner;
            if toggled.changed() {
                sets.push((format!("stroke{id}.loop_enabled"), on.to_string()));
            }
            if let Some(v) = frames(ui, ("proj_loop_start", id), stroke.loop_start) {
                sets.push((format!("stroke{id}.loop_start"), v));
            }
            if let Some(v) = frames(ui, ("proj_loop_len", id), stroke.loop_length) {
                sets.push((format!("stroke{id}.loop_length"), v));
            }
        });
    }

    ui.horizontal(|ui| {
        ui.add_sized(
            [120.0, ui.spacing().interact_size.y],
            egui::Label::new("Velocity").halign(egui::Align::LEFT),
        );
        for (label, field, value) in [
            ("attack", "attack_amount", snapshot.velocity.attack_amount),
            ("amplitude", "amplitude", snapshot.velocity.amplitude),
            ("timbre", "timbre", snapshot.velocity.timbre),
        ] {
            ui.label(label);
            if let Some(v) = drag(ui, ("proj_vel", field), value as f64, 0.0..=255.0, 0.5) {
                sets.push((format!("velocity.{field}"), v));
            }
        }
    });

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Audio files, as the editor will look for them.")
            .small()
            .weak(),
    );
    for file in &snapshot.files {
        let id = file.id;
        let path = paths.entry(id).or_insert_with(|| file.path.clone());
        ui.horizontal(|ui| {
            ui.add_sized(
                [120.0, ui.spacing().interact_size.y],
                egui::Label::new(format!("File {id}")).halign(egui::Align::LEFT),
            );
            let response = ui.add(egui::TextEdit::singleline(path).desired_width(320.0));
            let done =
                response.lost_focus() || response.ctx.input(|i| i.key_pressed(egui::Key::Enter));
            if done && *path != file.path {
                sets.push((format!("file{id}.path"), path.clone()));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::nsmpproj::NewZone;

    fn project_bytes() -> Vec<u8> {
        let project = Project::new(
            "Marimba",
            &[
                NewZone {
                    path: "low.wav".into(),
                    sample_rate: 44100,
                    frames: 44100,
                    root_key: 48,
                },
                NewZone {
                    path: "high.wav".into(),
                    sample_rate: 44100,
                    frames: 44100,
                    root_key: 72,
                },
            ],
            0,
        )
        .unwrap();
        nord_format::to_bytes(&Entity::SampleProject(project)).unwrap()
    }

    /// Every edit lands under the id the snapshot shows, and the result still
    /// decodes and round-trips.
    #[test]
    fn edits_land_by_id_and_round_trip() {
        let bytes = project_bytes();
        let out = apply(
            &bytes,
            &[
                ("name".into(), "Vibes".into()),
                ("zone129.root_key".into(), "C2".into()),
                ("file1.path".into(), "verylow.wav".into()),
            ],
        )
        .unwrap();

        let entity = nord_format::from_stream(&mut Cursor::new(&out)).unwrap();
        let snapshot = snapshot(&entity).unwrap().unwrap();
        assert_eq!(snapshot.name, "Vibes");
        let zone = snapshot.zones.iter().find(|z| z.id == 129).unwrap();
        assert_eq!(zone.root_key, 36);
        assert_eq!(snapshot.files[0].path, "verylow.wav");
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), out);
    }

    #[test]
    fn stroke_and_velocity_edits_land_and_read_back() {
        let bytes = project_bytes();
        let out = apply(
            &bytes,
            &[
                ("stroke1.loop_enabled".into(), "on".into()),
                ("stroke1.loop_start".into(), "1000".into()),
                ("stroke1.loop_length".into(), "500".into()),
                ("stroke1.gain".into(), "0.25".into()),
                ("stroke1.velocity_max".into(), "90".into()),
                ("velocity.attack_amount".into(), "64".into()),
            ],
        )
        .unwrap();

        let entity = nord_format::from_stream(&mut Cursor::new(&out)).unwrap();
        let snapshot = snapshot(&entity).unwrap().unwrap();
        let stroke = snapshot.strokes.iter().find(|s| s.id == 1).unwrap();
        assert!(stroke.loop_enabled);
        assert_eq!((stroke.loop_start, stroke.loop_length), (1000.0, 500.0));
        assert_eq!(stroke.gain, 0.25);
        assert_eq!(stroke.velocity, (0, 90));
        assert_eq!(snapshot.velocity.attack_amount, 64);
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), out);
    }

    /// A bad path or an unknown id is refused before anything is encoded.
    #[test]
    fn unknown_paths_are_refused() {
        let bytes = project_bytes();
        for (path, value) in [
            ("zone999.root_key", "C4"),
            ("zone129.detune", "1"),
            ("file9.path", "x.wav"),
            ("file1.rate", "48000"),
            ("stroke1.nope", "1"),
            ("stroke9.gain", "1"),
            ("stroke1.velocity_max", "200"),
            ("stroke1.loop_start", "-1"),
            ("velocity.nope", "1"),
        ] {
            assert!(
                apply(&bytes, &[(path.into(), value.into())]).is_err(),
                "{path}"
            );
        }
    }

    /// An inverted key range cannot leave half an edit behind.
    #[test]
    fn an_inverted_range_is_refused_whole() {
        let bytes = project_bytes();
        let err = apply(&bytes, &[("zone129.bottom_note".into(), "C8".into())]);
        assert!(err.is_err());
    }
}
