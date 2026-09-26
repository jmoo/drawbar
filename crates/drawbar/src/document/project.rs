//! The Sample Editor project document.
//!
//! A `.nsmpproj` is the text file the editor saves and generates an `nsmp`
//! from — the same object as an instrument, seen from the source side, so it
//! wears the same key map, zone rows and velocity field. What is settable is
//! what `nord-format` can edit in place: the instrument's name and velocity
//! defaults, each zone's root key and key range, each stroke's trim, loop,
//! gain and velocity window, and each audio file's path. Paths are the CLI's —
//! `name`, `zone129.root_key`, `stroke1.loop_start`, `velocity.attack_amount`,
//! `file1.path` — with zones and files addressed by their stored ids and a
//! stroke by its global id.

use std::collections::HashMap;
use std::io::Cursor;
use std::ops::RangeInclusive;

use eframe::egui;
use nord_format::formats::nsmpproj::{
    Project, StrokeField, VelocityDefaults, HIGHEST_NOTE, LOWEST_NOTE, MAX_VELOCITY,
};
use nord_format::note;
use nord_format::Entity;

use super::capability::{Fact, Offset, Row, State as Cap};
use super::controls::{self, Sets};
use super::keys;
use super::sample::{self, note_picker, MapAct, MapZone, RowSpec, Sounds, State, VelocityAsk};
use super::table::PAD;
use crate::app;
use crate::midi::Played;

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
    /// The stroke this zone plays, where one of them is switched on.
    ///
    /// ⚠️ A zone plays one stroke: a project may hold more, but only the enabled one is
    /// written into an instrument, and a zone with none switched on plays nothing.
    ///
    /// Inferred from specimens; not confirmed on hardware.
    pub played: Option<u32>,
}

/// One audio file, under the id the strokes reference it by.
#[derive(Clone, PartialEq, Eq)]
pub struct AudioFile {
    pub id: u32,
    pub path: String,
    pub rate: u32,
}

/// One stroke, under the global id both blocks naming it use.
#[derive(Clone, PartialEq)]
pub struct Stroke {
    pub id: u32,
    /// The audio file it plays.
    pub file: u32,
    pub start: f64,
    pub stop: f64,
    pub gain: f64,
    /// `velocity_min..=velocity_max`.
    pub velocity: (u8, u8),
    pub loop_enabled: bool,
    pub loop_start: f64,
    pub loop_length: f64,
    pub crossfade: f64,
}

/// Everything settable, in one read.
#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub zones: Vec<Zone>,
    pub files: Vec<AudioFile>,
    pub strokes: Vec<Stroke>,
    pub velocity: VelocityDefaults,
    /// What the editor that wrote the file calls itself.
    pub created_by: (String, String),
    pub version: u32,
}

pub fn snapshot(entity: &Entity) -> Option<Result<Snapshot, String>> {
    let project = project(entity)?;
    Some(read(project))
}

fn read(project: &Project) -> Result<Snapshot, String> {
    // Gain and the velocity window sit in the `map_stroke`, the trim and loop
    // points in the `common_stroke`; the global id is what joins them.
    let zones = project.zones().map_err(|e| e.to_string())?;
    let played: Vec<_> = zones.iter().flat_map(|z| z.strokes.clone()).collect();
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
                file: s.file_id,
                start: s.start,
                stop: s.stop,
                gain: map.gain,
                velocity: map.velocity,
                loop_enabled: s.loop_enabled,
                loop_start: s.loop_start,
                loop_length: s.loop_length,
                crossfade: s.loop_crossfade,
            })
        })
        .collect::<Result<_, String>>()?;

    Ok(Snapshot {
        name: project.name().map_err(|e| e.to_string())?,
        strokes,
        velocity: project.velocity_defaults().map_err(|e| e.to_string())?,
        created_by: project.created_by().map_err(|e| e.to_string())?,
        version: project.file_format_version().map_err(|e| e.to_string())?,
        zones: zones
            .iter()
            .map(|z| Zone {
                id: z.zone_id,
                root_key: z.root_key,
                bottom_note: z.bottom_note,
                top_note: z.top_note,
                enabled: z.enabled,
                played: z.strokes.iter().find(|s| s.enabled).map(|s| s.global_id),
            })
            .collect(),
        files: project
            .audio_files()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|f| AudioFile {
                id: f.id,
                path: f.path,
                rate: f.sample_rate,
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
///
/// `decimals` caps what the drag can land on: egui aims at the roundest value within a
/// pointer's width of where the drag reached, and at a fractional display scale that is
/// a half — which a field holding a whole number refuses.
fn drag<H: std::hash::Hash>(
    ui: &mut egui::Ui,
    id: (&str, H),
    value: f64,
    range: RangeInclusive<f64>,
    speed: f64,
    decimals: Option<usize>,
) -> Option<String> {
    let mut moved = value;
    let response = ui.push_id(id, |ui| {
        ui.add(
            egui::DragValue::new(&mut moved)
                .range(range)
                .speed(speed)
                .max_decimals_opt(decimals),
        )
    });
    (response.inner.changed() && moved != value).then(|| moved.to_string())
}

/// A frame position. Nothing in the format caps one: the editor repairs a
/// position past the file's end on load.
///
/// Inferred from specimens; not confirmed on hardware.
fn frames(ui: &mut egui::Ui, id: (&str, u32), value: f64) -> Option<String> {
    drag(ui, id, value, 0.0..=f64::MAX, 1.0, None)
}

/// A velocity end, which the `map_stroke` holds as a `u8`.
fn whole(ui: &mut egui::Ui, id: (&str, u32), value: f64, top: f64) -> Option<String> {
    drag(ui, id, value, 0.0..=top, 0.5, Some(0))
}

/// How far each end of the trim may be dragged: never past the other, because a zone
/// whose trim-in is at or past its trim-out plays nothing and is refused at build.
///
/// A trim a file already states inverted is still inside its own control, so it can be
/// dragged back rather than left where nothing can reach it.
fn trim_ends(start: f64, stop: f64) -> (RangeInclusive<f64>, RangeInclusive<f64>) {
    (
        0.0..=(stop - 1.0).max(start),
        (start + 1.0).min(stop)..=f64::MAX,
    )
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

/// The keyboard a project's zones are laid out over, which the format states.
const SPAN: keys::Span = keys::Span {
    low: LOWEST_NOTE,
    high: HIGHEST_NOTE,
};

/// The pinned key map over a project's zones.
pub fn map(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &Snapshot,
    sets: &mut Sets,
    played: &Played,
) {
    let zones = map_zones(snapshot);
    let acts = sample::key_map(
        ui,
        state,
        &zones,
        SPAN,
        keys::Edges::Both,
        Sounds::NotUntilBuilt,
        played,
    );
    for act in acts {
        // A project has nothing to sound, so a struck key is a reading and never a write.
        if let MapAct::Bounds(bounds) = act {
            sets.extend(moved(&snapshot.zones, &bounds));
        }
    }
}

/// What a moved band writes. The map draws only the zones that answer, so its bands are
/// those zones in their own order — a switched-off zone is neither drawn nor moved.
fn moved(zones: &[Zone], bounds: &[(u8, u8)]) -> Sets {
    let mut sets = Sets::new();
    let answering = zones.iter().filter(|zone| zone.enabled);
    for (zone, (low, top)) in answering.zip(bounds) {
        if *top != zone.top_note {
            sets.push((format!("zone{}.top_note", zone.id), note::name(*top)));
        }
        if *low != zone.bottom_note {
            sets.push((format!("zone{}.bottom_note", zone.id), note::name(*low)));
        }
    }
    sets
}

/// The zones as the map and the velocity field state them.
///
/// A zone the project has switched off answers nothing, so it is not in the map at all:
/// a band over keys nothing plays is the one thing the map must not draw.
fn map_zones(snapshot: &Snapshot) -> Vec<MapZone> {
    snapshot
        .zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| zone.enabled)
        .map(|(row, zone)| MapZone {
            row,
            low: zone.bottom_note,
            top: zone.top_note,
            root: zone.root_key,
            name: format!("Zone {}", zone.id),
            velocity: played(snapshot, zone).map(|stroke| stroke.velocity),
        })
        .collect()
}

/// The stroke a zone plays.
fn played<'a>(snapshot: &'a Snapshot, zone: &Zone) -> Option<&'a Stroke> {
    let id = zone.played?;
    snapshot.strokes.iter().find(|stroke| stroke.id == id)
}

/// The file a stroke plays.
fn source<'a>(snapshot: &'a Snapshot, stroke: &Stroke) -> Option<&'a AudioFile> {
    snapshot.files.iter().find(|file| file.id == stroke.file)
}

/// The strokes, the velocity windows over them, and the instrument's own parameters.
///
/// The name is the header's: every document's name is edited in the one place.
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &Snapshot,
    paths: &mut HashMap<u32, String>,
    sets: &mut Sets,
) {
    velocity(ui, state, snapshot, sets);

    let quiet = app::caption(ui.visuals());
    controls::heading(
        ui,
        "Source strokes",
        "click a row for its fields",
        Some((&format!("{} zones", snapshot.zones.len()), quiet)),
    );
    let specs: Vec<RowSpec> = snapshot
        .zones
        .iter()
        .map(|zone| {
            let stroke = played(snapshot, zone);
            RowSpec {
                name: format!("Zone {}", zone.id),
                answers: format!(
                    "{} – {}",
                    note::name(zone.bottom_note),
                    note::name(zone.top_note)
                ),
                facts: facts_of(snapshot, zone, stroke),
                size: stroke
                    .map(|stroke| length(snapshot, stroke))
                    .unwrap_or_default(),
                hint: match zone.enabled {
                    true => format!("root {}", note::name(zone.root_key)),
                    false => "switched off — it answers nothing".to_string(),
                },
            }
        })
        .collect();
    sample::rows(ui, state, ("Zone", "Source"), &specs, |ui, index| {
        fields(ui, snapshot, index, paths, sets)
    });

    parameters(ui, snapshot, sets);
}

/// The middle column of a zone's row: the file it plays, and how it plays it.
fn facts_of(snapshot: &Snapshot, zone: &Zone, stroke: Option<&Stroke>) -> String {
    let mut parts = Vec::new();
    if !zone.enabled {
        parts.push("switched off".to_string());
    }
    let Some(stroke) = stroke else {
        parts.push("no stroke".to_string());
        return parts.join(" · ");
    };
    if let Some(file) = source(snapshot, stroke) {
        parts.push(leaf(&file.path).to_string());
    }
    parts.push(sample::decibels(20.0 * stroke.gain.log10()));
    parts.push(format!("vel {}–{}", stroke.velocity.0, stroke.velocity.1));
    parts.join(" · ")
}

/// A path as the row prints it: the file, not the folders above it.
fn leaf(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A stroke's trimmed length, in the seconds its own file's rate makes of it.
///
/// A project's audio is on disk rather than in the file, so there are no bytes to
/// measure — the trim is what a zone costs.
fn length(snapshot: &Snapshot, stroke: &Stroke) -> String {
    let frames = stroke.stop - stroke.start;
    if frames < 0.0 {
        return "inverted trim".to_string();
    }
    match source(snapshot, stroke).map(|file| file.rate) {
        Some(rate) if rate > 0 => format!("{:.3} s", frames / f64::from(rate)),
        _ => format!("{frames:.0} fr"),
    }
}

/// The fields of one open zone.
fn fields(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    index: usize,
    paths: &mut HashMap<u32, String>,
    sets: &mut Sets,
) {
    let zone = &snapshot.zones[index];
    let id = zone.id;
    sample::strip(ui, |ui| {
        sample::cell(ui, "Root key", 72.0, |ui| {
            if let Some(note) = note_picker(ui, ("proj_root", id as usize), zone.root_key) {
                sets.push((format!("zone{id}.root_key"), note::name(note)));
            }
        });
        sample::cell(ui, "Top note", 72.0, |ui| {
            if let Some(note) = note_picker(ui, ("proj_top", id as usize), zone.top_note) {
                sets.push((format!("zone{id}.top_note"), note::name(note)));
            }
        });
        sample::cell(ui, "Bottom note", 72.0, |ui| {
            if let Some(note) = note_picker(ui, ("proj_btm", id as usize), zone.bottom_note) {
                sets.push((format!("zone{id}.bottom_note"), note::name(note)));
            }
        });
        let Some(stroke) = played(snapshot, zone) else {
            return;
        };
        let gid = stroke.id;
        let top = f64::from(MAX_VELOCITY);
        sample::cell(ui, "Velocity window", 112.0, |ui| {
            ui.horizontal(|ui| {
                if let Some(v) = whole(ui, ("proj_vmin", gid), stroke.velocity.0 as f64, top) {
                    sets.push((format!("stroke{gid}.velocity_min"), v));
                }
                if let Some(v) = whole(ui, ("proj_vmax", gid), stroke.velocity.1 as f64, top) {
                    sets.push((format!("stroke{gid}.velocity_max"), v));
                }
            });
        });
        sample::cell(ui, "Gain", 80.0, |ui| {
            if let Some(v) = drag(ui, ("proj_gain", gid), stroke.gain, 0.0..=16.0, 0.01, None) {
                sets.push((format!("stroke{gid}.gain"), v));
            }
        });
        let (into, out) = trim_ends(stroke.start, stroke.stop);
        sample::cell(ui, "Trim in → out", 150.0, |ui| {
            ui.horizontal(|ui| {
                if let Some(v) = drag(ui, ("proj_start", gid), stroke.start, into, 1.0, None) {
                    sets.push((format!("stroke{gid}.start"), v));
                }
                if let Some(v) = drag(ui, ("proj_stop", gid), stroke.stop, out, 1.0, None) {
                    sets.push((format!("stroke{gid}.stop"), v));
                }
            });
        });
        sample::cell(ui, "Loop in → length", 170.0, |ui| {
            ui.horizontal(|ui| {
                let mut on = stroke.loop_enabled;
                let toggled = ui
                    .push_id(("proj_loop", gid), |ui| ui.checkbox(&mut on, ""))
                    .inner;
                if toggled.changed() {
                    sets.push((format!("stroke{gid}.loop_enabled"), on.to_string()));
                }
                if let Some(v) = frames(ui, ("proj_loop_start", gid), stroke.loop_start) {
                    sets.push((format!("stroke{gid}.loop_start"), v));
                }
                if let Some(v) = frames(ui, ("proj_loop_len", gid), stroke.loop_length) {
                    sets.push((format!("stroke{gid}.loop_length"), v));
                }
            });
        });
        sample::cell(ui, "Crossfade", 90.0, |ui| {
            if let Some(v) = frames(ui, ("proj_xfade", gid), stroke.crossfade) {
                sets.push((format!("stroke{gid}.loop_crossfade"), v));
            }
        });
        if let Some(file) = source(snapshot, stroke) {
            let held = paths.entry(file.id).or_insert_with(|| file.path.clone());
            sample::cell(ui, "Source file", 280.0, |ui| {
                // ⚠️ Written when the box is left, which an Enter typed into it does:
                // an Enter pressed anywhere else is not this box being finished with.
                let response = ui.add(egui::TextEdit::singleline(held).desired_width(270.0));
                if response.lost_focus() && *held != file.path {
                    sets.push((format!("file{}.path", file.id), held.clone()));
                }
            });
        }
    });
}

/// The key × velocity field: one window per zone, and every edge draggable, because a
/// project is where a window is stated.
fn velocity(ui: &mut egui::Ui, state: &mut State, snapshot: &Snapshot, sets: &mut Sets) {
    let playing: Vec<(usize, &Zone, &Stroke)> = snapshot
        .zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| zone.enabled)
        .filter_map(|(row, zone)| played(snapshot, zone).map(|stroke| (row, zone, stroke)))
        .collect();
    if playing.is_empty() {
        return;
    }
    let blocks: Vec<keys::VelBlock> = playing
        .iter()
        .map(|(_, zone, stroke)| keys::VelBlock {
            low: zone.bottom_note,
            top: zone.top_note,
            window: stroke.velocity,
            name: format!("Zone {}", zone.id),
            hint: format!(
                "Zone {} answers {}–{} at velocity {}–{} · drag an edge to move the window",
                zone.id,
                note::name(zone.bottom_note),
                note::name(zone.top_note),
                stroke.velocity.0,
                stroke.velocity.1
            ),
        })
        .collect();
    let rows: Vec<usize> = playing.iter().map(|(row, _, _)| *row).collect();
    let asked = sample::velocity_field(
        ui,
        "one window per zone — drag the top or bottom edge",
        SPAN,
        &blocks,
        &rows,
        keys::Handles::Draggable,
        sample::selected(state),
    );
    match asked {
        Some(VelocityAsk::Open(row)) => sample::pick_row(state, row),
        Some(VelocityAsk::Window { block, window }) => {
            let stroke = playing[block].2;
            if window.0 != stroke.velocity.0 {
                sets.push((
                    format!("stroke{}.velocity_min", stroke.id),
                    window.0.to_string(),
                ));
            }
            if window.1 != stroke.velocity.1 {
                sets.push((
                    format!("stroke{}.velocity_max", stroke.id),
                    window.1.to_string(),
                ));
            }
        }
        None => {}
    }
}

/// The instrument's own parameters. A project is the source, so these are the only
/// sound parameters the editors can write.
fn parameters(ui: &mut egui::Ui, snapshot: &Snapshot, sets: &mut Sets) {
    controls::heading(
        ui,
        "Sound parameters",
        "all samplib_attrs — the project is the source",
        None,
    );
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        sample::strip(ui, |ui| {
            for (label, field, value) in [
                ("Attack", "attack_amount", snapshot.velocity.attack_amount),
                (
                    "Velocity → amplitude",
                    "amplitude",
                    snapshot.velocity.amplitude,
                ),
                ("Velocity → timbre", "timbre", snapshot.velocity.timbre),
            ] {
                sample::cell(ui, label, 132.0, |ui| {
                    if let Some(moved) = crate::knob::ui(ui, field, i64::from(value), 0, 255) {
                        sets.push((format!("velocity.{field}"), moved));
                    }
                });
            }
        });
    });
    ui.add_space(8.0);
}

/// What the file says about itself.
pub fn metadata(ui: &mut egui::Ui, snapshot: &Snapshot) {
    controls::heading(
        ui,
        "About this file",
        "what the file says about itself — read here, never written differently",
        None,
    );
    let (product, version) = &snapshot.created_by;
    super::capability::facts(
        ui,
        &[
            Fact {
                key: "Format",
                value: "project".to_string(),
                note: "drawbar's own file, and the editor's",
            },
            Fact {
                key: "File format version",
                value: snapshot.version.to_string(),
                note: "what decides which blocks this reader expects",
            },
            Fact {
                key: "Project tree",
                value: format!(
                    "{} strokes · {} WAVs",
                    snapshot.strokes.len(),
                    snapshot.files.len()
                ),
                note: "the source, so nothing here is baked",
            },
            Fact {
                key: "Written by",
                value: format!("{product} {version}"),
                note: "whatever last saved it",
            },
            Fact {
                key: "Build target",
                value: "nsmp".to_string(),
                note: "the codec is understood, but building is not implemented",
            },
        ],
    );
}

/// The nineteen capabilities of the instrument editor, as a project stands in them.
///
/// A project is the source: what an instrument has baked in, this states outright.
pub fn capabilities() -> Vec<Row> {
    let row = |name: &'static str, state: Cap, note: &'static str| Row { name, state, note };
    vec![
        row("name", Cap::Editable, "the project's own name"),
        row(
            "category / sub",
            Cap::Absent,
            "no category block this reader decodes",
        ),
        row(
            "key zones: root / top / low",
            Cap::Editable,
            "each map_zone states all three",
        ),
        row(
            "velocity layers",
            Cap::Editable,
            "a window per stroke, in its map_stroke",
        ),
        row(
            "per-zone gain / detune",
            Cap::Editable,
            "m_gain; the detune beside it has no setter",
        ),
        row(
            "per-key table",
            Cap::Absent,
            "a project states zones, not keys",
        ),
        row(
            "instrument gain",
            Cap::ReadOnly,
            "m_mapGain, which nothing here writes",
        ),
        row(
            "loop points / crossfade",
            Cap::Editable,
            "the loop, its length and its crossfade, in frames",
        ),
        row(
            "loop decay / detune",
            Cap::ReadOnly,
            "m_loopDecay, which no control on this face writes — nord-cli does",
        ),
        row("release samples", Cap::Absent, "a piano library's bank 2"),
        row(
            "pedal resonance samples",
            Cap::Absent,
            "a piano library's bank 1",
        ),
        row(
            "sound parameters",
            Cap::Editable,
            "the samplib_attrs velocity defaults",
        ),
        row(
            "stereo / channels",
            Cap::ReadOnly,
            "the WAV's own, which is not read here",
        ),
        row(
            "replace / add a stroke",
            Cap::Editable,
            "the source path; the project is the source",
        ),
        row(
            "cut / move / drop strokes",
            Cap::ReadOnly,
            "the zones are read; nothing here removes one",
        ),
        row(
            "decode / audition",
            Cap::NeedsEncode,
            "a project is built into an instrument before it plays",
        ),
        row("size trim", Cap::Absent, "not a thing a project has"),
        row(
            "write to the instrument",
            Cap::Absent,
            "build into an nsmp first",
        ),
        row(
            "byte-exact round trip",
            Cap::Verified,
            "the text is rewritten as it was read",
        ),
    ]
}

/// Where each field the Basic face writes lands in the file.
///
/// A project is text, so these are the keys `nord_format` writes rather than offsets.
pub fn offsets(snapshot: &Snapshot) -> Vec<Offset> {
    vec![
        Offset {
            at: "m_name".to_string(),
            holds: snapshot.name.clone(),
            note: "the instrument's name, in the root block",
        },
        Offset {
            at: "map_zone".to_string(),
            holds: format!("{} blocks", snapshot.zones.len()),
            note: "root key, key range, and one map_stroke per stroke",
        },
        Offset {
            at: "common_stroke".to_string(),
            holds: format!("{} blocks", snapshot.strokes.len()),
            note: "the trim and the loop, in file frames",
        },
        Offset {
            at: "audio_file".to_string(),
            holds: format!("{} blocks", snapshot.files.len()),
            note: "m_fullName and m_sampleRate per WAV",
        },
        Offset {
            at: "samplib_attrs".to_string(),
            holds: "3 velocity fields".to_string(),
            note: "m_atkVelocityAmount, m_velAmpl, m_velTimbre",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::nsmpproj::NewZone;

    fn project_bytes() -> Vec<u8> {
        project_of(&[48, 72])
    }

    /// A project of one zone per root key, each playing a WAV of its own — what the
    /// editor writes for a new instrument.
    fn project_of(roots: &[u8]) -> Vec<u8> {
        let zones: Vec<NewZone> = roots
            .iter()
            .map(|root| NewZone {
                path: format!("root{root}.wav"),
                sample_rate: 44100,
                frames: 44100,
                root_key: *root,
            })
            .collect();
        let project = Project::new("Marimba", &zones, 0).unwrap();
        nord_format::to_bytes(&Entity::SampleProject(project)).unwrap()
    }

    fn read_back(bytes: &[u8]) -> Snapshot {
        let entity = nord_format::from_stream(&mut Cursor::new(bytes)).unwrap();
        snapshot(&entity).unwrap().unwrap()
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
                ("stroke1.loop_crossfade".into(), "240".into()),
                ("stroke1.gain".into(), "0.25".into()),
                ("stroke1.velocity_max".into(), "90".into()),
                ("velocity.attack_amount".into(), "64".into()),
            ],
        )
        .unwrap();

        let snapshot = read_back(&out);
        let stroke = snapshot.strokes.iter().find(|s| s.id == 1).unwrap();
        assert!(stroke.loop_enabled);
        assert_eq!((stroke.loop_start, stroke.loop_length), (1000.0, 500.0));
        assert_eq!(stroke.crossfade, 240.0);
        assert_eq!(stroke.gain, 0.25);
        assert_eq!(stroke.velocity, (0, 90));
        assert_eq!(snapshot.velocity.attack_amount, 64);
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

    /// The map draws the zones that answer keys, each with the window of the stroke it
    /// plays — and a zone the project switched off is not one of them.
    #[test]
    fn the_map_shows_the_zones_that_answer_a_key() {
        let snapshot = read_back(&project_bytes());
        let zones = map_zones(&snapshot);
        assert_eq!(zones.len(), snapshot.zones.len());
        for (zone, shown) in snapshot.zones.iter().zip(&zones) {
            assert_eq!((shown.low, shown.top), (zone.bottom_note, zone.top_note));
            assert_eq!(shown.root, zone.root_key);
            assert!(shown.velocity.is_some(), "each zone plays one stroke");
        }
        assert_eq!(SPAN.low, LOWEST_NOTE);
        assert_eq!(SPAN.high, HIGHEST_NOTE);
    }

    /// A context dressed as the app dresses it: the semibold family a band is set in is
    /// not bound by default, and laying one out without it panics.
    fn dressed() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        ctx.set_visuals(egui::Visuals::dark());
        ctx
    }

    /// One frame of the pinned map over `snapshot`: what it painted and where, and what
    /// it wrote.
    fn mapped(
        ctx: &egui::Context,
        state: &mut State,
        snapshot: &Snapshot,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Rect)>, Sets) {
        let mut sets = Sets::new();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 400.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |ui| {
                map(ui, state, snapshot, &mut sets, &Played::default());
            });
        });
        let mut said = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut said);
        }
        (said, sets)
    }

    fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => into.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
            _ => {}
        }
    }

    fn press(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// One frame of the body over `snapshot`: what it painted and where, and what it
    /// wrote.
    fn bodied(
        ctx: &egui::Context,
        state: &mut State,
        snapshot: &Snapshot,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Rect)>, Sets) {
        let mut sets = Sets::new();
        let mut paths = HashMap::new();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |page| {
                ui(page, state, snapshot, &mut paths, &mut sets);
            });
        });
        let mut said = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut said);
        }
        (said, sets)
    }

    /// The highest place a word was painted: the velocity field stands above the rows,
    /// and both name a zone the same way.
    fn highest(said: &[(String, egui::Rect)], word: &str) -> egui::Rect {
        said.iter()
            .filter(|(text, _)| text == word)
            .map(|(_, at)| *at)
            .reduce(|a, b| match a.center().y < b.center().y {
                true => a,
                false => b,
            })
            .unwrap_or_else(|| panic!("{word} was never painted: {said:?}"))
    }

    /// One frame of the velocity-min control on its own, and what it spelled.
    fn velocity_box(ctx: &egui::Context, events: Vec<egui::Event>) -> Option<String> {
        let mut spelled = None;
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(200.0, 60.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |ui| {
                spelled = whole(ui, ("proj_vmin", 1), 0.0, f64::from(MAX_VELOCITY));
            });
        });
        spelled
    }

    /// ⚠️ A velocity end is a `u8`: egui aims at the roundest value within a pointer's
    /// width of where a drag reached, and at a fractional display scale that is a half
    /// the field refuses. Every value the control spells has to be one the format takes.
    #[test]
    fn a_dragged_velocity_end_is_spelled_as_a_whole_number() {
        let ctx = dressed();
        ctx.set_pixels_per_point(1.5);
        let at = egui::pos2(30.0, 20.0);
        velocity_box(&ctx, Vec::new());
        velocity_box(
            &ctx,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let mut spelled = None;
        for step in [10.0, 3.0, 3.0] {
            let to = egui::pos2(at.x + step, at.y);
            spelled = velocity_box(&ctx, vec![egui::Event::PointerMoved(to)]).or(spelled);
        }

        let spelled = spelled.expect("the drag moved the control");
        assert!(
            spelled.parse::<u8>().is_ok(),
            "the control spelled {spelled:?}, which is not a velocity"
        );
        assert!(
            apply(
                &project_bytes(),
                &[("stroke1.velocity_min".into(), spelled.clone())]
            )
            .is_ok(),
            "and the format takes {spelled:?}"
        );
    }

    /// ⚠️ A block is not a row either: the velocity field draws one block per zone that
    /// answers, so a click on the last block must open the last row.
    #[test]
    fn clicking_a_velocity_block_opens_the_row_it_stands_for() {
        let ctx = dressed();
        let mut snapshot = read_back(&project_of(&[48, 60, 72]));
        snapshot.zones[1].enabled = false;
        let bottom = snapshot.zones[2].id;

        let mut state = State::default();
        let (said, _) = bodied(&ctx, &mut state, &snapshot, Vec::new());
        let block = highest(&said, &format!("Zone {bottom}"));
        // Below the block's name, which sits over the handle at the window's top edge.
        let at = egui::pos2(block.center().x, block.center().y + 20.0);

        let (_, sets) = bodied(&ctx, &mut state, &snapshot, press(at));
        assert_eq!(
            sample::selected(&state),
            Some(2),
            "the second block stands on the third zone"
        );
        assert!(sets.is_empty(), "a pick is not an edit");
    }

    /// ⚠️ A band is not a row: the map draws only the zones that answer a key, so a
    /// click on the last band must open the last row even with a zone switched off
    /// between them.
    #[test]
    fn clicking_a_band_opens_the_row_it_stands_for() {
        let ctx = dressed();
        let mut snapshot = read_back(&project_of(&[48, 60, 72]));
        snapshot.zones[1].enabled = false;
        let bottom = snapshot.zones[2].id;

        let mut state = State::default();
        let (said, _) = mapped(&ctx, &mut state, &snapshot, Vec::new());
        assert_eq!(sample::selected(&state), None);
        let band = said
            .iter()
            .find(|(text, _)| *text == format!("Zone {bottom}"))
            .unwrap_or_else(|| panic!("the bottom zone's band was never painted: {said:?}"))
            .1;

        let (_, sets) = mapped(&ctx, &mut state, &snapshot, press(band.center()));
        assert_eq!(
            sample::selected(&state),
            Some(2),
            "the second band stands on the third zone"
        );
        assert!(sets.is_empty(), "a pick is not an edit");
    }

    /// A zone's row reads the file it plays and how long the trim leaves it.
    #[test]
    fn a_row_reads_the_source_it_plays() {
        let snapshot = read_back(&project_bytes());
        let zone = &snapshot.zones[0];
        let stroke = played(&snapshot, zone).expect("a stroke");
        let file = source(&snapshot, stroke).expect("the stroke names a file");
        assert_eq!(file.rate, 44100);
        assert_eq!(length(&snapshot, stroke), "1.000 s");
        let facts = facts_of(&snapshot, zone, Some(stroke));
        assert!(facts.contains(&file.path), "{facts}");
        assert!(facts.contains("vel 0–127"), "{facts}");
        assert_eq!(leaf("/Users/x/Nord/low.wav"), "low.wav");
    }

    /// One frame of one zone's open fields, with `paths` as the boxes hold them.
    fn opened(
        ctx: &egui::Context,
        snapshot: &Snapshot,
        paths: &mut HashMap<u32, String>,
        events: Vec<egui::Event>,
    ) -> Sets {
        let mut sets = Sets::new();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 400.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |ui| {
                fields(ui, snapshot, 0, paths, &mut sets);
            });
        });
        sets
    }

    /// ⚠️ The source box writes what it holds when it is left, and an Enter pressed
    /// somewhere else is not that: a key landing in another control must not commit a
    /// path nobody has finished typing.
    #[test]
    fn a_half_typed_source_path_waits_for_the_box_to_be_left() {
        let ctx = dressed();
        let snapshot = read_back(&project_bytes());
        let stroke = played(&snapshot, &snapshot.zones[0]).expect("a stroke");
        let file = source(&snapshot, stroke).expect("it names a file").id;
        let mut paths = HashMap::from([(file, "half typed".to_string())]);

        assert!(opened(&ctx, &snapshot, &mut paths, Vec::new()).is_empty());
        let sets = opened(
            &ctx,
            &snapshot,
            &mut paths,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            sets.is_empty(),
            "an Enter with the box unfocused wrote {sets:?}"
        );
    }

    /// The same project with the one stroke of its first zone switched off, which the
    /// format allows and nothing here writes.
    fn stroke_switched_off(bytes: &[u8]) -> Vec<u8> {
        const FLAG: &str = "m_isEnabled = 1";
        let text = String::from_utf8(bytes.to_vec()).expect("a project is text");
        let stroke = text.find("map_stroke {").expect("a map_stroke block");
        let flag = text[stroke..].find(FLAG).expect("its own flag") + stroke;
        let mut out = text;
        out.replace_range(flag..flag + FLAG.len(), "m_isEnabled = 0");
        out.into_bytes()
    }

    /// ⚠️ Only the enabled stroke is written into an instrument, so a zone with none
    /// switched on plays nothing: the row says so rather than reading a stroke that
    /// would never be built.
    #[test]
    fn a_zone_whose_stroke_is_switched_off_plays_nothing() {
        let snapshot = read_back(&stroke_switched_off(&project_bytes()));
        let zone = &snapshot.zones[0];
        assert_eq!(zone.played, None);
        assert!(played(&snapshot, zone).is_none());
        assert!(
            facts_of(&snapshot, zone, None).contains("no stroke"),
            "{}",
            facts_of(&snapshot, zone, None)
        );
        assert!(
            snapshot.zones[1].played.is_some(),
            "the other zone still plays its own"
        );
        // A band with no stroke states no velocity window, so it is not in the field.
        assert!(map_zones(&snapshot)[0].velocity.is_none());
    }

    /// ⚠️ A zone whose trim-in is at or past its trim-out plays nothing, and the build
    /// refuses it: neither end can be dragged onto the other, and a trim a file already
    /// states inverted is named rather than read as a zone of no length.
    #[test]
    fn an_inverted_trim_cannot_be_dragged_and_is_named_where_it_is_stated() {
        let (into, out) = trim_ends(0.0, 44100.0);
        assert_eq!(*into.end(), 44099.0, "the trim-in stops short of the out");
        assert_eq!(*out.start(), 1.0, "and the out stops short of the in");

        let (into, out) = trim_ends(900.0, 100.0);
        assert!(
            into.contains(&900.0) && out.contains(&100.0),
            "an inverted trim is still inside its own control, so it can be dragged back"
        );

        let snapshot = read_back(&project_bytes());
        let stroke = snapshot.strokes.first().expect("a stroke");
        assert_eq!(length(&snapshot, stroke), "1.000 s");
        let inverted = Stroke {
            start: stroke.stop,
            stop: stroke.start,
            ..stroke.clone()
        };
        assert_eq!(length(&snapshot, &inverted), "inverted trim");
    }

    /// A gain is read in the one unit both documents read it in, and a stroke turned
    /// all the way down says so rather than reading as the floor of a logarithm.
    #[test]
    fn a_stroke_with_no_gain_reads_as_silence() {
        let snapshot = read_back(&project_bytes());
        let zone = &snapshot.zones[0];
        let stroke = played(&snapshot, zone).expect("a stroke");
        assert_eq!(stroke.gain, 1.0);
        assert!(
            facts_of(&snapshot, zone, Some(stroke)).contains("+0.0 dB"),
            "unity gain is no change at all"
        );

        let silent = Stroke {
            gain: 0.0,
            ..stroke.clone()
        };
        let facts = facts_of(&snapshot, zone, Some(&silent));
        assert!(facts.contains("silent"), "{facts}");
    }

    /// A moved band writes both ends of the zone it moved, under the id the file gives
    /// it — and a zone the project switched off is not a band at all.
    #[test]
    fn a_moved_band_writes_the_zone_it_moved_by_id() {
        let mut snapshot = read_back(&project_bytes());
        let held: Vec<(u8, u8)> = snapshot
            .zones
            .iter()
            .map(|zone| (zone.bottom_note, zone.top_note))
            .collect();
        assert!(moved(&snapshot.zones, &held).is_empty(), "nothing moved");

        let first = snapshot.zones[0].id;
        let mut wider = held.clone();
        wider[0] = (held[0].0, held[0].1 + 1);
        assert_eq!(
            moved(&snapshot.zones, &wider),
            [(format!("zone{first}.top_note"), note::name(held[0].1 + 1))]
        );

        // A switched-off zone is skipped, so the bands line up with the zones that
        // answer rather than with the rows.
        snapshot.zones[0].enabled = false;
        let answering: Vec<(u8, u8)> = held[1..].to_vec();
        assert!(moved(&snapshot.zones, &answering).is_empty());
        let second = snapshot.zones[1].id;
        let mut wider = answering.clone();
        wider[0] = (answering[0].0, answering[0].1 + 1);
        assert_eq!(
            moved(&snapshot.zones, &wider)[0].0,
            format!("zone{second}.top_note")
        );
    }

    /// ⚠️ The capability table is a claim about this editor. Every row it calls
    /// editable has to name a path [`set`] accepts.
    #[test]
    fn every_editable_capability_names_a_path_the_editor_accepts() {
        let bytes = project_bytes();
        let paths = [
            ("name", "name", "Vibes"),
            ("key zones: root / top / low", "zone129.root_key", "C2"),
            ("velocity layers", "stroke1.velocity_max", "90"),
            ("per-zone gain / detune", "stroke1.gain", "0.25"),
            ("loop points / crossfade", "stroke1.loop_crossfade", "240"),
            ("sound parameters", "velocity.amplitude", "64"),
            ("replace / add a stroke", "file1.path", "other.wav"),
        ];
        for row in capabilities() {
            if row.state != Cap::Editable {
                continue;
            }
            let (_, path, value) = paths
                .iter()
                .find(|(name, _, _)| *name == row.name)
                .unwrap_or_else(|| panic!("{} is editable and unlisted", row.name));
            assert!(
                apply(&bytes, &[((*path).into(), (*value).into())]).is_ok(),
                "{}: {path} is not a path the editor accepts",
                row.name
            );
        }
    }
}
